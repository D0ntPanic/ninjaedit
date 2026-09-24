//! Settings: what the user has configured, kept in `settings.toml` in the
//! [storage](crate::storage) directory.
//!
//! Every setting has a default, and the file holds only the settings that
//! differ from theirs: a setting not in the file is at its default, and
//! resetting one takes it out of the file. The settings are grouped into
//! categories, one table each:
//!
//! ```toml
//! [editor]
//! continuation-indent = "indent"
//!
//! [terminal]
//! shell = "/opt/homebrew/bin/fish"
//! scrollback = 50000
//!
//! [search]
//! max-results = 1000
//!
//! [build]
//! cmake-generator = "Xcode"
//!
//! [completion]
//! rust-model = "~/models/rust-70m"
//! min-line-confidence = 0.8
//! ```
//!
//! Keys the editor doesn't know are kept as they are and written back, so
//! a file shared between versions of the editor loses nothing to the
//! older one.
//!
//! Frontends work through [`SettingKey`], which lists every setting with
//! its category, name, description, and [kind](SettingKind), and through
//! the text form of each value: a setting is read as text and set from
//! text (a number's digits, a program's path, the value of one of its
//! choices), with the parsing and the checking done here. The kind tells
//! a frontend which control suits the setting, a text field or a choice
//! among a fixed set of options, and a settings page is so a list of
//! such controls whatever the settings are: adding a setting means a
//! variant here and nothing in the frontend. Code that uses a setting reads it through its typed
//! accessor, [`terminal_scrollback`](Settings::terminal_scrollback) and
//! the like.

use crate::auto_indent::{CodeStyle, ContinuationIndent};
use crate::build::DEFAULT_CMAKE_GENERATOR;
use crate::completion::ConfidenceThresholds;
use crate::project_search::MAX_MATCHES;
use crate::syntax::Language;
use crate::terminal::emulator::DEFAULT_SCROLLBACK;
use std::fmt;
use toml::{Table, Value};

/// A group of related settings, shown together.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Editor,
    Terminal,
    Search,
    Build,
    /// Code completion: the model for each language it is available in,
    /// and how sure a model must be to offer a completion.
    Completion,
}

impl Category {
    /// Every category, in the order a settings page shows them.
    pub const ALL: [Category; 5] = [
        Category::Editor,
        Category::Terminal,
        Category::Search,
        Category::Build,
        Category::Completion,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Category::Editor => "Editor",
            Category::Terminal => "Terminal",
            Category::Search => "Search",
            Category::Build => "Build",
            Category::Completion => "Code Completion",
        }
    }

    /// The category's table in the settings file.
    fn table(self) -> &'static str {
        match self {
            Category::Editor => "editor",
            Category::Terminal => "terminal",
            Category::Search => "search",
            Category::Build => "build",
            Category::Completion => "completion",
        }
    }

    /// The settings in this category, in the order a page shows them.
    pub fn keys(self) -> impl Iterator<Item = SettingKey> {
        SettingKey::ALL
            .into_iter()
            .filter(move |key| key.category() == self)
    }
}

/// What kind of value a setting holds, for a frontend to pick a control
/// for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingKind {
    /// Text typed by the user, checked when it is set: a path, a name, a
    /// number.
    Text,
    /// One of a fixed set of options, listed in the order a frontend
    /// shows them.
    Choice(&'static [SettingChoice]),
}

/// One option of a [`SettingKind::Choice`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SettingChoice {
    /// The option's value in text form: what [`Settings::text`] reports
    /// when it is chosen, [`Settings::set_text`] takes to choose it, and
    /// the settings file holds.
    pub value: &'static str,
    /// The option as a frontend shows it.
    pub label: &'static str,
}

/// The options of [`SettingKey::ContinuationIndent`].
const CONTINUATION_CHOICES: [SettingChoice; 2] = [
    SettingChoice {
        value: "align",
        label: "Align with the bracket",
    },
    SettingChoice {
        value: "indent",
        label: "Indent one level",
    },
];

/// One setting. Each variant has a field in [`Settings`] and a key in
/// the settings file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingKey {
    /// How lines continuing inside brackets are indented as code is
    /// typed; see [`ContinuationIndent`].
    ContinuationIndent,
    /// The program the shell tool runs; blank to detect the user's shell.
    Shell,
    /// How many lines of output a terminal keeps to scroll back through.
    TerminalScrollback,
    /// How many matches a project search stops at.
    SearchMaxResults,
    /// The generator CMake configures with, passed as `-G`; blank to let
    /// CMake choose its own.
    CMakeGenerator,
    /// The directory of the code completion model for Rust files; blank
    /// for no completion in them. Completion models are trained per
    /// language, so each language that has one gets a setting like this.
    RustCompletionModel,
    /// How sure the model must be of a line of a completion, on average
    /// over its tokens, for the line to be offered; see
    /// [`ConfidenceThresholds::line`].
    CompletionLineConfidence,
    /// How sure the model must be of every token of a line of a
    /// completion for the line to be offered; see
    /// [`ConfidenceThresholds::token`].
    CompletionTokenConfidence,
}

