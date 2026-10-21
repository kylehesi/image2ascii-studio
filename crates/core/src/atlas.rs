//! Glyph calibration: rasterises every character of the charset in the target
//! font and measures how much of its cell it actually inks. Hand-written
//! "density ramps" are only right for the font they were tuned on; measuring
//! is right for every font.

use fontdue::{Font, FontSettings};

use crate::charset;
use crate::error::{Error, Result};
use crate::grid;

/// Rasterisation size used for measurement. Large enough that pixel snapping
/// doesn't distort coverage ratios between thin glyphs.
pub const MEASURE_PX: f32 = 64.0;
const LUT_SIZE: usize = 4096;

/// Sub-cell grid that shape matching compares on: square-ish samples for a
/// typical 1:2 cell, fine enough to tell `/` from `\` and `[` from `]`.
pub const SHAPE_X: usize = 4;
pub const SHAPE_Y: usize = 8;
pub const SHAPE_DIMS: usize = SHAPE_X * SHAPE_Y;

/// Character cell geometry of a monospace font at a pixel size. Shared by
/// calibration and rendering so exported images use exactly the cells the
/// glyphs were measured in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CellMetrics {
    pub advance: f32,
    pub line_height: f32,
    pub ascent: f32,
    /// Negative: below the baseline.
    pub descent: f32,
}

impl CellMetrics {
    pub fn of(font: &Font, px: f32) -> Result<Self> {
        let line = font
            .horizontal_line_metrics(px)
            .ok_or_else(|| Error::Font("font has no horizontal metrics".into()))?;
        let metrics = Self {
            advance: font.metrics('M', px).advance_width,
            line_height: line.new_line_size,
            ascent: line.ascent,
            descent: line.descent,
        };
        if !(metrics.advance > 0.0 && metrics.line_height > 0.0) {
            return Err(Error::Font("font reports a zero-sized cell".into()));
        }
        Ok(metrics)
    }

    /// Baseline offset from the top of the cell: the ascent..descent box is
    /// centred vertically and any line gap splits evenly.
    pub fn baseline(&self) -> f32 {
        (self.line_height - (self.ascent - self.descent)) / 2.0 + self.ascent
    }
}

#[derive(Debug, Clone)]
pub struct Glyph {
    pub ch: char,
    /// Fraction of the cell covered by ink, as measured.
    pub coverage: f32,
    /// Coverage normalised so the charset spans [0, 1].
    pub level: f32,
    /// Cell-sized 8-bit coverage bitmap (empty for synthetic atlases).
    pub bitmap: Vec<u8>,
    /// Ink on the `SHAPE_X`×`SHAPE_Y` grid, row-major, on the same
    /// normalised scale as `level` (so its mean is `level`). Flat for
    /// atlases without bitmaps.
    pub shape: Vec<f32>,
}

#[derive(Debug, Clone)]
pub struct GlyphAtlas {
    /// Sorted by ascending `level`.
    glyphs: Vec<Glyph>,
    cell_width: usize,
    cell_height: usize,
    cell_aspect: f32,
    min_coverage: f32,
    coverage_range: f32,
    /// Quantised level → index of the nearest glyph.
    lut: Vec<u16>,
}

impl GlyphAtlas {
    pub fn new(font_data: &[u8], charset: &str) -> Result<Self> {
        Self::with_px(font_data, charset, MEASURE_PX)
    }

    /// Like [`GlyphAtlas::new`] at a custom pixel size — useful when the
    /// bitmaps are wanted for rendering rather than only for measurement.
    pub fn with_px(font_data: &[u8], charset: &str, px: f32) -> Result<Self> {
        if !(px.is_finite() && px >= 4.0) {
            return Err(Error::InvalidOption(format!("glyph size {px} is too small")));
        }
        let chars = charset::parse(charset)?;
        let font = Font::from_bytes(font_data, FontSettings::default())
            .map_err(|e| Error::Font(e.to_string()))?;
        let metrics = CellMetrics::of(&font, px)?;
        let cell_w = metrics.advance.ceil() as usize;
        let cell_h = metrics.line_height.ceil() as usize;
        let baseline = metrics.baseline().round() as i32;

        let mut cells = Vec::with_capacity(chars.len());
        for ch in chars {
            if ch != ' ' && font.lookup_glyph_index(ch) == 0 {
                return Err(Error::Charset(format!("font has no glyph for {ch:?}")));
            }
            let (m, raster) = font.rasterize(ch, px);
            let mut bitmap = vec![0u8; cell_w * cell_h];
            let top = baseline - (m.ymin + m.height as i32);
            for gy in 0..m.height {
                let cy = top + gy as i32;
                if !(0..cell_h as i32).contains(&cy) {
                    continue;
                }
                for gx in 0..m.width {
                    let cx = m.xmin + gx as i32;
                    if !(0..cell_w as i32).contains(&cx) {
                        continue;
                    }
                    let dst = &mut bitmap[cy as usize * cell_w + cx as usize];
                    *dst = dst.saturating_add(raster[gy * m.width + gx]);
                }
            }
            cells.push((ch, bitmap));
        }
        Self::from_cells(cells, cell_w, cell_h, metrics.advance / metrics.line_height)
    }

