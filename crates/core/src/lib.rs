//! Image → ASCII conversion core.
//!
//! Pipeline: decode ([`SourceImage`]) → tone map on perceptual lightness →
//! exact area resampling in linear light → match each cell (by tone, or by
//! tone pattern) against glyphs measured from the real font ([`GlyphAtlas`])
//! → [`AsciiArt`]. [`quality`] scores the result against the source.
//!
//! The crate does no I/O: everything takes bytes or in-memory data, so the
//! same code runs in the CLI and (compiled to WebAssembly) in the browser.

mod art;
mod atlas;
pub mod charset;
mod color;
mod error;
#[cfg(feature = "embedded-font")]
pub mod fonts;
mod grid;
mod matcher;
mod par;
mod preprocess;
pub mod quality;
pub mod render;
pub mod reveal;
mod source;

pub use art::AsciiArt;
pub use atlas::{Glyph, GlyphAtlas, MEASURE_PX, SHAPE_DIMS, SHAPE_X, SHAPE_Y};
pub use charset::Preset;
pub use error::{Error, Result};
pub use reveal::RevealTimeline;
pub use source::{MAX_WORKING_SIDE, SourceImage};

pub const MAX_COLS: usize = 2000;
pub const MAX_ROWS: usize = 4000;

/// What the art will be displayed on. Decides whether ink means light or dark.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Background {
    /// Light characters on a dark background (terminals, dark-mode pages).
    #[default]
    Dark,
    /// Dark characters on a light background (paper, light-mode pages).
    Light,
}

/// How cell tone maps to glyph density.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToneMapping {
    /// Density ∝ perceived lightness (L*). Matches how glyphs read when they
    /// are individually visible — the normal way ASCII art is looked at.
    #[default]
    Perceptual,
    /// Density ∝ emitted light. Physically exact once the glyphs blur
    /// together (very small fonts, viewing from far away).
    Linear,
}

/// How a glyph is chosen for each cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// By average tone only: smooth, classic look.
    Density,
    /// By tone pattern within the cell: edges and fine lines follow the
    /// glyph strokes (`/`, `|`, `_`, …). Needs a font-measured atlas.
    #[default]
    Shape,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    /// Output width in characters.
    pub cols: usize,
    pub mode: Mode,
    /// Shape mode: weight of structure vs. tone, 0..=1 (0 = density matching).
    pub shape_weight: f32,
    /// Error-diffusion dithering strength, 0..=1. Recovers tones between
    /// glyph levels at the cost of some texture.
    pub dither: f32,
    /// Cell width / height. `None` uses the atlas font's metrics.
    pub cell_aspect: Option<f32>,
    pub background: Background,
    pub tone_mapping: ToneMapping,
    /// Stretch tones so the `levels_low`..`levels_high` percentiles span the full range.
    pub auto_levels: bool,
    pub levels_low: f32,
    pub levels_high: f32,
    /// Local contrast (CLAHE) blend, 0 = off, 1 = full.
    pub clahe: f32,
    /// CLAHE clip limit: higher = stronger local contrast, more noise.
    pub clahe_clip: f32,
    /// CLAHE tiles along the image's long side.
    pub clahe_tiles: usize,
    /// Contrast around mid-grey, 1 = unchanged.
    pub contrast: f32,
    /// Added to lightness, 0 = unchanged.
    pub brightness: f32,
    /// > 1 brightens mid-tones, < 1 darkens them.
    pub gamma: f32,
    /// Unsharp-mask amount on the sampling grid, 0 = off.
    pub sharpen: f32,
}

impl Options {
    pub const DEFAULT: Self = Self {
        cols: 120,
        mode: Mode::Shape,
        shape_weight: 0.5,
        dither: 0.0,
        cell_aspect: None,
        background: Background::Dark,
        tone_mapping: ToneMapping::Perceptual,
        auto_levels: true,
        levels_low: 0.5,
        levels_high: 99.5,
        clahe: 0.4,
        clahe_clip: 2.5,
        clahe_tiles: 8,
        contrast: 1.0,
        brightness: 0.0,
        gamma: 1.0,
        sharpen: 0.0,
    };

