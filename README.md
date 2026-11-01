# datarc

`datarc` creates pointer-sized, atomically reference-counted records with multiple
inline, variable-length fields. There is one allocation, one `usize` strong count,
and no weak count. Cloning shares the allocation without cloning its contents.

```rust
use datarc::datarc;

#[datarc]
pub struct Record {
    pub id: u64,
    #[datarc(len = u16)]
    pub values: [i32],
    pub labels: [String],
}

let record = Record::new(42, (1..3).map(|i| i * 10), ["hello".into()]);
let shared = record.clone();
assert_eq!(*shared.id(), 42);
assert_eq!(shared.values(), &[10, 20]);
assert_eq!(shared.labels(), &["hello"]);
assert_eq!(std::mem::size_of::<Record>(), std::mem::size_of::<usize>());
```

Add the `datarc` runtime crate as a dependency; it re-exports the attribute macro
from `datarc-macros`. Use `#[datarc]` on a struct with named fields. The original
name becomes the shared handle. Field visibility and documentation transfer to
the corresponding accessor.

Declare a field as `[T]` to store a dynamic array of `T` and generate an `&[T]`
accessor. Each array has its own length, defaulting to `usize`. Set
`#[datarc(len = u16)]` to choose a length type for that field; supported types are
`u8`, `u16`, `u32`, `u64`, `u128`, and `usize`.

Other fields default to `once` (also spelled `#[datarc(once)]`) and have `&T`
accessors. Fixed arrays `[T; N]` and references `&[T]` remain ordinary fields.
The explicit spelling `#[datarc(many)] field: T` is also supported, including
`len` in the same attribute. An explicit `many` on `[T]` is redundant and accepted;
an explicit `once` on `[T]` is rejected.

Both constructors take arguments in declaration order: a `T` for each ordinary
field and an `impl IntoIterator<Item = T, IntoIter: ExactSizeIterator>` for each
array. Vectors, owned fixed arrays, ranges, and mapped exact-size iterators are
accepted. Elements are generated or moved directly into the final allocation;
no temporary collection or `Clone` bound is required. To copy borrowed elements,
use `.iter().copied()` or `.iter().cloned()` explicitly. Iterators with unknown
lengths, such as filtered streams, must first be collected into a `Vec<T>`.
Previously unannotated `.collect()` calls may need `.collect::<Vec<_>>()` now
that constructors accept multiple collection types.

Lengths are obtained once and checked before allocation or element generation.
Actual item counts are checked as well: `try_new` returns
`BuildError::IteratorLengthMismatch { field, expected }` if an iterator yields
too few or too many elements. At most one excess element is requested.
`try_new` also reports unrepresentable field lengths and allocation layouts;
`new` panics on any of these errors. Both consume their inputs, including on
error. Iterator panics propagate. Already initialized elements and ordinary
fields are destroyed, and the allocation is freed, on either error or unwind.
Allocator exhaustion follows the standard allocator's failure behavior.
Strong-count overflow aborts the process.

The allocation begins with an atomic strong count, followed by the field lengths
and ordinary values, then each array in declaration order with alignment padding.
Accessors have inline hints and compute offsets directly from immutable lengths
of preceding arrays, without repeating construction's layout validation. The
handle stores only a thin pointer. Even empty arrays have properly aligned slice
pointers. The last owner drops every value and releases the allocation, including
when a destructor unwinds (a second destructor panic follows Rust's usual abort
behavior).

Records implement `Clone` without field bounds. They are `Send + Sync` when all
stored types are `Send + Sync`. Access is immutable, though fields can provide
their own interior mutability. Type, lifetime, and const generics are supported.

This initial API does not support tuple/unit structs, custom derives, `repr`
attributes, conditional fields, or field names `new` and `try_new`. Use the record's
name instead of `Self` in field types and generic bounds. Documentation, deprecation,
and lint attributes are supported. There are no weak handles or mutation APIs.

Run `cargo test --workspace` to test both crates and the documentation example.
