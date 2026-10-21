//! Quality measurement. Art is rendered back on a fine sub-cell grid, in the
//! tone space the matcher works in, and compared with the source sampled on
//! the same grid. The evaluation grid is finer than the matcher's shape grid,
//! so the score isn't just the matcher grading its own objective.
//!
//! Used by tests to guard against regressions and by tools to tune options.

use std::collections::HashMap;

use crate::error::{Error, Result};
use crate::{AsciiArt, GlyphAtlas, Options, Prepared, SourceImage, grid, par, preprocess};

/// Evaluation samples per cell.
pub const EVAL_X: usize = 8;
pub const EVAL_Y: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Score {
    /// Mean structural similarity, −1..=1 (1 = identical). Rewards edges and
    /// texture landing in the right place.
    pub ssim: f32,
    /// Mean absolute tone difference once both images are blurred to about
    /// cell scale, 0..=1 (0 = identical). Rewards getting tones right.
    pub tone_error: f32,
}

/// Scores `art` (made from `src` with `atlas` and `opts`) against the source.
pub fn evaluate(src: &SourceImage, atlas: &GlyphAtlas, opts: &Options, art: &AsciiArt) -> Result<Score> {
    evaluate_prepared(&Prepared::new(src, opts)?, atlas, opts, art)
}

/// [`evaluate`] from an already tone-mapped image.
pub fn evaluate_prepared(prepared: &Prepared, atlas: &GlyphAtlas, opts: &Options, art: &AsciiArt) -> Result<Score> {
    if atlas.cell_size().0 == 0 {
        return Err(Error::InvalidOption("quality evaluation needs an atlas with glyph bitmaps".into()));
    }
    if !prepared.is_for(opts) {
        return Err(Error::InvalidOption("image was prepared with different tone options".into()));
    }
    let (w, h) = (art.cols() * EVAL_X, art.rows() * EVAL_Y);

    let mut reference = grid::area_average(&prepared.ink, prepared.width, prepared.height, w, h);
    preprocess::to_tone_space(&mut reference, opts);

    let samples: HashMap<char, Vec<f32>> =
        atlas.glyphs().iter().map(|g| (g.ch, atlas.sample(g, EVAL_X, EVAL_Y))).collect();
    let mut rendered = vec![0f32; w * h];
    for (cell, ch) in art.cells().iter().enumerate() {
        let glyph = samples
            .get(ch)
            .ok_or_else(|| Error::Charset(format!("art uses {ch:?}, which this atlas doesn't have")))?;
        let (cx, cy) = (cell % art.cols(), cell / art.cols());
        for y in 0..EVAL_Y {
            let dst = (cy * EVAL_Y + y) * w + cx * EVAL_X;
            rendered[dst..dst + EVAL_X].copy_from_slice(&glyph[y * EVAL_X..(y + 1) * EVAL_X]);
        }
    }

    let blurred_ref = blur(&reference, w, h, EVAL_X as f32);
    let blurred_out = blur(&rendered, w, h, EVAL_X as f32);
    let tone_error =
        blurred_ref.iter().zip(&blurred_out).map(|(a, b)| (a - b).abs()).sum::<f32>() / (w * h) as f32;
    Ok(Score { ssim: ssim(&reference, &rendered, w, h), tone_error })
}

