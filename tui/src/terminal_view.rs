//! The view for one terminal tool: draws a [`Terminal`]'s screen into a
//! rectangle and turns keyboard and mouse input into bytes for the
//! program running in it.
//!
//! Like the editor view, this owns what the model doesn't: the scrollback
//! offset the user has scrolled to, the screen regions from the last
//! render (to hit-test the mouse and to resize the pty to match), and any
//! mouse drag in progress. The [`Terminal`] itself keeps the screen and
//! encodes input; the view only decides when to send it.
//!
//! Bytes the program should get back (its own input, or answers to the
//! queries the emulator makes) are returned from [`handle_key`] and the
//! other input methods so the application can write them to the session.
//! Nothing here talks to the pty directly.
//!
//! Scrolling back through the scrollback shows older output; any output
//! from the program, or any key sent to it, jumps back to the bottom, as
//! terminals do. While the program is using the mouse itself (a
//! full-screen program that turned mouse reporting on) the wheel is sent
//! to it rather than scrolling the view.
//!
//! Source locations in the output (the file and line of a compiler's
//! warning or error; see [`find_source_links`]) are drawn as links,
//! underlined in the theme's link color, and [`link_at`] tells the
//! application which one a click landed on so it can open the file.
//! Links are found in the rows on screen each time they change, with a
//! line the terminal wrapped scanned whole, so a location cut by the
//! right edge is still one link.
//!
//! [`handle_key`]: TerminalView::handle_key
//! [`link_at`]: TerminalView::link_at

use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ninjaedit_core::terminal::{
    Color as TermColor, Key, Modifiers, MouseButton as TermButton, MouseEvent as TermMouse,
    MouseEventKind as TermMouseKind, Row, Style as TermStyle, Terminal, Underline,
};
use ninjaedit_core::{SourceLocation, find_source_links};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position as ScreenPosition, Rect};
use ratatui::style::{Color, Modifier, Style};
use std::ops::Range;

/// Lines scrolled per mouse wheel notch when the view, not the program,
/// is scrolling.
const WHEEL_LINES: usize = 3;

/// The part of a source link drawn on one screen row.
#[derive(Clone, Debug, PartialEq, Eq)]
struct LinkSpan {
    /// The screen row, and the screen columns along it.
    y: u16,
    x: Range<u16>,
    location: SourceLocation,
}

pub struct TerminalView {
    terminal: Terminal,
    /// How many lines the view is scrolled up into the scrollback; zero
    /// at the bottom, showing the live screen.
    scrollback: usize,
    /// Whether a mouse drag is being sent to the program, and which
    /// button it started with, so drag and release events can follow the
    /// pointer outside the view.
    dragging: Option<MouseButton>,
    /// The screen region the terminal was last drawn into.
    area: Rect,
    /// The source links on screen at the last render, and what the
    /// screen was (its generation, scroll offset, and area) when they
    /// were found, so they are found again only when it changes.
    links: Vec<LinkSpan>,
    links_for: Option<(u64, usize, Rect)>,
}

impl TerminalView {
    /// A view onto a terminal of the given size.
    pub fn new(cols: u16, rows: u16) -> TerminalView {
        TerminalView {
            terminal: Terminal::new(cols as usize, rows as usize),
            scrollback: 0,
            dragging: None,
            area: Rect::default(),
            links: Vec::new(),
            links_for: None,
        }
    }

    pub fn terminal_mut(&mut self) -> &mut Terminal {
        &mut self.terminal
    }

    /// The terminal, for tests to look at its state.
    #[cfg(test)]
    pub fn terminal_mut_for_test(&self) -> &Terminal {
        &self.terminal
    }

    /// Feed the program's output to the emulator. When the user is scrolled
    /// back reading older output, the view stays anchored to those lines as
    /// new ones push into the scrollback, rather than following the program
    /// to the bottom or letting the text drift under them.
    pub fn process(&mut self, bytes: &[u8]) {
        let before = self.terminal.scrollback_len();
        self.terminal.process(bytes);
        if self.scrollback != 0 {
            let after = self.terminal.scrollback_len();
            let added = after.saturating_sub(before);
            self.scrollback = (self.scrollback + added).min(after);
        }
    }

    /// The size the pty should be told about: the last drawn size, or the
    /// terminal's current size before the first render.
    pub fn size(&self) -> (u16, u16) {
        let (cols, rows) = self.terminal.size();
        (cols as u16, rows as u16)
    }

