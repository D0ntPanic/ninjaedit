//! Right-click menus: a short list of commands floating at the mouse
//! pointer, for what can be done to the thing under it. The command
//! palette (Ctrl+P) is the keyboard's way to the same commands; this is
//! the mouse's, for the few that are wanted right where one is looking
//! (cut, copy, and paste over the text, later merging and the like over
//! a commit).
//!
//! A menu is opened with a list of [`MenuEntry`]s: the commands it
//! offers, with separators between groups of them. It lists only the
//! commands that apply to what is showing, deciding with the same
//! [`Command::is_available`] the palette uses, so the two can never
//! disagree about what can be done. Separators are tidied after the
//! filtering: a group that is filtered out altogether leaves no doubled
//! separator behind, and none is shown first or last. With no command
//! left at all there is nothing to show, and [`ContextMenu::new`] gives
//! no menu.
//!
//! The menu is placed with its top left corner at the pointer, moved
//! left or up as needed to fit on the screen. The pointer highlights the
//! row it is over, and a click on a row runs its command; the arrow
//! keys and Enter do the same from the keyboard, and Escape closes the
//! menu. Any other key closes it too and then does what it always does:
//! the menu is a suggestion, not a mode, so a Ctrl+C typed with it open
//! still copies.

use crate::command::{Command, Context};
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position as ScreenPosition, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, BorderType, Clear, Widget};

/// One line of the list a menu is opened with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuEntry {
    Command(Command),
    /// A line between groups of commands.
    Separator,
}

/// What the application should do after the menu handled an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuOutcome {
    /// Keep the menu open.
    Continue,
    /// Close the menu; the event is spent.
    Close,
    /// Close the menu and run the command.
    Activate(Command),
    /// The event wasn't the menu's: a key it has no use for, or a press
    /// outside it. Close the menu and let the event go on to whatever it
    /// was meant for.
    Unhandled,
}

pub struct ContextMenu {
    /// The entries left after filtering and tidying: at least one
    /// command, with no separator first, last, or next to another.
    entries: Vec<MenuEntry>,
    /// Index into `entries` of the highlighted row, always a command;
    /// `None` until the pointer or a key picks one.
    selected: Option<usize>,
    /// Where the menu was opened: the pointer's position.
    at: ScreenPosition,
    /// The whole menu, including its border, from the last render.
    area: Rect,
    /// The entry rows from the last render.
    rows: Rect,
}

/// The menu's colors, each the command palette's counterpart unless the
/// theme sets it.
struct Colors {
    background: Color,
    border: Color,
    text: Color,
    shortcut: Color,
    selection_background: Color,
    selection_text: Color,
}

impl Colors {
    fn of(theme: &Theme) -> Colors {
        Colors {
            background: theme
                .context_menu_background
                .unwrap_or(theme.command_palette_background),
            border: theme
                .context_menu_box_color
                .unwrap_or(theme.command_palette_box_color),
            text: theme
                .context_menu_text
                .unwrap_or(theme.command_palette_result_text),
            shortcut: theme
                .context_menu_shortcut_text
                .unwrap_or(theme.command_palette_result_context_text),
            selection_background: theme
                .context_menu_selection_background
                .unwrap_or(theme.command_palette_selection_background),
            selection_text: theme
                .context_menu_selection_text
                .unwrap_or(theme.command_palette_selection_text),
        }
    }
}

/// Keep the entries whose commands apply, and tidy the separators
/// between them so that none comes first, last, or right after another.
fn applicable(entries: &[MenuEntry], context: &Context) -> Vec<MenuEntry> {
    let mut kept: Vec<MenuEntry> = Vec::new();
    for &entry in entries {
        match entry {
            MenuEntry::Command(command) if command.is_available(context) => kept.push(entry),
            MenuEntry::Command(_) => {}
            MenuEntry::Separator => {
                if kept
                    .last()
                    .is_some_and(|last| *last != MenuEntry::Separator)
                {
                    kept.push(entry);
                }
            }
        }
    }
    if kept.last() == Some(&MenuEntry::Separator) {
        kept.pop();
    }
    kept
}

