use datarc::{datarc, BuildError};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct Counted {
    drops: Arc<AtomicUsize>,
    panic_on_drop: bool,
}

impl Drop for Counted {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::Relaxed);
        assert!(!self.panic_on_drop, "intentional element destructor panic");
    }
}

fn counted(drops: &Arc<AtomicUsize>) -> Counted {
    Counted {
        drops: drops.clone(),
        panic_on_drop: false,
    }
}

#[datarc]
struct Record {
    fixed: Counted,
    #[datarc(len = u8)]
    first: [Counted],
    second: [Counted],
}

// Deliberately violates ExactSizeIterator's contract. That is a logic error,
// but this safe trait must never be trusted for memory safety.
struct Reported<I> {
    inner: I,
    reported: usize,
}

impl<I: Iterator> Iterator for Reported<I> {
    type Item = I::Item;
    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.reported, Some(self.reported))
    }
}

impl<I: Iterator> ExactSizeIterator for Reported<I> {
    fn len(&self) -> usize {
        self.reported
    }
}

#[test]
fn arrays_vectors_and_mapped_iterators_move_non_clone_values() {
    let drops = Arc::new(AtomicUsize::new(0));
    let record = Record::new(
        counted(&drops),
        (0..3).map(|_| counted(&drops)),
        [counted(&drops), counted(&drops)],
    );
    assert!(Arc::ptr_eq(&record.fixed().drops, &drops));
    assert_eq!(record.first().len(), 3);
    assert_eq!(record.second().len(), 2);
    assert_eq!(drops.load(Ordering::Relaxed), 0);
    drop(record);
    assert_eq!(drops.load(Ordering::Relaxed), 6);
    assert_eq!(Arc::strong_count(&drops), 1);
}

#[datarc]
struct Numbers {
    values: [u32],
    names: [String],
}

#[test]
fn ranges_empty_iterators_and_explicitly_cloned_slices() {
    let names = [String::from("first"), String::from("second")];
    let record = Numbers::try_new(0..3, names.iter().cloned()).unwrap();
    assert_eq!(record.values(), &[0, 1, 2]);
    assert_eq!(record.names(), &names);
    let empty = Numbers::new(std::iter::empty(), []);
    assert!(empty.values().is_empty());
    assert!(empty.names().is_empty());
}

#[test]
fn incorrect_lengths_in_any_array_drop_all_inputs_and_initialized_values() {
    for (field, reported, actual) in [
        ("first", 0, 1),
        ("first", 3, 1),
        ("first", 1, 3),
        ("second", 0, 1),
        ("second", 3, 1),
        ("second", 1, 3),
    ] {
        let drops = Arc::new(AtomicUsize::new(0));
        let (first_actual, first_reported) = if field == "first" {
            (actual, reported)
        } else {
            (2, 2)
        };
        let (second_actual, second_reported) = if field == "second" {
            (actual, reported)
        } else {
            (2, 2)
        };
        let first: Vec<_> = (0..first_actual).map(|_| counted(&drops)).collect();
        let second: Vec<_> = (0..second_actual).map(|_| counted(&drops)).collect();
        let result = Record::try_new(
            counted(&drops),
            Reported {
                inner: first.into_iter(),
                reported: first_reported,
            },
            Reported {
                inner: second.into_iter(),
                reported: second_reported,
            },
        );
        assert!(
            matches!(result, Err(BuildError::IteratorLengthMismatch { field: actual_field, expected })
            if actual_field == field && expected == reported)
        );
        assert_eq!(
            drops.load(Ordering::Relaxed),
            1 + first_actual + second_actual
        );
        assert_eq!(Arc::strong_count(&drops), 1);
    }
}

#[test]
fn excess_items_are_bounded_even_for_an_infinite_iterator() {
    let drops = Arc::new(AtomicUsize::new(0));
    let result = Record::try_new(
        counted(&drops),
        Reported {
            inner: std::iter::repeat_with(|| counted(&drops)),
            reported: 2,
        },
        [counted(&drops)],
    );
    assert!(matches!(
        result,
        Err(BuildError::IteratorLengthMismatch {
            field: "first",
            expected: 2
        })
    ));
    // Two initialized items, one excess item, the fixed field and later input.
    assert_eq!(drops.load(Ordering::Relaxed), 5);
    assert_eq!(Arc::strong_count(&drops), 1);
}

