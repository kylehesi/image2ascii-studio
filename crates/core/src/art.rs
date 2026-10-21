/// A finished character grid, row-major.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsciiArt {
    cols: usize,
    rows: usize,
    cells: Vec<char>,
}

impl AsciiArt {
    pub(crate) fn new(cols: usize, rows: usize, cells: Vec<char>) -> Self {
        debug_assert_eq!(cells.len(), cols * rows);
        Self { cols, rows, cells }
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn cells(&self) -> &[char] {
        &self.cells
    }

    pub fn row(&self, r: usize) -> &[char] {
        &self.cells[r * self.cols..(r + 1) * self.cols]
    }

    /// Plain text: one line per row, trailing spaces trimmed, `\n` endings.
    pub fn to_text(&self) -> String {
        let mut out = String::with_capacity((self.cols + 1) * self.rows);
        for r in 0..self.rows {
            let row = self.row(r);
            let end = row.iter().rposition(|&c| c != ' ').map_or(0, |i| i + 1);
            out.extend(&row[..end]);
            out.push('\n');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_trims_trailing_spaces() {
        let art = AsciiArt::new(3, 2, vec!['a', ' ', ' ', ' ', 'b', ' ']);
        assert_eq!(art.to_text(), "a\n b\n");
    }
}
