use ::image::{ColorType, DynamicImage, Rgb, RgbImage};

use super::dispatch::make_compatible;
use super::*;

/// An 8x8 RGB gradient — RGB (no alpha) so every format, including JPEG, can encode it.
fn sample_image() -> DynamicImage {
    sample_image_sized(8, 8)
}

/// An RGB gradient of the given size. AV1-based encoders (AVIF) need a non-tiny frame, so those tests use a larger one.
fn sample_image_sized(width: u32, height: u32) -> DynamicImage {
    let mut img = RgbImage::new(width, height);
    for (x, y, px) in img.enumerate_pixels_mut() {
        *px = Rgb([(x * 4) as u8, (y * 4) as u8, 128]);
    }
    DynamicImage::ImageRgb8(img)
}

fn encode(image: &DynamicImage, format: ImageFormat) -> Vec<u8> {
    let mut bytes = Vec::new();
    encode_writer(image, &mut bytes, format, None).unwrap();
    bytes
}

#[test]
fn format_from_extension_maps_aliases() {
    assert_eq!(ImageFormat::from_extension("JPG"), Some(ImageFormat::Jpeg));
    assert_eq!(ImageFormat::from_extension("jpeg"), Some(ImageFormat::Jpeg));
    assert_eq!(ImageFormat::from_extension("heic"), Some(ImageFormat::Heif));
    assert_eq!(ImageFormat::from_extension("heif"), Some(ImageFormat::Heif));
    assert_eq!(ImageFormat::from_extension("xyz"), None);
    assert_eq!(ImageFormat::from_path("/a/b/photo.PNG"), Some(ImageFormat::Png));
    assert_eq!(ImageFormat::from_path("/a/b/noext"), None);
}

#[test]
fn detects_native_formats_from_magic() {
    let img = sample_image();
    for format in [
        ImageFormat::Png,
        ImageFormat::Jpeg,
        ImageFormat::Bmp,
        ImageFormat::Gif,
        ImageFormat::Tiff,
    ] {
        let bytes = encode(&img, format);
        assert_eq!(format_from_bytes(&bytes).unwrap(), format, "format {format:?}");
    }
}

#[test]
fn native_round_trips_preserve_dimensions() {
    let img = sample_image();
    for format in [
        ImageFormat::Png,
        ImageFormat::Jpeg,
        ImageFormat::Bmp,
        ImageFormat::Gif,
        ImageFormat::Tiff,
    ] {
        let bytes = encode(&img, format);
        let decoded = decode_bytes(&bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (8, 8), "format {format:?}");
    }
}

#[test]
fn png_round_trip_is_lossless() {
    let img = sample_image();
    let bytes = encode(&img, ImageFormat::Png);
    let decoded = decode_bytes_with_format(&bytes, ImageFormat::Png).unwrap();
    assert_eq!(decoded.to_rgb8(), img.to_rgb8());
}

#[test]
fn encode_file_then_decode_file() {
    let img = sample_image();
    let path = std::env::temp_dir().join(format!("rust_sak_image_{}.png", std::process::id()));
    encode_file(&img, &path, None).unwrap();
    let decoded = decode_file(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (8, 8));
}

#[test]
fn jpeg_quality_option_is_honored() {
    let img = sample_image();
    let bytes = encode(&img, ImageFormat::Jpeg);
    assert_eq!(format_from_bytes(&bytes).unwrap(), ImageFormat::Jpeg);
    // Explicit options for the matching format are accepted.
    let mut tuned = Vec::new();
    encode_writer(
        &img,
        &mut tuned,
        ImageFormat::Jpeg,
        Some(EncodeOptions::Jpeg { quality: 50 }),
    )
    .unwrap();
    assert_eq!(format_from_bytes(&tuned).unwrap(), ImageFormat::Jpeg);
}

#[test]
fn unknown_extension_errors() {
    let img = sample_image();
    assert!(matches!(
        encode_file(&img, "/tmp/file.xyz", None),
        Err(ImageError::UnknownExtension)
    ));
    assert!(matches!(
        decode_file("/tmp/file.xyz"),
        Err(ImageError::UnknownExtension)
    ));
}

#[test]
fn unrecognized_bytes_error() {
    assert!(matches!(
        format_from_bytes(&[0, 1, 2, 3]),
        Err(ImageError::UnrecognizedFormat)
    ));
    assert!(matches!(
        decode_bytes(&[0, 1, 2, 3]),
        Err(ImageError::UnrecognizedFormat)
    ));
}

#[test]
fn options_format_mismatch_errors() {
    let img = sample_image();
    let mut bytes = Vec::new();
    let err = encode_writer(
        &img,
        &mut bytes,
        ImageFormat::Png,
        Some(EncodeOptions::Jpeg { quality: 80 }),
    )
    .unwrap_err();
    assert!(matches!(
        err,
        ImageError::FormatMismatch {
            expected: ImageFormat::Png,
            options: ImageFormat::Jpeg
        }
    ));
}

#[test]
fn webp_round_trip() {
    let img = sample_image();
    let bytes = encode(&img, ImageFormat::WebP);
    assert_eq!(format_from_bytes(&bytes).unwrap(), ImageFormat::WebP);
    let decoded = decode_bytes(&bytes).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (8, 8));
}

#[test]
fn avif_round_trip() {
    let img = sample_image_sized(64, 64);
    let bytes = encode(&img, ImageFormat::Avif);
    assert_eq!(format_from_bytes(&bytes).unwrap(), ImageFormat::Avif);
    let decoded = decode_bytes(&bytes).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (64, 64));
}

#[test]
fn heif_round_trip() {
    let img = sample_image();
    let bytes = encode(&img, ImageFormat::Heif);
    assert_eq!(format_from_bytes(&bytes).unwrap(), ImageFormat::Heif);
    let decoded = decode_bytes(&bytes).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (8, 8));
}

#[test]
fn probe_bytes_reports_dimensions_and_format() {
    let img = sample_image();
    for format in [
        ImageFormat::Png,
        ImageFormat::Jpeg,
        ImageFormat::Bmp,
        ImageFormat::Gif,
        ImageFormat::Tiff,
    ] {
        let bytes = encode(&img, format);
        let info = probe_bytes(&bytes).unwrap();
        assert_eq!(info.format, format, "format {format:?}");
        assert_eq!((info.width, info.height), (8, 8), "format {format:?}");
        assert_eq!(info.bit_depth, 8, "format {format:?}");
    }
}

