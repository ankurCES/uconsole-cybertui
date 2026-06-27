//! ANSI / VT100 parser. Converts raw bytes from a child PTY into a `Grid`
//! of `Cell`s that ratatui can render. Implements the `vte::Perform` trait
//! for printable ASCII, CSI SGR (colours + attrs), CSI cursor moves (CUP,
//! CUF, CUB, CUD, CUU), line feeds, carriage returns, and the clears
//! (ED, EL). Enough for htop, vim, less, top, ssh, etc. — anything that
//! uses the standard terminal escape sequences.
//!
//! State machine lives on the `Parser`; presentation lives on the
//! `Performer`. One `Parser`, one `Performer` per PTY session.

//! Phase-2 module: PTY/ANSI rendering. Wired up by `wm::pty::Pty` once
//! the pane-grid lands (see ROADMAP.md).
#![allow(dead_code)]

use ratatui::style::{Color, Modifier};
use vte::{Parser, Perform};

/// A single character cell in the rendered grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub mods: Modifier,
}

impl Cell {
    fn blank(bg: Color) -> Self {
        Self { ch: ' ', fg: Color::Reset, bg, mods: Modifier::empty() }
    }
}

impl Default for Cell {
    fn default() -> Self { Self::blank(Color::Reset) }
}

/// A fixed-size grid of cells. Width/height are set by `resize`; writes
/// outside the grid are clipped (a real terminal scrolls, but for v0 we
/// just drop).
#[derive(Debug, Clone)]
pub struct Grid {
    pub rows: u16,
    pub cols: u16,
    cells: Vec<Cell>,
    bg: Color,
}

impl Grid {
    pub fn new(rows: u16, cols: u16) -> Self {
        let bg = Color::Reset;
        let cells = (0..rows as usize).flat_map(|_| (0..cols as usize).map(|_| Cell::blank(bg))).collect();
        Self { rows, cols, cells, bg }
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        if rows == self.rows && cols == self.cols { return; }
        // Build a fresh grid; copy what fits. Anything that doesn't fit
        // is dropped — same as a real terminal in the no-scrollback case.
        let bg = self.bg;
        let mut next = Self::new(rows, cols);
        next.bg = bg;
        let r = rows.min(self.rows) as usize;
        let c = cols.min(self.cols) as usize;
        for y in 0..r {
            for x in 0..c {
                next.cells[y * cols as usize + x] =
                    self.cells[y * self.cols as usize + x].clone();
            }
        }
        *self = next;
    }

    pub fn cell(&self, row: u16, col: u16) -> Option<&Cell> {
        if row >= self.rows || col >= self.cols { return None; }
        Some(&self.cells[row as usize * self.cols as usize + col as usize])
    }

    pub fn cells(&self) -> &[Cell] { &self.cells }

    fn put(&mut self, row: u16, col: u16, ch: char, fg: Color, bg: Color, mods: Modifier) {
        if row >= self.rows || col >= self.cols { return; }
        self.cells[row as usize * self.cols as usize + col as usize] =
            Cell { ch, fg, bg, mods };
    }

    fn fill_row(&mut self, row: u16, cell: Cell) {
        if row >= self.rows { return; }
        for c in 0..self.cols as usize {
            self.cells[row as usize * self.cols as usize + c] = cell.clone();
        }
    }
}

/// Cursor + style state for the parser. Kept separate from `Grid` so the
/// grid can be cloned for rendering without dragging mutable cursor state.
#[derive(Debug, Clone)]
pub struct State {
    pub row: u16,
    pub col: u16,
    pub fg: Color,
    pub bg: Color,
    pub mods: Modifier,
}

impl State {
    fn new() -> Self {
        Self { row: 0, col: 0, fg: Color::Reset, bg: Color::Reset, mods: Modifier::empty() }
    }
}

pub struct Performer<'a> {
    pub grid: &'a mut Grid,
    pub state: &'a mut State,
}

