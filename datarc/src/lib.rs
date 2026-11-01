#![deny(unsafe_op_in_unsafe_fn)]
#![doc = include_str!("../../README.md")]

extern crate self as datarc;

pub use datarc_macros::datarc;

#[cfg(doctest)]
mod compile_checks;

/// A record could not be represented by its selected length types or layout.
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

    /// Its atomic is private so safe code cannot corrupt a live reference count.
    #[repr(transparent)]
    pub struct RefCount(AtomicUsize);

    impl Default for RefCount {
        fn default() -> Self {
            Self(AtomicUsize::new(1))
        }
    }

    /// # Safety
    /// Implementations must return the same count and allocation layout for the
    /// lifetime of an allocation. `drop_tail` must drop exactly the initialized
    /// trailing elements, without destroying the header or freeing the allocation.
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
        /// `ptr` must be a fully initialized allocation with count one, allocated
        /// by the global allocator using `T::layout()`, and have no other owner.
        pub unsafe fn from_raw(ptr: NonNull<T>) -> Self {
            Self {
                ptr,
                marker: PhantomData,
            }
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
            let old = self.get().ref_count().0.fetch_add(1, Ordering::Relaxed);
            // Bound the count well below wraparound, including racing clones.
            if old >= isize::MAX as usize {
                std::process::abort();
            }
            Self {
                ptr: self.ptr,
                marker: PhantomData,
            }
        }
    }

    impl<T: Storage> Drop for Shared<T> {
        fn drop(&mut self) {
            if self.get().ref_count().0.fetch_sub(1, Ordering::Release) != 1 {
                return;
            }
            fence(Ordering::Acquire);
            let allocation = Allocation {
                ptr: self.ptr,
                layout: self.get().layout(),
            };
            // SAFETY: the last strong owner has exclusive access to the storage.
            // The guard drops the header and frees memory even if a tail panics.
            unsafe { T::drop_tail(self.ptr.as_ptr()) };
            drop(allocation);
        }
    }

    struct Allocation<T> {
        ptr: NonNull<T>,
        layout: Layout,
    }

    impl<T> Drop for Allocation<T> {
        fn drop(&mut self) {
            struct Free(NonNull<u8>, Layout);
            impl Drop for Free {
                fn drop(&mut self) {
                    // SAFETY: this guard owns the allocation and its exact layout.
                    unsafe { dealloc(self.0.as_ptr(), self.1) };
                }
            }
            let _free = Free(self.ptr.cast(), self.layout);
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

    pub fn allocate(layout: Layout) -> NonNull<u8> {
        // Generated storage always includes a nonzero-sized reference count.
        assert!(layout.size() > 0);
        // SAFETY: Layout already enforces the allocator's size/alignment rules.
        NonNull::new(unsafe { alloc(layout) }).unwrap_or_else(|| handle_alloc_error(layout))
    }
}
