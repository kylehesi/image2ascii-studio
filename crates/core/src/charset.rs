//! Character sets. Order doesn't matter: the atlas measures each glyph's
//! actual ink coverage in the chosen font and sorts by that.

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Preset {
    /// Ten classic density characters.
    Simple,
    /// Vertically balanced glyphs at roughly even density steps. Symbols
    /// alone leave a gap between `*` and `%` (about a third of the tonal
    /// range in typical monospace fonts), so a few compact letters fill it.
    /// The default for density matching.
    #[default]
    Standard,
    /// `Standard` plus directional and off-centre glyphs (`/ \ | _ ( ) ' ,`
    /// …). They read as noise when placed by tone alone, but shape matching
    /// puts them exactly on edges. The default for shape matching.
    Structure,
    /// Every printable ASCII character — the most tonal levels.
    Full,
}

const SIMPLE: &str = " .:-=+*#%@";
// Chosen with `examples/glyphs.rs`: ink centred vertically (no top- or
// bottom-only marks like ' ` , ^ that read as texture) and evenly spaced levels.
const STANDARD: &str = " .:-~+=*xoXH%#8&$@";
const STRUCTURE_EXTRA: &str = "',`_^\"/\\|()[]<>!;";

impl Preset {
    pub fn chars(self) -> String {
        match self {
            Preset::Simple => SIMPLE.to_owned(),
            Preset::Standard => STANDARD.to_owned(),
            Preset::Structure => format!("{STANDARD}{STRUCTURE_EXTRA}"),
            Preset::Full => (0x20u8..=0x7e).map(char::from).collect(),
        }
    }

    /// The preset that suits a matching mode best.
    pub fn for_mode(mode: crate::Mode) -> Self {
        match mode {
            crate::Mode::Density => Preset::Standard,
            crate::Mode::Shape => Preset::Structure,
        }
    }
}

/// Deduplicates (keeping first occurrence) and validates a charset.
pub(crate) fn parse(charset: &str) -> Result<Vec<char>> {
    let mut chars = Vec::new();
    for ch in charset.chars() {
        if ch.is_control() {
            return Err(Error::Charset(format!("control character {ch:?} is not printable")));
        }
        if !chars.contains(&ch) {
            chars.push(ch);
        }
    }
    if chars.len() < 2 {
        return Err(Error::Charset("needs at least two distinct characters".into()));
    }
    Ok(chars)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_is_all_printable_ascii() {
        let full = Preset::Full.chars();
        assert_eq!(full.len(), 95);
        assert!(full.starts_with(' ') && full.ends_with('~'));
    }

    #[test]
    fn parse_dedupes_and_validates() {
        assert_eq!(parse("aab a").unwrap(), vec!['a', 'b', ' ']);
        assert!(parse("x").is_err());
        assert!(parse("ab\n").is_err());
    }
}