impl SettingKey {
    /// Every setting, in the order a settings page lists them (grouped by
    /// category, in [`Category::ALL`]'s order).
    pub const ALL: [SettingKey; 8] = [
        SettingKey::ContinuationIndent,
        SettingKey::Shell,
        SettingKey::TerminalScrollback,
        SettingKey::SearchMaxResults,
        SettingKey::CMakeGenerator,
        SettingKey::RustCompletionModel,
        SettingKey::CompletionLineConfidence,
        SettingKey::CompletionTokenConfidence,
    ];

    pub fn category(self) -> Category {
        match self {
            SettingKey::ContinuationIndent => Category::Editor,
            SettingKey::Shell | SettingKey::TerminalScrollback => Category::Terminal,
            SettingKey::SearchMaxResults => Category::Search,
            SettingKey::CMakeGenerator => Category::Build,
            SettingKey::RustCompletionModel
            | SettingKey::CompletionLineConfidence
            | SettingKey::CompletionTokenConfidence => Category::Completion,
        }
    }

    /// The completion model setting for a language, if completion can be
    /// had for it at all.
    pub fn completion_model(language: Language) -> Option<SettingKey> {
        match language {
            Language::Rust => Some(SettingKey::RustCompletionModel),
            _ => None,
        }
    }

    /// What kind of value the setting holds.
    pub fn kind(self) -> SettingKind {
        match self {
            SettingKey::ContinuationIndent => SettingKind::Choice(&CONTINUATION_CHOICES),
            SettingKey::Shell
            | SettingKey::TerminalScrollback
            | SettingKey::SearchMaxResults
            | SettingKey::CMakeGenerator
            | SettingKey::RustCompletionModel
            | SettingKey::CompletionLineConfidence
            | SettingKey::CompletionTokenConfidence => SettingKind::Text,
        }
    }

    /// The short name shown beside the setting's field.
    pub fn name(self) -> &'static str {
        match self {
            SettingKey::ContinuationIndent => "Continuation indent",
            SettingKey::Shell => "Shell executable",
            SettingKey::TerminalScrollback => "Scrollback lines",
            SettingKey::SearchMaxResults => "Maximum search results",
            SettingKey::CMakeGenerator => "CMake generator",
            SettingKey::RustCompletionModel => "Rust model",
            SettingKey::CompletionLineConfidence => "Line confidence",
            SettingKey::CompletionTokenConfidence => "Token confidence",
        }
    }

    /// A line about what the setting does.
    pub fn description(self) -> &'static str {
        match self {
            SettingKey::ContinuationIndent => "How a line continuing inside brackets is indented",
            SettingKey::Shell => "The program the shell tool runs; blank to use your login shell",
            SettingKey::TerminalScrollback => {
                "Lines of output each terminal keeps to scroll back through"
            }
            SettingKey::SearchMaxResults => {
                "A project search stops after finding this many matches"
            }
            SettingKey::CMakeGenerator => {
                "Passed as -G when CMake configures: Ninja, \"Unix Makefiles\", Xcode, and the like; blank for CMake's own choice"
            }
            SettingKey::RustCompletionModel => {
                "The checkpoint directory (config.json, model.safetensors, tokenizer.json) of the completion model for Rust files; blank for no completion in them"
            }
            SettingKey::CompletionLineConfidence => {
                "How sure the model must be of a line, on average over its tokens, to offer it: 0 to 1, lower for more eager completion, higher for more cautious"
            }
            SettingKey::CompletionTokenConfidence => {
                "How sure the model must be of every token of a line to offer it: 0 to 1, lower for more eager completion, higher for more cautious"
            }
        }
    }

    /// The key within the category's table in the settings file.
    fn key(self) -> &'static str {
        match self {
            SettingKey::ContinuationIndent => "continuation-indent",
            SettingKey::Shell => "shell",
            SettingKey::TerminalScrollback => "scrollback",
            SettingKey::SearchMaxResults => "max-results",
            SettingKey::CMakeGenerator => "cmake-generator",
            SettingKey::RustCompletionModel => "rust-model",
            SettingKey::CompletionLineConfidence => "min-line-confidence",
            SettingKey::CompletionTokenConfidence => "min-token-confidence",
        }
    }

    /// The setting's key as the file spells it, `terminal.shell` and so
    /// on, for messages.
    fn path(self) -> String {
        format!("{}.{}", self.category().table(), self.key())
    }

    /// The default value as text: what the field holds after a reset,
    /// and what the file leaves out.
    pub fn default_text(self) -> String {
        match self {
            SettingKey::ContinuationIndent => {
                continuation_name(ContinuationIndent::default()).to_owned()
            }
            SettingKey::Shell => String::new(),
            SettingKey::TerminalScrollback => DEFAULT_SCROLLBACK.to_string(),
            SettingKey::SearchMaxResults => MAX_MATCHES.to_string(),
            SettingKey::CMakeGenerator => DEFAULT_CMAKE_GENERATOR.to_owned(),
            SettingKey::RustCompletionModel => String::new(),
            SettingKey::CompletionLineConfidence => ConfidenceThresholds::DEFAULT.line.to_string(),
            SettingKey::CompletionTokenConfidence => {
                ConfidenceThresholds::DEFAULT.token.to_string()
            }
        }
    }

    /// What a field shows while its text is empty. For the shell, whose
    /// default is to detect the user's shell, this says which one that is
    /// on this machine; for the CMake generator, that a blank leaves the
    /// choice to CMake; for the others it is the default value.
    pub fn placeholder(self) -> String {
        match self {
            SettingKey::Shell => format!(
                "autodetected: {}",
                crate::terminal::detected_shell().to_string_lossy()
            ),
            SettingKey::CMakeGenerator => "CMake's default generator".to_owned(),
            SettingKey::RustCompletionModel => "no completion for Rust files".to_owned(),
            _ => self.default_text(),
        }
    }
}

