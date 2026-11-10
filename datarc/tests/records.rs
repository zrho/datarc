#![deny(unsafe_op_in_unsafe_fn)]

use datarc::BuildError;
use std::mem::{align_of, size_of, size_of_val};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};

#[repr(align(128))]
#[derive(Default)]
struct Aligned(u8);

#[datarc::record]
pub struct Mixed {
    pub id: u64,
    #[datarc(len = u8)]
    pub bytes: [u8],
    pub name: String,
    #[datarc(len = u16)]
    aligned: [Aligned],
    pub words: [String],
}

/// Checks that a record and its Option are pointer-sized, independently sized arrays
/// share one aligned allocation, and clones share storage.
#[test]
fn thin_handle_and_independent_inline_arrays() {
    let value = Mixed::new(
        9,
        vec![1, 2, 3],
        "name".into(),
        vec![Aligned(7), Aligned(8)],
        vec!["word".into()],
    );
    assert_eq!(size_of::<Mixed>(), size_of::<usize>());
    assert_eq!(size_of::<Option<Mixed>>(), size_of::<usize>());
    assert_eq!(*value.id(), 9);
    assert_eq!(value.name(), "name");
    assert_eq!(value.bytes(), &[1, 2, 3]);
    assert_eq!(
        value.aligned().iter().map(|a| a.0).collect::<Vec<_>>(),
        [7, 8]
    );
    assert_eq!(value.words(), &["word"]);
    assert_eq!(value.aligned().as_ptr() as usize % align_of::<Aligned>(), 0);

    // All arrays lie in this one allocation, after its fixed-size header.
    use datarc::__private::Storage;
    let base = value.__datarc.as_ptr() as usize;
    let end = base + value.__datarc.get().layout().size();
    for (start, size) in [
        (value.bytes().as_ptr() as usize, size_of_val(value.bytes())),
        (
            value.aligned().as_ptr() as usize,
            size_of_val(value.aligned()),
        ),
        (value.words().as_ptr() as usize, size_of_val(value.words())),
    ] {
        assert!(base <= start && start + size <= end);
    }
    let shared = value.clone();
    assert!(std::ptr::eq(value.id(), shared.id()));
    assert!(std::ptr::eq(value.bytes(), shared.bytes()));
    drop(value);
    assert_eq!(shared.words(), &["word"]);
}

/// Checks that empty inline arrays expose empty slices with correctly aligned pointers.
#[test]
fn empty_arrays_are_aligned() {
    let value = Mixed::new(0, vec![], String::new(), vec![], vec![]);
    assert!(value.bytes().is_empty());
    assert!(value.aligned().is_empty());
    assert!(value.words().is_empty());
    assert_eq!(value.aligned().as_ptr() as usize % align_of::<Aligned>(), 0);
}

#[datarc::record]
struct Arithmetic<T, U, V> {
    tag: u8,
    first: [T],
    separator: bool,
    second: [U],
    third: [V],
}

#[derive(Default)]
#[repr(align(256))]
struct AlignedZst;

/// Checks accessor addresses and lengths against Layout::extend across varied array
/// sizes, alignment padding, and zero-sized elements.
#[test]
fn accessor_offsets_match_checked_layout_across_padding_and_zero_sized_arrays() {
    use std::alloc::Layout;

    fn check<T: Default, U: Default, V: Default>() {
        for first in [0, 1, 2, 3, 7, 16] {
            for second in [0, 1, 2, 3, 7, 16] {
                for third in [0, 2] {
                    let record = Arithmetic::<T, U, V>::new(
                        42,
                        (0..first).map(|_| T::default()),
                        true,
                        (0..second).map(|_| U::default()),
                        (0..third).map(|_| V::default()),
                    );
                    assert_eq!(*record.tag(), 42);
                    assert!(*record.separator());
                    let base = record.__datarc.as_ptr().cast::<u8>();
                    let mut layout = Layout::new::<__DatarcStorage_Arithmetic<T, U, V>>();
                    for (tail, actual, actual_len, expected_len) in [
                        (
                            Layout::array::<T>(first).unwrap(),
                            record.first().as_ptr().cast::<u8>(),
                            record.first().len(),
                            first,
                        ),
                        (
                            Layout::array::<U>(second).unwrap(),
                            record.second().as_ptr().cast::<u8>(),
                            record.second().len(),
                            second,
                        ),
                        (
                            Layout::array::<V>(third).unwrap(),
                            record.third().as_ptr().cast::<u8>(),
                            record.third().len(),
                            third,
                        ),
                    ] {
                        let (next, offset) = layout.extend(tail).unwrap();
                        layout = next;
                        // SAFETY: checked Layout::extend describes an offset
                        // within (or one past) this live allocation.
                        assert_eq!(actual, unsafe { base.add(offset) });
                        assert_eq!(actual_len, expected_len);
                    }
                }
            }
        }
    }

    check::<u8, u32, [u64; 3]>();
    check::<AlignedZst, u8, Aligned>();
    check::<u8, AlignedZst, u64>();
}

