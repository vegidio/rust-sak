# `image` module

Encode and decode images across **8 formats** behind one small, uniform, synchronous API. Decoding always yields a `image::DynamicImage`; encoding takes one.

| Family           | Formats                                   | Backed by                                                                                         |
|------------------|-------------------------------------------|---------------------------------------------------------------------------------------------------|
| Native           | `bmp`, `gif`, `jpeg`/`jpg`, `png`, `tiff` | the [`image`](https://crates.io/crates/image) crate                                               |
| Dedicated codecs | `avif`, `heif`/`heic`, `webp`             | the author's `avif-rs` / `heif-rs` / `webp-rs` crates (never the `image` crate's built-in codecs) |

## Enabling

Gated behind the `image` Cargo feature:

```toml
[dependencies]
rust-sak = { version = "1", features = ["image"] }
# you'll also want the `image` crate for `DynamicImage`:
image = { version = "0.25", default-features = false }
```

```rust
use rust_sak::image::{
    decode_file, decode_bytes, decode_bytes_with_format, format_from_bytes,
    probe_file, probe_bytes,
    encode_file, encode_writer,
    ImageFormat, ImageInfo, EncodeOptions, PngCompression, PngFilter, Preset, Chroma,
    ImageError, Result,
};
```

> **Build note:** the `avif`/`heif`/`webp` crates download prebuilt **static** codec binaries on first build (internet required, or point `AVIF_BINARIES_DIR` / `HEIF_BINARIES_DIR` / `WEBP_BINARIES_DIR` at pre-extracted archives). No system libraries are needed at runtime. The AVIF (SVT-AV1) encoder keeps per-encode global state, so encoding several AVIFs **concurrently with different options is unsafe**, and it hangs on sub-16px frames.

## Public functions

All are synchronous and return `Result<T, ImageError>`.

### Decoding → `DynamicImage`

| Function                   | Signature                                                                                | What it does                                                                                                    |
|----------------------------|------------------------------------------------------------------------------------------|-----------------------------------------------------------------------------------------------------------------|
| `decode_file`              | `fn decode_file(path: impl AsRef<Path>) -> Result<DynamicImage>`                         | Reads and decodes a file; format chosen from the **extension**. `UnknownExtension` if unrecognized.             |
| `decode_bytes`             | `fn decode_bytes(bytes: &[u8]) -> Result<DynamicImage>`                                  | Decodes in-memory bytes; format guessed from the **magic bytes**. `UnrecognizedFormat` if no signature matches. |
| `decode_bytes_with_format` | `fn decode_bytes_with_format(bytes: &[u8], format: ImageFormat) -> Result<DynamicImage>` | Decodes in-memory bytes with an **explicit** format (no guessing).                                              |

### Detecting & probing (no decode)

| Function            | Signature                                                    | What it does                                                                                                              |
|---------------------|--------------------------------------------------------------|---------------------------------------------------------------------------------------------------------------------------|
| `format_from_bytes` | `fn format_from_bytes(bytes: &[u8]) -> Result<ImageFormat>`  | Sniffs the format from magic bytes **without decoding pixels**. `UnrecognizedFormat` if no match.                         |
| `probe_bytes`       | `fn probe_bytes(bytes: &[u8]) -> Result<ImageInfo>`          | Reads metadata (dimensions, color type, bit depth) from the header; format guessed from **magic bytes**. No pixel decode. |
| `probe_file`        | `fn probe_file(path: impl AsRef<Path>) -> Result<ImageInfo>` | Same, for a file; format from the **extension**. Reads the file bytes but never decodes pixels.                           |

### Encoding

| Function        | Signature                                                                                                                            | What it does                                                        |
|-----------------|--------------------------------------------------------------------------------------------------------------------------------------|---------------------------------------------------------------------|
| `encode_file`   | `fn encode_file(image: &DynamicImage, path: impl AsRef<Path>, options: Option<EncodeOptions>) -> Result<()>`                         | Encodes and writes to `path`; format chosen from the **extension**. |
| `encode_writer` | `fn encode_writer(image: &DynamicImage, writer: &mut impl Write, format: ImageFormat, options: Option<EncodeOptions>) -> Result<()>` | Encodes to any `Write` sink with an **explicit** format.            |

