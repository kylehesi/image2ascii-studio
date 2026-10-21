//! SVG export. Self-contained (no scripts, no external fonts or images), so
//! it works as an `<img>` in a GitHub README, where the reveal animation
//! plays too.
//!
//! The animation plays a [`RevealTimeline`] in any order: characters that
//! appear within the same 1/60 s frame are grouped and the group is switched
//! on at that moment, which keeps the file small without changing what you see.

use std::collections::{BTreeMap, HashMap};
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

    let mut glyph = |ch: char| define(ch, lay.font_px, &mut ids);
    let body = match timeline.filter(|t| t.duration_ms() > 0) {
        Some(tl) => animated_body(art, &lay, opts, tl, &mut glyph),
        None => static_body(art, &lay, opts, &mut glyph),
    };

    if !defs.is_empty() {
        s.push_str("<defs>");
        s.push_str(&defs);
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
    s.push('>');
    s.push_str(&body);
    s.push_str("</g></svg>\n");
    Ok(s)
}

const FONTS: &str = "'JetBrains Mono','Cascadia Mono',Consolas,'SFMono-Regular',Menlo,'DejaVu Sans Mono',monospace";

/// The finished art, one group (paths) or `<text>` (text) per inked row.
fn static_body(art: &AsciiArt, lay: &Layout, opts: &SvgOptions, glyph: &mut dyn FnMut(char) -> Option<String>) -> String {
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
                    if let Some(id) = glyph(ch) {
                        let _ = write!(body, r##"<use href="#{id}" x="{}"/>"##, num(c as f32 * lay.cell_w));
                    }
                }
                body.push_str("</g>");
            }
            SvgGlyphs::Text => text_run(&mut body, lay, row, 0, row.len(), y),
        }
    }
    body
}

