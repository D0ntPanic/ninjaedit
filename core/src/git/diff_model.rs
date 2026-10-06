//! A file's diff as someone reads it: the rows shown, with a cursor
//! that moves through them and a selection, for the git pages to show a
//! [`FileDiff`] with as the editor shows a file.
//!
//! The diff is read-only, so the cursor only moves and selects; it does
//! that with a [`Caret`], as the editor's does, over the rows as shown:
//! every line of the diff, removed, added, or context, is a line of text
//! in the order shown. The row standing for hidden lines between changes
//! (a gap) offers ways to reveal them, its [`GapAction`]s: lines above
//! the change below, lines below the change above, or all of them. The
//! cursor stops on each of those in turn, as it would on the characters
//! of a line, so that a frontend drawing them as buttons can have the
//! keyboard press the one the cursor is on
//! ([`DiffModel::action_at_cursor`]). Expanding a gap adds rows around
//! the cursor, which stays on the line it was on, as does the selection;
//! a cursor on a gap stays on it, on the same action, so pressing it
//! again reveals more, unless the gap is revealed in full, which leaves
//! the cursor on the first line it revealed.
//!
//! What a selection copies is the text as shown: its lines in order,
//! removed and added alike, without the diff's markers, and without the
//! hidden lines it stretches over. Commands that work on lines rather
//! than text take the selection as [`DiffLine`]s instead
//! ([`DiffModel::selected_lines`]).

use super::diff::{DiffLine, DiffRow, FileDiff};
use crate::caret::{Caret, LineSelection, LineSource, Movement, TextPos};
use std::borrow::Cow;
use std::ops::Range;

/// The lines revealing a gap's lines above or below a change reveals at
/// once.
pub const EXPAND_LINES: usize = 10;

/// A way to reveal a gap's hidden lines, as the gap offers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GapAction {
    /// Reveal [`EXPAND_LINES`] more above the change below the gap.
    Up,
    /// Reveal [`EXPAND_LINES`] more below the change above the gap.
    Down,
    /// Reveal the whole gap.
    All,
}

/// What a row offers to reveal its hidden lines, in the order they are
/// shown: lines above the change below it if there is one, lines below
/// the change above it if there is one, and all of them when there are
/// more than either reveals at once (or there is no change on either
/// side to reveal them from). Nothing for a row that isn't a gap.
fn actions_of(row: &DiffRow) -> Vec<GapAction> {
    let DiffRow::Gap {
        hidden, up, down, ..
    } = *row
    else {
        return Vec::new();
    };
    let mut actions = Vec::new();
    if up {
        actions.push(GapAction::Up);
    }
    if down {
        actions.push(GapAction::Down);
    }
    if hidden > EXPAND_LINES || actions.is_empty() {
        actions.push(GapAction::All);
    }
    actions
}

/// What a gap is to the caret: a line with a stop for each action, the
/// first at its start and the last at its end, one character apart.
const GAP_STOPS: &[u8] = b"..";

/// A diff with a cursor and selection in it; see the [module
/// documentation](self). Positions are [`TextPos`]es whose line is a
/// row of [`rows`](Self::rows).
pub struct DiffModel {
    diff: FileDiff,
    /// The rows as shown, built again whenever a gap is expanded.
    rows: Vec<DiffRow>,
    caret: Caret,
}

/// The rows of a diff as lines of text for the caret: a line's text, and
/// for a gap, a stop for each of its actions (see [`GAP_STOPS`]).
struct RowLines<'a> {
    diff: &'a FileDiff,
    rows: &'a [DiffRow],
}

impl LineSource for RowLines<'_> {
    fn line_count(&self) -> usize {
        self.rows.len()
    }

    fn line(&self, index: usize) -> Cow<'_, [u8]> {
        match self.rows.get(index) {
            Some(DiffRow::Line(line)) => Cow::Borrowed(self.diff.text(line).as_bytes()),
            Some(row) => {
                let stops = actions_of(row).len().saturating_sub(1);
                Cow::Borrowed(&GAP_STOPS[..stops])
            }
            None => Cow::Borrowed(&[]),
        }
    }
}

/// What a row stands for, to find it again once rows are added around
/// it: a line is the same line whichever row shows it, and a gap keeps
/// its number until it is revealed in full.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RowKey {
    Line(DiffLine),
    Gap(usize),
}

impl RowKey {
    fn of(row: &DiffRow) -> RowKey {
        match row {
            DiffRow::Line(line) => RowKey::Line(*line),
            DiffRow::Gap { gap, .. } => RowKey::Gap(*gap),
        }
    }
}

