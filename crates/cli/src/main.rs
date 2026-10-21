use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use image2ascii_core::render::{self, Canvas, Frame, Rgb, Style, SvgGlyphs, SvgOptions, Typeface};
use image2ascii_core::{
    AsciiArt, Background, GlyphAtlas, Mode, Options, Preset, RevealOrder, RevealTimeline, SourceImage, ToneMapping,
    quality,
};
use clap::{Parser, ValueEnum};

const D: Options = Options::DEFAULT;

/// Convert an image to ASCII art.
#[derive(Parser)]
#[command(name = "image2ascii", version)]
struct Args {
    /// Image to convert (PNG, JPEG, WebP, GIF, BMP, TIFF).
    input: PathBuf,

    /// Write the art to a file instead of only printing it; the extension picks
    /// the format: .txt, .svg (animated reveal) or .png. Repeat for several.
    #[arg(short, long)]
    output: Vec<PathBuf>,

    /// Width in characters.
    #[arg(short, long, default_value_t = D.cols)]
    cols: usize,

    /// How glyphs are chosen: by tone pattern (shape) or average tone (density).
    #[arg(short, long, value_enum, default_value_t = ModeArg::Shape)]
    mode: ModeArg,

    /// Shape mode: structure vs. tone weight, 0..1.
    #[arg(long, default_value_t = D.shape_weight)]
    shape_weight: f32,

    /// Error-diffusion dithering strength, 0..1.
    #[arg(long, default_value_t = D.dither)]
    dither: f32,

    /// Print a quality score (SSIM and tone error vs. the source) to stderr.
    #[arg(long)]
    score: bool,

    /// Built-in character set.
    #[arg(long, value_enum, default_value_t = CharsetArg::Auto)]
    charset: CharsetArg,

    /// Custom characters (any order — they are measured and sorted). Overrides --charset.
    #[arg(long)]
    chars: Option<String>,

    /// Background the art will be shown on.
    #[arg(long, value_enum, default_value_t = BackgroundArg::Dark)]
    background: BackgroundArg,

    /// How tone maps to density: perceptual (as glyphs are read) or linear (as they blur together).
    #[arg(long, value_enum, default_value_t = ToneArg::Perceptual)]
    tone: ToneArg,

    /// Monospace font (.ttf/.otf) for calibration and SVG/PNG. Defaults to the bundled JetBrains Mono.
    #[arg(long)]
    font: Option<PathBuf>,

    /// Character cell width/height. Defaults to the font's metrics.
    #[arg(long)]
    cell_aspect: Option<f32>,

    /// Disable automatic tone stretching.
    #[arg(long)]
    no_levels: bool,

    /// Lower auto-levels percentile.
    #[arg(long, default_value_t = D.levels_low)]
    levels_low: f32,

    /// Upper auto-levels percentile.
    #[arg(long, default_value_t = D.levels_high)]
    levels_high: f32,

    /// Local contrast strength, 0..1.
    #[arg(long, default_value_t = D.clahe)]
    clahe: f32,

    /// Local contrast clip limit (>= 1).
    #[arg(long, default_value_t = D.clahe_clip)]
    clahe_clip: f32,

    /// Contrast multiplier.
    #[arg(long, default_value_t = D.contrast)]
    contrast: f32,

    /// Brightness offset, -1..1.
    #[arg(long, default_value_t = D.brightness, allow_negative_numbers = true)]
    brightness: f32,

    /// Gamma; > 1 brightens mid-tones.
    #[arg(long, default_value_t = D.gamma)]
    gamma: f32,

    /// Edge sharpening amount.
    #[arg(long, default_value_t = D.sharpen)]
    sharpen: f32,

    /// Play the reveal animation in the terminal.
    #[arg(long)]
    reveal: bool,

    /// Reveal animation (terminal and SVG).
    #[arg(long, value_enum, default_value_t = OrderArg::Typewriter)]
    reveal_order: OrderArg,

    /// Reveal animation length in milliseconds.
    #[arg(long, default_value_t = 3000)]
    reveal_ms: u32,

