use std::cmp::Ordering;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

#[datarc::record]
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Record {
    key: u32,
    values: [i32],
}

#[test]
fn derived_record_traits_compare_and_format_fields() {
    let first = Record::new(1, [2, 3]);
    let equal = Record::new(1, [2, 3]);
    let later = Record::new(1, [2, 4]);
    assert_eq!(first, equal);
    assert_ne!(first, later);
    assert!(first < later);
    assert!(first <= equal);
    assert!(later > first);
    assert!(equal >= first);
    assert_eq!(first.cmp(&later), Ordering::Less);
    assert_eq!(first.partial_cmp(&later), Some(Ordering::Less));
    assert_eq!(format!("{first:?}"), "Record { key: 1, values: [2, 3] }");
    assert_eq!(
        format!("{first:#?}"),
        "Record {\n    key: 1,\n    values: [\n        2,\n        3,\n    ],\n}"
    );
}

#[datarc::record]
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Generic<'data, T, const N: usize> {
    value: T,
    text: &'data str,
    rows: [[u8; N]],
}

#[datarc::record]
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Empty {}

#[datarc::record]
#[derive(Debug, PartialEq, PartialOrd)]
struct Float {
    value: f64,
}

#[test]
fn generic_payload_lifetimes_empty_records_and_partial_ordering() {
    let text = String::from("borrowed");
    let owner = Generic::new(1, text.as_str(), [[2, 3]]);
    let equal = Generic::new(1, text.as_str(), [[2, 3]]);
    assert_eq!(owner, equal);
    assert_eq!(owner.cmp(&equal), Ordering::Equal);
    assert_eq!(Empty::new(), Empty::new());
    assert_eq!(Empty::new().cmp(&Empty::new()), Ordering::Equal);
    let nan = Float::new(f64::NAN);
    assert_ne!(nan, nan);
    assert_eq!(nan.partial_cmp(&nan), None);
}

// The payload has none of these traits. The owner implementations use only key.
struct Payload;

#[datarc::record]
struct Manual<T> {
    payload: T,
    key: u8,
}

static CALLS: [AtomicUsize; 9] = [const { AtomicUsize::new(0) }; 9];

impl<T> std::fmt::Debug for Manual<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        CALLS[0].fetch_add(1, Relaxed);
        write!(f, "key({})", self.key())
    }
}

impl<T> PartialEq for Manual<T> {
    fn eq(&self, other: &Self) -> bool {
        CALLS[1].fetch_add(1, Relaxed);
        self.key() == other.key()
    }
    // Keep separate counters to check each custom method.
    #[allow(clippy::partialeq_ne_impl)]
    fn ne(&self, other: &Self) -> bool {
        CALLS[2].fetch_add(1, Relaxed);
        self.key() != other.key()
    }
}
impl<T> Eq for Manual<T> {}

// Do not call cmp: each method has its own counter.
#[allow(clippy::non_canonical_partial_ord_impl)]
impl<T> PartialOrd for Manual<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        CALLS[3].fetch_add(1, Relaxed);
        Some(other.key().cmp(self.key()))
    }
    fn lt(&self, other: &Self) -> bool {
        CALLS[4].fetch_add(1, Relaxed);
        self.key() > other.key()
    }
    fn le(&self, other: &Self) -> bool {
        CALLS[5].fetch_add(1, Relaxed);
        self.key() >= other.key()
    }
    fn gt(&self, other: &Self) -> bool {
        CALLS[6].fetch_add(1, Relaxed);
        self.key() < other.key()
    }
    fn ge(&self, other: &Self) -> bool {
        CALLS[7].fetch_add(1, Relaxed);
        self.key() <= other.key()
    }
}
impl<T> Ord for Manual<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        CALLS[8].fetch_add(1, Relaxed);
        other.key().cmp(self.key())
    }
}

#[test]
fn every_method_uses_the_custom_owner_implementation() {
    let first = Manual::new(Payload, 1);
    let later = Manual::new(Payload, 2);
    assert_eq!(format!("{first:?}"), "key(1)");
    assert!(!first.eq(&later));
    assert!(first.ne(&later));
    assert_eq!(first.partial_cmp(&later), Some(Ordering::Greater));
    assert!(!first.lt(&later));
    assert!(!first.le(&later));
    assert!(first.gt(&later));
    assert!(first.ge(&later));
    assert_eq!(first.cmp(&later), Ordering::Greater);
    for calls in &CALLS {
        assert_eq!(calls.load(Relaxed), 1);
    }
}

#[datarc::packed]
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Value {
    First(Record),
    Second(Record),
}

#[test]
fn enum_traits_delegate_without_changing_ownership() {
    let first = ValuePacked::from(Value::First(Record::new(1, [2])));
    let equal = first.clone();
    let second = ValuePacked::from(Value::Second(Record::new(0, [])));
    assert_eq!(first, equal);
    assert!(first < second);
    assert_eq!(first.cmp(&second), Ordering::Less);
    assert_eq!(first.partial_cmp(&equal), Some(Ordering::Equal));
    assert_eq!(format!("{first:?}"), first.with_enum(|v| format!("{v:?}")));
    drop(equal);
    assert!(matches!(first.into_enum(), Value::First(_)));
}
