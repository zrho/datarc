# datarc

`datarc` creates pointer-sized, atomically reference-counted records with multiple
inline, variable-length fields. There is one allocation, one `usize` strong count,
and no weak count. Cloning shares the allocation without cloning its contents.

```rust
use datarc::datarc;

#[datarc]
pub struct Record {
    pub id: u64,
    #[datarc(many, len = u16)]
    pub values: i32,
    #[datarc(many)]
    pub labels: String,
}

let record = Record::new(42, vec![10, 20], vec!["hello".into()]);
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

Fields default to `once` (also spelled `#[datarc(once)]`) and have `&T` accessors.
Annotate a field of type `T` with `#[datarc(many)]` to store a dynamic array of `T`
and generate an `&[T]` accessor. Each array has its own length. Lengths default to
`usize`; `len` accepts `u8`, `u16`, `u32`, `u64`, `u128`, or `usize` on each `many`
field independently.

Both constructors take arguments in declaration order: a `T` for each ordinary
field and a `Vec<T>` for each array. Values move into the allocation, including
the array elements; no `Clone` bound is required. Input vectors are temporary
construction storage and are not retained in the record.

`new` panics if a length or allocation layout cannot be represented. `try_new`
returns `Result<Self, datarc::BuildError>` for these cases. Both consume their
inputs, including on error. Allocator exhaustion follows the standard allocator's
failure behavior. Strong-count overflow aborts the process.

The allocation begins with an atomic strong count, followed by the field lengths
and ordinary values, then each array in declaration order with alignment padding.
Accessors have inline hints and compute offsets directly from immutable lengths
of preceding arrays, without repeating construction's layout validation. The
handle stores only a thin pointer. Even empty arrays have properly aligned slice
pointers. The last owner
drops every value and releases the allocation, including when a destructor
unwinds (a second destructor panic follows Rust's usual abort behavior).

Records implement `Clone` without field bounds. They are `Send + Sync` when all
stored types are `Send + Sync`. Access is immutable, though fields can provide
their own interior mutability. Type, lifetime, and const generics are supported.

This initial API does not support tuple/unit structs, custom derives, `repr`
attributes, conditional fields, or field names `new` and `try_new`. Use the record's
name instead of `Self` in field types and generic bounds. Documentation, deprecation,
and lint attributes are supported. There are no weak handles or mutation APIs.

Run `cargo test --workspace` to test both crates and the documentation example.
