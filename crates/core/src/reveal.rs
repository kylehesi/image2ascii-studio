//! Reveal animations: when each cell of an [`AsciiArt`] becomes visible.
//! Renderers (terminal, SVG, web canvas) all play the same timeline, so the
//! animation looks identical wherever it's exported.

use crate::art::AsciiArt;

/// The order characters appear in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RevealOrder {
    /// One character at a time, first to last, in reading order.
    #[default]
    Typewriter,
    /// Whole lines, top to bottom.
    Lines,
    /// Characters in a shuffled order (the same shuffle every time).
    Random,
    /// A circle growing outwards from the centre.
    Centre,
}

impl RevealOrder {
    pub const ALL: [Self; 4] = [Self::Typewriter, Self::Lines, Self::Random, Self::Centre];

    pub fn name(self) -> &'static str {
        match self {
            Self::Typewriter => "typewriter",
            Self::Lines => "lines",
            Self::Random => "random",
            Self::Centre => "centre",
        }
    }

    /// Parses [`RevealOrder::name`]; also accepts the spelling "center".
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "center" => Some(Self::Centre),
            _ => Self::ALL.into_iter().find(|o| o.name() == name),
        }
    }

    /// Whether cells appear in reading order, so the text can simply be
    /// streamed (a terminal needs no cursor movement).
    pub fn is_sequential(self) -> bool {
        matches!(self, Self::Typewriter | Self::Lines)
    }
}

/// Typical cell width / height, for making the centre reveal a circle
/// rather than an ellipse. Exact font metrics would change it by a few %.
const CELL_ASPECT: f32 = 0.5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevealTimeline {
    cols: usize,
    order: RevealOrder,
    duration_ms: u32,
    /// Per cell, row-major. For sequential orders it never decreases in
    /// reading order; blank cells share the time of the character before them.
    appear_ms: Vec<u32>,
}

impl RevealTimeline {
    /// Reveals `art` in `order` over `duration_ms`. Only inked cells take
    /// time, so a wide empty background never stalls the animation.
    pub fn new(art: &AsciiArt, order: RevealOrder, duration_ms: u32) -> Self {
        let appear_ms = match order {
            RevealOrder::Typewriter => sequential(art, duration_ms, |i| i),
            RevealOrder::Lines => sequential(art, duration_ms, |i| i / art.cols().max(1)),
            RevealOrder::Random => random(art, duration_ms),
            RevealOrder::Centre => centre(art, duration_ms),
        };
        Self { cols: art.cols(), order, duration_ms, appear_ms }
    }

    /// [`RevealOrder::Typewriter`] — the default reveal.
    pub fn typewriter(art: &AsciiArt, duration_ms: u32) -> Self {
        Self::new(art, RevealOrder::Typewriter, duration_ms)
    }