impl ContextMenu {
    /// A menu of the `entries` that apply in `context`, opened at the
    /// pointer's position, or `None` if no command does: there would be
    /// nothing to show.
    pub fn new(entries: &[MenuEntry], context: &Context, x: u16, y: u16) -> Option<ContextMenu> {
        let entries = applicable(entries, context);
        if entries.is_empty() {
            return None;
        }
        Some(ContextMenu {
            entries,
            selected: None,
            at: ScreenPosition::new(x, y),
            area: Rect::default(),
            rows: Rect::default(),
        })
    }

    /// The commands the menu lists, in order, with `None` for each
    /// separator.
    #[cfg(test)]
    pub fn commands(&self) -> Vec<Option<Command>> {
        self.entries
            .iter()
            .map(|entry| match entry {
                MenuEntry::Command(command) => Some(*command),
                MenuEntry::Separator => None,
            })
            .collect()
    }

    /// The highlighted command, if any.
    pub fn selected(&self) -> Option<Command> {
        match self.selected.map(|index| self.entries[index]) {
            Some(MenuEntry::Command(command)) => Some(command),
            _ => None,
        }
    }

    /// Whether the mouse position is over the menu.
    pub fn contains(&self, x: u16, y: u16) -> bool {
        self.area.contains(ScreenPosition::new(x, y))
    }