    pub fn validate(&self) -> Result<()> {
        let bad = |msg: String| Err(Error::InvalidOption(msg));
        if !(1..=MAX_COLS).contains(&self.cols) {
            return bad(format!("cols must be 1..={MAX_COLS}, got {}", self.cols));
        }
        if let Some(a) = self.cell_aspect {
            if !(a.is_finite() && a > 0.05 && a < 20.0) {
                return bad(format!("cell aspect {a} is out of range"));
            }
        }
        if !(0.0..=100.0).contains(&self.levels_low)
            || !(0.0..=100.0).contains(&self.levels_high)
            || self.levels_low >= self.levels_high
        {
            return bad(format!(
                "levels must satisfy 0 <= low < high <= 100, got {}..{}",
                self.levels_low, self.levels_high
            ));
        }
        if !(0.0..=1.0).contains(&self.clahe) {
            return bad(format!("clahe must be 0..=1, got {}", self.clahe));
        }
        if !(self.clahe_clip.is_finite() && self.clahe_clip >= 1.0) {
            return bad(format!("clahe clip must be >= 1, got {}", self.clahe_clip));
        }
        if !(1..=64).contains(&self.clahe_tiles) {
            return bad(format!("clahe tiles must be 1..=64, got {}", self.clahe_tiles));
        }
        if !(self.contrast.is_finite() && (0.0..=10.0).contains(&self.contrast)) {
            return bad(format!("contrast must be 0..=10, got {}", self.contrast));
        }
        if !(-1.0..=1.0).contains(&self.brightness) {
            return bad(format!("brightness must be -1..=1, got {}", self.brightness));
        }
        if !(self.gamma.is_finite() && (0.05..=20.0).contains(&self.gamma)) {
            return bad(format!("gamma must be 0.05..=20, got {}", self.gamma));
        }
        if !(0.0..=10.0).contains(&self.sharpen) {
            return bad(format!("sharpen must be 0..=10, got {}", self.sharpen));
        }
        if !(0.0..=1.0).contains(&self.shape_weight) {
            return bad(format!("shape weight must be 0..=1, got {}", self.shape_weight));
        }
        if !(0.0..=1.0).contains(&self.dither) {
            return bad(format!("dither must be 0..=1, got {}", self.dither));
        }
        Ok(())
    }

    /// Character grid size this image will get, in (cols, rows).
    pub fn grid_size(&self, src: &SourceImage, atlas: &GlyphAtlas) -> Result<(usize, usize)> {
        self.grid_for(src.width(), src.height(), atlas)
    }

    fn grid_for(&self, width: usize, height: usize, atlas: &GlyphAtlas) -> Result<(usize, usize)> {
        self.validate()?;
        let aspect = self.cell_aspect.unwrap_or(atlas.cell_aspect());
        let cols = self.cols;
        let rows = grid::rows_for(width, height, cols, aspect);
        if rows > MAX_ROWS {
            return Err(Error::InvalidOption(format!(
                "{cols} columns would need {rows} rows (max {MAX_ROWS}); use fewer columns"
            )));
        }
        Ok((cols, rows))
    }
}

impl Default for Options {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// The tone-mapped image (the expensive, full-resolution part of a
/// conversion). Interactive callers keep one and reuse it while only grid or
/// matching options change — width, mode, charset, dither, sharpening and
/// tone mapping all apply after this stage.
pub struct Prepared {
    width: usize,
    height: usize,
    ink: Vec<f32>,
    key: ToneKey,
}

/// The options `preprocess::ink_plane` depends on.
#[derive(Debug, Clone, PartialEq)]
struct ToneKey {
    background: Background,
    auto_levels: bool,
    levels: (f32, f32),
    clahe: (f32, f32, usize),
    contrast: f32,
    brightness: f32,
    gamma: f32,
}

impl ToneKey {
    fn of(o: &Options) -> Self {
        Self {
            background: o.background,
            auto_levels: o.auto_levels,
            levels: (o.levels_low, o.levels_high),
            clahe: (o.clahe, o.clahe_clip, o.clahe_tiles),
            contrast: o.contrast,
            brightness: o.brightness,
            gamma: o.gamma,
        }
    }
}

impl Prepared {
    pub fn new(src: &SourceImage, opts: &Options) -> Result<Self> {
        opts.validate()?;
        Ok(Self { width: src.width(), height: src.height(), ink: preprocess::ink_plane(src, opts), key: ToneKey::of(opts) })
    }

