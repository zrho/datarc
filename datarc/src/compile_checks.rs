//! Non-Send types in ordinary fields cannot cross threads.
//! ```compile_fail
//! #[datarc::record]
//! struct Record { value: std::rc::Rc<u8> }
//! fn require_send<T: Send>() {}
//! require_send::<Record>();
//! ```
//!
//! Non-Send types in arrays cannot cross threads either.
//! ```compile_fail
//! #[datarc::record]
//! struct Record { values: [std::rc::Rc<u8>] }
//! fn require_send<T: Send>() {}
//! require_send::<Record>();
//! ```
//!
//! Array elements must also be Sync for the shared handle to be Send.
//! ```compile_fail
//! #[datarc::record]
//! struct Record { values: [std::cell::Cell<u8>] }
//! fn require_send<T: Send>() {}
//! require_send::<Record>();
//! ```
//!
//! Shared handles containing non-Sync elements cannot be shared by reference.
//! ```compile_fail
//! #[datarc::record]
//! struct Record { values: [std::cell::Cell<u8>] }
//! fn require_sync<T: Sync>() {}
//! require_sync::<Record>();
//! ```
//!
//! An accessor cannot outlive its handle.
//! ```compile_fail
//! #[datarc::record]
//! struct Record { values: [u8] }
//! let values = { let record = Record::new(vec![1]); record.values() };
//! assert_eq!(values, &[1]);
//! ```
//!
//! Borrowed array elements cannot outlive their source.
//! ```compile_fail
//! #[datarc::record]
//! struct Record<'data> { values: [&'data str] }
//! let record = { let text = String::from("hello"); Record::new(vec![text.as_str()]) };
//! assert_eq!(record.values(), &["hello"]);
//! ```
//!
//! Private fields generate private accessors.
//! ```compile_fail
//! mod inner {
//!     #[datarc::record]
//!     pub struct Record { value: u8 }
//! }
//! let record = inner::Record::new(1);
//! let _ = record.value();
//! ```
//!
//! Iterators with unknown lengths require explicit collection first.
//! ```compile_fail
//! #[datarc::record]
//! struct Record { values: [u8] }
//! let _ = Record::new((0..10).filter(|n| n % 2 == 0));
//! ```
//!
//! Stored references retain their own payload lifetime in the generated owner.
//! ```compile_fail
//! #[datarc::record]
//! struct Record<'data> { text: &'data str }
//! let owner = { let text = String::from("payload"); Record::new(text.as_str()) };
//! assert_eq!(*owner.text(), "payload");
//! ```
//!
//! Custom permanent allocation requires the caller to uphold an unsafe contract.
//! ```compile_fail
//! #[datarc::record]
//! struct Record { value: u32 }
//! let _ = Record::new_static(|_| std::ptr::null_mut(), 1);
//! ```
//!
//! Permanent storage does not extend lifetimes of borrowed payloads.
//! ```compile_fail
//! #[datarc::record]
//! struct Record<'data> { text: &'data str }
//! let owner = {
//!     let text = String::from("payload");
//!     unsafe { Record::new_static(|layout| std::alloc::alloc(layout), text.as_str()) }
//! };
//! assert_eq!(*owner.text(), "payload");
//! ```
//!
//! Packed owners retain non-Send payload restrictions.
//! ```compile_fail
//! #[datarc::record]
//! struct Record { value: std::rc::Rc<u8> }
//! #[datarc::packed]
//! enum Value { Record(Record) }
//! fn send<T: Send>() {}
//! send::<ValuePacked>();
//! ```
//!
//! Packed owners retain non-Sync payload restrictions.
//! ```compile_fail
//! #[datarc::record]
//! struct Record { value: std::cell::Cell<u8> }
//! #[datarc::packed]
//! enum Value { Record(Record) }
//! fn sync<T: Sync>() {}
//! sync::<ValuePacked>();
//! ```
//!
//! Borrowed inspection cannot return references into its temporary enum.
//! ```compile_fail
//! #[datarc::record]
//! struct Record { value: u32 }
//! #[datarc::packed]
//! enum Value { Record(Record) }
//! let packed = ValuePacked::from(Value::Record(Record::new(1)));
//! let borrowed = packed.with_enum(|value| match value {
//!     Value::Record(owner) => owner.value(),
//! });
//! println!("{borrowed}");
//! ```
//!
//! Packed owners preserve the lifetime of borrowed payloads.
//! ```compile_fail
//! #[datarc::record]
//! struct Record<'data> { value: &'data str }
//! #[datarc::packed]
//! enum Value<'data> { Record(Record<'data>) }
//! let packed = {
//!     let text = String::from("borrowed");
//!     ValuePacked::from(Value::Record(Record::new(text.as_str())))
//! };
//! drop(packed);
//! ```
//!
//! Unaligned pointer types cannot be packed.
//! ```compile_fail
//! #[datarc::packed]
//! enum Value { Byte(Box<u8>) }
//! ```
//!
//! Hash caching requires each field to implement Hash.
//! ```compile_fail
//! struct NotHash;
//! #[datarc::record]
//! #[datarc(hash)]
//! struct Record { value: NotHash }
//! let _ = Record::new(NotHash);
//! ```
//!
//! A custom hasher must implement Hasher and Default.
//! ```compile_fail
//! #[derive(Default)]
//! struct NotAHasher;
//! #[datarc::record]
//! #[derive(Hash)]
//! #[datarc(hash, hasher = NotAHasher)]
//! struct Record { value: u32 }
//! let _ = Record::new(42);
//! ```
//!
//! The packed attribute cannot be used on unions.
//! ```compile_fail
//! #[datarc::packed]
//! union Value { byte: u8, word: u32 }
//! ```
//!
//! Owners do not gain Debug without an explicit derive.
//! ```compile_fail
//! #[datarc::record]
//! struct Record { value: u32 }
//! fn require<T: std::fmt::Debug>() {}
//! require::<Record>();
//! ```
//!
//! Owners do not gain PartialEq without an explicit derive.
//! ```compile_fail
//! #[datarc::record]
//! struct Record { value: u32 }
//! fn require<T: PartialEq>() {}
//! require::<Record>();
//! ```
//!
//! Pointer-shortcut equality requires Eq, even when the record has PartialEq.
//! ```compile_fail
//! #[datarc::record]
//! #[derive(PartialEq)]
//! #[datarc(eq(ptr_eq))]
//! struct Record { value: f64 }
//! let owner = Record::new(f64::NAN);
//! let _ = owner == owner.clone();
//! ```
//!
//! Conditional ordering also requires Eq when pointer equality is enabled.
//! ```compile_fail
//! #[datarc::record]
//! #[derive(PartialEq, PartialOrd)]
//! #[datarc(eq(ptr_eq))]
//! struct Record<T> { value: T }
//! let owner = Record::new(f64::NAN);
//! let _ = owner.partial_cmp(&owner);
//! ```
//!
//! PartialEq alone does not enable Eq on the owner.
//! ```compile_fail
//! #[datarc::record]
//! #[derive(PartialEq)]
//! struct Record { value: u32 }
//! fn require<T: Eq>() {}
//! require::<Record>();
//! ```
//!
//! Equality does not enable PartialOrd on the owner.
//! ```compile_fail
//! #[datarc::record]
//! #[derive(PartialEq, Eq)]
//! struct Record { value: u32 }
//! fn require<T: PartialOrd>() {}
//! require::<Record>();
//! ```
//!
//! PartialOrd does not enable Ord on the owner.
//! ```compile_fail
//! #[datarc::record]
//! #[derive(PartialEq, Eq, PartialOrd)]
//! struct Record { value: u32 }
//! fn require<T: Ord>() {}
//! require::<Record>();
//! ```
//!
//! Packed enums also require the original enum to implement the trait.
//! ```compile_fail
//! #[datarc::record]
//! #[derive(Debug)]
//! struct Record { value: u32 }
//! #[datarc::packed]
//! enum Value { Record(Record) }
//! fn require<T: std::fmt::Debug>() {}
//! require::<ValuePacked>();
//! ```
