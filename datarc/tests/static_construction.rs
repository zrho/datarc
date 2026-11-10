#![deny(unsafe_op_in_unsafe_fn)]

use std::alloc::Layout;
use std::cell::{Cell, UnsafeCell};
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[repr(align(256))]
struct Bytes([MaybeUninit<u8>; 4096]);

struct Arena {
    claimed: AtomicBool,
    bytes: UnsafeCell<Bytes>,
}

// SAFETY: the atomic claim hands out the buffer exactly once, after which
// initialization and access belong exclusively to the generated record.
unsafe impl Sync for Arena {}

impl Arena {
    const fn new() -> Self {
        Self {
            claimed: AtomicBool::new(false),
            bytes: UnsafeCell::new(Bytes([MaybeUninit::uninit(); 4096])),
        }
    }

    fn allocate(&'static self, layout: Layout) -> *mut u8 {
        assert!(layout.size() <= 4096);
        assert!(layout.align() <= 256);
        assert!(!self.claimed.swap(true, Ordering::Relaxed));
        // SAFETY: the successful claim grants exclusive initialization access.
        unsafe { (*self.bytes.get()).0.as_mut_ptr().cast() }
    }

    /// # Safety
    /// The arena must contain a successfully initialized record header.
    unsafe fn count(&self) -> usize {
        // SAFETY: generated repr(C) headers start with a transparent RefCount
        // containing AtomicUsize; the buffer remains live and aligned forever.
        unsafe { (&*self.bytes.get().cast::<AtomicUsize>()).load(Ordering::Relaxed) }
    }
}

struct Probe(&'static AtomicUsize);

impl Drop for Probe {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[repr(align(128))]
struct Aligned(u8);

#[datarc::record]
struct Record {
    fixed: Probe,
    #[datarc(len = u16)]
    values: [Probe],
    #[datarc(len = u8)]
    aligned: [Aligned],
}

/// Checks that static and counted records use the same owner type, preserve alignment,
/// and only counted records drop their contents.
#[test]
fn static_records_mix_with_counted_records_and_never_drop_their_contents() {
    static ARENA: Arena = Arena::new();
    static IMMORTAL_DROPS: AtomicUsize = AtomicUsize::new(0);
    static NORMAL_DROPS: AtomicUsize = AtomicUsize::new(0);
    let calls = Cell::new(0);
    let token = String::from("FnOnce allocator");
    // SAFETY: the arena supplies a unique properly aligned permanent region.
    let permanent = unsafe {
        Record::new_static(
            |layout| {
                drop(token);
                calls.set(calls.get() + 1);
                ARENA.allocate(layout)
            },
            Probe(&IMMORTAL_DROPS),
            [Probe(&IMMORTAL_DROPS), Probe(&IMMORTAL_DROPS)],
            [Aligned(7), Aligned(8)],
        )
    };
    assert_eq!(calls.get(), 1);
    assert_eq!(
        std::mem::size_of_val(&permanent),
        std::mem::size_of::<usize>()
    );
    let shared = permanent.clone();
    assert!(std::ptr::eq(shared.fixed(), permanent.fixed()));
    assert_eq!(permanent.values().len(), 2);
    assert_eq!(permanent.aligned()[0].0, 7);
    assert_eq!(permanent.aligned()[1].0, 8);
    assert_eq!(permanent.aligned().as_ptr() as usize % 128, 0);
    drop(shared);
    // SAFETY: new_static has completed header initialization.
    assert_eq!(unsafe { ARENA.count() }, usize::MAX);
    for _ in 0..32 {
        drop(permanent.clone());
    }
    let ordinary = Record::new(Probe(&NORMAL_DROPS), [Probe(&NORMAL_DROPS)], []);
    let mixed: [Record; 2] = [ordinary, permanent.clone()];
    drop(mixed);
    assert_eq!(NORMAL_DROPS.load(Ordering::Relaxed), 2);
    drop(permanent);
    assert_eq!(IMMORTAL_DROPS.load(Ordering::Relaxed), 0);
    // SAFETY: immortal contents remain initialized after every handle is dropped.
    assert_eq!(unsafe { ARENA.count() }, usize::MAX);
}

/// Checks that concurrent cloning and dropping of static owners leaves the immortal
/// reference-count sentinel unchanged and drops no payloads.
#[test]
fn concurrent_static_clones_and_drops_leave_the_sentinel_unchanged() {
    static ARENA: Arena = Arena::new();
    static DROPS: AtomicUsize = AtomicUsize::new(0);
    // SAFETY: this arena is used exactly once and lives for the whole process.
    let owner =
        unsafe { Record::new_static(|layout| ARENA.allocate(layout), Probe(&DROPS), [], []) };
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                for _ in 0..64 {
                    let cloned = owner.clone();
                    assert!(cloned.values().is_empty());
                    drop(cloned);
                }
            });
        }
    });
    drop(owner);
    assert_eq!(DROPS.load(Ordering::Relaxed), 0);
    // SAFETY: the immortal header remains initialized in the arena.
    assert_eq!(unsafe { ARENA.count() }, usize::MAX);
}

