//! Merge conflict markers, recognized in every language.
//!
//! When git (or the editor itself, bringing on-disk changes into a buffer
//! with unsaved edits; see the [`merge`](crate::merge) module) can't
//! combine two versions of some lines, it leaves both in the file between
//! marker lines:
//!
//! ```text
//! <<<<<<< ours
//! the lines as we had them
//! ||||||| original
//! the lines both sides started from (diff3 style only)
//! =======
//! the lines as they have them
//! >>>>>>> theirs
//! ```
//!
//! The markers are a property of merging, not of the language the file is
//! written in, so rather than teaching every lexer about them a
//! [`Conflicts`] lexer wraps a language's lexer and looks for them on
//! every line. It works the way the Markdown lexer hands fenced code to
//! another language: while inside a conflict, a [`Context::Conflict`]
//! holds the bottom slot of the [`LexState`] stack, saying which side the
//! line is on, and the wrapped lexer sees the stack above it as its own.
//! Outside a conflict the wrapped lexer's state passes through untouched,
//! so a file without conflicts lexes exactly as it would unwrapped.
//!
//! Marker lines become a single [`TokenKind::ConflictMarker`] token. The
//! lines between them are lexed by the wrapped language as usual, and its
//! state flows straight through all the sides: the text after the conflict
//! continues from where the last side left off. Strictly, both sides start
//! from the state at the opening marker and only one of them survives, but
//! a fixed-size state can't hold two continuations, and a conflict is
//! about to be resolved by hand anyway.
//!
//! A marker is exactly seven of its character at the start of a line,
//! followed by a space (then a label) or the end of the line. The
//! separator and the base marker only count inside a conflict, and only
//! from the side that can precede them, so `=======` under a Markdown
//! heading or `>>>>>>>` on a deeply quoted line is left to the language
//! unless the file is actually in the middle of a conflict.

use super::{ConflictSide, Context, Language, LexState, Lexer, Token, TokenKind};

/// A language's lexer with conflict markers recognized across it.
#[derive(Clone, Copy)]
pub struct Conflicts {
    pub inner: Language,
}

/// The length of a marker: git's default, and what the editor's own
/// merges write.
const MARKER_LEN: usize = 7;

/// A marker line, by its leading character.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Marker {
    /// `<<<<<<<`: the start of a conflict and of our side.
    Start,
    /// `|||||||`: the start of the common ancestor's lines.
    Base,
    /// `=======`: the start of their side.
    Separator,
    /// `>>>>>>>`: the end of the conflict.
    End,
}

/// The marker a line is, if it is one.
fn marker_of(line: &[u8]) -> Option<Marker> {
    let marker = match line.first()? {
        b'<' => Marker::Start,
        b'|' => Marker::Base,
        b'=' => Marker::Separator,
        b'>' => Marker::End,
        _ => return None,
    };
    let run = line.iter().take_while(|&&b| b == line[0]).count();
    if run != MARKER_LEN {
        return None;
    }
    match line.get(MARKER_LEN) {
        None | Some(b' ') => Some(marker),
        Some(_) => None,
    }
}

/// Whether a line's content (without its terminator) is the `<<<<<<<`
/// marker that opens a conflict: where "next conflict" and "previous
/// conflict" take the cursor.
pub(crate) fn is_conflict_start(line: &[u8]) -> bool {
    marker_of(line) == Some(Marker::Start)
}

impl Lexer for Conflicts {
    fn lex_line(&self, state: LexState, line: &[u8], out: &mut Vec<Token>) -> LexState {
        let side = match state.bottom() {
            Some(Context::Conflict { side }) => Some(side),
            _ => None,
        };
        // Which marker, if any, moves the state on from this side. A
        // stray opening marker inside a conflict starts it over rather
        // than nesting, since nesting is never what a merge writes.
        let next = match (marker_of(line), side) {
            (Some(Marker::Start), _) => Some(Some(ConflictSide::Ours)),
            (Some(Marker::Base), Some(ConflictSide::Ours)) => Some(Some(ConflictSide::Base)),
            (Some(Marker::Separator), Some(ConflictSide::Ours | ConflictSide::Base)) => {
                Some(Some(ConflictSide::Theirs))
            }
            (Some(Marker::End), Some(_)) => Some(None),
            _ => None,
        };
        let inner_state = if side.is_some() {
            state.without_bottom()
        } else {
            state
        };
        if let Some(next) = next {
            out.push(Token {
                range: 0..line.len(),
                kind: TokenKind::ConflictMarker,
            });
            return wrap(inner_state, next);
        }
        let inner_state = self.inner.lexer().lex_line(inner_state, line, out);
        wrap(inner_state, side)
    }
}