#[test]
fn probe_matches_decode() {
    let img = sample_image();
    let bytes = encode(&img, ImageFormat::Png);
    let info = probe_bytes(&bytes).unwrap();
    let decoded = decode_bytes(&bytes).unwrap();
    assert_eq!((info.width, info.height), (decoded.width(), decoded.height()));
}

#[test]
fn probe_file_reads_metadata() {
    let img = sample_image();
    let path = std::env::temp_dir().join(format!("rust_sak_probe_{}.png", std::process::id()));
    encode_file(&img, &path, None).unwrap();
    let info = probe_file(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(info.format, ImageFormat::Png);
    assert_eq!((info.width, info.height), (8, 8));
    assert_eq!(info.bit_depth, 8);
}

#[test]
fn probe_bytes_handles_dedicated_codecs() {
    // AV1 encoders need a non-tiny frame, so AVIF uses a larger sample.
    for (format, img) in [
        (ImageFormat::WebP, sample_image()),
        (ImageFormat::Avif, sample_image_sized(64, 64)),
        (ImageFormat::Heif, sample_image()),
    ] {
        let bytes = encode(&img, format);
        let info = probe_bytes(&bytes).unwrap();
        assert_eq!(info.format, format, "format {format:?}");
        assert_eq!(
            (info.width, info.height),
            (img.width(), img.height()),
            "format {format:?}"
        );
    }
}

#[test]
fn probe_file_handles_dedicated_codecs_and_matches_probe_bytes() {
    // These three codecs parse a byte slice, so `probe_file` reads a bounded prefix rather than streaming. This is the
    // check that the prefix is actually enough, and that the file and byte paths agree.
    for (format, img) in [
        (ImageFormat::WebP, sample_image()),
        (ImageFormat::Avif, sample_image_sized(64, 64)),
        (ImageFormat::Heif, sample_image()),
    ] {
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "rust_sak_probe_file_{}_{}.{}",
            std::process::id(),
            format.extension(),
            format.extension(),
        ));
        encode_file(&img, &path, None).unwrap();

        let from_file = probe_file(&path).unwrap();
        let from_bytes = probe_bytes(&std::fs::read(&path).unwrap()).unwrap();
        std::fs::remove_file(&path).unwrap();

        assert_eq!(from_file.format, format, "format {format:?}");
        assert_eq!(
            (from_file.width, from_file.height),
            (img.width(), img.height()),
            "format {format:?}"
        );
        assert_eq!(
            (from_file.width, from_file.height, from_file.bit_depth),
            (from_bytes.width, from_bytes.height, from_bytes.bit_depth),
            "file and byte probes disagree for {format:?}"
        );
    }
}

#[test]
fn probe_file_reads_only_the_header_of_a_native_format() {
    // A native probe streams from the file, so trailing junk past the image is never read. Appending 8 MiB of it makes
    // the difference observable: a `fs::read`-based probe would pull all of it in.
    let img = sample_image();
    let path = std::env::temp_dir().join(format!("rust_sak_probe_tail_{}.png", std::process::id()));
    encode_file(&img, &path, None).unwrap();

    let mut padded = std::fs::read(&path).unwrap();
    padded.extend(std::iter::repeat_n(0_u8, 8 << 20));
    std::fs::write(&path, &padded).unwrap();

    let info = probe_file(&path).unwrap();
    std::fs::remove_file(&path).unwrap();

    assert_eq!((info.width, info.height), (8, 8));
}

#[test]
fn concurrent_avif_encodes_with_different_options_stay_correct() {
    // The bundled SVT-AV1 encoder keeps per-encode global state, so this is the regression test for the mutex that
    // serializes the AVIF arm. Without it the two threads' settings can bleed into each other.
    let img = sample_image_sized(64, 64);

    let handles: Vec<_> = [20_u8, 90]
        .into_iter()
        .map(|quality| {
            let img = img.clone();
            std::thread::spawn(move || {
                let mut bytes = Vec::new();
                encode_writer(
                    &img,
                    &mut bytes,
                    ImageFormat::Avif,
                    Some(EncodeOptions::Avif {
                        quality,
                        speed: 10,
                        threads: Some(1),
                    }),
                )
                .unwrap();
                bytes
            })
        })
        .collect();

    for handle in handles {
        let bytes = handle.join().unwrap();
        let info = probe_bytes(&bytes).unwrap();
        assert_eq!(info.format, ImageFormat::Avif);
        assert_eq!((info.width, info.height), (64, 64));
    }
}

#[test]
fn probe_unknown_extension_errors() {
    assert!(matches!(probe_file("/tmp/file.xyz"), Err(ImageError::UnknownExtension)));
}

#[test]
fn probe_unrecognized_bytes_error() {
    assert!(matches!(
        probe_bytes(&[0, 1, 2, 3]),
        Err(ImageError::UnrecognizedFormat)
    ));
}

// ── Colour-type compatibility ──────────────────────────────────────────────────────────────────────────────────
//
// Every codec accepts a fixed set of colour types and refuses the rest, so an ordinary picture — a developed camera
// RAW is 16-bit, a grayscale PNG has no colour — used to be unwritable in some formats. The dispatch converts to
// what the target accepts before a codec is reached; these pin the table it follows and the invariant that nothing
// reaches a codec without it.

/// Every supported format, so the tests below enumerate rather than sample. The column order of [`CONVERSIONS`].
const EVERY_FORMAT: [ImageFormat; 8] = [
    ImageFormat::Bmp,
    ImageFormat::Gif,
    ImageFormat::Jpeg,
    ImageFormat::Png,
    ImageFormat::Tiff,
    ImageFormat::Avif,
    ImageFormat::Heif,
    ImageFormat::WebP,
];

