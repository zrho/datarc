# datarc

`datarc` derives a pointer-sized, atomically reference-counted owner from an
ordinary borrowed view struct. The owner stores an atomic `usize` strong count,
ordinary values, and multiple variable-length arrays in one allocation, with no
weak count. The declared view remains a normal Rust struct.

```rust
use datarc::{ArcData, DataArc};

#[derive(DataArc, Debug)]
pub struct Record<'a> {
    pub id: &'a u64,
    #[datarc(len = u16)]
    pub values: &'a [i32],
    pub labels: &'a [String],
}

let id = 42;
let values = [10, 20];
let labels = [String::from("hello")];
let view = Record { id: &id, values: &values, labels: &labels };

// Clone the referenced values into a new allocation.
let owner: <Record<'_> as DataArc>::Owned = RecordArc::try_from(&view).unwrap();
let shared = owner.clone(); // Increment the strong count; no field cloning.

// Borrow a view value without allocation, cloning, or changing the strong count.
let borrowed = shared.as_view();
let via_from: Record<'_> = (&shared).into();
assert_eq!(borrowed.values, &[10, 20]);
assert!(std::ptr::eq(borrowed.values, via_from.values));
assert_eq!(std::mem::size_of::<RecordArc>(), std::mem::size_of::<usize>());

// Construct the owner directly from owned values and exact-size iterable inputs.
let direct = RecordArc::new(42, (1..3).map(|i| i * 10), ["hello".into()]);
assert_eq!(direct.values(), &[10, 20]);

fn generic_view<A: ArcData>(owner: &A) -> A::View<'_> { owner.as_view() }
assert_eq!(*generic_view(&direct).id, 42);
```

Add the `datarc` runtime crate as a dependency; it re-exports the derive macro
from `datarc-macros`. `#[derive(DataArc)]` generates `RecordArc` by default. Use
`#[datarc(owned = CustomName)]` on the view to choose another owner name. The
owner has the same visibility as the view; its accessors inherit each field's
visibility and documentation. Other derives, representation attributes, and
user-written methods on the original view retain their ordinary Rust meaning.

Each field must be a shared reference with a named generic lifetime:

- `&'a T` stores one owned `T` and produces an `&T` owner accessor.
- `&'a [T]` stores an inline array of `T` and produces an `&[T]` owner accessor.
- `&'a [T; N]` stores one fixed array, rather than a dynamic array.

Each dynamic array has an independent length, defaulting to `usize`.
`#[datarc(len = u16)]` selects the length storage type for a field. Supported
types are `u8`, `u16`, `u32`, `u64`, `u128`, and `usize`.

The `DataArc` trait associates a view with `type Owned: ArcData`. The `ArcData`
trait associates an owner with `type View<'a>: DataArc<Owned = Self>` and provides
`as_view(&self) -> Self::View<'_>`. The returned view is a value containing
references tied to the owner's borrow; it cannot outlive that borrow.

Conversions use the standard traits:

- `From<&RecordArc> for Record<'_>` borrows the owner's contents.
- `TryFrom<Record<'_>> for RecordArc` clones the referenced contents.
- `TryFrom<&Record<'_>> for RecordArc` does the same without consuming the view.

Cloning conversions are available only when every stored value and array
element implements `Clone`. The type association, borrowing, owner construction,
and owner `Clone` implementation remain available for non-`Clone` referents.
Conversions into owners return `BuildError` for invalid lengths or layouts;
there is no panicking `From<View>` implementation. A `Clone` implementation may
panic or allocate internally; partial clones are cleaned up on unwind.

Owner constructors take arguments in declaration order: a `T` for each ordinary
field and an `impl IntoIterator<Item = T, IntoIter: ExactSizeIterator>` for each
array. Vectors, owned fixed arrays, ranges, and mapped exact-size iterators are
accepted. Elements are generated or moved directly into the final record storage;
no temporary collection or `Clone` bound is required. To copy borrowed elements,
use `.iter().copied()` or `.iter().cloned()` explicitly. Iterators with unknown
lengths, such as filtered streams, must first be collected into a `Vec<T>`.
Unannotated `.collect()` calls may need `.collect::<Vec<_>>()` because constructors
accept multiple collection types.

Constructor lengths are obtained once and checked before allocation or element
generation. Actual item counts are checked too: `try_new` returns
`BuildError::IteratorLengthMismatch { field, expected }` if an iterator yields
too few or too many elements. At most one excess element is requested.
`try_new` also reports unrepresentable field lengths and allocation layouts;
`new` panics on these errors. Both consume their inputs, including on error.
Iterator panics propagate after destroying initialized values and freeing the
allocation. Allocator exhaustion follows the standard allocator's failure
behavior. Strong-count overflow aborts the process.

The allocation begins with an atomic strong count, followed by the field lengths
and ordinary values, then each array in declaration order with alignment padding.
Accessors have inline hints and compute offsets directly from immutable lengths
of preceding arrays, without repeating construction's layout validation. The
handle stores only a thin pointer; it does not cache a reference-bearing view.
Empty arrays have properly aligned slice pointers. The last owner drops every
value and releases the allocation, including when a destructor unwinds (a second
destructor panic follows Rust's usual abort behavior).

Owners are `Send + Sync` when all stored types are `Send + Sync`. Access is
immutable, though fields can provide their own interior mutability. Type and
const generics are preserved. Outer view lifetimes are removed from the owner;
separate lifetimes inside payload types are retained. For example,
`&'view &'data str` stores an `&'data str`, so its owner still borrows the text.
Cloning follows each payload type's `Clone` behavior rather than recursively
owning everything it references.

The derive requires named fields and sized referents or slices of sized elements.
Mutable references, outer `'static` references, and `&str` are unsupported; use
`&String` or `&[u8]` for text. A view lifetime cannot also appear inside its payload
types or trait bounds; use separate lifetimes. Outlives bounds such as `T: 'view`
and `'data: 'view` are supported. Field names `new`, `try_new`, and `as_view` are
reserved by generated owner methods. There are no weak handles or mutation APIs.

Run `cargo test --workspace` to test both crates and the documentation example.