/// A wrapped lexer's state as seen from outside: inside `side` of a
/// conflict, or as it is when not in one.
fn wrap(inner: LexState, side: Option<ConflictSide>) -> LexState {
    match side {
        Some(side) => inner.with_bottom(Context::Conflict { side }),
        None => inner,
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::check;
    use super::*;

    #[test]
    fn markers_in_plain_text() {
        check(
            Language::Plain,
            "before\n\
             \n\
             <<<<<<< editor\n\
             XXXXXXXXXXXXXX\n\
             ours\n\
             \n\
             ||||||| original\n\
             XXXXXXXXXXXXXXXX\n\
             base\n\
             \n\
             =======\n\
             XXXXXXX\n\
             theirs\n\
             \n\
             >>>>>>> disk\n\
             XXXXXXXXXXXX\n\
             after\n\
             \n",
        );
    }

    #[test]
    fn sides_are_tracked_in_the_state() {
        let lexer = Conflicts {
            inner: Language::Plain,
        };
        let mut out = Vec::new();
        let mut lex = |state, line: &str| lexer.lex_line(state, line.as_bytes(), &mut out);
        let side = |state: LexState| match state.bottom() {
            Some(Context::Conflict { side }) => Some(side),
            _ => None,
        };
        let start = LexState::default();
        assert_eq!(side(lex(start, "text")), None);
        let ours = lex(start, "<<<<<<< a");
        assert_eq!(side(ours), Some(ConflictSide::Ours));
        assert_eq!(side(lex(ours, "body")), Some(ConflictSide::Ours));
        let base = lex(ours, "|||||||");
        assert_eq!(side(base), Some(ConflictSide::Base));
        let theirs = lex(base, "=======");
        assert_eq!(side(theirs), Some(ConflictSide::Theirs));
        assert_eq!(side(lex(ours, "=======")), Some(ConflictSide::Theirs));
        assert_eq!(side(lex(theirs, ">>>>>>> b")), None);
        assert_eq!(lex(theirs, ">>>>>>> b"), LexState::default());
        // Markers out of order are ordinary text.
        assert_eq!(side(lex(theirs, "=======")), Some(ConflictSide::Theirs));
        assert_eq!(side(lex(theirs, "|||||||")), Some(ConflictSide::Theirs));
        assert_eq!(side(lex(start, "=======")), None);
        assert_eq!(side(lex(start, ">>>>>>> x")), None);
        // A second opening marker starts over on our side.
        assert_eq!(side(lex(theirs, "<<<<<<< c")), Some(ConflictSide::Ours));
    }

    #[test]
    fn conflict_start() {
        assert!(is_conflict_start(b"<<<<<<<"));
        assert!(is_conflict_start(b"<<<<<<< HEAD"));
        assert!(!is_conflict_start(b"<<<<<<<HEAD"));
        assert!(!is_conflict_start(b"======="));
        assert!(!is_conflict_start(b">>>>>>> theirs"));
        assert!(!is_conflict_start(b""));
    }

    #[test]
    fn marker_shape() {
        assert_eq!(marker_of(b"<<<<<<<"), Some(Marker::Start));
        assert_eq!(marker_of(b"<<<<<<< HEAD"), Some(Marker::Start));
        assert_eq!(marker_of(b"<<<<<<<HEAD"), None);
        assert_eq!(marker_of(b"<<<<<<"), None);
        assert_eq!(marker_of(b"<<<<<<<<"), None);
        assert_eq!(marker_of(b" <<<<<<<"), None);
        assert_eq!(marker_of(b"======="), Some(Marker::Separator));
        assert_eq!(marker_of(b"========"), None);
        assert_eq!(marker_of(b"|||||||"), Some(Marker::Base));
        assert_eq!(marker_of(b">>>>>>> theirs"), Some(Marker::End));
        assert_eq!(marker_of(b""), None);
        assert_eq!(marker_of(b"x"), None);
    }

    #[test]
    fn wrapped_language_keeps_its_state_across_a_conflict() {
        // A comment opened before the conflict continues through it, and
        // the language's lexer highlights the sides as usual.
        check(
            Language::Rust,
            "/* open\n\
             ccccccc\n\
             <<<<<<< editor\n\
             XXXXXXXXXXXXXX\n\
             still comment */ let x = 1;\n\
             cccccccccccccccc kkk v o np\n\
             =======\n\
             XXXXXXX\n\
             fn f() {}\n\
             kk Fpp pp\n\
             >>>>>>> disk\n\
             XXXXXXXXXXXX\n\
             let y = 2;\n\
             kkk v o np\n",
        );
    }

    #[test]
    fn separator_outside_a_conflict_is_left_to_the_language() {
        // A setext heading underline in Markdown.
        check(
            Language::Markdown,
            "Title\n\
             \n\
             =======\n\
             HHHHHHH\n",
        );
    }
}