impl DiffModel {
    /// The diff with the cursor at its first row, and tab stops every
    /// `tab_width` columns.
    pub fn new(diff: FileDiff, tab_width: usize) -> DiffModel {
        let rows = diff.rows();
        DiffModel {
            diff,
            rows,
            caret: Caret::new(tab_width),
        }
    }

    pub fn diff(&self) -> &FileDiff {
        &self.diff
    }

    /// The rows shown: see [`FileDiff::rows`].
    pub fn rows(&self) -> &[DiffRow] {
        &self.rows
    }

    fn lines(&self) -> RowLines<'_> {
        RowLines {
            diff: &self.diff,
            rows: &self.rows,
        }
    }

    /// The rows as lines of text, and the caret to move through them.
    fn parts(&mut self) -> (RowLines<'_>, &mut Caret) {
        let lines = RowLines {
            diff: &self.diff,
            rows: &self.rows,
        };
        (lines, &mut self.caret)
    }

    // ----- Cursor and selection -------------------------------------------

    pub fn cursor(&self) -> TextPos {
        self.caret.cursor()
    }

    /// The display column of the cursor on its row.
    pub fn cursor_column(&self) -> usize {
        self.caret.column_of(&self.lines(), self.caret.cursor())
    }

    /// What is selected, in reading order, or `None` when the selection
    /// is empty.
    pub fn selection(&self) -> Option<Range<TextPos>> {
        self.caret.selection()
    }

    /// What of a row the selection covers; see [`Caret::selection_on`].
    pub fn selection_on(&self, row: usize) -> Option<LineSelection> {
        self.caret.selection_on(&self.lines(), row)
    }

    /// The display columns of a row the selection covers, for drawing
    /// it: those of the characters selected, and one more past the end
    /// of the row when the selection runs on past it, standing for the
    /// line break.
    pub fn selected_columns(&self, row: usize) -> Option<Range<usize>> {
        let selected = self.selection_on(row)?;
        let lines = self.lines();
        let start = self
            .caret
            .column_of(&lines, TextPos::new(row, selected.bytes.start));
        let end = self
            .caret
            .column_of(&lines, TextPos::new(row, selected.bytes.end));
        Some(start..end + usize::from(selected.past_end))
    }

    pub fn move_cursor(&mut self, movement: Movement) {
        let (lines, caret) = self.parts();
        caret.move_cursor(&lines, movement);
    }

    pub fn extend_selection(&mut self, movement: Movement) {
        let (lines, caret) = self.parts();
        caret.extend_selection(&lines, movement);
    }

    /// The position at a display column of a row, as a click there means
    /// it: the character covering the column, or the end of the row past
    /// its last one.
    pub fn position_at(&self, row: usize, column: usize) -> TextPos {
        self.caret.position_at_column(&self.lines(), row, column)
    }

    /// Put the cursor at a display column of a row, clearing the
    /// selection, as a click does.
    pub fn set_cursor_at(&mut self, row: usize, column: usize) {
        let pos = self.position_at(row, column);
        let (lines, caret) = self.parts();
        caret.set_cursor(&lines, pos);
    }

    /// Select from where the selection began (or the cursor, without
    /// one) to a display column of a row, as a drag or a Shift+click
    /// does.
    pub fn extend_to(&mut self, row: usize, column: usize) {
        let pos = self.position_at(row, column);
        let anchor = self.caret.anchor().unwrap_or(self.caret.cursor());
        let (lines, caret) = self.parts();
        caret.set_selection(&lines, anchor, pos);
    }

    /// Select the word at a display column of a row, as a double-click
    /// does; see [`Caret::select_word_at`].
    pub fn select_word_at(&mut self, row: usize, column: usize) {
        let pos = self.position_at(row, column);
        let (lines, caret) = self.parts();
        caret.select_word_at(&lines, pos);
    }

    /// Select whole rows, from `anchor` to `row` either way round, as a
    /// click or a drag in the line numbers does: from the start of the
    /// first to the start of the row after the last (or the end of the
    /// last row there is), with the cursor at the end `row` is at.
    pub fn select_rows(&mut self, anchor: usize, row: usize) {
        let end = |row: usize| {
            if row + 1 < self.rows.len() {
                TextPos::new(row + 1, 0)
            } else {
                TextPos::new(row, usize::MAX)
            }
        };
        let (from, to) = if anchor <= row {
            (TextPos::new(anchor, 0), end(row))
        } else {
            (end(anchor), TextPos::new(row, 0))
        };
        let (lines, caret) = self.parts();
        caret.set_selection(&lines, from, to);
    }

    pub fn select_all(&mut self) {
        let (lines, caret) = self.parts();
        caret.select_all(&lines);
    }

    pub fn clear_selection(&mut self) {
        self.caret.clear_selection();
    }

    /// The selected text as shown, or `None` when nothing is selected:
    /// the selected part of each line, removed and added alike, joined
    /// by `\n`, leaving out the gaps of hidden lines.
    pub fn selected_text(&self) -> Option<String> {
        let range = self.selection()?;
        let mut text = String::new();
        for row in range.start.line..=range.end.line {
            let Some(DiffRow::Line(line)) = self.rows.get(row) else {
                continue;
            };
            if let Some(selected) = self.selection_on(row) {
                text.push_str(&self.diff.text(line)[selected.bytes]);
                if selected.past_end {
                    text.push('\n');
                }
            }
        }
        Some(text)
    }

    /// The lines of the diff the selection takes in, in the order shown,
    /// for commands that work on whole lines; with nothing selected, the
    /// cursor's line. A line counts when any of its text is selected (or,
    /// for an empty one, when the selection takes in its start); one the
    /// selection only touches at its end, or the gaps, don't.
    pub fn selected_lines(&self) -> Vec<DiffLine> {
        let line_at = |row: usize| match self.rows.get(row) {
            Some(DiffRow::Line(line)) => Some(*line),
            _ => None,
        };
        let Some(range) = self.selection() else {
            return line_at(self.cursor().line).into_iter().collect();
        };
        (range.start.line..=range.end.line)
            .filter(|&row| {
                self.selection_on(row).is_some_and(|selected| {
                    !selected.bytes.is_empty()
                        || (selected.bytes.start == 0 && self.lines().line(row).is_empty())
                })
            })
            .filter_map(line_at)
            .collect()
    }

    // ----- Expanding the context ------------------------------------------

    /// The line of the file's new side (from zero) the cursor is at, for
    /// opening the file there: the cursor's line; for a removed line,
    /// the next line the new side has (what replaced it), or the one
    /// before at the end of the file; and for a gap, the first of its
    /// hidden lines. `None` for a diff with no lines on its new side.
    pub fn new_line_at_cursor(&self) -> Option<usize> {
        let row = self.cursor().line;
        let new_at = |row: &DiffRow| match row {
            DiffRow::Line(line) => line.new,
            DiffRow::Gap { .. } => None,
        };
        if let Some(DiffRow::Gap { .. }) = self.rows.get(row) {
            let before = self.rows[..row].iter().rev().find_map(new_at);
            return Some(before.map_or(0, |line| line + 1));
        }
        self.rows
            .get(row..)?
            .iter()
            .find_map(new_at)
            .or_else(|| self.rows[..row].iter().rev().find_map(new_at))
    }

    /// What a row offers to reveal its hidden lines, in the order shown;
    /// nothing for a row that isn't a gap. See [`GapAction`].
    pub fn gap_actions(&self, row: usize) -> Vec<GapAction> {
        self.rows.get(row).map(actions_of).unwrap_or_default()
    }

    /// The gap the cursor is on, if it is on one, and the action it is
    /// on there: what Enter presses.
    pub fn action_at_cursor(&self) -> Option<(usize, GapAction)> {
        let cursor = self.cursor();
        let Some(DiffRow::Gap { gap, .. }) = self.rows.get(cursor.line) else {
            return None;
        };
        let actions = self.gap_actions(cursor.line);
        let action = actions.get(cursor.byte).or(actions.last())?;
        Some((*gap, *action))
    }

    /// Put the cursor on one of a gap's actions, as a click on it does.
    /// Nothing happens for a row that isn't a gap, or an action it
    /// doesn't offer.
    pub fn put_cursor_on_action(&mut self, row: usize, action: GapAction) {
        let Some(stop) = self.gap_actions(row).iter().position(|a| *a == action) else {
            return;
        };
        let (lines, caret) = self.parts();
        caret.set_cursor(&lines, TextPos::new(row, stop));
    }

    /// Reveal a gap's hidden lines as an action says.
    pub fn take_action(&mut self, gap: usize, action: GapAction) {
        match action {
            GapAction::Up => self.expand_up(gap, EXPAND_LINES),
            GapAction::Down => self.expand_down(gap, EXPAND_LINES),
            GapAction::All => self.expand_all(gap),
        }
    }

    /// Reveal up to `count` more hidden lines of a gap above the change
    /// below it; see [`FileDiff::expand_up`].
    pub fn expand_up(&mut self, gap: usize, count: usize) {
        self.reveal(|diff| diff.expand_up(gap, count));
    }

    /// Reveal up to `count` more hidden lines of a gap below the change
    /// above it; see [`FileDiff::expand_down`].
    pub fn expand_down(&mut self, gap: usize, count: usize) {
        self.reveal(|diff| diff.expand_down(gap, count));
    }

    /// Reveal all of a gap's lines.
    pub fn expand_all(&mut self, gap: usize) {
        self.reveal(|diff| diff.expand_all(gap));
    }

    /// What the row at a position stands for.
    fn key_at(&self, pos: TextPos) -> Option<RowKey> {
        self.rows.get(pos.line).map(RowKey::of)
    }

    /// Reveal lines of the diff, keeping the cursor and the selection on
    /// the rows they were on.
    fn reveal(&mut self, reveal: impl FnOnce(&mut FileDiff)) {
        let cursor = self.caret.cursor();
        let anchor = self.caret.anchor();
        let cursor_key = self.key_at(cursor);
        let anchor_key = anchor.and_then(|anchor| self.key_at(anchor));
        reveal(&mut self.diff);
        self.rows = self.diff.rows();
        // Only the gap's rows change, so a gap revealed in full leaves
        // the rows above it where they were, and its first line in the
        // row it was in.
        let find = |pos: TextPos, key: Option<RowKey>| match self
            .rows
            .iter()
            .position(|row| Some(RowKey::of(row)) == key)
        {
            Some(row) => TextPos::new(row, pos.byte),
            None => TextPos::new(pos.line, 0),
        };
        let cursor = find(cursor, cursor_key);
        let anchor = anchor.map(|anchor| find(anchor, anchor_key));
        let (lines, caret) = self.parts();
        match anchor {
            Some(anchor) => caret.set_selection(&lines, anchor, cursor),
            None => caret.set_cursor(&lines, cursor),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::diff::{CONTEXT_LINES, ChangeKind, Contents, LineKind};
    use super::*;
    use git2::{DiffOptions, Patch};

    /// The diff of `old` to `new`, as the changes page builds one.
    fn diff_of(old: &str, new: &str) -> FileDiff {
        let old = Contents::from_bytes(old.as_bytes().to_vec());
        let new = Contents::from_bytes(new.as_bytes().to_vec());
        let mut options = DiffOptions::new();
        options.context_lines(CONTEXT_LINES);
        let patch =
            Patch::from_buffers(&old.bytes, None, &new.bytes, None, Some(&mut options)).unwrap();
        FileDiff::build(
            ChangeKind::Modified,
            "t.txt",
            None,
            &old,
            &new,
            Some(&patch),
        )
        .unwrap()
    }

    /// Thirty lines `l0` to `l29`, with `l5` and `l25` changed to `L5`
    /// and `L25`. With three lines of context the rows are: a gap (l0
    /// and l1), l2 to l8 with l5 replaced, a gap (l9 to l21), l22 to l28
    /// with l25 replaced, and a gap (l29).
    fn model() -> DiffModel {
        let old: String = (0..30).map(|i| format!("l{i}\n")).collect();
        let new = old.replace("l5\n", "L5\n").replace("l25\n", "L25\n");
        DiffModel::new(diff_of(&old, &new), 4)
    }

    /// What each row shows: a line's text, or `…` for a gap.
    fn shown(model: &DiffModel) -> Vec<String> {
        model
            .rows()
            .iter()
            .map(|row| match row {
                DiffRow::Line(line) => model.diff().text(line).to_owned(),
                DiffRow::Gap { .. } => "…".to_owned(),
            })
            .collect()
    }

    #[test]
    fn the_cursor_moves_through_the_rows_as_shown() {
        let mut model = model();
        assert_eq!(
            shown(&model),
            [
                "…", "l2", "l3", "l4", "l5", "L5", "l6", "l7", "l8", "…", "l22", "l23", "l24",
                "l25", "L25", "l26", "l27", "l28", "…"
            ]
        );
        assert_eq!(model.cursor(), TextPos::new(0, 0));
        assert_eq!(model.action_at_cursor(), Some((0, GapAction::Up)));
        // Down a column through the gap, which stops on its second
        // action, and out the other side, back at the column.
        model.set_cursor_at(8, 1);
        model.move_cursor(Movement::Down);
        assert_eq!(model.cursor(), TextPos::new(9, 1));
        assert_eq!(model.action_at_cursor(), Some((1, GapAction::Down)));
        model.move_cursor(Movement::Down);
        assert_eq!(model.cursor(), TextPos::new(10, 1));
        assert_eq!(model.cursor_column(), 1);
        // Right off the end of a row goes through the gap's actions one
        // at a time, and on to the next row.
        model.set_cursor_at(8, 99);
        assert_eq!(model.cursor(), TextPos::new(8, 2));
        let mut actions = Vec::new();
        for _ in 0..3 {
            model.move_cursor(Movement::Right);
            actions.extend(model.action_at_cursor());
        }
        assert_eq!(
            actions,
            [
                (1, GapAction::Up),
                (1, GapAction::Down),
                (1, GapAction::All)
            ]
        );
        model.move_cursor(Movement::Right);
        assert_eq!(model.cursor(), TextPos::new(10, 0));
        assert_eq!(model.action_at_cursor(), None);
    }

    #[test]
    fn a_gaps_actions_reveal_its_lines_with_the_cursor_kept_on_them() {
        let mut model = model();
        // Two lines above the first change: only up from it, ten at a
        // time, which takes them all. Thirteen between the changes:
        // either way, or all. One after the last: only down.
        assert_eq!(model.gap_actions(0), [GapAction::Up]);
        assert_eq!(
            model.gap_actions(9),
            [GapAction::Up, GapAction::Down, GapAction::All]
        );
        assert_eq!(model.gap_actions(18), [GapAction::Down]);
        assert_eq!(model.gap_actions(1), []);
        // Down from the change above, the cursor on the action: the
        // lines appear above the gap, which moves down with the cursor,
        // and three are left, so all of them is no longer offered.
        model.put_cursor_on_action(9, GapAction::Down);
        let (gap, action) = model.action_at_cursor().unwrap();
        model.take_action(gap, action);
        assert_eq!(shown(&model)[9..20].first().map(String::as_str), Some("l9"));
        assert_eq!(model.cursor().line, 19);
        assert_eq!(model.gap_actions(19), [GapAction::Up, GapAction::Down]);
        assert_eq!(model.action_at_cursor(), Some((1, GapAction::Down)));
        // Again, and the gap is gone: the cursor is on the first line it
        // revealed.
        model.take_action(1, GapAction::Down);
        assert_eq!(shown(&model)[19], "l19");
        assert_eq!(model.cursor(), TextPos::new(19, 0));
        assert_eq!(model.action_at_cursor(), None);
        // A file whose lines didn't change hides them all, with nothing
        // to reveal them from but all of them.
        let unchanged = DiffModel::new(diff_of("a\nb\n", "a\nb\n"), 4);
        assert_eq!(unchanged.gap_actions(0), [GapAction::All]);
    }

    #[test]
    fn a_selection_copies_the_text_shown_without_the_gaps() {
        let mut model = model();
        // From the middle of l8, over the gap, into l22.
        model.set_cursor_at(8, 1);
        model.extend_to(10, 2);
        assert_eq!(model.selected_text().as_deref(), Some("8\nl2"));
        // Removed and added lines alike, as they are shown.
        model.select_rows(3, 5);
        assert_eq!(model.selected_text().as_deref(), Some("l4\nl5\nL5\n"));
        assert_eq!(model.cursor(), TextPos::new(6, 0));
        // Backwards, the cursor is at the top.
        model.select_rows(14, 13);
        assert_eq!(model.selected_text().as_deref(), Some("l25\nL25\n"));
        assert_eq!(model.cursor(), TextPos::new(13, 0));
        // Down to the gap at the very end, which adds nothing.
        model.select_rows(17, 18);
        assert_eq!(model.selected_text().as_deref(), Some("l28\n"));
        // A word, and the columns a row's selection is drawn over: the
        // line break counts as one past the end.
        model.select_word_at(14, 1);
        assert_eq!(model.selected_text().as_deref(), Some("L25"));
        assert_eq!(model.selected_columns(14), Some(0..3));
        model.set_cursor_at(16, 1);
        model.extend_selection(Movement::Down);
        assert_eq!(model.selected_columns(16), Some(1..4));
        assert_eq!(model.selected_columns(17), Some(0..1));
        model.clear_selection();
        assert_eq!(model.selected_text(), None);
        assert_eq!(model.selected_columns(16), None);
    }

    #[test]
    fn selected_lines_are_those_with_text_selected() {
        let mut model = model();
        let kinds = |model: &DiffModel| -> Vec<(LineKind, Option<usize>, Option<usize>)> {
            model
                .selected_lines()
                .iter()
                .map(|line| (line.kind, line.old, line.new))
                .collect()
        };
        // Nothing selected: the cursor's line, and none on a gap.
        model.set_cursor_at(4, 0);
        assert_eq!(kinds(&model), [(LineKind::Removed, Some(5), None)]);
        model.set_cursor_at(0, 0);
        assert_eq!(kinds(&model), []);
        // From the end of l4 to inside L5: l4 only by its line break,
        // so it doesn't count.
        model.set_cursor_at(3, 2);
        model.extend_to(5, 1);
        assert_eq!(
            kinds(&model),
            [
                (LineKind::Removed, Some(5), None),
                (LineKind::Added, None, Some(5))
            ]
        );
        // Over a gap, which counts for nothing.
        model.set_cursor_at(8, 0);
        model.extend_to(10, 1);
        assert_eq!(
            kinds(&model),
            [
                (LineKind::Context, Some(8), Some(8)),
                (LineKind::Context, Some(22), Some(22))
            ]
        );
    }

    #[test]
    fn expanding_keeps_the_cursor_and_selection_on_their_lines() {
        let mut model = model();
        // A selection from l22 to inside l24, the cursor at its end.
        model.set_cursor_at(10, 0);
        model.extend_to(12, 1);
        // Ten lines revealed below l8, above the selection.
        model.expand_down(1, 10);
        assert_eq!(model.rows().len(), 29);
        assert_eq!(model.cursor(), TextPos::new(22, 1));
        assert_eq!(shown(&model)[22], "l24");
        assert_eq!(model.selected_text().as_deref(), Some("l22\nl23\nl"));
        // On the gap that is left, Enter's gap is the rest of it; all of
        // it revealed leaves the cursor on its first line.
        model.set_cursor_at(19, 0);
        assert_eq!(shown(&model)[19], "…");
        let (gap, _) = model.action_at_cursor().unwrap();
        model.expand_all(gap);
        assert_eq!(model.cursor(), TextPos::new(19, 0));
        assert_eq!(shown(&model)[19], "l19");
        // Lines revealed above a change move the rows below them: the
        // gap's one row becomes its two lines.
        model.set_cursor_at(1, 1);
        assert_eq!(shown(&model)[1], "l2");
        model.expand_up(0, 10);
        assert_eq!(shown(&model)[..2], ["l0", "l1"]);
        assert_eq!(model.cursor(), TextPos::new(2, 1));
        assert_eq!(shown(&model)[2], "l2");
    }

    #[test]
    fn the_new_line_at_the_cursor_is_where_to_open_the_file() {
        let mut model = model();
        let at = |model: &mut DiffModel, row| {
            model.set_cursor_at(row, 0);
            model.new_line_at_cursor()
        };
        assert_eq!(at(&mut model, 0), Some(0), "the first gap: l0");
        assert_eq!(at(&mut model, 1), Some(2), "l2");
        assert_eq!(at(&mut model, 4), Some(5), "removed l5: what replaced it");
        assert_eq!(at(&mut model, 5), Some(5), "added L5");
        assert_eq!(at(&mut model, 9), Some(9), "the gap after l8: l9");
        assert_eq!(at(&mut model, 18), Some(29), "the last gap: l29");
        // A file deleted to its end: the removed lines have nothing after.
        let mut deleted = DiffModel::new(diff_of("a\nb\n", "a\n"), 4);
        assert_eq!(at(&mut deleted, 1), Some(0), "b, removed: the line before");
        let mut gone = DiffModel::new(diff_of("a\n", ""), 4);
        assert_eq!(at(&mut gone, 0), None);
    }

    #[test]
    fn a_diff_without_lines_has_nowhere_to_go() {
        let mut model = DiffModel::new(diff_of("same\n", "same\n"), 4);
        let rows = model.rows().len();
        for movement in [Movement::Down, Movement::Right, Movement::DocumentEnd] {
            model.move_cursor(movement);
        }
        model.select_all();
        assert_eq!(rows, 1, "the one gap of an unchanged file");
        assert_eq!(model.selected_text(), None);
        assert_eq!(model.selected_lines(), []);
    }
}
