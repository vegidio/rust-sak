use ::image::{DynamicImage, ImageBuffer, Pixel, Primitive, imageops};

/// Rotates an image by an arbitrary angle onto a canvas expanded to contain it.
///
/// **A positive angle turns the picture clockwise**, matching CSS `transform: rotate()` and the direction every
/// on-screen rotation control reports. A rotation whose sign is ambiguous is a rotation applied backwards
/// somewhere, so the convention is stated here rather than left to the caller to discover.
///
/// The destination is the source's bounding box after the turn, rounded outward — for a source of `w` by `h`
/// turned by `t`:
///
/// ```text
/// width  = ceil(w * |cos t| + h * |sin t|)
/// height = ceil(w * |sin t| + h * |cos t|)
/// ```
///
/// with the source's center mapped to the destination's center. Each destination pixel is mapped back through the
/// inverse rotation and sampled bilinearly from the four surrounding source pixels, weighted by their alpha so
/// that a transparent neighbor cannot bleed its color into the edge. The area no source pixel covers is left
/// **fully transparent**; there is no fill color, because the crop that normally follows a rotation excludes the
/// corners anyway, and transparent is the honest answer where it does not.
///
/// Because rotation necessarily produces pixels no source pixel covers, the result always carries alpha: `Rgb8`
/// rotates to `Rgba8`, `Rgb16` to `Rgba16`, `Luma8` to `LumaA8`, `Rgb32F` to `Rgba32F`, and an input that already
/// has an alpha channel keeps its own type. **Bit depth is preserved** — a developed 16-bit RAW is not flattened
/// to eight bits by being turned.
///
/// Angles are taken modulo 360, and the exact quarter turns (0, 90, 180, 270 and their equivalents) are handled by
/// the [`image`](::image) crate's own exact rotations, so they neither interpolate nor gain a row.
///
/// ```
/// use image::{DynamicImage, RgbImage};
/// use rust_sak::image::rotate;
///
/// let source = DynamicImage::ImageRgb8(RgbImage::new(240, 120));
/// let rotated = rotate(&source, 30.0);
///
/// // 240 * cos 30 + 120 * sin 30 = 267.85, and 240 * sin 30 + 120 * cos 30 = 223.92.
/// assert_eq!((rotated.width(), rotated.height()), (268, 224));
/// ```
#[must_use]
pub fn rotate(image: &DynamicImage, degrees: f64) -> DynamicImage {
    match image {
        DynamicImage::ImageLuma8(_) | DynamicImage::ImageLumaA8(_) => {
            DynamicImage::ImageLumaA8(rotate_buffer(&image.to_luma_alpha8(), degrees))
        }
        DynamicImage::ImageLuma16(_) | DynamicImage::ImageLumaA16(_) => {
            DynamicImage::ImageLumaA16(rotate_buffer(&image.to_luma_alpha16(), degrees))
        }
        DynamicImage::ImageRgb16(_) | DynamicImage::ImageRgba16(_) => {
            DynamicImage::ImageRgba16(rotate_buffer(&image.to_rgba16(), degrees))
        }
        DynamicImage::ImageRgb32F(_) | DynamicImage::ImageRgba32F(_) => {
            DynamicImage::ImageRgba32F(rotate_buffer(&image.to_rgba32f(), degrees))
        }
        // `Rgb8`, `Rgba8`, and whatever `DynamicImage` gains next: eight-bit RGBA loses nothing that the
        // enumerated arms above would have kept, so it is the right landing place for an unknown variant.
        _ => DynamicImage::ImageRgba8(rotate_buffer(&image.to_rgba8(), degrees)),
    }
}

/// The subpixel types a [`DynamicImage`] is made of, and how each one crosses the `f32` accumulator the sampler
/// works in.
///
/// It exists because the round trip is not uniform: an integral subpixel has to round to nearest and saturate,
/// while an `f32` one must pass through untouched — rounding it would quantize every value to 0 or 1.
trait Subpixel: Primitive + 'static {
    fn to_accumulator(self) -> f32;
    fn from_accumulator(value: f32) -> Self;
}

impl Subpixel for u8 {
    fn to_accumulator(self) -> f32 {
        f32::from(self)
    }

    fn from_accumulator(value: f32) -> Self {
        value.round().clamp(0.0, f32::from(Self::MAX)) as Self
    }
}

impl Subpixel for u16 {
    fn to_accumulator(self) -> f32 {
        f32::from(self)
    }

