use std::cell::Cell;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

#[derive(Default)]
struct Calls {
    eq: Cell<usize>,
    hash: Cell<usize>,
}

struct Key {
    value: u64,
    calls: Rc<Calls>,
}

fn key(value: u64, calls: &Rc<Calls>) -> Key {
    Key {
        value,
        calls: calls.clone(),
    }
}

impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        self.calls.eq.set(self.calls.eq.get() + 1);
        self.value == other.value
    }
}
impl Eq for Key {}

impl Hash for Key {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.calls.hash.set(self.calls.hash.get() + 1);
        state.write_u64(self.value);
    }
}

#[derive(Default)]
struct ParityHasher(u64);

impl Hasher for ParityHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        self.0 = bytes.iter().map(|byte| u64::from(*byte)).sum::<u64>() % 2;
    }
    fn write_u64(&mut self, value: u64) {
        self.0 = value % 2;
    }
}

macro_rules! fixture {
    ($owner:ident, $($options:tt)*) => {
        #[datarc::record]
        #[derive(PartialEq, Eq)]
        #[datarc($($options)*)]
        struct $owner { key: Key }
    };
}

fixture!(Pointer, eq(ptr_eq));
fixture!(HashOnly, hash, hasher = ParityHasher, eq(hash));
fixture!(Combined, hash, hasher = ParityHasher, eq(ptr_eq, hash));
fixture!(DefaultEquality, hash, hasher = ParityHasher);

#[datarc::record]
#[derive(PartialEq, Hash)]
#[datarc(eq(hash), hash)]
struct PartialOnly {
    key: u64,
}

#[test]
fn hash_shortcut_accepts_partial_eq_without_eq() {
    let first = PartialOnly::new(1);
    assert!(first == PartialOnly::new(1));
    assert!(first != PartialOnly::new(2));
}

#[test]
fn pointer_shortcut_skips_field_comparison_only_for_shared_allocations() {
    let calls = Rc::new(Calls::default());
    let first = Pointer::new(key(1, &calls));
    let shared = first.clone();
    let equal = Pointer::new(key(1, &calls));
    let different = Pointer::new(key(2, &calls));
    assert!(first.eq(&shared));
    assert!(!first.ne(&shared));
    assert_eq!(calls.eq.get(), 0);
    assert!(first.eq(&equal));
    assert!(!first.ne(&equal));
    assert!(!first.eq(&different));
    assert!(first.ne(&different));
    assert_eq!(calls.eq.get(), 4);
    assert_eq!(calls.hash.get(), 0);
}

macro_rules! hash_cases {
    ($owner:ident, $pointer:literal) => {{
        let calls = Rc::new(Calls::default());
        let first = $owner::new(key(1, &calls));
        let shared = first.clone();
        let equal = $owner::new(key(1, &calls));
        let different = $owner::new(key(2, &calls));
        let collision = $owner::new(key(3, &calls));
        assert_eq!(first.cached_hash(), collision.cached_hash());
        assert_ne!(first.cached_hash(), different.cached_hash());
        assert_eq!(calls.hash.get(), 4);
        assert!(!first.eq(&different));
        assert!(first.ne(&different));
        assert_eq!(calls.eq.get(), 0);
        assert!(first.eq(&shared));
        assert!(!first.ne(&shared));
        let shared_calls = if $pointer { 0 } else { 2 };
        assert_eq!(calls.eq.get(), shared_calls);
        assert!(first.eq(&equal));
        assert!(!first.ne(&equal));
        assert!(!first.eq(&collision));
        assert!(first.ne(&collision));
        assert_eq!(calls.eq.get(), shared_calls + 4);
        assert_eq!(calls.hash.get(), 4);
    }};
}

#[test]
fn hash_shortcut_rejects_mismatches_and_compares_collisions() {
    hash_cases!(HashOnly, false);
}

#[test]
fn combined_shortcuts_preserve_collision_fallback() {
    hash_cases!(Combined, true);
}

#[test]
fn caching_alone_keeps_comparing_fields() {
    let calls = Rc::new(Calls::default());
    let first = DefaultEquality::new(key(1, &calls));
    let different = DefaultEquality::new(key(2, &calls));
    assert!(first.eq(&first.clone()));
    assert!(!first.ne(&first.clone()));
    assert!(!first.eq(&different));
    assert!(first.ne(&different));
    assert_eq!(calls.eq.get(), 4);
}

#[datarc::record]
struct Opaque<T> {
    value: T,
}

#[datarc::record]
struct Empty {}

#[test]
fn allocation_identity_needs_no_equality_traits() {
    struct Payload;
    let first = Opaque::new(Payload);
    let equal = Opaque::new(Payload);
    assert!(first.ptr_eq(&first));
    assert!(first.ptr_eq(&first.clone()));
    assert!(!first.ptr_eq(&equal));
    let empty = Empty::new();
    assert!(empty.ptr_eq(&empty.clone()));
    assert!(!empty.ptr_eq(&Empty::new()));
    assert_eq!(std::mem::size_of_val(&first), std::mem::size_of::<usize>());
    assert_eq!(std::mem::size_of_val(&empty), std::mem::size_of::<usize>());
    // SAFETY: the allocator returns unique aligned storage retained forever.
    let immortal = unsafe { Empty::new_static(|layout| std::alloc::alloc(layout)) };
    assert!(immortal.ptr_eq(&immortal.clone()));
    assert!(!immortal.ptr_eq(&empty));
}

#[datarc::record]
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
#[datarc(eq(ptr_eq))]
struct Generic<'data, T, const N: usize> {
    value: T,
    text: &'data str,
    rows: [[u8; N]],
}

#[test]
fn pointer_option_preserves_conditional_generic_ordering() {
    let text = String::from("borrowed");
    let first = Generic::new(1, text.as_str(), [[2, 3]]);
    let equal = Generic::new(1, text.as_str(), [[2, 3]]);
    let later = Generic::new(2, text.as_str(), [[2, 3]]);
    assert!(first.ptr_eq(&first.clone()));
    assert_eq!(first, equal);
    assert!(first < later);
    assert_eq!(first.cmp(&equal), std::cmp::Ordering::Equal);
    assert_eq!(first.partial_cmp(&later), Some(std::cmp::Ordering::Less));
    let partial_only = Generic::new(f64::NAN, text.as_str(), [[2, 3]]);
    assert!(partial_only.ptr_eq(&partial_only.clone()));
}

#[test]
fn ptr_eq_is_available_outside_the_defining_module() {
    mod inner {
        #[datarc::record]
        pub struct Public {
            value: u8,
        }
    }
    let value = inner::Public::new(1);
    assert!(value.ptr_eq(&value.clone()));
}
