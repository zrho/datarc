use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[datarc::record]
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Record {
    pub id: u64,
    #[datarc(mut, len = u16)]
    pub text: str,
    #[datarc(len = u16)]
    pub values: [i32],
    pub suffix: str,
}

/// Checks pointer-sized owners, direct construction, and shared storage.
#[test]
fn record_construction_and_shared_storage() {
    let mut owner = Record::new(42, String::from("héllo"), [1, 2, 3], "");
    assert_eq!(std::mem::size_of::<Record>(), std::mem::size_of::<usize>());
    assert_eq!(*owner.id(), 42);
    assert_eq!(owner.values(), &[1, 2, 3]);
    assert_eq!(owner.text(), "héllo");
    assert_eq!(owner.suffix(), "");
    let shared = owner.clone();
    assert!(std::ptr::eq(shared.id(), owner.id()));
    assert!(std::ptr::eq(shared.values(), owner.values()));
    assert!(std::ptr::eq(shared.text(), owner.text()));
    let separate = Record::try_new(42, "héllo", [1, 2, 3], "").unwrap();
    assert_eq!(owner, separate);
    assert_eq!(owner.cmp(&separate), std::cmp::Ordering::Equal);
    assert!(format!("{owner:?}").contains("héllo"));
    assert!(!std::ptr::eq(separate.values(), owner.values()));
    assert_eq!(separate.values(), owner.values());
    owner.text_mut().make_ascii_uppercase();
    assert_eq!(owner.text(), "HéLLO");
    assert_eq!(shared.text(), "héllo");
    assert_eq!(owner.values(), shared.values());
    assert!(!owner.ptr_eq(&shared));
    let address = owner.text().as_ptr();
    owner.text_mut().make_ascii_lowercase();
    assert_eq!(owner.text().as_ptr(), address);
    assert_eq!(owner.text(), "héllo");

    // SAFETY: the allocation is unique, correctly aligned, and retained forever.
    let mut permanent = unsafe {
        Record::new_static(
            |layout| std::alloc::alloc(layout),
            42,
            "héllo",
            [1, 2, 3],
            "!",
        )
    };
    let shared_static = permanent.clone();
    permanent.text_mut().make_ascii_uppercase();
    assert_eq!(permanent.text(), "HéLLO");
    assert_eq!(shared_static.text(), "héllo");
    assert_eq!(permanent.suffix(), "!");
    assert_eq!(permanent.values(), &[1, 2, 3]);
    assert!(!permanent.ptr_eq(&shared_static));
}

struct NoClone;

#[datarc::record]
struct NonClone {
    value: NoClone,
    items: [NoClone],
}

/// Checks that non-Clone values support construction and shared owner cloning.
#[test]
fn non_clone_payloads_support_ownership_and_getters() {
    let owner = NonClone::new(NoClone, [NoClone, NoClone]);
    let clone = owner.clone();
    assert!(std::ptr::eq(clone.value(), owner.value()));
    assert!(std::ptr::eq(clone.items(), owner.items()));
    assert_eq!(clone.items().len(), 2);
}

#[datarc::record]
#[derive(Debug)]
struct Generic<'data, T, const N: usize = 2>
where
    T: std::fmt::Debug,
{
    value: T,
    payload: &'data str,
    rows: [[u8; N]],
    fixed: [u8; N],
}

/// Checks type and const generics and borrowed payload lifetimes.
#[test]
fn generic_record_preserves_borrowed_payload_lifetimes() {
    let text = String::from("payload");
    let owner = Generic::new(17, text.as_str(), [[1, 2], [3, 4]], [5, 6]);
    assert_eq!(*owner.value(), 17);
    assert_eq!(*owner.payload(), "payload");
    assert_eq!(owner.rows(), &[[1, 2], [3, 4]]);
    assert_eq!(owner.fixed(), &[5, 6]);
    let shared = owner.clone();
    assert!(std::ptr::eq(shared.rows(), owner.rows()));
    assert_eq!(std::mem::size_of_val(&shared), std::mem::size_of::<usize>());
}

