//! Non-Send types in ordinary fields cannot cross threads.
//! ```compile_fail
//! use datarc::datarc;
//! #[datarc]
//! struct Record { value: std::rc::Rc<u8> }
//! fn require_send<T: Send>() {}
//! require_send::<Record>();
//! ```
//!
//! Non-Send types in arrays cannot cross threads either.
//! ```compile_fail
//! use datarc::datarc;
//! #[datarc]
//! struct Record { #[datarc(many)] values: std::rc::Rc<u8> }
//! fn require_send<T: Send>() {}
//! require_send::<Record>();
//! ```
//!
//! Array elements must also be Sync for the shared handle to be Send.
//! ```compile_fail
//! use datarc::datarc;
//! #[datarc]
//! struct Record { #[datarc(many)] values: std::cell::Cell<u8> }
//! fn require_send<T: Send>() {}
//! require_send::<Record>();
//! ```
//!
//! Shared handles containing non-Sync elements cannot be shared by reference.
//! ```compile_fail
//! use datarc::datarc;
//! #[datarc]
//! struct Record { #[datarc(many)] values: std::cell::Cell<u8> }
//! fn require_sync<T: Sync>() {}
//! require_sync::<Record>();
//! ```
//!
//! An accessor cannot outlive its handle.
//! ```compile_fail
//! use datarc::datarc;
//! #[datarc]
//! struct Record { #[datarc(many)] values: u8 }
//! let values = { let record = Record::new(vec![1]); record.values() };
//! assert_eq!(values, &[1]);
//! ```
//!
//! Borrowed array elements cannot outlive their source.
//! ```compile_fail
//! use datarc::datarc;
//! #[datarc]
//! struct Record<'a> { #[datarc(many)] values: &'a str }
//! let record = { let text = String::from("hello"); Record::new(vec![text.as_str()]) };
//! assert_eq!(record.values(), &["hello"]);
//! ```
//!
//! Private fields generate private accessors.
//! ```compile_fail
//! use datarc::datarc;
//! mod inner {
//!     use super::*;
//!     #[datarc]
//!     pub struct Record { value: u8 }
//! }
//! let record = inner::Record::new(1);
//! let _ = record.value();
//! ```