    /// An atlas from caller-drawn glyphs: `cell_width`×`cell_height` 8-bit
    /// coverage bitmaps, row-major. `cell_aspect` defaults to the bitmap's.
    pub fn from_bitmaps(
        cell_width: usize,
        cell_height: usize,
        glyphs: &[(char, &[u8])],
        cell_aspect: Option<f32>,
    ) -> Result<Self> {
        if cell_width == 0 || cell_height == 0 {
            return Err(Error::InvalidOption("glyph cells must not be empty".into()));
        }
        let aspect = cell_aspect.unwrap_or(cell_width as f32 / cell_height as f32);
        if !(aspect.is_finite() && aspect > 0.0) {
            return Err(Error::InvalidOption(format!("cell aspect {aspect} must be positive")));
        }
        let chars = charset::parse(&glyphs.iter().map(|g| g.0).collect::<String>())?;
        let mut cells = Vec::with_capacity(chars.len());
        for ch in chars {
            let bitmap = glyphs.iter().find(|g| g.0 == ch).map_or(&[][..], |g| g.1);
            if bitmap.len() != cell_width * cell_height {
                return Err(Error::InvalidOption(format!(
                    "bitmap for {ch:?} has {} bytes, expected {cell_width}×{cell_height}",
                    bitmap.len()
                )));
            }
            cells.push((ch, bitmap.to_vec()));
        }
        Self::from_cells(cells, cell_width, cell_height, aspect)
    }

    fn from_cells(cells: Vec<(char, Vec<u8>)>, cell_w: usize, cell_h: usize, aspect: f32) -> Result<Self> {
        let glyphs = cells
            .into_iter()
            .map(|(ch, bitmap)| {
                let ink: u64 = bitmap.iter().map(|&v| v as u64).sum();
                let coverage = ink as f32 / (255.0 * (cell_w * cell_h) as f32);
                Glyph { ch, coverage, level: 0.0, bitmap, shape: Vec::new() }
            })
            .collect();
        Self::finish(glyphs, cell_w, cell_h, aspect)
    }

    /// An atlas from known coverages, without a font — for tests and for
    /// callers that calibrate glyphs some other way. Shape matching treats
    /// these glyphs as flat, i.e. it degrades to density matching.
    pub fn from_coverages(entries: &[(char, f32)], cell_aspect: f32) -> Result<Self> {
        if !(cell_aspect.is_finite() && cell_aspect > 0.0) {
            return Err(Error::InvalidOption(format!("cell aspect {cell_aspect} must be positive")));
        }
        let charset: String = entries.iter().map(|e| e.0).collect();
        let chars = charset::parse(&charset)?;
        let glyphs = chars
            .into_iter()
            .map(|ch| {
                let coverage = entries.iter().find(|e| e.0 == ch).map_or(0.0, |e| e.1);
                Glyph { ch, coverage, level: 0.0, bitmap: Vec::new(), shape: Vec::new() }
            })
            .collect();
        Self::finish(glyphs, 0, 0, cell_aspect)
    }

    fn finish(mut glyphs: Vec<Glyph>, cell_width: usize, cell_height: usize, cell_aspect: f32) -> Result<Self> {
        if !glyphs.iter().all(|g| g.coverage.is_finite()) {
            return Err(Error::Charset("glyph coverages must be finite".into()));
        }
        glyphs.sort_by(|a, b| a.coverage.total_cmp(&b.coverage));
        let min_coverage = glyphs[0].coverage;
        let coverage_range = glyphs[glyphs.len() - 1].coverage - min_coverage;
        if coverage_range <= 1e-4 {
            return Err(Error::Charset("all characters have the same density in this font".into()));
        }
        let mut atlas = Self {
            glyphs: Vec::new(),
            cell_width,
            cell_height,
            cell_aspect,
            min_coverage,
            coverage_range,
            lut: Vec::new(),
        };
        for g in &mut glyphs {
            g.level = (g.coverage - min_coverage) / coverage_range;
            g.shape = atlas.sample(g, SHAPE_X, SHAPE_Y);
        }

        let lut = (0..LUT_SIZE)
            .map(|i| {
                let t = (i as f32 + 0.5) / LUT_SIZE as f32;
                let above = glyphs.partition_point(|g| g.level < t).min(glyphs.len() - 1);
                let nearest = match above.checked_sub(1) {
                    Some(below) if t - glyphs[below].level <= glyphs[above].level - t => below,
                    _ => above,
                };
                nearest as u16
            })
            .collect();
        atlas.glyphs = glyphs;
        atlas.lut = lut;
        Ok(atlas)
    }

