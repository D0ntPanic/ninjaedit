//! Writing a hole again with the model's help, as a user would, and
//! counting how much the model wrote.
//!
//! The hole is deleted from the editor's buffer and written back a step
//! at a time, the cursor always at the start of a word. At each step a
//! completion is asked for at the cursor, the way typing asks for one,
//! and compared with the text that was there:
//!
//! * The words it predicted, the longest run of them from the cursor,
//!   are accepted and written in along with the whitespace after them
//!   (a line break and the next line's indentation included, as Enter
//!   would give it). A word is predicted only when the completion ends
//!   it where the text does: `foo` for `foo(` is, `foobar` isn't.
//! * If it predicted none, the next word is typed instead.
//!
//! Either way, the next completion is asked for where that leaves the
//! cursor, until the hole is written. Lines are scored as they are
//! finished, by the completion that finished them; see [`crate::stats`]
//! for what is counted.
//!
//! What is compared is the model's whole completion (as the editor has
//! it in [`Editor::full_suggestion`]), not only the part its confidence
//! thresholds offer, so that the scores are the model's own; the offered
//! part is counted apart, to see how much of the model a user would be
//! shown and how often that is right. The same completion can be offered
//! at other thresholds too, to compare them on the same completions.

use crate::document::{self, Document};
use crate::holes::Hole;
use crate::stats::Stats;
use anyhow::Result;
use ninjaedit_core::{ConfidenceThresholds, Editor, LineConfidence};

/// A completion, as the editor's completer reports it.
pub struct Answer {
    /// The serial of the request it answers.
    pub serial: u64,
    pub text: String,
    /// How many bytes of `text` the thresholds in use offer.
    pub offered: usize,
    /// How sure the model was of each line.
    pub lines: Vec<LineConfidence>,
}

impl Answer {
    /// Whether offering `offered` bytes of it offers only the start of
    /// its first line with text.
    fn partial(&self, offered: usize) -> bool {
        let first = self.lines.iter().find(|line| !line.blank);
        offered > 0 && first.is_some_and(|line| offered < line.end)
    }
}

/// Where completions come from: the model, or in tests a stand-in.
pub trait Source {
    /// Answer the completion request the editor has made wanted, if it
    /// has, leaving it to the caller to offer the answer to the editor.
    fn complete(&mut self, editor: &mut Editor) -> Result<Option<Answer>>;
}

/// What other thresholds would have offered: each with the offers counted
/// at it.
pub type Sweep = [(ConfidenceThresholds, Stats)];

/// A line of a hole that has words in it, by the indices of its first
/// and last words.
struct HoleLine {
    first: usize,
    last: usize,
    /// Whether the hole took the line from its start, so it can be
    /// predicted whole.
    whole_chance: bool,
}

