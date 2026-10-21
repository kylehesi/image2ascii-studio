//! Reveal animations: when each cell of an [`AsciiArt`] becomes visible.
//! Renderers (terminal, SVG, web canvas) all play the same timeline, so the
//! animation looks identical wherever it's exported.

use crate::art::AsciiArt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevealTimeline {
    cols: usize,
    duration_ms: u32,
    /// Per cell, row-major; non-decreasing in reading order.
    appear_ms: Vec<u32>,
}

impl RevealTimeline {
    /// Typewriter reveal: characters appear one by one in reading order,
    /// left→right, top→bottom, over `duration_ms`. Blank cells take no time,
    /// so a wide empty background doesn't stall the animation; each blank
    /// shares the time of the character before it.
    pub fn typewriter(art: &AsciiArt, duration_ms: u32) -> Self {
        let inked = art.cells().iter().filter(|&&c| c != ' ').count();
        let step_den = inked.saturating_sub(1).max(1) as u64;
        let mut seen = 0u64;
        let mut t = 0;
        let appear_ms = art
            .cells()
            .iter()
            .map(|&c| {
                if c != ' ' {
                    t = (seen * duration_ms as u64 / step_den) as u32;
                    seen += 1;
                }
                t
            })
            .collect();
        Self { cols: art.cols(), duration_ms, appear_ms }
    }

    pub fn duration_ms(&self) -> u32 {
        self.duration_ms
    }

    /// Appearance time of every cell, row-major.
    pub fn appear_ms(&self) -> &[u32] {
        &self.appear_ms
    }

    pub fn at(&self, col: usize, row: usize) -> u32 {
        self.appear_ms[row * self.cols + col]
    }

    /// Appearance time of every character of `art.to_text()`, in order —
    /// what a player that streams the text needs. A newline shares the time
    /// of the character before it.
    pub fn text_schedule(&self, art: &AsciiArt) -> Vec<u32> {
        let mut out = Vec::with_capacity((art.cols() + 1) * art.rows());
        for r in 0..art.rows() {
            let row = art.row(r);
            let end = row.iter().rposition(|&c| c != ' ').map_or(0, |i| i + 1);
            out.extend((0..end).map(|c| self.at(c, r)));
            out.push(out.last().copied().unwrap_or(0));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typewriter_spreads_ink_evenly_and_skips_blanks() {
        let art = AsciiArt::new(3, 2, vec!['a', ' ', 'b', ' ', ' ', 'c']);
        let tl = RevealTimeline::typewriter(&art, 1000);
        assert_eq!(tl.appear_ms(), &[0, 0, 500, 500, 500, 1000]);
        assert_eq!(tl.at(2, 1), 1000);
    }

    #[test]
    fn text_schedule_lines_up_with_text() {
        let art = AsciiArt::new(3, 3, vec!['a', ' ', ' ', ' ', ' ', ' ', ' ', 'b', ' ']);
        let tl = RevealTimeline::typewriter(&art, 100);
        // "a\n\n b\n": the blank row and the leading blank take no time.
        assert_eq!(art.to_text(), "a\n\n b\n");
        assert_eq!(tl.text_schedule(&art), vec![0, 0, 0, 0, 100, 100]);
        assert_eq!(tl.text_schedule(&art).len(), art.to_text().chars().count());
    }

    #[test]
    fn degenerate_arts_do_not_divide_by_zero() {
        let blank = AsciiArt::new(2, 1, vec![' ', ' ']);
        assert_eq!(RevealTimeline::typewriter(&blank, 500).appear_ms(), &[0, 0]);
        let single = AsciiArt::new(1, 1, vec!['#']);
        assert_eq!(RevealTimeline::typewriter(&single, 500).appear_ms(), &[0]);
    }
}