impl<'a> Perform for Performer<'a> {
    fn print(&mut self, c: char) {
        let r = self.state.row;
        let co = self.state.col;
        self.grid.put(r, co, c, self.state.fg, self.state.bg, self.state.mods);
        self.state.col = self.state.col.saturating_add(1);
        if self.state.col >= self.grid.cols {
            self.state.col = 0;
            self.linefeed();
        }
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            0x07 => {} // BEL — ignore
            0x08 => { self.state.col = self.state.col.saturating_sub(1); } // BS
            0x09 => { // HT — tab to next multiple of 8
                let next = ((self.state.col / 8) + 1) * 8;
                self.state.col = next.min(self.grid.cols.saturating_sub(1));
            }
            0x0a => self.linefeed(), // LF
            0x0d => { self.state.col = 0; } // CR
            _ => {} // ignore the rest for v0
        }
    }

    fn hook(&mut self, _params: &vte::Params, _intermediates: &[u8], _ignore: bool, _action: char) {}
    fn put(&mut self, _byte: u8) {}
    fn unhook(&mut self) {}

    fn osc_dispatch(&mut self, _params: &[&[u8]], _bell_terminated: bool) {}
    fn csi_dispatch(&mut self, params: &vte::Params, intermediates: &[u8], _ignore: bool, action: char) {
        // vte 0.13's Params is iterable (Params::iter -> ParamsIter<'_> of
        // &[u16] sub-param slices). Flatten into one Vec<u16> for easy
        // indexing. Empty params default to 0 for any unspecified slot.
        let p: Vec<u16> = params.iter().flatten().copied().collect();
        let p0 = p.first().copied().unwrap_or(0);
        let p1 = p.get(1).copied().unwrap_or(0);
        let n = |i: usize| p.get(i).copied().unwrap_or(0);
        // We only care about a handful of CSI sequences for v0.
        match action {
            'A' => { // CUU — cursor up
                let d = p0.max(1) as u16;
                self.state.row = self.state.row.saturating_sub(d);
            }
            'B' => { // CUD — cursor down
                let d = p0.max(1) as u16;
                self.state.row = (self.state.row + d).min(self.grid.rows.saturating_sub(1));
            }
            'C' => { // CUF — cursor forward
                let d = p0.max(1) as u16;
                self.state.col = (self.state.col + d).min(self.grid.cols.saturating_sub(1));
            }
            'D' => { // CUB — cursor back
                let d = p0.max(1) as u16;
                self.state.col = self.state.col.saturating_sub(d);
            }
            'H' | 'f' => { // CUP / HVP — cursor position (1-based)
                let row = p0.max(1) as u16;
                let col = p1.max(1) as u16;
                self.state.row = row.saturating_sub(1).min(self.grid.rows.saturating_sub(1));
                self.state.col = col.saturating_sub(1).min(self.grid.cols.saturating_sub(1));
            }
            'J' => { // ED — erase in display
                match p0 {
                    0 => self.erase_to_end(),
                    1 => self.erase_to_start(),
                    2 | 3 => self.erase_all(),
                    _ => {}
                }
            }
            'K' => { // EL — erase in line
                match p0 {
                    0 => self.erase_line_to_end(),
                    1 => self.erase_line_to_start(),
                    2 => self.erase_line(),
                    _ => {}
                }
            }
            'm' => { // SGR — select graphic rendition
                self.sgr(&p);
            }
            _ => {
                // Ignore everything else (cursor visibility, scroll region,
                // mouse, etc. — v0 only paints text).
                let _ = intermediates;
                let _ = n;
            }
        }
    }
}

impl<'a> Performer<'a> {
    fn linefeed(&mut self) {
        // LF + CR is the typical app pattern; some apps send LF only.
        // We treat LF as LF + CR so lines start at col 0.
        self.state.col = 0;
        if self.state.row + 1 < self.grid.rows {
            self.state.row += 1;
        } else {
            // Scroll up: shift all rows up by one, blank the bottom.
            let cols = self.grid.cols as usize;
            let rows = self.grid.rows as usize;
            let blank = Cell::blank(self.grid.bg);
            self.grid.cells.drain(0..cols);
            for _ in 0..cols { self.grid.cells.push(blank.clone()); }
            let _ = rows;
        }
    }

