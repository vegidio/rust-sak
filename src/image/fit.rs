use std::borrow::Cow;
use std::num::NonZeroU32;

use ::image::DynamicImage;
use ::image::imageops::FilterType;

/// How much larger than the bound the prefilter leaves the picture before the Lanczos pass.
///
/// Three times the bound keeps enough detail for Lanczos3's six-tap window to resolve, and it is where the two-stage
/// result stops being distinguishable from a pure Lanczos3 pass (better than 40 dB PSNR) for about a third of the cost.
const PREFILTER_FACTOR: u32 = 3;

/// Scales `img` down so its longer edge is `bound`, keeping the aspect ratio. **Never enlarges.**
///
/// A picture whose longer edge is already no larger than `bound` comes back [`Cow::Borrowed`] — no copy and no
/// filter pass — so a caller that only needs to read the result pays nothing for a picture that already fits.
///
/// Otherwise the picture is resampled with Lanczos3. When the bound is less than a third of the longer edge, a fast
/// integer-averaging prefilter first brings it down to three times the bound, because a Lanczos pass over a full
/// 24-megapixel photograph to produce a 256-pixel thumbnail spends nearly all of its time on detail it throws away.
///
/// The short edge is rounded to the nearest pixel and is at least 1, so a 1×10000 strip still has a column. The
/// colour type is preserved: an RGBA picture stays RGBA, a 16-bit one stays 16-bit.
///
/// ```
/// use std::num::NonZeroU32;
///
/// use image::{DynamicImage, RgbImage};
/// use rust_sak::image::fit;
///
/// let photo = DynamicImage::ImageRgb8(RgbImage::new(4000, 3000));
/// let thumbnail = fit(&photo, NonZeroU32::new(400).unwrap());
///
/// assert_eq!((thumbnail.width(), thumbnail.height()), (400, 300));
/// ```
#[must_use]
pub fn fit(img: &DynamicImage, bound: NonZeroU32) -> Cow<'_, DynamicImage> {
    let bound = bound.get();
    let (width, height) = (img.width(), img.height());
    let longer = width.max(height);

    if longer <= bound {
        return Cow::Borrowed(img);
    }

    let (target_width, target_height) = fitted_dimensions(width, height, bound);

    // Saturating, so a bound near `u32::MAX` simply skips the prefilter instead of wrapping into a tiny one.
    let prefiltered = bound.saturating_mul(PREFILTER_FACTOR);
    let resized = if prefiltered < longer {
        img.thumbnail(prefiltered, prefiltered)
            .resize_exact(target_width, target_height, FilterType::Lanczos3)
    } else {
        img.resize_exact(target_width, target_height, FilterType::Lanczos3)
    };

    Cow::Owned(resized)
}

/// The size of a `width`×`height` picture scaled so its longer edge is `bound`: the short edge rounded to the
/// nearest pixel and at least 1. Only called when the longer edge exceeds `bound`.
fn fitted_dimensions(width: u32, height: u32, bound: u32) -> (u32, u32) {
    let short_edge = |short: u32, long: u32| {
        // In `u64`, so `short * bound` cannot overflow for any pair of `u32` dimensions.
        let scaled = (u64::from(short) * u64::from(bound) + u64::from(long) / 2) / u64::from(long);

        // At most `bound`, because `short <= long`, so it fits back in a `u32`.
        (scaled as u32).max(1)
    };

    if width >= height {
        (bound, short_edge(height, width))
    } else {
        (short_edge(width, height), bound)
    }
}

#[cfg(test)]
mod tests {
    use ::image::{GrayImage, ImageBuffer, Luma, Rgb, RgbImage, Rgba, RgbaImage};

    use super::*;

    fn bound(value: u32) -> NonZeroU32 {
        NonZeroU32::new(value).unwrap()
    }

    fn dimensions(width: u32, height: u32, bound_px: u32) -> (u32, u32) {
        let source = DynamicImage::ImageRgb8(RgbImage::new(width, height));
        let fitted = fit(&source, bound(bound_px));

        (fitted.width(), fitted.height())
    }

    /// A busy, non-repeating RGB pattern, so a resampler that cuts corners shows up in the PSNR.
    fn detailed(width: u32, height: u32) -> DynamicImage {
        let mut image = RgbImage::new(width, height);
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            let wave = |frequency: f32, phase: f32| {
                let value = (x as f32 * frequency + phase).sin() * (y as f32 * frequency * 0.7).cos();
                ((value + 1.0) * 127.5) as u8
            };
            *pixel = Rgb([wave(0.013, 0.0), wave(0.021, 1.0), wave(0.034, 2.0)]);
        }

