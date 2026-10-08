//! A box showing a git hook that failed, floating over the page whose
//! action ran it: a title naming the hook and how it ended, a line
//! saying what that meant for the action, what the hook wrote, as its
//! hidden terminal showed it, and the buttons.
//!
//! A hook that stopped the action (a `pre-commit` that refused the
//! commit) has two buttons when git has a flag to skip it: the action
//! again without the hook ("Commit anyway", git's `--no-verify`) and
//! Dismiss. Going ahead past a hook is the kind of thing the discard
//! box asks about, so the action's button is drawn as a dangerous one
//! and never has the keyboard to begin with: Enter presses Dismiss
//! until the user moves to the other with ← → or Tab. A hook that ran
//! after the fact (`post-commit`) stopped nothing, so its box only has
//! Dismiss.
//!
//! The output starts scrolled to its end, where a failure usually says
//! why; ↑ ↓, Page Up and Page Down, Home and End, and the wheel scroll
//! it. Escape dismisses, as does a press outside the box.
//!
//! The box only shows: the page that opened it keeps the action, and
//! does it again on [`HookOutcome::Override`].

use crate::diff_pane::display_width;
use crate::fields::wrap_words;
use crate::palette::{palette_background, render_frame};
use crate::terminal_view::term_style;
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ninjaedit_core::git::HookFailure;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position as ScreenPosition, Rect};
use ratatui::style::{Modifier, Style};
use std::ops::Range;

/// The widest the box gets, border included.
const MAX_WIDTH: u16 = 120;
/// The least the box needs, and the least output it shows.
const MIN_WIDTH: u16 = 24;
const MIN_OUTPUT_ROWS: u16 = 3;
const DISMISS: &str = "Dismiss";
/// What the bottom border says.
const HINT: &str = "↑↓ scroll · Esc dismiss";
/// What shows in place of output when the hook wrote none.
const NO_OUTPUT: &str = "(the hook wrote nothing)";

/// What the page should do after the box handled an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookOutcome {
    /// Keep the box open.
    Continue,
    /// Close the box.
    Dismiss,
    /// Close the box and do the action again without the hook.
    Override,
}

/// One of the box's buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Button {
    Override,
    Dismiss,
}

pub struct HookBox {
    failure: HookFailure,
    /// What the failure meant for the action, in a sentence.
    body: String,
    /// The button that does the action without the hook, if it can be
    /// skipped: what it does, in a word or two.
    action: Option<String>,
    focused: Button,
    /// How many lines of output are scrolled back from the end.
    scroll: usize,
    /// The whole box, its output, and its buttons, from the last render.
    area: Rect,
    output_area: Rect,
    override_area: Rect,
    dismiss_area: Rect,
}

impl HookBox {
    /// A box for a hook that failed. `action` names the button that
    /// does what the hook stopped anyway; it is left out for a hook
    /// that git has no flag to skip, or that stopped nothing.
    pub fn new(failure: HookFailure, body: impl Into<String>, action: Option<String>) -> HookBox {
        let action = action.filter(|_| failure.hook.blocks() && failure.hook.skippable());
        HookBox {
            failure,
            body: body.into(),
            action,
            focused: Button::Dismiss,
            scroll: 0,
            area: Rect::default(),
            output_area: Rect::default(),
            override_area: Rect::default(),
            dismiss_area: Rect::default(),
        }
    }

    /// The size of terminal to run hooks in for their output to show
    /// in a box on `page` as the hook drew it: as wide as the box's
    /// output, and as tall as it can be. A page not yet drawn gets a
    /// usual terminal's size.
    pub fn terminal_size(page: Rect) -> (usize, usize) {
        if page.width < MIN_WIDTH {
            return (80, 24);
        }
        let width = Self::width(page);
        // The border, a blank row around the output, the title, the
        // body, and the buttons.
        let rows = page.height.saturating_sub(2 + 2 + 2 + 2 + 1);
        ((width - 4) as usize, rows.max(MIN_OUTPUT_ROWS) as usize)
    }

    fn width(page: Rect) -> u16 {
        MAX_WIDTH.min(page.width.saturating_sub(4))
    }

    pub fn title(&self) -> String {
        format!(
            "The {} hook failed ({})",
            self.failure.hook,
            self.failure.how()
        )
    }

    #[cfg(test)]
    pub fn action(&self) -> Option<&str> {
        self.action.as_deref()
    }

    #[cfg(test)]
    pub fn failure(&self) -> &HookFailure {
        &self.failure
    }

    fn press(&self, button: Button) -> HookOutcome {
        match button {
            Button::Override => HookOutcome::Override,
            Button::Dismiss => HookOutcome::Dismiss,
        }
    }

    fn lines(&self) -> Range<usize> {
        self.failure.output_lines()
    }