    fn from_accumulator(value: f32) -> Self {
        value.round().clamp(0.0, f32::from(Self::MAX)) as Self
    }
}

impl Subpixel for f32 {
    fn to_accumulator(self) -> f32 {
        self
    }

    fn from_accumulator(value: f32) -> Self {
        value
    }
}

/// The largest number of channels any [`DynamicImage`] pixel has, and so the width of the accumulator.
const MAX_CHANNELS: usize = 4;

/// Rotates one alpha-bearing buffer. Every caller has already promoted its image, so the last channel is alpha.
fn rotate_buffer<P>(source: &ImageBuffer<P, Vec<P::Subpixel>>, degrees: f64) -> ImageBuffer<P, Vec<P::Subpixel>>
where
    P: Pixel + 'static,
    P::Subpixel: Subpixel,
{
    // Taken modulo 360 first, so 450 degrees is the same quarter turn as 90 and takes the exact path with it.
    let angle = degrees.rem_euclid(360.0);

    // The exact quarter turns are the `image` crate's own, rather than this sampler's, because at those angles no
    // interpolation should occur at all. The sampler very nearly agrees - but `cos 90` is 6.1e-17 rather than
    // zero in binary floating point, so it blends a sliver of the neighbouring pixel about 1e-14 wide. Rounding
    // to an integer subpixel hides that; an `f32` subpixel keeps it, and so would every pixel of a rotated
    // 32-float picture. Taking the exact path makes a quarter turn exact by construction rather than by luck,
    // and it is a memcpy-shaped operation rather than a resample.
    match angle {
        0.0 => return source.clone(),
        90.0 => return imageops::rotate90(source),
        180.0 => return imageops::rotate180(source),
        270.0 => return imageops::rotate270(source),
        _ => {}
    }

    let (source_width, source_height) = source.dimensions();
    let (sin, cos) = angle.to_radians().sin_cos();

    let width = round_outward(f64::from(source_width) * cos.abs() + f64::from(source_height) * sin.abs());
    let height = round_outward(f64::from(source_width) * sin.abs() + f64::from(source_height) * cos.abs());

    let mut destination = ImageBuffer::from_pixel(width, height, zeroed::<P>());
    if source_width == 0 || source_height == 0 {
        return destination;
    }

    // Pixel centers sit at integer indices, so the center of a `w`-wide image is at `w / 2 - 0.5`.
    let source_center = (
        f64::from(source_width) / 2.0 - 0.5,
        f64::from(source_height) / 2.0 - 0.5,
    );
    let destination_center = (f64::from(width) / 2.0 - 0.5, f64::from(height) / 2.0 - 0.5);

    for y in 0..height {
        let offset_y = f64::from(y) - destination_center.1;

        for x in 0..width {
            let offset_x = f64::from(x) - destination_center.0;

            // The inverse of the clockwise rotation `(x cos - y sin, x sin + y cos)`, which is what takes a
            // destination pixel back to the source coordinate it was drawn from.
            let sample_x = offset_x.mul_add(cos, offset_y * sin) + source_center.0;
            let sample_y = offset_y.mul_add(cos, -offset_x * sin) + source_center.1;

            let Some(pixel) = sample(source, sample_x, sample_y) else {
                continue;
            };

            destination.put_pixel(x, y, pixel);
        }
    }

    destination
}

