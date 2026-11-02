//! Non-Send types in ordinary fields cannot cross threads.
//! ```compile_fail
//! use datarc::Packable;
//! #[derive(Packable)]
//! struct Record<'a> { value: &'a std::rc::Rc<u8> }
//! fn require_send<T: Send>() {}
//! require_send::<RecordPacked>();
//! ```
//!
//! Non-Send types in arrays cannot cross threads either.
//! ```compile_fail
//! use datarc::Packable;
//! #[derive(Packable)]
//! struct Record<'a> { values: &'a [std::rc::Rc<u8>] }
//! fn require_send<T: Send>() {}
//! require_send::<RecordPacked>();
//! ```
//!
//! Array elements must also be Sync for the shared handle to be Send.
//! ```compile_fail
//! use datarc::Packable;
//! #[derive(Packable)]
//! struct Record<'a> { values: &'a [std::cell::Cell<u8>] }
//! fn require_send<T: Send>() {}
//! require_send::<RecordPacked>();
//! ```
//!
//! Shared handles containing non-Sync elements cannot be shared by reference.
//! ```compile_fail
//! use datarc::Packable;
//! #[derive(Packable)]
//! struct Record<'a> { values: &'a [std::cell::Cell<u8>] }
//! fn require_sync<T: Sync>() {}
//! require_sync::<RecordPacked>();
//! ```
//!
//! An accessor cannot outlive its handle.
//! ```compile_fail
//! use datarc::Packable;
//! #[derive(Packable)]
//! struct Record<'a> { values: &'a [u8] }
//! let values = { let record = RecordPacked::new(vec![1]); record.values() };
//! assert_eq!(values, &[1]);
//! ```
//!
//! Borrowed array elements cannot outlive their source.
//! ```compile_fail
//! use datarc::Packable;
//! #[derive(Packable)]
//! struct Record<'view, 'data> { values: &'view [&'data str] }
//! let record = { let text = String::from("hello"); RecordPacked::new(vec![text.as_str()]) };
//! assert_eq!(record.values(), &["hello"]);
//! ```
//!
//! Private fields generate private accessors.
//! ```compile_fail
//! use datarc::Packable;
//! mod inner {
//!     use super::*;
//!     #[derive(Packable)]
//!     pub struct Record<'a> { value: &'a u8 }
//! }
//! let record = inner::RecordPacked::new(1);
//! let _ = record.value();
//! ```
//!
//! Iterators with unknown lengths require explicit collection first.
//! ```compile_fail
//! use datarc::Packable;
//! #[derive(Packable)]
//! struct Record<'a> { values: &'a [u8] }
//! let _ = RecordPacked::new((0..10).filter(|n| n % 2 == 0));
//! ```
//!
//! The derive preserves ownership and borrowing for non-Clone data, but it does
//! not enable a cloning conversion for such referents.
//! ```compile_fail
//! use datarc::Packable;
//! struct NotClone;
//! #[derive(Packable)]
//! struct View<'a> { value: &'a NotClone }
//! let value = NotClone;
//! let _ = ViewPacked::from(View { value: &value });
//! ```
//!
//! Slice elements must also be Clone to copy a view into an owner.
//! ```compile_fail
//! use datarc::Packable;
//! struct NotClone;
//! #[derive(Packable)]
//! struct View<'a> { values: &'a [NotClone] }
//! let values = [NotClone];
//! let _ = ViewPacked::from(&View { values: &values });
//! ```
//!
//! A derived borrowed view cannot outlive the owner that lends its references.
//! ```compile_fail
//! use datarc::Packable;
//! #[derive(Packable)]
//! struct View<'a> { values: &'a [u32] }
//! let view = { let owner = ViewPacked::new([1, 2]); View::from(&owner) };
//! assert_eq!(view.values, &[1, 2]);
//! ```
//!
//! Stored references retain their own payload lifetime in the generated owner.
//! ```compile_fail
//! use datarc::Packable;
//! #[derive(Packable)]
//! struct View<'view, 'data> { text: &'view &'data str }
//! let owner = { let text = String::from("payload"); ViewPacked::new(text.as_str()) };
//! assert_eq!(*View::from(&owner).text, "payload");
//! ```
//!
//! Custom permanent allocation requires the caller to uphold an unsafe contract.
//! ```compile_fail
//! use datarc::Packable;
//! #[derive(Packable)]
//! struct View<'a> { value: &'a u32 }
//! let _ = ViewPacked::new_static(|_| std::ptr::null_mut(), 1);
//! ```
//!
//! Permanent storage does not extend lifetimes of borrowed payloads.
//! ```compile_fail
//! use datarc::Packable;
//! #[derive(Packable)]
//! struct View<'view, 'data> { text: &'view &'data str }
//! let owner = {
//!     let text = String::from("payload");
//!     unsafe { ViewPacked::new_static(|layout| std::alloc::alloc(layout), text.as_str()) }
//! };
//! assert_eq!(*View::from(&owner).text, "payload");
//! ```

