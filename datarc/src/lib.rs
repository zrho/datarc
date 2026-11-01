#![deny(unsafe_op_in_unsafe_fn)]
#![doc = include_str!("../../README.md")]

extern crate self as datarc;

pub use datarc_macros::Packable;

/// Associates a borrowed record or an enum of shared pointers with its packed owner.
///
/// The derive generates standard `From` conversions. Generic callers can require
/// the conversion bounds they need in addition to this type association.
pub trait Packable {
    type Packed;
}

#[cfg(doctest)]
mod compile_checks;

/// An owning, thin pointer that can be stored in a packed enum.
///
/// Generated `Packable` struct owners implement this trait automatically.
///
/// # Safety
/// `into_raw` must transfer one ownership unit to a non-null pointer whose address
/// is divisible by 16, preserving provenance. `from_raw` must restore that unit
/// without changing reference counts. Round trips must preserve the value and
/// its drop behavior. Shared access to a temporarily restored owner must be valid
/// while the original ownership unit remains alive (as for `Arc`).
pub unsafe trait PackedPointer: Clone {
    fn into_raw(self) -> std::ptr::NonNull<()>;

    /// Restore an ownership unit previously transferred by `into_raw`.
    ///
    /// # Safety
    /// The pointer must come from this type's `into_raw`, with all tag bits
    /// removed, and its ownership unit must still be alive. The caller must
    /// either take over that unit or suppress destruction of the restored value
    /// and limit its use to shared access while the original unit stays alive.
    unsafe fn from_raw(ptr: std::ptr::NonNull<()>) -> Self;
}

/// Record construction failed validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildError {
    /// The input collection exceeds its field's length type.
    LengthOverflow {
        field: &'static str,
        len: usize,
        length_type: &'static str,
    },
    /// The combined allocation exceeds Rust's allocation layout limits.
    LayoutOverflow,
    /// An iterator yielded fewer or more items than its initially reported length.
    IteratorLengthMismatch {
        field: &'static str,
        expected: usize,
    },
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LengthOverflow {
                field,
                len,
                length_type,
            } => {
                write!(
                    f,
                    "field `{field}` has length {len}, which does not fit in {length_type}"
                )
            }
            Self::LayoutOverflow => f.write_str("record allocation layout is too large"),
            Self::IteratorLengthMismatch { field, expected } => write!(
                f,
                "iterator for field `{field}` did not yield its reported length of {expected}"
            ),
        }
    }
}

impl std::error::Error for BuildError {}

/// Implementation support for generated code; not a stable public API.
#[doc(hidden)]
pub mod __private {
    use std::alloc::{alloc, dealloc, handle_alloc_error, Layout};
    use std::marker::PhantomData;
    use std::ptr::{self, NonNull};
    use std::sync::atomic::{fence, AtomicUsize, Ordering};

    const STATIC_REFCOUNT: usize = usize::MAX;

    // Lifetime-dependent bounds on these helpers allow the derive to generate
    // conditional conversions even for concrete, non-Clone referent types.
    pub trait CloneValue {
        type Value;
        fn clone_value(self) -> Self::Value;
    }

    impl<T: Clone> CloneValue for &T {
        type Value = T;
        fn clone_value(self) -> T {
            T::clone(self)
        }
    }

    pub trait CloneItems {
        type Item;
        type Iter: ExactSizeIterator<Item = Self::Item>;
        fn clone_items(self) -> Self::Iter;
    }

    impl<'a, T: Clone> CloneItems for &'a [T] {
        type Item = T;
        type Iter = std::iter::Cloned<std::slice::Iter<'a, T>>;
        fn clone_items(self) -> Self::Iter {
            self.iter().cloned()
        }
    }

    /// Its atomic is private so safe code cannot corrupt a live reference count.
    #[repr(transparent)]
    pub struct RefCount(AtomicUsize);

    impl RefCount {
        pub fn new_static() -> Self {
            Self(AtomicUsize::new(STATIC_REFCOUNT))
        }
    }

    impl Default for RefCount {
        fn default() -> Self {
            Self(AtomicUsize::new(1))
        }
    }

