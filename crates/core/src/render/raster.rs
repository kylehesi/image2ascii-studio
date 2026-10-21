//! PNG export. Compositing happens in linear light with premultiplied alpha:
//! glyph coverage is an area fraction, so blending it in sRGB would make thin
//! strokes too dark and undo the tone calibration.

use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ExtendedColorType, ImageEncoder, RgbaImage};

use super::{Layout, Rgb, Style, Typeface};
use crate::AsciiArt;
use crate::color::srgb_to_linear;
use crate::error::Result;

/// Renders `art` to an RGBA image (transparent outside a frame's rounded corners).
pub fn render_rgba(art: &AsciiArt, face: &Typeface, style: &Style) -> Result<RgbaImage> {
    let lay = Layout::new(art, face, style, true)?;
    let mut canvas = Canvas::new(lay.width as usize, lay.height as usize);

    match &lay.frame {
        Some(f) => {
            let (w, h) = (lay.width, lay.height);
            canvas.rounded_rect(0.0, 0.0, w, h, f.radius, f.border);
            canvas.rounded_rect(1.0, 1.0, w - 2.0, h - 2.0, (f.radius - 1.0).max(0.0), style.bg);
            for (x, y, c) in f.dots {
                canvas.circle(x, y, f.dot_radius, c);
            }
            let total: f32 = f.title.chars().map(|c| face.advance(c, f.title_px)).sum();
            let mut x = f.title_centre_x - total / 2.0;
            for ch in f.title.chars() {
                let g = face.raster(ch, f.title_px);
                canvas.glyph(&g, x.round() as i32, f.title_baseline.round() as i32, f.title_color);
                x += g.advance;
            }
        }
        None => canvas.rounded_rect(0.0, 0.0, lay.width, lay.height, 0.0, style.bg),
    }

    let mut cache = std::collections::HashMap::new();
    for r in 0..art.rows() {
        let y = (lay.origin.1 + r as f32 * lay.cell_h + lay.baseline) as i32;
        for (c, &ch) in art.row(r).iter().enumerate() {
            if ch == ' ' {
                continue;
            }
            let g = cache.entry(ch).or_insert_with(|| face.raster(ch, style.px));
            let x = (lay.origin.0 + c as f32 * lay.cell_w) as i32;
            canvas.glyph(g, x, y, style.fg);
        }
    }
    Ok(canvas.into_image())
}

/// [`render_rgba`] encoded as PNG bytes. Maximum compression: text art is
/// mostly flat background, and this cuts files to about a third of the default.
pub fn render_png(art: &AsciiArt, face: &Typeface, style: &Style) -> Result<Vec<u8>> {
    let img = render_rgba(art, face, style)?;
    let mut out = Vec::new();
    PngEncoder::new_with_quality(&mut out, CompressionType::Best, FilterType::Adaptive).write_image(
        img.as_raw(),
        img.width(),
        img.height(),
        ExtendedColorType::Rgba8,
    )?;
    Ok(out)
}

/// Premultiplied linear-light RGBA.
struct Canvas {
    width: usize,
    height: usize,
    px: Vec<[f32; 4]>,
}

fn linear(c: Rgb) -> [f32; 3] {
    [c.0, c.1, c.2].map(|v| srgb_to_linear(v as f32 / 255.0))
}

impl Canvas {
    fn new(width: usize, height: usize) -> Self {
        Self { width, height, px: vec![[0.0; 4]; width * height] }
    }

    /// Source-over with `coverage` of opaque `color`.
    fn blend(&mut self, x: usize, y: usize, coverage: f32, color: [f32; 3]) {
        if coverage <= 0.0 {
            return;
        }
        let a = coverage.min(1.0);
        let p = &mut self.px[y * self.width + x];
        for i in 0..3 {
            p[i] = color[i] * a + p[i] * (1.0 - a);
        }
        p[3] = a + p[3] * (1.0 - a);
    }

    /// Anti-aliased fill of a shape given by its signed distance (in pixels)
    /// at each pixel centre, over a bounding box.
    fn fill(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, color: Rgb, sdf: impl Fn(f32, f32) -> f32) {
        let color = linear(color);
        let (xa, ya) = (x0.floor().max(0.0) as usize, y0.floor().max(0.0) as usize);
        let (xb, yb) = ((x1.ceil() as usize).min(self.width), (y1.ceil() as usize).min(self.height));
        for y in ya..yb {
            for x in xa..xb {
                let d = sdf(x as f32 + 0.5, y as f32 + 0.5);
                self.blend(x, y, (0.5 - d).clamp(0.0, 1.0), color);
            }
        }
    }