/// Bilinearly samples `source` at a real-valued coordinate, returning `None` where no source pixel covers it.
///
/// Colors are accumulated weighted by their own alpha and divided back out at the end, so a neighbor that is
/// transparent contributes its coverage but not its color. Blending a transparent black neighbor in directly is
/// what gives a naively rotated photograph a dark fringe along its new edges.
fn sample<P>(source: &ImageBuffer<P, Vec<P::Subpixel>>, x: f64, y: f64) -> Option<P>
where
    P: Pixel + 'static,
    P::Subpixel: Subpixel,
{
    let (width, height) = source.dimensions();

    // Every caller promoted its buffer, so the last channel is the alpha the sampler weights by.
    let alpha = P::CHANNEL_COUNT as usize - 1;

    let left = x.floor();
    let top = y.floor();

    // A coordinate is covered while its floor is within one pixel of the image, which is what lets the outermost
    // half-pixel of the source blend out to transparent instead of ending on a hard, aliased edge.
    if left < -1.0 || top < -1.0 || left >= f64::from(width) || top >= f64::from(height) {
        return None;
    }

    // Both are within one pixel of the image, so an `i64` holds them whatever the dimensions.
    let (left, top) = (left as i64, top as i64);
    let (fraction_x, fraction_y) = ((x - left as f64) as f32, (y - top as f64) as f32);

    let weights = [
        (left, top, (1.0 - fraction_x) * (1.0 - fraction_y)),
        (left + 1, top, fraction_x * (1.0 - fraction_y)),
        (left, top + 1, (1.0 - fraction_x) * fraction_y),
        (left + 1, top + 1, fraction_x * fraction_y),
    ];

    let mut accumulator = [0.0_f32; MAX_CHANNELS];
    let mut coverage = 0.0_f32;

    for (sample_x, sample_y, weight) in weights {
        if sample_x < 0 || sample_y < 0 || sample_x >= i64::from(width) || sample_y >= i64::from(height) {
            continue;
        }

        let pixel = source.get_pixel(sample_x as u32, sample_y as u32);
        let values = pixel.channels();

        let weighted_alpha = values[alpha].to_accumulator() * weight;
        for channel in 0..alpha {
            accumulator[channel] += values[channel].to_accumulator() * weighted_alpha;
        }
        coverage += weighted_alpha;
    }

    // Every covering neighbor was fully transparent, so the destination keeps the transparent pixel it started as.
    if coverage <= 0.0 {
        return None;
    }

    let mut pixel = zeroed::<P>();
    let values = pixel.channels_mut();
    for channel in 0..alpha {
        values[channel] = P::Subpixel::from_accumulator(accumulator[channel] / coverage);
    }
    values[alpha] = P::Subpixel::from_accumulator(coverage);

    Some(pixel)
}

/// A fully transparent pixel of `P`: all channels at zero, alpha included.
fn zeroed<P: Pixel>() -> P
where
    P::Subpixel: Subpixel,
{
    *P::from_slice(&[P::Subpixel::from_accumulator(0.0); MAX_CHANNELS][..P::CHANNEL_COUNT as usize])
}

/// Rounds a bounding-box dimension outward, tolerating the last few bits of floating-point noise.
///
/// A bare `ceil` would answer 268 for a box that is 267.0000000001 wide only because `cos` and `sin` are not
/// exact, so the value is first settled to six decimal places — far finer than any real dimension, far coarser
/// than the error.
fn round_outward(value: f64) -> u32 {
    let settled = (value * 1e6).round() / 1e6;

    settled.ceil().max(0.0) as u32
}

#[cfg(test)]
mod tests {
    use ::image::{DynamicImage, GrayAlphaImage, GrayImage, Luma, LumaA, Rgb, RgbImage, Rgba, RgbaImage, imageops};

    use super::*;

    /// A deliberately asymmetric source: wide, with a distinctly colored marker in one corner, so the rotated
    /// result says unambiguously which way the picture turned.
    fn marked() -> DynamicImage {
        let mut image = RgbImage::from_pixel(240, 120, Rgb([128, 128, 128]));
        for y in 0..24 {
            for x in 0..24 {
                image.put_pixel(x, y, Rgb([255, 0, 0]));
            }
        }

        DynamicImage::ImageRgb8(image)
    }

    /// The center of mass of the pixels near `colour`, in destination pixels.
    fn marker_center(image: &RgbaImage, colour: [u8; 3]) -> (f64, f64) {
        let (mut sum_x, mut sum_y, mut count) = (0.0, 0.0, 0.0);

        for (x, y, pixel) in image.enumerate_pixels() {
            let [red, green, blue, alpha] = pixel.0;
            if alpha < 200 {
                continue;
            }

            let near = |value: u8, target: u8| value.abs_diff(target) < 60;
            if near(red, colour[0]) && near(green, colour[1]) && near(blue, colour[2]) {
                sum_x += f64::from(x);
                sum_y += f64::from(y);
                count += 1.0;
            }
        }

        assert!(count > 0.0, "no pixel of {colour:?} survived the rotation");

        (sum_x / count, sum_y / count)
    }

