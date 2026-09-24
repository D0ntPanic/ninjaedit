//! What the evaluation counts, and the rates reported from the counts.
//!
//! Words are runs of identifier characters or of punctuation (see
//! [`words`](crate::document::words)); whitespace isn't counted, and a
//! line is counted only if it has a word in it. Where a hole ends before
//! a closing bracket already on its last line (see [`crate::holes`]),
//! that line's end is where the hole ends, as it is for the editor's
//! suggestions.
//!
//! * **Next word**: of the words written in the holes, the share that
//!   came from accepting a completion rather than being typed.
//! * **Partial line**: of the lines that weren't predicted whole, the
//!   share whose rest was predicted by a completion asked for partway
//!   through the line, with part of it already written.
//! * **Whole line**: of the lines the hole took all of, the share that a
//!   single completion predicted from the line's start to its end, and no
//!   further on the line, as Tab would take it.
//! * **Multi-line**: of the completions asked for with two lines or more
//!   still to write, the share that predicted at least two of them to
//!   their ends.
//! * **Characters**: of the non-whitespace characters written, the share
//!   that came from accepting completions: the typing saved.

use serde::Serialize;
use std::ops::AddAssign;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Stats {
    pub holes: usize,
    pub completions: usize,
    pub words: usize,
    pub words_accepted: usize,
    pub chars: usize,
    pub chars_accepted: usize,
    pub lines: usize,
    /// Lines the hole took all of, which could be predicted whole.
    pub whole_line_chances: usize,
    pub whole_lines: usize,
    pub partial_lines: usize,
    /// Completions asked for with at least two lines left to write.
    pub multi_line_chances: usize,
    pub multi_lines: usize,
}

impl AddAssign for Stats {
    fn add_assign(&mut self, other: Stats) {
        self.holes += other.holes;
        self.completions += other.completions;
        self.words += other.words;
        self.words_accepted += other.words_accepted;
        self.chars += other.chars;
        self.chars_accepted += other.chars_accepted;
        self.lines += other.lines;
        self.whole_line_chances += other.whole_line_chances;
        self.whole_lines += other.whole_lines;
        self.partial_lines += other.partial_lines;
        self.multi_line_chances += other.multi_line_chances;
        self.multi_lines += other.multi_lines;
    }
}

/// One of the reported rates.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Rate {
    pub hits: usize,
    pub total: usize,
}

impl Rate {
    /// The rate as a fraction; `None` when there was nothing to count.
    pub fn value(self) -> Option<f64> {
        (self.total > 0).then(|| self.hits as f64 / self.total as f64)
    }

    pub fn percent(self) -> String {
        match self.value() {
            Some(v) => format!("{:.1}%", v * 100.0),
            None => "-".to_owned(),
        }
    }
}

impl Stats {
    pub fn next_word(&self) -> Rate {
        Rate {
            hits: self.words_accepted,
            total: self.words,
        }
    }

    pub fn partial_line(&self) -> Rate {
        Rate {
            hits: self.partial_lines,
            total: self.lines - self.whole_lines,
        }
    }

    pub fn whole_line(&self) -> Rate {
        Rate {
            hits: self.whole_lines,
            total: self.whole_line_chances,
        }
    }

    pub fn multi_line(&self) -> Rate {
        Rate {
            hits: self.multi_lines,
            total: self.multi_line_chances,
        }
    }

    pub fn characters(&self) -> Rate {
        Rate {
            hits: self.chars_accepted,
            total: self.chars,
        }
    }

    /// Every rate, with the name it is reported under and its key in the
    /// JSON report.
    pub fn rates(&self) -> [(&'static str, &'static str, Rate); 5] {
        [
            ("next word", "next_word", self.next_word()),
            ("partial line", "partial_line", self.partial_line()),
            ("whole line", "whole_line", self.whole_line()),
            ("multi-line", "multi_line", self.multi_line()),
            ("characters", "characters", self.characters()),
        ]
    }

    /// The counts and rates as JSON.
    pub fn to_json(self) -> serde_json::Value {
        let mut value = serde_json::to_value(self).expect("plain counts");
        let rates = self
            .rates()
            .into_iter()
            .map(|(_, key, rate)| (key.to_owned(), serde_json::json!(rate.value())))
            .collect::<serde_json::Map<_, _>>();
        value["rates"] = serde_json::Value::Object(rates);
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_come_from_the_counts() {
        let mut stats = Stats {
            words: 10,
            words_accepted: 4,
            lines: 5,
            whole_line_chances: 4,
            whole_lines: 1,
            partial_lines: 2,
            ..Stats::default()
        };
        assert_eq!(stats.next_word().percent(), "40.0%");
        assert_eq!(stats.whole_line().percent(), "25.0%");
        // Two of the four lines not predicted whole.
        assert_eq!(stats.partial_line().percent(), "50.0%");
        assert_eq!(stats.multi_line().percent(), "-");
        stats += stats;
        assert_eq!(stats.words, 20);
        assert_eq!(stats.next_word().value(), Some(0.4));
        let json = stats.to_json();
        assert_eq!(json["words"], 20);
        assert_eq!(json["rates"]["next_word"], 0.4);
        assert!(json["rates"]["multi_line"].is_null());
    }
}