/// The art revealed over time, in any [`RevealOrder`](crate::RevealOrder):
/// characters appearing in the same 1/60 s frame share a group whose opacity
/// switches from 0 to 1 at that moment (SMIL `<animate calcMode="discrete">`).
/// A group's static opacity is 1, so viewers without SMIL show the finished art.
fn animated_body(
    art: &AsciiArt,
    lay: &Layout,
    opts: &SvgOptions,
    timeline: &RevealTimeline,
    glyph: &mut dyn FnMut(char) -> Option<String>,
) -> String {
    let cycle = timeline.duration_ms() as f64 + opts.loop_pause_ms.unwrap_or(0) as f64;
    let repeat = if opts.loop_pause_ms.is_some() { r#"repeatCount="indefinite""# } else { r#"fill="freeze""# };

    // Inked cells per frame, each frame's cells in reading order.
    let mut frames: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
    for (i, &ch) in art.cells().iter().enumerate() {
        if ch != ' ' {
            let frame = (timeline.appear_ms()[i] as f64 / FRAME_MS).round() as u64;
            frames.entry(frame).or_default().push(i);
        }
    }

    let mut body = String::new();
    for (frame, cells) in frames {
        body.push_str("<g>");
        if frame > 0 {
            let t = (frame as f64 * FRAME_MS / cycle).min(1.0);
            let _ = write!(
                body,
                r#"<animate attributeName="opacity" calcMode="discrete" dur="{}ms" {repeat} values="0;1" keyTimes="0;{}"/>"#,
                cycle.round() as u64,
                // 5 places keep consecutive 1/60 s frames distinct up to ~10 minute cycles.
                trim(format!("{t:.5}")),
            );
        }
        match opts.glyphs {
            SvgGlyphs::Paths => {
                for i in cells {
                    let (c, r) = (i % art.cols(), i / art.cols());
                    if let Some(id) = glyph(art.cells()[i]) {
                        let (x, y) = (c as f32 * lay.cell_w, r as f32 * lay.cell_h + lay.baseline);
                        let _ = write!(body, r##"<use href="#{id}" x="{}" y="{}"/>"##, num(x), num(y));
                    }
                }
            }
            SvgGlyphs::Text => {
                // Consecutive cells of one row become one run of text.
                let mut k = 0;
                while k < cells.len() {
                    let start = cells[k];
                    let mut end = start + 1;
                    while k + 1 < cells.len() && cells[k + 1] == end && end % art.cols() != 0 {
                        k += 1;
                        end += 1;
                    }
                    let r = start / art.cols();
                    let row = art.row(r);
                    let y = r as f32 * lay.cell_h + lay.baseline;
                    text_run(&mut body, lay, row, start % art.cols(), (end - 1) % art.cols() + 1, y);
                    k += 1;
                }
            }
        }
        body.push_str("</g>");
    }
    body
}

/// `<text>` for columns `from..to` of a row, stretched to exactly its cells.
fn text_run(body: &mut String, lay: &Layout, row: &[char], from: usize, to: usize, y: f32) {
    let text: String = row[from..to].iter().collect();
    let _ = write!(
        body,
        r#"<text x="{}" y="{}" textLength="{}" lengthAdjust="spacing" xml:space="preserve">{}</text>"#,
        num(from as f32 * lay.cell_w),
        num(y),
        num((to - from) as f32 * lay.cell_w),
        escape(&text)
    );
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

    /// For each glyph drawn: (column, row, keyTime it switches on).
    fn reveal_times(doc: &roxmltree::Document, lay_w: f32, lay_h: f32, baseline: f32) -> Vec<(usize, usize, f32)> {
        let mut out = Vec::new();
        for g in doc.descendants().filter(|n| n.has_tag_name("g") && n.children().any(|c| c.has_tag_name("use"))) {
            let anim = g.children().find(|c| c.has_tag_name("animate"));
            let t = anim.map_or(0.0, |a| {
                let times: Vec<f32> = a.attribute("keyTimes").unwrap().split(';').map(|t| t.parse().unwrap()).collect();
                assert_eq!(a.attribute("values"), Some("0;1"));
                assert_eq!(times[0], 0.0, "discrete animations must start at keyTime 0");
                assert!(times[1] > 0.0 && times[1] <= 1.0, "{times:?}");
                times[1]
            });
            for u in g.children().filter(|c| c.has_tag_name("use")) {
                let x: f32 = u.attribute("x").unwrap().parse().unwrap();
                let y: f32 = u.attribute("y").unwrap().parse().unwrap();
                out.push(((x / lay_w).round() as usize, ((y - baseline) / lay_h).round() as usize, t));
            }
        }
        out
    }

    #[test]
    fn animation_plays_every_reveal_order() {
        let art = art();
        let o = opts(SvgGlyphs::Paths, false);
        let lay = Layout::new(&art, &Typeface::boxes(), &o.style, false).unwrap();
        let inked = art.cells().iter().filter(|&&c| c != ' ').count();
        for order in crate::RevealOrder::ALL {
            let timeline = RevealTimeline::new(&art, order, 2000);
            let svg = render_svg(&art, &Typeface::boxes(), &o, Some(&timeline)).unwrap();
            let doc = parse(&svg);
            let shown = reveal_times(&doc, lay.cell_w, lay.cell_h, lay.baseline);
            assert_eq!(shown.len(), inked, "{order:?}: every inked cell drawn exactly once");
            for &(c, r, t) in &shown {
                assert_ne!(art.row(r)[c], ' ', "{order:?}: glyph lands on an inked cell");
                // Each glyph switches on in the 1/60 s frame its timeline says.
                let expected = timeline.at(c, r) as f32 / 2000.0;
                assert!((t - expected).abs() <= 1.0 / 60.0 / 2.0 + 1e-3, "{order:?} ({c},{r}): {t} vs {expected}");
            }
            assert!(shown.iter().any(|s| (s.2 - 1.0).abs() < 1e-3), "{order:?}: last character lands at the end");
            for a in doc.descendants().filter(|n| n.has_tag_name("animate")) {
                assert_eq!(a.attribute("fill"), Some("freeze"));
                assert_eq!(a.attribute("dur"), Some("2000ms"));
            }
        }
    }

    #[test]
    fn text_mode_animates_runs_of_cells() {
        let art = art();
        for order in [crate::RevealOrder::Typewriter, crate::RevealOrder::Random] {
            let timeline = RevealTimeline::new(&art, order, 1500);
            let svg = render_svg(&art, &Typeface::boxes(), &opts(SvgGlyphs::Text, false), Some(&timeline)).unwrap();
            let doc = parse(&svg);
            // All the art's characters are present across the runs, none twice.
            let drawn: usize = doc
                .descendants()
                .filter(|n| n.has_tag_name("text"))
                .map(|t| t.text().unwrap_or("").chars().filter(|&c| c != ' ').count())
                .sum();
            assert_eq!(drawn, art.cells().iter().filter(|&&c| c != ' ').count(), "{order:?}");
        }
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
