use datarc::Packable;
use std::cmp::Ordering;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

#[derive(Packable, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Record<'a, 'b> {
    key: &'a u32,
    values: &'b [i32],
}

#[test]
fn derived_record_traits_compare_contents_and_format_the_view() {
    let first = RecordPacked::new(1, [2, 3]);
    let equal = RecordPacked::new(1, [2, 3]);
    let later = RecordPacked::new(1, [2, 4]);
    assert_eq!(first, equal);
    assert_ne!(first, later);
    assert!(first < later);
    assert!(first <= equal);
    assert!(later > first);
    assert!(equal >= first);
    assert_eq!(first.cmp(&later), Ordering::Less);
    assert_eq!(first.partial_cmp(&later), Some(Ordering::Less));
    assert_eq!(format!("{first:?}"), format!("{:?}", Record::from(&first)));
    assert_eq!(
        format!("{first:#?}"),
        format!("{:#?}", Record::from(&first))
    );
}

#[derive(Packable, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[datarc(owned = GenericOwner)]
struct Generic<'view, 'data, T, const N: usize> {
    value: &'view T,
    text: &'view &'data str,
    rows: &'view [[u8; N]],
}

#[derive(Packable, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Empty {}

#[derive(Packable, Debug, PartialEq, PartialOrd)]
struct Float<'a> {
    value: &'a f64,
}

#[test]
fn generic_payload_lifetimes_empty_views_and_partial_ordering() {
    let text = String::from("borrowed");
    let owner = GenericOwner::new(1, text.as_str(), [[2, 3]]);
    let equal = GenericOwner::new(1, text.as_str(), [[2, 3]]);
    assert_eq!(owner, equal);
    assert_eq!(owner.cmp(&equal), Ordering::Equal);
    assert_eq!(EmptyPacked::new(), EmptyPacked::new());
    assert_eq!(EmptyPacked::new().cmp(&EmptyPacked::new()), Ordering::Equal);
    let nan = FloatPacked::new(f64::NAN);
    assert_ne!(nan, nan);
    assert_eq!(nan.partial_cmp(&nan), None);
}

// The payload has none of the forwarded traits. Only the view's implementations
// decide how the record behaves; the generated owner must not inspect its fields.
struct Payload;

#[derive(Packable)]
struct Manual<'a, T> {
    payload: &'a T,
    key: &'a u8,
}

static CALLS: [AtomicUsize; 9] = [const { AtomicUsize::new(0) }; 9];

impl<T> std::fmt::Debug for Manual<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        CALLS[0].fetch_add(1, Relaxed);
        write!(f, "key({})", self.key)
    }
}

impl<T> PartialEq for Manual<'_, T> {
    fn eq(&self, other: &Self) -> bool {
        CALLS[1].fetch_add(1, Relaxed);
        self.key == other.key
    }
    fn ne(&self, other: &Self) -> bool {
        CALLS[2].fetch_add(1, Relaxed);
        self.key != other.key
    }
}
impl<T> Eq for Manual<'_, T> {}

impl<T> PartialOrd for Manual<'_, T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        CALLS[3].fetch_add(1, Relaxed);
        Some(other.key.cmp(self.key))
    }
    fn lt(&self, other: &Self) -> bool {
        CALLS[4].fetch_add(1, Relaxed);
        self.key > other.key
    }
    fn le(&self, other: &Self) -> bool {
        CALLS[5].fetch_add(1, Relaxed);
        self.key >= other.key
    }
    fn gt(&self, other: &Self) -> bool {
        CALLS[6].fetch_add(1, Relaxed);
        self.key < other.key
    }
    fn ge(&self, other: &Self) -> bool {
        CALLS[7].fetch_add(1, Relaxed);
        self.key <= other.key
    }
}
impl<T> Ord for Manual<'_, T> {
    fn cmp(&self, other: &Self) -> Ordering {
        CALLS[8].fetch_add(1, Relaxed);
        other.key.cmp(self.key)
    }
}

#[test]
fn every_method_delegates_to_the_custom_view_implementation() {
    let first = ManualPacked::new(Payload, 1);
    let later = ManualPacked::new(Payload, 2);
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

#[derive(Packable, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Value {
    First(RecordPacked),
    Second(RecordPacked),
}

#[test]
fn enum_traits_delegate_without_changing_ownership() {
    let first = ValuePacked::from(Value::First(RecordPacked::new(1, [2])));
    let equal = first.clone();
    let second = ValuePacked::from(Value::Second(RecordPacked::new(0, [])));
    assert_eq!(first, equal);
    assert!(first < second);
    assert_eq!(first.cmp(&second), Ordering::Less);
    assert_eq!(first.partial_cmp(&equal), Some(Ordering::Equal));
    assert_eq!(format!("{first:?}"), first.with_enum(|v| format!("{v:?}")));
    drop(equal);
    assert!(matches!(first.into_enum(), Value::First(_)));
}