For both encoders, pass `options: None` to use the format's defaults. A `Some(_)` whose variant targets a **different** format than the destination yields `ImageError::FormatMismatch`.

## Types

### `ImageFormat`

`Bmp` · `Gif` · `Jpeg` · `Png` · `Tiff` · `Avif` · `Heif` · `WebP`. Helpers:

- `ImageFormat::from_extension(&str) -> Option<Self>` — case-insensitive, no leading dot (`jpg`↔`jpeg`, `heif`↔`heic`).
- `ImageFormat::from_path(impl AsRef<Path>) -> Option<Self>`
- `ImageFormat::from_magic(&[u8]) -> Option<Self>` — signature sniff (incl. ISO-BMFF `ftyp` brand to tell avif from heif).
- `ImageFormat::extension(self) -> &'static str` — canonical lowercase extension.

### `ImageInfo`

Returned by `probe_file` / `probe_bytes`. A plain `Copy` struct of header metadata:

```rust
pub struct ImageInfo {
    pub format: ImageFormat,
    pub width: u32,
    pub height: u32,
    pub color_type: image::ColorType, // the `image` crate's enum (channel layout + sample size)
    pub bit_depth: u8,                // bits per channel (8/10/12/16/…)
}
```

`bit_depth` carries the true per-channel depth — important for high-bit-depth AVIF/HEIF (10/12-bit), which `color_type` alone cannot tell apart from 16-bit.

### `EncodeOptions`

One variant per format — **the variant doubles as the format selector**. Build it directly, or use `EncodeOptions::default_for(format)`; `.format()` returns the `ImageFormat` it targets.

| Variant                | Tunables (with defaults)                                                                                                             |
|------------------------|--------------------------------------------------------------------------------------------------------------------------------------|
| `Bmp` / `Gif` / `Tiff` | none                                                                                                                                 |
| `Jpeg`                 | `quality: u8` (1–100, default 75)                                                                                                    |
| `Png`                  | `compression: PngCompression`, `filter: PngFilter`                                                                                   |
| `Avif`                 | `quality: u8` (0–100, default 60), `speed: u8` (0–10, default 6), `threads: Option<u32>` (default auto)                              |
| `Heif`                 | `quality: u8` (0–100, default 50), `preset: Preset` (default `Medium`), `chroma: Chroma` (default `Yuv420`)                          |
| `WebP`                 | `quality: u8` (default 75), `quality_alpha: u8` (default 100), `compression: u8` (0–6, default 4), `lossless: bool`, `threads: bool` |

Supporting enums:

- `PngCompression` — `Fast` / `Default` *(default)* / `Best`.
- `PngFilter` — `NoFilter` / `Sub` / `Up` / `Avg` / `Paeth` / `Adaptive` *(default)*.
- `Preset`, `Chroma` — re-exported from the `heif` crate (HEIF x265 speed preset and chroma subsampling).

### `ImageError` / `Result<T>`

`Result<T>` is an alias for `std::result::Result<T, ImageError>`. `ImageError` variants:

- Codec wrappers: `Image(image::ImageError)`, `Io(std::io::Error)`, `Webp(..)`, `Avif(..)`, `Heif(..)` (each with a `From` impl).
- Dispatch errors: `UnknownExtension`, `UnrecognizedFormat`, `FormatMismatch { expected, options }`.

Implements `Display` and `std::error::Error`.

## Usage

```rust
use image::{DynamicImage, RgbaImage};
use rust_sak::image::{
    decode_bytes, encode_writer, encode_file, format_from_bytes, ImageFormat, EncodeOptions,
};

// Round-trip a tiny image through PNG (a native codec — no extra binaries needed).
let original = DynamicImage::ImageRgba8(RgbaImage::new(2, 2));

let mut bytes = Vec::new();
encode_writer(&original, &mut bytes, ImageFormat::Png, None).unwrap();

assert_eq!(format_from_bytes(&bytes).unwrap(), ImageFormat::Png);

let decoded = decode_bytes(&bytes).unwrap();
assert_eq!((decoded.width(), decoded.height()), (2, 2));

// Encode to a file with custom options (format taken from the ".jpg" extension).
encode_file(&original, "/tmp/out.jpg", Some(EncodeOptions::Jpeg { quality: 90 })).unwrap();
```
