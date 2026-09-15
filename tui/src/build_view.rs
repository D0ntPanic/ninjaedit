//! The build configuration page: a mode, like the settings page, that
//! stands in for the editor in the upper part of the screen. It edits
//! the project's [`BuildConfig`]: the build roots, and under each its
//! configurations and targets.
//!
//! The page has two panes. On the left is a tree: each root, and under
//! it its configurations and its targets in name order, with the
//! current configuration and target tagged. On the right are the options
//! of whatever the tree has selected, as text fields laid out the way
//! the settings page lays out the settings: the option's name above
//! the field and a line about it below (wrapped to the pane's width),
//! with the reason a value was refused in that line's place until the
//! text changes. Selecting a root or a list heading shows what can be
//! done with it instead of fields.
//!
//! The keyboard is in one pane at a time. In the tree, ↑ and ↓ move the
//! selection, and Enter, → or Tab move to the options; in the options,
//! Tab and ↓ move down through the fields, ↑ from the first field (or
//! Shift+Tab) goes back to the tree, Enter applies the focused field,
//! Ctrl+S applies them all, and Escape puts the focused field's text
//! back. A field is applied when it is left, as on the settings page.
//! The mouse selects in the tree and focuses fields.
//!
//! Wherever the keyboard is, Ctrl+N adds an entry to the list the
//! selection is in (a configuration, a target, or, with a root
//! selected, a root, which the application asks for with a palette of
//! the project's root files), Ctrl+D duplicates the selected
//! configuration or target, and Delete in the tree removes the selected
//! root, configuration, or target, when pressed twice. A target found
//! in the project (a package, a CMake executable) can't be removed,
//! only disabled, which Delete does at once and undoes on the next
//! press; a disabled target is crossed out in the tree.
//!
//! The page only edits the configuration; the application saves it
//! whenever the page reports a change.

use crate::clipboard::Clipboard;
use crate::fields::{FieldKey, Fields, wrap_words};
use crate::palette::palette_background;
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ninjaedit_core::{BuildConfig, BuildSystem, ConfigurationKey, TargetKey};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position as ScreenPosition, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

/// The name of the page, shown in its tab.
pub const TITLE: &str = "Build Configuration";
/// The tag on the current configuration and target in the tree.
const CURRENT_TAG: &str = "current";
/// After the Targets heading of a root whose targets are still being
/// found.
const FINDING_TAG: &str = "(finding…)";
/// The note under that heading.
const FINDING_NOTE: &str =
    "The project is still being looked through; the targets found will appear here";
/// The widest a field gets.
const MAX_FIELD_WIDTH: u16 = 76;
/// The tree pane's width, within these bounds, as a share of the page.
const MIN_TREE_WIDTH: u16 = 22;
const MAX_TREE_WIDTH: u16 = 40;
/// How far the options pane's content is indented from its edge.
const INDENT: u16 = 2;
/// Rows scrolled per mouse wheel notch.
const WHEEL_LINES: usize = 3;
const NO_ROOTS: &str = "No build roots";
const NO_ROOTS_HINT: &str = "Ctrl+N adds a Cargo.toml or CMakeLists.txt";

/// What the application should do after the page handled an event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BuildOutcome {
    /// Nothing the application needs to act on.
    Continue,
    /// The configuration changed: save it.
    Changed,
    /// The user asked to add a root: offer the project's root files.
    AddRoot,
    /// A message for the status bar, such as a request to confirm.
    Notice(String),
}

/// One row of the tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Node {
    Root(usize),
    /// The `Configurations` heading under a root.
    Configurations(usize),
    Configuration(usize, usize),
    /// The `Targets` heading under a root.
    Targets(usize),
    Target(usize, usize),
}

impl Node {
    fn root(self) -> usize {
        match self {
            Node::Root(root)
            | Node::Configurations(root)
            | Node::Configuration(root, _)
            | Node::Targets(root)
            | Node::Target(root, _) => root,
        }
    }

    /// How far the row is indented.
    fn indent(self) -> u16 {
        match self {
            Node::Root(_) => 1,
            Node::Configurations(_) | Node::Targets(_) => 3,
            Node::Configuration(..) | Node::Target(..) => 5,
        }
    }

    /// The label of the row's root.
    fn root_label(self, config: &BuildConfig) -> String {
        config
            .root(self.root())
            .map(|r| r.label())
            .unwrap_or_default()
    }

    /// Whether the row is a disabled target.
    fn is_disabled(self, config: &BuildConfig) -> bool {
        match self {
            Node::Target(root, i) => config
                .root(root)
                .and_then(|r| r.targets().get(i))
                .is_some_and(|t| t.disabled),
            _ => false,
        }
    }

    fn label(self, config: &BuildConfig) -> String {
        let root = config.root(self.root());
        match self {
            Node::Root(_) => root.map(|r| r.label()).unwrap_or_default(),
            Node::Configurations(_) => "Configurations".to_owned(),
            Node::Targets(_) if root.is_some_and(|r| r.is_pending()) => {
                format!("Targets {FINDING_TAG}")
            }
            Node::Targets(_) => "Targets".to_owned(),
            Node::Configuration(_, i) => root
                .and_then(|r| r.configurations().get(i))
                .map(|c| c.name.clone())
                .unwrap_or_default(),
            Node::Target(_, i) => root
                .and_then(|r| r.targets().get(i))
                .map(|t| t.name.clone())
                .unwrap_or_default(),
        }
    }

    /// Whether this is the current configuration or target.
    fn is_current(self, config: &BuildConfig) -> bool {
        let Some(current) = config.current() else {
            return false;
        };
        match self {
            Node::Configuration(root, i) => current.root == root && current.configuration == i,
            Node::Target(root, i) => current.root == root && current.target == i,
            _ => false,
        }
    }
}

/// Which pane has the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pane {
    Tree,
    Options,
}

/// One entry of the options pane, as laid out before the width is
/// known; the text ones wrap into as many rows as they need.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Line {
    Blank,
    Heading(String),
    Text(String),
    /// The name of the field at this index.
    Name(usize),
    Field(usize),
    /// The field's description, or the reason its text was refused.
    Note(usize),
}

/// One screen row of the options pane, after wrapping to a width.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Row {
    Blank,
    Heading(String),
    Text(String),
    Name(usize),
    Field(usize),
    /// One line of a field's note: the field, whether it is the reason
    /// its value was refused, and the line.
    Note(usize, bool, String),
}

pub struct BuildView {
    nodes: Vec<Node>,
    /// The selected row of the tree, an index into `nodes`.
    selected: usize,
    pane: Pane,
    /// The fields of the selected configuration or target, and for each
    /// why its text was refused, if it was.
    fields: Fields,
    errors: Vec<Option<String>>,
    /// The node the fields were built for, and its root's system,
    /// which says what the fields are.
    fields_node: Option<Node>,
    field_system: Option<BuildSystem>,
    /// For each field, its name and its description.
    field_info: Vec<(String, String)>,
    /// The rows of the options pane, in order.
    lines: Vec<Line>,
    tree_scroll: usize,
    options_scroll: usize,
    /// Whether the next render should scroll the tree to show the
    /// selected row, after the selection moved; otherwise the wheel is
    /// free to scroll it out of view.
    reveal_tree: bool,
    /// Likewise for the options and the focused field.
    reveal: bool,
    /// The node Delete was pressed for once, awaiting the second press.
    confirm_remove: Option<Node>,
    /// The page and its panes from the last render.
    area: Rect,
    tree_area: Rect,
    options_area: Rect,
}

impl BuildView {
    /// A page showing `config`, with the current configuration selected
    /// in the tree (or the first row when there is none).
    pub fn new(config: &BuildConfig) -> BuildView {
        let mut view = BuildView {
            nodes: Vec::new(),
            selected: 0,
            pane: Pane::Tree,
            fields: Fields::new(0),
            errors: Vec::new(),
            fields_node: None,
            field_system: None,
            field_info: Vec::new(),
            lines: Vec::new(),
            tree_scroll: 0,
            options_scroll: 0,
            reveal_tree: true,
            reveal: false,
            confirm_remove: None,
            area: Rect::default(),
            tree_area: Rect::default(),
            options_area: Rect::default(),
        };
        view.rebuild_nodes(config);
        if let Some(current) = config.current() {
            let node = Node::Configuration(current.root, current.configuration);
            if let Some(index) = view.nodes.iter().position(|n| *n == node) {
                view.selected = index;
            }
        }
        view.rebuild_fields(config);
        view
    }