        DynamicImage::ImageRgb8(image)
    }

    fn psnr(a: &RgbImage, b: &RgbImage) -> f64 {
        assert_eq!(a.dimensions(), b.dimensions());

        let squared: f64 = a
            .as_raw()
            .iter()
            .zip(b.as_raw())
            .map(|(&x, &y)| (f64::from(x) - f64::from(y)).powi(2))
            .sum();
        let mse = squared / a.as_raw().len() as f64;

        if mse == 0.0 {
            return f64::INFINITY;
        }

        10.0 * (255.0_f64.powi(2) / mse).log10()
    }

    #[test]
    fn a_landscape_picture_fits_by_its_width() {
        assert_eq!(dimensions(4000, 3000, 400), (400, 300));
        assert_eq!(dimensions(1920, 1080, 320), (320, 180));
    }

    #[test]
    fn a_portrait_picture_fits_by_its_height() {
        assert_eq!(dimensions(3000, 4000, 400), (300, 400));
        assert_eq!(dimensions(1080, 1920, 320), (180, 320));
    }

    #[test]
    fn a_square_picture_fits_on_both_edges() {
        assert_eq!(dimensions(1000, 1000, 256), (256, 256));
    }

    #[test]
    fn the_short_edge_is_rounded_to_the_nearest_pixel() {
        // 333 * 100 / 1000 = 33.3 and 335 * 100 / 1000 = 33.5, which round to 33 and 34.
        assert_eq!(dimensions(1000, 333, 100), (100, 33));
        assert_eq!(dimensions(1000, 335, 100), (100, 34));
    }

    #[test]
    fn a_picture_within_the_bound_comes_back_borrowed() {
        let source = DynamicImage::ImageRgb8(RgbImage::new(200, 100));

        for bound_px in [200, 400, u32::MAX] {
            let fitted = fit(&source, bound(bound_px));

            assert!(
                matches!(fitted, Cow::Borrowed(_)),
                "bound {bound_px} copied the picture"
            );
            assert!(std::ptr::eq(fitted.as_ref(), &source));
        }
    }

    #[test]
    fn a_picture_larger_than_the_bound_is_never_enlarged_past_it() {
        let source = DynamicImage::ImageRgb8(RgbImage::new(201, 100));
        let fitted = fit(&source, bound(200));

        assert!(matches!(fitted, Cow::Owned(_)));
        assert_eq!((fitted.width(), fitted.height()), (200, 100));
    }

    #[test]
    fn the_colour_type_survives_both_paths() {
        let sources = [
            DynamicImage::ImageRgb8(RgbImage::new(900, 600)),
            DynamicImage::ImageRgba8(RgbaImage::new(900, 600)),
            DynamicImage::ImageLuma8(GrayImage::new(900, 600)),
            DynamicImage::ImageRgba16(ImageBuffer::new(900, 600)),
            DynamicImage::ImageRgb32F(ImageBuffer::new(900, 600)),
        ];

        // 450 takes the single-stage path, 100 the prefiltered one.
        for source in &sources {
            for bound_px in [450, 100] {
                let fitted = fit(source, bound(bound_px));

                assert_eq!(fitted.color(), source.color(), "bound {bound_px}");
            }
        }
    }

    #[test]
    fn transparency_is_kept() {
        let mut source = RgbaImage::new(800, 800);
        for (x, _, pixel) in source.enumerate_pixels_mut() {
            *pixel = if x < 400 {
                Rgba([255, 0, 0, 255])
            } else {
                Rgba([0, 0, 0, 0])
            };
        }

        let source = DynamicImage::ImageRgba8(source);
        let fitted = fit(&source, bound(100));
        let fitted = fitted.as_rgba8().expect("an RGBA source fits to an RGBA result");

        assert_eq!(fitted.get_pixel(10, 50).0[3], 255);
        assert_eq!(fitted.get_pixel(90, 50).0[3], 0);
    }

    #[test]
    fn a_one_pixel_strip_keeps_its_one_pixel() {
        assert_eq!(dimensions(1, 10_000, 100), (1, 100));
        assert_eq!(dimensions(10_000, 1, 100), (100, 1));
        assert_eq!(dimensions(3, 10_000, 16), (1, 16));
    }

    #[test]
    fn the_pixels_are_resampled_not_dropped() {
        let source = DynamicImage::ImageLuma8(GrayImage::from_pixel(1200, 900, Luma([180])));
        let fitted = fit(&source, bound(120));

        assert!(fitted.to_luma8().pixels().all(|pixel| pixel.0[0].abs_diff(180) <= 1));
    }

    #[test]
    fn the_prefiltered_path_matches_a_pure_lanczos_pass() {
        let source = detailed(3000, 2000);
        let target = bound(240);

        let two_stage = fit(&source, target).to_rgb8();
        let (width, height) = (two_stage.width(), two_stage.height());
        let single_stage = source.resize_exact(width, height, FilterType::Lanczos3).to_rgb8();

        let quality = psnr(&two_stage, &single_stage);
        assert!(
            quality > 40.0,
            "two-stage PSNR against single-stage was {quality:.1} dB"
        );
    }
}