/// Why a settings file could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsError(pub(crate) String);

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SettingsError {}

/// The user's settings; see the [module documentation](self). Each field
/// is `None` at its default.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settings {
    continuation_indent: Option<ContinuationIndent>,
    shell: Option<String>,
    terminal_scrollback: Option<usize>,
    search_max_results: Option<usize>,
    cmake_generator: Option<String>,
    rust_completion_model: Option<String>,
    completion_line_confidence: Option<f64>,
    completion_token_confidence: Option<f64>,
    /// Whatever else the settings file held: tables and keys this version
    /// of the editor doesn't know, kept to write back.
    unknown: Table,
}

const FILE_HEADER: &str =
    "# ninjaedit settings. Only settings changed from their defaults are listed.\n";

impl Settings {
    // ----- Typed accessors ------------------------------------------------

    /// How lines continuing inside brackets are indented.
    pub fn continuation_indent(&self) -> ContinuationIndent {
        self.continuation_indent.unwrap_or_default()
    }

    /// The code style settings together, for an editor to lay out code by.
    pub fn code_style(&self) -> CodeStyle {
        CodeStyle {
            continuation: self.continuation_indent(),
        }
    }

    /// The program the shell tool runs, or `None` to detect the user's
    /// shell.
    pub fn shell(&self) -> Option<&str> {
        self.shell.as_deref()
    }

    /// How many lines of output a terminal keeps to scroll back through.
    pub fn terminal_scrollback(&self) -> usize {
        self.terminal_scrollback.unwrap_or(DEFAULT_SCROLLBACK)
    }

    /// How many matches a project search stops at.
    pub fn search_max_results(&self) -> usize {
        self.search_max_results.unwrap_or(MAX_MATCHES)
    }

    /// The generator CMake configures with (`-G`), Ninja unless set.
    /// Empty to pass none and let CMake choose its own.
    pub fn cmake_generator(&self) -> &str {
        self.cmake_generator
            .as_deref()
            .unwrap_or(DEFAULT_CMAKE_GENERATOR)
    }

    /// The checkpoint directory of the code completion model for a
    /// language, as the user typed it (a leading `~` stands for the home
    /// directory), or `None` when there is no completion for it: none
    /// exists for the language, or none has been set.
    pub fn completion_model(&self, language: Language) -> Option<&str> {
        match SettingKey::completion_model(language)? {
            SettingKey::RustCompletionModel => self.rust_completion_model.as_deref(),
            _ => None,
        }
    }

    /// How sure a completion model must be of a line to offer it.
    pub fn completion_thresholds(&self) -> ConfidenceThresholds {
        let default = ConfidenceThresholds::DEFAULT;
        ConfidenceThresholds {
            line: self.completion_line_confidence.unwrap_or(default.line),
            token: self.completion_token_confidence.unwrap_or(default.token),
        }
    }

    // ----- Text form ------------------------------------------------------

    /// A setting's value as text: what a field holds. Blank for a shell
    /// left to be detected.
    pub fn text(&self, key: SettingKey) -> String {
        match key {
            SettingKey::ContinuationIndent => {
                continuation_name(self.continuation_indent()).to_owned()
            }
            SettingKey::Shell => self.shell().unwrap_or_default().to_owned(),
            SettingKey::TerminalScrollback => self.terminal_scrollback().to_string(),
            SettingKey::SearchMaxResults => self.search_max_results().to_string(),
            SettingKey::CMakeGenerator => self.cmake_generator().to_owned(),
            SettingKey::RustCompletionModel => {
                self.rust_completion_model.clone().unwrap_or_default()
            }
            SettingKey::CompletionLineConfidence => self.completion_thresholds().line.to_string(),
            SettingKey::CompletionTokenConfidence => self.completion_thresholds().token.to_string(),
        }
    }