    /// # Safety
    /// Implementations must return the same count and allocation layout for the
    /// lifetime of an allocation. An immortal count must never change to or from
    /// a counted value after publication. `drop_tail` must drop exactly the
    /// initialized trailing elements, without destroying the header or freeing
    /// the allocation.
    /// The header's Send/Sync and drop-check behavior must include all tail types.
    pub unsafe trait Storage: Sized {
        fn ref_count(&self) -> &RefCount;
        fn layout(&self) -> Layout;

        /// # Safety
        /// `ptr` must point to a live, exclusively owned allocation of this storage.
        unsafe fn drop_tail(ptr: *mut Self);
    }

    #[repr(transparent)]
    pub struct Shared<T: Storage> {
        ptr: NonNull<T>,
        marker: PhantomData<T>,
    }

    // SAFETY: sharing or transferring ownership requires every stored value to
    // support both transfer and shared access, just as for Arc<T>.
    unsafe impl<T: Storage + Send + Sync> Send for Shared<T> {}
    unsafe impl<T: Storage + Send + Sync> Sync for Shared<T> {}

    impl<T: Storage> Shared<T> {
        /// # Safety
        /// `ptr` must be fully initialized and transfer one live strong ownership
        /// unit. Counted storage must use a global allocation made with
        /// `T::layout()`; immortal storage must remain valid permanently.
        pub unsafe fn from_raw(ptr: NonNull<T>) -> Self {
            Self {
                ptr,
                marker: PhantomData,
            }
        }

        pub fn into_raw(self) -> NonNull<T> {
            let this = std::mem::ManuallyDrop::new(self);
            this.ptr
        }

        pub fn get(&self) -> &T {
            // SAFETY: every Shared owns a strong reference to initialized storage.
            unsafe { self.ptr.as_ref() }
        }

        pub fn as_ptr(&self) -> *mut T {
            self.ptr.as_ptr()
        }
    }

    impl<T: Storage> Clone for Shared<T> {
        fn clone(&self) -> Self {
            let count = &self.get().ref_count().0;
            // The sentinel is fixed before publication, so a relaxed load is
            // sufficient and immortal allocations never perform a count update.
            if count.load(Ordering::Relaxed) != STATIC_REFCOUNT {
                let old = count.fetch_add(1, Ordering::Relaxed);
                // Bound the count well below wraparound, including racing clones.
                if old >= isize::MAX as usize {
                    std::process::abort();
                }
            }
            Self {
                ptr: self.ptr,
                marker: PhantomData,
            }
        }
    }

    impl<T: Storage> Drop for Shared<T> {
        fn drop(&mut self) {
            let count = &self.get().ref_count().0;
            if count.load(Ordering::Relaxed) == STATIC_REFCOUNT {
                return;
            }
            if count.fetch_sub(1, Ordering::Release) != 1 {
                return;
            }
            fence(Ordering::Acquire);
            let allocation = Allocation {
                ptr: self.ptr,
                layout: self.get().layout(),
                deallocate: true,
            };
            // SAFETY: the last strong owner has exclusive access to the storage.
            // The guard drops the header and frees memory even if a tail panics.
            unsafe { T::drop_tail(self.ptr.as_ptr()) };
            drop(allocation);
        }
    }

    /// Owns fully initialized storage until construction publishes an owner.
    pub struct InitializationGuard<T: Storage> {
        allocation: Option<Allocation<T>>,
    }

    impl<T: Storage> InitializationGuard<T> {
        /// # Safety
        /// The pointer must own fully initialized storage with the given layout.
        pub unsafe fn new(ptr: NonNull<T>, layout: Layout, deallocate: bool) -> Self {
            Self {
                allocation: Some(unsafe { Allocation::new(ptr, layout, deallocate) }),
            }
        }

        pub fn disarm(mut self) {
            self.allocation
                .take()
                .expect("active initialization guard")
                .into_raw();
        }
    }

    impl<T: Storage> Drop for InitializationGuard<T> {
        fn drop(&mut self) {
            if let Some(allocation) = self.allocation.take() {
                // The allocation guard also cleans up the header if a tail panics.
                unsafe { T::drop_tail(allocation.ptr.as_ptr()) };
                drop(allocation);
            }
        }
    }

