//! PNG export. Compositing happens in linear light: glyph coverage is an area
//! fraction, so blending it in sRGB would make thin strokes too dark and undo
//! the tone calibration. Pixels are stored as 8-bit sRGB (4 bytes each, so an
//! 8K canvas fits comfortably in browser memory) and converted through lookup
//! tables around each blend.

use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ExtendedColorType, ImageEncoder, RgbaImage};

use super::{GlyphRaster, Layout, Rgb, Style, Typeface};
use crate::AsciiArt;
use crate::color::{srgb8_to_linear_lut, srgb_to_linear};
use crate::error::Result;

/// Below this font size glyphs are placed at fractional positions; rounding
/// to whole pixels would make small cells visibly uneven. Above it, rounding
/// keeps strokes crisp and the unevenness is negligible.
const SUBPIXEL_BELOW_PX: f32 = 20.0;
/// Canvases larger than this get standard rather than maximum compression,
/// which would take seconds for little gain.
const BEST_COMPRESSION_MAX_PIXELS: u64 = 4_000_000;

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
                canvas.glyph(&g, x, f.title_baseline, f.title_px, f.title_color);
                x += g.advance;
            }
        }
        None => canvas.rounded_rect(0.0, 0.0, lay.width, lay.height, 0.0, style.bg),
    }

    let mut cache = std::collections::HashMap::new();
    for r in 0..art.rows() {
        let y = lay.origin.1 + r as f32 * lay.cell_h + lay.baseline;
        for (c, &ch) in art.row(r).iter().enumerate() {
            if ch == ' ' {
                continue;
            }
            let g = cache.entry(ch).or_insert_with(|| face.raster(ch, lay.font_px));
            canvas.glyph(g, lay.origin.0 + c as f32 * lay.cell_w, y, lay.font_px, style.fg);
        }
    }
    Ok(canvas.into_image())
}

/// [`render_rgba`] encoded as PNG bytes. Text art is mostly flat background,
/// so maximum compression cuts typical files to about a third.
pub fn render_png(art: &AsciiArt, face: &Typeface, style: &Style) -> Result<Vec<u8>> {
    let img = render_rgba(art, face, style)?;
    let compression = if img.width() as u64 * img.height() as u64 <= BEST_COMPRESSION_MAX_PIXELS {
        CompressionType::Best
    } else {
        CompressionType::Default
    };
    let mut out = Vec::new();
    PngEncoder::new_with_quality(&mut out, compression, FilterType::Adaptive).write_image(
        img.as_raw(),
        img.width(),
        img.height(),
        ExtendedColorType::Rgba8,
    )?;
    Ok(out)
}

/// Straight-alpha 8-bit sRGB pixels, blended in linear light.
struct Canvas {
    width: usize,
    height: usize,
    px: Vec<[u8; 4]>,
    to_linear: [f32; 256],
    /// Linear value (quantised to 4096 steps) → sRGB byte.
    to_srgb: Vec<u8>,
}

const ENCODE_STEPS: usize = 4096;

fn linear(c: Rgb) -> [f32; 3] {
    [c.0, c.1, c.2].map(|v| srgb_to_linear(v as f32 / 255.0))
}

impl Canvas {
    fn new(width: usize, height: usize) -> Self {
        let to_srgb = (0..ENCODE_STEPS)
            .map(|i| {
                let v = i as f32 / (ENCODE_STEPS - 1) as f32;
                let s = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
                (s * 255.0).round() as u8
            })
            .collect();
        Self { width, height, px: vec![[0; 4]; width * height], to_linear: srgb8_to_linear_lut(), to_srgb }
    }

    fn encode(&self, v: f32) -> u8 {
        self.to_srgb[(v.clamp(0.0, 1.0) * (ENCODE_STEPS - 1) as f32).round() as usize]
    }

