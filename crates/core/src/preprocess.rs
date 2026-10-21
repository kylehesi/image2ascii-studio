//! Tone mapping: turns a [`SourceImage`] into an *ink plane* — for each pixel,
//! the fraction of the character cell that should be covered by glyph ink.

use crate::color::{LightnessLut, lightness_to_y, y_to_lightness};
use crate::par;
use crate::source::SourceImage;
use crate::{Background, Options, ToneMapping};

const LEVEL_BINS: usize = 4096;
/// Auto-levels is skipped when the chosen percentiles are closer than this;
/// stretching a near-flat image only amplifies noise.
const MIN_LEVEL_RANGE: f32 = 0.02;
const CLAHE_BINS: usize = 256;
/// Rows per histogram task.
const BAND: usize = 64;

pub(crate) fn ink_plane(src: &SourceImage, opts: &Options) -> Vec<f32> {
    let (w, h) = (src.width(), src.height());
    let (luma, alpha) = (src.luma(), src.alpha());

    let lut = LightnessLut::new();
    let mut plane = vec![0f32; w * h];
    par::for_each_chunk(&mut plane, w, |y, row| {
        for (v, &l) in row.iter_mut().zip(&luma[y * w..]) {
            *v = lut.get(l);
        }
    });
    if opts.auto_levels {
        auto_levels(&mut plane, w, h, alpha, opts.levels_low / 100.0, opts.levels_high / 100.0);
    }
    if opts.clahe > 0.0 {
        clahe(&mut plane, w, h, alpha, opts.clahe_tiles, opts.clahe_clip, opts.clahe);
    }

    let inv_gamma = 1.0 / opts.gamma;
    let bg = match opts.background {
        Background::Dark => 0.0,
        Background::Light => 1.0,
    };
    par::for_each_chunk(&mut plane, w, |y, row| {
        for (x, v) in row.iter_mut().enumerate() {
            let mut l = ((*v - 0.5) * opts.contrast + 0.5 + opts.brightness).clamp(0.0, 1.0);
            if opts.gamma != 1.0 {
                l = l.powf(inv_gamma);
            }
            // Back to linear light: what the eye averages over a cell is emitted light.
            let a = alpha.map_or(1.0, |a| a[y * w + x]);
            let lin = lightness_to_y(l) * a + bg * (1.0 - a);
            *v = match opts.background {
                Background::Dark => lin,
                Background::Light => 1.0 - lin,
            };
        }
    });
    plane
}

/// Moves area-averaged ink values (linear light) into the space glyph levels
/// are compared in. Averaging must happen before this, never after.
pub(crate) fn to_tone_space(values: &mut [f32], opts: &Options) {
    if opts.tone_mapping == ToneMapping::Linear {
        return;
    }
    for v in values {
        *v = match opts.background {
            Background::Dark => y_to_lightness(*v),
            Background::Light => 1.0 - y_to_lightness(1.0 - *v),
        };
    }
}

fn visible(alpha: Option<&[f32]>, i: usize) -> bool {
    alpha.is_none_or(|a| a[i] > 0.0)
}

/// Stretches lightness so the `low`..`high` quantiles span the full range.
fn auto_levels(plane: &mut [f32], w: usize, h: usize, alpha: Option<&[f32]>, low: f32, high: f32) {
    let bands = par::map(h.div_ceil(BAND), |b| {
        let mut hist = vec![0u64; LEVEL_BINS];
        for i in b * BAND * w..((b + 1) * BAND).min(h) * w {
            if visible(alpha, i) {
                hist[bin(plane[i], LEVEL_BINS)] += 1;
            }
        }
        hist
    });
    let mut hist = vec![0u64; LEVEL_BINS];
    for band in bands {
        for (total, c) in hist.iter_mut().zip(band) {
            *total += c;
        }
    }
    let total: u64 = hist.iter().sum();
    if total == 0 {
        return;
    }
    let lo = quantile(&hist, total, low);
    let hi = quantile(&hist, total, high);
    if hi - lo < MIN_LEVEL_RANGE {
        return;
    }
    let scale = 1.0 / (hi - lo);
    par::for_each_chunk(plane, w, |_, row| {
        for v in row {
            *v = ((*v - lo) * scale).clamp(0.0, 1.0);
        }
    });
}

