//! Hermetic fixtures shared by integration tests and benchmarks: glyphs drawn
//! in code (no font files) and synthetic images (no photos).

#![allow(dead_code)]

use image2ascii_core::{GlyphAtlas, SourceImage};

const CW: usize = 8;
const CH: usize = 16;

/// A 13-glyph atlas on 8×16 cells, with tone levels and shapes that exercise
/// both matchers: dots, bars, diagonals, half-blocks.
pub fn atlas() -> GlyphAtlas {
    let glyph = |f: &dyn Fn(f32, f32) -> bool| -> Vec<u8> {
        (0..CW * CH).map(|i| if f((i % CW) as f32 + 0.5, (i / CW) as f32 + 0.5) { 255 } else { 0 }).collect()
    };
    let near = |a: f32, b: f32| (a - b).abs() < 1.1;
    let glyphs: Vec<(char, Vec<u8>)> = vec![
        (' ', glyph(&|_, _| false)),
        ('.', glyph(&|x, y| (3.0..5.0).contains(&x) && (12.0..14.0).contains(&y))),
        (':', glyph(&|x, y| (3.0..5.0).contains(&x) && ((4.0..6.0).contains(&y) || (10.0..12.0).contains(&y)))),
        ('-', glyph(&|x, y| (1.0..7.0).contains(&x) && (7.0..9.0).contains(&y))),
        ('_', glyph(&|_, y| y >= 13.0)),
        ('|', glyph(&|x, _| (3.0..5.0).contains(&x))),
        ('/', glyph(&|x, y| near(x, (16.0 - y) / 2.0))),
        ('\\', glyph(&|x, y| near(x, y / 2.0))),
        ('+', glyph(&|x, y| ((3.0..5.0).contains(&x) && (3.0..13.0).contains(&y)) || ((1.0..7.0).contains(&x) && (7.0..9.0).contains(&y)))),
        ('[', glyph(&|x, _| x < 3.0)),
        (']', glyph(&|x, _| x > 5.0)),
        ('#', glyph(&|x, y| (x as usize + y as usize) % 2 == 0)),
        ('@', glyph(&|_, _| true)),
    ];
    let refs: Vec<(char, &[u8])> = glyphs.iter().map(|(c, b)| (*c, b.as_slice())).collect();
    GlyphAtlas::from_bitmaps(CW, CH, &refs, None).expect("fixture atlas is valid")
}

/// Grey image from a function of (x, y) → [0, 1] (sRGB-encoded value).
pub fn image(w: usize, h: usize, f: impl Fn(f32, f32) -> f32) -> SourceImage {
    let rgba: Vec<u8> = (0..w * h)
        .flat_map(|i| {
            let v = (f((i % w) as f32, (i / w) as f32).clamp(0.0, 1.0) * 255.0).round() as u8;
            [v, v, v, 255]
        })
        .collect();
    SourceImage::from_rgba8(w, h, &rgba).expect("fixture image is valid")
}

/// 4×4 supersampled coverage, for anti-aliased shapes.
fn supersample(x: f32, y: f32, inside: impl Fn(f32, f32) -> bool) -> f32 {
    let mut n = 0;
    for sy in 0..4 {
        for sx in 0..4 {
            if inside(x + (sx as f32 + 0.5) / 4.0, y + (sy as f32 + 0.5) / 4.0) {
                n += 1;
            }
        }
    }
    n as f32 / 16.0
}

/// Named synthetic test images.
pub fn images() -> Vec<(&'static str, SourceImage)> {
    vec![
        ("gradient", image(256, 64, |x, _| x / 255.0)),
        ("disc", image(160, 160, |x, y| supersample(x, y, |x, y| (x - 80.0).hypot(y - 80.0) < 60.0))),
        ("rings", image(192, 192, |x, y| 0.5 + 0.5 * ((x - 96.0).hypot(y - 96.0) / 6.0).cos())),
        ("diagonal", image(160, 160, |x, y| supersample(x, y, |x, y| x + y < 160.0))),
        ("bars", image(240, 120, |x, _| if (x as usize / 20) % 2 == 0 { 0.9 } else { 0.1 })),
    ]
}