    fn erase_to_end(&mut self) {
        let blank = Cell::blank(self.grid.bg);
        let row = self.state.row as usize;
        let col = self.state.col as usize;
        let cols = self.grid.cols as usize;
        for c in col..cols {
            self.grid.cells[row * cols + c] = blank.clone();
        }
        for r in (row + 1)..self.grid.rows as usize {
            for c in 0..cols {
                self.grid.cells[r * cols + c] = blank.clone();
            }
        }
    }

    fn erase_to_start(&mut self) {
        let blank = Cell::blank(self.grid.bg);
        let row = self.state.row as usize;
        let col = self.state.col as usize;
        let cols = self.grid.cols as usize;
        for r in 0..row {
            for c in 0..cols {
                self.grid.cells[r * cols + c] = blank.clone();
            }
        }
        for c in 0..=col {
            self.grid.cells[row * cols + c] = blank.clone();
        }
    }

    fn erase_all(&mut self) {
        let blank = Cell::blank(self.grid.bg);
        self.grid.fill_row(0, blank.clone()); // no-op for first row, but...
        let cols = self.grid.cols as usize;
        for c in 0..self.grid.cells.len() {
            self.grid.cells[c] = blank.clone();
        }
        let _ = cols;
    }

    fn erase_line_to_end(&mut self) {
        let blank = Cell::blank(self.grid.bg);
        let row = self.state.row as usize;
        let col = self.state.col as usize;
        let cols = self.grid.cols as usize;
        for c in col..cols {
            self.grid.cells[row * cols + c] = blank.clone();
        }
    }

    fn erase_line_to_start(&mut self) {
        let blank = Cell::blank(self.grid.bg);
        let row = self.state.row as usize;
        let col = self.state.col as usize;
        let cols = self.grid.cols as usize;
        for c in 0..=col {
            self.grid.cells[row * cols + c] = blank.clone();
        }
    }

    fn erase_line(&mut self) {
        let blank = Cell::blank(self.grid.bg);
        self.grid.fill_row(self.state.row, blank);
    }

