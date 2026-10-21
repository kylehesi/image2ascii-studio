//! Dev tool: sweeps conversion settings over a set of images and prints the
//! mean quality score of each, to tune defaults on data instead of by eye.
//!
//! cargo run --release -p ascii-core --example quality -- <font> <cols> <image>...
//!
//! Extra charsets to compare can be passed in CHARSETS, one per line.

use std::{env, fs};

use ascii_core::quality::{Score, evaluate};
use ascii_core::{GlyphAtlas, Mode, Options, Preset, SourceImage};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().skip(1).collect();
    let [font, cols, images @ ..] = args.as_slice() else {
        return Err("usage: quality <font> <cols> <image>...".into());
    };
    let cols: usize = cols.parse()?;
    let font = fs::read(font)?;
    let mut sources = Vec::new();
    for path in images {
        let bytes = fs::read(path).map_err(|e| format!("{path}: {e}"))?;
        sources.push(SourceImage::decode(&bytes).map_err(|e| format!("{path}: {e}"))?);
    }

    let mut charsets = vec![
        ("standard".to_owned(), Preset::Standard.chars()),
        ("structure".to_owned(), Preset::Structure.chars()),
        ("full".to_owned(), Preset::Full.chars()),
    ];
    if let Ok(extra) = env::var("CHARSETS") {
        charsets.extend(extra.lines().enumerate().map(|(i, c)| (format!("custom{i}"), c.to_owned())));
    }
    let atlases =
        charsets.iter().map(|(n, c)| Ok((n.clone(), GlyphAtlas::new(&font, c)?))).collect::<ascii_core::Result<Vec<_>>>()?;
    let base = Options { cols, ..Options::DEFAULT };
    let mut configs: Vec<(String, Options)> = Vec::new();
    for dither in [0.0, 0.5] {
        configs.push((format!("density dither={dither}"), Options { mode: Mode::Density, dither, ..base.clone() }));
    }
    for weight in [0.35, 0.5, 0.65] {
        for dither in [0.0, 0.5] {
            configs.push((
                format!("shape w={weight} dither={dither}"),
                Options { mode: Mode::Shape, shape_weight: weight, dither, ..base.clone() },
            ));
        }
    }

    println!("{:<9} {:<34} {:>7} {:>9}", "charset", "config", "ssim", "tone_err");
    for (name, atlas) in &atlases {
        for (label, opts) in &configs {
            let mut total = Score { ssim: 0.0, tone_error: 0.0 };
            for src in &sources {
                let art = ascii_core::convert(src, atlas, opts)?;
                let s = evaluate(src, atlas, opts, &art)?;
                total.ssim += s.ssim / sources.len() as f32;
                total.tone_error += s.tone_error / sources.len() as f32;
            }
            println!("{name:<9} {label:<34} {:>7.4} {:>9.4}", total.ssim, total.tone_error);
        }
    }
    Ok(())
}