    /// The indices of the command entries, in order.
    fn command_indices(&self) -> impl DoubleEndedIterator<Item = usize> + '_ {
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| matches!(entry, MenuEntry::Command(_)))
            .map(|(index, _)| index)
    }

    /// Highlight the next command down (or up), wrapping around the
    /// ends; with nothing highlighted, the first (or last).
    fn step(&mut self, down: bool) {
        let next = match (self.selected, down) {
            (Some(current), true) => self.command_indices().find(|&i| i > current),
            (Some(current), false) => self.command_indices().rev().find(|&i| i < current),
            (None, _) => None,
        };
        self.selected = next.or_else(|| {
            if down {
                self.command_indices().next()
            } else {
                self.command_indices().next_back()
            }
        });
    }

    fn activate(&self) -> MenuOutcome {
        match self.selected() {
            Some(command) => MenuOutcome::Activate(command),
            None => MenuOutcome::Continue,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> MenuOutcome {
        match key.code {
            KeyCode::Esc => MenuOutcome::Close,
            KeyCode::Enter => self.activate(),
            KeyCode::Down => {
                self.step(true);
                MenuOutcome::Continue
            }
            KeyCode::Up => {
                self.step(false);
                MenuOutcome::Continue
            }
            KeyCode::Home => {
                let first = self.command_indices().next();
                self.selected = first;
                MenuOutcome::Continue
            }
            KeyCode::End => {
                let last = self.command_indices().next_back();
                self.selected = last;
                MenuOutcome::Continue
            }
            _ => MenuOutcome::Unhandled,
        }
    }

    /// The index of the command entry on the row under a position, if
    /// the position is over one (and not over a separator or the
    /// border).
    fn command_at(&self, x: u16, y: u16) -> Option<usize> {
        if !self.rows.contains(ScreenPosition::new(x, y)) {
            return None;
        }
        let index = (y - self.rows.y) as usize;
        match self.entries.get(index) {
            Some(MenuEntry::Command(_)) => Some(index),
            _ => None,
        }
    }

    /// Handle a mouse event anywhere on the screen. The pointer
    /// highlights the row it is over, and a press on one runs it; a
    /// press anywhere else is [`MenuOutcome::Unhandled`], for the
    /// application to close the menu and act on the press itself. The
    /// release of the button that opened the menu, which arrives once it
    /// is open, does nothing, so a menu that had to be moved over the
    /// pointer to fit on the screen isn't run by the click that opened
    /// it.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> MenuOutcome {
        let over = self.command_at(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Moved | MouseEventKind::Drag(_) => {
                self.selected = over;
                MenuOutcome::Continue
            }
            MouseEventKind::Down(MouseButton::Left | MouseButton::Right) => match over {
                Some(index) => {
                    self.selected = Some(index);
                    self.activate()
                }
                None if self.contains(mouse.column, mouse.row) => MenuOutcome::Continue,
                None => MenuOutcome::Unhandled,
            },
            MouseEventKind::Down(MouseButton::Middle) => {
                if self.contains(mouse.column, mouse.row) {
                    MenuOutcome::Continue
                } else {
                    MenuOutcome::Unhandled
                }
            }
            _ => MenuOutcome::Continue,
        }
    }

    /// The width the rows need: a space, the widest label, two spaces
    /// and the widest shortcut when there is one, and a space.
    fn rows_width(&self) -> u16 {
        let (mut label, mut shortcut) = (0, 0);
        for entry in &self.entries {
            if let MenuEntry::Command(command) = entry {
                label = label.max(Span::raw(command.label()).width());
                if let Some(key) = command.shortcut() {
                    shortcut = shortcut.max(Span::raw(key).width() + 2);
                }
            }
        }
        (2 + label + shortcut) as u16
    }

    /// Draw the menu at the pointer, or as near it as fits on `screen`.
    pub fn render(&mut self, screen: Rect, buf: &mut Buffer, theme: &Theme) {
        let width = (self.rows_width() + 2).min(screen.width);
        let height = (self.entries.len() as u16 + 2).min(screen.height);
        if width < 4 || height < 3 {
            self.area = Rect::default();
            self.rows = Rect::default();
            return;
        }
        // The top left corner sits at the pointer. With no room to the
        // right the menu is moved left until it fits; with no room below
        // it is opened upward, its bottom at the pointer, or failing that
        // moved up until it fits.
        let x = self
            .at
            .x
            .min(screen.right().saturating_sub(width))
            .max(screen.x);
        let y = if self.at.y + height <= screen.bottom() {
            self.at.y
        } else if self.at.y + 1 >= screen.y + height {
            self.at.y + 1 - height
        } else {
            screen.bottom() - height
        }
        .max(screen.y);
        self.area = Rect::new(x, y, width, height);

        let colors = Colors::of(theme);
        let background = Style::default().fg(colors.text).bg(colors.background);
        let border = background.fg(colors.border);
        Clear.render(self.area, buf);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .style(background)
            .border_style(border);
        self.rows = block.inner(self.area);
        block.render(self.area, buf);

        let width = self.rows.width as usize;
        for (index, entry) in self
            .entries
            .iter()
            .enumerate()
            .take(self.rows.height as usize)
        {
            let y = self.rows.y + index as u16;
            match entry {
                MenuEntry::Separator => {
                    buf.set_string(self.area.x, y, "├", border);
                    buf.set_string(self.rows.x, y, "─".repeat(width), border);
                    buf.set_string(self.area.right() - 1, y, "┤", border);
                }
                MenuEntry::Command(command) => {
                    let (base, key) = if self.selected == Some(index) {
                        let base = Style::default()
                            .fg(colors.selection_text)
                            .bg(colors.selection_background);
                        buf.set_style(Rect::new(self.rows.x, y, self.rows.width, 1), base);
                        (base, base)
                    } else {
                        (background, background.fg(colors.shortcut))
                    };
                    // The shortcut sits at the right end of the row when
                    // there is room for it after the label.
                    let shortcut = command
                        .shortcut()
                        .map(|shortcut| (shortcut, Span::raw(shortcut).width()))
                        .filter(|(_, needed)| {
                            1 + Span::raw(command.label()).width() + 2 + needed < width
                        });
                    let label_width = width.saturating_sub(2);
                    buf.set_stringn(self.rows.x + 1, y, command.label(), label_width, base);
                    if let Some((shortcut, needed)) = shortcut {
                        let x = self.rows.x + (width - 1 - needed) as u16;
                        buf.set_string(x, y, shortcut, key);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::FileContext;
    use crossterm::event::KeyModifiers;

    const MENU: &[MenuEntry] = &[
        MenuEntry::Command(Command::Undo),
        MenuEntry::Command(Command::Redo),
        MenuEntry::Separator,
        MenuEntry::Command(Command::Cut),
        MenuEntry::Command(Command::Copy),
        MenuEntry::Command(Command::Paste),
        MenuEntry::Separator,
        MenuEntry::Command(Command::SelectAll),
    ];

    fn file(file: FileContext) -> Context {
        Context {
            file: Some(file),
            ..Context::default()
        }
    }

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

    #[test]
    fn lists_only_the_commands_that_apply_and_tidies_the_separators() {
        // Everything applies: the list as given.
        let all = file(FileContext {
            can_undo: true,
            can_redo: true,
            has_selection: true,
            ..FileContext::default()
        });
        let menu = ContextMenu::new(MENU, &all, 0, 0).unwrap();
        assert_eq!(
            menu.commands(),
            [
                Some(Command::Undo),
                Some(Command::Redo),
                None,
                Some(Command::Cut),
                Some(Command::Copy),
                Some(Command::Paste),
                None,
                Some(Command::SelectAll),
            ]
        );

        // A fresh file with nothing selected: the first group goes,
        // taking the separator that would come first with it, and the
        // middle group shrinks to Paste.
        let fresh = file(FileContext::default());
        let menu = ContextMenu::new(MENU, &fresh, 0, 0).unwrap();
        assert_eq!(
            menu.commands(),
            [Some(Command::Paste), None, Some(Command::SelectAll)]
        );

        // A whole group in the middle filtered out leaves one separator,
        // not two, and one at the end is dropped.
        let entries = [
            MenuEntry::Command(Command::Paste),
            MenuEntry::Separator,
            MenuEntry::Command(Command::Cut),
            MenuEntry::Command(Command::Copy),
            MenuEntry::Separator,
            MenuEntry::Command(Command::SelectAll),
            MenuEntry::Separator,
            MenuEntry::Command(Command::Undo),
            MenuEntry::Separator,
        ];
        let menu = ContextMenu::new(&entries, &fresh, 0, 0).unwrap();
        assert_eq!(
            menu.commands(),
            [Some(Command::Paste), None, Some(Command::SelectAll)]
        );
        // Separators given in a row are one.
        let entries = [
            MenuEntry::Separator,
            MenuEntry::Separator,
            MenuEntry::Command(Command::Paste),
            MenuEntry::Separator,
            MenuEntry::Separator,
            MenuEntry::Command(Command::SelectAll),
        ];
        let menu = ContextMenu::new(&entries, &fresh, 0, 0).unwrap();
        assert_eq!(
            menu.commands(),
            [Some(Command::Paste), None, Some(Command::SelectAll)]
        );
    }

    #[test]
    fn no_applicable_command_means_no_menu() {
        // No file open: none of the editor's commands apply.
        assert!(ContextMenu::new(MENU, &Context::default(), 0, 0).is_none());
        let only_separators = [MenuEntry::Separator, MenuEntry::Separator];
        assert!(ContextMenu::new(&only_separators, &Context::default(), 0, 0).is_none());
        assert!(ContextMenu::new(&[], &Context::default(), 0, 0).is_none());
    }

    #[test]
    fn keys_step_over_separators_and_wrap() {
        let context = file(FileContext::default());
        let mut menu = ContextMenu::new(MENU, &context, 0, 0).unwrap();
        // Paste, separator, Select all. Nothing is highlighted to begin
        // with, so Enter does nothing.
        assert_eq!(menu.selected(), None);
        assert_eq!(menu.handle_key(key(KeyCode::Enter)), MenuOutcome::Continue);
        assert_eq!(menu.handle_key(key(KeyCode::Down)), MenuOutcome::Continue);
        assert_eq!(menu.selected(), Some(Command::Paste));
        menu.handle_key(key(KeyCode::Down));
        assert_eq!(menu.selected(), Some(Command::SelectAll));
        menu.handle_key(key(KeyCode::Down));
        assert_eq!(menu.selected(), Some(Command::Paste));
        menu.handle_key(key(KeyCode::Up));
        assert_eq!(menu.selected(), Some(Command::SelectAll));
        menu.handle_key(key(KeyCode::Home));
        assert_eq!(menu.selected(), Some(Command::Paste));
        menu.handle_key(key(KeyCode::End));
        assert_eq!(menu.selected(), Some(Command::SelectAll));
        assert_eq!(
            menu.handle_key(key(KeyCode::Enter)),
            MenuOutcome::Activate(Command::SelectAll)
        );
        assert_eq!(menu.handle_key(key(KeyCode::Esc)), MenuOutcome::Close);
        assert_eq!(
            menu.handle_key(key(KeyCode::Char('x'))),
            MenuOutcome::Unhandled
        );

        // Up from nothing highlights the last command.
        let mut menu = ContextMenu::new(MENU, &context, 0, 0).unwrap();
        menu.handle_key(key(KeyCode::Up));
        assert_eq!(menu.selected(), Some(Command::SelectAll));
    }

    #[test]
    fn renders_at_the_pointer_with_shortcuts_and_separators() {
        let context = file(FileContext {
            can_undo: true,
            ..FileContext::default()
        });
        let mut menu = ContextMenu::new(MENU, &context, 3, 1).unwrap();
        let screen = Rect::new(0, 0, 30, 10);
        let mut buf = Buffer::empty(screen);
        menu.render(screen, &mut buf, &Theme::default());
        let rows = screen_rows(&buf);
        assert_eq!(&rows[1][3..], "╭────────────────────╮     ");
        assert_eq!(&rows[2][3..], "│ Undo        Ctrl+Z │     ");
        assert_eq!(&rows[3][3..], "├────────────────────┤     ");
        assert_eq!(&rows[4][3..], "│ Paste       Ctrl+V │     ");
        assert_eq!(&rows[5][3..], "├────────────────────┤     ");
        assert_eq!(&rows[6][3..], "│ Select all  Ctrl+A │     ");
        assert_eq!(&rows[7][3..], "╰────────────────────╯     ");
        assert!(rows[0].trim().is_empty());
        assert!(rows[8].trim().is_empty());
        assert!(menu.contains(3, 1));
        assert!(menu.contains(24, 7));
        assert!(!menu.contains(25, 7));
        assert!(!menu.contains(3, 8));

        // The pointer highlights the row it is over, but not a separator
        // or the border; a press on a row runs it.
        assert_eq!(
            menu.handle_mouse(mouse(MouseEventKind::Moved, 10, 4)),
            MenuOutcome::Continue
        );
        assert_eq!(menu.selected(), Some(Command::Paste));
        menu.handle_mouse(mouse(MouseEventKind::Moved, 10, 5));
        assert_eq!(menu.selected(), None);
        menu.handle_mouse(mouse(MouseEventKind::Moved, 10, 1));
        assert_eq!(menu.selected(), None);
        assert_eq!(
            menu.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 10, 5)),
            MenuOutcome::Continue
        );
        assert_eq!(
            menu.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 10, 6)),
            MenuOutcome::Activate(Command::SelectAll)
        );
        // A press outside is for the application to deal with; the
        // release of the button that opened the menu is nothing.
        assert_eq!(
            menu.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 0, 0)),
            MenuOutcome::Unhandled
        );
        assert_eq!(
            menu.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Right), 10, 6)),
            MenuOutcome::Continue
        );
    }

    #[test]
    fn moves_to_fit_on_the_screen() {
        let context = file(FileContext::default());
        let theme = Theme::default();
        let screen = Rect::new(0, 0, 30, 10);
        // Paste, separator, Select all: 5 rows, 22 columns.
        let mut menu = ContextMenu::new(MENU, &context, 20, 2).unwrap();
        let mut buf = Buffer::empty(screen);
        menu.render(screen, &mut buf, &theme);
        assert_eq!(menu.area, Rect::new(8, 2, 22, 5));

        // No room below: opened upward, its bottom row at the pointer.
        let mut menu = ContextMenu::new(MENU, &context, 2, 8).unwrap();
        menu.render(screen, &mut buf, &theme);
        assert_eq!(menu.area, Rect::new(2, 4, 22, 5));

        // No room either way: moved up as far as it takes.
        let short = Rect::new(0, 0, 30, 6);
        let mut menu = ContextMenu::new(MENU, &context, 2, 3).unwrap();
        menu.render(short, &mut buf, &theme);
        assert_eq!(menu.area, Rect::new(2, 1, 22, 5));

        // A screen too small for a menu at all draws nothing.
        let tiny = Rect::new(0, 0, 30, 2);
        let mut menu = ContextMenu::new(MENU, &context, 2, 0).unwrap();
        menu.render(tiny, &mut buf, &theme);
        assert_eq!(menu.area, Rect::default());
        assert!(!menu.contains(2, 0));
    }
}
