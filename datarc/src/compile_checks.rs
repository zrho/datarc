//! Non-Send types in ordinary fields cannot cross threads.
//! ```compile_fail
//! use datarc::DataArc;
//! #[derive(DataArc)]
//! struct Record<'a> { value: &'a std::rc::Rc<u8> }
//! fn require_send<T: Send>() {}
//! require_send::<RecordArc>();
//! ```
//!
//! Non-Send types in arrays cannot cross threads either.
//! ```compile_fail
//! use datarc::DataArc;
//! #[derive(DataArc)]
//! struct Record<'a> { values: &'a [std::rc::Rc<u8>] }
//! fn require_send<T: Send>() {}
//! require_send::<RecordArc>();
//! ```
//!
//! Array elements must also be Sync for the shared handle to be Send.
//! ```compile_fail
//! use datarc::DataArc;
//! #[derive(DataArc)]
//! struct Record<'a> { values: &'a [std::cell::Cell<u8>] }
//! fn require_send<T: Send>() {}
//! require_send::<RecordArc>();
//! ```
//!
//! Shared handles containing non-Sync elements cannot be shared by reference.
//! ```compile_fail
//! use datarc::DataArc;
//! #[derive(DataArc)]
//! struct Record<'a> { values: &'a [std::cell::Cell<u8>] }
//! fn require_sync<T: Sync>() {}
//! require_sync::<RecordArc>();
//! ```
//!
//! An accessor cannot outlive its handle.
//! ```compile_fail
//! use datarc::DataArc;
//! #[derive(DataArc)]
//! struct Record<'a> { values: &'a [u8] }
//! let values = { let record = RecordArc::new(vec![1]); record.values() };
//! assert_eq!(values, &[1]);
//! ```
//!
//! Borrowed array elements cannot outlive their source.
//! ```compile_fail
//! use datarc::DataArc;
//! #[derive(DataArc)]
//! struct Record<'view, 'data> { values: &'view [&'data str] }
//! let record = { let text = String::from("hello"); RecordArc::new(vec![text.as_str()]) };
//! assert_eq!(record.values(), &["hello"]);
//! ```
//!
//! Private fields generate private accessors.
//! ```compile_fail
//! use datarc::DataArc;
//! mod inner {
//!     use super::*;
//!     #[derive(DataArc)]
//!     pub struct Record<'a> { value: &'a u8 }
//! }
//! let record = inner::RecordArc::new(1);
//! let _ = record.value();
//! ```
//!
//! Iterators with unknown lengths require explicit collection first.
//! ```compile_fail
//! use datarc::DataArc;
//! #[derive(DataArc)]
//! struct Record<'a> { values: &'a [u8] }
//! let _ = RecordArc::new((0..10).filter(|n| n % 2 == 0));
//! ```
//!
//! The derive preserves ownership and borrowing for non-Clone data, but it does
//! not enable a cloning conversion for such referents.
//! ```compile_fail
//! use datarc::DataArc;
//! struct NotClone;
//! #[derive(DataArc)]
//! struct View<'a> { value: &'a NotClone }
//! let value = NotClone;
//! let _ = ViewArc::try_from(View { value: &value });
//! ```
//!
//! Slice elements must also be Clone to copy a view into an owner.
//! ```compile_fail
//! use datarc::DataArc;
//! struct NotClone;
//! #[derive(DataArc)]
//! struct View<'a> { values: &'a [NotClone] }
//! let values = [NotClone];
//! let _ = ViewArc::try_from(&View { values: &values });
//! ```
//!
//! A derived borrowed view cannot outlive the owner that lends its references.
//! ```compile_fail
//! use datarc::DataArc;
//! #[derive(DataArc)]
//! struct View<'a> { values: &'a [u32] }
//! let view = { let owner = ViewArc::new([1, 2]); owner.as_view() };
//! assert_eq!(view.values, &[1, 2]);
//! ```
//!
//! Stored references retain their own payload lifetime in the generated owner.
//! ```compile_fail
//! use datarc::DataArc;
//! #[derive(DataArc)]
//! struct View<'view, 'data> { text: &'view &'data str }
//! let owner = { let text = String::from("payload"); ViewArc::new(text.as_str()) };
//! assert_eq!(*owner.as_view().text, "payload");
//! ```