    fn expanded_bounds(width: f64, height: f64, degrees: f64) -> (u32, u32) {
        let (sin, cos) = degrees.to_radians().sin_cos();

        // Settled before rounding outward, exactly as `round_outward` does and for the same reason: `cos 90` is
        // 6.1e-17 rather than zero, so a bare `ceil` here would demand a 121-pixel-wide quarter turn of a
        // 120-pixel-tall image. Stated as the real-number formula it is, not by calling the code under test.
        let outward = |value: f64| ((value * 1e6).round() / 1e6).ceil() as u32;

        (
            outward(width * cos.abs() + height * sin.abs()),
            outward(width * sin.abs() + height * cos.abs()),
        )
    }

    #[test]
    fn positive_angle_turns_clockwise() {
        let rotated = rotate(&marked(), 30.0).to_rgba8();
        let (x, y) = marker_center(&rotated, [255, 0, 0]);

        // The marker's center starts at (11.5, 11.5), which is (-108, -48) from the source's center. Turned
        // clockwise it lands at (-69.5, -95.6) from the destination's center, and counter-clockwise at
        // (-117.5, 12.4) - the two are 100 pixels apart, so this assertion fails loudly if the sign is flipped.
        let center = (
            f64::from(rotated.width()) / 2.0 - 0.5,
            f64::from(rotated.height()) / 2.0 - 0.5,
        );
        let clockwise = (center.0 - 69.5, center.1 - 95.6);
        let counter_clockwise = (center.0 - 117.5, center.1 + 12.4);

        assert!(
            (x - clockwise.0).abs() < 1.0 && (y - clockwise.1).abs() < 1.0,
            "marker at ({x:.1}, {y:.1}), clockwise predicts ({:.1}, {:.1})",
            clockwise.0,
            clockwise.1,
        );
        assert!(
            (x - counter_clockwise.0).abs() > 10.0 || (y - counter_clockwise.1).abs() > 10.0,
            "marker at ({x:.1}, {y:.1}) is also where counter-clockwise would put it, so this proves nothing",
        );
    }

    #[test]
    fn negative_angle_turns_counter_clockwise() {
        let rotated = rotate(&marked(), -30.0).to_rgba8();
        let (x, y) = marker_center(&rotated, [255, 0, 0]);

        let center = (
            f64::from(rotated.width()) / 2.0 - 0.5,
            f64::from(rotated.height()) / 2.0 - 0.5,
        );

        assert!(
            (x - (center.0 - 117.5)).abs() < 1.0 && (y - (center.1 + 12.4)).abs() < 1.0,
            "marker at ({x:.1}, {y:.1})",
        );
    }

    #[test]
    fn expands_the_canvas_to_the_rotated_bounding_box() {
        let source = DynamicImage::ImageRgb8(RgbImage::new(240, 120));

        for degrees in [
            0.0, 12.0, 30.0, 45.0, 90.0, 135.0, 180.0, 270.0, -12.0, -30.0, -45.0, -90.0,
        ] {
            let rotated = rotate(&source, degrees);
            let (width, height) = expanded_bounds(240.0, 120.0, degrees);

            assert_eq!(
                (rotated.width(), rotated.height()),
                (width, height),
                "rotating 240x120 by {degrees} degrees",
            );
        }
    }

    #[test]
    fn the_bounding_box_is_the_documented_formula() {
        // The numbers spelled out, so a change to `round_outward` cannot quietly move them.
        assert_eq!(dimensions(240, 120, 30.0), (268, 224));
        assert_eq!(dimensions(240, 120, 45.0), (255, 255));
        assert_eq!(dimensions(100, 100, 45.0), (142, 142));
        assert_eq!(dimensions(240, 120, 0.0), (240, 120));
        assert_eq!(dimensions(240, 120, 90.0), (120, 240));
        assert_eq!(dimensions(240, 120, 360.0), (240, 120));
        assert_eq!(dimensions(240, 120, 450.0), (120, 240));
    }

    fn dimensions(width: u32, height: u32, degrees: f64) -> (u32, u32) {
        let rotated = rotate(&DynamicImage::ImageRgb8(RgbImage::new(width, height)), degrees);

        (rotated.width(), rotated.height())
    }

