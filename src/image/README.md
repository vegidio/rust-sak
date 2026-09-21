# `image` module

Encode and decode images across **8 formats** behind one small, uniform, synchronous API — plus **camera RAW decoding** behind the optional `image-raw` feature. Decoding always yields a `image::DynamicImage`; encoding takes one.

| Family           | Formats                                   | Backed by                                                                                         |
|------------------|-------------------------------------------|---------------------------------------------------------------------------------------------------|
| Native           | `bmp`, `gif`, `jpeg`/`jpg`, `png`, `tiff` | the [`image`](https://crates.io/crates/image) crate                                               |
| Dedicated codecs | `avif`, `heif`/`heic`, `webp`             | the author's `avif-rs` / `heif-rs` / `webp-rs` crates (never the `image` crate's built-in codecs) |
| Camera RAW *(decode only)* | `dng`, `nef`, `cr2`/`cr3`, `arw`, `raf`, `orf`, `rw2` and ~20 more | `zenraw` on its `rawler` backend — behind the separate `image-raw` feature. See [RAW decoding](#raw-decoding-image-raw) |

## Enabling

Gated behind the `image` Cargo feature:

```toml
[dependencies]
rust-sak = { version = "2", features = ["image"] }
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

> **Build note:** the `avif`/`heif`/`webp` crates download prebuilt **static** codec binaries on first build (internet required, or point `AVIF_BINARIES_DIR` / `HEIF_BINARIES_DIR` / `WEBP_BINARIES_DIR` at pre-extracted archives). No system libraries are needed at runtime. The AVIF (SVT-AV1) encoder keeps per-encode global state, so this module serializes AVIF encodes against each other internally: encoding several AVIFs concurrently is **safe**, but they are processed one at a time rather than in parallel. It still hangs on sub-16px frames.

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

#### Every format writes every picture

Each codec natively accepts only some of `DynamicImage`'s ten colour types — GIF takes 8-bit colour and nothing else, libwebp is 8-bit throughout, TIFF has no grayscale-with-alpha at any depth. **You never have to care.** Before a picture reaches a codec it is converted to the colour type that codec accepts, so **every format encodes every picture**; there is no colour type that makes an encode fail.

The conversion loses as little as the destination forces:

- **Depth** comes down only where the encoder cannot hold it. `png`, `tiff`, `avif` and `heif` keep **16 bits per channel**; `bmp`, `gif`, `jpeg` and `webp` are **8-bit**, so a 16-bit picture is narrowed for those four. A 32-float picture written as PNG becomes 16-bit, not 8-bit.
- **Transparency** is kept wherever the format has an alpha channel, and dropped only where it does not (`jpeg`).
- **Grayscale** is expanded to colour only where the format has no grayscale representation (`gif`).

A picture the target already accepts is **not** converted — it is handed to the codec as it stands, so a lossless format writes it byte for byte. This matters most for RAW, which always develops to 16-bit: a `.nef` exported as PNG or TIFF keeps its depth, and the same file exported as WebP or JPEG comes back 8-bit.

## RAW decoding (`image-raw`)

Camera RAW is a **ninth family, decode only**, behind its own Cargo feature. A camera writes RAW and software reads it, so there is no encoder here and no `RawFormat` member of `ImageFormat`.

```toml
rust-sak = { version = "2", features = ["image-raw"] }  # implies "image"
```

With the feature on, **`decode_file` and `decode_bytes` open RAW too** — RAW is another arm of the ordinary decode path, not a parallel one, so code that opens whatever a user supplied needs no RAW-versus-native triage of its own:

```rust,ignore
// Works for png, jpeg, webp, avif, heif… and for nef, cr3, dng, arw.
let image = rust_sak::image::decode_file(path)?;
```

The functions below stay as the **narrow** forms, for a caller that wants anything non-RAW refused, and for the RAW-only metadata that `ImageInfo` cannot carry.

| Function          | Signature                                                            | What it does                                                                                            |
|-------------------|----------------------------------------------------------------------|---------------------------------------------------------------------------------------------------------|
| `decode_raw_bytes` | `fn decode_raw_bytes(bytes: &[u8]) -> Result<DynamicImage>`         | Develops RAW bytes into a display-ready picture. `NotRaw` if they are not RAW.                          |
| `decode_raw_file`  | `fn decode_raw_file(path: impl AsRef<Path>) -> Result<DynamicImage>` | Same, for a file. `UnknownExtension` if the path names no RAW format.                                   |
| `probe_raw_bytes`  | `fn probe_raw_bytes(bytes: &[u8]) -> Result<RawImageInfo>`          | Metadata without decoding pixels. Leaves `format` as `None` for the TIFF-based formats — see below.     |
| `probe_raw_file`   | `fn probe_raw_file(path: impl AsRef<Path>) -> Result<RawImageInfo>` | Same, for a file; always names the `format`, from the extension. **Reads the whole file** (see below).  |
| `is_raw_bytes`     | `fn is_raw_bytes(bytes: &[u8]) -> bool`                             | Cheap header check. **A plain TIFF answers `true`** — see below.                                         |

**Decoding always yields `DynamicImage::ImageRgb16`** — 16 bits per channel, sRGB. The decoder runs its full pipeline to get there: black/white-level normalisation, demosaic, white balance, the camera's colour matrix, tone curve and sRGB gamma, then the crop and EXIF orientation the camera recorded. Sixteen bits are kept rather than narrowed to eight because a RAW file exists precisely because the sensor recorded more than eight.

### Three things worth knowing

- **The extension is what distinguishes RAW formats, not the content.** Nearly every RAW format is a TIFF container, so a NEF, an ARW, a CR2 and a PEF all open with the same four bytes. `RawFormat::from_magic` therefore resolves only the containers that identify themselves (DNG, CR3, RAF, RW2, ORF) and returns `None` for the rest rather than guessing, and `is_raw_bytes` cannot tell an ordinary `.tiff` photograph from a NEF. **Route on the file extension**, not on content, or a genuine TIFF ends up in the RAW decoder.

  This is why `decode_file` opens every RAW format while `decode_bytes` opens only the self-identifying containers: given a name the answer is unambiguous, given bytes it is not. A native extension always wins, so a `.tiff` is never claimed for RAW. When you have the bytes and know they are RAW, call `decode_raw_bytes` and say so.
- **`probe_raw_file` reads the whole file**, where `probe_file` reads only a header. That is the backend's constraint, not a choice: RAW metadata lives in IFD chains whose offsets routinely point deep into a 40 MB file, so a bounded prefix would miss more often than it hit. Worth knowing if you are listing a directory of RAWs.
- **A camera the backend does not know is a refusal, never a wrong picture.** `rawler` reads 300-plus cameras; one outside that set comes back as `ImageError::Raw` naming the failure.

### Types

| Type           | What it is                                                                                                                            |
|----------------|-----------------------------------------------------------------------------------------------------------------------------------------|
| `RawFormat`    | The decode-only format enum — `Dng`, `Nef`, `Cr2`, `Cr3`, `Arw`, `Raf`, `Orf`, `Rw2` and ~17 more, with `from_extension`/`from_path`/`extension`/`from_magic`. |
| `RawImageInfo` | `format: Option<RawFormat>`, `width`, `height`, `bit_depth: Option<u8>`, `make`, `model`, `is_dng`. Separate from `ImageInfo` because `ImageInfo::format` has no RAW member, and because make and model are what a photographer's file listing wants. |

## Types

### `ImageFormat`

`Bmp` · `Gif` · `Jpeg` · `Png` · `Tiff` · `Avif` · `Heif` · `WebP`. Helpers:

- `ImageFormat::from_extension(&str) -> Option<Self>` — case-insensitive, no leading dot (`jpg`↔`jpeg`, `heif`↔`heic`).
- `ImageFormat::from_path(impl AsRef<Path>) -> Option<Self>`
- `ImageFormat::from_magic(&[u8]) -> Option<Self>` — signature sniff (incl. ISO-BMFF `ftyp` brand to tell avif from heif).
- `ImageFormat::extension(self) -> &'static str` — canonical lowercase extension.

### `ImageInfo`

Returned by `probe_file` / `probe_bytes`. A plain `Copy` struct of header metadata:

```rust,ignore
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

With `image-raw` on, two further variants appear: `Raw(zenraw::RawError)` for a decode or probe that failed, and `NotRaw` for bytes that are recognisable and are not RAW.

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
let out = std::env::temp_dir().join("out.jpg");
encode_file(&original, &out, Some(EncodeOptions::Jpeg { quality: 90 })).unwrap();
```
