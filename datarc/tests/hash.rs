use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

thread_local! {
    static WATCH_NEXT_ALLOCATION: Cell<bool> = const { Cell::new(false) };
    static WATCHED_ALLOCATION: Cell<*mut u8> = const { Cell::new(std::ptr::null_mut()) };
    static WATCHED_DEALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

struct TrackingAllocator;

// SAFETY: all operations delegate to System with unchanged arguments. Tracking
// uses allocation-free thread-local cells and observes only the selected pointer.
unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: inherited allocator contract.
        let ptr = unsafe { System.alloc(layout) };
        if WATCH_NEXT_ALLOCATION
            .try_with(|watch| watch.replace(false))
            .unwrap_or(false)
        {
            WATCHED_ALLOCATION.with(|watched| watched.set(ptr));
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if WATCHED_ALLOCATION
            .try_with(|watched| watched.get() == ptr)
            .unwrap_or(false)
        {
            WATCHED_DEALLOCATIONS.with(|count| count.set(count.get() + 1));
            // Stop tracking before another allocation can reuse this address.
            WATCHED_ALLOCATION.with(|watched| watched.set(std::ptr::null_mut()));
        }
        // SAFETY: inherited allocator contract.
        unsafe { System.dealloc(ptr, layout) };
    }
}

#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator;

fn hash_with<H: Hasher + Default>(value: &impl Hash) -> u64 {
    let mut hasher = H::default();
    value.hash(&mut hasher);
    hasher.finish()
}

#[datarc::record]
#[derive(Hash)]
#[datarc(hash)]
struct Record {
    id: u64,
    text: str,
    #[datarc(len = u8)]
    values: [i32],
}

/// Checks that constructors cache the field hash with DefaultHasher, including empty slices.
#[test]
fn default_hasher_matches_fields_for_constructors() {
    let id = 42;
    let values = [3, -7, 11];
    let expected = hash_with::<DefaultHasher>(&(id, "héllo", values.as_slice()));
    let owners = [
        Record::new(id, "héllo", values),
        Record::try_new(id, String::from("héllo"), values).unwrap(),
    ];
    for owner in owners {
        let cached: u64 = owner.cached_hash();
        assert_eq!(cached, expected);
        assert_eq!(
            cached,
            hash_with::<DefaultHasher>(&(owner.id(), owner.text(), owner.values()))
        );
        assert_eq!(owner.clone().cached_hash(), expected);
        assert_eq!(std::mem::size_of_val(&owner), std::mem::size_of::<usize>());
    }
    let empty = Record::new(0, "", []);
    assert_eq!(
        empty.cached_hash(),
        hash_with::<DefaultHasher>(&(empty.id(), empty.text(), empty.values()))
    );
}

// Field hashing defines which payload data contributes to the cache.
#[derive(Clone)]
struct Payload {
    key: u64,
    calls: Arc<AtomicUsize>,
}

#[derive(Default)]
struct CustomHasher(u64);

impl Hasher for CustomHasher {
    fn finish(&self) -> u64 {
        self.0 ^ 0x9e37_79b9_7f4a_7c15
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 = self.0.wrapping_mul(257).wrapping_add(u64::from(*byte));
        }
    }
}

#[datarc::record]
#[derive(Hash)]
#[datarc(hash, hasher = CustomHasher)]
struct Manual {
    payload: Payload,
    values: [u32],
}

impl Hash for Payload {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.calls.fetch_add(1, Ordering::SeqCst);
        "manual payload hash".hash(state);
        self.key.hash(state);
    }
}

