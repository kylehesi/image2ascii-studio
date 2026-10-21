use std::io::Cursor;

use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageReader, Limits};

use crate::color::{luminance, srgb_to_linear, srgb8_to_linear_lut, unit};
use crate::error::{Error, Result};
use crate::par;

/// Longest side kept for processing. Larger inputs are box-filtered down in
/// linear light first: past this size extra pixels cannot change a character
/// grid, they only cost memory.
pub const MAX_WORKING_SIDE: usize = 4096;

/// Largest image dimension the decoder will accept at all.
const MAX_DECODE_SIDE: u32 = 32_768;
/// Decoder allocation ceiling, so a hostile header can't request gigabytes.
const MAX_DECODE_ALLOC: u64 = 1 << 30;

/// A decoded image reduced to what the converter needs: linear-light
/// luminance and coverage (alpha), both in [0, 1], row-major.
#[derive(Debug, Clone)]
pub struct SourceImage {
    width: usize,
    height: usize,
    luma: Vec<f32>,
    /// `None` when every pixel is fully opaque.
    alpha: Option<Vec<f32>>,
}

impl SourceImage {
    /// Decodes PNG, JPEG, WebP, GIF, BMP or TIFF bytes, applying EXIF
    /// orientation so phone photos come out upright.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut reader = ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .map_err(image::ImageError::IoError)?;
        let mut limits = Limits::default();
        limits.max_image_width = Some(MAX_DECODE_SIDE);
        limits.max_image_height = Some(MAX_DECODE_SIDE);
        limits.max_alloc = Some(MAX_DECODE_ALLOC);
        reader.limits(limits);

        let mut decoder = reader.into_decoder()?;
        // A broken EXIF block shouldn't make an otherwise fine image unreadable.
        let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
        let mut img = DynamicImage::from_decoder(decoder)?;
        img.apply_orientation(orientation);
        Self::from_dynamic(&img)
    }

    pub fn from_dynamic(img: &DynamicImage) -> Result<Self> {
        let (width, height) = (img.width() as usize, img.height() as usize);
        if width == 0 || height == 0 {
            return Err(Error::EmptyImage);
        }
        let high_precision = matches!(
            img,
            DynamicImage::ImageLuma16(_)
                | DynamicImage::ImageLumaA16(_)
                | DynamicImage::ImageRgb16(_)
                | DynamicImage::ImageRgba16(_)
                | DynamicImage::ImageRgb32F(_)
                | DynamicImage::ImageRgba32F(_)
        );
        let (luma, alpha) = if high_precision {
            img.to_rgba32f()
                .pixels()
                .map(|p| {
                    let [r, g, b, a] = p.0.map(unit);
                    (luminance(srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b)), a)
                })
                .unzip()
        } else {
            planes_from_rgba8(width, img.to_rgba8().as_raw())
        };
        Ok(Self::from_planes(width, height, luma, alpha))
    }

    /// Builds a source from straight (non-premultiplied) sRGB RGBA bytes, e.g.
    /// a browser `ImageData` buffer.
    pub fn from_rgba8(width: usize, height: usize, rgba: &[u8]) -> Result<Self> {
        if width == 0 || height == 0 {
            return Err(Error::EmptyImage);
        }
        let expected = width.checked_mul(height).and_then(|n| n.checked_mul(4));
        if expected != Some(rgba.len()) {
            return Err(Error::InvalidOption(format!(
                "RGBA buffer is {} bytes, expected {width}×{height}×4",
                rgba.len()
            )));
        }
        let (luma, alpha) = planes_from_rgba8(width, rgba);
        Ok(Self::from_planes(width, height, luma, alpha))
    }

    fn from_planes(width: usize, height: usize, luma: Vec<f32>, alpha: Vec<f32>) -> Self {
        let alpha = (!alpha.iter().all(|&a| a >= 1.0)).then_some(alpha);
        let src = Self { width, height, luma, alpha };
        let factor = width.max(height).div_ceil(MAX_WORKING_SIDE);
        if factor > 1 { src.box_downscale(factor) } else { src }
    }

    /// Averages `factor`×`factor` blocks. Luminance is alpha-weighted so
    /// transparent pixels don't drag colour into their neighbours.
    fn box_downscale(&self, factor: usize) -> Self {
        let (w, h) = (self.width, self.height);
        let (nw, nh) = (w.div_ceil(factor), h.div_ceil(factor));

        let rows = par::map(nh, |oy| {
            let mut sum_ya = vec![0f32; nw];
            let mut sum_y = vec![0f32; nw];
            let mut sum_a = vec![0f32; nw];
            let mut count = vec![0u32; nw];
            for y in oy * factor..((oy + 1) * factor).min(h) {
                for x in 0..w {
                    let i = y * w + x;
                    let o = x / factor;
                    let a = self.alpha.as_ref().map_or(1.0, |al| al[i]);
                    sum_ya[o] += self.luma[i] * a;
                    sum_y[o] += self.luma[i];
                    sum_a[o] += a;
                    count[o] += 1;
                }
            }
            (0..nw)
                .map(|o| {
                    let n = count[o] as f32;
                    let luma = if sum_a[o] > 0.0 { sum_ya[o] / sum_a[o] } else { sum_y[o] / n };
                    (luma, sum_a[o] / n)
                })
                .collect::<Vec<_>>()
        });
        let (luma, alpha): (Vec<f32>, Vec<f32>) = rows.into_iter().flatten().unzip();
        let alpha = self.alpha.as_ref().map(|_| alpha);
        Self { width: nw, height: nh, luma, alpha }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// Linear-light luminance, row-major.
    pub fn luma(&self) -> &[f32] {
        &self.luma
    }

    /// Per-pixel coverage, or `None` when the image is fully opaque.
    pub fn alpha(&self) -> Option<&[f32]> {
        self.alpha.as_deref()
    }
}