/// What each format converts each source colour type to, `None` where the encoder takes the picture as it stands.
///
/// Written out cell by cell rather than derived, so it is an independent statement of the intended behaviour rather
/// than a second copy of the code under test. BMP and JPEG are `None` in every row because their own
/// `make_compatible_img` hook runs afterwards; GIF is the narrowest column because its encoder takes 8-bit colour and
/// nothing else; TIFF moves only grayscale-with-alpha, which it has at no depth.
#[rustfmt::skip]
const CONVERSIONS: [(ColorType, [Option<ColorType>; 8]); 10] = {
    use ColorType::{L8, L16, La8, La16, Rgb8, Rgb16, Rgb32F, Rgba8, Rgba16, Rgba32F};

    //        source     BMP   GIF                 JPEG  PNG                 TIFF                 AVIF                 HEIF                 WebP
    [
        (L8,      [None, Some(Rgb8),  None, None,          None,          None,          None,          None         ]),
        (La8,     [None, Some(Rgba8), None, None,          Some(Rgba8),   None,          None,          None         ]),
        (Rgb8,    [None, None,        None, None,          None,          None,          None,          None         ]),
        (Rgba8,   [None, None,        None, None,          None,          None,          None,          None         ]),
        (L16,     [None, Some(Rgb8),  None, None,          None,          None,          None,          Some(L8)     ]),
        (La16,    [None, Some(Rgba8), None, None,          Some(Rgba16),  None,          None,          Some(La8)    ]),
        (Rgb16,   [None, Some(Rgb8),  None, None,          None,          None,          None,          Some(Rgb8)   ]),
        (Rgba16,  [None, Some(Rgba8), None, None,          None,          None,          None,          Some(Rgba8)  ]),
        (Rgb32F,  [None, Some(Rgb8),  None, Some(Rgb16),   None,          Some(Rgb16),   Some(Rgb16),   Some(Rgb8)   ]),
        (Rgba32F, [None, Some(Rgba8), None, Some(Rgba16),  None,          Some(Rgba16),  Some(Rgba16),  Some(Rgba8)  ]),
    ]
};

/// One image of every `DynamicImage` variant, each holding the same picture at the given size.
fn every_color_type(width: u32, height: u32) -> Vec<DynamicImage> {
    let img = sample_image_sized(width, height);
    vec![
        DynamicImage::ImageLuma8(img.to_luma8()),
        DynamicImage::ImageLumaA8(img.to_luma_alpha8()),
        DynamicImage::ImageRgb8(img.to_rgb8()),
        DynamicImage::ImageRgba8(img.to_rgba8()),
        DynamicImage::ImageLuma16(img.to_luma16()),
        DynamicImage::ImageLumaA16(img.to_luma_alpha16()),
        DynamicImage::ImageRgb16(img.to_rgb16()),
        DynamicImage::ImageRgba16(img.to_rgba16()),
        DynamicImage::ImageRgb32F(img.to_rgb32f()),
        DynamicImage::ImageRgba32F(img.to_rgba32f()),
    ]
}

/// A 16-bit picture, which is what every developed camera RAW is and what half the table exists for.
fn sixteen_bit_image() -> DynamicImage {
    DynamicImage::ImageRgb16(sample_image().to_rgb16())
}

/// The same picture with a varying alpha channel, so that a codec keeping transparency is distinguishable from one
/// that noticed the image was opaque and left the channel out.
fn sixteen_bit_transparent_image() -> DynamicImage {
    let mut img = sample_image().to_rgba16();
    for (x, _, px) in img.enumerate_pixels_mut() {
        px[3] = (x * 8192) as u16;
    }
    DynamicImage::ImageRgba16(img)
}

#[test]
fn every_color_type_converts_to_what_its_target_format_accepts() {
    let images = every_color_type(8, 8);
    assert_eq!(
        images.len(),
        CONVERSIONS.len(),
        "the table must have a row for every DynamicImage variant"
    );

    for image in images {
        let source = image.color();
        let row = CONVERSIONS
            .iter()
            .find(|(color, _)| *color == source)
            .unwrap_or_else(|| panic!("{source:?} has no row in the table"))
            .1;

        for (format, expected) in EVERY_FORMAT.into_iter().zip(row) {
            let actual = make_compatible(&image, format).map(|converted| converted.color());
            assert_eq!(actual, expected, "{source:?} written as {format:?}");
        }
    }
}

#[test]
fn every_color_type_encodes_in_every_format_through_both_paths() {
    // Big enough for the AV1 encoder behind AVIF, which will not take a tiny frame.
    let (width, height) = (32, 32);
    let dir = std::env::temp_dir();

    for image in every_color_type(width, height) {
        let source = image.color();

        for format in EVERY_FORMAT {
            // A `Vec` is `Write` but not `Seek`, so this is the path BMP, GIF and TIFF buffer whole.
            let mut bytes = Vec::new();
            encode_writer(&image, &mut bytes, format, None)
                .unwrap_or_else(|e| panic!("{source:?} as {format:?} through a writer: {e}"));
            let decoded = decode_bytes_with_format(&bytes, format)
                .unwrap_or_else(|e| panic!("{source:?} as {format:?} did not decode: {e}"));
            assert_eq!(
                (decoded.width(), decoded.height()),
                (width, height),
                "{source:?} as {format:?} through a writer"
            );

            // And the streaming path, which reaches the same dispatch by a different route.
            let path = dir.join(format!(
                "rust_sak_compat_{}_{source:?}.{}",
                std::process::id(),
                format.extension()
            ));
            encode_file(&image, &path, None).unwrap_or_else(|e| panic!("{source:?} as {format:?} to a file: {e}"));
            let decoded = decode_file(&path).unwrap_or_else(|e| panic!("{source:?} as {format:?} from a file: {e}"));
            std::fs::remove_file(&path).unwrap();
            assert_eq!(
                (decoded.width(), decoded.height()),
                (width, height),
                "{source:?} as {format:?} through a file"
            );
        }
    }
}

#[test]
fn depth_survives_where_the_target_format_holds_it() {
    let image = sixteen_bit_image();
    for format in [ImageFormat::Png, ImageFormat::Tiff] {
        let info = probe_bytes(&encode(&image, format)).unwrap();
        assert_eq!(info.bit_depth, 16, "format {format:?}");
    }
}

#[test]
fn depth_is_narrowed_only_where_the_target_format_forces_it() {
    let image = sixteen_bit_image();
    for format in [ImageFormat::Gif, ImageFormat::WebP] {
        let bytes = encode(&image, format);
        assert_eq!(probe_bytes(&bytes).unwrap().bit_depth, 8, "format {format:?}");
        let decoded = decode_bytes_with_format(&bytes, format).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (8, 8), "format {format:?}");
    }
}

