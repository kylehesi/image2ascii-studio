//! SVG export. Self-contained (no scripts, no external fonts or images), so
//! it works as an `<img>` in a GitHub README, where the reveal animation
//! plays too.
//!
//! The animation is a clip path with one rectangle per row whose width steps
//! along that row's [`RevealTimeline`] (SMIL `<animate calcMode="discrete">`).
//! Characters that appear within the same 1/60 s frame are merged into one
//! step, which keeps the file small without changing what you see.

use std::collections::HashMap;
use std::fmt::Write as _;

use super::{Layout, Style, Typeface, num};
use crate::error::Result;
use crate::{AsciiArt, RevealTimeline};

const FRAME_MS: f64 = 1000.0 / 60.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SvgGlyphs {
    /// Glyph outlines from the font, defined once and reused: looks exactly
    /// like the font the tones were calibrated with, on any viewer.
    #[default]
    Paths,
    /// Plain `<text>` rows: smallest file and selectable text, but drawn in
    /// whatever monospace font the viewer has.
    Text,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SvgOptions {
    pub style: Style,
    pub glyphs: SvgGlyphs,
    /// Replay the reveal after holding the finished art this long. `None`
    /// plays once and stays.
    pub loop_pause_ms: Option<u32>,
}

/// Renders `art` as SVG. With a `timeline` (and a non-zero duration) the
/// characters are revealed over time; without one the SVG is static.
pub fn render_svg(art: &AsciiArt, face: &Typeface, opts: &SvgOptions, timeline: Option<&RevealTimeline>) -> Result<String> {
    let style = &opts.style;
    let lay = Layout::new(art, face, style, false)?;
    let mut s = String::with_capacity(art.cols() * art.rows() * 24 + 4096);
    let _ = write!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">"#,
        w = num(lay.width),
        h = num(lay.height)
    );

    // Background and window chrome.
    match &lay.frame {
        Some(f) => {
            let _ = write!(
                s,
                r#"<rect x="0.5" y="0.5" width="{}" height="{}" rx="{}" fill="{}" stroke="{}"/>"#,
                num(lay.width - 1.0),
                num(lay.height - 1.0),
                num(f.radius),
                style.bg.hex(),
                f.border.hex()
            );
            for (x, y, c) in f.dots {
                let _ = write!(s, r#"<circle cx="{}" cy="{}" r="{}" fill="{}"/>"#, num(x), num(y), num(f.dot_radius), c.hex());
            }
        }
        None => {
            let _ = write!(s, r#"<rect width="100%" height="100%" fill="{}"/>"#, style.bg.hex());
        }
    }

    // Glyph outlines, defined once each (art glyphs and title glyphs share ids).
    let mut ids: HashMap<(char, u32), String> = HashMap::new();
    let mut defs = String::new();
    let mut define = |ch: char, px: f32, ids: &mut HashMap<(char, u32), String>| -> Option<String> {
        let key = (ch, px.to_bits());
        if let Some(id) = ids.get(&key) {
            return Some(id.clone());
        }
        let d = face.outline(ch, px)?;
        let id = short_id(ids.len());
        let _ = write!(defs, r#"<path id="{id}" d="{d}"/>"#);
        ids.insert(key, id.clone());
        Some(id)
    };

    let mut title = String::new();
    if let Some(f) = &lay.frame {
        match opts.glyphs {
            SvgGlyphs::Paths => {
                let total: f32 = f.title.chars().map(|c| face.advance(c, f.title_px)).sum();
                let mut x = f.title_centre_x - total / 2.0;
                let _ = write!(title, r#"<g fill="{}">"#, f.title_color.hex());
                for ch in f.title.chars() {
                    if let Some(id) = define(ch, f.title_px, &mut ids) {
                        let _ = write!(title, r##"<use href="#{id}" x="{}" y="{}"/>"##, num(x), num(f.title_baseline));
                    }
                    x += face.advance(ch, f.title_px);
                }
                title.push_str("</g>");
            }
            SvgGlyphs::Text => {
                let _ = write!(
                    title,
                    r#"<text x="{}" y="{}" text-anchor="middle" font-family="{FONTS}" font-size="{}" fill="{}">{}</text>"#,
                    num(f.title_centre_x),
                    num(f.title_baseline),
                    num(f.title_px),
                    f.title_color.hex(),
                    escape(&f.title)
                );
            }
        }
    }

    let mut body = String::new();
    for r in 0..art.rows() {
        let row = art.row(r);
        if row.iter().all(|&c| c == ' ') {
            continue;
        }
        let y = r as f32 * lay.cell_h + lay.baseline;
        match opts.glyphs {
            SvgGlyphs::Paths => {
                let _ = write!(body, r#"<g transform="translate(0 {})">"#, num(y));
                for (c, &ch) in row.iter().enumerate() {
                    if ch == ' ' {
                        continue;
                    }
                    if let Some(id) = define(ch, lay.font_px, &mut ids) {
                        let _ = write!(body, r##"<use href="#{id}" x="{}"/>"##, num(c as f32 * lay.cell_w));
                    }
                }
                body.push_str("</g>");
            }
            SvgGlyphs::Text => {
                let text: String = row.iter().collect();
                let _ = write!(
                    body,
                    r#"<text y="{}" textLength="{}" lengthAdjust="spacing" xml:space="preserve">{}</text>"#,
                    num(y),
                    num(art.cols() as f32 * lay.cell_w),
                    escape(&text)
                );
            }
        }
    }

    let clip = timeline.filter(|t| t.duration_ms() > 0).map(|t| reveal_clip(art, &lay, t, opts.loop_pause_ms));
    if !defs.is_empty() || clip.is_some() {
        s.push_str("<defs>");
        s.push_str(&defs);
        if let Some(clip) = &clip {
            s.push_str(clip);
        }
        s.push_str("</defs>");
    }
    s.push_str(&title);

    let _ = write!(s, r#"<g transform="translate({} {})" fill="{}""#, num(lay.origin.0), num(lay.origin.1), style.fg.hex());
    if opts.glyphs == SvgGlyphs::Text {
        // Coding fonts merge `->`, `==` … into ligatures, which would break the grid.
        let _ = write!(
            s,
            r#" font-family="{FONTS}" font-size="{}" style="font-variant-ligatures:none;font-feature-settings:'liga' 0,'calt' 0""#,
            num(lay.font_px)
        );
    }
    if clip.is_some() {
        s.push_str(r#" clip-path="url(#reveal)""#);
    }
    s.push('>');
    s.push_str(&body);
    s.push_str("</g></svg>\n");
    Ok(s)
}

const FONTS: &str = "'JetBrains Mono','Cascadia Mono',Consolas,'SFMono-Regular',Menlo,'DejaVu Sans Mono',monospace";

/// One clip rectangle per inked row; its width steps from 0 to the full row
/// as the row's characters appear. The rectangle's static width is the full
/// row, so viewers without SMIL show the finished art.
fn reveal_clip(art: &AsciiArt, lay: &Layout, timeline: &RevealTimeline, loop_pause_ms: Option<u32>) -> String {
    let duration = timeline.duration_ms() as f64;
    let cycle = duration + loop_pause_ms.unwrap_or(0) as f64;
    let full = art.cols() as f32 * lay.cell_w;
    let repeat = if loop_pause_ms.is_some() { r#"repeatCount="indefinite""# } else { r#"fill="freeze""# };

    let mut s = String::from(r#"<clipPath id="reveal">"#);
    for r in 0..art.rows() {
        // (frame, revealed columns) steps, merged per frame.
        let mut steps: Vec<(u64, usize)> = Vec::new();
        for (c, &ch) in art.row(r).iter().enumerate() {
            if ch == ' ' {
                continue;
            }
            let frame = (timeline.at(c, r) as f64 / FRAME_MS).round() as u64;
            match steps.last_mut() {
                Some(last) if last.0 == frame => last.1 = c + 1,
                _ => steps.push((frame, c + 1)),
            }
        }
        if steps.is_empty() {
            continue;
        }
        if steps[0].0 > 0 {
            steps.insert(0, (0, 0));
        }
        let mut values = String::new();
        let mut times = String::new();
        for (i, &(frame, cols)) in steps.iter().enumerate() {
            if i > 0 {
                values.push(';');
                times.push(';');
            }
            values.push_str(&num(cols as f32 * lay.cell_w));
            let t = (frame as f64 * FRAME_MS / cycle).min(1.0);
            // 5 places keep consecutive 1/60 s frames distinct up to ~10 minute cycles.
            let _ = write!(times, "{}", trim(format!("{t:.5}")));
        }
        let _ = write!(
            s,
            r#"<rect y="{}" width="{}" height="{}"><animate attributeName="width" calcMode="discrete" dur="{}ms" {repeat} values="{values}" keyTimes="{times}"/></rect>"#,
            num(r as f32 * lay.cell_h),
            num(full),
            num(lay.cell_h),
            cycle.round() as u64,
        );
    }
    s.push_str("</clipPath>");
    s
}

fn trim(s: String) -> String {
    let t = s.trim_end_matches('0').trim_end_matches('.');
    if t.is_empty() { "0".to_owned() } else { t.to_owned() }
}

/// a, b, …, z, aa, ab, … — short ids keep thousands of `<use>`s small.
fn short_id(mut n: usize) -> String {
    let mut id = Vec::new();
    loop {
        id.push(b'a' + (n % 26) as u8);
        n /= 26;
        if n == 0 {
            break;
        }
        n -= 1;
    }
    id.reverse();
    String::from_utf8(id).expect("ascii")
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::{Frame, Rgb};
    use crate::{Background, GlyphAtlas, Options, SourceImage};

    fn art() -> AsciiArt {
        // A diagonal-ish image through a coverage atlas: mixes ink and blanks.
        let rgba: Vec<u8> = (0..24 * 12).flat_map(|i| {
            let v = if (i % 24) > (i / 24) * 2 { 230 } else { 10 };
            [v, v, v, 255]
        }).collect();
        let src = SourceImage::from_rgba8(24, 12, &rgba).unwrap();
        let atlas = GlyphAtlas::from_coverages(&[(' ', 0.0), ('<', 0.3), ('&', 0.6), ('#', 0.9)], 0.5).unwrap();
        crate::convert(&src, &atlas, &Options { cols: 12, ..Options::DEFAULT }).unwrap()
    }

    fn opts(glyphs: SvgGlyphs, frame: bool) -> SvgOptions {
        let mut style = Style::for_background(Background::Dark);
        if frame {
            style.frame = Some(Frame { title: "a <b> & \"c\"".into() });
        }
        SvgOptions { style, glyphs, loop_pause_ms: None }
    }

    fn parse(svg: &str) -> roxmltree::Document<'_> {
        roxmltree::Document::parse(svg).unwrap_or_else(|e| panic!("invalid SVG: {e}\n{svg}"))
    }

    #[test]
    fn paths_mode_uses_one_glyph_per_inked_cell() {
        let art = art();
        let svg = render_svg(&art, &Typeface::boxes(), &opts(SvgGlyphs::Paths, false), None).unwrap();
        let doc = parse(&svg);
        let inked = art.cells().iter().filter(|&&c| c != ' ').count();
        let uses = doc.descendants().filter(|n| n.has_tag_name("use")).count();
        assert_eq!(uses, inked);
        // One shared definition per distinct glyph.
        let distinct = art.cells().iter().filter(|&&c| c != ' ').collect::<std::collections::HashSet<_>>().len();
        assert_eq!(doc.descendants().filter(|n| n.has_tag_name("path")).count(), distinct);
        assert!(!svg.contains("<animate"), "no timeline, no animation");
    }

    #[test]
    fn text_mode_escapes_and_keeps_columns() {
        let art = art();
        let svg = render_svg(&art, &Typeface::boxes(), &opts(SvgGlyphs::Text, true), None).unwrap();
        let doc = parse(&svg);
        let texts: Vec<_> = doc.descendants().filter(|n| n.has_tag_name("text")).collect();
        // Title first, then one per inked row, each the full untrimmed row.
        assert_eq!(texts[0].text(), Some("a <b> & \"c\""));
        let row = texts[1].text().unwrap();
        assert_eq!(row.chars().count(), art.cols());
        assert!(texts[1].attribute("textLength").is_some());
    }

    #[test]
    fn frame_adds_chrome() {
        let svg = render_svg(&art(), &Typeface::boxes(), &opts(SvgGlyphs::Paths, true), None).unwrap();
        let doc = parse(&svg);
        assert_eq!(doc.descendants().filter(|n| n.has_tag_name("circle")).count(), 3);
        let bg = doc.descendants().find(|n| n.has_tag_name("rect")).unwrap();
        assert!(bg.attribute("rx").is_some());
    }

    #[test]
    fn animation_follows_the_timeline() {
        let art = art();
        let timeline = RevealTimeline::typewriter(&art, 2000);
        let svg = render_svg(&art, &Typeface::boxes(), &opts(SvgGlyphs::Paths, false), Some(&timeline)).unwrap();
        let doc = parse(&svg);
        let anims: Vec<_> = doc.descendants().filter(|n| n.has_tag_name("animate")).collect();
        let inked_rows = (0..art.rows()).filter(|&r| art.row(r).iter().any(|&c| c != ' ')).count();
        assert_eq!(anims.len(), inked_rows);

        let mut last_end = 0.0f32;
        for a in &anims {
            let times: Vec<f32> = a.attribute("keyTimes").unwrap().split(';').map(|t| t.parse().unwrap()).collect();
            let values: Vec<f32> = a.attribute("values").unwrap().split(';').map(|v| v.parse().unwrap()).collect();
            assert_eq!(times.len(), values.len());
            assert_eq!(times[0], 0.0, "discrete animations must start at keyTime 0");
            assert!(times.windows(2).all(|w| w[0] < w[1]), "{times:?}");
            assert!(values.windows(2).all(|w| w[0] < w[1]), "{values:?}");
            assert!(times.iter().all(|t| (0.0..=1.0).contains(t)));
            // Rows reveal in order: a row starts no earlier than the previous one finished.
            let start = if values[0] == 0.0 { times[1] } else { times[0] };
            assert!(start + 1e-3 >= last_end, "row starts at {start} before previous ended at {last_end}");
            last_end = *times.last().unwrap();
            assert_eq!(a.attribute("fill"), Some("freeze"));
        }
        assert!((last_end - 1.0).abs() < 1e-3, "last character lands at the end: {last_end}");
    }

    #[test]
    fn looping_animation_repeats_with_a_pause() {
        let art = art();
        let timeline = RevealTimeline::typewriter(&art, 1000);
        let o = SvgOptions { loop_pause_ms: Some(1000), ..opts(SvgGlyphs::Paths, false) };
        let svg = render_svg(&art, &Typeface::boxes(), &o, Some(&timeline)).unwrap();
        let doc = parse(&svg);
        let a = doc.descendants().rfind(|n| n.has_tag_name("animate")).unwrap();
        assert_eq!(a.attribute("repeatCount"), Some("indefinite"));
        assert_eq!(a.attribute("dur"), Some("2000ms"));
        let last: f32 = a.attribute("keyTimes").unwrap().split(';').next_back().unwrap().parse().unwrap();
        assert!((last - 0.5).abs() < 0.01, "reveal ends halfway through the cycle: {last}");
    }

    #[test]
    fn colours_come_from_style() {
        let mut o = opts(SvgGlyphs::Paths, false);
        o.style.fg = Rgb(1, 2, 3);
        let svg = render_svg(&art(), &Typeface::boxes(), &o, None).unwrap();
        assert!(svg.contains(r##"fill="#010203""##));
    }

    #[test]
    fn short_ids_are_unique() {
        let ids: std::collections::HashSet<_> = (0..2000).map(short_id).collect();
        assert_eq!(ids.len(), 2000);
        assert_eq!(short_id(0), "a");
        assert_eq!(short_id(26), "aa");
    }
}
