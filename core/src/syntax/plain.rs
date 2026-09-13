//! Plain text: a lexer that classifies nothing.
//!
//! This is the language of files whose kind isn't known from their name.
//! Giving them a lexer, rather than no highlighting at all, means the
//! things that apply to every file alike, such as merge conflict markers
//! (see the [`conflicts`](super::conflicts) module), are recognized in
//! them too.

use super::{LexState, Lexer, Token};

pub struct Plain;

impl Lexer for Plain {
    fn lex_line(&self, state: LexState, _line: &[u8], _out: &mut Vec<Token>) -> LexState {
        state
    }
}
