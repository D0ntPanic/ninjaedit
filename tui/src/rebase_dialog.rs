//! The dialog that plans an interactive rebase, floating over the git
//! log page that opened it: the commits to replay as a graph, newest
//! first as the log lists them, each with what to do with it, which the
//! user changes and reorders before starting the rebase (see the core
//! crate's `git::interactive` module).
//!
//! Each commit has a dropdown at its left for its action: pick it, drop
//! it, stop to edit it (or reword it: in the editor that is the same
//! stop), or fold it into the commit below it, as a squash (its message
//! after that commit's, to be written together) or a fixup (its message
//! dropped). The graph beside them shows what comes of it: the line on
//! the left is the commits the branch ends up with, down to the commit
//! the rebase goes onto, under them all; a squash (`●`) or a fixup
//! (`○`) branches off it to the right and joins it again at the commit
//! it folds into, a run of them together; a dropped commit has no node,
//! and its text is struck through.
//!
//! ↑ and ↓ select a commit, and Alt+↑ and Alt+↓ move it, as Alt+↑ and
//! Alt+↓ move a list's items on the settings page; a commit can be
//! dragged with the mouse too. Enter or Space opens the selected
//! commit's dropdown (as does a click on it), where ↑ ↓ and Enter pick
//! an action, or the action's first letter does at once, as in git's
//! list of steps: `p`, `d`, `e`, `s`, `f`. A squash or a fixup needs a
//! commit kept below it to fold into, so they aren't offered for the
//! bottom one; one that moving or dropping commits leaves with nothing
//! to fold into is named under the graph, and the rebase can't start
//! until that is fixed. Ctrl+S, or the Start button, starts it; Escape,
//! or Cancel, closes the dialog. Tab moves the keyboard between the
//! graph and the buttons, as in the confirm box. A press outside the
//! dialog does nothing, so that a plan worked on isn't lost to a stray
//! click.

use crate::commit_row::lane_palette;
use crate::diff_pane::display_width;
use crate::palette::{palette_background, render_frame};
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ninjaedit_core::git::{NODE, RebaseAction, RebasePlan, RebaseStep, short_id};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position as ScreenPosition, Rect};
use ratatui::style::{Modifier, Style};

/// The actions the dropdown offers, in its order. Reword is left out:
/// here it is the same stop as an edit.
const ACTIONS: [RebaseAction; 5] = [
    RebaseAction::Pick,
    RebaseAction::Drop,
    RebaseAction::Edit,
    RebaseAction::Squash,
    RebaseAction::Fixup,
];
/// The dropdown control's width: `[squash ▾]`.
const CONTROL_WIDTH: u16 = 10;
/// The graph's width: the main line, a gap, and the folds' line.
const GRAPH_WIDTH: u16 = 3;
/// The node of a fixup, whose message is dropped.
const FIXUP_NODE: &str = "○";
const START: &str = "Start rebase";
const CANCEL: &str = "Cancel";
/// The smallest dialog worth drawing.
const MIN_WIDTH: u16 = 40;
const MIN_HEIGHT: u16 = 12;

/// What the page should do after the dialog handled an event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RebaseDialogOutcome {
    /// Keep the dialog open.
    Continue,
    /// Close the dialog, doing nothing.
    Cancel,
    /// Close the dialog and start the rebase the plan describes.
    Start(RebasePlan),
}

/// What has the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    Graph,
    Start,
    Cancel,
}

/// A commit of the plan, as the dialog shows it.
#[derive(Clone, Debug)]
pub struct PlanCommit {
    pub step: RebaseStep,
    pub author: String,
    /// When it was made, as the log shows it.
    pub time: String,
}

/// The selected commit's dropdown, open.
struct Dropdown {
    /// Which commit it is for.
    row: usize,
    /// The action under the keyboard, an index into [`ACTIONS`].
    highlighted: usize,
    /// Where it was drawn.
    area: Rect,
}

/// Which line of the graph a commit's row draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Line {
    Node,
    Transition,
}

/// Which line of the graph a cell is part of, for its color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lane {
    /// The commits the branch ends up with.
    Main,
    /// A run of folds, joining the main line at the commit they fold
    /// into.
    Fold,
}