#[datarc::record]
struct Separate<'a, 'b> {
    first: &'a u32,
    second: &'b [u32],
}

/// Checks that fields can retain independent payload lifetimes.
#[test]
fn independent_payload_lifetimes_are_preserved() {
    let first = 5;
    let second = [6, 7];
    let owner = Separate::new(&first, &second);
    assert!(std::ptr::eq(*owner.first(), &first));
    assert!(std::ptr::eq(*owner.second(), second.as_slice()));
}

#[datarc::record]
struct Empty {}

#[datarc::record]
struct Fixed {
    value: String,
}

/// Checks construction of empty records and fixed-only records.
#[test]
fn empty_and_fixed_only_records() {
    let owner = Empty::new();
    assert!(owner.ptr_eq(&owner.clone()));
    let owner = Fixed::new("fixed".into());
    let copy = owner.clone();
    assert_eq!(copy.value(), "fixed");
}

#[datarc::record]
struct Compact {
    #[datarc(len = u8)]
    bytes: [u8],
    #[datarc(len = u8)]
    text: str,
}

/// Checks that constructors reject lengths that exceed the configured type.
#[test]
fn construction_rejects_length_errors() {
    assert!(Compact::try_new(vec![0; 256], "").is_err());
    assert!(std::panic::catch_unwind(|| Compact::new(vec![0; 256], "")).is_err());
    let text = "é".repeat(128);
    assert!(matches!(
        Compact::try_new([], &text),
        Err(datarc::BuildError::LengthOverflow {
            field: "text",
            len: 256,
            length_type: "u8"
        })
    ));
    let owner = Compact::new([], "é".repeat(127));
    assert_eq!(owner.text().len(), 254);
}

struct Counted {
    live: Arc<AtomicUsize>,
    clones: Arc<AtomicUsize>,
}

impl Counted {
    fn new(live: &Arc<AtomicUsize>, clones: &Arc<AtomicUsize>) -> Self {
        live.fetch_add(1, Ordering::Relaxed);
        Self {
            live: live.clone(),
            clones: clones.clone(),
        }
    }
}

impl Clone for Counted {
    fn clone(&self) -> Self {
        self.clones.fetch_add(1, Ordering::Relaxed);
        Self::new(&self.live, &self.clones)
    }
}

impl Drop for Counted {
    fn drop(&mut self) {
        self.live.fetch_sub(1, Ordering::Relaxed);
    }
}

#[datarc::record]
struct CountedRecord {
    fixed: Counted,
    first: [Counted],
    second: [Counted],
}

/// Checks that construction moves payloads and owner cloning shares them.
#[test]
fn construction_and_owner_clone_do_not_clone_payloads() {
    let live = Arc::new(AtomicUsize::new(0));
    let clones = Arc::new(AtomicUsize::new(0));
    let owner = CountedRecord::new(
        Counted::new(&live, &clones),
        [Counted::new(&live, &clones)],
        [Counted::new(&live, &clones)],
    );
    assert_eq!(live.load(Ordering::Relaxed), 3);
    assert_eq!(clones.load(Ordering::Relaxed), 0);
    let shared = owner.clone();
    assert!(std::ptr::eq(shared.fixed(), owner.fixed()));
    assert!(std::ptr::eq(shared.first(), owner.first()));
    assert!(std::ptr::eq(shared.second(), owner.second()));
    assert_eq!(clones.load(Ordering::Relaxed), 0);
    drop(owner);
    assert_eq!(live.load(Ordering::Relaxed), 3);
    drop(shared);
    assert_eq!(live.load(Ordering::Relaxed), 0);
}

#[datarc::record]
struct AccessorName {
    as_view: u32,
}

/// Checks that a field named as_view receives a working accessor.
#[test]
fn as_view_is_available_as_a_field_name() {
    let owner = AccessorName::new(42);
    assert_eq!(*owner.as_view(), 42);
    assert!(std::ptr::eq(owner.clone().as_view(), owner.as_view()));
}