#[test]
fn alpha_survives_where_the_target_format_holds_it() {
    // 16-bit *and* genuinely transparent: an opaque alpha channel is one a codec is free to leave out, so the
    // fixture varies it to make keeping it observable. PNG keeps the depth as well; WebP has to give that up.
    let image = sixteen_bit_transparent_image();
    for format in [ImageFormat::Png, ImageFormat::WebP] {
        let info = probe_bytes(&encode(&image, format)).unwrap();
        assert!(
            info.color_type.has_alpha(),
            "format {format:?} dropped the alpha channel"
        );
    }
}

#[test]
fn a_picture_the_format_accepts_is_written_unchanged() {
    // Nothing to convert, so nothing is allocated and the pixels survive a lossless round trip exactly.
    let image = sample_image();
    assert!(make_compatible(&image, ImageFormat::Png).is_none());

    let decoded = decode_bytes_with_format(&encode(&image, ImageFormat::Png), ImageFormat::Png).unwrap();
    assert_eq!(decoded, image);
}

#[test]
fn bmp_and_jpeg_write_a_sixteen_bit_picture_with_their_own_hook() {
    // The dispatch leaves these two alone because each implements the `image` crate's `make_compatible_img`, which
    // runs inside `write_with_encoder` afterwards. If a future `image` release drops either hook, the encode starts
    // failing and this test is where it shows up rather than a caller's export.
    let image = sixteen_bit_image();
    for format in [ImageFormat::Bmp, ImageFormat::Jpeg] {
        assert!(
            make_compatible(&image, format).is_none(),
            "the dispatch must not convert for {format:?}"
        );
        let decoded = decode_bytes_with_format(&encode(&image, format), format)
            .unwrap_or_else(|e| panic!("{format:?} no longer converts 16-bit input itself: {e}"));
        assert_eq!((decoded.width(), decoded.height()), (8, 8), "format {format:?}");
    }
}

// ── RAW ────────────────────────────────────────────────────────────────────────────────────────────────────────
//
// Behind the same gate as the code they cover: `image-raw` is off by default, and a build without it has no `RawFormat`
// for these to name.

#[cfg(feature = "image-raw")]
mod raw {
    use super::super::*;
    use super::{encode, sample_image};
    use crate::image::raw_dispatch::image_from_pixel_buffer;
    use ::image::DynamicImage;

    /// Every extension the backend accepts, paired with the variant it must map to. This is the table `rawler`
    /// publishes as `supported_extensions()`, spelled out so a backend that drops or adds a format is caught here
    /// rather than at a user's file dialog.
    const EXTENSIONS: &[(&str, RawFormat)] = &[
        ("ari", RawFormat::Ari),
        ("arw", RawFormat::Arw),
        ("crw", RawFormat::Crw),
        ("cr2", RawFormat::Cr2),
        ("cr3", RawFormat::Cr3),
        ("crm", RawFormat::Cr3),
        ("dcr", RawFormat::Dcr),
        ("dcs", RawFormat::Dcs),
        ("dng", RawFormat::Dng),
        ("erf", RawFormat::Erf),
        ("fff", RawFormat::Fff),
        ("iiq", RawFormat::Iiq),
        ("kdc", RawFormat::Kdc),
        ("mef", RawFormat::Mef),
        ("mos", RawFormat::Mos),
        ("mrw", RawFormat::Mrw),
        ("nef", RawFormat::Nef),
        ("nrw", RawFormat::Nrw),
        ("orf", RawFormat::Orf),
        ("ori", RawFormat::Orf),
        ("pef", RawFormat::Pef),
        ("qtk", RawFormat::Qtk),
        ("raf", RawFormat::Raf),
        ("raw", RawFormat::Rw2),
        ("rw2", RawFormat::Rw2),
        ("rwl", RawFormat::Rw2),
        ("srw", RawFormat::Srw),
        ("3fr", RawFormat::ThreeFr),
        ("x3f", RawFormat::X3f),
    ];

    /// One of each variant, for the tests that must cover the enum exhaustively rather than the extension table.
    const EVERY_VARIANT: &[RawFormat] = &[
        RawFormat::Ari,
        RawFormat::Arw,
        RawFormat::Crw,
        RawFormat::Cr2,
        RawFormat::Cr3,
        RawFormat::Dcr,
        RawFormat::Dcs,
        RawFormat::Dng,
        RawFormat::Erf,
        RawFormat::Fff,
        RawFormat::Iiq,
        RawFormat::Kdc,
        RawFormat::Mef,
        RawFormat::Mos,
        RawFormat::Mrw,
        RawFormat::Nef,
        RawFormat::Nrw,
        RawFormat::Orf,
        RawFormat::Pef,
        RawFormat::Qtk,
        RawFormat::Raf,
        RawFormat::Rw2,
        RawFormat::Srw,
        RawFormat::ThreeFr,
        RawFormat::X3f,
    ];

    // ── The DNG fixture ───────────────────────────────────────────────────────────────────────────────────────
    //
    // Built here rather than committed. `rust-sak` has no `include` key, so everything in the repository ships in
    // the published crate, and a 10-30 MB camera file for one test is not a reasonable thing to make every consumer
    // download. Building it also makes the test *say* something: the sensor values are known, so the assertion can
    // be "a red CFA develops to a red picture" rather than "it did not error".
    //
    // `rawler`'s DNG decoder reads each of these tags from the IFD and falls back to a default where one is absent,
    // so a minimal file is a supported file rather than a lucky one.

    /// TIFF field types, by their on-disk codes.
    const BYTE: u16 = 1;
    const ASCII: u16 = 2;
    const SHORT: u16 = 3;
    const LONG: u16 = 4;
    const RATIONAL: u16 = 5;
    const SRATIONAL: u16 = 10;

    /// CFA colour codes, as `CFAPattern` spells them.
    const RED: u8 = 0;
    const GREEN: u8 = 1;
    const BLUE: u8 = 2;

    /// The sensor value written where the CFA says "this photosite saw the bright channel".
    const BRIGHT: u16 = 60_000;
    /// The sensor value written everywhere else.
    const DIM: u16 = 2_000;

    /// One IFD entry, with its payload not yet placed.
    struct Field {
        tag: u16,
        kind: u16,
        count: u32,
        payload: Vec<u8>,
    }

