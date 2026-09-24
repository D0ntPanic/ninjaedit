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
//!
//! Those measure the model's whole completions, as though all of each
//! were offered. How much the confidence thresholds let through is
//! counted apart:
//!
//! * **Offered**: of the completions asked for, the share that offered
//!   anything.
//! * **Offered right**: of the completions that offered anything, the
//!   share whose offered text was right as far as it went, every word of
//!   it matching the text that was there.
//! * **Offered length**: the average length of what was offered, over
//!   the completions that offered anything, in non-whitespace characters
//!   and in lines with any.
//! * **Partial offers**: of the completions that offered anything, the
//!   share that offered only the start of the model's first line, the
//!   model being unsure of the rest of it.
//! * **Partial right**: of those, the share right as far as they went.

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
    /// Completions that offered anything.
    pub offered: usize,
    pub offered_right: usize,
    /// Non-whitespace characters offered, over all completions.
    pub offered_chars: usize,
    /// Lines with non-whitespace characters offered, over all
    /// completions.
    pub offered_lines: usize,
    /// Completions that offered only the start of their first line.
    pub offered_partial: usize,
    pub offered_partial_right: usize,
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
        self.offered += other.offered;
        self.offered_right += other.offered_right;
        self.offered_chars += other.offered_chars;
        self.offered_lines += other.offered_lines;
        self.offered_partial += other.offered_partial;
        self.offered_partial_right += other.offered_partial_right;
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

/// One of the reported averages.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Average {
    pub sum: usize,
    pub count: usize,
}

impl Average {
    /// The average; `None` when there was nothing to average.
    pub fn value(self) -> Option<f64> {
        (self.count > 0).then(|| self.sum as f64 / self.count as f64)
    }

    pub fn display(self) -> String {
        match self.value() {
            Some(v) => format!("{v:.1}"),
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

    pub fn offered_rate(&self) -> Rate {
        Rate {
            hits: self.offered,
            total: self.completions,
        }
    }

    pub fn offered_right(&self) -> Rate {
        Rate {
            hits: self.offered_right,
            total: self.offered,
        }
    }

    pub fn partial_offers(&self) -> Rate {
        Rate {
            hits: self.offered_partial,
            total: self.offered,
        }
    }

    pub fn partial_right(&self) -> Rate {
        Rate {
            hits: self.offered_partial_right,
            total: self.offered_partial,
        }
    }

    pub fn offered_chars(&self) -> Average {
        Average {
            sum: self.offered_chars,
            count: self.offered,
        }
    }

    pub fn offered_lines(&self) -> Average {
        Average {
            sum: self.offered_lines,
            count: self.offered,
        }
    }

    /// Every rate, with the name it is reported under and its key in the
    /// JSON report.
    pub fn rates(&self) -> [(&'static str, &'static str, Rate); 9] {
        [
            ("next word", "next_word", self.next_word()),
            ("partial line", "partial_line", self.partial_line()),
            ("whole line", "whole_line", self.whole_line()),
            ("multi-line", "multi_line", self.multi_line()),
            ("characters", "characters", self.characters()),
            ("offered", "offered", self.offered_rate()),
            ("offered right", "offered_right", self.offered_right()),
            ("partial offers", "partial_offers", self.partial_offers()),
            ("partial right", "partial_right", self.partial_right()),
        ]
    }

    /// Every average, with the name it is reported under and its key in
    /// the JSON report.
    pub fn averages(&self) -> [(&'static str, &'static str, Average); 2] {
        [
            ("offered chars", "offered_chars", self.offered_chars()),
            ("offered lines", "offered_lines", self.offered_lines()),
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
        let averages = self
            .averages()
            .into_iter()
            .map(|(_, key, average)| (key.to_owned(), serde_json::json!(average.value())))
            .collect::<serde_json::Map<_, _>>();
        value["averages"] = serde_json::Value::Object(averages);
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
        assert_eq!(stats.offered_rate().percent(), "-");
        assert_eq!(stats.offered_chars().display(), "-");
        stats.completions = 4;
        stats.offered = 3;
        stats.offered_right = 2;
        stats.offered_chars = 40;
        stats.offered_lines = 4;
        assert_eq!(stats.offered_rate().percent(), "75.0%");
        assert_eq!(stats.offered_right().percent(), "66.7%");
        assert_eq!(stats.offered_chars().display(), "13.3");
        assert_eq!(stats.offered_lines().value(), Some(4.0 / 3.0));
        assert_eq!(stats.partial_right().percent(), "-");
        stats.offered_partial = 1;
        assert_eq!(stats.partial_offers().percent(), "33.3%");
        stats += stats;
        assert_eq!(stats.words, 20);
        assert_eq!(stats.next_word().value(), Some(0.4));
        let json = stats.to_json();
        assert_eq!(json["words"], 20);
        assert_eq!(json["rates"]["next_word"], 0.4);
        assert!(json["rates"]["multi_line"].is_null());
        assert_eq!(json["rates"]["offered"], 0.75);
        assert_eq!(json["averages"]["offered_lines"], 8.0 / 6.0);
    }
}
