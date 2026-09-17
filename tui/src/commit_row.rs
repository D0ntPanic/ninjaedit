//! A commit's row as the git log draws it, for the log itself and for
//! the diff of a submodule, which is a graph of the submodule's commits
//! drawn the same way.
//!
//! A commit takes two lines: the node line, with the commit's node in
//! its lane of the graph and the references and message beside it, and
//! the transition line below, with the graph's lines running on to the
//! next commit and the author, id, and time beside them. The graph gets
//! at most a share of the width and stays put while the text scrolls
//! sideways. The commit HEAD is on (or, for a submodule, the commit the
//! submodule now points at) gets a different node and its message in
//! the theme's HEAD color, bold. In the log HEAD's commit also says so
//! in words before its references, as `git log --decorate` does:
//! `HEAD → main` when HEAD is on a branch, `HEAD` alone when it is
//! detached, so that where you are is said and not only colored.

use crate::diff_pane::{Piece, draw_pieces, pieces_width};
use crate::theme::Theme;
use ninjaedit_core::git::{Commit, NODE, RefKind, cells_for};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

/// The node of the commit HEAD is on.
pub(crate) const HEAD_NODE: &str = "◉";
/// The graph takes at most this share of a pane's width.
const MAX_GRAPH_SHARE: usize = 3;

/// Which of a commit's two lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CommitLine {
    Node,
    Transition,
}

/// How a commit stands out among the others of a graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Highlight {
    /// One commit among many.
    None,
    /// The commit HEAD is on, in the log: its node, its message in the
    /// HEAD color, and `HEAD` said before its references.
    Head,
    /// The commit a submodule now points at, in its diff: drawn as HEAD
    /// is, but not called HEAD, since it isn't.
    Target,
}

impl Highlight {
    /// [`Head`](Self::Head) when `is_head`, else [`None`](Self::None).
    pub(crate) fn head_if(is_head: bool) -> Highlight {
        if is_head {
            Highlight::Head
        } else {
            Highlight::None
        }
    }

    /// [`Target`](Self::Target) when `is_target`, else
    /// [`None`](Self::None).
    pub(crate) fn target_if(is_target: bool) -> Highlight {
        if is_target {
            Highlight::Target
        } else {
            Highlight::None
        }
    }
}

/// How many lanes of the graph a pane `width` columns wide draws, at
/// most.
pub(crate) fn lane_cap(width: u16) -> usize {
    (width as usize / MAX_GRAPH_SHARE).div_ceil(2).max(1)
}

/// The colors the graph's lanes cycle through.
pub(crate) fn lane_palette(theme: &Theme) -> [Color; 8] {
    [
        theme.terminal_blue,
        theme.terminal_green,
        theme.terminal_yellow,
        theme.terminal_magenta,
        theme.terminal_cyan,
        theme.terminal_red,
        theme.terminal_bright_blue,
        theme.terminal_bright_magenta,
    ]
}

/// A commit's two lines of text: the references and the message, then
/// the author, the id, and the time. Styled over `row_style` with the
/// theme's colors when given (a highlighted row bold and in HEAD's
/// color), otherwise plain, for measuring. HEAD's commit in the log
/// says `HEAD → ` before the branch HEAD is on, or `HEAD ` alone when
/// HEAD is detached and there is no branch to point at.
pub(crate) fn commit_lines(
    commit: &Commit,
    highlight: Highlight,
    styles: Option<(&Theme, Style)>,
) -> (Vec<Piece>, Vec<Piece>) {
    let is_head = highlight != Highlight::None;
    let row_style = styles.map_or_else(Style::default, |(_, style)| style);
    let colored = |pick: fn(&Theme) -> Color| match styles {
        Some((theme, style)) => style.fg(pick(theme)),
        None => row_style,
    };
    let mut first: Vec<Piece> = Vec::new();
    if highlight == Highlight::Head && !commit.refs.iter().any(|label| label.is_head) {
        first.push(("HEAD ".to_owned(), colored(|t| t.git_head_text)));
    }
    for label in &commit.refs {
        if label.is_head {
            first.push(("HEAD → ".to_owned(), colored(|t| t.git_head_text)));
        }
        let color: fn(&Theme) -> Color = match label.kind {
            RefKind::Branch => |t| t.git_branch_text,
            RefKind::RemoteBranch => |t| t.git_remote_text,
            RefKind::Tag => |t| t.git_tag_text,
        };
        let name = if label.kind == RefKind::Tag {
            format!("tag: {}", label.name)
        } else {
            label.name.clone()
        };
        first.push((
            format!("{name} "),
            colored(color).add_modifier(Modifier::BOLD),
        ));
    }
    let summary_style = if is_head {
        colored(|t| t.git_head_text).add_modifier(Modifier::BOLD)
    } else {
        row_style
    };
    if !first.is_empty() {
        first.push((" ".to_owned(), row_style));
    }
    first.push((commit.summary.clone(), summary_style));
    let second = vec![
        (commit.author.clone(), colored(|t| t.git_author_text)),
        ("  ".to_owned(), row_style),
        (commit.short_id(), colored(|t| t.git_hash_text)),
        ("  ".to_owned(), row_style),
        (commit.time.to_string(), colored(|t| t.git_date_text)),
    ];
    (first, second)
}

/// The columns a commit's row reaches, drawn `lanes` lanes wide: its
/// part of the graph, a gap, and the longer of its two lines.
pub(crate) fn commit_extent(commit: &Commit, highlight: Highlight, lanes: usize) -> usize {
    let (first, second) = commit_lines(commit, highlight, None);
    cells_for(lanes) + 1 + pieces_width(&first).max(pieces_width(&second))
}

/// Draw one of a commit's lines into the one-row `area`: its part of
/// the graph, `lanes` wide, in the lanes' colors over `row_style`, then
/// the line's text after a gap, scrolled sideways by `scroll` columns
/// with the graph staying put.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_commit_line(
    buf: &mut Buffer,
    area: Rect,
    commit: &Commit,
    line: CommitLine,
    highlight: Highlight,
    lanes: usize,
    row_style: Style,
    theme: &Theme,
    scroll: usize,
) {
    let is_head = highlight != Highlight::None;
    let lane_colors = lane_palette(theme);
    let cells = match line {
        CommitLine::Node => commit.graph.node_cells(lanes),
        CommitLine::Transition => commit.graph.transition_cells(lanes),
    };
    for (k, cell) in cells.iter().enumerate() {
        let x = area.x + k as u16;
        if x >= area.right() {
            break;
        }
        let mut style = row_style;
        if let Some(color) = cell.color {
            style = style.fg(lane_colors[color % lane_colors.len()]);
        }
        let glyph = if cell.node && is_head {
            style = style.add_modifier(Modifier::BOLD);
            HEAD_NODE
        } else if cell.node {
            NODE
        } else {
            cell.glyph
        };
        buf[(x, area.y)].set_symbol(glyph).set_style(style);
    }
    let text_x = area.x + cells_for(lanes) as u16 + 1;
    let text_width = area.right().saturating_sub(text_x) as usize;
    if text_width == 0 {
        return;
    }
    let (first, second) = commit_lines(commit, highlight, Some((theme, row_style)));
    let pieces = match line {
        CommitLine::Node => &first,
        CommitLine::Transition => &second,
    };
    draw_pieces(buf, text_x, area.y, text_width, pieces, scroll);
}