    fn field(tag: u16, kind: u16, count: u32, payload: Vec<u8>) -> Field {
        Field {
            tag,
            kind,
            count,
            payload,
        }
    }

    fn shorts(tag: u16, values: &[u16]) -> Field {
        let payload = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        field(tag, SHORT, values.len() as u32, payload)
    }

    fn longs(tag: u16, values: &[u32]) -> Field {
        let payload = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        field(tag, LONG, values.len() as u32, payload)
    }

    fn bytes_field(tag: u16, values: &[u8]) -> Field {
        field(tag, BYTE, values.len() as u32, values.to_vec())
    }

    fn ascii(tag: u16, text: &str) -> Field {
        let mut payload = text.as_bytes().to_vec();
        payload.push(0);
        let count = payload.len() as u32;
        field(tag, ASCII, count, payload)
    }

    /// Unsigned rationals, written as numerator/denominator pairs.
    fn rationals(tag: u16, values: &[(u32, u32)]) -> Field {
        let payload = values
            .iter()
            .flat_map(|(n, d)| [n.to_le_bytes(), d.to_le_bytes()])
            .flatten()
            .collect();
        field(tag, RATIONAL, values.len() as u32, payload)
    }

    /// Signed rationals, as `ColorMatrix1` needs — a camera matrix has negative terms.
    fn srationals(tag: u16, values: &[(i32, i32)]) -> Field {
        let payload = values
            .iter()
            .flat_map(|(n, d)| [n.to_le_bytes(), d.to_le_bytes()])
            .flatten()
            .collect();
        field(tag, SRATIONAL, values.len() as u32, payload)
    }

    /// Builds a complete little-endian DNG: an uncompressed 16-bit Bayer CFA image whose bright photosites are the
    /// ones `cfa` labels with `bright_channel`.
    ///
    /// The layout is the plain one the TIFF spec describes — 8-byte header, then IFD0, then the payloads too large
    /// to sit inside an entry, then the strip. Entries must be written in ascending tag order, which is why the
    /// list below is sorted rather than grouped by meaning.
    fn minimal_dng(width: u32, height: u32, cfa: [u8; 4], bright_channel: u8) -> Vec<u8> {
        // The sensor readings themselves: a photosite of the bright channel is bright, everything else is dim, plus
        // a gentle gradient so the picture is not one flat colour and a transposed row would show.
        let mut strip = Vec::with_capacity((width * height * 2) as usize);
        for y in 0..height {
            for x in 0..width {
                let colour = cfa[((y % 2) * 2 + (x % 2)) as usize];
                let base = if colour == bright_channel { BRIGHT } else { DIM };
                let gradient = ((x + y) * 8) as u16;
                strip.extend((base.saturating_add(gradient)).to_le_bytes());
            }
        }

        let fields = vec![
            longs(254, &[0]),                   // NewSubfileType: the full-resolution image
            longs(256, &[width]),               // ImageWidth
            longs(257, &[height]),              // ImageLength
            shorts(258, &[16]),                 // BitsPerSample
            shorts(259, &[1]),                  // Compression: none
            shorts(262, &[32803]),              // PhotometricInterpretation: CFA
            ascii(271, "rust-sak"),             // Make
            ascii(272, "Synthetic DNG"),        // Model
            longs(273, &[0]),                   // StripOffsets — patched once the layout is known
            shorts(277, &[1]),                  // SamplesPerPixel
            longs(278, &[height]),              // RowsPerStrip: the whole image in one strip
            longs(279, &[strip.len() as u32]),  // StripByteCounts
            shorts(284, &[1]),                  // PlanarConfiguration: chunky
            shorts(33421, &[2, 2]),             // CFARepeatPatternDim
            bytes_field(33422, &cfa),           // CFAPattern
            bytes_field(50706, &[1, 4, 0, 0]),  // DNGVersion 1.4.0.0
            bytes_field(50707, &[1, 1, 0, 0]),  // DNGBackwardVersion
            ascii(50708, "rust-sak synthetic"), // UniqueCameraModel
            shorts(50714, &[0]),                // BlackLevel
            longs(50717, &[65535]),             // WhiteLevel
            // ColorMatrix1 maps XYZ to camera. Identity keeps the fixture's arithmetic legible: whatever the
            // demosaic produces per channel is what the picture shows, so a red CFA cannot come out red by way of
            // a matrix that happened to boost red.
            srationals(
                50721,
                &[(1, 1), (0, 1), (0, 1), (0, 1), (1, 1), (0, 1), (0, 1), (0, 1), (1, 1)],
            ),
            // AsShotNeutral of 1:1:1 means no white-balance scaling, for the same reason.
            rationals(50728, &[(1, 1), (1, 1), (1, 1)]),
            shorts(50778, &[21]), // CalibrationIlluminant1: D65
        ];

        // A payload of four bytes or fewer lives inside the entry; anything larger is placed after the IFD and
        // referenced by offset.
        const HEADER: usize = 8;
        let ifd_len = 2 + fields.len() * 12 + 4;
        let heap_start = HEADER + ifd_len;

        let mut heap = Vec::new();
        let mut entries = Vec::with_capacity(fields.len() * 12);
        let mut strip_offset_position = None;

        for f in &fields {
            entries.extend(f.tag.to_le_bytes());
            entries.extend(f.kind.to_le_bytes());
            entries.extend(f.count.to_le_bytes());

            if f.tag == 273 {
                // StripOffsets points at the strip, which sits after the heap — so remember where to patch it.
                strip_offset_position = Some(entries.len());
                entries.extend(0_u32.to_le_bytes());
            } else if f.payload.len() <= 4 {
                let mut inline = f.payload.clone();
                inline.resize(4, 0);
                entries.extend(inline);
            } else {
                entries.extend(((heap_start + heap.len()) as u32).to_le_bytes());
                heap.extend(&f.payload);
                // Offsets must be even, which a trailing odd-length ASCII payload can break.
                if heap.len() % 2 != 0 {
                    heap.push(0);
                }
            }
        }

        let strip_start = heap_start + heap.len();
        let position = strip_offset_position.expect("StripOffsets is in the field list");
        entries[position..position + 4].copy_from_slice(&(strip_start as u32).to_le_bytes());

        let mut dng = Vec::with_capacity(strip_start + strip.len());
        dng.extend(b"II"); // little-endian
        dng.extend(42_u16.to_le_bytes()); // the TIFF magic number
        dng.extend((HEADER as u32).to_le_bytes()); // IFD0 starts immediately
        dng.extend((fields.len() as u16).to_le_bytes());
        dng.extend(&entries);
        dng.extend(0_u32.to_le_bytes()); // no IFD1
        dng.extend(&heap);
        dng.extend(&strip);
        dng
    }