    /// The most the output can be scrolled back: all but a screenful.
    fn max_scroll(&self) -> usize {
        self.lines()
            .len()
            .saturating_sub(self.output_area.height as usize)
    }

    /// Scroll the output back (positive) or forward, within it.
    fn scroll_by(&mut self, lines: isize) {
        let max = self.max_scroll() as isize;
        self.scroll = (self.scroll as isize + lines).clamp(0, max) as usize;
    }

    fn page(&self) -> isize {
        (self.output_area.height.saturating_sub(1)).max(1) as isize
    }

    /// Handle a key. The box is the page's until dismissed, so a key it
    /// has no use for does nothing.
    pub fn handle_key(&mut self, key: KeyEvent) -> HookOutcome {
        match key.code {
            KeyCode::Esc => return HookOutcome::Dismiss,
            KeyCode::Enter => return self.press(self.focused),
            KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab
                if self.action.is_some() =>
            {
                self.focused = match self.focused {
                    Button::Override => Button::Dismiss,
                    Button::Dismiss => Button::Override,
                };
            }
            KeyCode::Up | KeyCode::Char('k') => self.scroll_by(1),
            KeyCode::Down | KeyCode::Char('j') => self.scroll_by(-1),
            KeyCode::PageUp => self.scroll_by(self.page()),
            KeyCode::PageDown => self.scroll_by(-self.page()),
            KeyCode::Home | KeyCode::Char('g') => self.scroll = self.max_scroll(),
            KeyCode::End | KeyCode::Char('G') => self.scroll = 0,
            _ => {}
        }
        HookOutcome::Continue
    }

    /// Whether the mouse position is over the box.
    pub fn contains(&self, x: u16, y: u16) -> bool {
        self.area.contains(ScreenPosition::new(x, y))
    }

    fn button_at(&self, x: u16, y: u16) -> Option<Button> {
        let at = ScreenPosition::new(x, y);
        if self.override_area.contains(at) {
            Some(Button::Override)
        } else if self.dismiss_area.contains(at) {
            Some(Button::Dismiss)
        } else {
            None
        }
    }

    /// Handle a mouse event anywhere on the screen. `wheel` is how many
    /// rows a wheel event scrolls.
    pub fn handle_mouse(&mut self, mouse: MouseEvent, wheel: usize) -> HookOutcome {
        let over = self.button_at(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Moved => {
                if let Some(button) = over {
                    self.focused = button;
                }
                HookOutcome::Continue
            }
            MouseEventKind::Down(MouseButton::Left) if over.is_some() => {
                self.press(over.expect("checked"))
            }
            MouseEventKind::Down(_) if !self.contains(mouse.column, mouse.row) => {
                HookOutcome::Dismiss
            }
            MouseEventKind::ScrollUp => {
                self.scroll_by(wheel as isize);
                HookOutcome::Continue
            }
            MouseEventKind::ScrollDown => {
                self.scroll_by(-(wheel as isize));
                HookOutcome::Continue
            }
            _ => HookOutcome::Continue,
        }
    }

    /// Draw the box across the middle of `page`.
    pub fn render(&mut self, page: Rect, buf: &mut Buffer, theme: &Theme) {
        self.area = Rect::default();
        self.output_area = Rect::default();
        self.override_area = Rect::default();
        self.dismiss_area = Rect::default();
        if page.width < MIN_WIDTH {
            return;
        }
        let width = Self::width(page);
        // Inside the border, a column of space either side.
        let text_width = (width - 4) as usize;
        let title = wrap_words(&self.title(), text_width);
        let body = wrap_words(&self.body, text_width);
        let text_rows = (title.len() + body.len()) as u16;
        let output_rows = (self.lines().len() as u16).max(1);
        // The border, the text, a blank row either side of the output,
        // and the buttons.
        let fixed = 2 + text_rows + 2 + 1;
        let room = page.height.saturating_sub(fixed + 2);
        if room < 1 {
            return;
        }
        let shown_rows = output_rows.min(room);
        let height = fixed + shown_rows;
        let x = page.x + (page.width - width) / 2;
        let y = page.y + (page.height - height) / 2;
        self.area = Rect::new(x, y, width, height);
        let inner = render_frame(self.area, Some(HINT), buf, theme);

        let background = palette_background(theme);
        let bold = background.add_modifier(Modifier::BOLD);
        let lines = title
            .iter()
            .map(|line| (line, bold))
            .chain(body.iter().map(|line| (line, background)));
        for (row, (line, style)) in lines.enumerate() {
            buf.set_stringn(inner.x + 1, inner.y + row as u16, line, text_width, style);
        }

        let output = Rect::new(
            inner.x + 1,
            inner.y + text_rows + 1,
            text_width as u16,
            shown_rows,
        );
        self.output_area = output;
        self.scroll = self.scroll.min(self.max_scroll());
        self.render_output(output, buf, theme);

        let focused = Style::default()
            .fg(theme.command_palette_selection_text)
            .bg(theme.command_palette_selection_background);
        let button_y = inner.bottom() - 1;
        let label = |text: &str| format!(" {text} ");
        let dismiss = label(DISMISS);
        let dismiss_width = display_width(&dismiss) as u16;
        let dismiss_x = inner.right().saturating_sub(dismiss_width + 1);
        let dismiss_style = if self.focused == Button::Dismiss {
            focused
        } else {
            background
        };
        buf.set_string(dismiss_x, button_y, &dismiss, dismiss_style);
        self.dismiss_area = Rect::new(dismiss_x, button_y, dismiss_width, 1);
        if let Some(action) = &self.action {
            let action = label(action);
            let action_width = display_width(&action) as u16;
            let action_x = dismiss_x.saturating_sub(action_width + 2);
            if action_x > inner.x {
                let action_style = if self.focused == Button::Override {
                    focused
                } else {
                    background.fg(theme.diff_removed_text)
                };
                buf.set_string(action_x, button_y, &action, action_style);
                self.override_area = Rect::new(action_x, button_y, action_width, 1);
            }
        }
    }

