#![deny(unsafe_op_in_unsafe_fn)]
use datarc::{Packable, PackedPointer};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

struct Counted(Arc<AtomicUsize>);
impl Drop for Counted {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Packable)]
struct Record<'a> {
    value: &'a Counted,
    bytes: &'a [u8],
}

#[derive(Packable)]
struct Empty {}

#[derive(Packable)]
enum Value {
    Record(RecordPacked),
    Empty(EmptyPacked),
}

/// Checks pointer-sized packed enums and their Options, shared payload addresses,
/// conversion round trips, and destruction after the last owner.
#[test]
fn packed_ownership_and_borrowing() {
    assert_eq!(
        std::mem::size_of::<ValuePacked>(),
        std::mem::size_of::<usize>()
    );
    assert_eq!(
        std::mem::size_of::<Option<ValuePacked>>(),
        std::mem::size_of::<usize>()
    );
    let drops = Arc::new(AtomicUsize::new(0));
    let record = RecordPacked::new(Counted(drops.clone()), [1, 2, 3]);
    let original_address = record.bytes().as_ptr();
    let packed: <Value as Packable>::Packed = Value::Record(record).into();
    packed.with_enum(|value| match value {
        Value::Record(record) => assert_eq!(record.bytes().as_ptr(), original_address),
        _ => panic!("wrong variant"),
    });
    let clone = packed.clone();
    let ordinary = Value::from(&packed);
    let from_borrowed_enum = ValuePacked::from(&ordinary);
    assert!(matches!(Value::from(from_borrowed_enum), Value::Record(_)));
    drop(packed);
    assert_eq!(drops.load(Ordering::Relaxed), 0);
    let unpacked: Value = clone.into();
    match &unpacked {
        Value::Record(record) => assert_eq!(record.bytes().as_ptr(), original_address),
        _ => panic!("wrong variant"),
    }
    drop(unpacked);
    assert_eq!(drops.load(Ordering::Relaxed), 0);
    drop(ordinary);
    assert_eq!(drops.load(Ordering::Relaxed), 1);
    let empty = ValuePacked::from(Value::Empty(EmptyPacked::new()));
    assert!(matches!(empty.clone().into_enum(), Value::Empty(_)));
    assert!(matches!(empty.into_enum(), Value::Empty(_)));
}

#[derive(Packable)]
struct Borrowed<'v, 'data, T, const N: usize> {
    text: &'v &'data str,
    value: &'v T,
    bytes: &'v [u8; N],
}
#[derive(Packable)]
#[datarc(packed = GenericPacked)]
enum Generic<'data, T, const N: usize = 2>
where
    T: std::fmt::Debug,
{
    First(BorrowedPacked<'data, T, N>),
    Second(BorrowedPacked<'data, T, N>),
}

/// Checks that packed enums preserve borrowed payloads and type and const generics, and
/// implement Send and Sync for suitable payloads.
#[test]
fn generic_borrowed_payloads_and_threads() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<GenericPacked<'static, u32>>();
    let text = String::from("borrowed");
    let packed = GenericPacked::from(Generic::Second(BorrowedPacked::new(
        text.as_str(),
        42u32,
        [1, 2],
    )));
    assert!(matches!(packed.clone().into_enum(), Generic::Second(_)));
    let original = packed.into_enum();
    if let Generic::Second(record) = original {
        assert_eq!(*record.text(), text);
    }
}

#[derive(Packable)]
enum Sixteen {
    V0(EmptyPacked),
    V1(EmptyPacked),
    V2(EmptyPacked),
    V3(EmptyPacked),
    V4(EmptyPacked),
    V5(EmptyPacked),
    V6(EmptyPacked),
    V7(EmptyPacked),
    V8(EmptyPacked),
    V9(EmptyPacked),
    V10(EmptyPacked),
    V11(EmptyPacked),
    V12(EmptyPacked),
    V13(EmptyPacked),
    V14(EmptyPacked),
    V15(EmptyPacked),
}

/// Checks that all 16 variant tags survive packed enum cloning and conversion back to
/// the original enum.
#[test]
fn all_four_tag_bits() {
    let constructors: [fn(EmptyPacked) -> Sixteen; 16] = [
        Sixteen::V0,
        Sixteen::V1,
        Sixteen::V2,
        Sixteen::V3,
        Sixteen::V4,
        Sixteen::V5,
        Sixteen::V6,
        Sixteen::V7,
        Sixteen::V8,
        Sixteen::V9,
        Sixteen::V10,
        Sixteen::V11,
        Sixteen::V12,
        Sixteen::V13,
        Sixteen::V14,
        Sixteen::V15,
    ];
    for constructor in constructors {
        let original = constructor(EmptyPacked::new());
        let discriminant = std::mem::discriminant(&original);
        let packed = SixteenPacked::from(original);
        assert_eq!(
            std::mem::discriminant(&packed.clone().into_enum()),
            discriminant
        );
        assert_eq!(std::mem::discriminant(&packed.into_enum()), discriminant);
    }
}

#[repr(align(64))]
struct Aligned;
#[derive(Packable)]
struct HighAlign<'a> {
    value: &'a Aligned,
}

/// Checks the minimum 16-byte pointer alignment, stronger payload alignment, and
/// ownership restoration through PackedPointer.
#[test]
fn alignment_and_raw_ownership_roundtrip() {
    let ptr = EmptyPacked::new().into_raw();
    assert_eq!(ptr.addr().get() % 16, 0);
    // SAFETY: restore the unit transferred above, exactly once.
    drop(unsafe { EmptyPacked::from_raw(ptr) });
    let ptr = HighAlignPacked::new(Aligned).into_raw();
    assert_eq!(ptr.addr().get() % 64, 0);
    // SAFETY: restore the unit transferred above, exactly once.
    drop(unsafe { HighAlignPacked::from_raw(ptr) });
}

/// Checks that packed static records request aligned storage and remain immortal even
/// when borrowed inspection panics.
#[test]
fn static_owners_and_panicking_inspection() {
    let drops = Arc::new(AtomicUsize::new(0));
    // SAFETY: the global allocation is deliberately retained forever.
    let owner = unsafe {
        RecordPacked::new_static(
            |layout| {
                assert!(layout.align() >= 16);
                std::alloc::alloc(layout)
            },
            Counted(drops.clone()),
            [],
        )
    };
    let packed = ValuePacked::from(Value::Record(owner));
    assert!(std::panic::catch_unwind(|| packed.with_enum(|_| panic!("inspection"))).is_err());
    drop(packed.clone().into_enum());
    drop(packed);
    assert_eq!(drops.load(Ordering::Relaxed), 0);
}

/// Checks that generic Packable functions can pack and unpack enums using associated
/// types and From bounds.
#[test]
fn enum_trait_supports_generic_conversion_roundtrips() {
    fn pack<E: Packable>(value: E) -> E::Packed
    where
        E::Packed: From<E>,
    {
        E::Packed::from(value)
    }
    fn unpack<E: Packable + From<E::Packed>>(packed: E::Packed) -> E {
        E::from(packed)
    }
    let packed = pack(Value::Empty(EmptyPacked::new()));
    assert!(matches!(unpack::<Value>(packed), Value::Empty(_)));
}
