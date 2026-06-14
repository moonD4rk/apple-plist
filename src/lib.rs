//! Apple property-list serialization for Rust: XML, binary (`bplist00`),
//! OpenStep, and GNUStep — encode and decode.
//!
//! `apple-plist` is a serde-native Rust library for Apple property lists.
//! Unlike most Rust plist crates it targets **all four** Apple property-list
//! dialects in both directions, handling the edge cases that show up in
//! real-world files.
//!
//! # Quick start
//!
//! Decoding auto-detects the format; encoding picks the format you name:
//!
//! ```
//! use apple_plist::{Format, Value, detect};
//!
//! let bytes = apple_plist::to_vec(&true, Format::Xml)?;
//! assert!(bytes.ends_with(b"<true/></plist>"));
//!
//! let value: Value = apple_plist::from_slice(&bytes)?;
//! assert_eq!(value, Value::Boolean(true));
//! assert_eq!(detect(&bytes), Some(Format::Xml));
//! # Ok::<(), apple_plist::Error>(())
//! ```
//!
//! Streaming and `Value`-tree variants live on [`Encoder`] and [`Decoder`];
//! both compile with every feature combination and work without `serde`.
//!
//! # Status
//!
//! The full surface is implemented: the core data model ([`Value`] and its
//! nine cases, [`Integer`], [`Real`], [`Date`], [`Uid`], the [`Error`]
//! model, [`Format`], the depth bound), all four codecs behind their feature
//! gates, format auto-detection ([`detect`], the XML-to-text retry ladder),
//! and the serde bridge ([`to_vec`], [`from_slice`], [`to_value`],
//! [`from_value`], and friends).
//!
//! # Cargo features
//!
//! All on by default; every combination compiles and features only add API:
//!
//! - `serde` — derive-driven (de)serialization: [`Encoder::encode`],
//!   [`Decoder::decode`], and the free functions [`to_vec`],
//!   [`to_vec_indent`], [`to_writer`], [`from_slice`], [`from_reader`],
//!   [`to_value`], [`from_value`].
//! - `xml` — the XML codec.
//! - `binary` — the binary (`bplist00`) codec.
//! - `openstep` — the OpenStep/GNUStep text codec (one feature: the two
//!   dialects share a parser, and OpenStep-versus-GNUStep is a runtime
//!   outcome).
//!
//! Decoding a format whose feature is compiled out fails with
//! [`Error::InvalidPlist`] from its rung of the detection ladder; encoding
//! one fails with [`Error::FeatureDisabled`].

mod date;
mod depth;
mod error;
mod format;
#[cfg(any(test, feature = "serde", feature = "xml", feature = "openstep"))]
mod scalar;
mod uid;
mod value;

#[cfg(feature = "binary")]
mod binary;
mod de;
mod ser;
#[cfg(feature = "openstep")]
mod text;
#[cfg(feature = "xml")]
mod xml;

pub use crate::date::Date;
pub use crate::de::{Decoder, detect};
#[cfg(feature = "serde")]
pub use crate::de::{from_reader, from_slice};
pub use crate::depth::MAX_PARSE_DEPTH;
pub use crate::error::{Error, Result};
pub use crate::format::Format;
pub use crate::ser::Encoder;
#[cfg(feature = "serde")]
pub use crate::ser::{to_vec, to_vec_indent, to_writer};
pub use crate::uid::Uid;
#[cfg(feature = "serde")]
pub use crate::value::de::from_value;
#[cfg(feature = "serde")]
pub use crate::value::ser::to_value;
pub use crate::value::{Dictionary, Integer, Real, Value};