    /// Draw the hook's output into `area`, as its terminal had it, the
    /// lines scrolled to cut at its width.
    fn render_output(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let base = Style::default()
            .fg(theme.terminal_text)
            .bg(theme.terminal_background);
        buf.set_style(area, base);
        let lines = self.lines();
        if lines.is_empty() {
            let dim = base.add_modifier(Modifier::DIM);
            buf.set_stringn(area.x, area.y, NO_OUTPUT, area.width as usize, dim);
            return;
        }
        let rows = area.height as usize;
        let end = lines.end - self.scroll;
        let start = end.saturating_sub(rows).max(lines.start);
        let terminal = &self.failure.output;
        for (row, number) in (start..end).enumerate() {
            let Some(line) = terminal.line(number) else {
                continue;
            };
            let y = area.y + row as u16;
            let mut col = 0;
            for cell in &line.cells {
                if col >= area.width as usize {
                    break;
                }
                if cell.spacer {
                    continue;
                }
                let style = term_style(&cell.style, theme);
                let x = area.x + col as u16;
                let symbol = if cell.text.is_empty() {
                    " "
                } else {
                    &cell.text
                };
                buf[(x, y)].set_symbol(symbol).set_style(style);
                let width = if cell.wide { 2 } else { 1 };
                if cell.wide && col + 1 < area.width as usize {
                    buf[(x + 1, y)].set_symbol(" ").set_style(style);
                }
                col += width;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use ninjaedit_core::git::Hook;
    use ninjaedit_core::terminal::{ExitStatus, Terminal};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn failure(hook: Hook, output: &str) -> HookFailure {
        let mut terminal = Terminal::new(40, 5);
        terminal.process(output.as_bytes());
        HookFailure {
            hook,
            status: ExitStatus {
                code: 1,
                signal: None,
            },
            output: Box::new(terminal),
        }
    }

    fn screen_rows(buf: &Buffer) -> Vec<String> {
        let area = buf.area;
        (area.y..area.bottom())
            .map(|y| {
                (area.x..area.right())
                    .map(|x| buf[(x, y)].symbol().to_owned())
                    .collect()
            })
            .collect()
    }

    fn rendered(dialog: &mut HookBox, width: u16, height: u16) -> Vec<String> {
        let page = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(page);
        dialog.render(page, &mut buf, &Theme::default());
        screen_rows(&buf)
            .iter()
            .map(|row| {
                row.trim_matches(|c| c == ' ' || c == '│')
                    .trim_end()
                    .to_owned()
            })
            .filter(|row| !row.is_empty())
            .collect()
    }

    #[test]
    fn shows_the_hook_its_output_and_dismiss_has_the_keyboard() {
        let output = "check one... ok\r\ncheck two... \x1b[31mfailed\x1b[0m\r\n";
        let mut dialog = HookBox::new(
            failure(Hook::PreCommit, output),
            "The commit wasn't made.",
            Some("Commit anyway".to_owned()),
        );
        let rows = rendered(&mut dialog, 50, 20);
        assert_eq!(rows[1], "The pre-commit hook failed (exit code 1)");
        assert_eq!(rows[2], "The commit wasn't made.");
        assert_eq!(rows[3], "check one... ok");
        assert_eq!(rows[4], "check two... failed");
        assert_eq!(rows[5], "Commit anyway    Dismiss");
        assert!(rows[6].contains(HINT), "{rows:#?}");
        assert_eq!(dialog.handle_key(key(KeyCode::Enter)), HookOutcome::Dismiss);
        assert_eq!(dialog.handle_key(key(KeyCode::Tab)), HookOutcome::Continue);
        assert_eq!(
            dialog.handle_key(key(KeyCode::Enter)),
            HookOutcome::Override
        );
        // No y for yes: going ahead is a button pressed on purpose.
        assert_eq!(
            dialog.handle_key(key(KeyCode::Char('y'))),
            HookOutcome::Continue
        );
        assert_eq!(dialog.handle_key(key(KeyCode::Esc)), HookOutcome::Dismiss);
    }

    #[test]
    fn a_hook_that_cannot_be_skipped_only_dismisses() {
        for (hook, action) in [
            (Hook::PrepareCommitMsg, Some("Commit anyway".to_owned())),
            (Hook::PostCommit, Some("Commit anyway".to_owned())),
            (Hook::PreCommit, None),
        ] {
            let mut dialog = HookBox::new(failure(hook, "no\r\n"), "Body.", action);
            assert_eq!(dialog.action(), None);
            let rows = rendered(&mut dialog, 50, 20);
            assert_eq!(rows[rows.len() - 3], "no", "{rows:#?}");
            assert_eq!(rows[rows.len() - 2], "Dismiss", "{rows:#?}");
            dialog.handle_key(key(KeyCode::Tab));
            assert_eq!(dialog.handle_key(key(KeyCode::Enter)), HookOutcome::Dismiss);
        }
        let mut quiet = HookBox::new(failure(Hook::PreCommit, ""), "Body.", None);
        let rows = rendered(&mut quiet, 50, 20);
        assert_eq!(rows[3], NO_OUTPUT);
    }

    #[test]
    fn long_output_starts_at_its_end_and_scrolls() {
        let output: String = (1..=30).map(|n| format!("line {n}\r\n")).collect();
        let mut terminal = Terminal::new(40, 5);
        terminal.process(output.as_bytes());
        let failure = HookFailure {
            hook: Hook::CommitMsg,
            status: ExitStatus {
                code: 1,
                signal: None,
            },
            output: Box::new(terminal),
        };
        let mut dialog = HookBox::new(failure, "Body.", None);
        // 12 rows: the border, the title, the body, the blanks, and the
        // buttons leave five for the output.
        let rows = rendered(&mut dialog, 50, 12);
        assert_eq!(dialog.output_area.height, 3, "{rows:#?}");
        assert_eq!(&rows[3..6], ["line 28", "line 29", "line 30"]);
        dialog.handle_key(key(KeyCode::Up));
        let rows = rendered(&mut dialog, 50, 12);
        assert_eq!(&rows[3..6], ["line 27", "line 28", "line 29"]);
        dialog.handle_key(key(KeyCode::Home));
        let rows = rendered(&mut dialog, 50, 12);
        assert_eq!(&rows[3..6], ["line 1", "line 2", "line 3"]);
        let wheel = mouse(MouseEventKind::ScrollDown, 10, 5);
        dialog.handle_mouse(wheel, 3);
        let rows = rendered(&mut dialog, 50, 12);
        assert_eq!(&rows[3..6], ["line 4", "line 5", "line 6"]);
        dialog.handle_key(key(KeyCode::End));
        let rows = rendered(&mut dialog, 50, 12);
        assert_eq!(&rows[3..6], ["line 28", "line 29", "line 30"]);
    }

    #[test]
    fn clicks_press_buttons_and_a_press_outside_dismisses() {
        let mut dialog = HookBox::new(
            failure(Hook::PreRebase, "no\r\n"),
            "The rebase didn't start.",
            Some("Rebase anyway".to_owned()),
        );
        rendered(&mut dialog, 60, 20);
        let action = dialog.override_area;
        let dismiss = dialog.dismiss_area;
        assert!(action.width > 0 && dismiss.width > 0);
        let press = |x, y| mouse(MouseEventKind::Down(MouseButton::Left), x, y);
        assert_eq!(
            dialog.handle_mouse(press(dialog.area.x + 2, dialog.area.y + 1), 3),
            HookOutcome::Continue
        );
        assert_eq!(
            dialog.handle_mouse(press(action.x, action.y), 3),
            HookOutcome::Override
        );
        assert_eq!(
            dialog.handle_mouse(press(dismiss.x, dismiss.y), 3),
            HookOutcome::Dismiss
        );
        assert_eq!(dialog.handle_mouse(press(0, 0), 3), HookOutcome::Dismiss);
    }

    #[test]
    fn the_terminal_fits_the_box_s_output() {
        let page = Rect::new(0, 0, 100, 40);
        let (cols, rows) = HookBox::terminal_size(page);
        assert_eq!(cols, 92);
        assert_eq!(rows, 31);
        assert_eq!(HookBox::terminal_size(Rect::default()), (80, 24));
        let wide = HookBox::terminal_size(Rect::new(0, 0, 300, 40));
        assert_eq!(wide.0, (MAX_WIDTH - 4) as usize);
    }
}