    /// The selected row of the tree, if there is one.
    pub fn selected_node(&self) -> Option<Node> {
        self.nodes.get(self.selected).copied()
    }

    /// Select a row of the tree by its node, after the application
    /// changed the configuration (added a root, say).
    pub fn select(&mut self, node: Node, config: &BuildConfig) {
        self.rebuild_nodes(config);
        if let Some(index) = self.nodes.iter().position(|n| *n == node) {
            self.selected = index;
        }
        self.pane = Pane::Tree;
        self.reveal_tree = true;
        self.rebuild_fields(config);
    }

    /// Discovery brought in a root's targets (see
    /// [`BuildConfig::apply_discovery`]): the rows move, since
    /// discovered targets go ahead of the user's own, and `moved` says
    /// where each of the root's targets went by its old index, `None`
    /// for one that is gone. The selection and the fields being edited
    /// follow their target, edits and all.
    pub fn targets_moved(&mut self, root: usize, moved: &[Option<usize>], config: &BuildConfig) {
        let follow = |node: Option<Node>| match node {
            Some(Node::Target(r, i)) if r == root => {
                moved.get(i).copied().flatten().map(|i| Node::Target(r, i))
            }
            other => other,
        };
        let selected = follow(self.selected_node());
        self.fields_node = follow(self.fields_node);
        self.confirm_remove = follow(self.confirm_remove);
        self.rebuild_nodes(config);
        self.selected = selected
            .and_then(|node| self.nodes.iter().position(|n| *n == node))
            .unwrap_or(self.selected)
            .min(self.nodes.len().saturating_sub(1));
        // The fields of a configuration or target still there stay as
        // typed; anything else is shown afresh, the Targets heading's
        // note included.
        let keep = matches!(
            self.fields_node,
            Some(Node::Configuration(..) | Node::Target(..))
        ) && self.fields_node == selected;
        if !keep {
            self.rebuild_fields(config);
        }
    }

    /// Show the configuration as it now is, after the application
    /// changed it: the tree's rows and the fields' values.
    pub fn refresh(&mut self, config: &BuildConfig) {
        let node = self.selected_node();
        self.rebuild_nodes(config);
        if let Some(node) = node
            && let Some(index) = self.nodes.iter().position(|n| *n == node)
        {
            self.selected = index;
        }
        self.rebuild_fields(config);
    }

    #[cfg(test)]
    pub fn text(&self, index: usize) -> &str {
        self.fields.text(index)
    }

    #[cfg(test)]
    pub fn error(&self, index: usize) -> Option<&str> {
        self.errors[index].as_deref()
    }

    #[cfg(test)]
    pub fn in_tree(&self) -> bool {
        self.pane == Pane::Tree
    }

    // ----- The tree and the fields ----------------------------------------

    /// List the tree's rows from the configuration, keeping the selection
    /// on the same row where it still exists.
    fn rebuild_nodes(&mut self, config: &BuildConfig) {
        let selected = self.selected_node();
        self.nodes.clear();
        for (r, root) in config.roots().iter().enumerate() {
            self.nodes.push(Node::Root(r));
            self.nodes.push(Node::Configurations(r));
            for i in root.configurations_by_name() {
                self.nodes.push(Node::Configuration(r, i));
            }
            self.nodes.push(Node::Targets(r));
            for i in root.targets_by_name() {
                self.nodes.push(Node::Target(r, i));
            }
        }
        self.selected = selected
            .and_then(|node| self.nodes.iter().position(|n| *n == node))
            .unwrap_or(self.selected)
            .min(self.nodes.len().saturating_sub(1));
        if self.nodes.is_empty() {
            self.pane = Pane::Tree;
        }
    }

    /// Build the options pane for the selected row: fields for a
    /// configuration or a target, a note for anything else.
    fn rebuild_fields(&mut self, config: &BuildConfig) {
        let node = self.selected_node();
        self.fields_node = node;
        self.lines.clear();
        self.options_scroll = 0;
        // Each field's name, value, and description.
        let mut texts: Vec<(String, String, String)> = Vec::new();
        let Some(node) = node else {
            self.field_system = None;
            self.field_info.clear();
            self.fields = Fields::new(0);
            self.errors.clear();
            self.pane = Pane::Tree;
            return;
        };
        let root = config.root(node.root());
        let system = root.map(|r| r.system());
        self.field_system = system;
        match (node, system) {
            (Node::Configuration(r, i), Some(system)) => {
                self.lines
                    .push(Line::Heading(format!("{} configuration", system.name())));
                self.lines.push(Line::Blank);
                for key in system.configuration_keys() {
                    texts.push((
                        key.name().to_owned(),
                        config.configuration_text(r, i, *key),
                        key.description(system).to_owned(),
                    ));
                }
            }
            (Node::Target(r, i), Some(system)) => {
                let target = root.and_then(|r| r.targets().get(i));
                let disabled = target.is_some_and(|t| t.disabled);
                self.lines.push(Line::Heading(format!(
                    "{} target{}",
                    system.name(),
                    if disabled { " (disabled)" } else { "" }
                )));
                self.lines.push(Line::Blank);
                if let Some(found) = root.zip(target).and_then(|(r, t)| r.discovered_as(t)) {
                    self.lines.push(Line::Text(format!(
                        "Found in {} as {found}, so it follows the project and can't be removed",
                        node.root_label(config)
                    )));
                    self.lines.push(Line::Text(if disabled {
                        "Disabled: not built until Delete enables it again".to_owned()
                    } else {
                        "Delete disables it".to_owned()
                    }));
                    self.lines.push(Line::Blank);
                }
                for key in system.target_keys() {
                    texts.push((
                        key.name().to_owned(),
                        config.target_text(r, i, *key),
                        key.description(system).to_owned(),
                    ));
                }
            }
            (Node::Root(_), Some(system)) => {
                self.lines
                    .push(Line::Heading(format!("{} root", system.name())));
                self.lines.push(Line::Blank);
                self.lines.push(Line::Text(node.label(config)));
                self.lines.push(Line::Blank);
                self.lines
                    .push(Line::Text("Ctrl+N adds another root".to_owned()));
                self.lines
                    .push(Line::Text("Delete removes this one".to_owned()));
            }
            (Node::Configurations(_), _) => {
                self.lines.push(Line::Heading("Configurations".to_owned()));
                self.lines.push(Line::Blank);
                self.lines.push(Line::Text(
                    "The ways of building this root; the current one is what Ctrl+B and Ctrl+R use"
                        .to_owned(),
                ));
                self.lines.push(Line::Blank);
                self.lines
                    .push(Line::Text("Ctrl+N adds a configuration".to_owned()));
            }
            (Node::Targets(r), _) => {
                self.lines.push(Line::Heading("Targets".to_owned()));
                self.lines.push(Line::Blank);
                self.lines.push(Line::Text(
                    "What this root builds and runs; the current one is what Ctrl+B and Ctrl+R use"
                        .to_owned(),
                ));
                if config.root(r).is_some_and(|root| root.is_pending()) {
                    self.lines.push(Line::Blank);
                    self.lines.push(Line::Text(FINDING_NOTE.to_owned()));
                }
                self.lines.push(Line::Blank);
                self.lines
                    .push(Line::Text("Ctrl+N adds a target".to_owned()));
            }
            _ => {}
        }
        for index in 0..texts.len() {
            self.lines.push(Line::Name(index));
            self.lines.push(Line::Field(index));
            self.lines.push(Line::Note(index));
            self.lines.push(Line::Blank);
        }
        self.fields = Fields::new(texts.len());
        self.errors = vec![None; texts.len()];
        self.field_info = texts
            .into_iter()
            .enumerate()
            .map(|(index, (name, value, description))| {
                self.fields.set_text(index, &value);
                (name, description)
            })
            .collect();
        if self.fields.len() == 0 {
            self.pane = Pane::Tree;
        }
    }