    /// The glyph's ink area-averaged onto an `sx`×`sy` grid, on the
    /// normalised `level` scale. Flat when the atlas has no bitmaps.
    pub(crate) fn sample(&self, g: &Glyph, sx: usize, sy: usize) -> Vec<f32> {
        if g.bitmap.is_empty() {
            return vec![g.level; sx * sy];
        }
        let ink: Vec<f32> = g.bitmap.iter().map(|&v| v as f32 / 255.0).collect();
        let mut grid = grid::area_average(&ink, self.cell_width, self.cell_height, sx, sy);
        for v in &mut grid {
            *v = (*v - self.min_coverage) / self.coverage_range;
        }
        grid
    }

    /// Glyphs sorted from least to most ink.
    pub fn glyphs(&self) -> &[Glyph] {
        &self.glyphs
    }

    pub fn glyph(&self, ch: char) -> Option<&Glyph> {
        self.glyphs.iter().find(|g| g.ch == ch)
    }

    /// Cell width / height of the measured font.
    pub fn cell_aspect(&self) -> f32 {
        self.cell_aspect
    }

    /// Bitmap cell size in pixels (zero for synthetic atlases).
    pub fn cell_size(&self) -> (usize, usize) {
        (self.cell_width, self.cell_height)
    }

    /// The glyph whose normalised coverage is closest to `level`.
    pub fn nearest(&self, level: f32) -> &Glyph {
        let i = ((level.max(0.0) * LUT_SIZE as f32) as usize).min(LUT_SIZE - 1);
        &self.glyphs[self.lut[i] as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn atlas() -> GlyphAtlas {
        GlyphAtlas::from_coverages(&[('@', 0.6), (' ', 0.0), ('.', 0.05), ('+', 0.3)], 0.5).unwrap()
    }

    #[test]
    fn sorts_and_normalises() {
        let a = atlas();
        let order: String = a.glyphs().iter().map(|g| g.ch).collect();
        assert_eq!(order, " .+@");
        assert_eq!(a.glyphs()[0].level, 0.0);
        assert_eq!(a.glyphs()[3].level, 1.0);
    }

    #[test]
    fn nearest_picks_closest_level() {
        let a = atlas();
        assert_eq!(a.nearest(0.0).ch, ' ');
        assert_eq!(a.nearest(0.1).ch, '.');
        assert_eq!(a.nearest(0.45).ch, '+');
        assert_eq!(a.nearest(0.9).ch, '@');
        assert_eq!(a.nearest(1.5).ch, '@');
        assert_eq!(a.nearest(-1.0).ch, ' ');
    }

    #[test]
    fn rejects_flat_charsets() {
        assert!(GlyphAtlas::from_coverages(&[('a', 0.2), ('b', 0.2)], 0.5).is_err());
    }

    /// 4×8 cells: '[' inks the left half, ']' the right half, '#' everything.
    fn halves() -> GlyphAtlas {
        let cell = |f: fn(usize) -> bool| (0..32).map(|i| if f(i % 4) { 255 } else { 0 }).collect::<Vec<u8>>();
        let (empty, left, right, full) = (cell(|_| false), cell(|x| x < 2), cell(|x| x >= 2), cell(|_| true));
        GlyphAtlas::from_bitmaps(4, 8, &[(' ', &empty), ('[', &left), (']', &right), ('#', &full)], None).unwrap()
    }

    #[test]
    fn shapes_capture_where_the_ink_is() {
        let a = halves();
        let left = a.glyph('[').unwrap();
        assert_eq!(left.level, 0.5);
        assert_eq!(left.shape.len(), SHAPE_DIMS);
        let mean = left.shape.iter().sum::<f32>() / SHAPE_DIMS as f32;
        assert!((mean - left.level).abs() < 1e-6, "shape mean must equal level");
        // Row 0 of the 4×8 shape grid: left two samples inked, right two empty.
        assert_eq!(&left.shape[..4], &[1.0, 1.0, 0.0, 0.0]);
        assert_eq!(a.cell_aspect(), 0.5);
    }

    #[test]
    fn from_bitmaps_validates_sizes() {
        assert!(GlyphAtlas::from_bitmaps(4, 8, &[(' ', &[0; 32]), ('#', &[255; 31])], None).is_err());
        assert!(GlyphAtlas::from_bitmaps(0, 8, &[], None).is_err());
    }

    #[test]
    fn coverage_atlases_have_flat_shapes() {
        let g = atlas().glyph('+').unwrap().clone();
        assert!(g.shape.iter().all(|&v| v == g.level));
    }

    #[test]
    fn rejects_non_fonts() {
        assert!(GlyphAtlas::new(b"not a font", " .#").is_err());
    }
}
