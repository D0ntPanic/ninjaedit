//! Bracket depth, for rainbow brackets.
//!
//! A bracket's color comes from how many pairs enclose it, so that the
//! two ends of a pair match and can be found at a glance in an expression
//! of many nested parentheses. The depth at the start of each line is kept
//! in its [`LexState`](super::LexState) alongside the language's contexts,
//! and brackets are counted from the tokens a language's lexer produces
//! rather than from the text: a bracket inside a string or a comment is
//! part of a string or comment token, so only the brackets of code count.
//! An expression embedded in a string, such as `${x}` in a template
//! literal or `{x}` in a Python f-string, has its delimiters lexed as
//! punctuation, so they count as a pair too and the brackets inside them
//! continue from their depth.
//!
//! Counting is the job of a file's lexer (see [`Language::file_lexer`]
//! and the `conflicts` module), not of each language's: a language's
//! lexer hands back bracket punctuation as [`TokenKind::Punctuation`], and
//! [`color`] turns each into one of the [`TokenKind::BRACKETS`] by depth.
//! Opening and closing brackets are told apart by their text alone, so
//! every language gets the same treatment:
//!
//! * A punctuation token that ends with `(`, `[`, or `{` opens a pair: a
//!   bracket, or a delimiter such as `${`, `#{`, or TOML's `[[`.
//! * One that starts with `)`, `]`, or `}` closes the innermost pair,
//!   whatever its opener; a mismatched pair is the compiler's business.
//!   One that closes nothing (a closing bracket at depth 0) is left as
//!   punctuation, uncolored, which shows that it is unmatched.
//!
//! Angle brackets are not counted, since `<` and `>` are as often
//! comparisons as brackets.
//!
//! Depth carries from line to line, so like a block comment a bracket
//! that is added or removed alone changes the states of every line after
//! it, and the highlighter re-lexes to the end of the file (in the
//! background; see the `highlighter` module). Brackets typed as a pair
//! leave the depth at the end of the line as it was, so re-lexing stops
//! at the next line.
//!
//! [`Language::file_lexer`]: super::Language::file_lexer

use super::{Token, TokenKind};

/// Give the bracket punctuation among a line's `tokens` its kind by
/// depth, starting `depth` pairs deep. Returns the depth at the end of
/// the line.
pub(super) fn color(tokens: &mut [Token], line: &[u8], mut depth: u8) -> u8 {
    for token in tokens {
        if token.kind != TokenKind::Punctuation {
            continue;
        }
        let text = &line[token.range.clone()];
        let (Some(&first), Some(&last)) = (text.first(), text.last()) else {
            continue;
        };
        if matches!(first, b')' | b']' | b'}') {
            if depth > 0 {
                depth -= 1;
                token.kind = TokenKind::bracket(depth);
            }
        } else if matches!(last, b'(' | b'[' | b'{') {
            token.kind = TokenKind::bracket(depth);
            depth = depth.saturating_add(1);
        }
    }
    depth
}

#[cfg(test)]
mod tests {
    use super::super::test_support::check_brackets;
    use super::super::{Language, LexState};

    #[test]
    fn brackets_are_colored_by_depth_cycling_past_four() {
        check_brackets(
            Language::Rust,
            "let x = f(a[b{c(d(e))}], g());\n\
             \x20        1 2 3 4 1 1432p  221p\n",
        );
    }

    #[test]
    fn depth_carries_across_lines() {
        check_brackets(
            Language::Rust,
            "fn main() {\n\
             \x20      11 1\n\
             \x20   if (x) {\n\
             \x20      2 2 2\n\
             \x20       call(\n\
             \x20           3\n\
             \x20           y[0],\n\
             \x20            4 4p\n\
             \x20       );\n\
             \x20       3p\n\
             \x20   }\n\
             \x20   2\n\
             }\n\
             1\n",
        );
    }

    #[test]
    fn brackets_in_strings_and_comments_dont_count() {
        check_brackets(
            Language::Rust,
            "f(\"(\", '[', r#\"{\"#) // )\n\
             \x201   p    p       1\n\
             g( /* ( [\n\
             \x201\n\
             ] */ )\n\
             \x20    1\n",
        );
        check_brackets(
            Language::Python,
            "f(\"\"\"(\n\
             \x201\n\
             [\"\"\", x) # (\n\
             \x20   p  1\n",
        );
    }

    #[test]
    fn unmatched_closers_stay_plain() {
        check_brackets(
            Language::Rust,
            "a) }\n\
             \x20p p\n\
             (b))\n\
             1 1p\n",
        );
    }

    #[test]
    fn embedded_expressions_are_pairs() {
        check_brackets(
            Language::JavaScript,
            "f(`a ${g(x[1])} b`);\n\
             \x201   22 3 4 432   1p\n",
        );
        check_brackets(
            Language::Ruby,
            "puts(\"#{a(b)}\")\n\
             \x20   1 22 3 32 1\n",
        );
        check_brackets(
            Language::Kotlin,
            "f(\"${a(b)}\")\n\
             \x201 22 3 32 1\n",
        );
        check_brackets(
            Language::CSharp,
            "f($\"{a(b)} {{x}}\");\n\
             \x201  2 3 32       1p\n",
        );
    }

    #[test]
    fn python_format_strings_are_pairs() {
        check_brackets(
            Language::Python,
            "f\"value: {((x+3)/5)+2}\"\n\
             \x20        123   3  2  1\n\
             print(f\"{a[0]:>{w}} {{x}}\")\n\
             \x20    1  2 3 3p 3 32       1\n",
        );
    }

    #[test]
    fn data_files_count_their_brackets() {
        check_brackets(
            Language::Json,
            "{\"a\": [1, {\"b\": []}]}\n\
             1   p 2 p 3   p 44321\n",
        );
        check_brackets(
            Language::Toml,
            "[[bin]]\n\
             11   11\n\
             a = { b = [1, 2] }\n\
             \x20   1     2 p  2 1\n",
        );
    }

    #[test]
    fn each_code_fence_starts_afresh() {
        check_brackets(
            Language::Markdown,
            "```rust\n\
             ppp\n\
             fn f() {\n\
             \x20   11 1\n\
             ```\n\
             ppp\n\
             (text)\n\
             \n\
             ```rust\n\
             ppp\n\
             g()\n\
             \x2011\n\
             ```\n\
             ppp\n",
        );
    }

    #[test]
    fn each_side_of_a_conflict_starts_from_the_same_depth() {
        check_brackets(
            Language::Rust,
            "f(a,\n\
             \x201 p\n\
             <<<<<<< ours\n\
             \n\
             \x20 g(b,\n\
             \x20  2 p\n\
             ||||||| base\n\
             \n\
             \x20 h(c,\n\
             \x20  2 p\n\
             =======\n\
             \n\
             \x20 k(d,\n\
             \x20  2 p\n\
             >>>>>>> theirs\n\
             \n\
             \x20 e));\n\
             \x20  21p\n",
        );
        // The depth after the conflict is the depth after its last side.
        let mut state = LexState::default();
        let lexer = Language::Rust.file_lexer();
        for line in ["(", "<<<<<<<", "(", "=======", ">>>>>>>"] {
            state = lexer.lex_line(state, line.as_bytes(), &mut Vec::new());
        }
        assert_eq!(state.bracket_depth(), 1);
    }
}
