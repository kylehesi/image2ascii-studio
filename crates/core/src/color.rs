//! Colour-space helpers. Averaging always happens in linear light; perceptual
//! adjustments (levels, CLAHE, contrast) happen on CIE lightness L*/100.

/// (6/29)^3
const EPSILON: f32 = 216.0 / 24389.0;
/// (29/3)^3
const KAPPA: f32 = 24389.0 / 27.0;

pub(crate) fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

pub(crate) fn srgb8_to_linear_lut() -> [f32; 256] {
    std::array::from_fn(|i| srgb_to_linear(i as f32 / 255.0))
}

/// Rec. 709 relative luminance of linear RGB.
pub(crate) fn luminance(r: f32, g: f32, b: f32) -> f32 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// Linear luminance → perceptual lightness, both in [0, 1].
pub(crate) fn y_to_lightness(y: f32) -> f32 {
    let l = if y > EPSILON { 116.0 * y.cbrt() - 16.0 } else { KAPPA * y };
    l / 100.0
}

/// Table for [`LightnessLut::get`]: `cbrt` per pixel dominated tone mapping.
pub(crate) struct LightnessLut([f32; Self::SIZE + 1]);

impl LightnessLut {
    const SIZE: usize = 4096;

    pub(crate) fn new() -> Self {
        Self(std::array::from_fn(|i| y_to_lightness(i as f32 / Self::SIZE as f32)))
    }

    /// [`y_to_lightness`] by linear interpolation; error < 1e-5 everywhere.
    pub(crate) fn get(&self, y: f32) -> f32 {
        let p = y.clamp(0.0, 1.0) * Self::SIZE as f32;
        let i = (p as usize).min(Self::SIZE - 1);
        let (a, b) = (self.0[i], self.0[i + 1]);
        a + (b - a) * (p - i as f32)
    }
}

/// Perceptual lightness → linear luminance, both in [0, 1].
pub(crate) fn lightness_to_y(l: f32) -> f32 {
    let l = l * 100.0;
    if l > KAPPA * EPSILON {
        ((l + 16.0) / 116.0).powi(3)
    } else {
        l / KAPPA
    }
}

/// Clamps to [0, 1], mapping NaN to 0 so corrupt float images can't poison
/// downstream math.
pub(crate) fn unit(v: f32) -> f32 {
    if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lightness_round_trips() {
        for i in 0..=1000 {
            let y = i as f32 / 1000.0;
            assert!((lightness_to_y(y_to_lightness(y)) - y).abs() < 1e-5, "y = {y}");
        }
    }

    #[test]
    fn lightness_lut_matches_exact_curve() {
        let lut = LightnessLut::new();
        let worst = (0..=100_000)
            .map(|i| i as f32 / 100_000.0)
            .map(|y| (lut.get(y) - y_to_lightness(y)).abs())
            .fold(0.0, f32::max);
        assert!(worst < 1e-5, "worst error {worst}");
    }

    #[test]
    fn middle_grey_is_perceptually_half() {
        // 18% grey sits near L* = 50 — the reason we don't adjust contrast in linear light.
        assert!((y_to_lightness(0.184) - 0.5).abs() < 0.01);
    }

    #[test]
    fn unit_handles_nan() {
        assert_eq!(unit(f32::NAN), 0.0);
        assert_eq!(unit(2.0), 1.0);
        assert_eq!(unit(-1.0), 0.0);
    }
}
