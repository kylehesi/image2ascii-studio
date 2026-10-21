//! Exporters beyond plain text: SVG (optionally with the reveal animation)
//! and PNG. Both lay the art out on the font's own cell grid, optionally
//! inside a window frame (title bar with three dots, like a terminal).

mod raster;
mod svg;

pub use raster::{render_png, render_rgba};
pub use svg::{SvgGlyphs, SvgOptions, render_svg};

use std::fmt::Write as _;

use fontdue::{Font, FontSettings};

use crate::atlas::CellMetrics;
use crate::error::{Error, Result};
use crate::{AsciiArt, Background};

/// A parsed font for rendering: bitmaps for PNG, outlines for SVG.
pub struct Typeface {
    inner: Inner,
}

// The size gap only exists in test builds, and there's one Typeface per run.
#[allow(clippy::large_enum_variant)]
enum Inner {
    Font { data: Vec<u8>, raster: Font },
    /// Every glyph is a solid box: lets render tests run without font files.
    #[cfg(test)]
    Boxes,
}

/// A rasterised glyph, positioned like fontdue's metrics: `xmin` from the
/// pen position, `ymin` from the baseline to the bitmap's bottom edge.
pub(crate) struct GlyphRaster {
    pub xmin: i32,
    pub ymin: i32,
    pub width: usize,
    pub height: usize,
    pub coverage: Vec<u8>,
    pub advance: f32,
}

impl Typeface {
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        let raster = Font::from_bytes(data, FontSettings::default()).map_err(|e| Error::Font(e.to_string()))?;
        ttf_parser::Face::parse(data, 0).map_err(|e| Error::Font(e.to_string()))?;
        Ok(Self { inner: Inner::Font { data: data.to_vec(), raster } })
    }

    #[cfg(test)]
    pub(crate) fn boxes() -> Self {
        Self { inner: Inner::Boxes }
    }

    /// Character cell (advance, line height) in pixels at font size `px` —
    /// what a text view needs to lay glyphs out on the calibrated grid.
    pub fn cell_size(&self, px: f32) -> Result<(f32, f32)> {
        self.metrics(px).map(|m| (m.advance, m.line_height))
    }

    /// (ascent, descent) at font size `px`; descent is negative.
    pub fn vertical_metrics(&self, px: f32) -> Result<(f32, f32)> {
        self.metrics(px).map(|m| (m.ascent, m.descent))
    }

    pub(crate) fn metrics(&self, px: f32) -> Result<CellMetrics> {
        match &self.inner {
            Inner::Font { raster, .. } => CellMetrics::of(raster, px),
            #[cfg(test)]
            Inner::Boxes => Ok(CellMetrics { advance: px * 0.6, line_height: px * 1.2, ascent: px * 0.9, descent: -px * 0.3 }),
        }
    }

    pub(crate) fn raster(&self, ch: char, px: f32) -> GlyphRaster {
        match &self.inner {
            Inner::Font { raster, .. } => {
                let (m, coverage) = raster.rasterize(ch, px);
                GlyphRaster { xmin: m.xmin, ymin: m.ymin, width: m.width, height: m.height, coverage, advance: m.advance_width }
            }
            #[cfg(test)]
            Inner::Boxes => {
                let m = self.metrics(px).expect("box metrics");
                let (width, height) = (m.advance.round() as usize, (m.ascent - m.descent).round() as usize);
                let coverage = if ch == ' ' { vec![0; width * height] } else { vec![255; width * height] };
                GlyphRaster { xmin: 0, ymin: m.descent.round() as i32, width, height, coverage, advance: m.advance }
            }
        }
    }

    /// SVG path data for `ch` at `px`, with the origin on the baseline at the
    /// pen position and y pointing down. `None` for glyphs without ink.
    pub(crate) fn outline(&self, ch: char, px: f32) -> Option<String> {
        match &self.inner {
            Inner::Font { data, .. } => {
                let face = ttf_parser::Face::parse(data, 0).ok()?;
                let id = face.glyph_index(ch)?;
                let mut path = PathData { d: String::new(), scale: px / face.units_per_em() as f32 };
                face.outline_glyph(id, &mut path)?;
                Some(path.d)
            }
            #[cfg(test)]
            Inner::Boxes => {
                let m = self.metrics(px).ok()?;
                (ch != ' ').then(|| format!("M0 {}H{}V{}H0Z", num(-m.ascent), num(m.advance), num(-m.descent)))
            }
        }
    }

    pub(crate) fn advance(&self, ch: char, px: f32) -> f32 {
        match &self.inner {
            Inner::Font { raster, .. } => raster.metrics(ch, px).advance_width,
            #[cfg(test)]
            Inner::Boxes => self.metrics(px).map(|m| m.advance).unwrap_or(0.0),
        }
    }
}

