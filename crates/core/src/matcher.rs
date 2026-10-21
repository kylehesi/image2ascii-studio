//! Choosing a glyph per cell.
//!
//! Density matching compares one number per cell (its tone) with each glyph's
//! level. Shape matching compares the cell's tone *pattern* on the
//! `SHAPE_X`×`SHAPE_Y` grid with each glyph's ink pattern, so edges, strands
//! of hair and outlines pick glyphs whose strokes run the same way.
//!
//! Shape cost for a cell with samples `t` (mean μt, standard deviation σt)
//! and a glyph with samples `g` (mean μg = level):
//!
//! ```text
//! cost = (1 − w)·(μt − μg)²  +  w·2σt²·(1 − corr(t, g))
//! ```
//!
//! The structure term is the squared error between the cell's pattern and
//! the glyph's pattern rescaled to the cell's own contrast. Comparing raw
//! patterns instead would be wrong: glyphs are near-binary strokes while
//! photos are smooth, so plain MSE penalises every inky glyph and drags the
//! whole image darker. Here a flat cell (σt ≈ 0) is matched on tone alone and
//! an edge cell prefers glyphs whose strokes line up with it.

use crate::atlas::{GlyphAtlas, SHAPE_DIMS};
use crate::par;

/// Density matching: nearest glyph level per cell.
pub(crate) fn density(levels: &[f32], cols: usize, rows: usize, atlas: &GlyphAtlas, dither: f32) -> Vec<char> {
    if dither > 0.0 {
        return diffuse(cols, rows, dither, |i, bias| {
            let t = levels[i] + bias;
            let g = atlas.nearest(t);
            (g.ch, t - g.level)
        });
    }
    par::map(levels.len(), |i| atlas.nearest(levels[i]).ch)
}

/// Shape matching. `samples` holds `SHAPE_DIMS` tone values per cell, cells
/// in row-major order and each cell's samples row-major within it. `weight`
/// is `w` above, 0..=1.
pub(crate) fn shape(
    samples: &[f32],
    cols: usize,
    rows: usize,
    atlas: &GlyphAtlas,
    weight: f32,
    dither: f32,
) -> Vec<char> {
    let table = ShapeTable::new(atlas, weight);
    let cell = |i: usize| &samples[i * SHAPE_DIMS..(i + 1) * SHAPE_DIMS];
    if dither > 0.0 {
        return diffuse(cols, rows, dither, |i, bias| {
            let (g, mean) = table.best(cell(i), bias);
            (atlas.glyphs()[g].ch, mean - atlas.glyphs()[g].level)
        });
    }
    par::map(cols * rows, |i| atlas.glyphs()[table.best(cell(i), 0.0).0].ch)
}

/// Below this pattern norm a cell or glyph counts as flat (no orientation).
const FLAT: f32 = 1e-6;

struct ShapeTable {
    levels: Vec<f32>,
    /// Per glyph: `(g − μg) / ‖g − μg‖`, or zeros for flat glyphs (space).
    unit: Vec<[f32; SHAPE_DIMS]>,
    tone_weight: f32,
    structure_weight: f32,
}

impl ShapeTable {
    fn new(atlas: &GlyphAtlas, weight: f32) -> Self {
        let glyphs = atlas.glyphs();
        let unit = glyphs
            .iter()
            .map(|g| {
                let centred: [f32; SHAPE_DIMS] = std::array::from_fn(|i| g.shape[i] - g.level);
                let norm = centred.iter().map(|v| v * v).sum::<f32>().sqrt();
                if norm > FLAT { centred.map(|v| v / norm) } else { [0.0; SHAPE_DIMS] }
            })
            .collect();
        Self {
            levels: glyphs.iter().map(|g| g.level).collect(),
            unit,
            tone_weight: 1.0 - weight,
            structure_weight: weight,
        }
    }

    /// Best glyph index for one cell whose tone is shifted by `bias`, and the
    /// shifted mean tone it was matched against.
    fn best(&self, t: &[f32], bias: f32) -> (usize, f32) {
        let raw_mean = t.iter().sum::<f32>() / SHAPE_DIMS as f32;
        let mean = raw_mean + bias;
        let dt: [f32; SHAPE_DIMS] = std::array::from_fn(|i| t[i] - raw_mean);
        let norm2 = dt.iter().map(|v| v * v).sum::<f32>();
        let norm = norm2.sqrt();
        // 2σt² = 2‖dt‖²/D; correlation = dt·unit_g / ‖dt‖.
        let contrast = 2.0 * norm2 / SHAPE_DIMS as f32;
        let mut best = (0, f32::INFINITY);
        for (g, unit) in self.unit.iter().enumerate() {
            let tone = mean - self.levels[g];
            let corr = if norm > FLAT { dt.iter().zip(unit).map(|(a, b)| a * b).sum::<f32>() / norm } else { 0.0 };
            let cost = self.tone_weight * tone * tone + self.structure_weight * contrast * (1.0 - corr);
            if cost < best.1 {
                best = (g, cost);
            }
        }
        (best.0, mean)
    }
}