    pub struct Allocation<T> {
        ptr: NonNull<T>,
        layout: Layout,
        deallocate: bool,
    }

    impl<T> Allocation<T> {
        /// # Safety
        /// `ptr` must point to an initialized header in a live allocation of
        /// `layout`. When `deallocate` is true it must be a global allocation,
        /// and the guard takes ownership of freeing it. Otherwise the caller
        /// retains responsibility for the storage. The guard always owns header
        /// destruction; trailing values must be destroyed before it is dropped.
        pub unsafe fn new(ptr: NonNull<T>, layout: Layout, deallocate: bool) -> Self {
            Self {
                ptr,
                layout,
                deallocate,
            }
        }

        pub fn into_raw(self) -> NonNull<T> {
            let ptr = self.ptr;
            std::mem::forget(self);
            ptr
        }
    }

    impl<T> Drop for Allocation<T> {
        fn drop(&mut self) {
            struct Free(NonNull<u8>, Layout, bool);
            impl Drop for Free {
                fn drop(&mut self) {
                    if self.2 {
                        // SAFETY: this guard owns the global allocation and its exact layout.
                        unsafe { dealloc(self.0.as_ptr(), self.1) };
                    }
                }
            }
            let _free = Free(self.ptr.cast(), self.layout, self.deallocate);
            // SAFETY: the allocation header is initialized and uniquely owned.
            unsafe { ptr::drop_in_place(self.ptr.as_ptr()) };
        }
    }

    /// # Safety
    /// The pointer must designate an initialized slice whose destruction is
    /// exclusively owned by this guard and whose storage outlives the guard.
    pub struct TailDrop<T>(*mut [T]);

    impl<T> TailDrop<T> {
        /// # Safety
        /// See the ownership requirements on `TailDrop`.
        pub unsafe fn new(ptr: *mut [T]) -> Self {
            Self(ptr)
        }
    }

    impl<T> Drop for TailDrop<T> {
        fn drop(&mut self) {
            // SAFETY: the creator transfers exclusive destruction rights.
            unsafe { ptr::drop_in_place(self.0) };
        }
    }

    /// Owns the initialized prefix of one array during construction.
    pub struct TailInit<T> {
        ptr: *mut T,
        capacity: usize,
        initialized: usize,
    }

    impl<T> TailInit<T> {
        /// # Safety
        /// `ptr` must be aligned and designate space for `capacity` elements in
        /// a live allocation. This guard takes exclusive initialization and
        /// destruction rights for that region, which must outlive the guard.
        pub unsafe fn new(ptr: *mut T, capacity: usize) -> Self {
            Self {
                ptr,
                capacity,
                initialized: 0,
            }
        }

        pub fn fill<I: Iterator<Item = T>>(
            &mut self,
            mut iter: I,
            field: &'static str,
        ) -> Result<(), crate::BuildError> {
            let mismatch = crate::BuildError::IteratorLengthMismatch {
                field,
                expected: self.capacity,
            };
            while self.initialized < self.capacity {
                let value = iter.next().ok_or_else(|| mismatch.clone())?;
                // SAFETY: the explicit bound above prevents trusting an
                // iterator's length report; this slot is not initialized yet.
                unsafe { self.ptr.add(self.initialized).write(value) };
                self.initialized += 1;
            }
            // Observe at most one excess item, including for infinite iterators
            // or a usize::MAX capacity of zero-sized elements.
            if iter.next().is_some() {
                return Err(mismatch);
            }
            Ok(())
        }
    }

    impl<T> Drop for TailInit<T> {
        fn drop(&mut self) {
            // SAFETY: only the prefix counted after successful writes is live.
            unsafe {
                ptr::drop_in_place(ptr::slice_from_raw_parts_mut(self.ptr, self.initialized))
            };
        }
    }

    pub fn allocate(layout: Layout) -> NonNull<u8> {
        // Generated storage always includes a nonzero-sized reference count.
        assert!(layout.size() > 0);
        // SAFETY: Layout already enforces the allocator's size/alignment rules.
        NonNull::new(unsafe { alloc(layout) }).unwrap_or_else(|| handle_alloc_error(layout))
    }
}
