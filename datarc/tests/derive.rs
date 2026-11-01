use datarc::{ArcData, DataArc};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[derive(DataArc, Debug)]
pub struct Record<'a> {
    pub id: &'a u64,
    #[datarc(len = u16)]
    pub values: &'a [i32],
}

#[test]
fn borrowed_view_and_owned_record_roundtrip() {
    let id = 42;
    let values = [1, 2, 3];
    let view = Record {
        id: &id,
        values: &values,
    };
    let owner: <Record<'_> as DataArc>::Owned = RecordArc::try_from(&view).unwrap();
    assert_eq!(
        std::mem::size_of::<RecordArc>(),
        std::mem::size_of::<usize>()
    );
    assert_eq!(*owner.id(), 42);
    assert_eq!(owner.values(), &values);
    let borrowed = owner.as_view();
    assert!(std::ptr::eq(borrowed.id, owner.id()));
    assert!(std::ptr::eq(borrowed.values, owner.values()));
    let borrowed_from: Record<'_> = (&owner).into();
    assert!(std::ptr::eq(borrowed_from.values, owner.values()));
    let copied = RecordArc::try_from(view).unwrap();
    assert!(!std::ptr::eq(copied.values(), owner.values()));
    assert_eq!(copied.values(), owner.values());
    fn generic_view<A: ArcData>(owner: &A) -> A::View<'_> {
        owner.as_view()
    }
    assert_eq!(generic_view(&owner).id, &42);
}

struct NoClone;

#[derive(DataArc)]
struct NonClone<'a> {
    value: &'a NoClone,
    items: &'a [NoClone],
}

#[test]
fn non_clone_referents_still_support_ownership_and_borrowing() {
    let owner = NonCloneArc::new(NoClone, [NoClone, NoClone]);
    let clone = owner.clone();
    let view: NonClone<'_> = (&clone).into();
    assert!(std::ptr::eq(view.value, owner.value()));
    assert!(std::ptr::eq(view.items, owner.items()));
    assert_eq!(view.items.len(), 2);
}

#[derive(DataArc, Debug)]
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

#[test]
fn generic_view_lifetime_is_separate_from_borrowed_payload_lifetimes() {
    let text = String::from("payload");
    let owner = GenericOwner::new(17, text.as_str(), [[1, 2], [3, 4]], [5, 6]);
    let view = owner.as_view();
    assert_eq!(*view.value, 17);
    assert_eq!(*view.payload, "payload");
    assert_eq!(view.rows, &[[1, 2], [3, 4]]);
    assert_eq!(view.fixed, &[5, 6]);
    let copied = GenericOwner::try_from(&view).unwrap();
    assert_eq!(copied.rows(), view.rows);
    assert_eq!(*copied.payload(), "payload");
    assert!(!std::ptr::eq(copied.rows(), view.rows));
    assert_eq!(std::mem::size_of_val(&copied), std::mem::size_of::<usize>());
}

#[derive(DataArc)]
struct Separate<'a, 'b> {
    first: &'a u32,
    second: &'b [u32],
}

#[test]
fn independent_source_lifetimes_are_reborrowed_from_one_owner() {
    let first = 5;
    let source = Separate {
        first: &first,
        second: &[6, 7],
    };
    let owner = SeparateArc::try_from(source).unwrap();
    assert_eq!(owner.as_view().first, &5);
    assert_eq!(owner.as_view().second, &[6, 7]);
}

#[derive(DataArc)]
struct Empty {}

#[derive(DataArc)]
struct Fixed<'a> {
    value: &'a String,
}

#[test]
fn empty_and_fixed_only_views() {
    let owner = EmptyArc::try_from(Empty {}).unwrap();
    let _: Empty = (&owner).into();
    let owner = FixedArc::new("fixed".into());
    let copy = FixedArc::try_from(owner.as_view()).unwrap();
    assert_eq!(copy.as_view().value, "fixed");
}

#[derive(DataArc)]
struct Compact<'a> {
    #[datarc(len = u8)]
    bytes: &'a [u8],
}

#[test]
fn cloning_conversion_reports_length_errors() {
    let bytes = vec![0; 256];
    let view = Compact { bytes: &bytes };
    assert!(matches!(
        CompactArc::try_from(&view),
        Err(datarc::BuildError::LengthOverflow {
            field: "bytes",
            len: 256,
            ..
        })
    ));
    assert!(matches!(
        CompactArc::try_from(view),
        Err(datarc::BuildError::LengthOverflow {
            field: "bytes",
            len: 256,
            ..
        })
    ));
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

#[derive(DataArc)]
struct CloneView<'a> {
    fixed: &'a Counted,
    first: &'a [Counted],
    second: &'a [Counted],
}

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
    let owner = CloneViewArc::try_from(&source).unwrap();
    assert_eq!(live.load(Ordering::Relaxed), 6);
    assert_eq!(clones.load(Ordering::Relaxed), 3);
    let shared = owner.clone();
    let view = shared.as_view();
    assert!(std::ptr::eq(view.fixed, owner.fixed()));
    assert!(std::ptr::eq(view.first, owner.first()));
    assert!(std::ptr::eq(view.second, owner.second()));
    assert_eq!(clones.load(Ordering::Relaxed), 3);
    drop(owner);
    assert_eq!(live.load(Ordering::Relaxed), 6);
    drop(shared);
    assert_eq!(live.load(Ordering::Relaxed), 3);
}

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
        assert!(std::panic::catch_unwind(|| CloneViewArc::try_from(&source)).is_err());
        assert_eq!(live.load(Ordering::Relaxed), 4);
        assert_eq!(source.first.len(), 2);
    }
}