struct PathData {
    d: String,
    scale: f32,
}

impl PathData {
    fn point(&mut self, x: f32, y: f32) {
        let _ = write!(self.d, "{} {}", num(x * self.scale), num(-y * self.scale));
    }
}

impl ttf_parser::OutlineBuilder for PathData {
    fn move_to(&mut self, x: f32, y: f32) {
        self.d.push('M');
        self.point(x, y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.d.push('L');
        self.point(x, y);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.d.push('Q');
        self.point(x1, y1);
        self.d.push(' ');
        self.point(x, y);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.d.push('C');
        self.point(x1, y1);
        self.d.push(' ');
        self.point(x2, y2);
        self.d.push(' ');
        self.point(x, y);
    }
    fn close(&mut self) {
        self.d.push('Z');
    }
}

/// Compact decimal: at most two places, no trailing zeros, no `-0`.
pub(crate) fn num(v: f32) -> String {
    let s = format!("{:.2}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" || s.is_empty() { "0".to_owned() } else { s.to_owned() }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// Parses `#rrggbb`, `rrggbb` or `#rgb`.
    pub fn parse(s: &str) -> Result<Self> {
        let hex = s.strip_prefix('#').unwrap_or(s);
        let bad = || Error::InvalidOption(format!("{s:?} is not a #rrggbb colour"));
        let digits: Vec<u8> = hex.chars().map(|c| c.to_digit(16).map(|d| d as u8)).collect::<Option<_>>().ok_or_else(bad)?;
        match digits.as_slice() {
            [r, g, b] => Ok(Self(r * 17, g * 17, b * 17)),
            [r1, r0, g1, g0, b1, b0] => Ok(Self(r1 * 16 + r0, g1 * 16 + g0, b1 * 16 + b0)),
            _ => Err(bad()),
        }
    }

    pub fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }
}

/// Window chrome around the art.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub title: String,
}

/// Smallest and largest exported canvas side, and the most pixels in total
/// (8K UHD) — enough for any screen, small enough for browser memory.
pub const MIN_CANVAS_SIDE: u32 = 32;
pub const MAX_CANVAS_SIDE: u32 = 8192;
pub const MAX_CANVAS_PIXELS: u64 = 7680 * 4320;

/// How big the exported image is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Canvas {
    /// Glyphs at this font size (px); the image grows to fit the art.
    FontSize(f32),
    /// Exactly `width`×`height` pixels. The art is scaled to fit inside and
    /// centred; space it doesn't fill (a different aspect ratio) stays background.
    Fixed { width: u32, height: u32 },
}

