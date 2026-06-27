//! `Window` — the runtime state for one pane.
//!
//! The split tree (`wm::tree`) only describes *where* panes live on screen.
//! The `Window` is what each pane *is*: either a built-in screen (one of the
//! 13 in `app::ScreenId`) or a live PTY (the Phase-2 infra in
//! `wm::pty` + `wm::broadcaster`).
//!
//! A `Window` owns whatever long-lived state its pane needs:
//!   * `Builtin` — nothing of its own; the renderer dispatches to the global
//!     `screens` list keyed by `ScreenId`.
//!   * `Terminal` — a `Grid` + `AnsiParser` + `PaneOutput` (broadcast
//!     receiver) + `PtyWriter` + last cached child PID. Each rendered frame
//!     drains the broadcast receiver into the parser, then paints the grid.

use crate::wm::ansi::{AnsiParser, Grid};
use crate::wm::broadcaster::{PaneId, PaneOutput, PtyWriter};
use crate::wm::pty::Pty;

/// What a pane hosts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WindowKind {
    /// One of the built-in `ScreenId` screens (System, Network, ...).
    Builtin(crate::app::screen::ScreenId),
    /// A live PTY pane.
    Terminal,
}

impl WindowKind {
    /// Short label for the pane title bar.
    pub fn label(&self) -> &'static str {
        match self {
            WindowKind::Builtin(id) => id.label(),
            WindowKind::Terminal => "terminal",
        }
    }
}

/// Runtime state for one pane.
///
/// `Window` is intentionally not `Clone` — it owns the PTY reader, the
/// broadcaster subscription, and the parsed grid. Cloning would mean cloning
/// a `broadcast::Receiver`, which isn't `Clone`. The WM owns windows in a
/// `HashMap<PaneId, Window>` and looks them up by id.
pub struct Window {
    pub id: PaneId,
    pub kind: WindowKind,
    pub focused: bool,
    /// Last known grid size — used to detect resizes and call `pty.resize`.
    pub last_rows: u16,
    pub last_cols: u16,
    /// Terminal-pane state. `None` for built-in screens.
    terminal: Option<TerminalState>,
}

/// Terminal-specific state. Owned by `Window` so the renderer can drive it
/// without re-checking which kind each frame.
pub struct TerminalState {
    pub grid: Grid,
    pub parser: AnsiParser,
    pub output: PaneOutput,
    pub writer: PtyWriter,
    /// The handle to the spawned child. Held so we can resize / kill it
    /// when the pane closes or the terminal exits.
    pub pty: Pty,
}

impl Window {
    /// Build a built-in window for `screen`.
    pub fn builtin(id: PaneId, screen: crate::app::screen::ScreenId) -> Self {
        Self {
            id,
            kind: WindowKind::Builtin(screen),
            focused: false,
            last_rows: 0,
            last_cols: 0,
            terminal: None,
        }
    }

    /// Build a terminal window around an already-spawned `Pty` and the
    /// broadcaster halves it was registered with. The `Grid` is allocated
    /// at `(rows, cols)`; the render loop is responsible for calling
    /// `resize` on size changes.
    pub fn terminal(
        id: PaneId,
        pty: Pty,
        output: PaneOutput,
        writer: PtyWriter,
        rows: u16,
        cols: u16,
    ) -> Self {
        Self {
            id,
            kind: WindowKind::Terminal,
            focused: false,
            last_rows: rows,
            last_cols: cols,
            terminal: Some(TerminalState {
                grid: Grid::new(rows, cols),
                parser: AnsiParser::new(),
                output,
                writer,
                pty,
            }),
        }
    }

    /// Convenience for tests / programmatic creation.
    pub fn is_terminal(&self) -> bool {
        self.terminal.is_some()
    }

    /// Drain any pending bytes from the broadcaster into the ANSI parser.
    /// `Lagged` errors are swallowed: the next paint will use whatever the
    /// grid currently holds.
    ///
    /// Returns the number of bytes drained (0 means nothing to do).
    pub fn drain_output(&mut self) -> usize {
        let Some(term) = self.terminal.as_mut() else {
            return 0;
        };
        let mut rx = term.output.subscribe();
        let mut total = 0usize;
        loop {
            match rx.try_recv() {
                Ok(chunk) => {
                    total += chunk.len();
                    term.parser.advance(&mut term.grid, &chunk);
                }
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => {
                    // A lagged receiver means we missed bytes. The grid may
                    // now be stale relative to the child's output, but
                    // there's nothing useful we can do other than keep
                    // going and let the next paint catch up.
                    continue;
                }
                Err(tokio::sync::broadcast::error::TryRecvError::Closed) => break,
            }
        }
        total
    }