    /// Set a setting from text, as typed into a field. Surrounding
    /// spaces are ignored, and a value equal to the default counts as
    /// the default. Returns whether the setting changed, or, when the
    /// text isn't a valid value, what would be.
    pub fn set_text(&mut self, key: SettingKey, text: &str) -> Result<bool, String> {
        let text = text.trim();
        let before = self.clone();
        match key {
            SettingKey::ContinuationIndent => {
                let continuation = parse_continuation(text)?;
                self.continuation_indent =
                    (continuation != ContinuationIndent::default()).then_some(continuation);
            }
            SettingKey::Shell => {
                self.shell = (!text.is_empty()).then(|| text.to_owned());
            }
            SettingKey::TerminalScrollback => {
                let lines = parse_count(text, 0)?;
                self.terminal_scrollback = (lines != DEFAULT_SCROLLBACK).then_some(lines);
            }
            SettingKey::SearchMaxResults => {
                let matches = parse_count(text, 1)?;
                self.search_max_results = (matches != MAX_MATCHES).then_some(matches);
            }
            SettingKey::CMakeGenerator => {
                self.cmake_generator = (text != DEFAULT_CMAKE_GENERATOR).then(|| text.to_owned());
            }
            SettingKey::RustCompletionModel => {
                self.rust_completion_model = (!text.is_empty()).then(|| text.to_owned());
            }
            SettingKey::CompletionLineConfidence => {
                let line = parse_fraction(text)?;
                self.completion_line_confidence =
                    (line != ConfidenceThresholds::DEFAULT.line).then_some(line);
            }
            SettingKey::CompletionTokenConfidence => {
                let token = parse_fraction(text)?;
                self.completion_token_confidence =
                    (token != ConfidenceThresholds::DEFAULT.token).then_some(token);
            }
        }
        Ok(*self != before)
    }

    /// Whether a setting is at its default.
    pub fn is_default(&self, key: SettingKey) -> bool {
        match key {
            SettingKey::ContinuationIndent => {
                self.continuation_indent() == ContinuationIndent::default()
            }
            SettingKey::Shell => self.shell.is_none(),
            SettingKey::TerminalScrollback => self.terminal_scrollback() == DEFAULT_SCROLLBACK,
            SettingKey::SearchMaxResults => self.search_max_results() == MAX_MATCHES,
            SettingKey::CMakeGenerator => self.cmake_generator() == DEFAULT_CMAKE_GENERATOR,
            SettingKey::RustCompletionModel => self.rust_completion_model.is_none(),
            SettingKey::CompletionLineConfidence => {
                self.completion_thresholds().line == ConfidenceThresholds::DEFAULT.line
            }
            SettingKey::CompletionTokenConfidence => {
                self.completion_thresholds().token == ConfidenceThresholds::DEFAULT.token
            }
        }
    }

    /// Put a setting back to its default. Returns whether that changed
    /// it.
    pub fn reset(&mut self, key: SettingKey) -> bool {
        let was_default = self.is_default(key);
        match key {
            SettingKey::ContinuationIndent => self.continuation_indent = None,
            SettingKey::Shell => self.shell = None,
            SettingKey::TerminalScrollback => self.terminal_scrollback = None,
            SettingKey::SearchMaxResults => self.search_max_results = None,
            SettingKey::CMakeGenerator => self.cmake_generator = None,
            SettingKey::RustCompletionModel => self.rust_completion_model = None,
            SettingKey::CompletionLineConfidence => self.completion_line_confidence = None,
            SettingKey::CompletionTokenConfidence => self.completion_token_confidence = None,
        }
        !was_default
    }

    // ----- The file -------------------------------------------------------

    /// Read a settings file. A setting of the wrong type is an error;
    /// keys the editor doesn't know are kept for [`to_toml`](Self::to_toml)
    /// to write back.
    pub fn parse(text: &str) -> Result<Settings, SettingsError> {
        let mut table: Table = text
            .parse()
            .map_err(|err: toml::de::Error| SettingsError(err.message().to_owned()))?;
        let mut settings = Settings::default();
        for category in Category::ALL {
            let Some(value) = table.get_mut(category.table()) else {
                continue;
            };
            let Some(entries) = value.as_table_mut() else {
                return Err(SettingsError(format!(
                    "`{}` must be a table of settings",
                    category.table()
                )));
            };
            for key in category.keys() {
                let Some(value) = entries.remove(key.key()) else {
                    continue;
                };
                settings.read_value(key, &value)?;
            }
            if entries.is_empty() {
                table.remove(category.table());
            }
        }
        settings.unknown = table;
        Ok(settings)
    }

