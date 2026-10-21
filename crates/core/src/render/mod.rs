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

#[derive(Debug, Clone, PartialEq)]
pub struct Style {
    /// Font size in pixels.
    pub px: f32,
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
        Self { px: 16.0, fg, bg, frame: None }
    }

    fn validate(&self) -> Result<()> {
        if !(self.px.is_finite() && (4.0..=256.0).contains(&self.px)) {
            return Err(Error::InvalidOption(format!("font size must be 4..=256 px, got {}", self.px)));
        }
        Ok(())
    }

    /// Frame border and title colours: the foreground faded towards the background.
    fn muted(&self, amount: f32) -> Rgb {
        let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * amount).round() as u8;
        Rgb(mix(self.bg.0, self.fg.0), mix(self.bg.1, self.fg.1), mix(self.bg.2, self.fg.2))
    }
}

const DOTS: [Rgb; 3] = [Rgb(0xff, 0x5f, 0x57), Rgb(0xfe, 0xbc, 0x2e), Rgb(0x28, 0xc8, 0x40)];

/// Where everything goes, in pixels. Shared by SVG (exact) and PNG (snapped
/// to whole pixels so glyph cells stay crisp).
pub(crate) struct Layout {
    pub width: f32,
    pub height: f32,
    /// Top-left of the character grid.
    pub origin: (f32, f32),
    pub cell_w: f32,
    pub cell_h: f32,
    /// Baseline offset within a cell.
    pub baseline: f32,
    pub frame: Option<FrameLayout>,
}

pub(crate) struct FrameLayout {
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

impl Layout {
    pub fn new(art: &AsciiArt, face: &Typeface, style: &Style, snap: bool) -> Result<Self> {
        style.validate()?;
        let m = face.metrics(style.px)?;
        let round = |v: f32| if snap { v.round().max(1.0) } else { v };
        let (cell_w, cell_h, baseline) = (round(m.advance), round(m.line_height), round(m.baseline()));
        let pad = round(style.px);
        let bar = if style.frame.is_some() { round(style.px * 2.25) } else { 0.0 };
        let width = pad * 2.0 + cell_w * art.cols() as f32;
        let height = bar + pad * 2.0 + cell_h * art.rows() as f32 - if bar > 0.0 { pad * 0.5 } else { 0.0 };
        let origin = (pad, if bar > 0.0 { bar + pad * 0.5 } else { pad });

        let frame = style.frame.as_ref().map(|f| {
            let dot_radius = style.px * 0.4;
            let dot = |i: usize| (pad + dot_radius + i as f32 * dot_radius * 3.2, bar / 2.0, DOTS[i]);
            let title_px = style.px * 0.85;
            let tm = face.metrics(title_px).unwrap_or(m);
            FrameLayout {
                radius: style.px * 0.6,
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
        Ok(Self { width: round(width), height: round(height), origin, cell_w, cell_h, baseline, frame })
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
