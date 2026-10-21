//! Character-grid geometry and exact area resampling.

use crate::par;

/// Rows needed so cells of `cell_aspect` (width / height) keep the image's
/// proportions at `cols` columns.
pub(crate) fn rows_for(width: usize, height: usize, cols: usize, cell_aspect: f32) -> usize {
    let rows = height as f64 / width as f64 * cols as f64 * cell_aspect as f64;
    (rows.round() as usize).max(1)
}

/// Resamples `plane` (w×h) to out_w×out_h by exact area averaging: every
/// output value is the integral of the source over its footprint, including
/// fractional pixels at the edges. Works for both down- and up-sampling.
///
/// Separable: each source row is integrated against the column boundaries,
/// then each resulting column is prefix-summed and integrated against the row
/// boundaries — memory stays O(h×out_w) instead of a full summed-area table,
/// and every stage is independent per row or per column.
pub(crate) fn area_average(plane: &[f32], w: usize, h: usize, out_w: usize, out_h: usize) -> Vec<f32> {
    let xb = boundaries(w, out_w);
    let yb = boundaries(h, out_h);

    // row_int[y * out_w + i]: integral of source row y over output column i.
    let mut row_int = vec![0f64; h * out_w];
    par::for_each_chunk(&mut row_int, out_w, |y, out| {
        let mut prefix = Vec::with_capacity(w + 1);
        prefix.push(0f64);
        let mut acc = 0f64;
        for &v in &plane[y * w..(y + 1) * w] {
            acc += v as f64;
            prefix.push(acc);
        }
        for (i, o) in out.iter_mut().enumerate() {
            *o = interp(&prefix, xb[i + 1]) - interp(&prefix, xb[i]);
        }
    });

    // Column-major prefix sums: cum[i * (h + 1) + y] = Σ row_int[0..y][i].
    let stride = h + 1;
    let mut cum = vec![0f64; stride * out_w];
    par::for_each_chunk(&mut cum, stride, |i, col| {
        for y in 0..h {
            col[y + 1] = col[y] + row_int[y * out_w + i];
        }
    });

    let mut out = vec![0f32; out_w * out_h];
    par::for_each_chunk(&mut out, out_w, |j, row| {
        let area_h = yb[j + 1] - yb[j];
        for (i, o) in row.iter_mut().enumerate() {
            let col = &cum[i * stride..(i + 1) * stride];
            let sum = interp(col, yb[j + 1]) - interp(col, yb[j]);
            *o = (sum / ((xb[i + 1] - xb[i]) * area_h)) as f32;
        }
    });
    out
}

/// Reorders a (cols·sx)×(rows·sy) row-major sample grid so each cell's
/// sx×sy samples are contiguous (row-major within the cell).
pub(crate) fn cell_major(samples: &[f32], cols: usize, rows: usize, sx: usize, sy: usize) -> Vec<f32> {
    let width = cols * sx;
    let mut out = vec![0f32; samples.len()];
    par::for_each_chunk(&mut out, sx * sy, |cell, dst| {
        let (cx, cy) = (cell % cols, cell / cols);
        for y in 0..sy {
            let src = &samples[(cy * sy + y) * width + cx * sx..][..sx];
            dst[y * sx..(y + 1) * sx].copy_from_slice(src);
        }
    });
    debug_assert_eq!(out.len(), cols * rows * sx * sy);
    out
}

fn boundaries(len: usize, parts: usize) -> Vec<f64> {
    (0..=parts).map(|i| i as f64 * len as f64 / parts as f64).collect()
}

/// Linear interpolation into a prefix-sum array: exact for the integral of a
/// piecewise-constant signal.
fn interp(prefix: &[f64], x: f64) -> f64 {
    let i = (x as usize).min(prefix.len() - 2);
    prefix[i] + (prefix[i + 1] - prefix[i]) * (x - i as f64)
}

/// Unsharp mask over the cell grid (3×3 binomial blur, edges clamped).
/// Applied after resampling, where it actually changes character choice.
pub(crate) fn sharpen(cells: &mut [f32], cols: usize, rows: usize, amount: f32) {
    let at = |x: isize, y: isize| {
        let x = x.clamp(0, cols as isize - 1) as usize;
        let y = y.clamp(0, rows as isize - 1) as usize;
        cells[y * cols + x]
    };
    const K: [f32; 3] = [1.0, 2.0, 1.0];
    let mut out = Vec::with_capacity(cells.len());
    for y in 0..rows as isize {
        for x in 0..cols as isize {
            let mut blur = 0.0;
            for (dy, ky) in (-1..=1).zip(K) {
                for (dx, kx) in (-1..=1).zip(K) {
                    blur += at(x + dx, y + dy) * kx * ky;
                }
            }
            let v = at(x, y);
            out.push((v + amount * (v - blur / 16.0)).clamp(0.0, 1.0));
        }
    }
    cells.copy_from_slice(&out);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_follow_cell_aspect() {
        assert_eq!(rows_for(1000, 1000, 100, 0.5), 50);
        assert_eq!(rows_for(2000, 1000, 100, 0.5), 25);
        assert_eq!(rows_for(10_000, 1, 10, 0.5), 1);
    }

    #[test]
    fn integer_blocks_average_exactly() {
        let plane = [0.0, 1.0, 0.2, 0.2, 1.0, 0.0, 0.2, 0.2];
        let out = area_average(&plane, 4, 2, 2, 1);
        assert_eq!(out, vec![0.5, 0.2]);
    }

    #[test]
    fn fractional_footprints_weight_partial_pixels() {
        // 3 pixels into 2 cells of 1.5 px: cell 0 = p0 + half of p1.
        let out = area_average(&[0.0, 0.6, 1.2], 3, 1, 2, 1);
        assert!((out[0] - 0.2).abs() < 1e-6);
        assert!((out[1] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn mean_is_preserved() {
        let plane: Vec<f32> = (0..37 * 23).map(|i| ((i * 7919) % 101) as f32 / 100.0).collect();
        let mean_in = plane.iter().sum::<f32>() / plane.len() as f32;
        let out = area_average(&plane, 37, 23, 10, 6);
        let mean_out = out.iter().sum::<f32>() / out.len() as f32;
        assert!((mean_in - mean_out).abs() < 1e-5);
    }

    #[test]
    fn upsampling_replicates() {
        let out = area_average(&[0.25, 0.75], 2, 1, 4, 2);
        assert_eq!(out, vec![0.25, 0.25, 0.75, 0.75, 0.25, 0.25, 0.75, 0.75]);
    }

    #[test]
    fn cell_major_groups_each_cells_samples() {
        // 2×1 cells of 2×2 samples: grid rows "a b c d" / "e f g h".
        let grid: Vec<f32> = (0..8).map(|v| v as f32).collect();
        assert_eq!(cell_major(&grid, 2, 1, 2, 2), vec![0.0, 1.0, 4.0, 5.0, 2.0, 3.0, 6.0, 7.0]);
    }

    #[test]
    fn sharpen_leaves_flat_fields_alone() {
        let mut cells = vec![0.4; 12];
        sharpen(&mut cells, 4, 3, 1.0);
        assert!(cells.iter().all(|&v| (v - 0.4).abs() < 1e-6));
    }
}