    // ----- Applying values ------------------------------------------------

    /// Apply a field's text to its option. Text that isn't a valid value
    /// is refused and the reason kept for the note under the field.
    fn commit(&mut self, index: usize, config: &mut BuildConfig) -> BuildOutcome {
        let Some(node) = self.fields_node else {
            return BuildOutcome::Continue;
        };
        if index >= self.fields.len() {
            return BuildOutcome::Continue;
        }
        let Some(system) = config.root(node.root()).map(|r| r.system()) else {
            return BuildOutcome::Continue;
        };
        let text = self.fields.text(index).to_owned();
        let (result, renamed, applied) = match node {
            Node::Configuration(r, i) => {
                let key = system.configuration_keys()[index];
                let result = config.set_configuration_text(r, i, key, &text);
                (
                    result,
                    key == ConfigurationKey::Name,
                    config.configuration_text(r, i, key),
                )
            }
            Node::Target(r, i) => {
                let key = system.target_keys()[index];
                let result = config.set_target_text(r, i, key, &text);
                (
                    result,
                    key == TargetKey::Name,
                    config.target_text(r, i, key),
                )
            }
            _ => return BuildOutcome::Continue,
        };
        match result {
            Ok(changed) => {
                // The text as the option has it: trimmed.
                self.fields.set_text(index, &applied);
                self.errors[index] = None;
                if changed && renamed {
                    // The tree shows the name.
                    self.rebuild_nodes(config);
                }
                if changed {
                    BuildOutcome::Changed
                } else {
                    BuildOutcome::Continue
                }
            }
            Err(reason) => {
                self.errors[index] = Some(reason);
                BuildOutcome::Continue
            }
        }
    }

    /// Apply every field's text, as Ctrl+S and leaving the page do.
    pub fn commit_all(&mut self, config: &mut BuildConfig) -> BuildOutcome {
        let mut outcome = BuildOutcome::Continue;
        for index in 0..self.fields.len() {
            if self.commit(index, config) == BuildOutcome::Changed {
                outcome = BuildOutcome::Changed;
            }
        }
        outcome
    }

    /// Escape: put the focused field's text back to the option's value.
    fn revert_focused(&mut self, config: &BuildConfig) {
        let (Some(node), Some(system)) = (
            self.fields_node,
            self.fields_node
                .and_then(|n| config.root(n.root()))
                .map(|r| r.system()),
        ) else {
            return;
        };
        let index = self.fields.focused();
        if index >= self.fields.len() {
            return;
        }
        let text = match node {
            Node::Configuration(r, i) => {
                config.configuration_text(r, i, system.configuration_keys()[index])
            }
            Node::Target(r, i) => config.target_text(r, i, system.target_keys()[index]),
            _ => return,
        };
        self.fields.set_text(index, &text);
        self.errors[index] = None;
    }

    // ----- Changing the tree ----------------------------------------------

    /// Move the tree's selection, applying the fields first, and build
    /// the options for the new row.
    fn select_index(&mut self, index: usize, config: &mut BuildConfig) -> BuildOutcome {
        if self.nodes.is_empty() {
            return BuildOutcome::Continue;
        }
        let index = index.min(self.nodes.len() - 1);
        let outcome = self.commit_all(config);
        self.selected = index;
        self.reveal_tree = true;
        self.rebuild_fields(config);
        outcome
    }

    /// Ctrl+N: add to the list the selection is in, and select the new
    /// entry with its name field focused; with a root selected (or no
    /// rows at all), ask for a root.
    fn add(&mut self, config: &mut BuildConfig) -> BuildOutcome {
        let added = match self.selected_node() {
            None | Some(Node::Root(_)) => return BuildOutcome::AddRoot,
            Some(Node::Configurations(r) | Node::Configuration(r, _)) => config
                .add_configuration(r)
                .map(|i| Node::Configuration(r, i)),
            Some(Node::Targets(r) | Node::Target(r, _)) => {
                config.add_target(r).map(|i| Node::Target(r, i))
            }
        };
        match added {
            Some(node) => {
                self.commit_all(config);
                self.select(node, config);
                self.focus_options();
                BuildOutcome::Changed
            }
            None => BuildOutcome::Continue,
        }
    }

    /// Ctrl+D: duplicate the selected configuration or target and
    /// select the copy.
    fn duplicate(&mut self, config: &mut BuildConfig) -> BuildOutcome {
        let copied = match self.selected_node() {
            Some(Node::Configuration(r, i)) => config
                .duplicate_configuration(r, i)
                .map(|i| Node::Configuration(r, i)),
            Some(Node::Target(r, i)) => config.duplicate_target(r, i).map(|i| Node::Target(r, i)),
            _ => {
                return BuildOutcome::Notice(
                    "Select a configuration or a target to duplicate".to_owned(),
                );
            }
        };
        match copied {
            Some(node) => {
                self.commit_all(config);
                self.select(node, config);
                self.focus_options();
                BuildOutcome::Changed
            }
            None => BuildOutcome::Continue,
        }
    }

    /// Delete: remove the selected root, configuration, or target, on
    /// the second press.
    fn remove(&mut self, config: &mut BuildConfig) -> BuildOutcome {
        let Some(node) = self.selected_node() else {
            return BuildOutcome::Continue;
        };
        let what = match node {
            Node::Root(_) => "root",
            Node::Configuration(..) => "configuration",
            Node::Target(..) => "target",
            Node::Configurations(_) | Node::Targets(_) => {
                return BuildOutcome::Notice(
                    "Select a root, a configuration, or a target to remove".to_owned(),
                );
            }
        };
        // A discovered target is disabled rather than removed, and
        // enabled again by another press; no confirming needed, since
        // nothing is lost either way.
        if let Node::Target(r, i) = node
            && let Some(target) = config.root(r).and_then(|root| root.targets().get(i))
            && target.is_auto()
        {
            let disable = !target.disabled;
            config.set_target_disabled(r, i, disable);
            self.refresh(config);
            return BuildOutcome::Changed;
        }
        if self.confirm_remove != Some(node) {
            self.confirm_remove = Some(node);
            return BuildOutcome::Notice(format!(
                "Press Delete again to remove the {what} {}",
                node.label(config)
            ));
        }
        self.confirm_remove = None;
        match node {
            Node::Root(r) => config.remove_root(r),
            Node::Configuration(r, i) => config.remove_configuration(r, i),
            Node::Target(r, i) => {
                if let Err(reason) = config.remove_target(r, i) {
                    return BuildOutcome::Notice(reason);
                }
            }
            _ => {}
        }
        // The fields belonged to what was removed.
        self.fields_node = None;
        self.rebuild_nodes(config);
        // Stay in the same list where it still has entries, else on the
        // row that follows.
        let next = match node {
            Node::Configuration(r, i) => {
                let count = config.root(r).map_or(0, |root| root.configurations().len());
                (count > 0).then(|| Node::Configuration(r, i.min(count - 1)))
            }
            Node::Target(r, i) => {
                let count = config.root(r).map_or(0, |root| root.targets().len());
                (count > 0).then(|| Node::Target(r, i.min(count - 1)))
            }
            _ => None,
        };
        if let Some(next) = next
            && let Some(index) = self.nodes.iter().position(|n| *n == next)
        {
            self.selected = index;
        } else {
            self.selected = self.selected.min(self.nodes.len().saturating_sub(1));
        }
        self.pane = Pane::Tree;
        self.reveal_tree = true;
        self.rebuild_fields(config);
        BuildOutcome::Changed
    }

    /// Move the keyboard to the options pane, if the selection has any.
    fn focus_options(&mut self) {
        if self.fields.len() > 0 {
            self.pane = Pane::Options;
            self.fields.focus(0);
            self.reveal = true;
        }
    }

