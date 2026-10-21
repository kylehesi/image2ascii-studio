//! Dev tool: converts an image and renders the result back to a PNG with the
//! same font, so tuning can be judged by eye. Prints the quality score too.
//!
//! cargo run --release -p ascii-core --example preview -- <image> <font> <out.png> [key=value]...
//!
//! Keys: cols, bg (dark|light), mode (shape|density), charset (simple|standard|structure|full|<chars>),
//! weight, dither, clahe, tone (perceptual|linear). Always draws light glyphs on black, so
//! bg=light previews dark-ink polarity as it looks on a dark page.

use std::{env, fs};

use ascii_core::{Background, GlyphAtlas, Mode, Options, Preset, SourceImage, ToneMapping, quality};
use image::{GrayImage, Luma};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().skip(1).collect();
    let [input, font, out, rest @ ..] = args.as_slice() else {
        return Err("usage: preview <image> <font> <out.png> [key=value]...".into());
    };
    let mut opts = Options::DEFAULT;
    let mut charset = None;
    for kv in rest {
        let (key, value) = kv.split_once('=').ok_or_else(|| format!("expected key=value, got {kv}"))?;
        match key {
            "cols" => opts.cols = value.parse()?,
            "bg" => opts.background = if value == "light" { Background::Light } else { Background::Dark },
            "mode" => opts.mode = if value == "density" { Mode::Density } else { Mode::Shape },
            "weight" => opts.shape_weight = value.parse()?,
            "dither" => opts.dither = value.parse()?,
            "clahe" => opts.clahe = value.parse()?,
            "tone" => opts.tone_mapping = if value == "linear" { ToneMapping::Linear } else { ToneMapping::Perceptual },
            "charset" => {
                charset = Some(match value {
                    "simple" => Preset::Simple.chars(),
                    "standard" => Preset::Standard.chars(),
                    "structure" => Preset::Structure.chars(),
                    "full" => Preset::Full.chars(),
                    custom => custom.to_owned(),
                })
            }
            _ => return Err(format!("unknown key {key}").into()),
        }
    }
    let charset = charset.unwrap_or_else(|| Preset::for_mode(opts.mode).chars());

    let font = fs::read(font)?;
    let measure = GlyphAtlas::new(&font, &charset)?;
    let draw = GlyphAtlas::with_px(&font, &charset, 12.0)?;
    let src = SourceImage::decode(&fs::read(input)?)?;

    let start = std::time::Instant::now();
    let art = ascii_core::convert(&src, &measure, &opts)?;
    let took = start.elapsed();
    let score = quality::evaluate(&src, &measure, &opts, &art)?;
    eprintln!(
        "{}×{} → {}×{} in {took:?}  ssim {:.4}  tone error {:.4}",
        src.width(),
        src.height(),
        art.cols(),
        art.rows(),
        score.ssim,
        score.tone_error
    );

    let (cw, ch) = draw.cell_size();
    let mut img = GrayImage::new((art.cols() * cw) as u32, (art.rows() * ch) as u32);
    for r in 0..art.rows() {
        for (c, &glyph) in art.row(r).iter().enumerate() {
            let bitmap = &draw.glyph(glyph).expect("same charset").bitmap;
            for y in 0..ch {
                for x in 0..cw {
                    img.put_pixel((c * cw + x) as u32, (r * ch + y) as u32, Luma([bitmap[y * cw + x]]));
                }
            }
        }
    }
    img.save(out)?;
    Ok(())
}
