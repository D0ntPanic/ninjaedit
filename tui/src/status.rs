//! What the status bar says, on its left: help, a message, or what is
//! going on. Each kind is drawn in its own color from the theme, so
//! that what the bar is saying can be told at a glance.
//!
//! [`StatusLine::Help`] lists key bindings: each key in the theme's
//! `status-bar-key-text` and what it does in `status-bar-help-text`,
//! with a dot between one binding and the next, as `↑↓ commit · Enter
//! files`. [`StatusLine::Info`] is something that happened, or the
//! file being edited, in `status-bar-filename-text`; and
//! [`StatusLine::Error`] something that couldn't be done, in
//! `status-bar-error-text`. [`StatusLine::Progress`] is something
//! under way that the user is waiting on, in
//! `status-bar-progress-text`; it has the bar to itself, in place of
//! the help, since it is what matters just then and the two together
//! wouldn't fit anyway.

use crate::diff_pane::Piece;
use crate::theme::Theme;
use ratatui::style::Style;
use std::fmt;

/// Between one binding and the next.
const SEPARATOR: &str = " · ";

/// The status bar's text, by kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StatusLine {
    /// Key bindings: each key (which may be empty, for a note among
    /// the bindings) and what it does.
    Help(Vec<(String, String)>),
    /// Something that happened, or what is being edited.
    Info(String),
    /// Something that couldn't be done.
    Error(String),
    /// Something under way, which the user is waiting on.
    Progress(String),
}

impl Default for StatusLine {
    fn default() -> StatusLine {
        StatusLine::Info(String::new())
    }
}

impl StatusLine {
    /// Bindings from a table of keys and what they do.
    pub fn help(bindings: &[(&str, &str)]) -> StatusLine {
        StatusLine::Help(
            bindings
                .iter()
                .map(|(key, action)| ((*key).to_owned(), (*action).to_owned()))
                .collect(),
        )
    }

    pub fn info(text: impl Into<String>) -> StatusLine {
        StatusLine::Info(text.into())
    }

    pub fn error(text: impl Into<String>) -> StatusLine {
        StatusLine::Error(text.into())
    }

    pub fn progress(text: impl Into<String>) -> StatusLine {
        StatusLine::Progress(text.into())
    }

    /// Whether this is something under way.
    pub fn is_progress(&self) -> bool {
        matches!(self, StatusLine::Progress(_))
    }

    /// The text without its colors: the bindings as `key action`, dots
    /// between them.
    pub fn text(&self) -> String {
        match self {
            StatusLine::Help(bindings) => bindings
                .iter()
                .map(|(key, action)| {
                    if key.is_empty() {
                        action.clone()
                    } else {
                        format!("{key} {action}")
                    }
                })
                .collect::<Vec<_>>()
                .join(SEPARATOR),
            StatusLine::Info(text) | StatusLine::Error(text) | StatusLine::Progress(text) => {
                text.clone()
            }
        }
    }

    /// The text in its colors over `base`, ready to draw.
    pub fn pieces(&self, theme: &Theme, base: Style) -> Vec<Piece> {
        match self {
            StatusLine::Help(bindings) => {
                let key_style = base.fg(theme.status_bar_key_text);
                let help_style = base.fg(theme.status_bar_help_text);
                let mut pieces = Vec::new();
                for (index, (key, action)) in bindings.iter().enumerate() {
                    if index > 0 {
                        pieces.push((SEPARATOR.to_owned(), help_style));
                    }
                    if !key.is_empty() {
                        pieces.push((format!("{key} "), key_style));
                    }
                    pieces.push((action.clone(), help_style));
                }
                pieces
            }
            StatusLine::Info(text) => vec![(text.clone(), base.fg(theme.status_bar_filename_text))],
            StatusLine::Error(text) => vec![(text.clone(), base.fg(theme.status_bar_error_text))],
            StatusLine::Progress(text) => {
                vec![(text.clone(), base.fg(theme.status_bar_progress_text))]
            }
        }
    }
}

impl fmt::Display for StatusLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_reads_as_keys_and_actions_in_their_colors() {
        let help = StatusLine::help(&[
            ("↑↓", "commit"),
            ("", "type the message"),
            ("Esc", "cancel"),
        ]);
        assert_eq!(help.text(), "↑↓ commit · type the message · Esc cancel");
        assert_eq!(help.to_string(), help.text());
        let theme = Theme::default();
        let pieces = help.pieces(&theme, Style::default());
        let texts: Vec<&str> = pieces.iter().map(|(text, _)| text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "↑↓ ",
                "commit",
                " · ",
                "type the message",
                " · ",
                "Esc ",
                "cancel"
            ]
        );
        assert_eq!(pieces[0].1.fg, Some(theme.status_bar_key_text));
        assert_eq!(pieces[1].1.fg, Some(theme.status_bar_help_text));
        assert_eq!(pieces[5].1.fg, Some(theme.status_bar_key_text));
    }

    #[test]
    fn messages_take_their_kinds_colors() {
        let theme = Theme::default();
        let color = |line: StatusLine| line.pieces(&theme, Style::default())[0].1.fg;
        assert_eq!(
            color(StatusLine::info("Saved a.rs")),
            Some(theme.status_bar_filename_text)
        );
        assert_eq!(
            color(StatusLine::error("Could not save")),
            Some(theme.status_bar_error_text)
        );
        assert_eq!(
            color(StatusLine::progress("fetching origin…")),
            Some(theme.status_bar_progress_text)
        );
        assert!(StatusLine::progress("x").is_progress());
        assert!(!StatusLine::error("x").is_progress());
    }
}