    /// The window title the program set, for the tab to show.
    pub fn title(&self) -> &str {
        self.terminal.title()
    }

    /// Whether a drag is being sent to the program, so the view wants
    /// drag and release events even outside its area.
    pub fn is_dragging(&self) -> bool {
        self.dragging.is_some()
    }

    /// Jump back to the live screen, as any input does.
    fn scroll_to_bottom(&mut self) {
        self.scrollback = 0;
    }

    // ----- Keyboard -------------------------------------------------------

    /// Handle a key press, returning the bytes to send to the program (its
    /// own input plus any answer to a query the key triggered). Keys the
    /// terminal doesn't turn into input, such as Shift+PageUp for
    /// scrollback, return no bytes.
    pub fn handle_key(&mut self, key: KeyEvent) -> Vec<u8> {
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        // Shift with Page Up/Down and the wheel is the terminal's own
        // scrollback, never sent to the program.
        if shift {
            match key.code {
                KeyCode::PageUp => {
                    self.scroll_by(self.page());
                    return Vec::new();
                }
                KeyCode::PageDown => {
                    self.scroll_by(-self.page());
                    return Vec::new();
                }
                _ => {}
            }
        }
        let Some((k, mods)) = translate_key(key) else {
            return Vec::new();
        };
        self.scroll_to_bottom();
        let mut bytes = self.terminal.encode_key(k, mods).unwrap_or_default();
        bytes.extend(self.terminal.take_responses());
        bytes
    }

    /// The bytes for pasted text, wrapped for bracketed paste if the
    /// program asked for it.
    pub fn paste(&mut self, text: &str) -> Vec<u8> {
        self.scroll_to_bottom();
        self.terminal.encode_paste(text)
    }

    // ----- Mouse ----------------------------------------------------------

