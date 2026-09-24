//! The text of a file as the evaluation sees it: `\n` line breaks, as the
//! completion model sees it too, with a way back to offsets in the
//! editor's buffer, the words it is measured in, and its brackets.

use ninjaedit_core::auto_indent::{closer_of, is_closer, is_code};
use ninjaedit_core::syntax::lex_text;
use ninjaedit_core::{FileBuffer, Language};
use std::ops::Range;

/// What a character is for splitting text into words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Space,
    /// Letters, digits and `_`: identifiers, keywords and numbers.
    Word,
    /// Everything else: operators and punctuation.
    Punct,
}

pub fn class(c: char) -> Class {
    if c.is_whitespace() {
        Class::Space
    } else if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else {
        Class::Punct
    }
}

/// The words of `text`: each maximal run of [word](Class::Word) or of
/// [punctuation](Class::Punct) characters, so `x.len() >= 1;` is `x`,
/// `.`, `len`, `()`, `>=`, `1` and `;`. Whitespace is not part of any
/// word.
pub fn words(text: &str) -> Vec<Range<usize>> {
    let mut words: Vec<Range<usize>> = Vec::new();
    let mut last = Class::Space;
    for (i, c) in text.char_indices() {
        let class = class(c);
        match words.last_mut() {
            Some(word) if class == last && class != Class::Space => word.end = i + c.len_utf8(),
            _ if class != Class::Space => words.push(i..i + c.len_utf8()),
            _ => {}
        }
        last = class;
    }
    words
}

/// One line of a [`Document`].
#[derive(Clone, Debug)]
pub struct Line {
    /// Where the line starts in the text.
    pub start: usize,
    /// Where its content ends, not counting trailing whitespace.
    pub content_end: usize,
    /// The length of its indentation.
    pub indent: usize,
    /// Its words, as offsets into the text.
    pub words: Vec<Range<usize>>,
}

/// A file's text, with `\n` line breaks.
pub struct Document {
    pub text: String,
    pub lines: Vec<Line>,
    /// The offsets of its brackets, opening and closing, in order;
    /// only those in code, not in strings or comments, as the editor
    /// lexes them.
    pub brackets: Vec<usize>,
}

impl Document {
    /// The document for a buffer's contents; `None` if they aren't UTF-8,
    /// as the model only ever sees text.
    pub fn from_buffer(buffer: &FileBuffer, language: Language) -> Option<Document> {
        let mut text = String::with_capacity(buffer.len());
        for line in 0..buffer.line_count() {
            if line > 0 {
                text.push('\n');
            }
            text.push_str(
                std::str::from_utf8(&buffer.bytes_in_range(buffer.line_content_range(line)))
                    .ok()?,
            );
        }
        Some(Document::new(text, language))
    }

    pub fn new(text: String, language: Language) -> Document {
        let mut lines = Vec::new();
        let mut brackets = Vec::new();
        let mut start = 0;
        // The lexer has no line after a final line break; that one is
        // empty anyway.
        let tokens = lex_text(language, &text)
            .into_iter()
            .chain(std::iter::repeat_with(Vec::new));
        for (content, tokens) in text.split('\n').zip(tokens) {
            let bytes = content.as_bytes();
            let mut code = vec![true; bytes.len()];
            for token in tokens.iter().filter(|t| !is_code(t.kind)) {
                code[token.range.start.min(bytes.len())..token.range.end.min(bytes.len())]
                    .fill(false);
            }
            brackets.extend(
                (0..bytes.len())
                    .filter(|&i| code[i] && (closer_of(bytes[i]).is_some() || is_closer(bytes[i])))
                    .map(|i| start + i),
            );
            let trimmed = content.trim_end();
            let indent = content.len() - content.trim_start().len();
            let words = words(trimmed)
                .into_iter()
                .map(|w| start + w.start..start + w.end)
                .collect();
            lines.push(Line {
                start,
                content_end: start + trimmed.len(),
                indent: indent.min(trimmed.len()),
                words,
            });
            start += content.len() + 1;
        }
        Document {
            text,
            lines,
            brackets,
        }
    }

    /// The line and the byte within it of an offset into the text, for
    /// finding the same place in the buffer.
    pub fn line_and_column(&self, offset: usize) -> (usize, usize) {
        let line = self.lines.partition_point(|l| l.start <= offset) - 1;
        (line, offset - self.lines[line].start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_are_runs_of_one_class() {
        let text = "let n = x.len() >= 1;  // é_1";
        let found: Vec<&str> = words(text).into_iter().map(|w| &text[w]).collect();
        assert_eq!(
            found,
            [
                "let", "n", "=", "x", ".", "len", "()", ">=", "1", ";", "//", "é_1"
            ]
        );
        assert!(words(" \n\t").is_empty());
    }

    #[test]
    fn lines_know_their_content_and_words() {
        let doc = Document::new("fn f() {\n    a(); \n\n}".to_owned(), Language::Rust);
        assert_eq!(doc.lines.len(), 4);
        let line = &doc.lines[1];
        assert_eq!(line.start, 9);
        assert_eq!(line.indent, 4);
        assert_eq!(&doc.text[line.start..line.content_end], "    a();");
        let words: Vec<&str> = line.words.iter().map(|w| &doc.text[w.clone()]).collect();
        assert_eq!(words, ["a", "();"]);
        assert!(doc.lines[2].words.is_empty());
        assert_eq!(doc.line_and_column(0), (0, 0));
        assert_eq!(doc.line_and_column(15), (1, 6));
        assert_eq!(doc.line_and_column(19), (2, 0));
        assert_eq!(doc.line_and_column(20), (3, 0));
    }

    #[test]
    fn a_buffer_with_crlf_line_breaks_reads_as_plain_line_breaks() {
        let buffer = FileBuffer::from_text("a\r\nb\r\n");
        let doc = Document::from_buffer(&buffer, Language::Rust).unwrap();
        assert_eq!(doc.text, "a\nb\n");
        assert_eq!(doc.lines.len(), 3);
        assert!(
            Document::from_buffer(&FileBuffer::from_bytes(b"\xff\n"), Language::Rust).is_none()
        );
    }

    #[test]
    fn brackets_are_found_only_in_code() {
        let text = "f(\"(\", ')', [1]); // {\n/* ) */ g(x)\n";
        let doc = Document::new(text.to_owned(), Language::Rust);
        let found: String = doc.brackets.iter().map(|&i| &text[i..=i]).collect();
        assert_eq!(found, "([])()");
        assert_eq!(doc.lines.len(), 3);
        // Plain text has no strings or comments to leave out.
        let doc = Document::new("\"(\"".to_owned(), Language::Plain);
        assert_eq!(doc.brackets, [1]);
    }
}