    // ----- Input ----------------------------------------------------------

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        clipboard: &mut Clipboard,
        config: &mut BuildConfig,
    ) -> BuildOutcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // A removal is confirmed only by Delete pressed twice running.
        let confirm = self.confirm_remove.take();
        match key.code {
            KeyCode::Char('n') if ctrl => return self.add(config),
            KeyCode::Char('d') if ctrl => return self.duplicate(config),
            KeyCode::Char('s') if ctrl => return self.commit_all(config),
            _ => {}
        }
        match self.pane {
            Pane::Tree => match key.code {
                KeyCode::Delete | KeyCode::Backspace => {
                    self.confirm_remove = confirm;
                    self.remove(config)
                }
                KeyCode::Up => self.select_index(self.selected.saturating_sub(1), config),
                KeyCode::Down => self.select_index(self.selected + 1, config),
                KeyCode::Home => self.select_index(0, config),
                KeyCode::End => self.select_index(usize::MAX, config),
                KeyCode::PageUp => {
                    self.select_index(self.selected.saturating_sub(self.tree_page()), config)
                }
                KeyCode::PageDown => self.select_index(self.selected + self.tree_page(), config),
                KeyCode::Enter | KeyCode::Right | KeyCode::Tab => {
                    self.focus_options();
                    BuildOutcome::Continue
                }
                _ => BuildOutcome::Continue,
            },
            Pane::Options => match key.code {
                KeyCode::Enter => self.commit(self.fields.focused(), config),
                KeyCode::Esc => {
                    self.revert_focused(config);
                    BuildOutcome::Continue
                }
                KeyCode::Up | KeyCode::BackTab if self.fields.focused() == 0 => {
                    let outcome = self.commit(0, config);
                    self.pane = Pane::Tree;
                    outcome
                }
                KeyCode::PageUp => {
                    self.options_scroll = self.options_scroll.saturating_sub(self.options_page());
                    BuildOutcome::Continue
                }
                KeyCode::PageDown => {
                    self.options_scroll += self.options_page();
                    BuildOutcome::Continue
                }
                _ => match self.fields.handle_key(key, clipboard) {
                    FieldKey::Moved { from, .. } => {
                        self.reveal = true;
                        self.commit(from, config)
                    }
                    FieldKey::Changed => {
                        self.errors[self.fields.focused()] = None;
                        BuildOutcome::Continue
                    }
                    FieldKey::Unchanged | FieldKey::Ignored => BuildOutcome::Continue,
                },
            },
        }
    }

    /// Add pasted text to the focused field.
    pub fn paste(&mut self, text: &str) {
        if self.pane == Pane::Options && self.fields.paste(text) {
            self.errors[self.fields.focused()] = None;
        }
    }

    /// Whether the screen position is over the page.
    pub fn contains(&self, x: u16, y: u16) -> bool {
        self.area.contains(ScreenPosition::new(x, y))
    }

    /// Whether a drag that started in a field is going on, in which case
    /// the page wants drag and release events wherever they happen.
    pub fn is_dragging(&self) -> bool {
        self.fields.is_dragging()
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent, config: &mut BuildConfig) -> BuildOutcome {
        let at = ScreenPosition::new(mouse.column, mouse.row);
        let in_tree = self.tree_area.contains(at);
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                if in_tree {
                    self.tree_scroll = self.tree_scroll.saturating_sub(WHEEL_LINES);
                } else {
                    self.options_scroll = self.options_scroll.saturating_sub(WHEEL_LINES);
                }
                BuildOutcome::Continue
            }
            MouseEventKind::ScrollDown => {
                if in_tree {
                    self.tree_scroll += WHEEL_LINES;
                } else {
                    self.options_scroll += WHEEL_LINES;
                }
                BuildOutcome::Continue
            }
            MouseEventKind::Down(MouseButton::Left) if in_tree => {
                self.confirm_remove = None;
                let row = self.tree_scroll + (mouse.row - self.tree_area.y) as usize;
                if row < self.nodes.len() {
                    let outcome = self.select_index(row, config);
                    self.pane = Pane::Tree;
                    return outcome;
                }
                BuildOutcome::Continue
            }
            _ => {
                let hit = self.fields.field_at(mouse.column, mouse.row).is_some();
                let outcome = match self.fields.handle_mouse(mouse) {
                    Some(from) => self.commit(from, config),
                    None => BuildOutcome::Continue,
                };
                if hit && matches!(mouse.kind, MouseEventKind::Down(_)) {
                    self.confirm_remove = None;
                    self.pane = Pane::Options;
                }
                outcome
            }
        }
    }

    fn tree_page(&self) -> usize {
        (self.tree_area.height as usize).saturating_sub(1).max(1)
    }

    fn options_page(&self) -> usize {
        (self.options_area.height as usize).saturating_sub(1).max(1)
    }

    // ----- Rendering ------------------------------------------------------

    /// Draw the page into `area`. Returns where the terminal cursor
    /// belongs, if the options have the keyboard and the focused field
    /// is on screen.
    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        config: &BuildConfig,
    ) -> Option<ScreenPosition> {
        self.area = area;
        let background = palette_background(theme);
        buf.set_style(area, background);
        if area.height == 0 || area.width < MIN_TREE_WIDTH {
            self.tree_area = Rect::default();
            self.options_area = Rect::default();
            return None;
        }
        // The tree takes a third of the width, within bounds, and the
        // options the rest past a rule; a narrow page is all tree.
        let tree_width = (area.width / 3).clamp(MIN_TREE_WIDTH, MAX_TREE_WIDTH);
        let (tree_width, options_width) = if area.width < tree_width + 1 + INDENT * 2 + 8 {
            (area.width, 0)
        } else {
            (tree_width, area.width - tree_width - 1)
        };
        self.tree_area = Rect::new(area.x, area.y, tree_width, area.height);
        self.options_area = Rect::new(area.x + tree_width + 1, area.y, options_width, area.height);

        self.render_tree(buf, theme, config);
        if options_width > 0 {
            let rule = background.fg(theme.gutter_guide);
            let x = area.x + tree_width;
            for y in area.y..area.bottom() {
                buf[(x, y)].set_symbol("│").set_style(rule);
            }
            return self.render_options(buf, theme);
        }
        self.fields.clear_layout();
        None
    }

    fn render_tree(&mut self, buf: &mut Buffer, theme: &Theme, config: &BuildConfig) {
        let area = self.tree_area;
        let background = palette_background(theme);
        let height = area.height as usize;
        let width = area.width as usize;
        if self.nodes.is_empty() {
            let dim = background.fg(theme.command_palette_result_context_text);
            buf.set_stringn(
                area.x + 1,
                area.y,
                NO_ROOTS,
                width.saturating_sub(1),
                background,
            );
            if height > 1 {
                buf.set_stringn(
                    area.x + 1,
                    area.y + 1,
                    NO_ROOTS_HINT,
                    width.saturating_sub(1),
                    dim,
                );
            }
            return;
        }
        // Scroll no further than needed to show the last row, then, when
        // the selection moved, as needed to show it.
        self.tree_scroll = self
            .tree_scroll
            .min(self.nodes.len().saturating_sub(height));
        if self.reveal_tree {
            self.reveal_tree = false;
            if self.selected < self.tree_scroll {
                self.tree_scroll = self.selected;
            } else if self.selected >= self.tree_scroll + height {
                self.tree_scroll = self.selected + 1 - height;
            }
        }
        let root_style = background
            .fg(theme.active_tab_text)
            .add_modifier(Modifier::BOLD);
        let heading = background.fg(theme.command_palette_result_context_text);
        let tag = background.fg(theme.status_bar_project_text);
        for (row, node) in self
            .nodes
            .iter()
            .enumerate()
            .skip(self.tree_scroll)
            .take(height)
        {
            let y = area.y + (row - self.tree_scroll) as u16;
            let selected = row == self.selected;
            let (style, tag_style) = if selected && self.pane == Pane::Tree {
                let style = Style::default()
                    .fg(theme.command_palette_selection_text)
                    .bg(theme.command_palette_selection_background);
                buf.set_style(Rect::new(area.x, y, area.width, 1), style);
                (style, style)
            } else if selected {
                let style = Style::default()
                    .fg(theme.unfocused_active_tab_text)
                    .bg(theme.unfocused_active_tab_background);
                buf.set_style(Rect::new(area.x, y, area.width, 1), style);
                (style, style)
            } else {
                match node {
                    Node::Root(_) => (root_style, tag),
                    Node::Configurations(_) | Node::Targets(_) => (heading, tag),
                    _ => (background, tag),
                }
            };
            let x = area.x + node.indent();
            let label = node.label(config);
            let available = width.saturating_sub(node.indent() as usize);
            let style = if node.is_disabled(config) {
                style.add_modifier(Modifier::CROSSED_OUT | Modifier::DIM)
            } else {
                style
            };
            buf.set_stringn(x, y, &label, available, style);
            if node.is_current(config) {
                let used = Span::raw(&label).width().min(available);
                let x = x + used as u16 + 2;
                if x < area.right() {
                    buf.set_stringn(x, y, CURRENT_TAG, (area.right() - x) as usize, tag_style);
                }
            }
        }
    }

    fn render_options(&mut self, buf: &mut Buffer, theme: &Theme) -> Option<ScreenPosition> {
        let area = self.options_area;
        let background = palette_background(theme);
        let height = area.height as usize;
        let width = area.width as usize;
        let field_width = area.width.saturating_sub(INDENT * 2).min(MAX_FIELD_WIDTH);
        let text_width = width.saturating_sub(INDENT as usize);

        // Lay the entries out as rows for this width: the text ones
        // wrap, a note in the error color when its value was refused.
        let rows: Vec<Row> = self
            .lines
            .iter()
            .flat_map(|line| match line {
                Line::Blank => vec![Row::Blank],
                Line::Heading(text) => wrap_words(text, text_width)
                    .into_iter()
                    .map(Row::Heading)
                    .collect(),
                Line::Text(text) => wrap_words(text, text_width)
                    .into_iter()
                    .map(Row::Text)
                    .collect(),
                Line::Name(index) => vec![Row::Name(*index)],
                Line::Field(index) => vec![Row::Field(*index)],
                Line::Note(index) => {
                    let (text, is_error) = match &self.errors[*index] {
                        Some(reason) => (reason.as_str(), true),
                        None => (self.field_info[*index].1.as_str(), false),
                    };
                    wrap_words(text, text_width)
                        .into_iter()
                        .map(|text| Row::Note(*index, is_error, text))
                        .collect()
                }
            })
            .collect();

        // Scroll no further than needed to show the last row, then as
        // needed to show the focused field with its name and note.
        self.options_scroll = self.options_scroll.min(rows.len().saturating_sub(height));
        if self.reveal {
            self.reveal = false;
            let focused = self.fields.focused();
            let first = rows.iter().position(|r| *r == Row::Name(focused));
            let last = rows
                .iter()
                .rposition(|r| matches!(r, Row::Note(index, ..) if *index == focused));
            if let (Some(first), Some(last)) = (first, last) {
                if first < self.options_scroll {
                    self.options_scroll = first;
                } else if last >= self.options_scroll + height {
                    self.options_scroll = last + 1 - height;
                }
            }
        }
        let heading = background
            .fg(theme.active_tab_text)
            .add_modifier(Modifier::BOLD);
        let name = background.add_modifier(Modifier::BOLD);
        let note = background.fg(theme.command_palette_result_context_text);
        let error = background.fg(theme.terminal_red);

        self.fields.clear_layout();
        let mut cursor = None;
        for (row, entry) in rows
            .into_iter()
            .skip(self.options_scroll)
            .take(height)
            .enumerate()
        {
            let y = area.y + row as u16;
            let x = area.x + INDENT;
            match entry {
                Row::Blank => {}
                Row::Heading(text) => {
                    buf.set_stringn(x, y, &text, text_width, heading);
                }
                Row::Text(text) => {
                    buf.set_stringn(x, y, &text, text_width, note);
                }
                Row::Name(index) => {
                    buf.set_stringn(x, y, &self.field_info[index].0, text_width, name);
                }
                Row::Field(index) => {
                    let row = Rect::new(x, y, field_width, 1);
                    let placeholder = self.placeholder(index);
                    if let Some(at) = self
                        .fields
                        .render_field(index, row, &placeholder, buf, theme)
                        && self.pane == Pane::Options
                    {
                        cursor = Some(at);
                    }
                }
                Row::Note(_, is_error, text) => {
                    let style = if is_error { error } else { note };
                    buf.set_stringn(x, y, &text, text_width, style);
                }
            }
        }
        cursor
    }

    /// What a field shows while blank.
    fn placeholder(&self, index: usize) -> String {
        let (Some(node), Some(system)) = (self.fields_node, self.field_system) else {
            return String::new();
        };
        match node {
            Node::Configuration(..) => system.configuration_keys()[index].placeholder().to_owned(),
            Node::Target(..) => system.target_keys()[index].placeholder(system).to_owned(),
            _ => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEventKind, KeyEventState};
    use ninjaedit_core::BuildRoot;
    use std::path::Path;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn press(view: &mut BuildView, config: &mut BuildConfig, code: KeyCode) -> BuildOutcome {
        view.handle_key(
            key(code, KeyModifiers::NONE),
            &mut Clipboard::local_only(),
            config,
        )
    }

    fn ctrl(view: &mut BuildView, config: &mut BuildConfig, c: char) -> BuildOutcome {
        view.handle_key(
            key(KeyCode::Char(c), KeyModifiers::CONTROL),
            &mut Clipboard::local_only(),
            config,
        )
    }

    fn type_str(view: &mut BuildView, config: &mut BuildConfig, text: &str) {
        for c in text.chars() {
            press(view, config, KeyCode::Char(c));
        }
    }

    fn draw(view: &mut BuildView, config: &BuildConfig, width: u16, height: u16) -> Vec<String> {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf, &Theme::default(), config);
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buf[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect()
    }

    fn row_with<'a>(screen: &'a [String], text: &str) -> &'a str {
        screen
            .iter()
            .find(|row| row.contains(text))
            .unwrap_or_else(|| panic!("no row with {text:?} in {screen:#?}"))
    }

    /// A project with a Cargo workspace of two packages and a nested
    /// CMake project.
    fn project() -> (tempfile::TempDir, BuildConfig) {
        let dir = tempfile::tempdir().unwrap();
        let write = |path: &str, text: &str| {
            let path = dir.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write("Cargo.toml", "[workspace]\nmembers = [\"app\", \"lib\"]\n");
        write("app/Cargo.toml", "[package]\nname = \"app\"\n");
        write("lib/Cargo.toml", "[package]\nname = \"lib\"\n");
        write("native/CMakeLists.txt", "add_executable(demo demo.c)\n");
        let config = BuildConfig::discover(dir.path());
        (dir, config)
    }

    #[test]
    fn shows_the_tree_and_the_selected_configurations_options() {
        let (_dir, config) = project();
        let mut view = BuildView::new(&config);
        let screen = draw(&mut view, &config, 100, 30);
        assert_eq!(
            screen[0].trim_start().split("  ").next(),
            Some("Cargo.toml")
        );
        assert!(screen[1].contains("Configurations"), "{screen:#?}");
        assert!(
            screen[2].contains("dev") && screen[2].contains(CURRENT_TAG),
            "{screen:#?}"
        );
        assert!(screen[3].contains("release"), "{screen:#?}");
        assert!(screen[4].contains("Targets"), "{screen:#?}");
        assert!(
            screen[5].contains("app") && screen[5].contains(CURRENT_TAG),
            "{screen:#?}"
        );
        assert!(screen[6].contains("lib"), "{screen:#?}");
        assert!(
            screen[0].contains('│'),
            "a rule between the panes: {screen:#?}"
        );
        // The options: the dev configuration's, its name field first.
        assert!(
            row_with(&screen, "Cargo configuration").contains("│"),
            "{screen:#?}"
        );
        let name = screen.iter().position(|r| r.contains("│  Name")).unwrap();
        assert!(screen[name + 1].contains("> dev"), "{screen:#?}");
        assert!(
            row_with(&screen, "Profile").contains("Profile"),
            "{screen:#?}"
        );
        assert!(
            screen.iter().any(|r| r.contains("Build arguments")),
            "{screen:#?}"
        );
        assert!(
            !screen.iter().any(|r| r.contains("Build type")),
            "not a CMake option: {screen:#?}"
        );
        assert!(view.in_tree());
    }

    #[test]
    fn arrows_move_the_selection_and_enter_goes_to_the_options() {
        let (_dir, mut config) = project();
        let mut view = BuildView::new(&config);
        assert_eq!(view.selected_node(), Some(Node::Configuration(0, 0)));
        press(&mut view, &mut config, KeyCode::Down);
        assert_eq!(view.selected_node(), Some(Node::Configuration(0, 1)));
        press(&mut view, &mut config, KeyCode::Down);
        press(&mut view, &mut config, KeyCode::Down);
        assert_eq!(view.selected_node(), Some(Node::Target(0, 0)));
        let screen = draw(&mut view, &config, 100, 30);
        assert!(
            row_with(&screen, "Cargo target").contains("target"),
            "{screen:#?}"
        );
        assert!(screen.iter().any(|r| r.contains("Package")), "{screen:#?}");
        assert_eq!(view.text(1), "app");
        press(&mut view, &mut config, KeyCode::Enter);
        assert!(!view.in_tree());
        // Up from the first field goes back to the tree; Tab moves on.
        press(&mut view, &mut config, KeyCode::Up);
        assert!(view.in_tree());
        press(&mut view, &mut config, KeyCode::Tab);
        assert!(!view.in_tree());
        press(&mut view, &mut config, KeyCode::Tab);
        ctrl(&mut view, &mut config, 'a');
        type_str(&mut view, &mut config, "other");
        assert_eq!(
            press(&mut view, &mut config, KeyCode::Enter),
            BuildOutcome::Changed
        );
        assert_eq!(config.roots()[0].targets()[0].package, "other");
        // Home and End in the tree; a root shows notes, not fields.
        press(&mut view, &mut config, KeyCode::Up);
        press(&mut view, &mut config, KeyCode::Up);
        press(&mut view, &mut config, KeyCode::Home);
        assert_eq!(view.selected_node(), Some(Node::Root(0)));
        let screen = draw(&mut view, &config, 100, 30);
        assert!(
            row_with(&screen, "Cargo root").contains("root"),
            "{screen:#?}"
        );
        assert!(
            screen.iter().any(|r| r.contains("Delete removes this one")),
            "{screen:#?}"
        );
        press(&mut view, &mut config, KeyCode::Enter);
        assert!(view.in_tree(), "nothing to focus");
        press(&mut view, &mut config, KeyCode::End);
        assert_eq!(view.selected_node(), Some(Node::Target(0, 1)));
    }

    #[test]
    fn renaming_updates_the_tree_and_bad_names_are_refused() {
        let (_dir, mut config) = project();
        let mut view = BuildView::new(&config);
        press(&mut view, &mut config, KeyCode::Enter);
        ctrl(&mut view, &mut config, 'a');
        type_str(&mut view, &mut config, "release");
        assert_eq!(
            press(&mut view, &mut config, KeyCode::Enter),
            BuildOutcome::Continue
        );
        assert_eq!(
            view.error(0),
            Some("there is already a configuration named release")
        );
        let screen = draw(&mut view, &config, 100, 30);
        assert!(
            screen.iter().any(|r| r.contains("already a configuration")),
            "{screen:#?}"
        );
        press(&mut view, &mut config, KeyCode::Esc);
        assert_eq!(view.text(0), "dev");
        assert_eq!(view.error(0), None);
        ctrl(&mut view, &mut config, 'a');
        type_str(&mut view, &mut config, " fast ");
        assert_eq!(
            press(&mut view, &mut config, KeyCode::Tab),
            BuildOutcome::Changed,
            "leaving the field applies it"
        );
        assert_eq!(view.text(0), "fast", "trimmed");
        let screen = draw(&mut view, &config, 100, 30);
        assert!(screen[2].contains("fast"), "{screen:#?}");
        assert_eq!(config.roots()[0].configurations()[0].name, "fast");
        // Leaving with bad text applies nothing for that field.
        press(&mut view, &mut config, KeyCode::Tab);
        type_str(&mut view, &mut config, "'");
        assert_eq!(view.commit_all(&mut config), BuildOutcome::Continue);
        assert!(view.error(2).is_some());
        assert_eq!(config.roots()[0].configurations()[0].build_args, "");
    }

    #[test]
    fn ctrl_n_and_ctrl_d_add_entries_and_delete_twice_removes() {
        let (_dir, mut config) = project();
        let mut view = BuildView::new(&config);
        // Ctrl+N on a configuration adds one and focuses its name.
        assert_eq!(ctrl(&mut view, &mut config, 'n'), BuildOutcome::Changed);
        assert_eq!(view.selected_node(), Some(Node::Configuration(0, 2)));
        assert!(!view.in_tree());
        assert_eq!(view.text(0), "New configuration");
        ctrl(&mut view, &mut config, 'a');
        type_str(&mut view, &mut config, "bench");
        press(&mut view, &mut config, KeyCode::Enter);
        assert_eq!(config.roots()[0].configurations()[2].name, "bench");
        // Ctrl+D copies it.
        assert_eq!(ctrl(&mut view, &mut config, 'd'), BuildOutcome::Changed);
        assert_eq!(view.selected_node(), Some(Node::Configuration(0, 3)));
        assert_eq!(view.text(0), "bench copy");
        // Ctrl+N on the targets heading adds a target. The copy sits
        // second in name order, before dev and release.
        press(&mut view, &mut config, KeyCode::Up);
        press(&mut view, &mut config, KeyCode::Down);
        assert_eq!(view.selected_node(), Some(Node::Configuration(0, 0)));
        press(&mut view, &mut config, KeyCode::Down);
        press(&mut view, &mut config, KeyCode::Down);
        assert_eq!(view.selected_node(), Some(Node::Targets(0)));
        assert_eq!(ctrl(&mut view, &mut config, 'n'), BuildOutcome::Changed);
        assert_eq!(view.selected_node(), Some(Node::Target(0, 2)));
        assert_eq!(config.roots()[0].targets()[2].name, "New target");
        // Ctrl+D on a heading is refused with a notice; on a root,
        // Ctrl+N asks for a root.
        press(&mut view, &mut config, KeyCode::Up);
        assert!(view.in_tree());
        press(&mut view, &mut config, KeyCode::Up);
        press(&mut view, &mut config, KeyCode::Up);
        press(&mut view, &mut config, KeyCode::Up);
        assert_eq!(view.selected_node(), Some(Node::Targets(0)));
        assert!(matches!(
            ctrl(&mut view, &mut config, 'd'),
            BuildOutcome::Notice(_)
        ));
        press(&mut view, &mut config, KeyCode::Home);
        assert_eq!(ctrl(&mut view, &mut config, 'n'), BuildOutcome::AddRoot);

        // Delete asks once, then removes; another key in between
        // cancels. `bench` comes first in name order.
        press(&mut view, &mut config, KeyCode::Down);
        press(&mut view, &mut config, KeyCode::Down);
        assert_eq!(view.selected_node(), Some(Node::Configuration(0, 2)));
        assert!(matches!(
            press(&mut view, &mut config, KeyCode::Delete),
            BuildOutcome::Notice(ref m) if m.contains("bench")
        ));
        press(&mut view, &mut config, KeyCode::Down);
        assert_eq!(
            press(&mut view, &mut config, KeyCode::Up),
            BuildOutcome::Continue
        );
        assert_eq!(config.roots()[0].configurations().len(), 4);
        assert!(matches!(
            press(&mut view, &mut config, KeyCode::Delete),
            BuildOutcome::Notice(_)
        ));
        assert_eq!(
            press(&mut view, &mut config, KeyCode::Delete),
            BuildOutcome::Changed
        );
        let names: Vec<&str> = config.roots()[0]
            .configurations()
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(names, vec!["dev", "release", "bench copy"]);
        assert_eq!(view.selected_node(), Some(Node::Configuration(0, 2)));
        // The user's own target goes on the second press; removing the
        // last leaves the heading selected. The discovered ones only
        // get disabled, at once, and enabled again on the next press.
        press(&mut view, &mut config, KeyCode::End);
        assert_eq!(view.selected_node(), Some(Node::Target(0, 2)));
        press(&mut view, &mut config, KeyCode::Delete);
        assert_eq!(
            press(&mut view, &mut config, KeyCode::Delete),
            BuildOutcome::Changed
        );
        assert_eq!(config.roots()[0].targets().len(), 2);
        assert_eq!(view.selected_node(), Some(Node::Target(0, 1)));
        assert_eq!(
            press(&mut view, &mut config, KeyCode::Delete),
            BuildOutcome::Changed
        );
        assert!(config.roots()[0].targets()[1].disabled);
        assert_eq!(config.roots()[0].targets().len(), 2, "not removed");
        assert_eq!(view.selected_node(), Some(Node::Target(0, 1)));
        let screen = draw(&mut view, &config, 100, 30);
        assert!(
            screen.iter().any(|r| r.contains("(disabled)")),
            "{screen:#?}"
        );
        assert!(
            screen
                .iter()
                .any(|r| r.contains("Found in Cargo.toml as the package lib")),
            "{screen:#?}"
        );
        assert_eq!(
            press(&mut view, &mut config, KeyCode::Delete),
            BuildOutcome::Changed
        );
        assert!(!config.roots()[0].targets()[1].disabled);
        press(&mut view, &mut config, KeyCode::Up);
        press(&mut view, &mut config, KeyCode::Up);
        assert_eq!(view.selected_node(), Some(Node::Targets(0)));
        // Removing the root: the page says there are none.
        press(&mut view, &mut config, KeyCode::Home);
        assert_eq!(view.selected_node(), Some(Node::Root(0)));
        press(&mut view, &mut config, KeyCode::Delete);
        assert_eq!(
            press(&mut view, &mut config, KeyCode::Delete),
            BuildOutcome::Changed
        );
        assert!(config.roots().is_empty());
        assert_eq!(view.selected_node(), None);
        let screen = draw(&mut view, &config, 60, 10);
        assert!(screen[0].contains(NO_ROOTS), "{screen:#?}");
        assert_eq!(ctrl(&mut view, &mut config, 'n'), BuildOutcome::AddRoot);
        assert_eq!(
            press(&mut view, &mut config, KeyCode::Delete),
            BuildOutcome::Continue
        );
    }

    #[test]
    fn targets_found_later_move_the_rows_but_not_the_selection_or_its_edits() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("CMakeLists.txt"),
            "add_executable(demo demo.c)\nadd_executable(tool tool.c)\n",
        )
        .unwrap();
        let mut config = BuildConfig::default();
        let root = BuildRoot::pending("CMakeLists.txt").unwrap();
        let discovery = root.discovery(dir.path());
        config.add_root(root).unwrap();
        let mine = config.add_target(0).unwrap();
        let mut view = BuildView::new(&config);
        let screen = draw(&mut view, &config, 100, 30);
        assert!(
            screen.iter().any(|r| r.contains("Targets (finding…)")),
            "{screen:#?}"
        );
        view.select(Node::Targets(0), &config);
        let screen = draw(&mut view, &config, 100, 30);
        assert!(
            screen
                .iter()
                .any(|r| r.contains("still being looked through")),
            "{screen:#?}"
        );
        // Start renaming the user's own target, without applying.
        view.select(Node::Target(0, mine), &config);
        press(&mut view, &mut config, KeyCode::Enter);
        ctrl(&mut view, &mut config, 'a');
        type_str(&mut view, &mut config, "renamed");
        assert_eq!(view.text(0), "renamed");

        // Discovery puts demo and tool ahead of it.
        let before: Vec<String> = config.roots()[0]
            .targets()
            .iter()
            .map(|t| t.name.clone())
            .collect();
        config.apply_discovery(discovery.run(None));
        let targets = config.roots()[0].targets();
        let moved: Vec<Option<usize>> = before
            .iter()
            .map(|name| targets.iter().position(|t| t.name == *name))
            .collect();
        assert_eq!(moved, vec![Some(0), Some(3)]);
        view.targets_moved(0, &moved, &config);
        assert_eq!(view.selected_node(), Some(Node::Target(0, 3)));
        assert!(!view.in_tree());
        assert_eq!(view.text(0), "renamed", "the edit is kept");
        assert_eq!(
            press(&mut view, &mut config, KeyCode::Enter),
            BuildOutcome::Changed
        );
        assert_eq!(config.roots()[0].targets()[3].name, "renamed");
        let screen = draw(&mut view, &config, 100, 30);
        assert!(!screen.iter().any(|r| r.contains("finding")), "{screen:#?}");
        assert!(screen.iter().any(|r| r.contains("demo")), "{screen:#?}");

        // With the heading selected, its note goes away with the mark.
        let root = BuildRoot::pending("CMakeLists.txt").unwrap();
        let discovery = root.discovery(dir.path());
        let mut fresh = BuildConfig::default();
        fresh.add_root(root).unwrap();
        let mut view = BuildView::new(&fresh);
        view.select(Node::Targets(0), &fresh);
        fresh.apply_discovery(discovery.run(None));
        view.targets_moved(0, &[Some(0)], &fresh);
        assert_eq!(view.selected_node(), Some(Node::Targets(0)));
        let screen = draw(&mut view, &fresh, 100, 30);
        assert!(
            !screen.iter().any(|r| r.contains("still being looked")),
            "{screen:#?}"
        );
    }

    #[test]
    fn the_application_can_select_a_node_and_refresh() {
        let (dir, mut config) = project();
        let mut view = BuildView::new(&config);
        let nested =
            BuildRoot::discover(dir.path(), Path::new("native").join("CMakeLists.txt")).unwrap();
        assert_eq!(nested.system(), BuildSystem::CMake);
        // Already there from discovery? No: discovery only finds the top
        // level, so the nested root is new.
        assert_eq!(config.roots().len(), 1);
        let index = config.add_root(nested).unwrap();
        view.select(Node::Root(index), &config);
        assert_eq!(view.selected_node(), Some(Node::Root(1)));
        let screen = draw(&mut view, &config, 100, 30);
        assert!(
            screen.iter().any(|r| r.contains("native/CMakeLists.txt")),
            "{screen:#?}"
        );
        assert!(screen.iter().any(|r| r.contains("Debug")), "{screen:#?}");
        // The current selection moves; refresh shows the tag moving.
        config.select_target(1, 1);
        view.refresh(&config);
        let screen = draw(&mut view, &config, 100, 30);
        assert!(
            row_with(&screen, "demo").contains(CURRENT_TAG),
            "{screen:#?}"
        );
        assert!(
            !row_with(&screen, " app").contains(CURRENT_TAG),
            "{screen:#?}"
        );
        // A CMake target's options.
        press(&mut view, &mut config, KeyCode::End);
        assert_eq!(view.selected_node(), Some(Node::Target(1, 1)));
        let screen = draw(&mut view, &config, 100, 30);
        assert!(
            screen.iter().any(|r| r.contains("CMake target")),
            "{screen:#?}"
        );
        assert!(
            screen.iter().any(|r| r.contains("Executable")),
            "{screen:#?}"
        );
        assert_eq!(view.text(2), "demo");
    }

    #[test]
    fn clicking_selects_rows_and_focuses_fields() {
        let (_dir, mut config) = project();
        let mut view = BuildView::new(&config);
        let screen = draw(&mut view, &config, 100, 30);
        // Click the `lib` target row.
        let row = screen.iter().position(|r| r.contains("lib")).unwrap() as u16;
        let outcome = view.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 6,
                row,
                modifiers: KeyModifiers::NONE,
            },
            &mut config,
        );
        assert_eq!(outcome, BuildOutcome::Continue);
        assert_eq!(view.selected_node(), Some(Node::Target(0, 1)));
        assert!(view.in_tree());
        let screen = draw(&mut view, &config, 100, 30);
        // Click the package field.
        let name = screen
            .iter()
            .position(|r| r.contains("│  Package"))
            .unwrap() as u16;
        let field_row = name + 1;
        assert!(screen[field_row as usize].contains("> lib"), "{screen:#?}");
        view.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 40,
                row: field_row,
                modifiers: KeyModifiers::NONE,
            },
            &mut config,
        );
        assert!(!view.in_tree());
        assert!(view.contains(40, field_row));
        // Typing goes to it, and clicking the tree applies it.
        press(&mut view, &mut config, KeyCode::End);
        type_str(&mut view, &mut config, "2");
        let outcome = view.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 6,
                row: 2,
                modifiers: KeyModifiers::NONE,
            },
            &mut config,
        );
        assert_eq!(outcome, BuildOutcome::Changed);
        assert_eq!(config.roots()[0].targets()[1].package, "lib2");
        assert_eq!(view.selected_node(), Some(Node::Configuration(0, 0)));
    }

    #[test]
    fn the_wheel_scrolls_the_tree_past_the_selection_and_arrows_bring_it_back() {
        let (_dir, mut config) = project();
        let mut view = BuildView::new(&config);
        // Seven rows on a four-row screen: the selection (dev, row 2) is
        // shown at first.
        let screen = draw(&mut view, &config, 60, 4);
        assert!(screen[0].contains("Cargo.toml"), "{screen:#?}");
        assert!(!screen.iter().any(|r| r.contains("Targets")), "{screen:#?}");
        let wheel = |view: &mut BuildView, config: &mut BuildConfig, kind| {
            view.handle_mouse(
                MouseEvent {
                    kind,
                    column: 3,
                    row: 1,
                    modifiers: KeyModifiers::NONE,
                },
                config,
            )
        };
        wheel(&mut view, &mut config, MouseEventKind::ScrollDown);
        let screen = draw(&mut view, &config, 60, 4);
        assert!(screen[0].contains("release"), "{screen:#?}");
        assert!(
            screen[3].contains("lib"),
            "the end is reachable: {screen:#?}"
        );
        let tree: Vec<&str> = screen
            .iter()
            .map(|r| r.split('│').next().unwrap())
            .collect();
        assert!(
            !tree.iter().any(|r| r.contains("dev")),
            "the selection may scroll away: {screen:#?}"
        );
        assert_eq!(
            view.selected_node(),
            Some(Node::Configuration(0, 0)),
            "without moving"
        );
        // Past the end it stops; back up it goes.
        wheel(&mut view, &mut config, MouseEventKind::ScrollDown);
        let screen = draw(&mut view, &config, 60, 4);
        assert!(screen[3].contains("lib"), "{screen:#?}");
        wheel(&mut view, &mut config, MouseEventKind::ScrollUp);
        let screen = draw(&mut view, &config, 60, 4);
        assert!(screen[0].contains("Cargo.toml"), "{screen:#?}");
        // Moving the selection off screen brings it into view again.
        wheel(&mut view, &mut config, MouseEventKind::ScrollDown);
        press(&mut view, &mut config, KeyCode::Down);
        let screen = draw(&mut view, &config, 60, 4);
        assert!(screen[0].contains("release"), "{screen:#?}");
        press(&mut view, &mut config, KeyCode::Home);
        let screen = draw(&mut view, &config, 60, 4);
        assert!(screen[0].contains("Cargo.toml"), "{screen:#?}");
    }

    #[test]
    fn the_tree_lists_configurations_and_targets_by_name() {
        let (_dir, mut config) = project();
        config.add_target(0);
        config
            .set_target_text(0, 2, TargetKey::Name, "Aardvark")
            .unwrap();
        config.add_configuration(0);
        config
            .set_configuration_text(0, 2, ConfigurationKey::Name, "bench")
            .unwrap();
        let mut view = BuildView::new(&config);
        let screen = draw(&mut view, &config, 60, 12);
        let tree: Vec<String> = screen
            .iter()
            .map(|r| r.split('│').next().unwrap().trim().to_owned())
            .collect();
        let rows: Vec<&str> = tree.iter().map(|r| r.split("  ").next().unwrap()).collect();
        assert_eq!(
            &rows[..9],
            &[
                "Cargo.toml",
                "Configurations",
                "bench",
                "dev",
                "release",
                "Targets",
                "Aardvark",
                "app",
                "lib"
            ],
            "{screen:#?}"
        );
        // The rows still stand for the right entries.
        press(&mut view, &mut config, KeyCode::Home);
        press(&mut view, &mut config, KeyCode::Down);
        press(&mut view, &mut config, KeyCode::Down);
        assert_eq!(view.selected_node(), Some(Node::Configuration(0, 2)));
        press(&mut view, &mut config, KeyCode::End);
        assert_eq!(view.selected_node(), Some(Node::Target(0, 1)));
    }

    #[test]
    fn a_disabled_target_is_crossed_out_in_the_tree() {
        let (_dir, mut config) = project();
        config.set_target_disabled(0, 1, true);
        let mut view = BuildView::new(&config);
        let area = Rect::new(0, 0, 60, 10);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf, &Theme::default(), &config);
        let row_of = |text: &str| {
            (0..10)
                .find(|y| {
                    (0..60)
                        .map(|x| buf[(x, *y)].symbol().to_owned())
                        .collect::<String>()
                        .contains(text)
                })
                .unwrap()
        };
        let lib = row_of("lib");
        let app = row_of("app");
        assert!(buf[(6, lib)].modifier.contains(Modifier::CROSSED_OUT));
        assert!(!buf[(6, app)].modifier.contains(Modifier::CROSSED_OUT));
    }

    #[test]
    fn notes_wrap_to_the_pane_and_the_focused_field_is_revealed_whole() {
        let (_dir, mut config) = project();
        let mut view = BuildView::new(&config);
        // Wide: the profile's description is one row.
        let screen = draw(&mut view, &config, 120, 30);
        let profile = screen
            .iter()
            .position(|r| r.contains("│  Profile"))
            .unwrap();
        assert!(
            screen[profile + 2].contains(
                "The Cargo profile to build with: dev, release, or one defined in Cargo.toml"
            ),
            "{screen:#?}"
        );
        assert!(
            screen[profile + 3].trim_end().ends_with('│'),
            "blank after: {screen:#?}"
        );
        // Narrow: it wraps at a space onto more rows, with nothing lost.
        let screen = draw(&mut view, &config, 60, 30);
        let profile = screen
            .iter()
            .position(|r| r.contains("│  Profile"))
            .unwrap();
        let note: Vec<&str> = screen[profile + 2..]
            .iter()
            .map(|r| r.split('│').nth(1).unwrap().trim())
            .take_while(|r| !r.is_empty())
            .collect();
        assert!(note.len() >= 2, "{screen:#?}");
        assert_eq!(
            note.join(" "),
            "The Cargo profile to build with: dev, release, or one defined in Cargo.toml"
        );
        assert!(note.iter().all(|r| r.len() <= 60 - 22 - 1 - 2), "{note:?}");
        // A refused value's reason wraps too, and moving to a field
        // scrolls so its whole note shows.
        press(&mut view, &mut config, KeyCode::Enter);
        for _ in 0..3 {
            press(&mut view, &mut config, KeyCode::Tab);
        }
        ctrl(&mut view, &mut config, 'a');
        type_str(&mut view, &mut config, "NOT_A_PAIR");
        press(&mut view, &mut config, KeyCode::Enter);
        let screen = draw(&mut view, &config, 60, 8);
        assert!(
            screen.iter().any(|r| r.contains("NOT_A_PAIR should be")),
            "{screen:#?}"
        );
        assert!(
            screen.iter().any(|r| r.contains("NAME=value")),
            "the whole reason is on screen: {screen:#?}"
        );
    }

    #[test]
    fn a_short_screen_scrolls_to_the_focused_field() {
        let (_dir, mut config) = project();
        let mut view = BuildView::new(&config);
        press(&mut view, &mut config, KeyCode::Enter);
        for _ in 0..3 {
            press(&mut view, &mut config, KeyCode::Tab);
        }
        let screen = draw(&mut view, &config, 80, 6);
        assert!(
            screen.iter().any(|r| r.contains("Environment")),
            "{screen:#?}"
        );
        assert!(!screen.iter().any(|r| r.contains("│  Name")), "{screen:#?}");
        // A narrow screen is all tree.
        let screen = draw(&mut view, &config, 24, 6);
        assert!(screen[0].contains("Cargo.toml"), "{screen:#?}");
        assert!(!screen.iter().any(|r| r.contains('│')), "{screen:#?}");
    }
}