/// Floyd–Steinberg error diffusion over the cell grid in serpentine order.
/// `pick(cell, bias)` chooses a glyph for a cell whose tone is shifted by
/// `bias` and returns it with the tone error left over; `strength` scales how
/// much of that error reaches the neighbours. Inherently sequential.
fn diffuse(cols: usize, rows: usize, strength: f32, mut pick: impl FnMut(usize, f32) -> (char, f32)) -> Vec<char> {
    let mut err = vec![0f32; cols * rows];
    let mut out = vec![' '; cols * rows];
    for y in 0..rows {
        let forward = y % 2 == 0;
        for k in 0..cols {
            let x = if forward { k } else { cols - 1 - k };
            let i = y * cols + x;
            let (ch, e) = pick(i, err[i]);
            out[i] = ch;
            let e = e * strength;
            let mut spread = |dx: isize, dy: usize, weight: f32| {
                let nx = x as isize + if forward { dx } else { -dx };
                if (0..cols as isize).contains(&nx) && y + dy < rows {
                    err[(y + dy) * cols + nx as usize] += e * weight;
                }
            };
            spread(1, 0, 7.0 / 16.0);
            spread(-1, 1, 3.0 / 16.0);
            spread(0, 1, 5.0 / 16.0);
            spread(1, 1, 1.0 / 16.0);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atlas::{SHAPE_X, SHAPE_Y};

    /// 4×8 cells: ' ' empty, '[' left half, ']' right half, '-' top half,
    /// '_' bottom half, '#' full.
    fn atlas() -> GlyphAtlas {
        let cell = |f: fn(usize, usize) -> bool| {
            (0..32).map(|i| if f(i % 4, i / 4) { 255 } else { 0 }).collect::<Vec<u8>>()
        };
        let glyphs = [
            (' ', cell(|_, _| false)),
            ('[', cell(|x, _| x < 2)),
            (']', cell(|x, _| x >= 2)),
            ('-', cell(|_, y| y < 4)),
            ('_', cell(|_, y| y >= 4)),
            ('#', cell(|_, _| true)),
        ];
        let refs: Vec<(char, &[u8])> = glyphs.iter().map(|(c, b)| (*c, b.as_slice())).collect();
        GlyphAtlas::from_bitmaps(4, 8, &refs, None).unwrap()
    }

    fn cell(f: impl Fn(usize, usize) -> f32) -> Vec<f32> {
        (0..SHAPE_DIMS).map(|i| f(i % SHAPE_X, i / SHAPE_X)).collect()
    }

    const PLAIN: f32 = 0.5;

    #[test]
    fn shape_finds_edge_orientation() {
        let a = atlas();
        let left = cell(|x, _| if x < 2 { 1.0 } else { 0.0 });
        let bottom = cell(|_, y| if y >= SHAPE_Y / 2 { 1.0 } else { 0.0 });
        let samples = [left, bottom].concat();
        assert_eq!(shape(&samples, 2, 1, &a, PLAIN, 0.0), vec!['[', '_']);
        // Density alone can only say "half-inked" and picks the same glyph for both.
        let d = density(&[0.5, 0.5], 2, 1, &a, 0.0);
        assert_eq!(d[0], d[1]);
    }

    #[test]
    fn zero_weight_is_density_matching() {
        let a = atlas();
        let samples: Vec<f32> = (0..20).flat_map(|i| cell(move |x, _| (i as f32 / 19.0) * (x as f32 / 3.0) * 2.0)).collect();
        let means: Vec<f32> = samples.chunks(SHAPE_DIMS).map(|c| c.iter().sum::<f32>() / SHAPE_DIMS as f32).collect();
        let tone_only = 0.0;
        let by_shape: Vec<f32> = shape(&samples, 20, 1, &a, tone_only, 0.0)
            .iter()
            .map(|&c| a.glyph(c).unwrap().level)
            .collect();
        let by_density: Vec<f32> = density(&means, 20, 1, &a, 0.0).iter().map(|&c| a.glyph(c).unwrap().level).collect();
        assert_eq!(by_shape, by_density);
    }

    #[test]
    fn flat_cells_prefer_flat_tone() {
        let a = atlas();
        assert_eq!(shape(&cell(|_, _| 1.0), 1, 1, &a, PLAIN, 0.0), vec!['#']);
        assert_eq!(shape(&cell(|_, _| 0.0), 1, 1, &a, PLAIN, 0.0), vec![' ']);
    }

    #[test]
    fn dithering_preserves_average_tone() {
        // A flat 0.25 field can't be matched by any single glyph (levels 0, 0.5, 1);
        // diffusion must alternate so the average comes out right.
        let a = atlas();
        let (cols, rows) = (40, 20);
        let out = density(&vec![0.25; cols * rows], cols, rows, &a, 1.0);
        let mean = out.iter().map(|&c| a.glyph(c).unwrap().level).sum::<f32>() / out.len() as f32;
        assert!((mean - 0.25).abs() < 0.02, "mean {mean}");
        let plain = density(&vec![0.25; cols * rows], cols, rows, &a, 0.0);
        let plain_mean = plain.iter().map(|&c| a.glyph(c).unwrap().level).sum::<f32>() / plain.len() as f32;
        assert!((plain_mean - 0.25).abs() > 0.2, "without dithering every cell rounds the same way");
    }

    #[test]
    fn shape_dithering_preserves_average_tone() {
        let a = atlas();
        let (cols, rows) = (30, 10);
        let samples: Vec<f32> = (0..cols * rows).flat_map(|_| cell(|_, _| 0.75)).collect();
        let out = shape(&samples, cols, rows, &a, PLAIN, 1.0);
        let mean = out.iter().map(|&c| a.glyph(c).unwrap().level).sum::<f32>() / out.len() as f32;
        assert!((mean - 0.75).abs() < 0.03, "mean {mean}");
    }
}
