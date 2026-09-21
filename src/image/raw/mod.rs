//! Camera RAW decoding, behind the `image-raw` feature.
//!
//! Gathered into one module so the feature gate is stated twice — on this `mod` and on the re-export beside it —
//! rather than once per item. See the [RAW decoding](super#raw-decoding) section of the module docs for what the
//! feature adds and what it costs.
//!
//! RAW reaches callers two ways. [`decode_file`](super::decode_file) and [`decode_bytes`](super::decode_bytes)
//! route to it through [`dispatch`], so the ordinary entry points open RAW like anything else; the functions
//! re-exported here are the narrow forms, which refuse anything that is not RAW and carry the RAW-only metadata
//! [`ImageInfo`](super::ImageInfo) has no room for.

mod decode_bytes;
mod decode_file;
pub(super) mod dispatch;
mod format;
mod info;
mod probe_bytes;
mod probe_file;

pub use decode_bytes::decode_raw_bytes;
pub use decode_file::decode_raw_file;
pub use format::{RawFormat, is_raw_bytes};
pub use info::RawImageInfo;
pub use probe_bytes::probe_raw_bytes;
pub use probe_file::probe_raw_file;