/// A cell of the graph: its glyph, and which line it is part of.
type Cell = (&'static str, Option<Lane>);

const EMPTY: Cell = (" ", None);

/// The graph for commits with these actions, newest first: for each, its
/// node line and the line under it, [`GRAPH_WIDTH`] cells each. The
/// commit the rebase goes onto is under the last, on the main line, and
/// a run of folds at the bottom joins that (which the plan refuses).
fn graph(actions: &[RebaseAction]) -> Vec<[[Cell; 3]; 2]> {
    let mut rows = Vec::with_capacity(actions.len());
    // Whether the main line has begun above, and whether a run of folds
    // is coming down the right.
    let mut main = false;
    let mut folding = false;
    for (index, &action) in actions.iter().enumerate() {
        let passing = |on: bool, glyph| if on { (glyph, Some(Lane::Main)) } else { EMPTY };
        let mut node = [passing(main, "│"), EMPTY, EMPTY];
        if action.keeps() {
            node[0] = (NODE, Some(Lane::Main));
            main = true;
        }
        if action.folds() {
            let glyph = if action == RebaseAction::Fixup {
                FIXUP_NODE
            } else {
                NODE
            };
            node[2] = (glyph, Some(Lane::Fold));
            folding = true;
        } else if folding {
            node[2] = ("│", Some(Lane::Fold));
        }
        // A run of folds joins the main line above the commit it folds
        // into: the next one kept, or the commit gone onto.
        let next_kept = actions.get(index + 1).is_none_or(|next| next.keeps());
        let transition = if folding && next_kept {
            folding = false;
            let corner = if main { "├" } else { "╭" };
            main = true;
            [
                (corner, Some(Lane::Main)),
                ("─", Some(Lane::Fold)),
                ("╯", Some(Lane::Fold)),
            ]
        } else {
            let fold = if folding {
                ("│", Some(Lane::Fold))
            } else {
                EMPTY
            };
            [passing(main, "│"), EMPTY, fold]
        };
        rows.push([node, transition]);
    }
    rows
}

pub struct RebaseDialog {
    /// The plan as it was made, whose steps the dialog replaces.
    plan: RebasePlan,
    /// The commits, newest first.
    commits: Vec<PlanCommit>,
    /// The summary of the commit gone onto.
    onto_summary: String,
    selected: usize,
    /// How many lines of the graph are scrolled off its top.
    scroll: usize,
    /// Whether the selection is to be scrolled into view at the next
    /// render.
    reveal: bool,
    focus: Focus,
    dropdown: Option<Dropdown>,
    /// Whether a commit is being dragged: the selected one.
    dragging: bool,
    /// Why the plan can't start, after Ctrl+S refused it, until it
    /// changes.
    refused: Option<String>,
    /// From the last render: the whole dialog, the graph's rows, the
    /// column the dropdown controls are in, and the buttons.
    area: Rect,
    graph_area: Rect,
    control_x: u16,
    start_area: Rect,
    cancel_area: Rect,
}

impl RebaseDialog {
    /// A dialog for `plan`, with `commits` its steps' commits as the
    /// log shows them, oldest first as the plan has them, and the
    /// summary of the commit it goes onto. The newest commit is
    /// selected.
    pub fn new(plan: RebasePlan, mut commits: Vec<PlanCommit>, onto_summary: String) -> Self {
        commits.reverse();
        RebaseDialog {
            plan,
            commits,
            onto_summary,
            selected: 0,
            scroll: 0,
            reveal: true,
            focus: Focus::Graph,
            dropdown: None,
            dragging: false,
            refused: None,
            area: Rect::default(),
            graph_area: Rect::default(),
            control_x: 0,
            start_area: Rect::default(),
            cancel_area: Rect::default(),
        }
    }

    /// The plan as the dialog has it now: the commits oldest first, with
    /// the actions chosen.
    pub fn plan(&self) -> RebasePlan {
        RebasePlan {
            steps: self
                .commits
                .iter()
                .rev()
                .map(|commit| commit.step.clone())
                .collect(),
            ..self.plan.clone()
        }
    }

    /// The actions of the commits, newest first.
    fn actions(&self) -> Vec<RebaseAction> {
        self.commits
            .iter()
            .map(|commit| commit.step.action)
            .collect()
    }

    /// Whether the commit at `row` has one kept below it to fold into.
    fn can_fold(&self, row: usize) -> bool {
        self.commits[row + 1..]
            .iter()
            .any(|commit| commit.step.action.keeps())
    }

    /// Whether `action` can be chosen for the commit at `row`.
    fn allows(&self, row: usize, action: RebaseAction) -> bool {
        !action.folds() || self.can_fold(row)
    }

    /// Give the commit at `row` an action, if it can have it.
    fn set_action(&mut self, row: usize, action: RebaseAction) {
        if self.allows(row, action) {
            self.commits[row].step.action = action;
            self.refused = None;
        }
    }

    fn select(&mut self, row: usize) {
        self.selected = row.min(self.commits.len().saturating_sub(1));
        self.reveal = true;
    }

    /// Move the selected commit to `row`, the selection with it.
    fn move_selected(&mut self, row: usize) {
        let row = row.min(self.commits.len().saturating_sub(1));
        if row == self.selected {
            return;
        }
        let commit = self.commits.remove(self.selected);
        self.commits.insert(row, commit);
        self.refused = None;
        self.select(row);
    }

    /// Ctrl+S, and the Start button: start the rebase, unless the plan
    /// can't be carried out, when the dialog says why.
    fn start(&mut self) -> RebaseDialogOutcome {
        let plan = self.plan();
        match plan.check() {
            Ok(()) => RebaseDialogOutcome::Start(plan),
            Err(err) => {
                self.refused = Some(format!("Can't start: {err}"));
                RebaseDialogOutcome::Continue
            }
        }
    }

    /// Why the plan can't start as it stands, if it can't.
    fn problem(&self) -> Option<String> {
        self.plan().check().err().map(|err| {
            let mut text = err.to_string();
            if let Some(first) = text.get(..1) {
                text.replace_range(..1, &first.to_uppercase());
            }
            text
        })
    }

    // ----- Keys -----------------------------------------------------------

    pub fn handle_key(&mut self, key: KeyEvent) -> RebaseDialogOutcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        if self.dropdown.is_some() {
            return self.handle_dropdown_key(key);
        }
        match key.code {
            KeyCode::Char('s') if ctrl => return self.start(),
            KeyCode::Esc => return RebaseDialogOutcome::Cancel,
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Graph => Focus::Start,
                    Focus::Start => Focus::Cancel,
                    Focus::Cancel => Focus::Graph,
                };
                return RebaseDialogOutcome::Continue;
            }
            KeyCode::BackTab => {
                self.focus = match self.focus {
                    Focus::Graph => Focus::Cancel,
                    Focus::Start => Focus::Graph,
                    Focus::Cancel => Focus::Start,
                };
                return RebaseDialogOutcome::Continue;
            }
            _ => {}
        }
        match self.focus {
            Focus::Start | Focus::Cancel => {
                match key.code {
                    KeyCode::Enter | KeyCode::Char(' ') if self.focus == Focus::Start => {
                        return self.start();
                    }
                    KeyCode::Enter | KeyCode::Char(' ') => return RebaseDialogOutcome::Cancel,
                    KeyCode::Left | KeyCode::Right => {
                        self.focus = if self.focus == Focus::Start {
                            Focus::Cancel
                        } else {
                            Focus::Start
                        };
                    }
                    _ => {}
                }
                RebaseDialogOutcome::Continue
            }
            Focus::Graph => {
                self.handle_graph_key(key, alt);
                RebaseDialogOutcome::Continue
            }
        }
    }

    fn handle_graph_key(&mut self, key: KeyEvent, alt: bool) {
        let last = self.commits.len().saturating_sub(1);
        let page = (self.graph_area.height as usize / 2).max(1);
        match key.code {
            KeyCode::Up if alt => self.move_selected(self.selected.saturating_sub(1)),
            KeyCode::Down if alt => self.move_selected(self.selected + 1),
            KeyCode::Up => self.select(self.selected.saturating_sub(1)),
            KeyCode::Down => self.select(self.selected + 1),
            KeyCode::PageUp => self.select(self.selected.saturating_sub(page)),
            KeyCode::PageDown => self.select(self.selected + page),
            KeyCode::Home => self.select(0),
            KeyCode::End => self.select(last),
            KeyCode::Enter | KeyCode::Char(' ') => self.open_dropdown(self.selected),
            KeyCode::Char(c) if key.modifiers.difference(KeyModifiers::SHIFT).is_empty() => {
                if let Some(action) = action_for_key(c) {
                    self.set_action(self.selected, action);
                }
            }
            _ => {}
        }
    }

    fn open_dropdown(&mut self, row: usize) {
        let action = self.commits[row].step.action;
        let highlighted = ACTIONS
            .iter()
            .position(|&a| {
                a == action || (action == RebaseAction::Reword && a == RebaseAction::Edit)
            })
            .unwrap_or(0);
        self.select(row);
        self.dropdown = Some(Dropdown {
            row,
            highlighted,
            area: Rect::default(),
        });
    }

    fn handle_dropdown_key(&mut self, key: KeyEvent) -> RebaseDialogOutcome {
        let Some(dropdown) = &mut self.dropdown else {
            return RebaseDialogOutcome::Continue;
        };
        let row = dropdown.row;
        match key.code {
            KeyCode::Esc => self.dropdown = None,
            KeyCode::Up | KeyCode::Down => {
                let down = key.code == KeyCode::Down;
                let mut at = dropdown.highlighted;
                // To the next action that can be chosen, stopping at the
                // ends.
                loop {
                    let next = if down { at + 1 } else { at.wrapping_sub(1) };
                    let Some(&action) = ACTIONS.get(next) else {
                        break;
                    };
                    at = next;
                    if self.allows(row, action) {
                        if let Some(dropdown) = &mut self.dropdown {
                            dropdown.highlighted = at;
                        }
                        break;
                    }
                }
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                let action = ACTIONS[dropdown.highlighted];
                self.set_action(row, action);
                self.dropdown = None;
            }
            KeyCode::Char(c) => {
                if let Some(action) = action_for_key(c)
                    && self.allows(row, action)
                {
                    self.set_action(row, action);
                    self.dropdown = None;
                }
            }
            _ => {}
        }
        RebaseDialogOutcome::Continue
    }

    // ----- The mouse ------------------------------------------------------

    /// Whether the mouse position is over the dialog.
    pub fn contains(&self, x: u16, y: u16) -> bool {
        self.area.contains(ScreenPosition::new(x, y))
    }

    /// The commit whose row a screen row of the graph is, if any.
    fn row_at(&self, y: u16) -> Option<usize> {
        if y < self.graph_area.y || y >= self.graph_area.bottom() {
            return None;
        }
        let line = self.scroll + (y - self.graph_area.y) as usize;
        let row = line / 2;
        (row < self.commits.len()).then_some(row)
    }

    /// Handle a mouse event anywhere on the screen.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> RebaseDialogOutcome {
        let at = ScreenPosition::new(mouse.column, mouse.row);
        if self.dragging {
            match mouse.kind {
                MouseEventKind::Drag(_) => self.drag_to(mouse.row),
                MouseEventKind::Up(_) => self.dragging = false,
                _ => {}
            }
            return RebaseDialogOutcome::Continue;
        }
        if let Some(dropdown) = &self.dropdown {
            if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                let area = dropdown.area;
                let row = dropdown.row;
                // The frame's rows are the actions, inside its border.
                if area.contains(at) && mouse.row > area.y && mouse.row < area.bottom() - 1 {
                    let action = ACTIONS[(mouse.row - area.y - 1) as usize];
                    if self.allows(row, action) {
                        self.set_action(row, action);
                        self.dropdown = None;
                    }
                    return RebaseDialogOutcome::Continue;
                }
                // A press elsewhere closes it, and is spent on that.
                self.dropdown = None;
            }
            return RebaseDialogOutcome::Continue;
        }
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if self.start_area.contains(at) {
                    return self.start();
                }
                if self.cancel_area.contains(at) {
                    return RebaseDialogOutcome::Cancel;
                }
                if let Some(row) = self.row_at(mouse.row) {
                    self.focus = Focus::Graph;
                    let control = self.control_x..self.control_x + CONTROL_WIDTH;
                    let node_line =
                        (self.scroll + (mouse.row - self.graph_area.y) as usize).is_multiple_of(2);
                    if node_line && control.contains(&mouse.column) {
                        self.open_dropdown(row);
                    } else {
                        self.select(row);
                        self.dragging = true;
                    }
                }
            }
            MouseEventKind::ScrollUp if self.contains(mouse.column, mouse.row) => {
                self.scroll = self.scroll.saturating_sub(2);
            }
            MouseEventKind::ScrollDown if self.contains(mouse.column, mouse.row) => {
                self.scroll = (self.scroll + 2).min(self.max_scroll());
            }
            _ => {}
        }
        RebaseDialogOutcome::Continue
    }

    /// A drag of the selected commit reached screen row `y`: move it to
    /// the commit there, scrolling when the pointer is past the graph's
    /// top or bottom.
    fn drag_to(&mut self, y: u16) {
        let area = self.graph_area;
        if area.height == 0 {
            return;
        }
        let y = if y < area.y {
            self.scroll = self.scroll.saturating_sub(1);
            area.y
        } else if y >= area.bottom() {
            self.scroll = (self.scroll + 1).min(self.max_scroll());
            area.bottom() - 1
        } else {
            y
        };
        let line = self.scroll + (y - area.y) as usize;
        self.move_selected(line / 2);
        // The move scrolls by the drag, not by revealing.
        self.reveal = false;
    }

    /// The lines of the graph: two a commit, and the commit gone onto.
    fn graph_lines(&self) -> usize {
        self.commits.len() * 2 + 1
    }

    fn max_scroll(&self) -> usize {
        self.graph_lines()
            .saturating_sub(self.graph_area.height as usize)
    }

    // ----- Drawing --------------------------------------------------------

    /// Draw the dialog over most of `page`.
    pub fn render(&mut self, page: Rect, buf: &mut Buffer, theme: &Theme) {
        self.area = Rect::default();
        self.graph_area = Rect::default();
        self.start_area = Rect::default();
        self.cancel_area = Rect::default();
        let width = page.width.saturating_sub(4).min(120);
        let height = page.height.saturating_sub(2);
        if width < MIN_WIDTH || height < MIN_HEIGHT {
            return;
        }
        let x = page.x + (page.width - width) / 2;
        let y = page.y + (page.height - height) / 2;
        self.area = Rect::new(x, y, width, height);
        let inner = render_frame(self.area, None, buf, theme);
        let background = palette_background(theme);
        let dim = background.fg(theme.command_palette_placeholder_text);
        let text_x = inner.x + 1;
        let text_width = inner.width.saturating_sub(2) as usize;

        // The heading: what is rebased onto what, and how to read it.
        let branch = self
            .plan
            .branch
            .as_deref()
            .map(|name| name.strip_prefix("refs/heads/").unwrap_or(name))
            .unwrap_or("HEAD")
            .to_owned();
        let title = format!(
            "Interactive rebase of {branch} onto {}",
            short_id(self.plan.onto)
        );
        buf.set_stringn(
            text_x,
            inner.y,
            &title,
            text_width,
            background.add_modifier(Modifier::BOLD),
        );
        let noun = if self.commits.len() == 1 {
            "commit"
        } else {
            "commits"
        };
        let mut about = format!(
            "{} {noun}, newest first. The line on the left is the commits the branch ends up with; ● squash and ○ fixup fold into the commit below them.",
            self.commits.len()
        );
        match self.plan.merges.len() {
            0 => {}
            1 => about.push_str(" 1 merge is left out, as git leaves merges."),
            n => about.push_str(&format!(" {n} merges are left out, as git leaves merges.")),
        }
        let about = crate::fields::wrap_words(&about, text_width);
        for (row, line) in about.iter().take(2).enumerate() {
            buf.set_stringn(text_x, inner.y + 1 + row as u16, line, text_width, dim);
        }

        // The graph, between the heading and the footer: a blank row,
        // the problem (if any), and the buttons.
        let top = inner.y + 4;
        let bottom = inner.bottom().saturating_sub(3);
        self.graph_area = Rect::new(inner.x, top, inner.width, bottom.saturating_sub(top));
        self.render_graph(buf, theme, text_x, text_width);

        // The footer.
        let problem = self.refused.clone().or_else(|| self.problem());
        let all_dropped = self
            .commits
            .iter()
            .all(|commit| commit.step.action == RebaseAction::Drop);
        let note = match problem {
            Some(problem) => Some((problem, background.fg(theme.diff_removed_text))),
            None if all_dropped => Some((
                format!(
                    "Every commit is dropped: {branch} will end at {}.",
                    short_id(self.plan.onto)
                ),
                dim,
            )),
            None => None,
        };
        if let Some((text, style)) = note {
            buf.set_stringn(text_x, inner.bottom() - 2, &text, text_width, style);
        }
        self.render_buttons(buf, theme, inner);

        // The dropdown over it all.
        self.render_dropdown(buf, theme);
    }

    fn render_graph(&mut self, buf: &mut Buffer, theme: &Theme, text_x: u16, text_width: usize) {
        let area = self.graph_area;
        if area.height == 0 {
            return;
        }
        let height = area.height as usize;
        if self.reveal {
            let first = self.selected * 2;
            if first < self.scroll {
                self.scroll = first;
            } else if first + 2 > self.scroll + height {
                self.scroll = (first + 2).saturating_sub(height);
            }
            self.reveal = false;
        }
        self.scroll = self.scroll.min(self.max_scroll());

        let background = palette_background(theme);
        let dim = background.fg(theme.command_palette_placeholder_text);
        // The selected commit as the log selects one: only the
        // background marks it, brighter while the graph has the
        // keyboard, and the text keeps its colors.
        let selection = background.bg(if self.focus == Focus::Graph {
            theme.list_selection_background
        } else {
            theme.unfocused_list_selection_background
        });
        let colors = lane_palette(theme);
        let lane_style = |lane: Option<Lane>, base: Style| match lane {
            Some(Lane::Main) => base.fg(colors[0]),
            Some(Lane::Fold) => base.fg(colors[1]),
            None => base,
        };
        self.control_x = text_x;
        let graph_x = text_x + CONTROL_WIDTH + 1;
        let summary_x = graph_x + GRAPH_WIDTH + 2;
        let summary_width = (text_x as usize + text_width).saturating_sub(summary_x as usize);
        let cells = graph(&self.actions());

        for screen_row in 0..height {
            let line = self.scroll + screen_row;
            let y = area.y + screen_row as u16;
            let row = line / 2;
            if row == self.commits.len() {
                // The commit gone onto, under them all, on the main line.
                if line.is_multiple_of(2) {
                    buf[(graph_x, y)].set_symbol(NODE).set_style(dim);
                    let text = format!("onto {}  {}", short_id(self.plan.onto), self.onto_summary);
                    buf.set_stringn(summary_x, y, &text, summary_width, dim);
                }
                continue;
            }
            if row > self.commits.len() {
                break;
            }
            let commit = &self.commits[row];
            let which = if line.is_multiple_of(2) {
                Line::Node
            } else {
                Line::Transition
            };
            let selected = row == self.selected;
            let row_style = if selected { selection } else { background };
            if selected {
                buf.set_style(Rect::new(text_x, y, text_width as u16, 1), row_style);
            }
            let graph_cells = cells[row][if which == Line::Node { 0 } else { 1 }];
            for (k, (glyph, lane)) in graph_cells.into_iter().enumerate() {
                buf[(graph_x + k as u16, y)]
                    .set_symbol(glyph)
                    .set_style(lane_style(lane, row_style));
            }
            let dropped = commit.step.action == RebaseAction::Drop;
            match which {
                Line::Node => {
                    let control = format!("[{:<6} ▾]", commit.step.action.word());
                    let control_style = row_style.add_modifier(Modifier::BOLD);
                    buf.set_stringn(text_x, y, &control, CONTROL_WIDTH as usize, control_style);
                    let summary_style = if dropped {
                        row_style
                            .fg(theme.command_palette_placeholder_text)
                            .add_modifier(Modifier::CROSSED_OUT)
                    } else {
                        row_style
                    };
                    buf.set_stringn(
                        summary_x,
                        y,
                        &commit.step.summary,
                        summary_width,
                        summary_style,
                    );
                }
                Line::Transition => {
                    let pieces = [
                        (commit.author.as_str(), theme.git_author_text),
                        ("  ", theme.command_palette_result_text),
                        (&short_id(commit.step.commit), theme.git_hash_text),
                        ("  ", theme.command_palette_result_text),
                        (commit.time.as_str(), theme.git_date_text),
                    ];
                    let mut x = summary_x as usize;
                    let end = summary_x as usize + summary_width;
                    for (text, color) in pieces {
                        if x >= end {
                            break;
                        }
                        let color = if dropped {
                            theme.command_palette_placeholder_text
                        } else {
                            color
                        };
                        let style = row_style.fg(color);
                        buf.set_stringn(x as u16, y, text, end - x, style);
                        x += display_width(text);
                    }
                }
            }
        }
    }

    fn render_buttons(&mut self, buf: &mut Buffer, theme: &Theme, inner: Rect) {
        let background = palette_background(theme);
        let focused = Style::default()
            .fg(theme.command_palette_selection_text)
            .bg(theme.command_palette_selection_background);
        let y = inner.bottom() - 1;
        let start = format!(" {START} ");
        let cancel = format!(" {CANCEL} ");
        let cancel_width = display_width(&cancel) as u16;
        let start_width = display_width(&start) as u16;
        let cancel_x = inner.right().saturating_sub(cancel_width + 1);
        let start_x = cancel_x.saturating_sub(start_width + 2);
        let style = |focus: Focus, idle: Style| if self.focus == focus { focused } else { idle };
        buf.set_string(
            start_x,
            y,
            &start,
            style(Focus::Start, background.add_modifier(Modifier::BOLD)),
        );
        buf.set_string(cancel_x, y, &cancel, style(Focus::Cancel, background));
        self.start_area = Rect::new(start_x, y, start_width, 1);
        self.cancel_area = Rect::new(cancel_x, y, cancel_width, 1);
    }

    fn render_dropdown(&mut self, buf: &mut Buffer, theme: &Theme) {
        let Some(row) = self.dropdown.as_ref().map(|dropdown| dropdown.row) else {
            return;
        };
        let allowed: Vec<bool> = ACTIONS.iter().map(|&a| self.allows(row, a)).collect();
        let Some(dropdown) = &mut self.dropdown else {
            return;
        };
        let lines: Vec<String> = ACTIONS
            .iter()
            .map(|&action| format!(" {:<7} {}", action.word(), what_it_does(action)))
            .collect();
        let width = lines
            .iter()
            .map(|line| display_width(line))
            .max()
            .unwrap_or(0) as u16
            + 3;
        let height = ACTIONS.len() as u16 + 2;
        // Under the commit's control, or over it when there is no room
        // below.
        let node_y = (row * 2)
            .checked_sub(self.scroll)
            .map(|line| self.graph_area.y + line as u16)
            .unwrap_or(self.graph_area.y);
        let screen = self.area;
        let y = if node_y + 1 + height <= screen.bottom() {
            node_y + 1
        } else {
            node_y.saturating_sub(height)
        };
        let x = self.control_x.min(screen.right().saturating_sub(width));
        let area = Rect::new(x, y, width.min(screen.width), height);
        dropdown.area = area;
        let inner = render_frame(area, None, buf, theme);
        let background = palette_background(theme);
        let highlight = Style::default()
            .fg(theme.command_palette_selection_text)
            .bg(theme.command_palette_selection_background);
        for (index, line) in lines.iter().enumerate() {
            let style = if !allowed[index] {
                background.fg(theme.command_palette_placeholder_text)
            } else if index == dropdown.highlighted {
                highlight
            } else {
                background
            };
            let y = inner.y + index as u16;
            if y >= inner.bottom() {
                break;
            }
            buf.set_style(Rect::new(inner.x, y, inner.width, 1), style);
            buf.set_stringn(inner.x, y, line, inner.width as usize, style);
        }
    }
}