    fn rounded_rect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, color: Rgb) {
        let (cx, cy, hx, hy) = (x + w / 2.0, y + h / 2.0, w / 2.0 - r, h / 2.0 - r);
        self.fill(x, y, x + w, y + h, color, |px, py| {
            let qx = (px - cx).abs() - hx;
            let qy = (py - cy).abs() - hy;
            qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
        });
    }

    fn circle(&mut self, cx: f32, cy: f32, r: f32, color: Rgb) {
        self.fill(cx - r - 1.0, cy - r - 1.0, cx + r + 1.0, cy + r + 1.0, color, |x, y| (x - cx).hypot(y - cy) - r);
    }

    /// Draws a glyph with its pen position at `x` and baseline at `y`.
    fn glyph(&mut self, g: &super::GlyphRaster, x: i32, y: i32, color: Rgb) {
        let color = linear(color);
        let left = x + g.xmin;
        let top = y - (g.ymin + g.height as i32);
        for gy in 0..g.height {
            let py = top + gy as i32;
            if !(0..self.height as i32).contains(&py) {
                continue;
            }
            for gx in 0..g.width {
                let px = left + gx as i32;
                if !(0..self.width as i32).contains(&px) {
                    continue;
                }
                let coverage = g.coverage[gy * g.width + gx] as f32 / 255.0;
                self.blend(px as usize, py as usize, coverage, color);
            }
        }
    }

    fn into_image(self) -> RgbaImage {
        let encode = |v: f32| {
            let v = v.clamp(0.0, 1.0);
            let s = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
            (s * 255.0).round() as u8
        };
        let raw = self
            .px
            .iter()
            .flat_map(|p| {
                let a = p[3];
                let un = |c: f32| if a > 0.0 { c / a } else { 0.0 };
                [encode(un(p[0])), encode(un(p[1])), encode(un(p[2])), (a * 255.0).round() as u8]
            })
            .collect();
        RgbaImage::from_raw(self.width as u32, self.height as u32, raw).expect("buffer matches size")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::Frame;
    use crate::{Background, GlyphAtlas, Options, SourceImage};

    fn art(fill: u8) -> AsciiArt {
        let src = SourceImage::from_rgba8(8, 4, &[fill, fill, fill, 255].repeat(32)).unwrap();
        let atlas = GlyphAtlas::from_coverages(&[(' ', 0.0), ('#', 1.0)], 0.5).unwrap();
        crate::convert(&src, &atlas, &Options { cols: 4, auto_levels: false, clahe: 0.0, ..Options::DEFAULT }).unwrap()
    }

    fn style(frame: bool) -> Style {
        let mut s = Style::for_background(Background::Dark);
        s.px = 10.0;
        s.frame = frame.then(|| Frame { title: "t".into() });
        s
    }

    #[test]
    fn png_round_trips_with_expected_size() {
        let a = art(255);
        let png = render_png(&a, &Typeface::boxes(), &style(false)).unwrap();
        let img = image::load_from_memory(&png).unwrap().to_rgba8();
        // 10 px padding each side; box cells are 6×12.
        assert_eq!(img.dimensions(), (10 * 2 + 6 * 4, 10 * 2 + 12 * a.rows() as u32));
    }

    #[test]
    fn blank_art_is_background_and_ink_is_foreground() {
        let s = style(false);
        let blank = render_rgba(&art(0), &Typeface::boxes(), &s).unwrap();
        assert!(blank.pixels().all(|p| p.0 == [s.bg.0, s.bg.1, s.bg.2, 255]));
        let full = render_rgba(&art(255), &Typeface::boxes(), &s).unwrap();
        // Centre of the first cell is inside a solid box glyph.
        let p = full.get_pixel(10 + 3, 10 + 6);
        assert_eq!(p.0, [s.fg.0, s.fg.1, s.fg.2, 255]);
    }

    #[test]
    fn frame_has_transparent_corners_and_dots() {
        let img = render_rgba(&art(255), &Typeface::boxes(), &style(true)).unwrap();
        assert_eq!(img.get_pixel(0, 0).0[3], 0, "outside the rounded corner");
        let (w, _) = img.dimensions();
        assert_eq!(img.get_pixel(w / 2, 1).0[3], 255, "border is opaque");
        // First dot centre: padding + radius, half the 22.5→23 px title bar.
        let dot = img.get_pixel(14, 11).0;
        assert_eq!(&dot[..3], &[0xff, 0x5f, 0x57]);
    }

    #[test]
    fn half_coverage_blends_in_linear_light() {
        let mut c = Canvas::new(1, 1);
        c.blend(0, 0, 1.0, linear(Rgb(0, 0, 0)));
        c.blend(0, 0, 0.5, linear(Rgb(255, 255, 255)));
        let v = c.into_image().get_pixel(0, 0).0[0];
        // 50% white over black emits half the light: sRGB ≈ 188, not 128.
        assert!((187..=189).contains(&v), "{v}");
    }
}
