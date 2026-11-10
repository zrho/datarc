#![deny(unsafe_op_in_unsafe_fn)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

std::thread_local! {
    static COUNT: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    static DEALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

struct Counting;

// SAFETY: every allocation operation delegates to the system allocator with
// unchanged arguments. Counters are thread-local and do not allocate.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNT.try_with(Cell::get).unwrap_or(false) {
            ALLOCATIONS.with(|n| n.set(n.get() + 1));
        }
        // SAFETY: inherited allocator contract.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if COUNT.try_with(Cell::get).unwrap_or(false) {
            DEALLOCATIONS.with(|n| n.set(n.get() + 1));
        }
        // SAFETY: inherited allocator contract.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

#[datarc::record]
struct Record {
    id: u64,
    #[datarc(len = u8)]
    bytes: [u8],
    #[datarc(len = u16)]
    words: [u64],
}

/// Checks that construction uses one allocation and releases input buffers, while
/// clones share storage until the last owner drops.
#[test]
fn construction_allocates_once_clone_never_allocates_and_last_drop_frees() {
    let bytes = vec![1, 2];
    let words = vec![3, 4];
    COUNT.with(|b| b.set(true));
    let record = Record::new(7, bytes, words);
    COUNT.with(|b| b.set(false));
    assert_eq!(ALLOCATIONS.with(Cell::get), 1);
    // Both input Vec buffers have been freed.
    assert_eq!(DEALLOCATIONS.with(Cell::get), 2);
    ALLOCATIONS.with(|n| n.set(0));
    DEALLOCATIONS.with(|n| n.set(0));

    COUNT.with(|b| b.set(true));
    let shared = record.clone();
    assert_eq!(*shared.id(), 7);
    assert_eq!(shared.bytes(), &[1, 2]);
    assert_eq!(shared.words(), &[3, 4]);
    drop(record);
    COUNT.with(|b| b.set(false));
    assert_eq!(ALLOCATIONS.with(Cell::get), 0);
    assert_eq!(DEALLOCATIONS.with(Cell::get), 0);

    COUNT.with(|b| b.set(true));
    drop(shared);
    COUNT.with(|b| b.set(false));
    assert_eq!(ALLOCATIONS.with(Cell::get), 0);
    assert_eq!(DEALLOCATIONS.with(Cell::get), 1);
}

/// Checks that mapped iterators write directly into a single allocation without
/// temporary buffers.
#[test]
fn direct_generation_uses_one_allocation_without_temporary_buffers() {
    COUNT.with(|b| b.set(true));
    let record = Record::new(7, (0..4).map(|i| i * 2), [3, 4]);
    COUNT.with(|b| b.set(false));
    assert_eq!(ALLOCATIONS.with(Cell::get), 1);
    assert_eq!(DEALLOCATIONS.with(Cell::get), 0);
    assert_eq!(record.bytes(), &[0, 2, 4, 6]);

    COUNT.with(|b| b.set(true));
    drop(record);
    COUNT.with(|b| b.set(false));
    assert_eq!(DEALLOCATIONS.with(Cell::get), 1);
}

/// Checks that an iterator yielding fewer items than reported returns a length error
/// and frees the partial allocation.
#[test]
fn count_mismatch_releases_the_partial_allocation() {
    struct WrongLength;
    impl Iterator for WrongLength {
        type Item = u64;
        fn next(&mut self) -> Option<u64> {
            None
        }
    }
    impl ExactSizeIterator for WrongLength {
        fn len(&self) -> usize {
            1
        }
    }

    COUNT.with(|b| b.set(true));
    let result = Record::try_new(7, [1, 2], WrongLength);
    COUNT.with(|b| b.set(false));
    assert!(matches!(
        result,
        Err(datarc::BuildError::IteratorLengthMismatch { .. })
    ));
    assert_eq!(ALLOCATIONS.with(Cell::get), 1);
    assert_eq!(DEALLOCATIONS.with(Cell::get), 1);
}

#[datarc::record]
struct Simple {
    id: u64,
    bytes: [u8],
}

/// Checks that construction allocates once and getters and clones reuse storage.
#[test]
fn record_construction_allocates_once_and_getters_do_not_allocate() {
    COUNT.with(|b| b.set(true));
    let owner = Simple::new(7, [1, 2, 3]);
    let shared = owner.clone();
    let id = owner.id();
    let bytes = owner.bytes();
    assert!(std::ptr::eq(bytes, shared.bytes()));
    drop(shared);
    COUNT.with(|b| b.set(false));
    assert_eq!(*id, 7);
    assert_eq!(ALLOCATIONS.with(Cell::get), 1);
    assert_eq!(DEALLOCATIONS.with(Cell::get), 0);
    COUNT.with(|b| b.set(true));
    drop(owner);
    COUNT.with(|b| b.set(false));
    assert_eq!(DEALLOCATIONS.with(Cell::get), 1);
}

#[derive(datarc::Packable)]
enum PackedValue {
    Record(Record),
}

/// Checks that enum packing, inspection, cloning, and unpacking allocate nothing, and
/// the final owner releases the record.
#[test]
fn packing_inspecting_cloning_and_unpacking_do_not_allocate() {
    let record = Record::new(7, [1, 2], [3, 4]);
    ALLOCATIONS.with(|n| n.set(0));
    DEALLOCATIONS.with(|n| n.set(0));
    COUNT.with(|b| b.set(true));
    let packed = PackedValuePacked::from(PackedValue::Record(record));
    packed.with_enum(|value| match value {
        PackedValue::Record(record) => assert_eq!(*record.id(), 7),
    });
    let clone = packed.clone();
    drop(packed);
    let unpacked = clone.into_enum();
    COUNT.with(|b| b.set(false));
    assert_eq!(ALLOCATIONS.with(Cell::get), 0);
    assert_eq!(DEALLOCATIONS.with(Cell::get), 0);
    COUNT.with(|b| b.set(true));
    drop(unpacked);
    COUNT.with(|b| b.set(false));
    assert_eq!(DEALLOCATIONS.with(Cell::get), 1);
}