#[datarc::record]
struct Generic<'a, T, const N: usize = 2>
where
    T: 'a,
{
    label: &'a str,
    #[datarc(len = u32)]
    items: [T],
    array: [u8; N],
}

/// Checks that owners preserve type and const generics and borrowed payload lifetimes
/// while sharing non-Clone elements.
#[test]
fn generics_lifetimes_and_non_clone_elements() {
    struct NotClone(i32);
    let label = String::from("borrowed");
    let value = Generic::new(label.as_str(), vec![NotClone(42)], [1, 2]);
    let copy = value.clone();
    assert_eq!(*copy.label(), "borrowed");
    assert_eq!(copy.items()[0].0, 42);
    assert_eq!(copy.array(), &[1, 2]);
}

#[datarc::record]
struct SliceSyntax<'a, const N: usize> {
    rows: [[u8; N]],
    #[datarc(len = u8)]
    labels: [String],
    fixed: [u8; N],
    borrowed: &'a [u8],
    #[datarc(len = u16)]
    numbers: [u32],
    #[allow(unused_parens)]
    parenthesized: ([u16]),
}

/// Checks that slice parsing distinguishes inline arrays from fixed arrays and borrowed
/// slices, including parenthesized types.
#[test]
fn slice_syntax_distinguishes_fixed_arrays_references_and_element_types() {
    let borrowed = [9, 8];
    let record = SliceSyntax::new(
        vec![[1, 2], [3, 4]],
        vec!["label".into()],
        [5, 6],
        &borrowed,
        vec![7],
        vec![10, 11],
    );
    let rows: &[[u8; 2]] = record.rows();
    let labels: &[String] = record.labels();
    let fixed: &[u8; 2] = record.fixed();
    let borrowed_slice: &&[u8] = record.borrowed();
    assert_eq!(rows, &[[1, 2], [3, 4]]);
    assert_eq!(labels, &["label"]);
    assert_eq!(fixed, &[5, 6]);
    assert!(std::ptr::eq(*borrowed_slice, borrowed.as_slice()));
    assert_eq!(record.numbers(), &[7]);
    assert_eq!(record.parenthesized(), &[10, 11]);
    assert_eq!(size_of_val(&record), size_of::<usize>());
}

macro_rules! forwarded_record {
    ($owner:ident, $ty:ty) => {
        #[datarc::record]
        struct $owner {
            values: $ty,
        }
    };
}

forwarded_record!(ForwardedSlice, [u16]);

/// Checks that slice types passed through a macro_rules type fragment are recognized as
/// inline arrays.
#[test]
fn slice_types_forwarded_through_macro_rules_are_recognized() {
    assert_eq!(ForwardedSlice::new(vec![1, 2]).values(), &[1, 2]);
}

#[datarc::record]
struct Fixed {
    value: String,
}

#[datarc::record]
struct Empty {}

/// Checks construction and cloning for records with only fixed fields or no fields,
/// including pointer-sized empty owners.
#[test]
fn fixed_only_and_empty_records() {
    assert_eq!(Fixed::new("fixed".into()).clone().value(), "fixed");
    let empty = Empty::new().clone();
    assert_eq!(size_of_val(&empty), size_of::<usize>());
}

struct Tracked {
    count: Arc<AtomicUsize>,
    panic: bool,
}

impl Drop for Tracked {
    fn drop(&mut self) {
        self.count.fetch_add(1, Ordering::Relaxed);
        assert!(!self.panic, "intentional destructor panic");
    }
}

#[datarc::record]
struct Dropping {
    fixed: Tracked,
    #[datarc(len = u8)]
    first: [Tracked],
    #[datarc(len = u64)]
    second: [Tracked],
}

fn tracked(count: &Arc<AtomicUsize>, panic: bool) -> Tracked {
    Tracked {
        count: count.clone(),
        panic,
    }
}

/// Checks that fixed fields and array elements remain alive while shared and are each
/// dropped once by the last owner.
#[test]
fn drops_exactly_once_after_last_owner() {
    let count = Arc::new(AtomicUsize::new(0));
    let value = Dropping::new(
        tracked(&count, false),
        vec![tracked(&count, false), tracked(&count, false)],
        vec![tracked(&count, false)],
    );
    let shared = value.clone();
    drop(value);
    assert_eq!(count.load(Ordering::Relaxed), 0);
    drop(shared);
    assert_eq!(count.load(Ordering::Relaxed), 4);
}