    /// Whether this was prepared with the same tone options as `opts`.
    pub fn is_for(&self, opts: &Options) -> bool {
        self.key == ToneKey::of(opts)
    }
}

pub fn convert(src: &SourceImage, atlas: &GlyphAtlas, opts: &Options) -> Result<AsciiArt> {
    convert_prepared(&Prepared::new(src, opts)?, atlas, opts)
}

/// [`convert`] from an already tone-mapped image; `opts` must match the tone
/// options it was prepared with ([`Prepared::is_for`]).
pub fn convert_prepared(prepared: &Prepared, atlas: &GlyphAtlas, opts: &Options) -> Result<AsciiArt> {
    if !prepared.is_for(opts) {
        return Err(Error::InvalidOption("image was prepared with different tone options".into()));
    }
    let (cols, rows) = opts.grid_for(prepared.width, prepared.height, atlas)?;
    let ink = &prepared.ink;
    let (sx, sy) = match opts.mode {
        Mode::Density => (1, 1),
        Mode::Shape => (SHAPE_X, SHAPE_Y),
    };
    // Averaging is always in linear light; only the averaged samples move to
    // the tone space glyph levels are compared in.
    let (gw, gh) = (cols * sx, rows * sy);
    let mut samples = grid::area_average(ink, prepared.width, prepared.height, gw, gh);
    preprocess::to_tone_space(&mut samples, opts);
    if opts.sharpen > 0.0 {
        grid::sharpen(&mut samples, gw, gh, opts.sharpen);
    }

    let chars = match opts.mode {
        Mode::Density => matcher::density(&samples, cols, rows, atlas, opts.dither),
        Mode::Shape => {
            let per_cell = grid::cell_major(&samples, cols, rows, sx, sy);
            matcher::shape(&per_cell, cols, rows, atlas, opts.shape_weight, opts.dither)
        }
    };
    Ok(AsciiArt::new(cols, rows, chars))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn atlas() -> GlyphAtlas {
        GlyphAtlas::from_coverages(&[(' ', 0.0), ('.', 0.1), (':', 0.2), ('+', 0.4), ('#', 0.7), ('@', 0.9)], 0.5)
            .unwrap()
    }

    fn flat(v: u8) -> SourceImage {
        SourceImage::from_rgba8(4, 4, &[v, v, v, 255].repeat(16)).unwrap()
    }

    #[test]
    fn black_and_white_hit_the_ends_of_the_ramp() {
        let opts = Options { cols: 4, ..Options::DEFAULT };
        let black = convert(&flat(0), &atlas(), &opts).unwrap();
        assert!(black.cells().iter().all(|&c| c == ' '));
        let white = convert(&flat(255), &atlas(), &opts).unwrap();
        assert!(white.cells().iter().all(|&c| c == '@'));
        let light_bg = Options { background: Background::Light, ..opts };
        let on_paper = convert(&flat(255), &atlas(), &light_bg).unwrap();
        assert!(on_paper.cells().iter().all(|&c| c == ' '));
    }

    #[test]
    fn gradient_gives_monotonic_density() {
        let w = 256;
        let rgba: Vec<u8> = (0..w).flat_map(|x| [x as u8, x as u8, x as u8, 255]).collect();
        let src = SourceImage::from_rgba8(w, 1, &rgba).unwrap();
        let opts = Options { cols: 32, cell_aspect: Some(32.0 / 256.0 * 8.0), ..Options::DEFAULT };
        let art = convert(&src, &atlas(), &opts).unwrap();
        let levels: Vec<f32> = art.row(0).iter().map(|&c| atlas().glyph(c).unwrap().level).collect();
        assert!(levels.windows(2).all(|p| p[1] >= p[0]), "{:?}", art.to_text());
        assert_eq!(art.row(0)[0], ' ');
        assert_eq!(art.row(0)[31], '@');
    }

    #[test]
    fn grid_respects_cell_aspect() {
        let src = SourceImage::from_rgba8(100, 50, &[0; 100 * 50 * 4]).unwrap();
        let art = convert(&src, &atlas(), &Options { cols: 40, ..Options::DEFAULT }).unwrap();
        assert_eq!((art.cols(), art.rows()), (40, 10));
    }

    #[test]
    fn invalid_options_are_rejected() {
        let src = flat(128);
        for opts in [
            Options { cols: 0, ..Options::DEFAULT },
            Options { gamma: 0.0, ..Options::DEFAULT },
            Options { levels_low: 60.0, levels_high: 40.0, ..Options::DEFAULT },
            Options { contrast: f32::NAN, ..Options::DEFAULT },
            Options { cell_aspect: Some(-1.0), ..Options::DEFAULT },
        ] {
            assert!(convert(&src, &atlas(), &opts).is_err(), "{opts:?}");
        }
    }

    #[test]
    fn prepared_images_are_reusable_across_grid_options() {
        let rgba: Vec<u8> = (0..64 * 48).flat_map(|i| [(i * 37 % 251) as u8, (i % 256) as u8, 90, 255]).collect();
        let src = SourceImage::from_rgba8(64, 48, &rgba).unwrap();
        let base = Options { cols: 30, ..Options::DEFAULT };
        let prepared = Prepared::new(&src, &base).unwrap();
        for opts in [
            base.clone(),
            Options { cols: 17, mode: Mode::Density, dither: 0.5, sharpen: 1.0, ..base.clone() },
            Options { tone_mapping: ToneMapping::Linear, shape_weight: 0.2, ..base.clone() },
        ] {
            assert!(prepared.is_for(&opts));
            assert_eq!(convert_prepared(&prepared, &atlas(), &opts).unwrap(), convert(&src, &atlas(), &opts).unwrap());
        }
        let retoned = Options { contrast: 1.5, ..base };
        assert!(!prepared.is_for(&retoned));
        assert!(convert_prepared(&prepared, &atlas(), &retoned).is_err());
    }

    #[test]
    fn output_is_deterministic() {
        let rgba: Vec<u8> = (0..64 * 48).flat_map(|i| [(i * 37 % 251) as u8, (i % 256) as u8, 90, 255]).collect();
        let src = SourceImage::from_rgba8(64, 48, &rgba).unwrap();
        let opts = Options { cols: 30, sharpen: 0.5, ..Options::DEFAULT };
        assert_eq!(convert(&src, &atlas(), &opts).unwrap(), convert(&src, &atlas(), &opts).unwrap());
    }
}