    /// Take a setting's value from the file.
    fn read_value(&mut self, key: SettingKey, value: &Value) -> Result<(), SettingsError> {
        match key {
            SettingKey::ContinuationIndent => {
                let continuation = value
                    .as_str()
                    .and_then(|text| parse_continuation(text.trim()).ok())
                    .ok_or_else(|| {
                        SettingsError(format!("`{}` must be \"align\" or \"indent\"", key.path()))
                    })?;
                self.continuation_indent =
                    (continuation != ContinuationIndent::default()).then_some(continuation);
            }
            SettingKey::Shell => {
                let shell = value
                    .as_str()
                    .ok_or_else(|| SettingsError(format!("`{}` must be a string", key.path())))?;
                self.shell = (!shell.trim().is_empty()).then(|| shell.trim().to_owned());
            }
            SettingKey::TerminalScrollback => {
                self.terminal_scrollback = Some(read_count(key, value, 0)?);
            }
            SettingKey::SearchMaxResults => {
                self.search_max_results = Some(read_count(key, value, 1)?);
            }
            SettingKey::CMakeGenerator => {
                let generator = value
                    .as_str()
                    .ok_or_else(|| SettingsError(format!("`{}` must be a string", key.path())))?
                    .trim();
                self.cmake_generator =
                    (generator != DEFAULT_CMAKE_GENERATOR).then(|| generator.to_owned());
            }
            SettingKey::RustCompletionModel => {
                let path = value
                    .as_str()
                    .ok_or_else(|| SettingsError(format!("`{}` must be a string", key.path())))?
                    .trim();
                self.rust_completion_model = (!path.is_empty()).then(|| path.to_owned());
            }
            SettingKey::CompletionLineConfidence => {
                self.completion_line_confidence = Some(read_fraction(key, value)?);
            }
            SettingKey::CompletionTokenConfidence => {
                self.completion_token_confidence = Some(read_fraction(key, value)?);
            }
        }
        Ok(())
    }

    /// The settings as a file: the ones that differ from their defaults,
    /// along with whatever the file they were read from held that the
    /// editor doesn't know.
    pub fn to_toml(&self) -> String {
        let mut table = self.unknown.clone();
        for key in SettingKey::ALL {
            if self.is_default(key) {
                continue;
            }
            let value = match key {
                SettingKey::ContinuationIndent | SettingKey::Shell => Value::String(self.text(key)),
                SettingKey::TerminalScrollback => Value::Integer(self.terminal_scrollback() as i64),
                SettingKey::SearchMaxResults => Value::Integer(self.search_max_results() as i64),
                SettingKey::CMakeGenerator | SettingKey::RustCompletionModel => {
                    Value::String(self.text(key))
                }
                SettingKey::CompletionLineConfidence => {
                    Value::Float(self.completion_thresholds().line)
                }
                SettingKey::CompletionTokenConfidence => {
                    Value::Float(self.completion_thresholds().token)
                }
            };
            let category = table
                .entry(key.category().table())
                .or_insert_with(|| Value::Table(Table::new()));
            if let Some(entries) = category.as_table_mut() {
                entries.insert(key.key().to_owned(), value);
            }
        }
        format!("{FILE_HEADER}\n{table}")
    }
}

/// Each [`ContinuationIndent`], in the order of [`CONTINUATION_CHOICES`].
const CONTINUATIONS: [ContinuationIndent; 2] =
    [ContinuationIndent::Align, ContinuationIndent::Indent];

/// The text form of a [`ContinuationIndent`]: its choice's value.
fn continuation_name(continuation: ContinuationIndent) -> &'static str {
    let index = CONTINUATIONS
        .iter()
        .position(|c| *c == continuation)
        .expect("every continuation is listed");
    CONTINUATION_CHOICES[index].value
}

/// Parse the text form of a [`ContinuationIndent`], in any case.
fn parse_continuation(text: &str) -> Result<ContinuationIndent, String> {
    CONTINUATION_CHOICES
        .iter()
        .position(|choice| choice.value.eq_ignore_ascii_case(text))
        .map(|index| CONTINUATIONS[index])
        .ok_or_else(|| "must be align or indent".to_owned())
}

/// Parse a count typed into a field: a whole number of at least `min`.
fn parse_count(text: &str, min: usize) -> Result<usize, String> {
    if text.is_empty() {
        return Err("enter a number".to_owned());
    }
    if !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err("must be a whole number".to_owned());
    }
    let count: usize = text.parse().map_err(|_| "too large".to_owned())?;
    if count < min {
        return Err(format!("must be at least {min}"));
    }
    Ok(count)
}

/// A count from the file: a whole number of at least `min`.
fn read_count(key: SettingKey, value: &Value, min: usize) -> Result<usize, SettingsError> {
    let count = value
        .as_integer()
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| *n >= min)
        .ok_or_else(|| {
            SettingsError(format!(
                "`{}` must be a whole number of at least {min}",
                key.path()
            ))
        })?;
    Ok(count)
}

/// Parse a fraction typed into a field: a number from 0 to 1.
fn parse_fraction(text: &str) -> Result<f64, String> {
    if text.is_empty() {
        return Err("enter a number".to_owned());
    }
    match text.parse::<f64>() {
        Ok(fraction) if (0.0..=1.0).contains(&fraction) => Ok(fraction),
        _ => Err("must be a number from 0 to 1".to_owned()),
    }
}

