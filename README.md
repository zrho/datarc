# datarc

`datarc` is a Rust library that provides a `Packable` derive macro for
generating efficient, pointer-sized, atomically reference-counted owners for
records and enums. Records pack their values and variable-length arrays into one
allocation; enums pack a variant tag into the pointer itself. Cloning an owner
shares its data without copying it.

## Records

Derive `Packable` on a borrowed view to generate an owning type named
`RecordPacked`. Ordinary references describe stored values, and slices describe
inline arrays:

```rust
use datarc::Packable;

#[derive(Packable)]
struct Record<'a> {
    id: &'a u64,
    values: &'a [i32],
}

let record = RecordPacked::new(42, [10, 20, 30]);
let shared = record.clone();
let view = Record::from(&shared);

assert_eq!(*view.id, 42);
assert_eq!(view.values, &[10, 20, 30]);
assert_eq!(record.values(), view.values);
```

The owner stores both fields in one allocation. Borrowing a view allocates
nothing and leaves the reference count unchanged. The last owner drops the
contents and frees the allocation.

Constructors take owned values and exact-size iterables, such as arrays, vectors,
or ranges. `RecordPacked::from(&view)` instead clones the referenced contents into a
new allocation. Use `try_new` for fallible construction, and `#[datarc(len = u16)]`
on a slice field to choose a smaller length type.

## Enums

Derive `Packable` on an enum whose variants each hold one generated record owner
to produce a pointer-sized `ValuePacked`:

```rust
use datarc::Packable;

#[derive(Packable)]
struct Text<'a> {
    bytes: &'a [u8],
}

#[derive(Packable)]
struct Number<'a> {
    value: &'a u64,
}

#[derive(Packable)]
enum Value {
    Text(TextPacked),
    Number(NumberPacked),
}

let packed = ValuePacked::from(Value::Number(NumberPacked::new(42)));
packed.with_enum(|value| match value {
    Value::Number(number) => assert_eq!(*number.value(), 42),
    Value::Text(_) => unreachable!(),
});

let restored: Value = packed.into();
assert!(matches!(restored, Value::Number(_)));
```

Up to 16 variants are supported. Packing and unpacking transfer ownership without
allocating or updating reference counts; `with_enum` inspects the value without
changing counts. Cloning a packed owner shares the underlying record.

## Advanced usage

### Static

The unsafe `RecordPacked::new_static(alloc, fields...)` constructor creates an
immortal record using a caller-supplied allocator, an `FnOnce(Layout) -> *mut u8`.
The allocator must return unique, writable storage with the requested size and
alignment that remains valid for the rest of the process, such as a permanent
arena region.

Static records use the same owner type, but cloning and dropping them skip
reference-count updates. Their contents and storage are never freed after
successful construction. This is a runtime constructor, not a `const fn`.

### Cached hashes

Add `#[datarc(hash)]` to a record view that implements `Hash` to compute and store
its hash once per allocation:

```rust
use datarc::Packable;

#[derive(Packable, Hash)]
#[datarc(hash)]
struct Key<'a> {
    bytes: &'a [u8],
}

let key = KeyPacked::new([1, 2, 3]);
assert_eq!(key.cached_hash(), key.clone().cached_hash());
```

The owner also implements `Hash` by feeding the cached `u64` into the supplied
hasher. Construction uses `DefaultHasher` by default; select another with
`#[datarc(hash, hasher = MyHasher)]`, where `MyHasher: Hasher + Default`.
Hash-relevant data must remain unchanged: interior mutations do not refresh the
cache.
