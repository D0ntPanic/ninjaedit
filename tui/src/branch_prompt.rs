//! The branch name box of the git log page: a one-line input floating
//! over the top of the page, like the go to line box over the editor,
//! into which a name for a new local branch is typed.
//!
//! It opens when a commit is checked out that only a remote's branch
//! points at, and a local branch by that branch's name already exists
//! elsewhere (see the core crate's `git::checkout` module): the new
//! branch that will track the remote's needs another name. The box
//! only owns the name's text and its look. Enter hands the name to the
//! page, which makes the branch; a name git won't take, or one a
//! branch already has, keeps the box open with its hint saying why.

use crate::clipboard::Clipboard;
use crate::input::{Input, InputKey};
use crate::palette::{MAX_WIDTH, render_frame};
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ninjaedit_core::git::Oid;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position as ScreenPosition, Rect};

const PLACEHOLDER: &str = "Name for the new branch";
/// The box's height, border included.
pub const HEIGHT: u16 = 3;

/// What the page should do after the box handled a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BranchPromptOutcome {
    /// Keep the box open.
    Continue,
    /// Escape: close the box, checking nothing out.
    Close,
    /// Enter with a name typed: make the branch by that name.
    Accept(String),
}

pub struct BranchPrompt {
    input: Input,
    /// The commit to check out once the branch is named.
    pub id: Oid,
    /// The remote's branch the new one is to track, as `origin/feature`.
    pub upstream: String,
    /// The local branch whose name is taken, which is why a name is
    /// asked for.
    taken: String,
    /// Why the last name given was refused, shown in the bottom border
    /// until the text changes.
    refused: Option<String>,
    /// The whole box, including its border, from the last render.
    area: Rect,
}

impl BranchPrompt {
    pub fn new(id: Oid, upstream: String, taken: String) -> BranchPrompt {
        BranchPrompt {
            input: Input::new(),
            id,
            upstream,
            taken,
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

    pub fn handle_key(&mut self, key: KeyEvent, clipboard: &mut Clipboard) -> BranchPromptOutcome {
        match key.code {
            KeyCode::Esc => BranchPromptOutcome::Close,
            KeyCode::Enter => BranchPromptOutcome::Accept(self.name().to_owned()),
            _ => {
                if self.input.handle_key(key, clipboard) == InputKey::Changed {
                    self.refused = None;
                }
                BranchPromptOutcome::Continue
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

    /// Draw the box over the top center of `page`. Returns where the
    /// terminal cursor belongs.
    pub fn render(
        &mut self,
        page: Rect,
        buf: &mut Buffer,
        theme: &Theme,
    ) -> Option<ScreenPosition> {
        let width = MAX_WIDTH.min(page.width.saturating_sub(2));
        if width < 10 || page.height < HEIGHT + 1 {
            self.area = Rect::default();
            return None;
        }
        let x = page.x + (page.width - width) / 2;
        self.area = Rect::new(x, page.y + 1, width, HEIGHT);
        let hint = match &self.refused {
            Some(why) => why.clone(),
            None => format!(
                "branch {} exists · new branch will track {}",
                self.taken, self.upstream
            ),
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