    /// Handle a mouse event, returning bytes to send to the program (a
    /// mouse report, when it asked for one). The wheel scrolls the view's
    /// scrollback unless the program is reading the mouse itself.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> Vec<u8> {
        // The wheel scrolls the scrollback when the program isn't using
        // the mouse, and there's scrollback to move through.
        let wheel = matches!(
            mouse.kind,
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
        );
        if wheel && !self.terminal.reports_mouse() {
            match mouse.kind {
                MouseEventKind::ScrollUp => self.scroll_by(WHEEL_LINES as isize),
                MouseEventKind::ScrollDown => self.scroll_by(-(WHEEL_LINES as isize)),
                _ => {}
            }
            return Vec::new();
        }
        let Some(event) = self.translate_mouse(mouse) else {
            return Vec::new();
        };
        self.terminal.encode_mouse(event).unwrap_or_default()
    }

    /// Turn a crossterm mouse event into the emulator's, in cell
    /// coordinates, tracking the drag so its motion and release follow.
    fn translate_mouse(&mut self, mouse: MouseEvent) -> Option<TermMouse> {
        let col = (mouse.column.saturating_sub(self.area.x)) as usize;
        let row = (mouse.row.saturating_sub(self.area.y)) as usize;
        let kind = match mouse.kind {
            MouseEventKind::Down(button) => {
                self.dragging = Some(button);
                TermMouseKind::Press(term_button(button))
            }
            MouseEventKind::Up(button) => {
                self.dragging = None;
                TermMouseKind::Release(term_button(button))
            }
            MouseEventKind::Drag(button) => TermMouseKind::Drag(term_button(button)),
            MouseEventKind::Moved => TermMouseKind::Move,
            MouseEventKind::ScrollUp => TermMouseKind::ScrollUp,
            MouseEventKind::ScrollDown => TermMouseKind::ScrollDown,
            MouseEventKind::ScrollLeft => TermMouseKind::ScrollLeft,
            MouseEventKind::ScrollRight => TermMouseKind::ScrollRight,
        };
        Some(TermMouse {
            kind,
            col,
            row,
            modifiers: modifiers(mouse.modifiers),
        })
    }

    // ----- Scrollback -----------------------------------------------------

    /// A page for Shift+PageUp: one line short of the view, like the
    /// editor's.
    fn page(&self) -> isize {
        (self.area.height.saturating_sub(1)).max(1) as isize
    }

    /// Scroll the view up (positive) or down through the scrollback,
    /// clamped to what there is.
    fn scroll_by(&mut self, lines: isize) {
        let max = self.terminal.scrollback_len();
        self.scrollback = (self.scrollback as isize + lines).clamp(0, max as isize) as usize;
    }

    /// How far back the view is scrolled, for the application to note when
    /// the terminal isn't at the bottom.
    pub fn scrollback_offset(&self) -> usize {
        self.scrollback
    }

    // ----- Links ----------------------------------------------------------

    /// The source location drawn as a link under screen position (`x`,
    /// `y`) at the last render, for a click there to open. `None` while
    /// the program is reading the mouse itself, since the click is its.
    pub fn link_at(&self, x: u16, y: u16) -> Option<&SourceLocation> {
        if self.terminal.reports_mouse() {
            return None;
        }
        self.links
            .iter()
            .find(|span| span.y == y && span.x.contains(&x))
            .map(|span| &span.location)
    }

    /// Find the source links in the rows now on screen, if the screen
    /// has changed since they were last found.
    fn refresh_links(&mut self, area: Rect) {
        let key = (self.terminal.generation(), self.scrollback, area);
        if self.links_for == Some(key) {
            return;
        }
        let rows: Vec<&Row> = self
            .terminal
            .rows(self.scrollback)
            .take(area.height as usize)
            .collect();
        let links = find_links(&rows, area);
        self.links = links;
        self.links_for = Some(key);
    }

    // ----- Rendering ------------------------------------------------------

    /// Draw the terminal into `area`, resizing it to fit, and return where
    /// the cursor should be drawn, if it should. The cursor is hidden while
    /// the program hides it and while the user is scrolled back into the
    /// scrollback, where a cursor would be meaningless.
    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
    ) -> Option<ScreenPosition> {
        self.area = area;
        let base = Style::default()
            .fg(theme.terminal_text)
            .bg(theme.terminal_background);
        buf.set_style(area, base);
        if area.width == 0 || area.height == 0 {
            return None;
        }
        let cols = area.width as usize;
        let rows = area.height as usize;
        if self.terminal.size() != (cols, rows) {
            self.terminal.resize(cols, rows);
            self.scrollback = self.scrollback.min(self.terminal.scrollback_len());
        }
        // Tell the emulator what the default colors look like, so a
        // program asking gets an answer matching the theme.
        self.terminal
            .set_default_colors(rgb(theme.terminal_text), rgb(theme.terminal_background));

        for (row, line) in self.terminal.rows(self.scrollback).enumerate() {
            if row >= rows {
                break;
            }
            let y = area.y + row as u16;
            let mut col = 0;
            for cell in &line.cells {
                if col >= cols {
                    break;
                }
                if cell.spacer {
                    continue;
                }
                let style = term_style(&cell.style, theme);
                let x = area.x + col as u16;
                if cell.text.is_empty() {
                    buf[(x, y)].set_symbol(" ").set_style(style);
                } else {
                    buf[(x, y)].set_symbol(&cell.text).set_style(style);
                }
                // A wide character owns the next cell too; skip drawing
                // over its right half.
                let width = if cell.wide { 2 } else { 1 };
                if cell.wide && col + 1 < cols {
                    buf[(x + 1, y)].set_symbol(" ").set_style(style);
                }
                col += width;
            }
        }

        // Source locations in the output are links: underlined, in the
        // theme's link color, over whatever the program drew.
        self.refresh_links(area);
        let link_style = Style::default()
            .fg(theme.terminal_link)
            .add_modifier(Modifier::UNDERLINED);
        for span in &self.links {
            for x in span.x.clone() {
                buf[(x, span.y)].set_style(link_style);
            }
        }

        // The cursor shows only on the live screen, when the program has
        // it visible.
        if self.scrollback != 0 {
            return None;
        }
        self.terminal.cursor().and_then(|(cx, cy)| {
            (cx < cols && cy < rows)
                .then(|| ScreenPosition::new(area.x + cx as u16, area.y + cy as u16))
        })
    }
}

