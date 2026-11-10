# datarc

`datarc` provides pointer-sized, atomically reference-counted owners for records
and enums. Records store values and variable-length arrays in one allocation;
enums store a variant tag in the pointer. Cloning an owner shares its data
without copying it.

## Records

Use `#[datarc::record]` on a named struct. Ordinary fields describe stored values,
and `[T]` fields describe inline arrays. The macro creates a shared owner with
the same name. Fixed arrays `[T; N]` are single values, and explicit references
are stored references:

```rust
#[datarc::record]
struct Record {
    id: u64,
    values: [i32],
}

let record = Record::new(42, [10, 20, 30]);
let shared = record.clone();
assert_eq!(*shared.id(), 42);
assert_eq!(record.values(), &[10, 20, 30]);
assert!(record.ptr_eq(&shared));
```

Constructors take owned values and exact-size iterables, such as arrays, vectors,
or ranges. Use `try_new` for fallible construction. Getters borrow the stored
values without allocating or changing the reference count. The last owner drops
the contents and frees the allocation. Struct documentation and visibility apply
to the owner; field documentation and visibility apply to the getters.

## Enums

Use `#[datarc::packed]` on an enum whose variants each hold one record owner to
produce a pointer-sized `ValuePacked`. The original enum remains available for
matching and conversions:

```rust
#[datarc::record]
struct Text {
    bytes: [u8],
}

#[datarc::record]
struct Number {
    value: u64,
}

#[datarc::packed]
enum Value {
    Text(Text),
    Number(Number),
}

let packed = ValuePacked::from(Value::Number(Number::new(42)));
packed.with_enum(|value| match value {
    Value::Number(number) => assert_eq!(*number.value(), 42),
    Value::Text(_) => unreachable!(),
});

let restored: Value = packed.into();
assert!(matches!(restored, Value::Number(_)));
```

Up to 16 variants are supported. Packing and unpacking transfer ownership without
allocating or updating reference counts. `with_enum` inspects the value without
changing counts. Cloning a packed owner shares the underlying record. Use
`#[datarc::packed(packed = CustomName)]` to set the packed owner's name.

## Advanced usage

### Standard traits

Record owners always implement `Clone`, without requiring fields to be `Clone`.
Add `#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]` after
`#[datarc::record]` to request field-based implementations of those traits.
Custom trait implementations target the record owner and can use its getters.
Packed enum owners delegate standard traits to the original enum.

Use `ptr_eq(&self, other: &Self)` to check whether record owners share an
allocation. Separate allocations can contain equal values. Enable equality
shortcuts with `#[datarc(eq(ptr_eq))]`, `#[datarc(hash, eq(hash))]`, or
`#[datarc(hash, eq(ptr_eq, hash))]`. Matching pointers establish equality;
different cached hashes refute it. Remaining comparisons use field values.
Shortcuts require deriving `PartialEq`. The pointer check runs first and requires
field types to implement `Eq`. Ordering remains field-based.

### Mutable fields

Use `#[datarc(mut)]` on a field to generate a `name_mut(&mut self)` accessor.
It returns `&mut T` for a value or `&mut [T]` for a sequence. A mutable slice
permits element changes, but cannot change the sequence length. Combine options
with `#[datarc(mut, len = u8)]` when needed.

```rust
#[datarc::record]
struct Buffer {
    #[datarc(mut)]
    label: String,
    #[datarc(mut, len = u8)]
    bytes: [u8],
}

let mut buffer = Buffer::new(String::from("data"), [1, 2, 3]);
let shared = buffer.clone();
buffer.label_mut().push_str("!");
buffer.bytes_mut()[0] = 9;
assert_eq!(buffer.label(), "data!");
assert_eq!(buffer.bytes(), &[9, 2, 3]);
assert_eq!(shared.bytes(), &[1, 2, 3]);
```

Mutable accessors require every stored value type and sequence element type to
implement `Clone`. Unique, non-static records mutate without copying. Shared or
static records first copy all fields into a new allocation; other owners keep
the original data. Owner cloning still shares storage. Mutable fields cannot
be combined with cached hashing (`#[datarc(hash)]`).

### Array lengths

Use `#[datarc(len = u8)]` on a `[T]` field to select a smaller stored length
type. The array length must fit in that type.

### Static storage

The unsafe `Record::new_static(alloc, fields...)` constructor creates an immortal
record using a caller-supplied allocator, an `FnOnce(Layout) -> *mut u8`. The
allocator must return unique, writable storage with the requested size and
alignment that remains valid for the rest of the process.

Static records use the same owner type, but cloning and dropping them skip
reference-count updates. Their contents and storage are never freed after
successful construction. This is a runtime constructor, not a `const fn`, and it
does not extend the lifetime of borrowed payloads.

### Cached hashes

Add `#[datarc(hash)]` to hash the fields once per allocation. Derive `Hash` to
use this cached value in the record's `Hash` implementation:

```rust
#[datarc::record]
#[derive(Hash)]
#[datarc(hash)]
struct Key {
    bytes: [u8],
}

let key = Key::new([1, 2, 3]);
assert_eq!(key.cached_hash(), key.clone().cached_hash());
```

The owner implements `Hash` by feeding the cached `u64` into the supplied hasher.
Construction uses `DefaultHasher` by default; select another with
`#[datarc(hash, hasher = MyHasher)]`, where `MyHasher: Hasher + Default`.
Hash-relevant data must remain unchanged: interior mutations do not refresh the
cache. With `eq(hash)`, hashing and equality must agree, and custom hashers must
produce consistent hashes across allocations. Matching hashes still require a
value comparison unless pointer equality establishes equality. Hash caching
alone does not enable equality shortcuts.