/// Delete `hole` from the editor over `doc` and write it back with the
/// help of `source`, returning what was counted. Each completion is also
/// offered at each thresholds of `sweep`, and what they offer counted
/// there. The buffer holds what it did before when this returns. With
/// `trace`, every completion is printed to stderr next to what was
/// expected.
pub fn reimplement(
    editor: &mut Editor,
    doc: &Document,
    hole: &Hole,
    source: &mut dyn Source,
    sweep: &mut Sweep,
    trace: bool,
) -> Result<Stats> {
    let target = &doc.text[hole.start..hole.end];
    let words = document::words(target);
    // The first line can be predicted whole only if the hole starts at
    // the start of its content; the rest the hole takes all of.
    let (first_line, _) = doc.line_and_column(hole.start);
    let begun = !doc.text[doc.lines[first_line].start..hole.start]
        .trim()
        .is_empty();
    let mut lines: Vec<HoleLine> = Vec::new();
    let mut line_of_word = Vec::with_capacity(words.len());
    for (i, word) in words.iter().enumerate() {
        if i == 0 || target[words[i - 1].end..word.start].contains('\n') {
            lines.push(HoleLine {
                first: i,
                last: i,
                whole_chance: i > 0 || !begun,
            });
        } else {
            lines.last_mut().expect("a line").last = i;
        }
        line_of_word.push(lines.len() - 1);
    }
    let chars = |range: std::ops::Range<usize>| -> usize {
        words[range]
            .iter()
            .map(|w| target[w.clone()].chars().count())
            .sum()
    };
    let mut stats = Stats {
        holes: 1,
        words: words.len(),
        chars: chars(0..words.len()),
        lines: lines.len(),
        whole_line_chances: lines.iter().filter(|l| l.whole_chance).count(),
        ..Stats::default()
    };

    let buffer_offset = |offset: usize| {
        let (line, column) = doc.line_and_column(offset);
        editor.buffer().offset_of_line(line) + column
    };
    let (start, end) = (buffer_offset(hole.start), buffer_offset(hole.end));
    editor.set_selection(start, end);
    editor.delete_selection();

    let mut next = 0;
    while next < words.len() {
        let at = words[next].start;
        editor.request_completion();
        let answer = source.complete(editor)?;
        // What is offered is right if the text goes on with it, past the
        // end of the hole if it goes that far.
        let text_on = &doc.text[hole.start + at..];
        for (thresholds, swept) in sweep.iter_mut() {
            let mut partial = false;
            if let Some(answer) = &answer {
                let offered = thresholds.offered(&answer.lines);
                editor.offer_completion_clipped(answer.serial, &answer.text, offered);
                partial = answer.partial(offered);
            }
            count_offer(swept, editor.suggestion(), partial, text_on);
        }
        let mut partial = false;
        if let Some(answer) = &answer {
            editor.offer_completion_clipped(answer.serial, &answer.text, answer.offered);
            partial = answer.partial(answer.offered);
        }
        let offered_right = count_offer(&mut stats, editor.suggestion(), partial, text_on);
        let suggestion = editor.full_suggestion().unwrap_or("").to_owned();
        let common = common_prefix(&suggestion, &target[at..]);

        let mut predicted = next;
        while predicted < words.len() {
            let word = &target[words[predicted].clone()];
            if !predicts_word(&suggestion, common, words[predicted].end - at, word) {
                break;
            }
            predicted += 1;
        }

        // The lines this completion finished, each through to its end
        // and no further on the line.
        let current = line_of_word[next];
        let mut lines_predicted = 0;
        for line in &lines[current..] {
            if line.last >= predicted || !ends_line(&suggestion, words[line.last].end - at) {
                break;
            }
            lines_predicted += 1;
            if line.whole_chance && line.first >= next {
                stats.whole_lines += 1;
            } else {
                stats.partial_lines += 1;
            }
        }
        if lines.len() - current >= 2 {
            stats.multi_line_chances += 1;
            if lines_predicted >= 2 {
                stats.multi_lines += 1;
            }
        }

        let taken = if predicted > next {
            stats.words_accepted += predicted - next;
            stats.chars_accepted += chars(next..predicted);
            predicted
        } else {
            next + 1
        };
        let upto = words.get(taken).map_or(target.len(), |w| w.start);
        if trace {
            let verdict = if predicted > next {
                format!("+{} words, {lines_predicted} lines", predicted - next)
            } else {
                "typed a word".to_owned()
            };
            let offered = match editor.suggestion() {
                None => "nothing".to_owned(),
                Some(o) if o.len() == suggestion.len() => "all".to_owned(),
                Some(o) => format!("{:?}", head(o)),
            };
            let right = if offered_right { ", right" } else { "" };
            eprintln!(
                "    {verdict}\n      suggested {:?}\n      offered   {offered}{right}\n      expected  {:?}",
                head(&suggestion),
                head(&target[at..])
            );
        }
        editor.dismiss_suggestion();
        editor.insert_text(&target[at..upto]);
        next = taken;
    }
    Ok(stats)
}

