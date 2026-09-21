//! Image encoding and decoding across many formats behind a small, uniform API.
//!
//! The native formats — **bmp, gif, jpeg/jpg, png, tiff** — are handled by the [`image`](::image) crate. The three
//! formats with their own dedicated crates — **avif** (`avif`), **heif/heic** (`heif`), and **webp** (`webp`) — are
//! routed through those crates instead of the `image` crate's built-in codecs.
//!
//! Decoding always yields a [`DynamicImage`](::image::DynamicImage); encoding takes one. The public surface:
//!
//! - [`decode_file`] — decode a file, format chosen from its extension.
//! - [`decode_bytes`] — decode in-memory bytes, format guessed from the magic bytes.
//! - [`decode_bytes_with_format`] — decode in-memory bytes with an explicitly given [`ImageFormat`].
//! - [`format_from_bytes`] — detect the [`ImageFormat`] from magic bytes without decoding.
//! - [`probe_bytes`] / [`probe_file`] — read an image's metadata ([`ImageInfo`]: dimensions, color type, bit
//!   depth) from its header without decoding the pixels.
//! - [`encode_file`] — encode and save to a path, format chosen from its extension.
//! - [`encode_writer`] — encode and write to any [`Write`](std::io::Write) sink with an explicit [`ImageFormat`].
//!
//! Lossy/codec-specific settings are supplied per format via [`EncodeOptions`]; pass `None` to use sensible defaults.
//!
//! ```
//! use image::{DynamicImage, RgbaImage};
//! use rust_sak::image::{ImageFormat, decode_bytes, encode_writer, format_from_bytes};
//!
//! // A tiny 2x2 image round-tripped through PNG (a native codec, no extra binaries required).
//! let original = DynamicImage::ImageRgba8(RgbaImage::new(2, 2));
//!
//! let mut bytes = Vec::new();
//! encode_writer(&original, &mut bytes, ImageFormat::Png, None).unwrap();
//!
//! assert_eq!(format_from_bytes(&bytes).unwrap(), ImageFormat::Png);
//!
//! let decoded = decode_bytes(&bytes).unwrap();
//! assert_eq!((decoded.width(), decoded.height()), (2, 2));
//! ```
//!
//! # RAW decoding
//!
//! Camera RAW/DNG decoding lives behind the **separate `image-raw` feature**, not `image`. Turning it on teaches
//! [`decode_file`] and [`decode_bytes`] to open RAW as well, and adds `decode_raw_bytes`/`decode_raw_file`,
//! `probe_raw_bytes`/`probe_raw_file`, `RawFormat` and `RawImageInfo` for callers that want RAW specifically —
//! 300-plus cameras' files, decoded to the same [`DynamicImage`](::image::DynamicImage) the other eight formats
//! produce. (Those names exist only when the feature is on, so they are written plainly here rather than linked.)
//! All of it lives in one `#[cfg(feature = "image-raw")]` module, so a build without the feature links none of it.
//!
//! **By name, RAW is fully resolved; by content, only partly.** [`decode_file`] routes on the extension, which is
//! unambiguous, so all of them open. [`decode_bytes`] has only the header, and the TIFF-based formats (NEF, ARW,
//! CR2, PEF, DNG) open exactly as an ordinary TIFF does — nothing in the bytes distinguishes them, so they decode
//! as TIFF, and only the containers with a signature of their own (CR3, RAF, RW2, ORF) route to RAW. A native
//! extension always wins over a RAW one, so a `.tiff` is never claimed for RAW.
//!
//! Probing stays split: `probe_file` returns an [`ImageInfo`], whose `format` is an [`ImageFormat`] with no RAW
//! member, while a RAW file's metadata includes the camera make and model that no `ImageInfo` field holds. See
//! `RawImageInfo` for why widening the one type was the worse trade.
//!
//! # Build notes
//!
//! The `avif`/`heif`/`webp` crates download prebuilt static codec binaries on first build (an internet connection is
//! required, or point `AVIF_BINARIES_DIR`/`HEIF_BINARIES_DIR`/`WEBP_BINARIES_DIR` at pre-extracted archives). They link
//! statically, so no system libraries are needed at runtime. The bundled AVIF (SVT-AV1) encoder keeps per-encode
//! global state, so this module serializes AVIF encodes against each other internally — encoding several AVIFs
//! concurrently is safe, though they will not run in parallel with one another.

// The module README is the long-form documentation; including it here is what puts it on docs.rs and turns its
// examples into doctests, so the prose cannot drift from the code without CI noticing.
#![doc = include_str!("README.md")]

mod decode_bytes;
mod decode_bytes_with_format;
mod decode_file;
mod dispatch;
mod encode_file;
mod encode_writer;
mod error;
mod format;
mod format_from_bytes;
mod info;
mod options;
mod probe_bytes;
mod probe_file;
#[cfg(feature = "image-raw")]
mod raw;

pub use decode_bytes::decode_bytes;
pub use decode_bytes_with_format::decode_bytes_with_format;
pub use decode_file::decode_file;
pub use encode_file::encode_file;
pub use encode_writer::encode_writer;
pub use error::{ImageError, Result};
pub use format::ImageFormat;
pub use format_from_bytes::format_from_bytes;
pub use info::ImageInfo;
pub use options::{Chroma, EncodeOptions, PngCompression, PngFilter, Preset};
pub use probe_bytes::probe_bytes;
pub use probe_file::probe_file;
#[cfg(feature = "image-raw")]
pub use raw::{
    RawFormat, RawImageInfo, decode_raw_bytes, decode_raw_file, is_raw_bytes, probe_raw_bytes, probe_raw_file,
};

#[cfg(test)]
mod tests;
