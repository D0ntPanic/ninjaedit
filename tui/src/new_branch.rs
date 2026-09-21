//! The new branch box ("Create branch" in the command palette): a
//! one-line input floating over the top of the screen, like the go to
//! line box, into which the name of a branch to make is typed.
//!
//! The branch is made where HEAD is in the repository the status bar
//! is showing (see the `heads` module), and HEAD moves on to it,
//! leaving every file as it is; the box's hint names the branch or
//! commit it is made from. The box only owns the name's text and its
//! look. Enter hands the name to the application, which makes the
//! branch; a name git won't take, or one a branch already has, keeps
//! the box open with its hint saying why.

use crate::clipboard::Clipboard;
use crate::heads::Repository;
use crate::input::{Input, InputKey};
use crate::palette::{MAX_WIDTH, render_frame};
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ninjaedit_core::git::Head;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position as ScreenPosition, Rect};

const PLACEHOLDER: &str = "Name for the new branch";
const EMPTY_HINT: &str = "enter a name for the branch";
/// The box's height, border included.
pub const HEIGHT: u16 = 3;

/// What the application should do after the box handled a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NewBranchOutcome {
    /// Keep the box open.
    Continue,
    /// Escape: close the box, making nothing.
    Close,
    /// Enter with a name typed: make the branch by that name.
    Accept(String),
}

pub struct NewBranchBox {
    input: Input,
    /// The repository the branch is to be made in, as the status bar
    /// had it when the box opened.
    pub repository: Repository,
    /// Where HEAD was then: what the branch is made from, for the hint.
    from: Head,
    /// Why the last name given was refused, shown in the bottom border
    /// until the text changes.
    refused: Option<String>,
    /// The whole box, including its border, from the last render.
    area: Rect,
}

impl NewBranchBox {
    pub fn new(repository: Repository, from: Head) -> NewBranchBox {
        NewBranchBox {
            input: Input::new(),
            repository,
            from,
            refused: None,
            area: Rect::default(),
        }
    }

    /// The name typed so far, trimmed.
    pub fn name(&self) -> &str {
        self.input.text().trim()
    }

    /// Say why the name given was refused; the box stays open.
    pub fn refuse(&mut self, why: String) {
        self.refused = Some(why);
    }

    pub fn handle_key(&mut self, key: KeyEvent, clipboard: &mut Clipboard) -> NewBranchOutcome {
        match key.code {
            KeyCode::Esc => NewBranchOutcome::Close,
            KeyCode::Enter => {
                if self.name().is_empty() {
                    self.refuse(EMPTY_HINT.to_owned());
                    NewBranchOutcome::Continue
                } else {
                    NewBranchOutcome::Accept(self.name().to_owned())
                }
            }
            _ => {
                if self.input.handle_key(key, clipboard) == InputKey::Changed {
                    self.refused = None;
                }
                NewBranchOutcome::Continue
            }
        }
    }

    /// Add pasted text to the name.
    pub fn paste(&mut self, text: &str) {
        if self.input.paste(text) {
            self.refused = None;
        }
    }

    /// Whether the mouse position is over the box.
    pub fn contains(&self, x: u16, y: u16) -> bool {
        self.area.contains(ScreenPosition::new(x, y))
    }

    /// Whether a drag that started in the text is going on, in which
    /// case the box wants drag and release events wherever they happen.
    pub fn is_dragging(&self) -> bool {
        self.input.is_dragging()
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        self.input.handle_mouse(mouse);
    }

    /// Draw the box over the top center of `screen`. Returns where the
    /// terminal cursor belongs.
    pub fn render(
        &mut self,
        screen: Rect,
        buf: &mut Buffer,
        theme: &Theme,
    ) -> Option<ScreenPosition> {
        let width = MAX_WIDTH.min(screen.width.saturating_sub(2));
        if width < 10 || screen.height < HEIGHT + 1 {
            self.area = Rect::default();
            return None;
        }
        let x = screen.x + (screen.width - width) / 2;
        self.area = Rect::new(x, screen.y + 1, width, HEIGHT);
        let hint = match &self.refused {
            Some(why) => why.clone(),
            None => match &self.from {
                Head::Branch(name) => format!("from {name}"),
                Head::Commit(id) => format!("at commit {id}"),
            },
        };
        let inner = render_frame(self.area, Some(&hint), buf, theme);
        if inner.height == 0 {
            return None;
        }
        self.input.render(
            Rect::new(inner.x, inner.y, inner.width, 1),
            PLACEHOLDER,
            buf,
            theme,
        )
    }
}