    /// Source-over with `coverage` of opaque `color` (linear).
    fn blend(&mut self, x: usize, y: usize, coverage: f32, color: [f32; 3]) {
        if coverage <= 0.0 {
            return;
        }
        let a = coverage.min(1.0);
        let i = y * self.width + x;
        let p = self.px[i];
        let da = p[3] as f32 / 255.0;
        let out_a = a + da * (1.0 - a);
        let mut out = [0u8; 4];
        for c in 0..3 {
            let d = self.to_linear[p[c] as usize];
            out[c] = self.encode((color[c] * a + d * da * (1.0 - a)) / out_a);
        }
        out[3] = (out_a * 255.0).round() as u8;
        self.px[i] = out;
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

    /// Draws a glyph rasterised at `px` with its pen position at `x` and
    /// baseline at `y`. Small sizes keep the fractional position (the bitmap
    /// is shifted by bilinear interpolation); large ones snap to whole pixels.
    fn glyph(&mut self, g: &GlyphRaster, x: f32, y: f32, px: f32, color: Rgb) {
        let color = linear(color);
        let left = x + g.xmin as f32;
        let top = y - (g.ymin + g.height as i32) as f32;
        let (ix, iy, fx, fy) = if px < SUBPIXEL_BELOW_PX {
            (left.floor(), top.floor(), left - left.floor(), top - top.floor())
        } else {
            (left.round(), top.round(), 0.0, 0.0)
        };
        let src = |gx: isize, gy: isize| {
            if (0..g.width as isize).contains(&gx) && (0..g.height as isize).contains(&gy) {
                g.coverage[gy as usize * g.width + gx as usize] as f32 / 255.0
            } else {
                0.0
            }
        };
        // With a fractional shift the glyph spreads into one more column and row.
        let (extra_x, extra_y) = ((fx > 0.0) as isize, (fy > 0.0) as isize);
        for dy in 0..g.height as isize + extra_y {
            let py = iy as isize + dy;
            if !(0..self.height as isize).contains(&py) {
                continue;
            }
            for dx in 0..g.width as isize + extra_x {
                let px_ = ix as isize + dx;
                if !(0..self.width as isize).contains(&px_) {
                    continue;
                }
                // Destination (dx, dy) samples the source at (dx − fx, dy − fy).
                let coverage = (1.0 - fy) * ((1.0 - fx) * src(dx, dy) + fx * src(dx - 1, dy))
                    + fy * ((1.0 - fx) * src(dx, dy - 1) + fx * src(dx - 1, dy - 1));
                self.blend(px_ as usize, py as usize, coverage, color);
            }
        }
    }

    fn into_image(self) -> RgbaImage {
        let raw = self.px.into_iter().flatten().collect();
        RgbaImage::from_raw(self.width as u32, self.height as u32, raw).expect("buffer matches size")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::{Canvas as Size, Frame};
    use crate::{Background, GlyphAtlas, Options, SourceImage};

    fn art(fill: u8) -> AsciiArt {
        let src = SourceImage::from_rgba8(8, 4, &[fill, fill, fill, 255].repeat(32)).unwrap();
        let atlas = GlyphAtlas::from_coverages(&[(' ', 0.0), ('#', 1.0)], 0.5).unwrap();
        crate::convert(&src, &atlas, &Options { cols: 4, auto_levels: false, clahe: 0.0, ..Options::DEFAULT }).unwrap()
    }

    fn style(frame: bool) -> Style {
        let mut s = Style::for_background(Background::Dark);
        s.canvas = Size::FontSize(10.0);
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

    /// Bounding box of pixels that differ from the background.
    fn ink_box(img: &RgbaImage, bg: Rgb) -> (u32, u32, u32, u32) {
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
        for (x, y, p) in img.enumerate_pixels() {
            if p.0[..3] != [bg.0, bg.1, bg.2] {
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            }
        }
        (x0, y0, x1, y1)
    }

    #[test]
    fn fixed_canvas_is_exact_and_letterboxes_the_art() {
        // The art is twice as wide as tall (4 box cells of 0.6 × 1 row of 1.2),
        // so in a square canvas it fills the width and sits centred vertically.
        let a = art(255);
        let mut s = style(false);
        s.canvas = Size::Fixed { width: 500, height: 500 };
        let img = render_rgba(&a, &Typeface::boxes(), &s).unwrap();
        assert_eq!(img.dimensions(), (500, 500));
        let (x0, y0, x1, y1) = ink_box(&img, s.bg);
        // Padding is 3% of the short side: 15 px.
        assert!(x0.abs_diff(15) <= 1 && x1.abs_diff(484) <= 1, "x {x0}..{x1}");
        let (top, bottom) = (y0 as i64, 499 - y1 as i64);
        assert!((top - bottom).abs() <= 2, "centred vertically: {top} above, {bottom} below");
        assert!(y1 - y0 > 200 && y1 - y0 < 260, "half the width tall: {}", y1 - y0);
    }

    #[test]
    fn fixed_canvas_with_frame_keeps_its_size() {
        let mut s = style(true);
        s.canvas = Size::Fixed { width: 640, height: 360 };
        let img = render_rgba(&art(255), &Typeface::boxes(), &s).unwrap();
        assert_eq!(img.dimensions(), (640, 360));
    }

    #[test]
    fn canvas_limits_are_enforced() {
        let a = art(255);
        for (w, h) in [(16, 500), (500, 9000), (8192, 8192)] {
            let mut s = style(false);
            s.canvas = Size::Fixed { width: w, height: h };
            assert!(render_rgba(&a, &Typeface::boxes(), &s).is_err(), "{w}×{h}");
        }
        // Scaling past the limits shrinks proportionally instead of failing.
        let (canvas, capped) = Size::fixed_scaled(6000, 4000, 4.0);
        assert!(capped);
        let Size::Fixed { width, height } = canvas else { panic!() };
        assert!(width <= 8192 && (width as u64 * height as u64) <= crate::render::MAX_CANVAS_PIXELS);
        assert!(((width as f32 / height as f32) - 1.5).abs() < 0.01, "aspect kept: {width}×{height}");
        assert_eq!(Size::fixed_scaled(430, 383, 2.0), (Size::Fixed { width: 860, height: 766 }, false));
    }

    #[test]
    fn small_glyphs_keep_fractional_positions() {
        // A one-pixel glyph drawn half a pixel to the right splits evenly.
        let g = GlyphRaster { xmin: 0, ymin: 0, width: 1, height: 1, coverage: vec![255], advance: 1.0 };
        let mut c = Canvas::new(3, 1);
        c.glyph(&g, 0.5, 1.0, 8.0, Rgb(255, 255, 255));
        let img = c.into_image();
        let (a, b) = (img.get_pixel(0, 0).0[3], img.get_pixel(1, 0).0[3]);
        assert!(a.abs_diff(128) <= 1 && b.abs_diff(128) <= 1, "{a} {b}");
        // Large glyphs snap instead.
        let mut c = Canvas::new(3, 1);
        c.glyph(&g, 0.6, 1.0, 32.0, Rgb(255, 255, 255));
        assert_eq!(c.into_image().get_pixel(1, 0).0[3], 255);
    }
}
