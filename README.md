# datarc

`datarc` derives a pointer-sized, atomically reference-counted owner from an
ordinary borrowed view struct. The owner stores an atomic `usize` strong count,
ordinary values, and multiple variable-length arrays in one allocation, with no
weak count. The declared view remains a normal Rust struct.

```rust
use datarc::Packable;

#[derive(Packable, Debug)]
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
let owner: <Record<'_> as Packable>::Packed = RecordArc::from(&view);
let shared = owner.clone(); // Increment the strong count; no field cloning.

// Borrow a view value without allocation, cloning, or changing the strong count.
let borrowed = Record::from(&shared);
let via_from: Record<'_> = (&shared).into();
assert_eq!(borrowed.values, &[10, 20]);
assert!(std::ptr::eq(borrowed.values, via_from.values));
assert_eq!(std::mem::size_of::<RecordArc>(), std::mem::size_of::<usize>());

// Construct the owner directly from owned values and exact-size iterable inputs.
let direct = RecordArc::new(42, (1..3).map(|i| i * 10), ["hello".into()]);
assert_eq!(direct.values(), &[10, 20]);

fn generic_view<'a, V: Packable + From<&'a V::Packed>>(owner: &'a V::Packed) -> V {
    owner.into()
}
assert_eq!(*generic_view::<Record<'_>>(&direct).id, 42);
```

Add the `datarc` runtime crate as a dependency; it re-exports the derive macro
from `datarc-macros`. `#[derive(Packable)]` generates `RecordArc` by default. Use
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

The `Packable` trait associates a view with `type Packed`. Borrowing uses
`From<&RecordArc> for Record<'_>`. The returned view is a value containing
references tied to the owner's borrow; it cannot outlive that borrow. Generic
callers specify the view type and use a `From<&V::Packed>` bound.

Conversions use the standard traits:

- `From<&RecordArc> for Record<'_>` borrows the owner's contents.
- `From<Record<'_>> for RecordArc` clones the referenced contents.
- `From<&Record<'_>> for RecordArc` does the same without consuming the view.

Cloning conversions are available only when every stored value and array
element implements `Clone`. The type association, borrowing, owner construction,
and owner `Clone` implementation remain available for non-`Clone` referents.
Conversions into owners panic on invalid lengths or allocation layouts, like
`new`. Use `try_new` with the view's field values and `.iter().cloned()` for
slice inputs when construction must return `BuildError` instead. A `Clone`
implementation may panic or allocate internally; partial clones are cleaned up
on unwind.

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

Owners also provide an unsafe `new_static(alloc, fields...)` constructor. The
`alloc` callback is an `FnOnce(Layout) -> *mut u8`; it must supply a unique,
writable region with the requested size and alignment, remaining valid for the
rest of the process on successful construction. This can be an arena region or
an intentionally retained heap allocation. The callback is called once, after
length and layout validation. A null result follows standard allocation-failure
behavior.

```rust
use datarc::Packable;

#[derive(Packable)]
struct Record<'a> {
    id: &'a u64,
    values: &'a [u32],
}

// SAFETY: alloc supplies fresh aligned storage which is intentionally never freed.
let permanent = unsafe {
    RecordArc::new_static(|layout| std::alloc::alloc(layout), 42, [1, 2, 3])
};
let shared = permanent.clone();
assert_eq!(*Record::from(&shared).id, 42);
assert_eq!(shared.values(), &[1, 2, 3]);
drop((permanent, shared)); // The contents and allocation remain alive.
```

The returned value has the same owner type as an ordinary record. Its count is
initialized to the immortal sentinel `usize::MAX` before publication. Clone and
Drop perform a relaxed count load and skip all count updates for immortal records;
field access is unchanged. Successful static construction never destroys the
stored values or frees the supplied storage. If construction fails or an iterator
panics, initialized values are destroyed, but the supplied region is still not
freed by datarc. The caller retains responsibility for that region on failure.
Borrowed payload lifetimes are preserved. `new_static` is a runtime constructor,
not a `const fn`.

### Cached hashes

Add `#[datarc(hash)]` to a struct view that implements `std::hash::Hash` to
compute a hash once when its owner is constructed:

```rust
use datarc::Packable;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

#[derive(Packable, Hash)]
#[datarc(hash)]
struct Key<'a> {
    id: &'a u64,
    bytes: &'a [u8],
}

let owner = KeyArc::new(42, [1, 2, 3]);
let mut hasher = DefaultHasher::new();
Key::from(&owner).hash(&mut hasher);
assert_eq!(owner.cached_hash(), hasher.finish());
assert_eq!(owner.clone().cached_hash(), owner.cached_hash());
```