    /// A red-dominant RGGB fixture — the one most tests want.
    fn red_dng() -> Vec<u8> {
        minimal_dng(64, 64, [RED, GREEN, GREEN, BLUE], RED)
    }

    /// The mean of each channel across the whole picture.
    fn channel_means(image: &DynamicImage) -> [f64; 3] {
        let rgb = image.as_rgb16().expect("a developed raw is 16-bit rgb");
        let mut totals = [0.0_f64; 3];
        for pixel in rgb.pixels() {
            for (total, sample) in totals.iter_mut().zip(pixel.0) {
                *total += f64::from(sample);
            }
        }
        let count = rgb.pixels().len() as f64;
        totals.map(|total| total / count)
    }

    /// A known 16-bit RGB pattern: every sample distinct, so a row copied from the wrong offset cannot match by
    /// accident the way a flat or symmetric pattern could.
    fn rgb16_pattern(width: u32, height: u32) -> Vec<u16> {
        let mut samples = Vec::with_capacity((width * height * 3) as usize);
        for y in 0..height {
            for x in 0..width {
                let base = (y * width + x) * 3;
                samples.extend([base as u16, base as u16 + 1, base as u16 + 2]);
            }
        }
        samples
    }

    #[test]
    fn the_strided_and_contiguous_conversions_agree() {
        use ::zenpixels::{PixelBuffer, PixelDescriptor};

        // 5 pixels wide at 6 bytes each is 30 bytes, which is not a multiple of lcm(6, 64) = 192 — so the
        // SIMD-aligned buffer is genuinely padded and the slow path is genuinely taken.
        let (width, height) = (5, 4);
        let expected = rgb16_pattern(width, height);

        let packed: Vec<u8> = expected.iter().flat_map(|s| s.to_ne_bytes()).collect();
        let contiguous = PixelBuffer::from_vec(packed, width, height, PixelDescriptor::RGB16_SRGB).unwrap();

        let mut strided = PixelBuffer::new_simd_aligned(width, height, PixelDescriptor::RGB16_SRGB, 64);
        for y in 0..height {
            let row: Vec<u8> = expected[(y * width * 3) as usize..((y + 1) * width * 3) as usize]
                .iter()
                .flat_map(|s| s.to_ne_bytes())
                .collect();
            strided.as_slice_mut().row_mut(y).copy_from_slice(&row);
        }

        // The premise of the test: one buffer takes each branch. Without this the two could agree by both being
        // contiguous, and the row-by-row path would never run.
        assert!(
            contiguous.as_slice().is_contiguous(),
            "the packed buffer should be contiguous"
        );
        assert!(
            !strided.as_slice().is_contiguous(),
            "the SIMD-aligned buffer should be padded, or this test proves nothing"
        );

        let from_contiguous = image_from_pixel_buffer(&contiguous).unwrap();
        let from_strided = image_from_pixel_buffer(&strided).unwrap();

        let DynamicImage::ImageRgb16(ref packed_image) = from_contiguous else {
            panic!("expected ImageRgb16, got {:?}", from_contiguous.color());
        };
        assert_eq!(packed_image.as_raw(), &expected, "the contiguous path lost the pattern");
        assert_eq!(
            from_strided, from_contiguous,
            "the row-by-row path disagrees with the contiguous one — stride is being mishandled"
        );
        assert_eq!((from_strided.width(), from_strided.height()), (width, height));
    }

    #[test]
    fn a_buffer_that_is_not_16_bit_rgb_is_refused() {
        use ::zenpixels::{PixelBuffer, PixelDescriptor};

        // Nothing in this module produces one — `Develop` always yields RGB16 — but the conversion says so rather
        // than reinterpreting whatever bytes it is handed.
        let buffer = PixelBuffer::from_vec(vec![0_u8; 4 * 4 * 4], 4, 4, PixelDescriptor::RGBA8_SRGB).unwrap();
        assert!(matches!(image_from_pixel_buffer(&buffer), Err(ImageError::Raw(_))));
    }

    #[test]
    fn the_two_raw_error_variants_name_their_failures_distinctly() {
        let not_raw = ImageError::NotRaw.to_string();
        let decode_failed = ImageError::Raw(::zenraw::RawError::Unsupported("no such camera".into())).to_string();

        // Each says which of the two things went wrong, rather than both reducing to "raw error".
        assert_eq!(not_raw, "the bytes are not a camera raw file");
        assert_eq!(decode_failed, "raw codec error: unsupported: no such camera");
        assert_ne!(not_raw, decode_failed);

        // And neither is confusable with the module's pre-existing refusals, which mean different things.
        for other in [ImageError::UnrecognizedFormat, ImageError::UnknownExtension] {
            assert_ne!(other.to_string(), not_raw);
            assert_ne!(other.to_string(), decode_failed);
        }
    }

    #[test]
    fn raw_image_info_is_a_value_that_compares() {
        let info = RawImageInfo {
            format: Some(RawFormat::Nef),
            width: 8256,
            height: 5504,
            bit_depth: Some(14),
            make: "NIKON CORPORATION".into(),
            model: "NIKON Z 8".into(),
            is_dng: false,
        };
        assert_eq!(info.clone(), info);
        // `bit_depth` is optional because the backend estimates it and cannot always derive one.
        let unknown_depth = RawImageInfo {
            bit_depth: None,
            ..info.clone()
        };
        assert_ne!(unknown_depth, info);
    }

    #[test]
    fn the_fixture_develops_to_the_picture_it_encodes() {
        // The whole point of building the fixture rather than committing one: the sensor values are known, so this
        // can assert the picture is *right* rather than merely that decoding returned something.
        let image = decode_raw_bytes(&red_dng()).unwrap();
        assert_eq!((image.width(), image.height()), (64, 64));
        assert!(
            image.as_rgb16().is_some(),
            "a developed raw must be ImageRgb16, got {:?}",
            image.color()
        );

        let [red, green, blue] = channel_means(&image);
        assert!(
            red > green && red > blue,
            "an RGGB sensor with its red photosites bright must develop to a red-dominant picture, \
             got r={red:.0} g={green:.0} b={blue:.0}"
        );
    }