/// Count a completion, and what it offered, into `stats`: `partial` if
/// that was only the start of its first line. Returns whether what it
/// offered was right as far as it went, `text_on` being the text that was
/// there from the cursor on.
fn count_offer(stats: &mut Stats, offered: Option<&str>, partial: bool, text_on: &str) -> bool {
    stats.completions += 1;
    let Some(offered) = offered else {
        return false;
    };
    let right = right_so_far(offered, text_on);
    stats.offered += 1;
    stats.offered_right += usize::from(right);
    stats.offered_chars += offered.chars().filter(|c| !c.is_whitespace()).count();
    stats.offered_lines += offered.lines().filter(|l| !l.trim().is_empty()).count();
    if partial {
        stats.offered_partial += 1;
        stats.offered_partial_right += usize::from(right);
    }
    right
}

/// The length of the longest common prefix of `a` and `b`, in bytes, at
/// a character boundary.
fn common_prefix(a: &str, b: &str) -> usize {
    a.char_indices()
        .zip(b.chars())
        .find(|&((_, x), y)| x != y)
        .map_or(a.len().min(b.len()), |((i, _), _)| i)
}

/// Whether a completion predicts the word `word` ending `end` bytes into
/// it, given that its first `common` bytes are right: it has the word,
/// and doesn't run on past the end of it with more of the same class.
fn predicts_word(completion: &str, common: usize, end: usize, word: &str) -> bool {
    if end > common {
        return false;
    }
    let class = document::class(word.chars().next_back().expect("words aren't empty"));
    completion[end..]
        .chars()
        .next()
        .is_none_or(|c| document::class(c) != class)
}

/// Whether `offered`, text offered at the start of `text`, is right as far
/// as it goes: `text` goes on with it, and not with more of the word it
/// ends in.
fn right_so_far(offered: &str, text: &str) -> bool {
    let Some(rest) = text.strip_prefix(offered) else {
        return false;
    };
    let last = offered
        .chars()
        .next_back()
        .expect("offered text isn't empty");
    rest.chars()
        .next()
        .is_none_or(|c| document::class(c) != document::class(last))
}

/// Whether a completion ends its line `end` bytes into it, but for
/// trailing whitespace: it stops there, or goes on to the next line.
fn ends_line(completion: &str, end: usize) -> bool {
    let after = completion[end..].trim_start_matches([' ', '\t']);
    after.is_empty() || after.starts_with('\n')
}

