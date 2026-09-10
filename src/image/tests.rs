use ::image::{DynamicImage, Rgb, RgbImage};

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