    pub fn order(&self) -> RevealOrder {
        self.order
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
    /// what a player that streams the text needs. Meaningful for sequential
    /// orders only. A newline shares the time of the character before it.
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

fn inked(art: &AsciiArt) -> impl Iterator<Item = usize> + '_ {
    art.cells().iter().enumerate().filter(|(_, c)| **c != ' ').map(|(i, _)| i)
}

/// Time of the `rank`-th of `steps` evenly spaced steps across `duration`.
fn step_time(rank: usize, steps: usize, duration_ms: u32) -> u32 {
    (rank as u64 * duration_ms as u64 / steps.saturating_sub(1).max(1) as u64) as u32
}

/// Reading-order reveal in steps: `step_of(cell index)` groups cells (each
/// cell for the typewriter, each row for lines); steps containing ink are
/// spread evenly over the duration, blanks share the previous time.
fn sequential(art: &AsciiArt, duration_ms: u32, step_of: impl Fn(usize) -> usize) -> Vec<u32> {
    let mut steps: Vec<usize> = inked(art).map(&step_of).collect();
    steps.dedup();
    let mut rank = 0;
    let mut t = 0;
    (0..art.cells().len())
        .map(|i| {
            if art.cells()[i] != ' ' {
                rank = steps.partition_point(|&s| s < step_of(i));
                t = step_time(rank, steps.len(), duration_ms);
            }
            t
        })
        .collect()
}

/// Inked cells in a fixed pseudo-random order, evenly spaced in time.
fn random(art: &AsciiArt, duration_ms: u32) -> Vec<u32> {
    let mut cells: Vec<usize> = inked(art).collect();
    // Fisher–Yates with SplitMix64: the same art always animates the same way.
    let mut state = 0x9e37_79b9_7f4a_7c15u64 ^ (art.cols() as u64) << 32 ^ art.rows() as u64;
    let mut next = || {
        state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    };
    for i in (1..cells.len()).rev() {
        cells.swap(i, (next() % (i as u64 + 1)) as usize);
    }
    let mut appear = vec![0; art.cells().len()];
    for (rank, &cell) in cells.iter().enumerate() {
        appear[cell] = step_time(rank, cells.len(), duration_ms);
    }
    appear
}

/// Time proportional to distance from the centre: a circle that grows at a
/// constant speed until it reaches the farthest character.
fn centre(art: &AsciiArt, duration_ms: u32) -> Vec<u32> {
    let (cols, rows) = (art.cols() as f32, art.rows() as f32);
    let distance = |i: usize| {
        let (c, r) = ((i % art.cols()) as f32 + 0.5, (i / art.cols()) as f32 + 0.5);
        ((c - cols / 2.0) * CELL_ASPECT).hypot(r - rows / 2.0)
    };
    let max = inked(art).map(distance).fold(0.0f32, f32::max);
    let mut appear = vec![0; art.cells().len()];
    if max > 0.0 {
        for i in inked(art) {
            appear[i] = (distance(i) / max * duration_ms as f32).round() as u32;
        }
    }
    appear
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(cols: usize, rows: usize, f: impl Fn(usize, usize) -> char) -> AsciiArt {
        AsciiArt::new(cols, rows, (0..cols * rows).map(|i| f(i % cols, i / cols)).collect())
    }

    fn inked_times(art: &AsciiArt, tl: &RevealTimeline) -> Vec<u32> {
        inked(art).map(|i| tl.appear_ms()[i]).collect()
    }

    #[test]
    fn typewriter_spreads_ink_evenly_and_skips_blanks() {
        let art = AsciiArt::new(3, 2, vec!['a', ' ', 'b', ' ', ' ', 'c']);
        let tl = RevealTimeline::typewriter(&art, 1000);
        assert_eq!(tl.appear_ms(), &[0, 0, 500, 500, 500, 1000]);
        assert_eq!(tl.at(2, 1), 1000);
    }

    #[test]
    fn lines_reveal_whole_rows_evenly() {
        // Rows 0, 2 and 3 have ink; row 1 is blank and takes no time.
        let art = grid(3, 4, |c, r| if r != 1 && c != 1 { '#' } else { ' ' });
        let tl = RevealTimeline::new(&art, RevealOrder::Lines, 1000);
        for r in 0..4 {
            let row: Vec<u32> = (0..3).map(|c| tl.at(c, r)).collect();
            assert!(row.windows(2).all(|w| w[0] == w[1]), "row {r} appears at once: {row:?}");
        }
        assert_eq!([tl.at(0, 0), tl.at(0, 1), tl.at(0, 2), tl.at(0, 3)], [0, 0, 500, 1000]);
    }

    #[test]
    fn sequential_orders_never_go_back_in_reading_order() {
        let art = grid(7, 5, |c, r| if (c * 3 + r) % 4 == 0 { ' ' } else { 'x' });
        for order in [RevealOrder::Typewriter, RevealOrder::Lines] {
            let tl = RevealTimeline::new(&art, order, 2000);
            assert!(order.is_sequential());
            assert!(tl.appear_ms().windows(2).all(|w| w[0] <= w[1]), "{order:?}");
            assert_eq!(*tl.appear_ms().last().unwrap(), 2000);
        }
    }

    #[test]
    fn random_is_a_deterministic_shuffle_of_even_steps() {
        let art = grid(20, 10, |_, _| '#');
        let tl = RevealTimeline::new(&art, RevealOrder::Random, 1990);
        let mut times = inked_times(&art, &tl);
        assert_ne!(times, RevealTimeline::typewriter(&art, 1990).appear_ms(), "not reading order");
        assert_eq!(tl, RevealTimeline::new(&art, RevealOrder::Random, 1990), "same every time");
        times.sort_unstable();
        let even: Vec<u32> = (0..200).map(|k| step_time(k, 200, 1990)).collect();
        assert_eq!(times, even, "each step used exactly once");
        assert!(!RevealOrder::Random.is_sequential());
    }

    #[test]
    fn centre_grows_outwards() {
        let art = grid(21, 11, |_, _| '#');
        let tl = RevealTimeline::new(&art, RevealOrder::Centre, 1000);
        let middle = tl.at(10, 5);
        assert_eq!(middle, *tl.appear_ms().iter().min().unwrap(), "the centre comes first");
        assert_eq!(*tl.appear_ms().iter().max().unwrap(), 1000, "the farthest ink lands at the end");
        assert_eq!(tl.at(0, 0), 1000);
        assert!(tl.at(5, 5) < tl.at(0, 5) && tl.at(10, 2) < tl.at(10, 0), "farther is later");
        // Cells are about twice as tall as wide: 2 columns ≈ 1 row of distance.
        assert!(tl.at(14, 5).abs_diff(tl.at(10, 3)) <= 10, "{} vs {}", tl.at(14, 5), tl.at(10, 3));
    }

    #[test]
    fn names_round_trip() {
        for order in RevealOrder::ALL {
            assert_eq!(RevealOrder::from_name(order.name()), Some(order));
        }
        assert_eq!(RevealOrder::from_name("center"), Some(RevealOrder::Centre));
        assert_eq!(RevealOrder::from_name("spiral"), None);
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
        let single = AsciiArt::new(1, 1, vec!['#']);
        for order in RevealOrder::ALL {
            assert_eq!(RevealTimeline::new(&blank, order, 500).appear_ms(), &[0, 0], "{order:?}");
            assert_eq!(RevealTimeline::new(&single, order, 500).appear_ms(), &[0], "{order:?}");
        }
    }
}