/// The source links among `rows`, the screen's rows top to bottom, as
/// spans in screen coordinates within `area`. Rows the terminal wrapped
/// are joined with the rows after them and scanned as one line, and a
/// link crossing the edge becomes one span per row.
fn find_links(rows: &[&Row], area: Rect) -> Vec<LinkSpan> {
    let cols = area.width as usize;
    let mut spans = Vec::new();
    let mut start = 0;
    while start < rows.len() {
        // The rows of one logical line: each wrapped row continues on
        // the next.
        let mut end = start;
        while end + 1 < rows.len() && rows[end].wrapped {
            end += 1;
        }
        // The line's text, and for each cell in it the byte it begins
        // at and where it is on screen.
        let mut text = String::new();
        let mut cells: Vec<(usize, usize, usize)> = Vec::new(); // (byte, row, col)
        for (row, line) in rows[start..=end].iter().enumerate() {
            let mut col = 0;
            for cell in &line.cells {
                if col >= cols {
                    break;
                }
                if !cell.spacer {
                    cells.push((text.len(), start + row, col));
                    text.push_str(&cell.text);
                }
                col += cell.width();
            }
        }
        for link in find_source_links(&text) {
            // The cells the link's bytes fall in, as one column range per
            // row.
            let mut current: Option<(usize, Range<usize>)> = None;
            for &(byte, row, col) in &cells {
                if !link.range.contains(&byte) {
                    continue;
                }
                match &mut current {
                    Some((r, range)) if *r == row => range.end = col + 1,
                    _ => {
                        if let Some((r, range)) = current.take() {
                            spans.push(span(r, range, &link.location, area));
                        }
                        current = Some((row, col..col + 1));
                    }
                }
            }
            if let Some((r, range)) = current {
                spans.push(span(r, range, &link.location, area));
            }
        }
        start = end + 1;
    }
    spans
}

fn span(row: usize, cols: Range<usize>, location: &SourceLocation, area: Rect) -> LinkSpan {
    let x = area.x + cols.start as u16..area.x + cols.end.min(area.width as usize) as u16;
    LinkSpan {
        y: area.y + row as u16,
        x,
        location: location.clone(),
    }
}

/// The emulator button for a crossterm one; other buttons map to left.
fn term_button(button: MouseButton) -> TermButton {
    match button {
        MouseButton::Left => TermButton::Left,
        MouseButton::Middle => TermButton::Middle,
        MouseButton::Right => TermButton::Right,
    }
}

fn modifiers(m: KeyModifiers) -> Modifiers {
    Modifiers {
        shift: m.contains(KeyModifiers::SHIFT),
        alt: m.contains(KeyModifiers::ALT),
        ctrl: m.contains(KeyModifiers::CONTROL),
    }
}

/// A crossterm key as the terminal's key and modifiers, or `None` for a
/// key press that sends nothing (a release, a bare modifier, a key the
/// terminal has no encoding for).
fn translate_key(key: KeyEvent) -> Option<(Key, Modifiers)> {
    let k = match key.code {
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Enter => Key::Enter,
        KeyCode::Tab => Key::Tab,
        KeyCode::BackTab => Key::Tab,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Esc => Key::Escape,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Insert => Key::Insert,
        KeyCode::Delete => Key::Delete,
        KeyCode::F(n) => Key::F(n),
        _ => return None,
    };
    let mut mods = modifiers(key.modifiers);
    // BackTab is Shift+Tab however the terminal reported it.
    if key.code == KeyCode::BackTab {
        mods.shift = true;
    }
    Some((k, mods))
}

/// The ratatui style for a terminal cell, mapping its colors through the
/// theme and applying inverse, hidden, and the text attributes.
fn term_style(style: &TermStyle, theme: &Theme) -> Style {
    let default_fg = theme.terminal_text;
    let default_bg = theme.terminal_background;
    let mut fg = color(style.fg, theme).unwrap_or(default_fg);
    let mut bg = color(style.bg, theme).unwrap_or(default_bg);
    // A dim default foreground is nudged toward the background so it reads
    // as dim even though the theme gives no separate color for it.
    if style.dim && style.fg == TermColor::Default {
        fg = blend(default_fg, default_bg);
    }
    if style.inverse {
        std::mem::swap(&mut fg, &mut bg);
    }
    if style.hidden {
        fg = bg;
    }
    let mut out = Style::default().fg(fg).bg(bg);
    if style.bold {
        out = out.add_modifier(Modifier::BOLD);
    }
    if style.dim {
        out = out.add_modifier(Modifier::DIM);
    }
    if style.italic {
        out = out.add_modifier(Modifier::ITALIC);
    }
    if style.underline != Underline::None {
        out = out.add_modifier(Modifier::UNDERLINED);
    }
    if style.blink {
        out = out.add_modifier(Modifier::SLOW_BLINK);
    }
    if style.strikethrough {
        out = out.add_modifier(Modifier::CROSSED_OUT);
    }
    out
}