impl Canvas {
    /// `Fixed` at `width`×`height` scaled by `scale`, shrunk proportionally if
    /// that would exceed the canvas limits. The bool says whether it was shrunk.
    pub fn fixed_scaled(width: u32, height: u32, scale: f32) -> (Self, bool) {
        let (w, h) = (width as f64 * scale as f64, height as f64 * scale as f64);
        let side = (MAX_CANVAS_SIDE as f64 / w.max(h)).min(1.0);
        let area = (MAX_CANVAS_PIXELS as f64 / (w * h)).sqrt().min(1.0);
        let shrink = side.min(area);
        let fit = |v: f64| ((v * shrink).floor() as u32).max(MIN_CANVAS_SIDE);
        (Self::Fixed { width: fit(w), height: fit(h) }, shrink < 1.0)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Style {
    pub canvas: Canvas,
    pub fg: Rgb,
    pub bg: Rgb,
    pub frame: Option<Frame>,
}

impl Style {
    /// GitHub's palette for the background the art was converted for.
    pub fn for_background(background: Background) -> Self {
        let (fg, bg) = match background {
            Background::Dark => (Rgb(0xc9, 0xd1, 0xd9), Rgb(0x0d, 0x11, 0x17)),
            Background::Light => (Rgb(0x1f, 0x23, 0x28), Rgb(0xff, 0xff, 0xff)),
        };
        Self { canvas: Canvas::FontSize(16.0), fg, bg, frame: None }
    }

    fn validate(&self) -> Result<()> {
        let bad = |msg: String| Err(Error::InvalidOption(msg));
        match self.canvas {
            Canvas::FontSize(px) if !(px.is_finite() && (4.0..=256.0).contains(&px)) => {
                bad(format!("font size must be 4..=256 px, got {px}"))
            }
            Canvas::Fixed { width, height }
                if !(MIN_CANVAS_SIDE..=MAX_CANVAS_SIDE).contains(&width)
                    || !(MIN_CANVAS_SIDE..=MAX_CANVAS_SIDE).contains(&height) =>
            {
                bad(format!("canvas sides must be {MIN_CANVAS_SIDE}..={MAX_CANVAS_SIDE} px, got {width}×{height}"))
            }
            Canvas::Fixed { width, height } if width as u64 * height as u64 > MAX_CANVAS_PIXELS => {
                bad(format!("canvas {width}×{height} exceeds {MAX_CANVAS_PIXELS} pixels"))
            }
            _ => Ok(()),
        }
    }

    /// Frame border and title colours: the foreground faded towards the background.
    fn muted(&self, amount: f32) -> Rgb {
        let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * amount).round() as u8;
        Rgb(mix(self.bg.0, self.fg.0), mix(self.bg.1, self.fg.1), mix(self.bg.2, self.fg.2))
    }
}

const DOTS: [Rgb; 3] = [Rgb(0xff, 0x5f, 0x57), Rgb(0xfe, 0xbc, 0x2e), Rgb(0x28, 0xc8, 0x40)];

/// Where everything goes, in pixels: the geometry every exporter (and the
/// web preview) draws with, so they all agree.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub width: f32,
    pub height: f32,
    /// Font size the glyphs are drawn at.
    pub font_px: f32,
    /// Top-left of the character grid.
    pub origin: (f32, f32),
    pub cell_w: f32,
    pub cell_h: f32,
    /// Baseline offset within a cell.
    pub baseline: f32,
    pub frame: Option<FrameLayout>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FrameLayout {
    pub bar_height: f32,
    pub radius: f32,
    pub border: Rgb,
    pub dots: [(f32, f32, Rgb); 3],
    pub dot_radius: f32,
    pub title: String,
    pub title_px: f32,
    pub title_color: Rgb,
    /// Title baseline, centred in the title bar.
    pub title_baseline: f32,
    pub title_centre_x: f32,
}

/// Title-bar height per pixel of font size in `Canvas::FontSize` mode; the
/// rest of the chrome is proportioned from the bar so both modes look alike.
const BAR_PER_PX: f32 = 2.25;

/// The layout `art` gets with `style` — what the SVG export uses.
pub fn layout(art: &AsciiArt, face: &Typeface, style: &Style) -> Result<Layout> {
    Layout::new(art, face, style, false)
}

impl Layout {
    /// `snap` rounds margins and the grid origin to whole pixels (PNG), and in
    /// `FontSize` mode the cells too, so glyphs land crisply on the pixel grid.
    pub(crate) fn new(art: &AsciiArt, face: &Typeface, style: &Style, snap: bool) -> Result<Self> {
        style.validate()?;
        let round = |v: f32| if snap { v.round().max(1.0) } else { v };
        let framed = style.frame.is_some();
        let (cols, rows) = (art.cols() as f32, art.rows() as f32);

        let (width, height, font_px, m, baseline, origin, bar) = match style.canvas {
            Canvas::FontSize(px) => {
                let m = face.metrics(px)?;
                let baseline = round(m.baseline());
                let (cell_w, cell_h) = (round(m.advance), round(m.line_height));
                let pad = round(px);
                let bar = if framed { round(px * BAR_PER_PX) } else { 0.0 };
                let width = round(pad * 2.0 + cell_w * cols);
                let height = round(bar + pad * 2.0 + cell_h * rows - if framed { pad * 0.5 } else { 0.0 });
                let origin = (pad, if framed { bar + pad * 0.5 } else { pad });
                let m = CellMetrics { advance: cell_w, line_height: cell_h, ..m };
                (width, height, px, m, baseline, origin, bar)
            }
            Canvas::Fixed { width, height } => {
                let (w, h) = (width as f32, height as f32);
                // Margins scale with the canvas: 3% of the short side, bar 7%.
                let unit = w.min(h) / 100.0;
                let pad = round((unit * 3.0).max(4.0));
                let bar = if framed { round((unit * 7.0).clamp(16.0, 72.0)) } else { 0.0 };
                let top = if framed { bar + pad * 0.5 } else { pad };
                let (box_w, box_h) = (w - pad * 2.0, h - top - pad);
                // Metrics are linear in size: measure once, then fit.
                let unit_m = face.metrics(100.0)?;
                let px = 100.0
                    * (box_w / (cols * unit_m.advance)).min(box_h / (rows * unit_m.line_height));
                if !(box_w > 0.0 && box_h > 0.0 && px > 0.0) {
                    return Err(Error::InvalidOption(format!("canvas {width}×{height} is too small for the frame")));
                }
                let m = face.metrics(px)?;
                let (grid_w, grid_h) = (m.advance * cols, m.line_height * rows);
                let origin = (pad + (box_w - grid_w) / 2.0, top + (box_h - grid_h) / 2.0);
                let origin = if snap { (origin.0.round(), origin.1.round()) } else { origin };
                (w, h, px, m, m.baseline(), origin, bar)
            }
        };

        let frame = style.frame.as_ref().map(|f| {
            let dot_radius = bar * 0.4 / BAR_PER_PX;
            let inset = bar / BAR_PER_PX;
            let dot = |i: usize| (inset + dot_radius + i as f32 * dot_radius * 3.2, bar / 2.0, DOTS[i]);
            let title_px = bar * 0.85 / BAR_PER_PX;
            let tm = face.metrics(title_px).unwrap_or(m);
            FrameLayout {
                bar_height: bar,
                radius: bar * 0.6 / BAR_PER_PX,
                border: style.muted(0.18),
                dots: [dot(0), dot(1), dot(2)],
                dot_radius,
                title: f.title.clone(),
                title_px,
                title_color: style.muted(0.55),
                title_baseline: bar / 2.0 + (tm.ascent + tm.descent) / 2.0,
                title_centre_x: width / 2.0,
            }
        });
        Ok(Self { width, height, font_px, origin, cell_w: m.advance, cell_h: m.line_height, baseline, frame })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_compact() {
        assert_eq!(num(1.0), "1");
        assert_eq!(num(1.5), "1.5");
        assert_eq!(num(1.256), "1.26");
        assert_eq!(num(-0.001), "0");
        assert_eq!(num(-3.1), "-3.1");
    }

    #[test]
    fn colours_parse() {
        assert_eq!(Rgb::parse("#0d1117").unwrap(), Rgb(13, 17, 23));
        assert_eq!(Rgb::parse("fff").unwrap(), Rgb(255, 255, 255));
        assert_eq!(Rgb(13, 17, 23).hex(), "#0d1117");
        assert!(Rgb::parse("#12345").is_err());
        assert!(Rgb::parse("#gggggg").is_err());
    }
}