#[test]
fn new_panics_on_length_mismatch_after_cleanup() {
    let drops = Arc::new(AtomicUsize::new(0));
    let result = std::panic::catch_unwind(|| {
        Record::new(
            counted(&drops),
            Reported {
                inner: [counted(&drops)].into_iter(),
                reported: 0,
            },
            [counted(&drops)],
        )
    });
    assert!(result.is_err());
    assert_eq!(drops.load(Ordering::Relaxed), 3);
    assert_eq!(Arc::strong_count(&drops), 1);
}

#[test]
fn iterator_panic_drops_earlier_arrays_current_prefix_and_header() {
    let drops = Arc::new(AtomicUsize::new(0));
    let result = std::panic::catch_unwind(|| {
        Record::new(
            counted(&drops),
            [counted(&drops), counted(&drops)],
            (0..3).map(|i| {
                assert_ne!(i, 1, "intentional iterator panic");
                counted(&drops)
            }),
        )
    });
    assert!(result.is_err());
    assert_eq!(drops.load(Ordering::Relaxed), 4);
    assert_eq!(Arc::strong_count(&drops), 1);
}

struct DropPanic<I>(I);
impl<I: Iterator> Iterator for DropPanic<I> {
    type Item = I::Item;
    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
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

#[test]
fn iterator_destructor_panic_keeps_all_initialization_guards_active() {
    let drops = Arc::new(AtomicUsize::new(0));
    let result = std::panic::catch_unwind(|| {
        Record::new(
            counted(&drops),
            [counted(&drops), counted(&drops)],
            DropPanic([counted(&drops)].into_iter()),
        )
    });
    assert!(result.is_err());
    assert_eq!(drops.load(Ordering::Relaxed), 4);
    assert_eq!(Arc::strong_count(&drops), 1);
}

struct LenPanic<I>(I);
impl<I: Iterator> Iterator for LenPanic<I> {
    type Item = I::Item;
    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }
}
impl<I: Iterator> ExactSizeIterator for LenPanic<I> {
    fn len(&self) -> usize {
        panic!("intentional len panic");
    }
}

struct IntoIterPanic<T>(Vec<T>);
impl<T> IntoIterator for IntoIterPanic<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;
    fn into_iter(self) -> Self::IntoIter {
        let _ = &self.0;
        panic!("intentional into_iter panic");
    }
}

#[test]
fn panics_before_allocation_drop_every_input() {
    for panic_in_len in [false, true] {
        let drops = Arc::new(AtomicUsize::new(0));
        let result = std::panic::catch_unwind(|| {
            if panic_in_len {
                Record::new(
                    counted(&drops),
                    [counted(&drops)],
                    LenPanic([counted(&drops)].into_iter()),
                )
            } else {
                Record::new(
                    counted(&drops),
                    [counted(&drops)],
                    IntoIterPanic(vec![counted(&drops)]),
                )
            }
        });
        assert!(result.is_err());
        assert_eq!(drops.load(Ordering::Relaxed), 3);
        assert_eq!(Arc::strong_count(&drops), 1);
    }
}

#[test]
fn destructor_panic_during_failed_initialization_still_cleans_other_fields() {
    let drops = Arc::new(AtomicUsize::new(0));
    let result = std::panic::catch_unwind(|| {
        Record::try_new(
            counted(&drops),
            [
                Counted {
                    drops: drops.clone(),
                    panic_on_drop: true,
                },
                counted(&drops),
            ],
            Reported {
                inner: [counted(&drops)].into_iter(),
                reported: 2,
            },
        )
    });
    assert!(result.is_err());
    assert_eq!(drops.load(Ordering::Relaxed), 4);
    assert_eq!(Arc::strong_count(&drops), 1);
}

#[test]
fn compact_length_overflow_is_checked_before_generating_elements() {
    let drops = Arc::new(AtomicUsize::new(0));
    let result = Record::try_new(
        counted(&drops),
        (0..256).map(|_| {
            panic!("must not generate elements before length validation");
        }),
        [counted(&drops)],
    );
    assert!(matches!(
        result,
        Err(BuildError::LengthOverflow {
            field: "first",
            len: 256,
            ..
        })
    ));
    assert_eq!(drops.load(Ordering::Relaxed), 2);
}

#[datarc]
struct ZeroSized {
    values: [()],
}

#[test]
fn zero_sized_arrays_validate_actual_counts_even_with_maximum_reported_length() {
    for reported in [0, 3, usize::MAX] {
        let result = ZeroSized::try_new(Reported {
            inner: std::iter::once(()),
            reported,
        });
        assert!(
            matches!(result, Err(BuildError::IteratorLengthMismatch { expected, .. }) if expected == reported)
        );
    }
    assert_eq!(ZeroSized::new([(); 3]).values().len(), 3);
}