/// The action a key picks, as git's list of steps abbreviates them.
fn action_for_key(c: char) -> Option<RebaseAction> {
    ACTIONS
        .into_iter()
        .find(|action| action.word().starts_with(c.to_ascii_lowercase()))
}

/// What an action does, in a few words for the dropdown.
fn what_it_does(action: RebaseAction) -> &'static str {
    match action {
        RebaseAction::Pick => "keep the commit",
        RebaseAction::Drop => "leave the commit out",
        RebaseAction::Edit | RebaseAction::Reword => "stop to change, split, or reword it",
        RebaseAction::Squash => "fold into the commit below, messages combined",
        RebaseAction::Fixup => "fold into the commit below, its message dropped",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ninjaedit_core::git::Oid;

    /// A plan of `n` commits onto `0000…01`, commit `k` (from 1, oldest
    /// first) with id `k + 1` and summary `Commit k`.
    pub(super) fn dialog(n: usize) -> RebaseDialog {
        let id = |k: usize| Oid::from_str(&format!("{k:040x}")).unwrap();
        let steps: Vec<RebaseStep> = (1..=n)
            .map(|k| RebaseStep {
                action: RebaseAction::Pick,
                commit: id(k + 1),
                summary: format!("Commit {k}"),
            })
            .collect();
        let commits = steps
            .iter()
            .map(|step| PlanCommit {
                step: step.clone(),
                author: "Ann Author".to_owned(),
                time: "2026-10-06 12:00".to_owned(),
            })
            .collect();
        let plan = RebasePlan {
            branch: Some("refs/heads/main".to_owned()),
            head: id(n + 1),
            onto: id(1),
            steps,
            merges: Vec::new(),
        };
        RebaseDialog::new(plan, commits, "Base".to_owned())
    }

    pub(super) fn screen(dialog: &mut RebaseDialog, width: u16, height: u16) -> Vec<String> {
        let page = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(page);
        dialog.render(page, &mut buf, &Theme::default());
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buf[(x, y)].symbol().to_owned())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn alt(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::ALT)
    }

    fn ctrl_s() -> KeyEvent {
        KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    /// The summaries of the commits, newest first, with their actions.
    fn listed(dialog: &RebaseDialog) -> Vec<(String, &'static str)> {
        dialog
            .commits
            .iter()
            .map(|commit| (commit.step.summary.clone(), commit.step.action.word()))
            .collect()
    }

    /// The graph's glyphs for the actions, a line a string.
    fn drawn(actions: &[RebaseAction]) -> Vec<String> {
        graph(actions)
            .into_iter()
            .flat_map(|lines| lines.map(|cells| cells.map(|(glyph, _)| glyph).concat()))
            .collect()
    }

    #[test]
    fn the_graph_is_a_line_with_folds_joining_it_from_the_right() {
        use RebaseAction::{Drop, Edit, Fixup, Pick, Squash};
        assert_eq!(drawn(&[Pick, Edit]), ["●  ", "│  ", "●  ", "│  "]);
        // A squash joins the commit kept below it, past a dropped one,
        // which has no node.
        assert_eq!(
            drawn(&[Pick, Squash, Drop, Pick]),
            ["●  ", "│  ", "│ ●", "│ │", "│ │", "├─╯", "●  ", "│  "]
        );
        assert_eq!(
            drawn(&[Squash, Pick, Drop, Pick]),
            ["  ●", "╭─╯", "●  ", "│  ", "│  ", "│  ", "●  ", "│  "]
        );
        // A run at the top begins the line where it joins it, a fixup's
        // node open, past a dropped commit.
        assert_eq!(
            drawn(&[Fixup, Drop, Squash, Pick]),
            ["  ○", "  │", "  │", "  │", "  ●", "╭─╯", "●  ", "│  "]
        );
        // One at the bottom joins the commit gone onto.
        assert_eq!(drawn(&[Pick, Fixup]), ["●  ", "│  ", "│ ○", "├─╯"]);
    }

    #[test]
    fn keys_choose_actions_reorder_and_start_with_the_plan_oldest_first() {
        let mut d = dialog(3);
        let names = |d: &RebaseDialog| -> Vec<String> {
            listed(d)
                .into_iter()
                .map(|(name, action)| format!("{name}:{action}"))
                .collect()
        };
        assert_eq!(
            names(&d),
            ["Commit 3:pick", "Commit 2:pick", "Commit 1:pick"]
        );
        // The bottom commit has nothing below to fold into.
        d.handle_key(key(KeyCode::End));
        d.handle_key(key(KeyCode::Char('s')));
        d.handle_key(key(KeyCode::Char('f')));
        assert_eq!(names(&d)[2], "Commit 1:pick");
        d.handle_key(key(KeyCode::Char('d')));
        assert_eq!(names(&d)[2], "Commit 1:drop");
        d.handle_key(key(KeyCode::Char('p')));
        // Up to the top, fixed up into the one below, then moved down.
        d.handle_key(key(KeyCode::Home));
        d.handle_key(key(KeyCode::Char('f')));
        d.handle_key(alt(KeyCode::Down));
        assert_eq!(
            names(&d),
            ["Commit 2:pick", "Commit 3:fixup", "Commit 1:pick"]
        );
        assert_eq!(d.selected, 1);
        d.handle_key(alt(KeyCode::Down));
        d.handle_key(alt(KeyCode::Down));
        assert_eq!(d.selected, 2, "no further than the bottom");
        d.handle_key(alt(KeyCode::Up));
        d.handle_key(key(KeyCode::Up));
        d.handle_key(key(KeyCode::Char('e')));
        assert_eq!(
            names(&d),
            ["Commit 2:edit", "Commit 3:fixup", "Commit 1:pick"]
        );
        let RebaseDialogOutcome::Start(plan) = d.handle_key(ctrl_s()) else {
            panic!("started");
        };
        let steps: Vec<(String, RebaseAction)> = plan
            .steps
            .iter()
            .map(|step| (step.summary.clone(), step.action))
            .collect();
        assert_eq!(
            steps,
            [
                ("Commit 1".to_owned(), RebaseAction::Pick),
                ("Commit 3".to_owned(), RebaseAction::Fixup),
                ("Commit 2".to_owned(), RebaseAction::Edit),
            ]
        );
        assert_eq!(d.handle_key(key(KeyCode::Esc)), RebaseDialogOutcome::Cancel);
    }

    #[test]
    fn a_plan_with_nothing_to_fold_into_says_so_and_wont_start() {
        let mut d = dialog(2);
        d.handle_key(key(KeyCode::Char('s')));
        d.handle_key(key(KeyCode::Down));
        d.handle_key(key(KeyCode::Char('d')));
        assert_eq!(listed(&d)[0].1, "squash");
        let rows = screen(&mut d, 90, 24);
        let problem = format!(
            "{} has no commit kept before it to fold into",
            short_id(d.commits[0].step.commit)
        );
        assert!(rows.iter().any(|row| row.contains(&problem)), "{rows:#?}");
        assert_eq!(d.handle_key(ctrl_s()), RebaseDialogOutcome::Continue);
        // Every commit dropped is allowed, and said.
        d.handle_key(key(KeyCode::Up));
        d.handle_key(key(KeyCode::Char('d')));
        let rows = screen(&mut d, 90, 24);
        assert!(
            rows.iter()
                .any(|row| row.contains("Every commit is dropped: main will end at")),
            "{rows:#?}"
        );
        assert!(matches!(
            d.handle_key(ctrl_s()),
            RebaseDialogOutcome::Start(_)
        ));
    }

    #[test]
    fn the_dropdown_offers_what_can_be_chosen() {
        let mut d = dialog(2);
        // The bottom commit: down from edit, past squash and fixup, it
        // stays.
        d.handle_key(key(KeyCode::Down));
        d.handle_key(key(KeyCode::Enter));
        assert!(d.dropdown.is_some());
        for _ in 0..4 {
            d.handle_key(key(KeyCode::Down));
        }
        d.handle_key(key(KeyCode::Enter));
        assert!(d.dropdown.is_none());
        assert_eq!(listed(&d)[1].1, "edit");
        // The top one can squash; Escape leaves it as it was.
        d.handle_key(key(KeyCode::Up));
        d.handle_key(key(KeyCode::Char(' ')));
        d.handle_key(key(KeyCode::Down));
        d.handle_key(key(KeyCode::Esc));
        assert!(d.dropdown.is_none());
        assert_eq!(listed(&d)[0].1, "pick");
        d.handle_key(key(KeyCode::Enter));
        d.handle_key(key(KeyCode::Char('s')));
        assert_eq!(listed(&d)[0].1, "squash");
        // Escape with no dropdown cancels.
        assert_eq!(d.handle_key(key(KeyCode::Esc)), RebaseDialogOutcome::Cancel);
    }

    #[test]
    fn clicks_pick_actions_and_a_drag_moves_a_commit() {
        let mut d = dialog(3);
        let rows = screen(&mut d, 90, 24);
        let row_of = |name: &str| rows.iter().position(|row| row.contains(name)).unwrap() as u16;
        let (top, bottom) = (row_of("Commit 3"), row_of("Commit 1"));
        // A click on the control opens its dropdown; one on an action
        // picks it.
        let control = d.control_x + 1;
        d.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), control, top));
        screen(&mut d, 90, 24);
        let area = d.dropdown.as_ref().unwrap().area;
        // Fixup, the last of them.
        d.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            area.x + 2,
            area.bottom() - 2,
        ));
        assert!(d.dropdown.is_none());
        assert_eq!(listed(&d)[0].1, "fixup");
        // Commit 1 dragged by its text to the top.
        let text = d.control_x + CONTROL_WIDTH + 8;
        d.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), text, bottom));
        d.handle_mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            text,
            bottom - 2,
        ));
        assert_eq!(listed(&d)[1].0, "Commit 1");
        d.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), text, top));
        d.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), text, top));
        assert_eq!(
            listed(&d)
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            ["Commit 1", "Commit 3", "Commit 2"]
        );
        assert_eq!(d.selected, 0);
        // A press outside the dialog does nothing; Cancel cancels.
        let outcome = d.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 0, 0));
        assert_eq!(outcome, RebaseDialogOutcome::Continue);
        screen(&mut d, 90, 24);
        let cancel = d.cancel_area;
        assert_eq!(
            d.handle_mouse(mouse(
                MouseEventKind::Down(MouseButton::Left),
                cancel.x,
                cancel.y
            )),
            RebaseDialogOutcome::Cancel
        );
        let start = d.start_area;
        assert!(matches!(
            d.handle_mouse(mouse(
                MouseEventKind::Down(MouseButton::Left),
                start.x,
                start.y
            )),
            RebaseDialogOutcome::Start(_)
        ));
    }

    #[test]
    fn the_selection_is_marked_by_the_logs_background_alone() {
        let theme = Theme::default();
        let mut d = dialog(2);
        d.handle_key(key(KeyCode::Char('d')));
        let page = Rect::new(0, 0, 90, 24);
        let mut buf = Buffer::empty(page);
        d.render(page, &mut buf, &theme);
        // The selected (dropped) commit's two lines: the first author
        // line is its second, its id after the author.
        let (x, y) = (0..page.height)
            .find_map(|y| {
                let row: String = (0..page.width).map(|x| buf[(x, y)].symbol()).collect();
                let at = row.find("Ann Author  ")?;
                Some((row[..at].chars().count() as u16 + 12, y))
            })
            .unwrap();
        let cell = &buf[(x, y)];
        assert_eq!(cell.bg, theme.list_selection_background);
        assert_eq!(cell.fg, theme.command_palette_placeholder_text);
        assert_eq!(
            buf[(d.control_x, y - 1)].bg,
            theme.list_selection_background
        );
        // Kept, the id is in its own color.
        d.handle_key(key(KeyCode::Char('p')));
        d.render(page, &mut buf, &theme);
        assert_eq!(buf[(x, y)].fg, theme.git_hash_text);
        assert_eq!(buf[(x, y)].bg, theme.list_selection_background);
        // With the keyboard on the buttons, the unfocused selection.
        d.handle_key(key(KeyCode::Tab));
        d.render(page, &mut buf, &theme);
        assert_eq!(buf[(x, y)].bg, theme.unfocused_list_selection_background);
    }

    #[test]
    fn a_long_plan_scrolls_to_the_selection() {
        let mut d = dialog(30);
        let rows = screen(&mut d, 90, 24);
        assert!(rows.iter().any(|row| row.contains("Commit 30")));
        assert!(!rows.iter().any(|row| row.contains("  Base")));
        d.handle_key(key(KeyCode::End));
        let rows = screen(&mut d, 90, 24);
        assert!(
            rows.iter()
                .any(|row| row.contains("Commit 1 ") || row.ends_with("Commit 1"))
        );
        assert!(!rows.iter().any(|row| row.contains("Commit 30")));
        // Tab to the buttons: Enter on Start starts.
        d.handle_key(key(KeyCode::Tab));
        assert!(matches!(
            d.handle_key(key(KeyCode::Enter)),
            RebaseDialogOutcome::Start(_)
        ));
    }
}