/// The first few lines of some text, for the trace.
fn head(text: &str) -> &str {
    let end = text
        .match_indices('\n')
        .nth(2)
        .map_or(text.len(), |(i, _)| i);
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::holes::{self, HoleKind, Rng};
    use ninjaedit_core::{FileBuffer, Language, TokenConfidence};

    const SOURCE: &str = "\
fn main() {
    let x = compute(a, b);
    let y = x + 1;
    print(y);
}
";

    /// A stand-in for the model that knows the text that is missing at
    /// the cursor, and answers with what `answer` makes of it. With
    /// `run_on`, it is given the rest of the file from the cursor
    /// instead, as a model goes on past the hole. `confidence` says how
    /// sure it is of each word of an answer, each word being a token,
    /// and `thresholds` how sure it must be to offer one.
    struct Oracle<F> {
        original: String,
        answer: F,
        run_on: bool,
        confidence: fn(&str) -> f32,
        thresholds: ConfidenceThresholds,
        asked: Vec<String>,
    }

    impl<F: FnMut(&str) -> String> Source for Oracle<F> {
        fn complete(&mut self, editor: &mut Editor) -> Result<Option<Answer>> {
            let Some(request) = editor.take_completion_request() else {
                return Ok(None);
            };
            let cursor = editor.cursor();
            let missing = self.original.len() - editor.buffer().len();
            let end = if self.run_on {
                self.original.len()
            } else {
                cursor + missing
            };
            let answer = (self.answer)(&self.original[cursor..end]);
            // What the cursor's line holds before it.
            let line = request.prefix.rsplit('\n').next().unwrap();
            self.asked.push(line.to_owned());
            let mut start = 0;
            let lines: Vec<LineConfidence> = answer
                .split('\n')
                .map(|line| {
                    let tokens: Vec<TokenConfidence> = document::words(line)
                        .into_iter()
                        .map(|word| TokenConfidence {
                            end: start + word.end,
                            prob: (self.confidence)(&line[word]),
                            word_end: true,
                        })
                        .collect();
                    let logprob_sum: f32 = tokens.iter().map(|t| t.prob.ln()).sum();
                    let line = LineConfidence {
                        end: start + line.len(),
                        confidence: (logprob_sum / tokens.len().max(1) as f32).exp(),
                        min_prob: tokens.iter().map(|t| t.prob).fold(1.0, f32::min),
                        blank: tokens.is_empty(),
                        tokens,
                    };
                    start = line.end + 1;
                    line
                })
                .collect();
            Ok(Some(Answer {
                serial: request.serial,
                offered: self.thresholds.offered(&lines),
                text: answer,
                lines,
            }))
        }
    }

    /// Every line of an answer, sure or not.
    const EVERYTHING: ConfidenceThresholds = ConfidenceThresholds {
        line: 0.0,
        token: 0.0,
    };

    fn run(hole_text: &str, answer: impl FnMut(&str) -> String) -> (Stats, Vec<String>) {
        run_offering(hole_text, answer, |_| 1.0, EVERYTHING, &mut [])
    }

    fn run_offering(
        hole_text: &str,
        answer: impl FnMut(&str) -> String,
        confidence: fn(&str) -> f32,
        thresholds: ConfidenceThresholds,
        sweep: &mut Sweep,
    ) -> (Stats, Vec<String>) {
        let doc = Document::new(SOURCE.to_owned(), Language::Rust);
        let start = SOURCE.find(hole_text).unwrap();
        let hole = Hole {
            kind: HoleKind::Lines,
            start,
            end: start + hole_text.len(),
        };
        let mut editor = Editor::new(FileBuffer::from_text(SOURCE));
        editor.set_language(Language::Rust);
        let mut oracle = Oracle {
            original: SOURCE.to_owned(),
            answer,
            run_on: false,
            confidence,
            thresholds,
            asked: Vec::new(),
        };
        let stats = reimplement(&mut editor, &doc, &hole, &mut oracle, sweep, false).unwrap();
        assert_eq!(
            editor.buffer().to_bytes(),
            SOURCE.as_bytes(),
            "written back"
        );
        (stats, oracle.asked)
    }

    const THREE_LINES: &str = "let x = compute(a, b);\n    let y = x + 1;\n    print(y);";

    #[test]
    fn a_perfect_model_writes_the_hole_in_one_go() {
        let (stats, asked) = run(THREE_LINES, |missing| missing.to_owned());
        assert_eq!(asked, ["    "]);
        assert_eq!(
            stats,
            Stats {
                holes: 1,
                completions: 1,
                words: 20,
                words_accepted: 20,
                chars: 36,
                chars_accepted: 36,
                lines: 3,
                whole_line_chances: 3,
                whole_lines: 3,
                partial_lines: 0,
                multi_line_chances: 1,
                multi_lines: 1,
                offered: 1,
                offered_right: 1,
                offered_chars: 36,
                offered_lines: 3,
                offered_partial: 0,
                offered_partial_right: 0,
            }
        );
    }

    #[test]
    fn the_whole_completion_is_scored_and_what_it_offers_counted_apart() {
        // Right about all three lines, but sure enough to offer only the
        // first: the second has `y` in it, which it is unsure of.
        let unsure_of_y = |word: &str| if word == "y" { 0.3 } else { 0.9 };
        let at = |line| ConfidenceThresholds { line, token: 0.5 };
        let mut sweep = [(at(0.0), Stats::default()), (at(0.95), Stats::default())];
        sweep[0].0.token = 0.0;
        let (stats, asked) =
            run_offering(THREE_LINES, str::to_owned, unsure_of_y, at(0.5), &mut sweep);
        assert_eq!(asked, ["    "]);
        assert_eq!((stats.words_accepted, stats.whole_lines), (20, 3));
        assert_eq!((stats.offered, stats.offered_right), (1, 1));
        assert_eq!(stats.offered_chars, "letx=compute(a,b);".len());
        assert_eq!(stats.offered_lines, 1);
        assert_eq!(stats.offered_partial, 0, "the whole first line");
        // The same completion at other thresholds: all of it, or none.
        let [(_, all), (_, none)] = sweep;
        assert_eq!((all.completions, all.offered, all.offered_right), (1, 1, 1));
        assert_eq!(all.offered_lines, 3);
        assert_eq!((none.completions, none.offered), (1, 0));
        assert_eq!(none.offered_chars().display(), "-");

        // Unsure of `a` in the first line: the start of it is offered,
        // up to the word before.
        let unsure_of_a = |word: &str| if word == "a" { 0.3 } else { 0.9 };
        let (stats, _) = run_offering(THREE_LINES, str::to_owned, unsure_of_a, at(0.5), &mut []);
        assert_eq!(stats.offered_chars, "letx=compute(".len());
        assert_eq!((stats.offered_partial, stats.offered_partial_right), (1, 1));
        assert_eq!(stats.partial_offers().percent(), "100.0%");

        // Something wrong offered.
        let (stats, _) = run(THREE_LINES, |missing| missing.replace("x + 1", "x + 2"));
        assert_eq!(
            stats.offered_right().percent(),
            "50.0%",
            "right once it was typed"
        );

        // Right as far as it goes, where a word ends.
        assert!(right_so_far("let x", "let x = 1"));
        assert!(right_so_far("x = 1;\n    y", "x = 1;\n    y()"));
        assert!(!right_so_far("let x", "let xy = 1"));
        assert!(!right_so_far("let y", "let x = 1"));
    }

    #[test]
    fn a_silent_model_has_every_word_typed() {
        let (stats, asked) = run(THREE_LINES, |_| String::new());
        assert_eq!(stats.completions, 20);
        assert_eq!(stats.words_accepted, 0);
        assert_eq!((stats.whole_lines, stats.partial_lines), (0, 0));
        // Asked at every word, and at each new line after its
        // indentation, as after Enter.
        assert_eq!(asked[..3], ["    ", "    let ", "    let x "]);
        assert_eq!(asked.iter().filter(|line| *line == "    ").count(), 3);
        // Two lines or more were left for the words of the first two.
        assert_eq!(stats.multi_line_chances, 9 + 7);
        assert_eq!(stats.multi_lines, 0);
    }

    #[test]
    fn a_model_that_predicts_a_line_at_a_time() {
        let (stats, asked) = run(THREE_LINES, |missing| {
            missing.split('\n').next().unwrap().to_owned()
        });
        assert_eq!(asked.len(), 3);
        assert_eq!((stats.whole_lines, stats.partial_lines), (3, 0));
        assert_eq!(stats.words_accepted, 20);
        assert_eq!(stats.whole_line().percent(), "100.0%");
        // Asked twice with two lines or more left, and right about one
        // line each time.
        assert_eq!(stats.multi_line().percent(), "0.0%");
    }

    #[test]
    fn a_wrong_word_is_typed_and_the_rest_of_the_line_predicted() {
        // The model gets `x =` right and `compute` wrong, then with
        // `compute` typed finishes the line, a partial line.
        let (stats, asked) = run("x = compute(a, b);", |missing| {
            missing
                .split('\n')
                .next()
                .unwrap()
                .replace("compute", "calc")
        });
        assert_eq!(asked, ["    let ", "    let x = ", "    let x = compute"]);
        assert_eq!(stats.words, 8);
        assert_eq!(stats.words_accepted, 7);
        assert_eq!(stats.lines, 1);
        assert_eq!(stats.whole_line_chances, 0, "begun before the hole");
        assert_eq!((stats.whole_lines, stats.partial_lines), (0, 1));
        assert_eq!(stats.partial_line().percent(), "100.0%");
    }

    #[test]
    fn running_on_past_the_line_end_is_not_a_line() {
        let (stats, _) = run("print(y);", |missing| format!("{missing} // done"));
        assert_eq!(stats.words_accepted, 4, "every word is right");
        assert_eq!((stats.whole_lines, stats.partial_lines), (0, 0));
        assert_eq!(stats.whole_line().percent(), "0.0%");
    }

    #[test]
    fn a_perfect_model_that_runs_on_gets_every_hole_right() {
        // The editor cuts a suggestion at the end of the cursor's scope,
        // and the holes stay within theirs, so a model that knows the
        // rest of the file gets all of every hole, wherever it starts.
        const SCOPES: &str = "\
fn main() {
    let total = compute(a, [b, c]);
    let v = vec![
        1,
        2,
    ];
    if v.is_empty() {
        println!(\"none)\");
        return;
    }
    go(v, total);
}

fn go(v: Vec<u32>, n: u32) {
    for x in v {
        use_it(x, n);
    }
}
";
        let doc = Document::new(SCOPES.to_owned(), Language::Rust);
        let mut editor = Editor::new(FileBuffer::from_text(SCOPES));
        editor.set_language(Language::Rust);
        let mut oracle = Oracle {
            original: SCOPES.to_owned(),
            answer: str::to_owned,
            run_on: true,
            confidence: |_| 1.0,
            thresholds: EVERYTHING,
            asked: Vec::new(),
        };
        let mut cut_short = 0;
        for seed in 0..20 {
            for hole in holes::choose(&doc, 4, &mut Rng::new(seed)) {
                let (last, _) = doc.line_and_column(hole.end);
                cut_short += usize::from(doc.lines[last].content_end != hole.end);
                let stats =
                    reimplement(&mut editor, &doc, &hole, &mut oracle, &mut [], false).unwrap();
                let text = &SCOPES[hole.start..hole.end];
                assert_eq!(stats.words_accepted, stats.words, "{text:?}");
                assert_eq!(stats.whole_lines, stats.whole_line_chances, "{text:?}");
                assert_eq!(
                    stats.whole_lines + stats.partial_lines,
                    stats.lines,
                    "{text:?}"
                );
                assert_eq!(editor.buffer().to_bytes(), SCOPES.as_bytes(), "{text:?}");
            }
        }
        assert!(cut_short > 0, "some holes end before a closing bracket");
    }

    #[test]
    fn a_word_is_predicted_only_if_it_ends_where_the_text_does() {
        let check = |completion: &str, text: &str, word_end: usize| {
            let word = &text[..word_end];
            let word = &word[document::words(word).last().unwrap().clone()];
            predicts_word(completion, common_prefix(completion, text), word_end, word)
        };
        assert!(check("foo(x)", "foo(y)", 3));
        assert!(check("foo", "foo(y)", 3));
        assert!(!check("foobar", "foo(y)", 3));
        assert!(!check("fo", "foo(y)", 3));
        assert!(!check("x);", "x)\n}", 2), "`);` isn't `)`");
        assert!(check("x)\n}", "x)\n}", 2));
        assert!(check("x) + 1", "x)\n}", 2));
        assert_eq!(common_prefix("héllo", "hélp"), 4);
        assert_eq!(common_prefix("ab", "abc"), 2);
        assert!(ends_line("a;  \nb", 2));
        assert!(ends_line("a;", 2));
        assert!(!ends_line("a; b", 2));
    }
}
