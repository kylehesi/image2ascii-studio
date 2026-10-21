//! WebAssembly bridge. The web app runs an [`Engine`] inside a Web Worker:
//! load an image once, then call `convert` as the user moves sliders.
//! Options are plain JS objects; any missing field falls back to the core
//! defaults, so the web app and the CLI agree unless told otherwise.

use std::fmt::Display;

use ascii_core::render::{self, Frame, Rgb, Style, SvgGlyphs, SvgOptions, Typeface};
use ascii_core::{
    AsciiArt, Background, GlyphAtlas, Mode, Options, Prepared, Preset, RevealTimeline, SourceImage, ToneMapping, fonts,
    quality,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

/// Calibrated atlases kept around; switching charsets back and forth is common.
const ATLAS_CACHE: usize = 8;

#[wasm_bindgen(start)]
fn start() {
    console_error_panic_hook::set_once();
}

fn err(e: impl Display) -> JsError {
    JsError::new(&e.to_string())
}

fn from_js<T: DeserializeOwned + Default>(value: JsValue) -> Result<T, JsError> {
    if value.is_undefined() || value.is_null() {
        return Ok(T::default());
    }
    serde_wasm_bindgen::from_value(value).map_err(err)
}

fn unknown(field: &str, value: &str) -> JsError {
    err(format!("unknown {field} {value:?}"))
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct ConvertOptions {
    cols: Option<usize>,
    mode: Option<String>,
    charset: Option<String>,
    /// Custom characters; override `charset` when non-empty.
    chars: Option<String>,
    background: Option<String>,
    tone: Option<String>,
    shape_weight: Option<f32>,
    dither: Option<f32>,
    auto_levels: Option<bool>,
    levels_low: Option<f32>,
    levels_high: Option<f32>,
    clahe: Option<f32>,
    clahe_clip: Option<f32>,
    contrast: Option<f32>,
    brightness: Option<f32>,
    gamma: Option<f32>,
    sharpen: Option<f32>,
}

fn background(name: Option<&str>) -> Result<Background, JsError> {
    match name {
        None | Some("dark") => Ok(Background::Dark),
        Some("light") => Ok(Background::Light),
        Some(other) => Err(unknown("background", other)),
    }
}

impl ConvertOptions {
    fn resolve(&self) -> Result<(Options, String), JsError> {
        let d = Options::DEFAULT;
        let mode = match self.mode.as_deref() {
            None | Some("shape") => Mode::Shape,
            Some("density") => Mode::Density,
            Some(other) => return Err(unknown("mode", other)),
        };
        let tone_mapping = match self.tone.as_deref() {
            None | Some("perceptual") => ToneMapping::Perceptual,
            Some("linear") => ToneMapping::Linear,
            Some(other) => return Err(unknown("tone", other)),
        };
        let charset = match (self.chars.as_deref(), self.charset.as_deref()) {
            (Some(custom), _) if !custom.is_empty() => custom.to_owned(),
            (_, None | Some("auto")) => Preset::for_mode(mode).chars(),
            (_, Some("simple")) => Preset::Simple.chars(),
            (_, Some("standard")) => Preset::Standard.chars(),
            (_, Some("structure")) => Preset::Structure.chars(),
            (_, Some("full")) => Preset::Full.chars(),
            (_, Some(other)) => return Err(unknown("charset", other)),
        };
        let options = Options {
            cols: self.cols.unwrap_or(d.cols),
            mode,
            shape_weight: self.shape_weight.unwrap_or(d.shape_weight),
            dither: self.dither.unwrap_or(d.dither),
            cell_aspect: None,
            background: background(self.background.as_deref())?,
            tone_mapping,
            auto_levels: self.auto_levels.unwrap_or(d.auto_levels),
            levels_low: self.levels_low.unwrap_or(d.levels_low),
            levels_high: self.levels_high.unwrap_or(d.levels_high),
            clahe: self.clahe.unwrap_or(d.clahe),
            clahe_clip: self.clahe_clip.unwrap_or(d.clahe_clip),
            clahe_tiles: d.clahe_tiles,
            contrast: self.contrast.unwrap_or(d.contrast),
            brightness: self.brightness.unwrap_or(d.brightness),
            gamma: self.gamma.unwrap_or(d.gamma),
            sharpen: self.sharpen.unwrap_or(d.sharpen),
        };
        options.validate().map_err(err)?;
        Ok((options, charset))
    }
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct ExportOptions {
    font_size: Option<f32>,
    frame: Option<bool>,
    title: Option<String>,
    fg: Option<String>,
    bg: Option<String>,
    animate: Option<bool>,
    reveal_ms: Option<u32>,
    loop_pause_ms: Option<u32>,
    svg_text: Option<bool>,
}

impl ExportOptions {
    fn style(&self, background: Background) -> Result<Style, JsError> {
        let mut style = Style::for_background(background);
        if let Some(px) = self.font_size {
            style.px = px;
        }
        if let Some(fg) = &self.fg {
            style.fg = Rgb::parse(fg).map_err(err)?;
        }
        if let Some(bg) = &self.bg {
            style.bg = Rgb::parse(bg).map_err(err)?;
        }
        if self.frame.unwrap_or(true) {
            style.frame = Some(Frame { title: self.title.clone().unwrap_or_else(|| "ascii.txt".into()) });
        }
        Ok(style)
    }
}

#[derive(Serialize)]
struct Converted {
    text: String,
    cols: usize,
    rows: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Score {
    ssim: f32,
    tone_error: f32,
}

struct Last {
    art: AsciiArt,
    options: Options,
    charset: String,
}

#[wasm_bindgen]
pub struct Engine {
    face: Typeface,
    source: Option<SourceImage>,
    /// Tone-mapped source, reused while only grid/matching options change.
    prepared: Option<Prepared>,
    atlases: Vec<(String, GlyphAtlas)>,
    last: Option<Last>,
}

impl Engine {
    fn atlas(&mut self, charset: &str) -> Result<&GlyphAtlas, JsError> {
        if let Some(i) = self.atlases.iter().position(|(c, _)| c == charset) {
            // Most recently used last, so eviction drops the stalest.
            let entry = self.atlases.remove(i);
            self.atlases.push(entry);
        } else {
            let atlas = GlyphAtlas::new(fonts::DEFAULT, charset).map_err(err)?;
            if self.atlases.len() == ATLAS_CACHE {
                self.atlases.remove(0);
            }
            self.atlases.push((charset.to_owned(), atlas));
        }
        Ok(&self.atlases.last().expect("just pushed").1)
    }

    fn last(&self) -> Result<&Last, JsError> {
        self.last.as_ref().ok_or_else(|| err("nothing converted yet"))
    }

    fn set_source(&mut self, source: SourceImage) {
        self.source = Some(source);
        self.prepared = None;
        self.last = None;
    }

    /// The prepared image for `options`, re-running tone mapping only when
    /// the tone options changed.
    fn prepare(&mut self, options: &Options) -> Result<&Prepared, JsError> {
        if !self.prepared.as_ref().is_some_and(|p| p.is_for(options)) {
            let source = self.source.as_ref().ok_or_else(|| err("no image loaded"))?;
            self.prepared = Some(Prepared::new(source, options).map_err(err)?);
        }
        Ok(self.prepared.as_ref().expect("just prepared"))
    }
}

#[wasm_bindgen]
impl Engine {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Result<Engine, JsError> {
        Ok(Engine {
            face: Typeface::from_bytes(fonts::DEFAULT).map_err(err)?,
            source: None,
            prepared: None,
            atlases: Vec::new(),
            last: None,
        })
    }

    /// Straight sRGB RGBA pixels, e.g. from `ImageData`.
    #[wasm_bindgen(js_name = loadRgba)]
    pub fn load_rgba(&mut self, width: u32, height: u32, rgba: &[u8]) -> Result<(), JsError> {
        let source = SourceImage::from_rgba8(width as usize, height as usize, rgba).map_err(err)?;
        self.set_source(source);
        Ok(())
    }

    /// An encoded file (PNG, JPEG, WebP, GIF, BMP, TIFF) — the fallback when
    /// the browser can't decode it itself.
    #[wasm_bindgen(js_name = loadEncoded)]
    pub fn load_encoded(&mut self, bytes: &[u8]) -> Result<(), JsError> {
        let source = SourceImage::decode(bytes).map_err(err)?;
        self.set_source(source);
        Ok(())
    }

    /// Working size of the loaded image `[width, height]` (after any downscale).
    #[wasm_bindgen(js_name = sourceSize)]
    pub fn source_size(&self) -> Option<Vec<u32>> {
        self.source.as_ref().map(|s| vec![s.width() as u32, s.height() as u32])
    }

    /// Converts the loaded image. Returns `{ text, cols, rows }`.
    pub fn convert(&mut self, options: JsValue) -> Result<JsValue, JsError> {
        let (options, charset) = from_js::<ConvertOptions>(options)?.resolve()?;
        self.prepare(&options)?;
        self.atlas(&charset)?;
        let atlas = &self.atlases.last().expect("cached").1;
        let prepared = self.prepared.as_ref().expect("prepared");
        let art = ascii_core::convert_prepared(prepared, atlas, &options).map_err(err)?;
        let result = Converted { text: art.to_text(), cols: art.cols(), rows: art.rows() };
        self.last = Some(Last { art, options, charset });
        serde_wasm_bindgen::to_value(&result).map_err(err)
    }

    /// Quality of the last conversion: `{ ssim, toneError }`.
    pub fn score(&mut self) -> Result<JsValue, JsError> {
        let (charset, options) = {
            let last = self.last()?;
            (last.charset.clone(), last.options.clone())
        };
        self.prepare(&options)?;
        self.atlas(&charset)?;
        let atlas = &self.atlases.last().expect("cached").1;
        let (prepared, last) = (self.prepared.as_ref().expect("prepared"), self.last()?);
        let s = quality::evaluate_prepared(prepared, atlas, &last.options, &last.art).map_err(err)?;
        serde_wasm_bindgen::to_value(&Score { ssim: s.ssim, tone_error: s.tone_error }).map_err(err)
    }

    /// Typewriter reveal: appearance time (ms) of each character of the last
    /// result's `text`, in order.
    #[wasm_bindgen(js_name = textSchedule)]
    pub fn text_schedule(&self, duration_ms: u32) -> Result<Vec<u32>, JsError> {
        let art = &self.last()?.art;
        Ok(RevealTimeline::typewriter(art, duration_ms).text_schedule(art))
    }

    /// The last result as SVG (animated unless `animate: false`).
    pub fn svg(&self, export: JsValue) -> Result<String, JsError> {
        let export: ExportOptions = from_js(export)?;
        let last = self.last()?;
        let opts = SvgOptions {
            style: export.style(last.options.background)?,
            glyphs: if export.svg_text.unwrap_or(false) { SvgGlyphs::Text } else { SvgGlyphs::Paths },
            loop_pause_ms: export.loop_pause_ms,
        };
        let timeline = RevealTimeline::typewriter(&last.art, export.reveal_ms.unwrap_or(3000));
        let animation = export.animate.unwrap_or(true).then_some(&timeline);
        render::render_svg(&last.art, &self.face, &opts, animation).map_err(err)
    }

    /// The last result as PNG bytes.
    pub fn png(&self, export: JsValue) -> Result<Vec<u8>, JsError> {
        let export: ExportOptions = from_js(export)?;
        let last = self.last()?;
        render::render_png(&last.art, &self.face, &export.style(last.options.background)?).map_err(err)
    }

    /// `[advance, lineHeight]` of the bundled font at `px`, for laying out
    /// a text preview on exactly the calibrated grid.
    #[wasm_bindgen(js_name = cellSize)]
    pub fn cell_size(&self, px: f32) -> Result<Vec<f32>, JsError> {
        let (w, h) = self.face.cell_size(px).map_err(err)?;
        Ok(vec![w, h])
    }

    /// The core's default conversion options, in the shape `convert` takes,
    /// so the UI never hard-codes its own copy.
    pub fn defaults() -> Result<JsValue, JsError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Defaults {
            cols: usize,
            mode: &'static str,
            charset: &'static str,
            background: &'static str,
            tone: &'static str,
            shape_weight: f32,
            dither: f32,
            auto_levels: bool,
            clahe: f32,
            contrast: f32,
            brightness: f32,
            gamma: f32,
            sharpen: f32,
        }
        let d = Options::DEFAULT;
        let defaults = Defaults {
            cols: d.cols,
            mode: if d.mode == Mode::Shape { "shape" } else { "density" },
            charset: "auto",
            background: if d.background == Background::Dark { "dark" } else { "light" },
            tone: if d.tone_mapping == ToneMapping::Perceptual { "perceptual" } else { "linear" },
            shape_weight: d.shape_weight,
            dither: d.dither,
            auto_levels: d.auto_levels,
            clahe: d.clahe,
            contrast: d.contrast,
            brightness: d.brightness,
            gamma: d.gamma,
            sharpen: d.sharpen,
        };
        serde_wasm_bindgen::to_value(&defaults).map_err(err)
    }

    /// Default `[fg, bg]` colours for a background name ("dark" / "light").
    pub fn palette(name: &str) -> Result<Vec<String>, JsError> {
        let style = Style::for_background(background(Some(name))?);
        Ok(vec![style.fg.hex(), style.bg.hex()])
    }
}