    #[test]
    fn quarter_turns_match_the_image_crate_exactly() {
        let source = marked();
        let promoted = source.to_rgba8();

        // Exact, not within a tolerance: at a quarter turn every destination pixel sits on a source pixel, so
        // nothing should be interpolated and no value should move by one.
        assert_eq!(rotate(&source, 90.0).to_rgba8(), imageops::rotate90(&promoted));
        assert_eq!(rotate(&source, 180.0).to_rgba8(), imageops::rotate180(&promoted));
        assert_eq!(rotate(&source, 270.0).to_rgba8(), imageops::rotate270(&promoted));
        assert_eq!(rotate(&source, 0.0).to_rgba8(), promoted);

        // And the equivalent angles reach the same exact path rather than the sampler.
        assert_eq!(rotate(&source, 450.0).to_rgba8(), imageops::rotate90(&promoted));
        assert_eq!(rotate(&source, -90.0).to_rgba8(), imageops::rotate270(&promoted));
        assert_eq!(rotate(&source, -270.0).to_rgba8(), imageops::rotate90(&promoted));

        // A 32-float picture is where taking the exact path stops being an optimisation and starts being the
        // only way to be exact: it has no rounding to absorb the 1e-14 sliver of a neighbour that sampling at
        // `cos 90 = 6.1e-17` blends in.
        let mut floats = ::image::ImageBuffer::<Rgba<f32>, Vec<f32>>::new(24, 10);
        for (x, y, pixel) in floats.enumerate_pixels_mut() {
            *pixel = Rgba([x as f32 / 24.0, y as f32 / 10.0, 0.25, 1.0]);
        }
        let floats = DynamicImage::ImageRgba32F(floats);

        let DynamicImage::ImageRgba32F(turned) = rotate(&floats, 90.0) else {
            panic!("a 32-float source must rotate to a 32-float result");
        };
        assert_eq!(turned, imageops::rotate90(floats.as_rgba32f().unwrap()));
    }

    #[test]
    fn rotating_back_returns_the_original_interior() {
        let mut source = RgbImage::new(120, 80);
        for (x, y, pixel) in source.enumerate_pixels_mut() {
            *pixel = Rgb([(x * 2) as u8, (y * 3) as u8, 96]);
        }
        let source = DynamicImage::ImageRgb8(source);

        let there = rotate(&source, 17.0);
        let back = rotate(&there, -17.0).to_rgba8();

        // The round trip lands on a larger canvas than it started on, so the original sits in the middle of it.
        let offset_x = (back.width() - 120) / 2;
        let offset_y = (back.height() - 80) / 2;

        let original = source.to_rgba8();
        let mut worst = 0_u8;
        // The interior only: two bilinear passes soften the outermost pixels, which is what a resample does.
        for y in 4..76 {
            for x in 4..116 {
                let expected = original.get_pixel(x, y).0;
                let actual = back.get_pixel(x + offset_x, y + offset_y).0;

                for channel in 0..3 {
                    worst = worst.max(expected[channel].abs_diff(actual[channel]));
                }
                assert_eq!(actual[3], 255, "the interior of a round trip is opaque at ({x}, {y})");
            }
        }

        assert!(worst <= 12, "worst channel drift across the round trip was {worst}");
    }

    #[test]
    fn promotes_to_the_alpha_bearing_sibling() {
        let rgb8 = DynamicImage::ImageRgb8(RgbImage::new(8, 8));
        let rgb16 = DynamicImage::ImageRgb16(::image::ImageBuffer::new(8, 8));
        let luma8 = DynamicImage::ImageLuma8(GrayImage::new(8, 8));
        let luma16 = DynamicImage::ImageLuma16(::image::ImageBuffer::new(8, 8));
        let rgb32f = DynamicImage::ImageRgb32F(::image::ImageBuffer::new(8, 8));

        assert!(matches!(rotate(&rgb8, 30.0), DynamicImage::ImageRgba8(_)));
        assert!(matches!(rotate(&rgb16, 30.0), DynamicImage::ImageRgba16(_)));
        assert!(matches!(rotate(&luma8, 30.0), DynamicImage::ImageLumaA8(_)));
        assert!(matches!(rotate(&luma16, 30.0), DynamicImage::ImageLumaA16(_)));
        assert!(matches!(rotate(&rgb32f, 30.0), DynamicImage::ImageRgba32F(_)));

        // An input that already carries alpha keeps its own type, at a quarter turn as well as at any other angle.
        let rgba8 = DynamicImage::ImageRgba8(RgbaImage::new(8, 8));
        let rgba16 = DynamicImage::ImageRgba16(::image::ImageBuffer::new(8, 8));
        let luma_alpha8 = DynamicImage::ImageLumaA8(GrayAlphaImage::new(8, 8));

        assert!(matches!(rotate(&rgba8, 30.0), DynamicImage::ImageRgba8(_)));
        assert!(matches!(rotate(&rgba16, 30.0), DynamicImage::ImageRgba16(_)));
        assert!(matches!(rotate(&luma_alpha8, 30.0), DynamicImage::ImageLumaA8(_)));
        assert!(matches!(rotate(&rgba16, 90.0), DynamicImage::ImageRgba16(_)));
    }