    /// Reveal animation frame rate (terminal).
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u32).range(1..=240))]
    fps: u32,

    /// SVG/PNG: image size, `original` (the input's size) or WIDTHxHEIGHT, e.g. 1080x1080.
    /// The art is scaled to fit and centred; unused space stays background.
    #[arg(long, default_value = "original", value_parser = parse_size)]
    size: SizeArg,

    /// SVG/PNG: multiply --size, e.g. 2 for a double-size image.
    #[arg(long, default_value_t = 1.0)]
    scale: f32,

    /// SVG/PNG: draw glyphs at this font size (px) and size the image to the
    /// art instead of using --size.
    #[arg(long, conflicts_with_all = ["size", "scale"])]
    font_size: Option<f32>,

    /// SVG/PNG: no window frame (title bar with dots).
    #[arg(long)]
    no_frame: bool,

    /// SVG/PNG: window title. Defaults to the input name with .txt.
    #[arg(long)]
    title: Option<String>,

    /// SVG/PNG: text colour, #rrggbb. Defaults to GitHub's for the background.
    #[arg(long)]
    fg: Option<String>,

    /// SVG/PNG: background colour, #rrggbb.
    #[arg(long)]
    bg: Option<String>,

    /// SVG: static image instead of the reveal animation.
    #[arg(long)]
    no_animate: bool,

    /// SVG: replay the reveal forever, holding the finished art this many milliseconds.
    #[arg(long)]
    loop_pause: Option<u32>,

    /// SVG: plain <text> rows in the viewer's font instead of exact glyph outlines.
    #[arg(long)]
    svg_text: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Format {
    Txt,
    Svg,
    Png,
}

fn format_of(path: &Path) -> Result<Format> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "txt" => Ok(Format::Txt),
        "svg" => Ok(Format::Svg),
        "png" => Ok(Format::Png),
        _ => bail!("{}: unknown output format, use .txt, .svg or .png", path.display()),
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum OrderArg {
    /// One character at a time, first to last.
    Typewriter,
    /// Whole lines, top to bottom.
    Lines,
    /// Characters in random order.
    Random,
    /// Outwards from the centre.
    #[value(alias = "center")]
    Centre,
}

