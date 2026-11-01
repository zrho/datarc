use datarc::Packable;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[derive(Packable, Debug)]
pub struct Record<'a> {
    pub id: &'a u64,
    #[datarc(len = u16)]
    pub values: &'a [i32],
}

/// Checks pointer-sized default owners, copying conversions from views, and borrowing
/// through From and generic Packable bounds.
#[test]
fn borrowed_view_and_owned_record_roundtrip() {
    let id = 42;
    let values = [1, 2, 3];
    let view = Record {
        id: &id,
        values: &values,
    };
    let owner: <Record<'_> as Packable>::Packed = RecordPacked::from(&view);
    assert_eq!(
        std::mem::size_of::<RecordPacked>(),
        std::mem::size_of::<usize>()
    );
    assert_eq!(*owner.id(), 42);
    assert_eq!(owner.values(), &values);
    let borrowed = Record::from(&owner);
    assert!(std::ptr::eq(borrowed.id, owner.id()));
    assert!(std::ptr::eq(borrowed.values, owner.values()));
    let borrowed_from: Record<'_> = (&owner).into();
    assert!(std::ptr::eq(borrowed_from.values, owner.values()));
    let copied = RecordPacked::from(view);
    assert!(!std::ptr::eq(copied.values(), owner.values()));
    assert_eq!(copied.values(), owner.values());
    fn generic_view<'a, V: Packable + From<&'a V::Packed>>(owner: &'a V::Packed) -> V {
        owner.into()
    }
    assert_eq!(generic_view::<Record<'_>>(&owner).id, &42);
}

struct NoClone;

#[derive(Packable)]
struct NonClone<'a> {
    value: &'a NoClone,
    items: &'a [NoClone],
}

/// Checks that non-Clone values support direct construction, shared owner cloning, and
/// borrowing without copying payloads.
#[test]
fn non_clone_referents_still_support_ownership_and_borrowing() {
    let owner = NonClonePacked::new(NoClone, [NoClone, NoClone]);
    let clone = owner.clone();
    let view: NonClone<'_> = (&clone).into();
    assert!(std::ptr::eq(view.value, owner.value()));
    assert!(std::ptr::eq(view.items, owner.items()));
    assert_eq!(view.items.len(), 2);
}

#[derive(Packable, Debug)]
#[datarc(owned = GenericOwner)]
#[repr(C)]
struct Generic<'view, 'data: 'view, T: 'view, const N: usize = 2>
where
    T: std::fmt::Debug + 'view,
{
    value: &'view T,
    payload: &'view &'data str,
    rows: &'view [[u8; N]],
    fixed: &'view [u8; N],
}

/// Checks that custom owner names, type and const generics, and payload lifetimes
/// survive construction and view conversions.
#[test]
fn generic_view_lifetime_is_separate_from_borrowed_payload_lifetimes() {
    let text = String::from("payload");
    let owner = GenericOwner::new(17, text.as_str(), [[1, 2], [3, 4]], [5, 6]);
    let view = Generic::from(&owner);
    assert_eq!(*view.value, 17);
    assert_eq!(*view.payload, "payload");
    assert_eq!(view.rows, &[[1, 2], [3, 4]]);
    assert_eq!(view.fixed, &[5, 6]);
    let copied = GenericOwner::from(&view);
    assert_eq!(copied.rows(), view.rows);
    assert_eq!(*copied.payload(), "payload");
    assert!(!std::ptr::eq(copied.rows(), view.rows));
    assert_eq!(std::mem::size_of_val(&copied), std::mem::size_of::<usize>());
}

#[derive(Packable)]
struct Separate<'a, 'b> {
    first: &'a u32,
    second: &'b [u32],
}

/// Checks that fields with separate source lifetimes can be copied into one owner and
/// borrowed back together.
#[test]
fn independent_source_lifetimes_are_reborrowed_from_one_owner() {
    let first = 5;
    let source = Separate {
        first: &first,
        second: &[6, 7],
    };
    let owner = SeparatePacked::from(source);
    assert_eq!(Separate::from(&owner).first, &5);
    assert_eq!(Separate::from(&owner).second, &[6, 7]);
}

#[derive(Packable)]
struct Empty {}

#[derive(Packable)]
struct Fixed<'a> {
    value: &'a String,
}

/// Checks view conversions for empty records and records containing only a fixed-size
/// value.
#[test]
fn empty_and_fixed_only_views() {
    let owner = EmptyPacked::from(Empty {});
    let _: Empty = (&owner).into();
    let owner = FixedPacked::new("fixed".into());
    let copy = FixedPacked::from(Fixed::from(&owner));
    assert_eq!(Fixed::from(&copy).value, "fixed");
}

