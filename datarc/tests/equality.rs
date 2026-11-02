use datarc::Packable;
use std::cell::Cell;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

#[derive(Default)]
struct Calls {
    eq: Cell<usize>,
    ne: Cell<usize>,
    hash: Cell<usize>,
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

// Payloads deliberately lack equality and hashing: only the view defines them.
macro_rules! fixture {
    ($view:ident, $($options:tt)*) => {
        #[derive(Packable)]
        #[datarc($($options)*)]
        struct $view<'a> {
            key: &'a u64,
            calls: &'a Rc<Calls>,
        }

        impl PartialEq for $view<'_> {
            fn eq(&self, other: &Self) -> bool {
                self.calls.eq.set(self.calls.eq.get() + 1);
                self.key == other.key
            }

            fn ne(&self, other: &Self) -> bool {
                self.calls.ne.set(self.calls.ne.get() + 1);
                self.key != other.key
            }
        }

        impl Eq for $view<'_> {}

        impl Hash for $view<'_> {
            fn hash<H: Hasher>(&self, state: &mut H) {
                self.calls.hash.set(self.calls.hash.get() + 1);
                state.write_u64(*self.key);
            }
        }
    };
}

fixture!(Pointer, eq(ptr_eq));
fixture!(HashOnly, hash, hasher = ParityHasher, eq(hash));
fixture!(Combined, hash, hasher = ParityHasher, eq(ptr_eq, hash));
fixture!(DefaultEquality, hash, hasher = ParityHasher);

#[derive(Packable, PartialEq, Hash)]
#[datarc(eq(hash), hash)]
struct PartialOnly<'a> {
    key: &'a u64,
}

#[test]
fn hash_shortcut_accepts_partial_eq_without_eq() {
    let first = PartialOnlyPacked::new(1);
    assert!(first == PartialOnlyPacked::new(1));
    assert!(first != PartialOnlyPacked::new(2));
}

#[test]
fn pointer_shortcut_skips_both_methods_only_for_shared_allocations() {
    let calls = Rc::new(Calls::default());
    let first = PointerPacked::new(1, calls.clone());
    let shared = first.clone();
    let equal = PointerPacked::new(1, calls.clone());
    let different = PointerPacked::new(2, calls.clone());
    assert!(first.eq(&shared));
    assert!(!first.ne(&shared));
    assert_eq!((calls.eq.get(), calls.ne.get()), (0, 0));
    assert!(first.eq(&equal));
    assert!(!first.ne(&equal));
    assert!(!first.eq(&different));
    assert!(first.ne(&different));
    assert_eq!((calls.eq.get(), calls.ne.get()), (2, 2));
    assert_eq!(calls.hash.get(), 0);
}

macro_rules! hash_cases {
    ($owner:ident, $pointer:literal) => {{
        let calls = Rc::new(Calls::default());
        let first = $owner::new(1, calls.clone());
        let shared = first.clone();
        let equal = $owner::new(1, calls.clone());
        let different = $owner::new(2, calls.clone());
        let collision = $owner::new(3, calls.clone());
        assert_eq!(first.cached_hash(), collision.cached_hash());
        assert_ne!(first.cached_hash(), different.cached_hash());
        assert_eq!(calls.hash.get(), 4);

        assert!(!first.eq(&different));
        assert!(first.ne(&different));
        assert_eq!((calls.eq.get(), calls.ne.get()), (0, 0));
        assert!(first.eq(&shared));
        assert!(!first.ne(&shared));
        let shared_calls = if $pointer { 0 } else { 1 };
        assert_eq!(
            (calls.eq.get(), calls.ne.get()),
            (shared_calls, shared_calls)
        );
        assert!(first.eq(&equal));
        assert!(!first.ne(&equal));
        assert!(!first.eq(&collision));
        assert!(first.ne(&collision));
        assert_eq!(
            (calls.eq.get(), calls.ne.get()),
            (shared_calls + 2, shared_calls + 2)
        );
        assert_eq!(calls.hash.get(), 4);
    }};
}

#[test]
fn hash_shortcut_rejects_mismatches_and_compares_collisions() {
    hash_cases!(HashOnlyPacked, false);
}

#[test]
fn combined_shortcuts_preserve_collision_fallback() {
    hash_cases!(CombinedPacked, true);
}

#[test]
fn caching_alone_keeps_delegating_eq_and_ne() {
    let calls = Rc::new(Calls::default());
    let first = DefaultEqualityPacked::new(1, calls.clone());
    let different = DefaultEqualityPacked::new(2, calls.clone());
    assert!(first.eq(&first.clone()));
    assert!(!first.ne(&first.clone()));
    assert!(!first.eq(&different));
    assert!(first.ne(&different));
    assert_eq!((calls.eq.get(), calls.ne.get()), (2, 2));
}

#[derive(Packable)]
struct Opaque<'a, T> {
    value: &'a T,
}

#[derive(Packable)]
struct Empty {}

#[test]
fn allocation_identity_needs_no_equality_traits() {
    struct Payload;
    let first = OpaquePacked::new(Payload);
    let equal = OpaquePacked::new(Payload);
    assert!(first.ptr_eq(&first));
    assert!(first.ptr_eq(&first.clone()));
    assert!(!first.ptr_eq(&equal));
    let empty = EmptyPacked::new();
    assert!(empty.ptr_eq(&empty.clone()));
    assert!(!empty.ptr_eq(&EmptyPacked::new()));
    assert_eq!(std::mem::size_of_val(&first), std::mem::size_of::<usize>());
    assert_eq!(std::mem::size_of_val(&empty), std::mem::size_of::<usize>());

    // SAFETY: the allocator returns unique aligned storage retained forever.
    let immortal = unsafe { EmptyPacked::new_static(|layout| std::alloc::alloc(layout)) };
    assert!(immortal.ptr_eq(&immortal.clone()));
    assert!(!immortal.ptr_eq(&empty));
}

#[derive(Packable, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[datarc(eq(ptr_eq))]
struct Generic<'view, 'data, T, const N: usize> {
    value: &'view T,
    text: &'view &'data str,
    rows: &'view [[u8; N]],
}

#[test]
fn pointer_option_preserves_conditional_generic_ordering() {
    let text = String::from("borrowed");
    let first = GenericPacked::new(1, text.as_str(), [[2, 3]]);
    let equal = GenericPacked::new(1, text.as_str(), [[2, 3]]);
    let later = GenericPacked::new(2, text.as_str(), [[2, 3]]);
    assert!(first.ptr_eq(&first.clone()));
    assert_eq!(first, equal);
    assert!(first < later);
    assert_eq!(first.cmp(&equal), std::cmp::Ordering::Equal);
    assert_eq!(first.partial_cmp(&later), Some(std::cmp::Ordering::Less));
    let partial_only = GenericPacked::new(f64::NAN, text.as_str(), [[2, 3]]);
    assert!(partial_only.ptr_eq(&partial_only.clone()));
}

#[test]
fn ptr_eq_is_available_outside_the_defining_module() {
    mod inner {
        use datarc::Packable;
        #[derive(Packable)]
        pub struct Public<'a> {
            value: &'a u8,
        }
    }
    let value = inner::PublicPacked::new(1);
    assert!(value.ptr_eq(&value.clone()));
}
