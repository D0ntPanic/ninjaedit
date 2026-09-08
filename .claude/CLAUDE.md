# CLAUDE.md

## Goals

The project goal is to build a terminal-based IDE with a focus on fast navigation of
large projects, management of git workflows with multiple branches, diff viewing
for code reviews, and quick access to builds of multiple configurations.

The project is written in Rust and the TUI interface uses ratatui for cross platform
terminal rendering.

## Project layout

* Root of the repository is the TUI interface for the editor
* `core` is the logic that backs the editor, including things like project management,
  indexing, searching, git actions, and data structures for file buffers