/// Straight sRGB RGBA bytes → (linear luminance, alpha) planes.
fn planes_from_rgba8(width: usize, rgba: &[u8]) -> (Vec<f32>, Vec<f32>) {
    let n = rgba.len() / 4;
    let lut = srgb8_to_linear_lut();
    let mut luma = vec![0f32; n];
    par::for_each_chunk(&mut luma, width, |y, row| {
        let src = &rgba[y * width * 4..];
        for (v, p) in row.iter_mut().zip(src.chunks_exact(4)) {
            *v = luminance(lut[p[0] as usize], lut[p[1] as usize], lut[p[2] as usize]);
        }
    });
    let alpha = rgba.chunks_exact(4).map(|p| p[3] as f32 / 255.0).collect();
    (luma, alpha)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, ImageFormat, Luma, Rgba};

    fn encode(img: DynamicImage, format: ImageFormat) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, format).unwrap();
        out.into_inner()
    }

    #[test]
    fn decodes_png_to_linear_luma() {
        let img = ImageBuffer::from_fn(4, 2, |x, _| Luma([if x < 2 { 0u8 } else { 255 }]));
        let src = SourceImage::decode(&encode(DynamicImage::ImageLuma8(img), ImageFormat::Png)).unwrap();
        assert_eq!((src.width(), src.height()), (4, 2));
        assert_eq!(src.luma()[0], 0.0);
        assert!((src.luma()[3] - 1.0).abs() < 1e-6);
        assert!(src.alpha().is_none());
    }

    #[test]
    fn sixteen_bit_png_uses_full_precision() {
        let img = ImageBuffer::from_pixel(2, 2, Luma([32768u16]));
        let src = SourceImage::decode(&encode(DynamicImage::ImageLuma16(img), ImageFormat::Png)).unwrap();
        let expected = srgb_to_linear(32768.0 / 65535.0);
        assert!((src.luma()[0] - expected).abs() < 1e-4);
    }

    #[test]
    fn keeps_alpha_only_when_needed() {
        let img = ImageBuffer::from_fn(2, 1, |x, _| Rgba([255, 255, 255, if x == 0 { 0 } else { 255 }]));
        let src = SourceImage::decode(&encode(DynamicImage::ImageRgba8(img), ImageFormat::Png)).unwrap();
        assert_eq!(src.alpha(), Some(&[0.0, 1.0][..]));
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(SourceImage::decode(b"definitely not an image").is_err());
        assert!(SourceImage::decode(&[]).is_err());
        let mut png = encode(DynamicImage::ImageLuma8(ImageBuffer::new(8, 8)), ImageFormat::Png);
        png.truncate(png.len() / 2);
        assert!(SourceImage::decode(&png).is_err());
    }

    #[test]
    fn rejects_mismatched_rgba_buffer() {
        assert!(SourceImage::from_rgba8(2, 2, &[0; 15]).is_err());
        assert!(SourceImage::from_rgba8(0, 2, &[]).is_err());
    }

    #[test]
    fn huge_images_are_box_filtered_in_linear_light() {
        let w = MAX_WORKING_SIDE * 2;
        // Alternating black/white columns must average to 0.5 linear, not 0.5 sRGB.
        let rgba: Vec<u8> = (0..w * 2)
            .flat_map(|i| {
                let v = if i % 2 == 0 { 0 } else { 255 };
                [v, v, v, 255]
            })
            .collect();
        let src = SourceImage::from_rgba8(w, 2, &rgba).unwrap();
        assert_eq!((src.width(), src.height()), (MAX_WORKING_SIDE, 1));
        assert!(src.luma().iter().all(|&y| (y - 0.5).abs() < 1e-6));
    }
}