fn bin(v: f32, bins: usize) -> usize {
    ((v.max(0.0) * bins as f32) as usize).min(bins - 1)
}

fn quantile(hist: &[u64], total: u64, q: f32) -> f32 {
    let target = ((q.clamp(0.0, 1.0) as f64) * total as f64) as u64;
    let mut acc = 0;
    for (b, &c) in hist.iter().enumerate() {
        acc += c;
        if acc > target {
            return (b as f32 + 0.5) / hist.len() as f32;
        }
    }
    1.0
}

/// Contrast-limited adaptive histogram equalisation, blended with the input
/// by `strength`. Pulls detail out of shadows and highlights locally (eyes,
/// hair, backlit faces) without the global blow-out of plain equalisation.
fn clahe(
    plane: &mut [f32],
    w: usize,
    h: usize,
    alpha: Option<&[f32]>,
    tiles: usize,
    clip: f32,
    strength: f32,
) {
    // Tiles stay roughly square: `tiles` along the long side.
    let long = w.max(h) as f32;
    let tx_n = ((tiles as f32 * w as f32 / long).round() as usize).clamp(1, w);
    let ty_n = ((tiles as f32 * h as f32 / long).round() as usize).clamp(1, h);

    // Each tile gets a CDF sampled at bin edges (CLAHE_BINS + 1 values) and
    // read with linear interpolation, so equalisation doesn't quantise to bins.
    let stride = CLAHE_BINS + 1;
    let source: &[f32] = plane;
    let cdfs: Vec<f32> = par::map(tx_n * ty_n, |t| {
        let (ty, tx) = (t / tx_n, t % tx_n);
        let (y0, y1) = (ty * h / ty_n, (ty + 1) * h / ty_n);
        let (x0, x1) = (tx * w / tx_n, (tx + 1) * w / tx_n);
        let mut hist = [0f32; CLAHE_BINS];
        let mut n = 0f32;
        for y in y0..y1 {
            for x in x0..x1 {
                let i = y * w + x;
                if visible(alpha, i) {
                    hist[bin(source[i], CLAHE_BINS)] += 1.0;
                    n += 1.0;
                }
            }
        }
        let mut cdf = vec![0f32; stride];
        clipped_cdf(&mut hist, n, clip, &mut cdf);
        cdf
    })
    .concat();

    let tile_w = w as f32 / tx_n as f32;
    let tile_h = h as f32 / ty_n as f32;
    let xs: Vec<_> = (0..w).map(|x| tile_neighbours(x, tile_w, tx_n)).collect();
    par::for_each_chunk(plane, w, |y, row| {
        let (ty0, ty1, fy) = tile_neighbours(y, tile_h, ty_n);
        let (row0, row1) = (ty0 * tx_n * stride, ty1 * tx_n * stride);
        for (v, &(tx0, tx1, fx)) in row.iter_mut().zip(&xs) {
            let orig = *v;
            // Bin position is shared by all four tile CDFs.
            let p = orig.clamp(0.0, 1.0) * CLAHE_BINS as f32;
            let b = (p as usize).min(CLAHE_BINS - 1);
            let f = p - b as f32;
            let eq = |base: usize| lerp(cdfs[base + b], cdfs[base + b + 1], f);
            let top = lerp(eq(row0 + tx0 * stride), eq(row0 + tx1 * stride), fx);
            let bottom = lerp(eq(row1 + tx0 * stride), eq(row1 + tx1 * stride), fx);
            *v = lerp(orig, lerp(top, bottom, fy), strength);
        }
    });
}