/// A terminal color as a ratatui one: the default color is left to the
/// caller, the sixteen ANSI colors go through the theme, and the rest are
/// their fixed RGB values.
fn color(color: TermColor, theme: &Theme) -> Option<Color> {
    match color {
        TermColor::Default => None,
        TermColor::Indexed(i) if i < 16 => Some(theme.terminal_palette(i)),
        other => other.fixed_rgb().map(|(r, g, b)| Color::Rgb(r, g, b)),
    }
}

/// The RGB of a theme color, for telling the emulator its default colors.
/// A non-RGB color (which the theme never uses) falls back to mid grey.
fn rgb(color: Color) -> (u8, u8, u8) {
    match color {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => (0x80, 0x80, 0x80),
    }
}

/// The midpoint of two colors, for a dimmed default foreground.
fn blend(a: Color, b: Color) -> Color {
    let (ar, ag, ab) = rgb(a);
    let (br, bg, bb) = rgb(b);
    let mix = |x: u8, y: u8| ((x as u16 + y as u16) / 2) as u8;
    Color::Rgb(mix(ar, br), mix(ag, bg), mix(ab, bb))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEventKind, KeyEventState};
    use ratatui::Terminal as RatTerminal;
    use ratatui::backend::TestBackend;

    fn view() -> TerminalView {
        TerminalView::new(20, 5)
    }

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn keys_become_program_input() {
        let mut v = view();
        assert_eq!(
            v.handle_key(key(KeyCode::Char('a'), KeyModifiers::NONE)),
            b"a"
        );
        assert_eq!(
            v.handle_key(key(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            b"\x03"
        );
        assert_eq!(v.handle_key(key(KeyCode::Enter, KeyModifiers::NONE)), b"\r");
        assert_eq!(
            v.handle_key(key(KeyCode::Up, KeyModifiers::NONE)),
            b"\x1b[A"
        );
        // A bare modifier or an unknown key sends nothing.
        assert!(
            v.handle_key(key(KeyCode::Null, KeyModifiers::NONE))
                .is_empty()
        );
    }

    #[test]
    fn a_query_reply_rides_along_with_input() {
        let mut v = view();
        // A device attributes request should be answered even though it
        // arrives as program output; here we make sure the responses the
        // emulator queues while handling a key are returned. Prime a
        // response by processing a query, then press a key.
        v.process(b"\x1b[6n");
        let bytes = v.handle_key(key(KeyCode::Char('x'), KeyModifiers::NONE));
        assert!(bytes.starts_with(b"x"), "{bytes:?}");
        assert!(
            bytes.windows(2).any(|w| w == b"[1"),
            "cursor report: {bytes:?}"
        );
    }

    #[test]
    fn draws_output_with_theme_colors() {
        let theme = Theme::default();
        let mut v = view();
        v.process(b"\x1b[31mhi\x1b[m there");
        let mut term = RatTerminal::new(TestBackend::new(20, 5)).unwrap();
        term.draw(|f| {
            v.render(f.area(), f.buffer_mut(), &theme);
        })
        .unwrap();
        let buf = term.backend().buffer();
        assert_eq!(buf[(0, 0)].symbol(), "h");
        assert_eq!(buf[(0, 0)].fg, theme.terminal_palette(1));
        assert_eq!(buf[(0, 0)].bg, theme.terminal_background);
        assert_eq!(buf[(3, 0)].symbol(), "t");
        assert_eq!(buf[(3, 0)].fg, theme.terminal_text);
    }

    #[test]
    fn wheel_scrolls_scrollback_then_input_returns_to_bottom() {
        let mut v = view();
        let mut term = RatTerminal::new(TestBackend::new(20, 5)).unwrap();
        // Fill past the screen so there's scrollback.
        for i in 0..20 {
            v.process(format!("line{i}\r\n").as_bytes());
        }
        term.draw(|f| {
            v.render(f.area(), f.buffer_mut(), &theme());
        })
        .unwrap();
        assert_eq!(v.scrollback_offset(), 0);
        let wheel = MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 1,
            row: 1,
            modifiers: KeyModifiers::NONE,
        };
        assert!(v.handle_mouse(wheel).is_empty());
        assert_eq!(v.scrollback_offset(), WHEEL_LINES);
        // A key jumps back to the bottom.
        v.handle_key(key(KeyCode::Char('x'), KeyModifiers::NONE));
        assert_eq!(v.scrollback_offset(), 0);
    }

    #[test]
    fn mouse_goes_to_a_program_that_asked() {
        let mut v = view();
        let mut term = RatTerminal::new(TestBackend::new(20, 5)).unwrap();
        v.process(b"\x1b[?1000h\x1b[?1006h");
        term.draw(|f| {
            v.render(f.area(), f.buffer_mut(), &theme());
        })
        .unwrap();
        let down = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 4,
            row: 2,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(v.handle_mouse(down), b"\x1b[<0;5;3M");
        assert!(v.is_dragging());
        let up = MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 4,
            row: 2,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(v.handle_mouse(up), b"\x1b[<0;5;3m");
        assert!(!v.is_dragging());
        // The wheel now goes to the program too, not the scrollback.
        let wheel = MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 4,
            row: 2,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(v.handle_mouse(wheel), b"\x1b[<64;5;3M");
    }

    #[test]
    fn source_locations_are_links() {
        let theme = theme();
        let mut v = TerminalView::new(30, 5);
        v.process(b"  --> src/main.rs:10:5\r\nnothing here\r\n");
        let mut term = RatTerminal::new(TestBackend::new(30, 5)).unwrap();
        term.draw(|f| {
            v.render(f.area(), f.buffer_mut(), &theme);
        })
        .unwrap();
        let buf = term.backend().buffer();
        // The location is underlined in the link color; the arrow before
        // it and the text after it are not.
        for x in 6..22 {
            let cell = &buf[(x, 0)];
            assert_eq!(cell.fg, theme.terminal_link, "column {x}");
            assert!(cell.modifier.contains(Modifier::UNDERLINED), "column {x}");
        }
        assert_eq!(buf[(2, 0)].fg, theme.terminal_text);
        assert!(!buf[(2, 0)].modifier.contains(Modifier::UNDERLINED));
        assert!(!buf[(22, 0)].modifier.contains(Modifier::UNDERLINED));
        assert!(!buf[(0, 1)].modifier.contains(Modifier::UNDERLINED));
        let link = v.link_at(6, 0).expect("a link under the path");
        assert_eq!(link.path, "src/main.rs");
        assert_eq!((link.line, link.column), (10, Some(5)));
        assert_eq!(v.link_at(2, 0), None, "the arrow isn't part of it");
        assert_eq!(v.link_at(0, 1), None);
    }

    #[test]
    fn a_link_wrapped_by_the_terminal_spans_both_rows() {
        let theme = theme();
        let mut v = view();
        // 30 columns in a 20 column terminal: the path breaks across
        // the edge.
        v.process(b"--> src/some/long/path.rs:10:5\r\n");
        let mut term = RatTerminal::new(TestBackend::new(20, 5)).unwrap();
        term.draw(|f| {
            v.render(f.area(), f.buffer_mut(), &theme);
        })
        .unwrap();
        let buf = term.backend().buffer();
        assert!(buf[(19, 0)].modifier.contains(Modifier::UNDERLINED));
        assert!(buf[(0, 1)].modifier.contains(Modifier::UNDERLINED));
        assert!(buf[(9, 1)].modifier.contains(Modifier::UNDERLINED));
        assert!(!buf[(10, 1)].modifier.contains(Modifier::UNDERLINED));
        let first = v.link_at(19, 0).expect("the first row");
        let second = v.link_at(0, 1).expect("the second row");
        assert_eq!(first, second);
        assert_eq!(first.path, "src/some/long/path.rs");
        assert_eq!(first.line, 10);
    }

    #[test]
    fn links_are_the_programs_while_it_reads_the_mouse() {
        let mut v = view();
        v.process(b"  --> src/main.rs:10:5\r\n");
        let mut term = RatTerminal::new(TestBackend::new(20, 5)).unwrap();
        term.draw(|f| {
            v.render(f.area(), f.buffer_mut(), &theme());
        })
        .unwrap();
        assert!(v.link_at(6, 0).is_some());
        v.process(b"\x1b[?1000h");
        assert_eq!(v.link_at(6, 0), None);
        v.process(b"\x1b[?1000l");
        assert!(v.link_at(6, 0).is_some());
    }

    fn theme() -> Theme {
        Theme::default()
    }
}