    /// Resize the grid (and the underlying PTY, if any) to `(rows, cols)`.
    /// No-op if the size hasn't changed.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        if rows == self.last_rows && cols == self.last_cols {
            return;
        }
        if let Some(term) = self.terminal.as_mut() {
            term.grid.resize(rows, cols);
            let _ = term.pty.resize(rows, cols);
        }
        self.last_rows = rows;
        self.last_cols = cols;
    }

    /// Paint one pane. Dispatches on `kind`:
    ///   * `Builtin(id)` — finds the matching `Screen` in `screens`
    ///     and calls its `render`.
    ///   * `Terminal`    — drains the broadcaster into the parser,
    ///     then paints the `Grid` as styled spans into a `Paragraph`
    ///     wrapped in a `Block` with the pane title.
    #[allow(dead_code)] // wired up in Task 2.3
    pub fn paint(
        &mut self,
        frame: &mut ratatui::Frame,
        area: ratatui::layout::Rect,
        screens: &mut [Box<dyn crate::app::screen::Screen>],
        app: &mut crate::app::App,
        theme: &crate::theme::Theme,
        focused: bool,
    ) {
        use ratatui::text::{Line, Span};
        use ratatui::widgets::{Block, Borders, Paragraph};
        self.focused = focused;
        match self.kind {
            WindowKind::Builtin(id) => {
                if let Some(s) = screens.iter_mut().find(|s| s.id() == id) {
                    s.render(frame, area, app, theme, focused);
                }
                // The screen's `render` already drew its own border with
                // the screen's title — we don't need to draw ours on top.
            }
            WindowKind::Terminal => {
                // Drain first, then borrow `self.terminal` mutably for the
                // rest of the arm. The split into two statements avoids a
                // `&mut self` overlapping a `&mut self.terminal` borrow.
                let _ = self.drain_output();
                if let Some(term) = self.terminal.as_mut() {
                    let title = crate::wm::render::pane_title(&self.kind);
                    let block = Block::default()
                        .title(Span::styled(title, theme.title()))
                        .borders(Borders::ALL)
                        .border_style(theme.border(focused));
                    let lines: Vec<Line> = (0..term.grid.rows as usize)
                        .map(|r| {
                            let spans: Vec<Span> = (0..term.grid.cols as usize)
                                .map(|c| {
                                    let cell = &term.grid.cells()[r * term.grid.cols as usize + c];
                                    Span::styled(
                                        cell.ch.to_string(),
                                        ratatui::style::Style::default()
                                            .fg(cell.fg)
                                            .bg(cell.bg)
                                            .add_modifier(cell.mods),
                                    )
                                })
                                .collect();
                            Line::from(spans)
                        })
                        .collect();
                    let p = Paragraph::new(lines)
                        .style(ratatui::style::Style::default().fg(theme.fg).bg(theme.bg))
                        .block(block);
                    frame.render_widget(p, area);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::screen::ScreenId;
    use portable_pty::CommandBuilder;

    #[test]
    fn builtin_window_has_no_terminal_state() {
        let w = Window::builtin(PaneId(1), ScreenId::System);
        assert_eq!(w.kind, WindowKind::Builtin(ScreenId::System));
        assert!(!w.is_terminal());
        assert_eq!(w.last_rows, 0);
        assert_eq!(w.last_cols, 0);
    }

    #[test]
    fn terminal_window_holds_grid_and_resizes() {
        let cmd = CommandBuilder::new("/bin/cat");
        let pty = Pty::spawn(cmd, 24, 80).expect("spawn cat");
        let (out, writer, _tasks) = crate::wm::broadcaster::spawn(pty.clone());
        let mut w = Window::terminal(PaneId(1), pty.clone(), out, writer, 24, 80);
        assert!(w.is_terminal());
        // Resize to a smaller grid; the grid should now be 10×20.
        w.resize(10, 20);
        assert_eq!(w.last_rows, 10);
        assert_eq!(w.last_cols, 20);
        // Resize to the same size → no-op (idempotent).
        w.resize(10, 20);
        assert_eq!(w.last_rows, 10);
        let _ = pty.kill();
    }

    #[test]
    fn drain_output_returns_zero_for_builtin() {
        let mut w = Window::builtin(PaneId(1), ScreenId::System);
        assert_eq!(w.drain_output(), 0);
    }
}