fn clipped_cdf(hist: &mut [f32; CLAHE_BINS], n: f32, clip: f32, cdf: &mut [f32]) {
    if n == 0.0 {
        for (b, c) in cdf.iter_mut().enumerate() {
            *c = b as f32 / CLAHE_BINS as f32;
        }
        return;
    }
    // Bounds the mapping's slope at `clip` — no integer floor, which would
    // silently disable limiting on small tiles.
    let limit = clip * n / CLAHE_BINS as f32;
    let mut excess = 0.0;
    for c in hist.iter_mut() {
        if *c > limit {
            excess += *c - limit;
            *c = limit;
        }
    }
    let bonus = excess / CLAHE_BINS as f32;
    let mut acc = 0.0;
    cdf[0] = 0.0;
    for (b, c) in hist.iter().enumerate() {
        acc += c + bonus;
        cdf[b + 1] = acc / n;
    }
}

/// The two tile centres bracketing pixel `p` along one axis and the blend
/// factor between them; clamps at the borders.
fn tile_neighbours(p: usize, tile_size: f32, n: usize) -> (usize, usize, f32) {
    let g = (p as f32 + 0.5) / tile_size - 0.5;
    if g <= 0.0 {
        return (0, 0, 0.0);
    }
    let i = g as usize;
    if i >= n - 1 { (n - 1, n - 1, 0.0) } else { (i, i + 1, g - i as f32) }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain() -> Options {
        Options { auto_levels: false, clahe: 0.0, ..Options::DEFAULT }
    }

    fn grey_ramp(w: usize, lo: u8, hi: u8) -> SourceImage {
        let rgba: Vec<u8> = (0..w)
            .flat_map(|x| {
                let v = lo + ((hi - lo) as usize * x / (w - 1)) as u8;
                [v, v, v, 255]
            })
            .collect();
        SourceImage::from_rgba8(w, 1, &rgba).unwrap()
    }

    #[test]
    fn neutral_options_preserve_linear_luminance() {
        let src = grey_ramp(64, 0, 255);
        let ink = ink_plane(&src, &plain());
        for (a, b) in ink.iter().zip(src.luma()) {
            assert!((a - b).abs() < 1e-5);
        }
    }

    #[test]
    fn light_background_inverts_ink() {
        let src = grey_ramp(16, 0, 255);
        let ink = ink_plane(&src, &Options { background: Background::Light, ..plain() });
        assert!((ink[0] - 1.0).abs() < 1e-6);
        assert!(ink[15].abs() < 1e-6);
    }

    #[test]
    fn auto_levels_stretches_a_dull_image() {
        let src = grey_ramp(256, 80, 160);
        let ink = ink_plane(&src, &Options { auto_levels: true, ..plain() });
        assert!(ink[0] < 0.01, "darkest should reach black, got {}", ink[0]);
        assert!(ink[255] > 0.99, "brightest should reach white, got {}", ink[255]);
    }

    #[test]
    fn transparent_pixels_become_background() {
        let src = SourceImage::from_rgba8(2, 1, &[255, 255, 255, 0, 255, 255, 255, 255]).unwrap();
        let ink = ink_plane(&src, &plain());
        assert_eq!(ink[0], 0.0);
        assert!((ink[1] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn clahe_keeps_gradients_nearly_monotonic() {
        // Interpolating between tile mappings can reverse a ramp slightly —
        // inherent to CLAHE. The clip limit must keep that far below one
        // glyph step (~1/20 of the range) so it never shows up as banding.
        let src = grey_ramp(200, 40, 200);
        for strength in [0.4, 1.0] {
            let ink = ink_plane(&src, &Options { clahe: strength, ..plain() });
            assert!(ink.iter().all(|v| (0.0..=1.0).contains(v)));
            let worst = ink.windows(2).map(|p| p[0] - p[1]).fold(0.0f32, f32::max);
            assert!(worst < 0.012, "strength {strength}: reversal {worst}");
        }
    }

    #[test]
    fn flat_image_survives_every_stage() {
        let src = SourceImage::from_rgba8(3, 3, &[128; 36]).unwrap();
        let ink = ink_plane(&src, &Options { clahe: 1.0, ..Options::DEFAULT });
        assert!(ink.iter().all(|v| v.is_finite()));
    }
}