Owners with `#[datarc(hash)]` also implement `Hash`: they write the cached
`u64` into the supplied hasher without hashing the view again. This hashes the
cached value, so the owner's final hash need not equal the view's final hash.

`cached_hash()` returns a `u64`. By default, construction uses
`std::collections::hash_map::DefaultHasher`. Select another hasher with
`#[datarc(hash, hasher = MyHasher)]`, where `MyHasher` implements
`std::hash::Hasher + Default`.

The hash comes from the fully initialized borrowed view's `Hash` implementation,
not from hashing each stored field independently. You can implement `Hash`
manually to select the data to hash. `new`, successful `try_new`, `new_static`,
and both conversions from a view compute the hash once per new allocation.
Cloning an owner shares its stored hash without hashing again; borrowing a view
and calling `cached_hash()` do not rehash either. Interior mutations do not
refresh the cache, so hash-relevant data must remain unchanged if you need the
cached value to match the current view. Cached values are not a stable serialized
fingerprint; their behavior depends on the chosen hasher and `Hash` implementation.

The allocation begins with an atomic strong count, followed by the field lengths
and ordinary values, then each array in declaration order with alignment padding.
Accessors have inline hints and compute offsets directly from immutable lengths
of preceding arrays, without repeating construction's layout validation. The
handle stores only a thin pointer; it does not cache a reference-bearing view.
Empty arrays have properly aligned slice pointers. The last ordinary owner drops
every value and releases the allocation, including when a destructor unwinds
(a second destructor panic follows Rust's usual abort behavior).

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
and `'data: 'view` are supported. Field names `new`, `try_new`, and `new_static`
are reserved by generated owner methods. There are no weak handles or mutation
APIs.

`#[derive(Packable)]` generates a pointer-sized owner for an enum with 1–16
variants, each containing exactly one generated struct owner:

```rust
use datarc::Packable;

#[derive(Packable)]
struct Text<'a> { bytes: &'a [u8] }
#[derive(Packable)]
struct Number<'a> { value: &'a u64 }

#[derive(Packable)]
enum Value {
    Text(TextArc),
    Number(NumberArc),
}

let packed = ValuePacked::from(Value::Number(NumberArc::new(42)));
assert_eq!(std::mem::size_of_val(&packed), std::mem::size_of::<usize>());
packed.with_enum(|value| match value {
    Value::Number(number) => assert_eq!(*number.value(), 42),
    Value::Text(_) => unreachable!(),
});
let shared = packed.clone();
let original: Value = shared.into();
assert!(matches!(original, Value::Number(_)));
```

The same `Packable` trait associates the original enum with `type Packed`.
The derive generates `From` conversions in both directions; generic callers add
`E::Packed: From<E>` or `E: From<E::Packed>` bounds as needed. The original enum
remains unchanged. The generated type has the same visibility;
`#[datarc(packed = CustomName)]` selects its name. Type, const, and payload lifetime
generics are preserved. Variants must be single-item tuples; conditional variants
or fields are unsupported. Explicit Rust discriminants do not determine pointer
tags; tags are assigned in declaration order.

`From<Enum>` packs by transferring ownership; `From<&Enum>` clones the stored
pointer into a packed owner. `into_enum()` and `From<Packed>` restore it without
allocation or reference-count updates. `with_enum` inspects a
temporary borrowed enum without changing counts. Its references cannot escape
the callback. `From<&Packed>` (also available as `to_enum()`) clones the stored
pointer into an ordinary enum, and `Clone` creates another packed owner. Dropping a packed owner drops its stored
pointer. The original enum need not implement `Clone`, and stored data need not
be cloneable. `Send` and `Sync` follow the original enum.

All generated allocation headers have a minimum alignment of 16 bytes, retaining
any larger alignment required by their fields or arrays. The four low pointer
bits store the tag; pointer provenance is preserved when adding or removing it.
This alignment can increase padding and allocation size for small records. The
handle and `Option` of the packed handle each occupy one pointer. Static
constructors request the same minimum alignment from their allocation callback.

Packed fields must implement the unsafe `PackedPointer` trait. All generated
struct owners do so automatically; arbitrary pointers such as `Box<T>` and `Arc<T>` do
not qualify automatically because their pointee alignment may be insufficient.
Custom implementations must uphold the trait's alignment, ownership, and shared
access contracts.

Run `cargo test --workspace` to test both crates and the documentation example.