//!
//! Packed owners retain non-Send payload restrictions.
//! ```compile_fail
//! use datarc::Packable;
//! #[derive(Packable)]
//! struct Record<'a> { value: &'a std::rc::Rc<u8> }
//! #[derive(Packable)]
//! enum Value { Record(RecordPacked) }
//! fn send<T: Send>() {}
//! send::<ValuePacked>();
//! ```
//!
//! Packed owners retain non-Sync payload restrictions.
//! ```compile_fail
//! use datarc::Packable;
//! #[derive(Packable)]
//! struct Record<'a> { value: &'a std::cell::Cell<u8> }
//! #[derive(Packable)]
//! enum Value { Record(RecordPacked) }
//! fn sync<T: Sync>() {}
//! sync::<ValuePacked>();
//! ```
//!
//! Borrowed inspection cannot return references into its temporary enum.
//! ```compile_fail
//! use datarc::Packable;
//! #[derive(Packable)]
//! struct Record<'a> { value: &'a u32 }
//! #[derive(Packable)]
//! enum Value { Record(RecordPacked) }
//! let packed = ValuePacked::from(Value::Record(RecordPacked::new(1)));
//! let borrowed = packed.with_enum(|value| match value {
//!     Value::Record(owner) => owner.value(),
//! });
//! println!("{borrowed}");
//! ```
//!
//! Packed owners preserve the lifetime of borrowed payloads.
//! ```compile_fail
//! use datarc::Packable;
//! #[derive(Packable)]
//! struct Record<'v, 'data> { value: &'v &'data str }
//! #[derive(Packable)]
//! enum Value<'data> { Record(RecordPacked<'data>) }
//! let packed = {
//!     let text = String::from("borrowed");
//!     ValuePacked::from(Value::Record(RecordPacked::new(text.as_str())))
//! };
//! drop(packed);
//! ```
//!
//! Unaligned pointer types cannot be packed.
//! ```compile_fail
//! use datarc::Packable;
//! #[derive(Packable)]
//! enum Value { Byte(Box<u8>) }
//! ```

//!
//! Hash caching requires the view to implement Hash.
//! ```compile_fail
//! use datarc::Packable;
//! #[derive(Packable)]
//! #[datarc(hash)]
//! struct View<'a> { value: &'a u32 }
//! let _ = ViewPacked::new(42);
//! ```
//!
//! A custom hasher must implement Hasher and Default.
//! ```compile_fail
//! use datarc::Packable;
//! #[derive(Default)]
//! struct NotAHasher;
//! #[derive(Packable, Hash)]
//! #[datarc(hash, hasher = NotAHasher)]
//! struct View<'a> { value: &'a u32 }
//! let _ = ViewPacked::new(42);
//! ```
//!
//! Unions cannot derive Packable.
//! ```compile_fail
//! use datarc::Packable;
//! #[derive(Packable)]
//! union Value { byte: u8, word: u32 }
//! ```
//!
//! Owners do not gain Debug when the view lacks it.
//! ```compile_fail
//! use datarc::Packable;
//! #[derive(Packable)]
//! struct View<'a> { value: &'a u32 }
//! fn require<T: std::fmt::Debug>() {}
//! require::<ViewPacked>();
//! ```
//!
//! Owners do not gain PartialEq when the view lacks it.
//! ```compile_fail
//! use datarc::Packable;
//! #[derive(Packable)]
//! struct View<'a> { value: &'a u32 }
//! fn require<T: PartialEq>() {}
//! require::<ViewPacked>();
//! ```
//!
//! Pointer-shortcut equality requires Eq, even when the view has PartialEq.
//! ```compile_fail
//! use datarc::Packable;
//! #[derive(Packable, PartialEq)]
//! #[datarc(eq(ptr_eq))]
//! struct View<'a> { value: &'a f64 }
//! let owner = ViewPacked::new(f64::NAN);
//! let _ = owner == owner.clone();
//! ```
//!
//! Conditional ordering also requires Eq when pointer equality is enabled.
//! ```compile_fail
//! use datarc::Packable;
//! #[derive(Packable, PartialEq, PartialOrd)]
//! #[datarc(eq(ptr_eq))]
//! struct View<'a, T> { value: &'a T }
//! let owner = ViewPacked::new(f64::NAN);
//! let _ = owner.partial_cmp(&owner);
//! ```
//!
//! PartialEq alone does not enable Eq on the owner.
//! ```compile_fail
//! use datarc::Packable;
//! #[derive(Packable, PartialEq)]
//! struct View<'a> { value: &'a u32 }
//! fn require<T: Eq>() {}
//! require::<ViewPacked>();
//! ```
//!
//! Equality does not enable PartialOrd on the owner.
//! ```compile_fail
//! use datarc::Packable;
//! #[derive(Packable, PartialEq, Eq)]
//! struct View<'a> { value: &'a u32 }
//! fn require<T: PartialOrd>() {}
//! require::<ViewPacked>();
//! ```
//!
//! PartialOrd does not enable Ord on the owner.
//! ```compile_fail
//! use datarc::Packable;
//! #[derive(Packable, PartialEq, Eq, PartialOrd)]
//! struct View<'a> { value: &'a u32 }
//! fn require<T: Ord>() {}
//! require::<ViewPacked>();
//! ```
//!
//! Packed enums also require the original enum to implement the trait.
//! ```compile_fail
//! use datarc::Packable;
//! #[derive(Packable, Debug)]
//! struct View<'a> { value: &'a u32 }
//! #[derive(Packable)]
//! enum Value { View(ViewPacked) }
//! fn require<T: std::fmt::Debug>() {}
//! require::<ValuePacked>();
//! ```