impl From<OrderArg> for RevealOrder {
    fn from(o: OrderArg) -> Self {
        match o {
            OrderArg::Typewriter => RevealOrder::Typewriter,
            OrderArg::Lines => RevealOrder::Lines,
            OrderArg::Random => RevealOrder::Random,
            OrderArg::Centre => RevealOrder::Centre,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum ModeArg {
    Shape,
    Density,
}

#[derive(Clone, Copy, ValueEnum)]
enum CharsetArg {
    /// `structure` for shape mode, `standard` for density mode.
    Auto,
    Simple,
    Standard,
    Structure,
    Full,
}

#[derive(Clone, Copy, ValueEnum)]
enum BackgroundArg {
    Dark,
    Light,
}

#[derive(Clone, Copy, ValueEnum)]
enum ToneArg {
    Perceptual,
    Linear,
}

fn main() -> Result<()> {
    let args = Args::parse();
    // Fail on a bad output name before doing any work.
    let outputs = args.output.iter().map(|p| Ok((p, format_of(p)?))).collect::<Result<Vec<_>>>()?;

    let bytes = fs::read(&args.input).with_context(|| format!("reading {}", args.input.display()))?;
    let src = SourceImage::decode(&bytes).with_context(|| format!("decoding {}", args.input.display()))?;

    let font = match &args.font {
        Some(p) => fs::read(p).with_context(|| format!("reading font {}", p.display()))?,
        None => image2ascii_core::fonts::DEFAULT.to_vec(),
    };

    let opts = Options {
        cols: args.cols,
        mode: match args.mode {
            ModeArg::Shape => Mode::Shape,
            ModeArg::Density => Mode::Density,
        },
        shape_weight: args.shape_weight,
        dither: args.dither,
        cell_aspect: args.cell_aspect,
        background: match args.background {
            BackgroundArg::Dark => Background::Dark,
            BackgroundArg::Light => Background::Light,
        },
        tone_mapping: match args.tone {
            ToneArg::Perceptual => ToneMapping::Perceptual,
            ToneArg::Linear => ToneMapping::Linear,
        },
        auto_levels: !args.no_levels,
        levels_low: args.levels_low,
        levels_high: args.levels_high,
        clahe: args.clahe,
        clahe_clip: args.clahe_clip,
        clahe_tiles: D.clahe_tiles,
        contrast: args.contrast,
        brightness: args.brightness,
        gamma: args.gamma,
        sharpen: args.sharpen,
    };
    let charset = match &args.chars {
        Some(c) => c.clone(),
        None => match args.charset {
            CharsetArg::Auto => Preset::for_mode(opts.mode),
            CharsetArg::Simple => Preset::Simple,
            CharsetArg::Standard => Preset::Standard,
            CharsetArg::Structure => Preset::Structure,
            CharsetArg::Full => Preset::Full,
        }
        .chars(),
    };
    let atlas = GlyphAtlas::new(&font, &charset).context("calibrating glyphs")?;
    let art = image2ascii_core::convert(&src, &atlas, &opts)?;
    let text = art.to_text();
    if args.score {
        let s = quality::evaluate(&src, &atlas, &opts, &art)?;
        eprintln!("ssim {:.4}  tone error {:.4}", s.ssim, s.tone_error);
    }

    let timeline = RevealTimeline::new(&art, args.reveal_order.into(), args.reveal_ms);
    let mut face = None;
    for (path, format) in &outputs {
        let bytes = match format {
            // Native line endings: some Windows programs show LF-only text as one line.
            Format::Txt if cfg!(windows) => text.replace('\n', "\r\n").into_bytes(),
            Format::Txt => text.clone().into_bytes(),
            Format::Svg | Format::Png => {
                let face = match &face {
                    Some(f) => f,
                    None => face.insert(Typeface::from_bytes(&font)?),
                };
                let style = style(&args, opts.background, src.original_size())?;
                if *format == Format::Png {
                    render::render_png(&art, face, &style)?
                } else {
                    let svg_opts = SvgOptions {
                        style,
                        glyphs: if args.svg_text { SvgGlyphs::Text } else { SvgGlyphs::Paths },
                        loop_pause_ms: args.loop_pause,
                    };
                    let animation = (!args.no_animate).then_some(&timeline);
                    render::render_svg(&art, face, &svg_opts, animation)?.into_bytes()
                }
            }
        };
        fs::write(path, &bytes).with_context(|| format!("writing {}", path.display()))?;
        eprintln!("wrote {} ({}×{}, {} KB)", path.display(), art.cols(), art.rows(), bytes.len().div_ceil(1024));
    }

    let stdout = io::stdout();
    if args.reveal && stdout.is_terminal() {
        play(&art, &timeline, args.fps, can_redraw(&art), &mut stdout.lock())?;
    } else if outputs.is_empty() {
        stdout.lock().write_all(text.as_bytes())?;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum SizeArg {
    Original,
    Fixed(u32, u32),
}

fn parse_size(s: &str) -> Result<SizeArg, String> {
    if s.eq_ignore_ascii_case("original") {
        return Ok(SizeArg::Original);
    }
    let parts: Vec<&str> = s.split(['x', 'X', '×', '*']).collect();
    match parts.as_slice() {
        [w, h] => match (w.trim().parse(), h.trim().parse()) {
            (Ok(w), Ok(h)) => Ok(SizeArg::Fixed(w, h)),
            _ => Err(format!("{s:?} is not WIDTHxHEIGHT")),
        },
        _ => Err(format!("{s:?} is not `original` or WIDTHxHEIGHT")),
    }
}

/// The export canvas: an explicit font size, or the requested size (the
/// input's original size by default) times --scale, capped to the limits.
fn canvas(args: &Args, original: (usize, usize)) -> Result<Canvas> {
    if let Some(px) = args.font_size {
        return Ok(Canvas::FontSize(px));
    }
    if !(args.scale.is_finite() && args.scale > 0.0 && args.scale <= 16.0) {
        bail!("--scale must be in (0, 16], got {}", args.scale);
    }
    let (w, h) = match args.size {
        SizeArg::Original => (original.0 as u32, original.1 as u32),
        SizeArg::Fixed(w, h) => (w, h),
    };
    let (canvas, capped) = Canvas::fixed_scaled(w, h, args.scale);
    if capped {
        let Canvas::Fixed { width, height } = canvas else { unreachable!() };
        eprintln!("note: image size capped to {width}×{height}");
    }
    Ok(canvas)
}

fn style(args: &Args, background: Background, original: (usize, usize)) -> Result<Style> {
    let mut style = Style::for_background(background);
    style.canvas = canvas(args, original)?;
    if let Some(fg) = &args.fg {
        style.fg = Rgb::parse(fg)?;
    }
    if let Some(bg) = &args.bg {
        style.bg = Rgb::parse(bg)?;
    }
    if !args.no_frame {
        let title = args.title.clone().unwrap_or_else(|| {
            let stem = args.input.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            format!("{stem}.txt")
        });
        style.frame = Some(Frame { title });
    }
    Ok(style)
}

/// Plays the reveal in the terminal. Reading-order reveals are streamed as
/// plain text; the others redraw the art in place each frame, which needs a
/// terminal with ANSI cursor movement that fits the whole art — otherwise the
/// finished art is printed at once.
fn play(art: &AsciiArt, timeline: &RevealTimeline, fps: u32, can_redraw: bool, out: &mut impl Write) -> io::Result<()> {
    if timeline.order().is_sequential() {
        stream(art, timeline, fps, out)
    } else if can_redraw {
        redraw(art, timeline, fps, out)
    } else {
        out.write_all(art.to_text().as_bytes())
    }
}

/// Whether the art can be redrawn in place: ANSI cursor movement works (on
/// Windows this also switches it on for older consoles) and the art fits.
fn can_redraw(art: &AsciiArt) -> bool {
    #[cfg(windows)]
    let ansi = crossterm::ansi_support::supports_ansi();
    #[cfg(not(windows))]
    let ansi = true;
    let fits = crossterm::terminal::size()
        .is_ok_and(|(w, h)| art.cols() <= w as usize && art.rows() < h as usize);
    ansi && fits
}

/// Runs `step(elapsed_ms)` once per frame at `fps` until it returns true.
fn frames(fps: u32, mut step: impl FnMut(u128) -> io::Result<bool>) -> io::Result<()> {
    let frame = Duration::from_secs_f64(1.0 / fps as f64);
    let start = Instant::now();
    for n in 1u32.. {
        let deadline = start + frame * n;
        if step(deadline.duration_since(start).as_millis())? {
            break;
        }
        if let Some(wait) = deadline.checked_duration_since(Instant::now()) {
            thread::sleep(wait);
        }
    }
    Ok(())
}

/// Reveals scattered characters by redrawing the art in place: reserve its
/// lines, then each frame move back up and print it with what has appeared so far.
fn redraw(art: &AsciiArt, timeline: &RevealTimeline, fps: u32, out: &mut impl Write) -> io::Result<()> {
    let (cols, rows) = (art.cols(), art.rows());
    let mut shown = vec![' '; cols * rows];
    let mut order: Vec<usize> = (0..shown.len()).filter(|&i| art.cells()[i] != ' ').collect();
    order.sort_by_key(|&i| timeline.appear_ms()[i]);

    out.write_all("\n".repeat(rows).as_bytes())?;
    let mut next = 0;
    let mut buf = String::new();
    frames(fps, |now_ms| {
        while next < order.len() && u128::from(timeline.appear_ms()[order[next]]) <= now_ms {
            shown[order[next]] = art.cells()[order[next]];
            next += 1;
        }
        buf.clear();
        // Cursor up `rows` lines, back to column 0. Characters only ever
        // appear, so overwriting each line with its trimmed text is enough.
        buf.push_str(&format!("\x1b[{rows}A\r"));
        for row in shown.chunks(cols) {
            let end = row.iter().rposition(|&c| c != ' ').map_or(0, |i| i + 1);
            buf.extend(&row[..end]);
            buf.push('\n');
        }
        out.write_all(buf.as_bytes())?;
        out.flush()?;
        Ok(next == order.len())
    })
}

/// Streams the text in reading order, frame by frame.
fn stream(art: &AsciiArt, timeline: &RevealTimeline, fps: u32, out: &mut impl Write) -> io::Result<()> {
    let stream: Vec<(u32, char)> = timeline.text_schedule(art).into_iter().zip(art.to_text().chars()).collect();
    let mut next = 0;
    let mut buf = String::new();
    frames(fps, |now_ms| {
        buf.clear();
        while next < stream.len() && u128::from(stream[next].0) <= now_ms {
            buf.push(stream[next].1);
            next += 1;
        }
        out.write_all(buf.as_bytes())?;
        out.flush()?;
        Ok(next == stream.len())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_are_well_formed() {
        use clap::CommandFactory;
        Args::command().debug_assert();
    }

    #[test]
    fn input_is_required() {
        assert!(Args::try_parse_from(["image2ascii"]).is_err());
    }

    fn sample_art() -> AsciiArt {
        let rgba: Vec<u8> = (0..40 * 30).flat_map(|i| [(i * 13 % 256) as u8, 120, (i % 200) as u8, 255]).collect();
        let src = SourceImage::from_rgba8(40, 30, &rgba).unwrap();
        let atlas = GlyphAtlas::from_coverages(&[(' ', 0.0), ('.', 0.1), ('+', 0.4), ('@', 0.9)], 0.5).unwrap();
        image2ascii_core::convert(&src, &atlas, &Options { cols: 20, ..Options::DEFAULT }).unwrap()
    }

    #[test]
    fn sequential_reveals_print_exactly_the_text_output() {
        let art = sample_art();
        for order in [RevealOrder::Typewriter, RevealOrder::Lines] {
            let mut out = Vec::new();
            play(&art, &RevealTimeline::new(&art, order, 30), 240, true, &mut out).unwrap();
            assert_eq!(String::from_utf8(out).unwrap(), art.to_text(), "{order:?}");
        }
    }

    #[test]
    fn scattered_reveals_redraw_in_place_and_end_on_the_art() {
        let art = sample_art();
        for order in [RevealOrder::Random, RevealOrder::Centre] {
            let mut out = Vec::new();
            play(&art, &RevealTimeline::new(&art, order, 30), 240, true, &mut out).unwrap();
            let out = String::from_utf8(out).unwrap();
            let last_frame = format!("\x1b[{}A\r{}", art.rows(), art.to_text());
            assert!(out.ends_with(&last_frame), "{order:?} ends on the finished art");
            assert!(out.starts_with(&"\n".repeat(art.rows())), "{order:?} reserves the lines first");
            // Without cursor movement it falls back to printing the art.
            let mut plain = Vec::new();
            play(&art, &RevealTimeline::new(&art, order, 30), 240, false, &mut plain).unwrap();
            assert_eq!(String::from_utf8(plain).unwrap(), art.to_text());
        }
    }

    #[test]
    fn reveal_orders_parse() {
        let args = Args::try_parse_from(["image2ascii", "x.png", "--reveal-order", "center"]).unwrap();
        assert_eq!(RevealOrder::from(args.reveal_order), RevealOrder::Centre);
        let args = Args::try_parse_from(["image2ascii", "x.png"]).unwrap();
        assert_eq!(RevealOrder::from(args.reveal_order), RevealOrder::Typewriter);
    }

    #[test]
    fn output_formats_come_from_extensions() {
        assert_eq!(format_of(Path::new("a.TXT")).unwrap(), Format::Txt);
        assert_eq!(format_of(Path::new("dir/a.svg")).unwrap(), Format::Svg);
        assert_eq!(format_of(Path::new("a.png")).unwrap(), Format::Png);
        assert!(format_of(Path::new("a.jpg")).is_err());
        assert!(format_of(Path::new("noext")).is_err());
    }

    #[test]
    fn sizes_parse() {
        assert_eq!(parse_size("original").unwrap(), SizeArg::Original);
        assert_eq!(parse_size("1920x1080").unwrap(), SizeArg::Fixed(1920, 1080));
        assert_eq!(parse_size("500×500").unwrap(), SizeArg::Fixed(500, 500));
        assert!(parse_size("500").is_err());
        assert!(parse_size("axb").is_err());
    }

    #[test]
    fn default_canvas_is_the_original_size() {
        let args = Args::try_parse_from(["image2ascii", "x.png"]).unwrap();
        assert_eq!(canvas(&args, (430, 383)).unwrap(), Canvas::Fixed { width: 430, height: 383 });
        let args = Args::try_parse_from(["image2ascii", "x.png", "--scale", "2"]).unwrap();
        assert_eq!(canvas(&args, (430, 383)).unwrap(), Canvas::Fixed { width: 860, height: 766 });
        let args = Args::try_parse_from(["image2ascii", "x.png", "--size", "500x500"]).unwrap();
        assert_eq!(canvas(&args, (50, 100)).unwrap(), Canvas::Fixed { width: 500, height: 500 });
        let args = Args::try_parse_from(["image2ascii", "x.png", "--font-size", "12"]).unwrap();
        assert_eq!(canvas(&args, (50, 100)).unwrap(), Canvas::FontSize(12.0));
        assert!(Args::try_parse_from(["image2ascii", "x.png", "--font-size", "12", "--size", "9x9"]).is_err());
    }

    #[test]
    fn negative_brightness_parses() {
        let args = Args::try_parse_from(["image2ascii", "x.png", "--brightness", "-0.2"]).unwrap();
        assert_eq!(args.brightness, -0.2);
    }
}
