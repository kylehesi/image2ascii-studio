//! Bundled font. JetBrains Mono Regular, © 2020 The JetBrains Mono Project
//! Authors, SIL Open Font License 1.1 — full text in
//! `assets/fonts/JetBrainsMono-OFL.txt`, which must ship alongside any
//! distribution of this font.
//!
//! Embedding a font makes output deterministic: the same image and options
//! give the same art on every machine and in the browser.

pub const JETBRAINS_MONO: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf");

/// The font used when the caller doesn't supply one.
pub const DEFAULT: &[u8] = JETBRAINS_MONO;

#[cfg(test)]
mod tests {
    use crate::{GlyphAtlas, Preset};

    #[test]
    fn bundled_font_calibrates_every_preset() {
        for preset in [Preset::Simple, Preset::Standard, Preset::Structure, Preset::Full] {
            let atlas = GlyphAtlas::new(super::DEFAULT, &preset.chars()).unwrap();
            assert!((0.4..0.7).contains(&atlas.cell_aspect()), "{}", atlas.cell_aspect());
        }
    }
}