    #[test]
    fn transposing_the_channel_order_moves_the_dominant_channel() {
        // This is what proves the assertion above has teeth. The same builder, the same bright photosites, the only
        // difference being which colour the CFA says they are — so if the decode ignored the CFA, or this module
        // shuffled the channels on the way into `DynamicImage`, both would come out the same and this would fail.
        let red = decode_raw_bytes(&minimal_dng(64, 64, [RED, GREEN, GREEN, BLUE], RED)).unwrap();
        let blue = decode_raw_bytes(&minimal_dng(64, 64, [BLUE, GREEN, GREEN, RED], BLUE)).unwrap();

        let [red_r, _, red_b] = channel_means(&red);
        let [blue_r, _, blue_b] = channel_means(&blue);

        assert!(red_r > red_b, "RGGB with bright red should be red-dominant");
        assert!(blue_b > blue_r, "BGGR with bright blue should be blue-dominant");
        // And stated the other way round: the red fixture's assertion applied to the transposed file fails.
        assert!(blue_r <= blue_b, "the transposed fixture must not also be red-dominant");
    }

    #[test]
    fn decoding_bytes_that_are_not_raw_is_refused_rather_than_attempted() {
        // A PNG is recognisable and is not RAW, so it gets the specific refusal — and no panic from inside a
        // decoder that was handed something it never expected.
        let png = encode(&sample_image(), ImageFormat::Png);
        assert!(matches!(decode_raw_bytes(&png), Err(ImageError::NotRaw)));
        assert!(matches!(decode_raw_bytes(&[]), Err(ImageError::NotRaw)));
        assert!(matches!(
            decode_raw_bytes(b"not an image at all"),
            Err(ImageError::NotRaw)
        ));
    }

    #[test]
    fn decode_raw_file_round_trips_through_disk() {
        let path = std::env::temp_dir().join(format!("rust_sak_raw_{}.dng", std::process::id()));
        std::fs::write(&path, red_dng()).unwrap();
        let decoded = decode_raw_file(&path).unwrap();
        std::fs::remove_file(&path).unwrap();

        assert_eq!((decoded.width(), decoded.height()), (64, 64));
        let [red, green, blue] = channel_means(&decoded);
        assert!(red > green && red > blue, "r={red:.0} g={green:.0} b={blue:.0}");
    }

    #[test]
    fn decode_raw_file_refuses_a_path_that_does_not_name_a_raw_format() {
        assert!(matches!(
            decode_raw_file("/tmp/photo.png"),
            Err(ImageError::UnknownExtension)
        ));
        assert!(matches!(
            decode_raw_file("/tmp/photo.tiff"),
            Err(ImageError::UnknownExtension)
        ));
        assert!(matches!(
            decode_raw_file("/tmp/noext"),
            Err(ImageError::UnknownExtension)
        ));
    }

    #[test]
    fn probing_reports_what_the_file_says() {
        let info = probe_raw_bytes(&red_dng()).unwrap();
        assert_eq!((info.width, info.height), (64, 64));
        assert!(info.is_dng, "the fixture carries a DNGVersion tag");
        // A DNG identifies itself in its header, so this is one of the formats `from_magic` can pin down.
        assert_eq!(info.format, Some(RawFormat::Dng));
        assert_eq!(info.make, "rust-sak");
        assert_eq!(info.model, "Synthetic DNG");
    }

    #[test]
    fn the_probed_dimensions_are_the_decoded_ones() {
        // The check that matters for a caller listing a directory: what `probe` promises is what `decode` delivers,
        // even though the two take completely different paths through the backend.
        let dng = red_dng();
        let info = probe_raw_bytes(&dng).unwrap();
        let decoded = decode_raw_bytes(&dng).unwrap();
        assert_eq!((info.width, info.height), (decoded.width(), decoded.height()));
    }

    #[test]
    fn probe_raw_file_names_the_format_from_the_extension() {
        let path = std::env::temp_dir().join(format!("rust_sak_probe_raw_{}.dng", std::process::id()));
        std::fs::write(&path, red_dng()).unwrap();
        let from_file = probe_raw_file(&path).unwrap();
        let from_bytes = probe_raw_bytes(&std::fs::read(&path).unwrap()).unwrap();
        std::fs::remove_file(&path).unwrap();

        assert_eq!(from_file, from_bytes, "the file and byte probes disagree");
        assert_eq!(from_file.format, Some(RawFormat::Dng));
    }

    #[test]
    fn probe_raw_refuses_what_is_not_raw() {
        let png = encode(&sample_image(), ImageFormat::Png);
        assert!(matches!(probe_raw_bytes(&png), Err(ImageError::NotRaw)));
        assert!(matches!(
            probe_raw_file("/tmp/photo.png"),
            Err(ImageError::UnknownExtension)
        ));
    }

    /// Decodes and probes every file in the directory named by `ZENRAW_SAMPLES_DIR`, and does nothing at all when
    /// that variable is unset.
    ///
    /// The synthesised fixture proves the seam, the stride handling and the error mapping; what it cannot prove is
    /// that a real Nikon file off a real card decodes, because a hand-built DNG only exercises the path a
    /// hand-built DNG takes. This is where that gets checked — by a developer with a folder of camera files, on
    /// demand. **Never in CI**: the files are tens of megabytes each and belong to whoever shot them.
    ///
    /// ```text
    /// ZENRAW_SAMPLES_DIR=~/Pictures/raw-samples cargo test --features image-raw -- --nocapture
    /// ```
    #[test]
    fn every_file_in_the_samples_directory_decodes_and_probes() {
        let Ok(dir) = std::env::var("ZENRAW_SAMPLES_DIR") else {
            // Unset is the normal case, and it is not a failure — this test simply has nothing to look at.
            return;
        };

        let entries = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("ZENRAW_SAMPLES_DIR is set to {dir}, which cannot be read: {e}"));