    fn sgr(&mut self, params: &[u16]) {
        // Default SGR is "\x1b[m" — params is empty, treat as reset.
        if params.is_empty() {
            self.state.fg = Color::Reset;
            self.state.bg = Color::Reset;
            self.state.mods = Modifier::empty();
            return;
        }
        let mut i = 0;
        while i < params.len() {
            let n = params[i];
            match n {
                0 => { self.state.fg = Color::Reset; self.state.bg = Color::Reset; self.state.mods = Modifier::empty(); }
                1 => self.state.mods |= Modifier::BOLD,
                2 => self.state.mods |= Modifier::DIM,
                3 => self.state.mods |= Modifier::ITALIC,
                4 => self.state.mods |= Modifier::UNDERLINED,
                5 | 6 => self.state.mods |= Modifier::SLOW_BLINK,
                7 => self.state.mods |= Modifier::REVERSED,
                9 => self.state.mods |= Modifier::CROSSED_OUT,
                22 => { self.state.mods.remove(Modifier::BOLD | Modifier::DIM); }
                23 => { self.state.mods.remove(Modifier::ITALIC); }
                24 => { self.state.mods.remove(Modifier::UNDERLINED); }
                25 => { self.state.mods.remove(Modifier::SLOW_BLINK); }
                27 => { self.state.mods.remove(Modifier::REVERSED); }
                30..=37 => self.state.fg = Color::Indexed((n - 30) as u8),
                39 => self.state.fg = Color::Reset,
                40..=47 => self.state.bg = Color::Indexed((n - 40) as u8),
                49 => self.state.bg = Color::Reset,
                90..=97 => self.state.fg = Color::Indexed((n - 90 + 8) as u8),
                100..=107 => self.state.bg = Color::Indexed((n - 100 + 8) as u8),
                38 | 48 => {
                    // Extended colour: 38;5;N or 38;2;R;G;B. v0 only handles
                    // the 256-colour form (5;N). Skip the next two params.
                    if i + 2 < params.len() {
                        if params[i + 1] == 5 {
                            let color = params[i + 2] as u8;
                            if n == 38 { self.state.fg = Color::Indexed(color); }
                            else        { self.state.bg = Color::Indexed(color); }
                        }
                        // 38;2;R;G;B (truecolor) is ignored — most TUI apps
                        // either send 5;N or fall back to indexed.
                        i += 2;
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }
}

/// Public façade: hold a `Parser` + a `Performer`'s state, call `advance`
/// with raw bytes from the PTY, and read the populated `Grid` afterwards.
pub struct AnsiParser {
    parser: Parser,
    state: State,
}

impl AnsiParser {
    pub fn new() -> Self {
        Self { parser: Parser::new(), state: State::new() }
    }

    pub fn advance(&mut self, grid: &mut Grid, bytes: &[u8]) {
        // vte 0.13's Parser::advance takes one byte at a time (not a slice
        // like earlier versions), so loop and feed.
        let mut performer = Performer { grid, state: &mut self.state };
        for &b in bytes {
            self.parser.advance(&mut performer, b);
        }
    }
}

impl Default for AnsiParser { fn default() -> Self { Self::new() } }

#[cfg(test)]
mod tests {
    use super::*;

    fn render(grid: &Grid) -> String {
        let mut out = String::new();
        for r in 0..grid.rows as usize {
            for c in 0..grid.cols as usize {
                out.push(grid.cells[r * grid.cols as usize + c].ch);
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn plain_text_lf_cr() {
        let mut grid = Grid::new(3, 10);
        let mut p = AnsiParser::new();
        p.advance(&mut grid, b"hello\r\nworld");
        // Render each row, trimmed on the right, so trailing blanks don't
        // confuse the assertion. The grid is 10 cols; only the populated
        // cells matter.
        let row = |r: u16| -> String {
            (0..grid.cols as usize)
                .map(|c| grid.cells()[r as usize * grid.cols as usize + c].ch)
                .collect::<String>()
                .trim_end()
                .to_string()
        };
        assert_eq!(row(0), "hello");
        assert_eq!(row(1), "world");
    }

    #[test]
    fn sgr_colors_and_bold() {
        let mut grid = Grid::new(1, 10);
        let mut p = AnsiParser::new();
        // bold + red "X"
        p.advance(&mut grid, b"\x1b[1;31mX\x1b[0mY");
        assert_eq!(grid.cells()[0].ch, 'X');
        assert!(grid.cells()[0].mods.contains(Modifier::BOLD));
        assert_eq!(grid.cells()[0].fg, Color::Indexed(1));
        assert_eq!(grid.cells()[1].ch, 'Y');
        assert_eq!(grid.cells()[1].fg, Color::Reset);
        assert!(grid.cells()[1].mods.is_empty());
    }

    #[test]
    fn cup_positioning() {
        let mut grid = Grid::new(5, 10);
        let mut p = AnsiParser::new();
        // row 3 col 2 (1-based) → state.row=2, state.col=1
        p.advance(&mut grid, b"\x1b[3;2HX");
        assert_eq!(grid.cells()[2 * 10 + 1].ch, 'X');
    }

    #[test]
    fn ed_clears_screen() {
        let mut grid = Grid::new(2, 5);
        let mut p = AnsiParser::new();
        p.advance(&mut grid, b"hello");
        p.advance(&mut grid, b"\x1b[2J");
        for c in grid.cells() {
            assert_eq!(c.ch, ' ');
        }
    }

    #[test]
    fn lf_at_bottom_scrolls() {
        // 3 rows × 4 cols.
        //  - "AAAA" lands at row 0.
        //  - "\r\nBBBB" lands at row 1 (state.row=1 after the LF).
        //  - "\r\n" (no chars) triggers the manual scroll: row 0 (AAAA)
        //    drops off, row 1 (BBBB) becomes row 0, row 1 becomes blank.
        // No further writes — keeps the auto-wrap-from-writing off the
        // table so we can isolate the LF-scroll mechanic.
        let mut grid = Grid::new(3, 4);
        let mut p = AnsiParser::new();
        p.advance(&mut grid, b"AAAA\r\nBBBB\r\n");
        let row = |r: u16| -> String {
            (0..grid.cols as usize)
                .map(|c| grid.cells()[r as usize * grid.cols as usize + c].ch)
                .collect::<String>()
                .trim_end()
                .to_string()
        };
        assert_eq!(row(0), "BBBB");
        assert_eq!(row(1), "");
        assert_eq!(row(2), "");
    }
}
