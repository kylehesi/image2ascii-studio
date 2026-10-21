//! Dev tool: prints every printable ASCII glyph's measured coverage, vertical
//! centre of ink and vertical spread in a font — the data behind the charset
//! presets.
//!
//! cargo run --release -p ascii-core --example glyphs -- <font>

use std::{env, fs};

use ascii_core::{GlyphAtlas, Preset};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let font = fs::read(env::args().nth(1).ok_or("usage: glyphs <font>")?)?;
    let atlas = GlyphAtlas::new(&font, &Preset::Full.chars())?;
    let (w, h) = atlas.cell_size();
    println!("cell {w}×{h}, aspect {:.3}", atlas.cell_aspect());
    println!("char  coverage  level  centre_y  spread_y");
    for g in atlas.glyphs() {
        let rows: Vec<f64> = (0..h).map(|y| g.bitmap[y * w..(y + 1) * w].iter().map(|&v| v as f64).sum()).collect();
        let total: f64 = rows.iter().sum();
        let (centre, spread) = if total > 0.0 {
            let c = rows.iter().enumerate().map(|(y, v)| y as f64 * v).sum::<f64>() / total;
            let var = rows.iter().enumerate().map(|(y, v)| (y as f64 - c).powi(2) * v).sum::<f64>() / total;
            (c / h as f64, var.sqrt() / h as f64)
        } else {
            (0.5, 0.0)
        };
        println!("{:?}  {:8.4}  {:5.3}  {:8.3}  {:8.3}", g.ch, g.coverage, g.level, centre, spread);
    }
    Ok(())
}
