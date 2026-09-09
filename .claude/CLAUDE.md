# CLAUDE.md

## Goals

The project goal is to build a terminal-based IDE with a focus on fast navigation of
large projects, management of git workflows with multiple branches, diff viewing
for code reviews, and quick access to builds of multiple configurations.

The project is written in Rust and the TUI interface uses ratatui for cross platform
terminal rendering.

## Project layout

* `core` is the logic that backs the editor, including things like project management,
  indexing, searching, git actions, and data structures for file buffers
* `tui` contains the TUI interface for the editor

There may be more frontends later in the project, so a model/view architecture should
be used. For example, the text editor contains an editor model in `core` that contains
the various actions that a text editor should perform in a frontend-agnostic way.
The `tui` crate uses that model to display the text and hands off incoming user
actions to the appropriate model update functions.