/// A fraction from the file: a number from 0 to 1, which may be written
/// as a whole number.
fn read_fraction(key: SettingKey, value: &Value) -> Result<f64, SettingsError> {
    value
        .as_float()
        .or_else(|| value.as_integer().map(|n| n as f64))
        .filter(|fraction| (0.0..=1.0).contains(fraction))
        .ok_or_else(|| SettingsError(format!("`{}` must be a number from 0 to 1", key.path())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_built_in_limits_and_a_detected_shell() {
        let settings = Settings::default();
        assert_eq!(settings.shell(), None);
        assert_eq!(settings.terminal_scrollback(), DEFAULT_SCROLLBACK);
        assert_eq!(settings.search_max_results(), MAX_MATCHES);
        assert_eq!(settings.cmake_generator(), "Ninja");
        assert_eq!(settings.completion_model(Language::Rust), None);
        assert_eq!(settings.completion_model(Language::Plain), None);
        for key in SettingKey::ALL {
            assert!(settings.is_default(key), "{key:?}");
            assert_eq!(settings.text(key), key.default_text(), "{key:?}");
        }
        assert_eq!(settings.text(SettingKey::Shell), "");
        assert!(
            SettingKey::Shell
                .placeholder()
                .starts_with("autodetected: ")
        );
        assert_eq!(
            SettingKey::TerminalScrollback.placeholder(),
            DEFAULT_SCROLLBACK.to_string()
        );
        assert_eq!(SettingKey::CMakeGenerator.default_text(), "Ninja");
        assert_ne!(SettingKey::CMakeGenerator.placeholder(), "Ninja");
        // Every setting has a category, and every category its settings.
        let listed: Vec<SettingKey> = Category::ALL.into_iter().flat_map(Category::keys).collect();
        assert_eq!(listed, SettingKey::ALL);
    }

    #[test]
    fn set_from_text_checks_and_normalizes() {
        let mut settings = Settings::default();
        assert_eq!(
            settings.set_text(SettingKey::TerminalScrollback, " 500 "),
            Ok(true)
        );
        assert_eq!(settings.terminal_scrollback(), 500);
        assert_eq!(settings.text(SettingKey::TerminalScrollback), "500");
        assert!(!settings.is_default(SettingKey::TerminalScrollback));
        assert_eq!(
            settings.set_text(SettingKey::TerminalScrollback, "500"),
            Ok(false),
            "unchanged"
        );
        assert_eq!(
            settings.set_text(SettingKey::TerminalScrollback, "0"),
            Ok(true)
        );
        assert_eq!(
            settings.set_text(SettingKey::TerminalScrollback, ""),
            Err("enter a number".to_owned())
        );
        assert_eq!(
            settings.set_text(SettingKey::TerminalScrollback, "1e3"),
            Err("must be a whole number".to_owned())
        );
        assert_eq!(
            settings.set_text(SettingKey::TerminalScrollback, "-1"),
            Err("must be a whole number".to_owned())
        );
        assert_eq!(
            settings.set_text(SettingKey::TerminalScrollback, "99999999999999999999999"),
            Err("too large".to_owned())
        );
        assert_eq!(
            settings.terminal_scrollback(),
            0,
            "a bad value changes nothing"
        );
        assert_eq!(
            settings.set_text(SettingKey::SearchMaxResults, "0"),
            Err("must be at least 1".to_owned())
        );
        assert_eq!(
            settings.set_text(SettingKey::SearchMaxResults, "1"),
            Ok(true)
        );

        // Typing the default is being at the default.
        assert_eq!(
            settings.set_text(
                SettingKey::TerminalScrollback,
                &DEFAULT_SCROLLBACK.to_string()
            ),
            Ok(true)
        );
        assert!(settings.is_default(SettingKey::TerminalScrollback));

        assert_eq!(
            settings.set_text(SettingKey::Shell, "  /bin/zsh "),
            Ok(true)
        );
        assert_eq!(settings.shell(), Some("/bin/zsh"));
        assert_eq!(settings.text(SettingKey::Shell), "/bin/zsh");
        assert_eq!(settings.set_text(SettingKey::Shell, "  "), Ok(true));
        assert_eq!(settings.shell(), None);
        assert!(settings.is_default(SettingKey::Shell));

        // The generator is any text; blank is a choice (CMake's own),
        // not the default.
        assert_eq!(
            settings.set_text(SettingKey::CMakeGenerator, " Xcode "),
            Ok(true)
        );
        assert_eq!(settings.cmake_generator(), "Xcode");
        assert_eq!(settings.text(SettingKey::CMakeGenerator), "Xcode");
        assert_eq!(settings.set_text(SettingKey::CMakeGenerator, ""), Ok(true));
        assert_eq!(settings.cmake_generator(), "");
        assert!(!settings.is_default(SettingKey::CMakeGenerator));
        assert_eq!(
            settings.set_text(SettingKey::CMakeGenerator, "Ninja"),
            Ok(true)
        );
        assert!(settings.is_default(SettingKey::CMakeGenerator));

        // A completion model is a path as typed, blank for none.
        assert_eq!(
            settings.set_text(SettingKey::RustCompletionModel, " ~/models/rust "),
            Ok(true)
        );
        assert_eq!(
            settings.completion_model(Language::Rust),
            Some("~/models/rust")
        );
        assert_eq!(
            settings.text(SettingKey::RustCompletionModel),
            "~/models/rust"
        );
        assert!(!settings.is_default(SettingKey::RustCompletionModel));
        assert_eq!(
            settings.completion_model(Language::C),
            None,
            "no model for C"
        );
        assert_eq!(
            settings.set_text(SettingKey::RustCompletionModel, ""),
            Ok(true)
        );
        assert_eq!(settings.completion_model(Language::Rust), None);
        assert!(settings.is_default(SettingKey::RustCompletionModel));
    }

    #[test]
    fn completion_thresholds_are_fractions() {
        let mut settings = Settings::default();
        assert_eq!(
            settings.completion_thresholds(),
            ConfidenceThresholds::DEFAULT
        );
        assert_eq!(
            settings.set_text(SettingKey::CompletionLineConfidence, " 0.75 "),
            Ok(true)
        );
        assert_eq!(settings.completion_thresholds().line, 0.75);
        assert_eq!(
            settings.completion_thresholds().token,
            ConfidenceThresholds::DEFAULT.token,
            "the other is untouched"
        );
        assert_eq!(settings.text(SettingKey::CompletionLineConfidence), "0.75");
        assert!(!settings.is_default(SettingKey::CompletionLineConfidence));
        for (text, err) in [
            ("", "enter a number"),
            ("1.5", "must be a number from 0 to 1"),
            ("-0.1", "must be a number from 0 to 1"),
            ("NaN", "must be a number from 0 to 1"),
            ("most", "must be a number from 0 to 1"),
        ] {
            assert_eq!(
                settings.set_text(SettingKey::CompletionTokenConfidence, text),
                Err(err.to_owned()),
                "{text:?}"
            );
        }
        assert_eq!(
            settings.set_text(SettingKey::CompletionTokenConfidence, "1"),
            Ok(true)
        );
        assert_eq!(settings.completion_thresholds().token, 1.0);

        let text = settings.to_toml();
        assert!(text.contains("[completion]"), "{text}");
        assert!(text.contains("min-line-confidence = 0.75"), "{text}");
        assert!(text.contains("min-token-confidence = 1.0"), "{text}");
        assert_eq!(Settings::parse(&text).unwrap(), settings);
        // Written by hand as a whole number, or at the default.
        let settings = Settings::parse(&format!(
            "[completion]\nmin-line-confidence = 0\nmin-token-confidence = {}\n",
            ConfidenceThresholds::DEFAULT.token
        ))
        .unwrap();
        assert_eq!(settings.completion_thresholds().line, 0.0);
        assert!(settings.is_default(SettingKey::CompletionTokenConfidence));
        assert_eq!(
            Settings::parse("[completion]\nmin-line-confidence = 2\n")
                .unwrap_err()
                .to_string(),
            "`completion.min-line-confidence` must be a number from 0 to 1"
        );
        assert_eq!(
            Settings::parse("[completion]\nmin-token-confidence = \"high\"\n")
                .unwrap_err()
                .to_string(),
            "`completion.min-token-confidence` must be a number from 0 to 1"
        );
    }

    #[test]
    fn continuation_indent_is_a_choice() {
        let mut settings = Settings::default();
        assert_eq!(settings.continuation_indent(), ContinuationIndent::Indent);
        assert_eq!(settings.text(SettingKey::ContinuationIndent), "indent");
        assert_eq!(
            settings.set_text(SettingKey::ContinuationIndent, " Align "),
            Ok(true)
        );
        assert_eq!(settings.continuation_indent(), ContinuationIndent::Align);
        assert_eq!(
            settings.code_style().continuation,
            ContinuationIndent::Align
        );
        assert_eq!(settings.text(SettingKey::ContinuationIndent), "align");
        assert_eq!(
            settings.set_text(SettingKey::ContinuationIndent, "tab"),
            Err("must be align or indent".to_owned())
        );
        assert_eq!(settings.continuation_indent(), ContinuationIndent::Align);

        let text = settings.to_toml();
        assert!(text.contains("[editor]"), "{text}");
        assert!(text.contains("continuation-indent = \"align\""), "{text}");
        assert_eq!(Settings::parse(&text).unwrap(), settings);

        assert_eq!(
            settings.set_text(SettingKey::ContinuationIndent, "indent"),
            Ok(true)
        );
        assert!(settings.is_default(SettingKey::ContinuationIndent));
        assert!(!settings.to_toml().contains("[editor]"));
        assert_eq!(
            Settings::parse("[editor]\ncontinuation-indent = \"sideways\"\n")
                .unwrap_err()
                .to_string(),
            "`editor.continuation-indent` must be \"align\" or \"indent\""
        );
    }

    #[test]
    fn every_choice_sets_its_value() {
        for key in SettingKey::ALL {
            let SettingKind::Choice(choices) = key.kind() else {
                continue;
            };
            assert!(
                choices.iter().any(|c| c.value == key.default_text()),
                "{key:?}: the default is one of the choices"
            );
            let mut settings = Settings::default();
            for choice in choices {
                assert!(settings.set_text(key, choice.value).is_ok(), "{choice:?}");
                assert_eq!(settings.text(key), choice.value);
            }
        }
        assert_eq!(SettingKey::Shell.kind(), SettingKind::Text);
    }

    #[test]
    fn reset_returns_to_the_default() {
        let mut settings = Settings::default();
        assert!(!settings.reset(SettingKey::Shell), "already the default");
        settings.set_text(SettingKey::Shell, "fish").unwrap();
        settings
            .set_text(SettingKey::SearchMaxResults, "7")
            .unwrap();
        assert!(settings.reset(SettingKey::Shell));
        assert_eq!(settings.shell(), None);
        assert_eq!(settings.search_max_results(), 7, "others are untouched");
        assert!(settings.reset(SettingKey::SearchMaxResults));
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn file_holds_only_what_differs_from_the_defaults() {
        let settings = Settings::default();
        let text = settings.to_toml();
        assert_eq!(Settings::parse(&text).unwrap(), settings);
        assert!(!text.contains('['), "{text}");

        let mut settings = Settings::default();
        settings.set_text(SettingKey::Shell, "/bin/bash").unwrap();
        settings
            .set_text(SettingKey::TerminalScrollback, "2000")
            .unwrap();
        let text = settings.to_toml();
        assert!(text.contains("[terminal]"), "{text}");
        assert!(text.contains("shell = \"/bin/bash\""), "{text}");
        assert!(text.contains("scrollback = 2000"), "{text}");
        assert!(!text.contains("[search]"), "{text}");
        assert!(!text.contains("[build]"), "{text}");
        assert_eq!(Settings::parse(&text).unwrap(), settings);

        // A blank generator is written, since it isn't the default.
        settings.set_text(SettingKey::CMakeGenerator, "").unwrap();
        let text = settings.to_toml();
        assert!(text.contains("[build]"), "{text}");
        assert!(text.contains("cmake-generator = \"\""), "{text}");
        assert_eq!(Settings::parse(&text).unwrap(), settings);
    }

    #[test]
    fn parses_a_hand_written_file_and_keeps_what_it_doesnt_know() {
        let text = "\
# a comment
title = \"mine\"

[terminal]
shell = \"fish\"
font = \"mono\"

[search]
max-results = 25

[build]
cmake-generator = \" Unix Makefiles \"

[completion]
rust-model = \"~/models/rust\"

[git]
sign = true
";
        let settings = Settings::parse(text).unwrap();
        assert_eq!(settings.shell(), Some("fish"));
        assert_eq!(
            settings.completion_model(Language::Rust),
            Some("~/models/rust")
        );
        assert_eq!(settings.search_max_results(), 25);
        assert_eq!(settings.cmake_generator(), "Unix Makefiles", "trimmed");
        assert_eq!(settings.terminal_scrollback(), DEFAULT_SCROLLBACK);

        let written = settings.to_toml();
        let again = Settings::parse(&written).unwrap();
        assert_eq!(again, settings);
        for kept in [
            "title = \"mine\"",
            "font = \"mono\"",
            "[git]",
            "sign = true",
        ] {
            assert!(written.contains(kept), "{written}");
        }
        assert!(!written.contains("# a comment"), "comments aren't kept");

        // Resetting a setting takes it out but leaves the rest.
        let mut settings = settings;
        settings.reset(SettingKey::Shell);
        settings.reset(SettingKey::SearchMaxResults);
        let written = settings.to_toml();
        assert!(!written.contains("shell"), "{written}");
        assert!(!written.contains("[search]"), "{written}");
        assert!(written.contains("font = \"mono\""), "{written}");
    }

    #[test]
    fn a_default_written_out_by_hand_reads_as_the_default() {
        let text = format!(
            "[terminal]\nscrollback = {DEFAULT_SCROLLBACK}\n[build]\ncmake-generator = \"Ninja\"\n"
        );
        let settings = Settings::parse(&text).unwrap();
        assert!(settings.is_default(SettingKey::TerminalScrollback));
        assert!(settings.is_default(SettingKey::CMakeGenerator));
        assert!(!settings.to_toml().contains("scrollback"));
        assert!(!settings.to_toml().contains("cmake-generator"));
    }

    #[test]
    fn rejects_bad_values_and_bad_syntax() {
        let err = |text: &str| Settings::parse(text).unwrap_err().to_string();
        assert!(
            err("[terminal\n").contains("expected"),
            "{}",
            err("[terminal\n")
        );
        assert_eq!(
            err("[terminal]\nscrollback = \"lots\"\n"),
            "`terminal.scrollback` must be a whole number of at least 0"
        );
        assert_eq!(
            err("[search]\nmax-results = 0\n"),
            "`search.max-results` must be a whole number of at least 1"
        );
        assert_eq!(
            err("[terminal]\nshell = 3\n"),
            "`terminal.shell` must be a string"
        );
        assert_eq!(
            err("[build]\ncmake-generator = true\n"),
            "`build.cmake-generator` must be a string"
        );
        assert_eq!(
            err("[completion]\nrust-model = 1\n"),
            "`completion.rust-model` must be a string"
        );
        assert_eq!(
            err("terminal = 1\n"),
            "`terminal` must be a table of settings"
        );
    }
}