    #[test]
    fn sixteen_bits_survive_the_rotation() {
        // A gradient of 1024 distinct levels - four times as many as eight bits can hold, so an 8-bit
        // intermediate anywhere in the rotation would flatten it to 256 and land every one on the 257 grid.
        let mut source = ::image::ImageBuffer::<Rgb<u16>, Vec<u16>>::new(1024, 4);
        for (x, _, pixel) in source.enumerate_pixels_mut() {
            let value = (x * 64) as u16;
            *pixel = Rgb([value, u16::MAX - value, value / 2]);
        }
        let source = DynamicImage::ImageRgb16(source);

        let DynamicImage::ImageRgba16(rotated) = rotate(&source, 10.0) else {
            panic!("a 16-bit source must rotate to a 16-bit result");
        };

        let mut values = rotated
            .pixels()
            .filter(|pixel| pixel.0[3] == u16::MAX)
            .map(|pixel| pixel.0[0])
            .collect::<Vec<_>>();
        values.sort_unstable();
        values.dedup();

        // More levels than eight bits can represent at all, and off the 257 grid an 8-bit value expanded back
        // to sixteen would sit on.
        assert!(values.len() > 256, "only {} distinct levels survived", values.len());
        assert!(
            values.iter().any(|value| value % 257 != 0),
            "every level landed on the 8-bit grid"
        );
    }

    #[test]
    fn uncovered_corners_are_transparent_and_the_interior_is_opaque() {
        let source = DynamicImage::ImageRgb8(RgbImage::from_pixel(120, 80, Rgb([200, 40, 40])));
        let rotated = rotate(&source, 30.0).to_rgba8();

        for (x, y) in [
            (0, 0),
            (rotated.width() - 1, 0),
            (0, rotated.height() - 1),
            (rotated.width() - 1, rotated.height() - 1),
        ] {
            assert_eq!(
                rotated.get_pixel(x, y).0,
                [0, 0, 0, 0],
                "corner ({x}, {y}) is not fully transparent"
            );
        }

        let center = rotated.get_pixel(rotated.width() / 2, rotated.height() / 2);
        assert_eq!(
            center.0,
            [200, 40, 40, 255],
            "the center of a rotated opaque image must stay opaque"
        );
    }

    #[test]
    fn a_transparent_neighbour_does_not_bleed_its_colour() {
        // Half opaque white, half fully transparent black. Sampling across the seam without weighting by alpha
        // would drag the white towards grey; weighting by it leaves the color alone and only the alpha falls.
        let mut source = RgbaImage::new(40, 40);
        for (x, _, pixel) in source.enumerate_pixels_mut() {
            *pixel = if x < 20 {
                Rgba([255, 255, 255, 255])
            } else {
                Rgba([0, 0, 0, 0])
            };
        }

        let rotated = rotate(&DynamicImage::ImageRgba8(source), 20.0).to_rgba8();

        for pixel in rotated.pixels() {
            let [red, green, blue, alpha] = pixel.0;
            if alpha == 0 {
                continue;
            }

            assert_eq!(
                [red, green, blue],
                [255, 255, 255],
                "a transparent neighbour darkened an edge pixel"
            );
        }
    }

    #[test]
    fn a_grey_image_keeps_its_single_channel() {
        let source = DynamicImage::ImageLuma8(GrayImage::from_pixel(60, 40, Luma([180])));

        let DynamicImage::ImageLumaA8(rotated) = rotate(&source, 25.0) else {
            panic!("a grey source must rotate to a grey result");
        };

        assert_eq!(
            rotated.get_pixel(rotated.width() / 2, rotated.height() / 2),
            &LumaA([180, 255])
        );
        assert_eq!(rotated.get_pixel(0, 0), &LumaA([0, 0]));
    }

    #[test]
    fn an_empty_image_rotates_to_an_empty_image() {
        let source = DynamicImage::ImageRgb8(RgbImage::new(0, 0));
        let rotated = rotate(&source, 30.0);

        assert_eq!((rotated.width(), rotated.height()), (0, 0));
    }
}