/// Mean SSIM of two w×h images with values nominally in [0, 1], using the
/// standard Gaussian window (σ = 1.5) and constants.
pub fn ssim(a: &[f32], b: &[f32], w: usize, h: usize) -> f32 {
    const C1: f32 = 0.01 * 0.01;
    const C2: f32 = 0.03 * 0.03;
    const SIGMA: f32 = 1.5;
    assert_eq!(a.len(), w * h);
    assert_eq!(b.len(), w * h);

    let product = |f: fn(f32, f32) -> f32| a.iter().zip(b).map(|(&x, &y)| f(x, y)).collect::<Vec<f32>>();
    let mu_a = blur(a, w, h, SIGMA);
    let mu_b = blur(b, w, h, SIGMA);
    let aa = blur(&product(|x, _| x * x), w, h, SIGMA);
    let bb = blur(&product(|_, y| y * y), w, h, SIGMA);
    let ab = blur(&product(|x, y| x * y), w, h, SIGMA);

    let total: f64 = (0..w * h)
        .map(|i| {
            let (ma, mb) = (mu_a[i], mu_b[i]);
            let var_a = aa[i] - ma * ma;
            let var_b = bb[i] - mb * mb;
            let cov = ab[i] - ma * mb;
            let s = ((2.0 * ma * mb + C1) * (2.0 * cov + C2)) / ((ma * ma + mb * mb + C1) * (var_a + var_b + C2));
            s as f64
        })
        .sum();
    (total / (w * h) as f64) as f32
}

/// Separable Gaussian blur, edges clamped.
fn blur(src: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    let radius = (3.0 * sigma).ceil() as isize;
    let kernel: Vec<f32> = (-radius..=radius).map(|d| (-((d * d) as f32) / (2.0 * sigma * sigma)).exp()).collect();
    let norm: f32 = kernel.iter().sum();
    let kernel: Vec<f32> = kernel.iter().map(|k| k / norm).collect();
    let clamp = |v: isize, n: usize| v.clamp(0, n as isize - 1) as usize;

    let mut horizontal = vec![0f32; w * h];
    par::for_each_chunk(&mut horizontal, w, |y, row| {
        let line = &src[y * w..(y + 1) * w];
        for (x, out) in row.iter_mut().enumerate() {
            *out = kernel.iter().enumerate().map(|(k, kv)| kv * line[clamp(x as isize + k as isize - radius, w)]).sum();
        }
    });
    let mut out = vec![0f32; w * h];
    par::for_each_chunk(&mut out, w, |y, row| {
        for (k, kv) in kernel.iter().enumerate() {
            let line = &horizontal[clamp(y as isize + k as isize - radius, h) * w..][..w];
            for (o, v) in row.iter_mut().zip(line) {
                *o += kv * v;
            }
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_images_score_one() {
        let img: Vec<f32> = (0..64 * 32).map(|i| ((i * 37) % 101) as f32 / 100.0).collect();
        assert!((ssim(&img, &img, 64, 32) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn structure_loss_lowers_the_score() {
        let edges: Vec<f32> = (0..64 * 32).map(|i| if (i % 64) / 8 % 2 == 0 { 1.0 } else { 0.0 }).collect();
        let flat = vec![0.5; 64 * 32];
        let shifted: Vec<f32> = (0..64 * 32).map(|i| if ((i % 64) + 1) / 8 % 2 == 0 { 1.0 } else { 0.0 }).collect();
        let s_flat = ssim(&edges, &flat, 64, 32);
        let s_shift = ssim(&edges, &shifted, 64, 32);
        assert!(s_flat < 0.1, "flat grey keeps no structure: {s_flat}");
        assert!(s_shift > s_flat && s_shift < 1.0, "a 1px shift is close but not identical: {s_shift}");
    }

    #[test]
    fn blur_preserves_flat_fields() {
        let out = blur(&[0.3; 50], 10, 5, 2.0);
        assert!(out.iter().all(|v| (v - 0.3).abs() < 1e-6));
    }

    #[test]
    fn needs_bitmaps() {
        let atlas = GlyphAtlas::from_coverages(&[(' ', 0.0), ('#', 1.0)], 0.5).unwrap();
        let src = SourceImage::from_rgba8(2, 2, &[0; 16]).unwrap();
        let art = crate::convert(&src, &atlas, &Options { cols: 2, ..Options::DEFAULT }).unwrap();
        assert!(evaluate(&src, &atlas, &Options::DEFAULT, &art).is_err());
    }
}