        let mut checked = 0_usize;
        for entry in entries {
            let path = entry.expect("directory entry").path();
            // Skip whatever else lives in the folder — sidecars, JPEGs, subdirectories — rather than failing on it.
            if RawFormat::from_path(&path).is_none() {
                continue;
            }

            let info = probe_raw_file(&path).unwrap_or_else(|e| panic!("probing {} failed: {e}", path.display()));
            let image = decode_raw_file(&path).unwrap_or_else(|e| panic!("decoding {} failed: {e}", path.display()));

            assert_eq!(
                (info.width, info.height),
                (image.width(), image.height()),
                "{}: probe and decode disagree on the dimensions",
                path.display()
            );
            assert!(
                image.as_rgb16().is_some(),
                "{}: expected ImageRgb16, got {:?}",
                path.display(),
                image.color()
            );
            assert_eq!(info.format, RawFormat::from_path(&path), "{}", path.display());

            eprintln!(
                "{}: {}x{} {} {} ({}-bit sensor)",
                path.display(),
                info.width,
                info.height,
                info.make,
                info.model,
                info.bit_depth.map_or_else(|| "?".to_string(), |d| d.to_string()),
            );
            checked += 1;
        }

        // A directory with no RAW files in it is a mistake worth reporting, since the run looks green otherwise.
        assert!(
            checked > 0,
            "ZENRAW_SAMPLES_DIR is set to {dir}, which holds no recognised RAW files"
        );
    }

    #[test]
    fn every_supported_extension_maps_to_its_format() {
        for (extension, expected) in EXTENSIONS {
            assert_eq!(
                RawFormat::from_extension(extension),
                Some(*expected),
                "extension {extension}"
            );
        }
    }

    #[test]
    fn every_variant_round_trips_through_its_canonical_extension() {
        for format in EVERY_VARIANT {
            let extension = format.extension();
            assert_eq!(
                RawFormat::from_extension(extension),
                Some(*format),
                "{format:?} canonical extension {extension} does not map back"
            );
        }
    }

    #[test]
    fn extension_lookup_ignores_case() {
        // Cameras write `.NEF` as readily as `.nef`, and on a case-sensitive filesystem those are different strings
        // for one format.
        for spelling in ["NEF", "Nef", "nEf", "nef"] {
            assert_eq!(RawFormat::from_extension(spelling), Some(RawFormat::Nef), "{spelling}");
        }
        assert_eq!(RawFormat::from_path("/pictures/DSC_0001.CR3"), Some(RawFormat::Cr3));
        assert_eq!(RawFormat::from_path("/pictures/DSC_0001.3FR"), Some(RawFormat::ThreeFr));
    }

    #[test]
    fn tiff_is_not_claimed_for_raw() {
        // Nearly every RAW format is a TIFF container, which is exactly why the extension has to be the arbiter: a
        // file called `.tiff` is an ordinary TIFF and must stay one, or the eight formats that already work break.
        for extension in ["tif", "tiff", "TIFF"] {
            assert_eq!(
                RawFormat::from_extension(extension),
                None,
                "{extension} was claimed for RAW"
            );
        }
        assert_eq!(RawFormat::from_path("/pictures/scan.tiff"), None);
        // And the non-RAW formats stay out too.
        for extension in ["png", "jpg", "webp", "heic", "avif", "xyz", ""] {
            assert_eq!(
                RawFormat::from_extension(extension),
                None,
                "{extension} was claimed for RAW"
            );
        }
    }

    #[test]
    fn path_without_a_recognized_extension_is_none() {
        assert_eq!(RawFormat::from_path("/pictures/noext"), None);
        assert_eq!(RawFormat::from_path("/pictures/archive.tar.gz"), None);
    }

    #[test]
    fn from_magic_resolves_only_the_self_identifying_containers() {
        // A bare TIFF header is shared by NEF, ARW, CR2, PEF and the rest, so it names none of them.
        assert_eq!(
            RawFormat::from_magic(b"II\x2a\x00\x08\x00\x00\x00\x00\x00\x00\x00"),
            None
        );
        assert_eq!(
            RawFormat::from_magic(b"MM\x00\x2a\x00\x00\x00\x08\x00\x00\x00\x00"),
            None
        );

        // Canon CR3 is ISO-BMFF with a `crx ` major brand.
        assert_eq!(
            RawFormat::from_magic(b"\x00\x00\x00\x18ftypcrx \x00\x00\x00\x01"),
            Some(RawFormat::Cr3)
        );
        // Fujifilm RAF leads with the vendor name.
        assert_eq!(
            RawFormat::from_magic(b"FUJIFILMCCD-RAW 0201FF383501"),
            Some(RawFormat::Raf)
        );
        // Panasonic RW2 is a TIFF variant with a 0x55 version marker.
        assert_eq!(
            RawFormat::from_magic(b"II\x55\x00\x18\x00\x00\x00\x00\x00\x00\x00"),
            Some(RawFormat::Rw2)
        );
        // Olympus ORF uses `IIRO`/`IIRS` where a TIFF would carry 42.
        assert_eq!(
            RawFormat::from_magic(b"II\x52\x4f\x08\x00\x00\x00\x00\x00\x00\x00"),
            Some(RawFormat::Orf)
        );

        // Not RAW at all.
        assert_eq!(RawFormat::from_magic(&encode(&sample_image(), ImageFormat::Png)), None);
        assert_eq!(RawFormat::from_magic(&[]), None);
        assert_eq!(RawFormat::from_magic(b"hello"), None);
    }

    #[test]
    fn is_raw_bytes_rejects_the_obviously_not_raw() {
        for format in [ImageFormat::Png, ImageFormat::Gif, ImageFormat::Bmp, ImageFormat::WebP] {
            let bytes = encode(&sample_image(), format);
            assert!(!is_raw_bytes(&bytes), "{format:?} was taken for RAW");
        }
        assert!(!is_raw_bytes(&[]));
        assert!(!is_raw_bytes(b"hello"));
    }

    #[test]
    fn is_raw_bytes_cannot_tell_a_tiff_from_a_raw() {
        // Pinned deliberately rather than worked around: this is a header-magic check and the RAW formats *are*
        // TIFFs, so a TIFF passes it. Callers routing between the RAW and TIFF decoders must use the extension —
        // the function's own docs say so, and this is the test that keeps that statement true.
        let tiff = encode(&sample_image(), ImageFormat::Tiff);
        assert!(is_raw_bytes(&tiff));
    }
}