/// Checks that a panicking array element destructor does not prevent other elements,
/// later arrays, or fixed fields from being dropped.
#[test]
fn unwinding_tail_drops_other_elements_tails_and_header() {
    let count = Arc::new(AtomicUsize::new(0));
    let value = Dropping::new(
        tracked(&count, false),
        vec![tracked(&count, true), tracked(&count, false)],
        vec![tracked(&count, false)],
    );
    assert!(std::panic::catch_unwind(|| drop(value)).is_err());
    assert_eq!(count.load(Ordering::Relaxed), 4);
    assert_eq!(Arc::strong_count(&count), 1);
}

/// Checks that a panicking fixed-field destructor still allows every array element to
/// be dropped.
#[test]
fn unwinding_header_still_frees_all_values() {
    let count = Arc::new(AtomicUsize::new(0));
    let value = Dropping::new(
        tracked(&count, true),
        vec![tracked(&count, false)],
        vec![tracked(&count, false)],
    );
    assert!(std::panic::catch_unwind(|| drop(value)).is_err());
    assert_eq!(count.load(Ordering::Relaxed), 3);
    assert_eq!(Arc::strong_count(&count), 1);
}

/// Checks that length overflow identifies the field, length, and storage type and drops
/// all consumed inputs.
#[test]
fn length_overflow_drops_inputs_and_reports_field() {
    let count = Arc::new(AtomicUsize::new(0));
    let result = Dropping::try_new(
        tracked(&count, false),
        (0..256).map(|_| tracked(&count, false)).collect::<Vec<_>>(),
        vec![tracked(&count, false)],
    );
    assert!(matches!(
        result,
        Err(BuildError::LengthOverflow {
            field: "first",
            len: 256,
            length_type: "u8"
        })
    ));
    assert_eq!(count.load(Ordering::Relaxed), 258);
}

/// Checks that a u8 length accepts 255 elements and rejects 256 through the panicking
/// constructor.
#[test]
fn compact_lengths_accept_their_maximum() {
    let value = Mixed::try_new(0, vec![1; 255], String::new(), vec![], vec![]).unwrap();
    assert_eq!(value.bytes().len(), 255);
    assert!(std::panic::catch_unwind(|| Mixed::new(
        0,
        vec![1; 256],
        String::new(),
        vec![],
        vec![]
    ))
    .is_err());
}

static ZST_DROPS: AtomicUsize = AtomicUsize::new(0);
#[repr(align(256))]
struct Zst;
impl Drop for Zst {
    fn drop(&mut self) {
        ZST_DROPS.fetch_add(1, Ordering::Relaxed);
    }
}

#[datarc::record]
struct ZeroSized {
    #[datarc(len = u128)]
    items: [Zst],
    empty: [Zst],
}

/// Checks alignment for populated and empty slices of zero-sized elements and verifies
/// one destructor call per stored element.
#[test]
fn aligned_zero_sized_elements_are_dropped() {
    let before = ZST_DROPS.load(Ordering::Relaxed);
    let value = ZeroSized::new(vec![Zst, Zst, Zst], vec![]);
    assert_eq!(value.items().len(), 3);
    assert!(value.empty().is_empty());
    assert_eq!(value.items().as_ptr() as usize % 256, 0);
    assert_eq!(value.empty().as_ptr() as usize % 256, 0);
    drop(value);
    assert_eq!(ZST_DROPS.load(Ordering::Relaxed) - before, 3);
}

#[datarc::record]
struct Concurrent {
    count: Arc<AtomicUsize>,
    items: [Tracked],
}

/// Checks Send and Sync support, concurrent owner cloning and access, and exactly one
/// payload drop after all threads finish.
#[test]
fn concurrent_clone_and_drop() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Concurrent>();
    let count = Arc::new(AtomicUsize::new(0));
    let value = Concurrent::new(count.clone(), vec![tracked(&count, false)]);
    let barrier = Arc::new(Barrier::new(5));
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let value = value.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                for _ in 0..100 {
                    let shared = value.clone();
                    assert_eq!(shared.items().len(), 1);
                    assert_eq!(shared.count().load(Ordering::Relaxed), 0);
                }
                barrier.wait();
            })
        })
        .collect();
    barrier.wait();
    drop(value);
    barrier.wait();
    for thread in threads {
        thread.join().unwrap();
    }
    assert_eq!(count.load(Ordering::Relaxed), 1);
}

#[datarc::record]
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct Recursive {
    children: [Recursive],
}

/// Checks that a record can own an inline array of owners of the same record type.
#[test]
fn recursive_records() {
    let tree = Recursive::new(vec![Recursive::new(vec![])]);
    assert!(tree.children()[0].children().is_empty());
}

#[datarc::record]
struct Names {
    __lengths: usize,
    __datarc_input_0: [u8],
    r#type: bool,
}

/// Checks that raw identifiers and field names resembling generated internals receive
/// working accessors without collisions.
#[test]
fn raw_identifiers_and_internal_looking_names() {
    let names = Names::new(5, vec![3], true);
    assert_eq!(*names.__lengths(), 5);
    assert_eq!(names.__datarc_input_0(), &[3]);
    assert!(*names.r#type());
}