/// Checks that custom field hashing runs once per allocation and owner hashing uses the cache.
#[test]
fn custom_manual_hash_runs_once_per_allocation_and_never_on_clone_or_access() {
    let calls = Arc::new(AtomicUsize::new(0));
    let payload = Payload {
        key: 91,
        calls: calls.clone(),
    };
    let values = [8, 13, 21];
    let expected = hash_with::<CustomHasher>(&(&payload, values.as_slice()));
    calls.store(0, Ordering::SeqCst);

    let first = Manual::new(payload.clone(), values);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let second = Manual::try_new(payload.clone(), values).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);

    for owner in [first, second] {
        let shared = owner.clone();
        let another = shared.clone();
        for _ in 0..3 {
            assert_eq!(owner.cached_hash(), expected);
            assert_eq!(shared.cached_hash(), expected);
            assert_eq!(another.cached_hash(), expected);
            assert_eq!(
                hash_with::<DefaultHasher>(&owner),
                hash_with::<DefaultHasher>(&expected)
            );
            assert_eq!(
                hash_with::<CustomHasher>(&shared),
                hash_with::<CustomHasher>(&expected)
            );
            assert_eq!(shared.values(), &values);
        }
        drop(owner);
        assert_eq!(shared.cached_hash(), expected);
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

/// Checks that static construction computes the hash once and immortal owner clones
/// reuse the cached value.
#[test]
fn static_construction_hashes_once_and_static_clones_preserve_cache() {
    let calls = Arc::new(AtomicUsize::new(0));
    let payload = Payload {
        key: 5,
        calls: calls.clone(),
    };
    let expected = hash_with::<CustomHasher>(&(&payload, [2u32, 4].as_slice()));
    calls.store(0, Ordering::SeqCst);
    // SAFETY: the callback supplies fresh aligned storage which is deliberately
    // retained for the rest of the process, as required by new_static.
    let owner = unsafe { Manual::new_static(|layout| std::alloc::alloc(layout), payload, [2, 4]) };
    let shared = owner.clone();
    assert_eq!(owner.cached_hash(), expected);
    assert_eq!(shared.cached_hash(), expected);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    // Static records intentionally never drop their payload. Keep the storage
    // alive for the process, as required by new_static's contract.
    drop((owner, shared));
}

struct DropProbe {
    index: usize,
    drops: Arc<[AtomicUsize; 6]>,
}

impl Drop for DropProbe {
    fn drop(&mut self) {
        self.drops[self.index].fetch_add(1, Ordering::SeqCst);
    }
}

#[datarc::record]
#[datarc(hash)]
struct PanickingHash {
    fixed: DropProbe,
    first: [DropProbe],
    second: [DropProbe],
}

impl Hash for DropProbe {
    fn hash<H: Hasher>(&self, _: &mut H) {
        assert_eq!(self.index, 0);
        for count in self.drops.iter() {
            assert_eq!(count.load(Ordering::SeqCst), 0);
        }
        panic!("field hash panicked");
    }
}

fn check_hash_panic_cleanup(static_storage: bool, fallible: bool) {
    let drops = Arc::new(std::array::from_fn(|_| AtomicUsize::new(0)));
    let probe = |index| DropProbe {
        index,
        drops: drops.clone(),
    };
    let fixed = probe(0);
    let first = [probe(1), probe(2)];
    let second = [probe(3), probe(4), probe(5)];
    let mut static_allocation = None;
    WATCHED_DEALLOCATIONS.with(|count| count.set(0));

    let result = catch_unwind(AssertUnwindSafe(|| {
        if static_storage {
            // SAFETY: the callback supplies fresh aligned storage. On failure,
            // the caller retains this storage and frees it after all cleanup.
            unsafe {
                PanickingHash::new_static(
                    |layout| {
                        WATCH_NEXT_ALLOCATION.with(|watch| watch.set(true));
                        let ptr = std::alloc::alloc(layout);
                        static_allocation = Some((ptr, layout));
                        ptr
                    },
                    fixed,
                    first,
                    second,
                );
            }
        } else {
            WATCH_NEXT_ALLOCATION.with(|watch| watch.set(true));
            if fallible {
                let _ = PanickingHash::try_new(fixed, first, second);
            } else {
                let _ = PanickingHash::new(fixed, first, second);
            }
        }
    }));
    WATCH_NEXT_ALLOCATION.with(|watch| watch.set(false));
    let panic = result.expect_err("the field hash must panic");
    assert_eq!(panic.downcast_ref::<&str>(), Some(&"field hash panicked"));
    for (index, count) in drops.iter().enumerate() {
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "probe {index} must drop exactly once"
        );
    }

    if let Some((ptr, layout)) = static_allocation {
        assert!(!ptr.is_null());
        assert_eq!(WATCHED_ALLOCATION.with(Cell::get), ptr);
        assert_eq!(WATCHED_DEALLOCATIONS.with(Cell::get), 0);
        // SAFETY: construction panicked and destroyed every initialized value.
        // The allocator observer confirms datarc did not free this allocation.
        unsafe { std::alloc::dealloc(ptr, layout) };
        assert_eq!(WATCHED_DEALLOCATIONS.with(Cell::get), 1);
    } else {
        assert!(!static_storage, "the static allocator must have run");
        assert_eq!(WATCHED_DEALLOCATIONS.with(Cell::get), 1);
    }
    WATCHED_ALLOCATION.with(|watched| watched.set(std::ptr::null_mut()));
}

/// Checks that a field-hashing panic in new or try_new drops all initialized values and
/// frees the ordinary allocation.
#[test]
fn hashing_panic_drops_fields_and_tails_and_frees_normal_allocation() {
    check_hash_panic_cleanup(false, false);
    check_hash_panic_cleanup(false, true);
}

/// Checks that a field-hashing panic drops all initialized values but leaves
/// caller-provided static storage for the caller to free.
#[test]
fn hashing_panic_drops_fields_and_tails_without_freeing_static_allocation() {
    check_hash_panic_cleanup(true, false);
}

#[datarc::record]
#[derive(Hash)]
#[datarc(hash)]
struct Generic<'data, T: Hash, const N: usize> {
    label: &'data str,
    fixed: [T; N],
    values: [T],
}

/// Checks cached hashing for records combining type and const generics, borrowed
/// payloads, fixed arrays, and empty slices.
#[test]
fn cached_hash_preserves_type_const_and_payload_lifetime_generics() {
    let text = String::from("borrowed payload");
    let owner = Generic::<u16, 3>::new(text.as_str(), [1, 2, 3], [5, 8]);
    let expected = hash_with::<DefaultHasher>(&(owner.label(), owner.fixed(), owner.values()));
    assert_eq!(owner.cached_hash(), expected);
    let shared = owner.clone();
    assert_eq!(shared.cached_hash(), expected);
    assert_eq!(*shared.label(), text.as_str());
    let empty = Generic::<u8, 0>::try_new(text.as_str(), [], []).unwrap();
    assert_eq!(
        empty.cached_hash(),
        hash_with::<DefaultHasher>(&(empty.label(), empty.fixed(), empty.values()))
    );
}