#[derive(Packable)]
struct Compact<'a> {
    #[datarc(len = u8)]
    bytes: &'a [u8],
}

/// Checks that both owned-view and borrowed-view conversions panic when a slice exceeds
/// its configured length type.
#[test]
fn cloning_conversion_panics_on_length_errors() {
    let bytes = vec![0; 256];
    let view = Compact { bytes: &bytes };
    assert!(std::panic::catch_unwind(|| CompactPacked::from(&view)).is_err());
    assert!(std::panic::catch_unwind(|| CompactPacked::from(view)).is_err());
}

struct Counted {
    live: Arc<AtomicUsize>,
    clones: Arc<AtomicUsize>,
    panic_at: usize,
}

impl Counted {
    fn new(live: &Arc<AtomicUsize>, clones: &Arc<AtomicUsize>, panic_at: usize) -> Self {
        live.fetch_add(1, Ordering::Relaxed);
        Self {
            live: live.clone(),
            clones: clones.clone(),
            panic_at,
        }
    }
}

impl Clone for Counted {
    fn clone(&self) -> Self {
        let number = self.clones.fetch_add(1, Ordering::Relaxed);
        assert_ne!(number, self.panic_at, "intentional clone panic");
        Self::new(&self.live, &self.clones, self.panic_at)
    }
}

impl Drop for Counted {
    fn drop(&mut self) {
        self.live.fetch_sub(1, Ordering::Relaxed);
    }
}

#[derive(Packable)]
struct CloneView<'a> {
    fixed: &'a Counted,
    first: &'a [Counted],
    second: &'a [Counted],
}

/// Checks that copying a view clones each payload once, while borrowing and cloning the
/// owner only share those payloads.
#[test]
fn conversion_clones_values_but_borrowing_and_owner_clone_do_not() {
    let live = Arc::new(AtomicUsize::new(0));
    let clones = Arc::new(AtomicUsize::new(0));
    let fixed = Counted::new(&live, &clones, usize::MAX);
    let first = [Counted::new(&live, &clones, usize::MAX)];
    let second = [Counted::new(&live, &clones, usize::MAX)];
    let source = CloneView {
        fixed: &fixed,
        first: &first,
        second: &second,
    };
    let owner = CloneViewPacked::from(&source);
    assert_eq!(live.load(Ordering::Relaxed), 6);
    assert_eq!(clones.load(Ordering::Relaxed), 3);
    let shared = owner.clone();
    let view = CloneView::from(&shared);
    assert!(std::ptr::eq(view.fixed, owner.fixed()));
    assert!(std::ptr::eq(view.first, owner.first()));
    assert!(std::ptr::eq(view.second, owner.second()));
    assert_eq!(clones.load(Ordering::Relaxed), 3);
    drop(owner);
    assert_eq!(live.load(Ordering::Relaxed), 6);
    drop(shared);
    assert_eq!(live.load(Ordering::Relaxed), 3);
}

/// Checks that a panic at each payload-cloning stage drops partial copies and leaves
/// every source value alive.
#[test]
fn clone_panics_release_every_partial_clone_and_preserve_source() {
    for panic_at in [0, 1, 2, 3] {
        let live = Arc::new(AtomicUsize::new(0));
        let clones = Arc::new(AtomicUsize::new(0));
        let fixed = Counted::new(&live, &clones, panic_at);
        let first = [
            Counted::new(&live, &clones, panic_at),
            Counted::new(&live, &clones, panic_at),
        ];
        let second = [Counted::new(&live, &clones, panic_at)];
        let source = CloneView {
            fixed: &fixed,
            first: &first,
            second: &second,
        };
        assert!(std::panic::catch_unwind(|| CloneViewPacked::from(&source)).is_err());
        assert_eq!(live.load(Ordering::Relaxed), 4);
        assert_eq!(source.first.len(), 2);
    }
}

#[derive(Packable)]
struct FormerlyReserved<'a> {
    as_view: &'a u32,
}

/// Checks that a field named as_view receives a working accessor and remains available
/// in borrowed views.
#[test]
fn as_view_is_available_as_a_field_name() {
    let owner = FormerlyReservedPacked::new(42);
    let view: FormerlyReserved<'_> = (&owner).into();
    assert_eq!(*view.as_view, 42);
    assert!(std::ptr::eq(view.as_view, owner.as_view()));
}