struct Short<I>(I);
impl<I: Iterator> Iterator for Short<I> {
    type Item = I::Item;
    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }
}
impl<I: Iterator> ExactSizeIterator for Short<I> {
    fn len(&self) -> usize {
        3
    }
}

struct DropPanic<I>(I);
impl<I: Iterator> Iterator for DropPanic<I> {
    type Item = I::Item;
    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }
}
impl<I: ExactSizeIterator> ExactSizeIterator for DropPanic<I> {
    fn len(&self) -> usize {
        self.0.len()
    }
}
impl<I> Drop for DropPanic<I> {
    fn drop(&mut self) {
        panic!("intentional iterator destructor panic");
    }
}

/// Checks that length errors and iterator panics clean up initialized static-record
/// values while retaining the supplied storage.
#[test]
fn failed_initialization_drops_values_without_deallocating_static_storage() {
    static ARENAS: [Arena; 3] = [Arena::new(), Arena::new(), Arena::new()];
    static DROPS: AtomicUsize = AtomicUsize::new(0);
    for (case, arena) in ARENAS.iter().enumerate() {
        DROPS.store(0, Ordering::Relaxed);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // SAFETY: each case claims its own permanent, aligned arena once.
            unsafe {
                match case {
                    0 => Record::new_static(
                        |layout| arena.allocate(layout),
                        Probe(&DROPS),
                        Short([Probe(&DROPS), Probe(&DROPS)].into_iter()),
                        [],
                    ),
                    1 => Record::new_static(
                        |layout| arena.allocate(layout),
                        Probe(&DROPS),
                        (0..3).map(|i| {
                            assert_ne!(i, 1, "intentional iterator panic");
                            Probe(&DROPS)
                        }),
                        [],
                    ),
                    _ => Record::new_static(
                        |layout| arena.allocate(layout),
                        Probe(&DROPS),
                        DropPanic([Probe(&DROPS), Probe(&DROPS)].into_iter()),
                        [],
                    ),
                }
            }
        }));
        assert!(result.is_err());
        assert!(arena.claimed.load(Ordering::Relaxed));
        assert_eq!(DROPS.load(Ordering::Relaxed), if case == 1 { 2 } else { 3 });
    }
}

/// Checks that an overflowing static-record length rejects construction before calling
/// the allocator and drops all inputs.
#[test]
fn lengths_are_validated_before_calling_the_allocator() {
    static DROPS: AtomicUsize = AtomicUsize::new(0);
    let called = Cell::new(false);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: the allocator never returns a region; validation must reject
        // the oversized field before calling it.
        unsafe {
            Record::new_static(
                |_| {
                    called.set(true);
                    panic!("allocator must not be called")
                },
                Probe(&DROPS),
                [Probe(&DROPS)],
                (0..256).map(|_| Aligned(0)),
            )
        }
    }));
    assert!(result.is_err());
    assert!(!called.get());
    assert_eq!(DROPS.load(Ordering::Relaxed), 2);
}

#[datarc::record]
struct Fixed {
    value: u64,
}

#[datarc::record]
struct Empty {}

/// Checks static construction and field access for fixed-only records and construction
/// of empty records in separate permanent regions.
#[test]
fn fixed_only_and_empty_records_support_static_allocation() {
    static FIXED: Arena = Arena::new();
    static EMPTY: Arena = Arena::new();
    // SAFETY: both arenas provide independent permanent regions exactly once.
    let fixed = unsafe { Fixed::new_static(|layout| FIXED.allocate(layout), 9) };
    assert_eq!(*fixed.value(), 9);
    // SAFETY: EMPTY is independent of FIXED and has not been claimed.
    let empty = unsafe { Empty::new_static(|layout| EMPTY.allocate(layout)) };
    drop((fixed, empty));
}

#[datarc::record]
struct Generic<'data, T, const N: usize> {
    data: &'data [u8; N],
    values: [T],
    __datarc_allocator: bool,
    __datarc_static: u32,
    __datarc_construct: u8,
}

/// Checks static construction with type and const generics, borrowed payloads, and
/// field names resembling generated constructor internals.
#[test]
fn generics_payload_lifetimes_and_internal_looking_names_work() {
    static ARENA: Arena = Arena::new();
    static DROPS: AtomicUsize = AtomicUsize::new(0);
    let source = [1, 2];
    // SAFETY: the allocation is permanent; the owner retains the source's
    // payload lifetime, so source data is borrowed only while it is live.
    let owner = unsafe {
        Generic::new_static(
            |layout| ARENA.allocate(layout),
            &source,
            [Probe(&DROPS)],
            true,
            7,
            8,
        )
    };
    assert_eq!(owner.data(), &&[1, 2]);
    assert_eq!(owner.values().len(), 1);
    assert!(*owner.__datarc_allocator());
    assert_eq!(*owner.__datarc_static(), 7);
    assert_eq!(*owner.__datarc_construct(), 8);
    drop(owner);
    assert_eq!(DROPS.load(Ordering::Relaxed), 0);
}
