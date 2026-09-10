//! Interface colors, loaded from a theme file.
//!
//! A theme is a TOML document. Its top-level keys name the parts of the
//! interface (`view-background`, `active-tab-text`, ...) and each is given
//! a color, either as `#rrggbb` or as the name of an entry in the `[names]`
//! table:
//!
//! ```toml
//! view-background = "mid"
//! view-text = "#e0e0e0"
//!
//! [names]
//! mid = "#2a2a2a"
//! ```
//!
//! The default theme at `theme/default.theme` is compiled into the binary,
//! so there is always a valid theme. A theme file only has to give the
//! colors it wants to change; anything it leaves out keeps the default.
//! Unknown keys are an error, since they are almost always typos.
//!
//! A few colors are optional, such as `selection-text`: leaving one out
//! (or setting it to `""`, to undo the default) means the interface falls
//! back to a related color instead.
//!
//! Syntax highlighting colors are the `syntax-` keys, one per
//! [`TokenKind`] (`syntax-keyword`, `syntax-doc-comment`, ...). They may
//! be bold or italic: a `*` before the color makes it bold and a `/`
//! makes it italic, in either order, on a name or an RGB value alike:
//!
//! ```toml
//! syntax-comment = "/dimmed"
//! syntax-function-definition = "*#e0e0e0"
//! syntax-type-definition = "*/cyan"
//! ```
//!
//! Every syntax key is optional. A kind that isn't styled borrows the
//! style of its parent kind (a doc comment is a comment, a primitive type
//! is a type; see [`TokenKind::parent`]), and at the root falls back to
//! `view-text`.

use ninjaedit_core::TokenKind;
use ratatui::style::{Color, Modifier, Style};
use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::sync::LazyLock;

/// A color with optional bold and italic, for text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextStyle {
    pub color: Color,
    pub bold: bool,
    pub italic: bool,
}

impl TextStyle {
    pub const fn plain(color: Color) -> TextStyle {
        TextStyle {
            color,
            bold: false,
            italic: false,
        }
    }

    /// Apply this style's foreground color and modifiers to `base`.
    pub fn apply(self, base: Style) -> Style {
        let mut style = base.fg(self.color);
        if self.bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        if self.italic {
            style = style.add_modifier(Modifier::ITALIC);
        }
        style
    }
}

const DEFAULT_THEME: &str = include_str!("../../theme/default.theme");

/// Declares the [`Theme`] fields alongside the theme file keys they are
/// read from, so the two can't drift apart.
macro_rules! theme_colors {
    (
        required { $($(#[$doc:meta])* $field:ident => $key:literal),* $(,)? }
        optional { $($(#[$odoc:meta])* $ofield:ident => $okey:literal),* $(,)? }
    ) => {
        /// The colors of every part of the interface.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct Theme {
            $($(#[$doc])* pub $field: Color,)*
            $($(#[$odoc])* pub $ofield: Option<Color>,)*
            /// Syntax styles by [`TokenKind::index`]; `None` where the
            /// theme leaves a kind to its parent.
            syntax: [Option<TextStyle>; TokenKind::COUNT],
        }

        impl Theme {
            /// Every key a theme file must set to be complete on its own.
            #[cfg(test)]
            const REQUIRED_KEYS: &'static [&'static str] = &[$($key),*];

            /// A theme with every required color set to `color` and no
            /// optional ones.
            fn uniform(color: Color) -> Theme {
                Theme {
                    $($field: color,)*
                    $($ofield: None,)*
                    syntax: [None; TokenKind::COUNT],
                }
            }

            /// Set (or, with `None`, unset) the style for a theme file key.
            fn set(&mut self, key: &str, style: Option<TextStyle>) -> Set {
                if let Some(kind) = syntax_kind(key) {
                    self.syntax[kind.index()] = style;
                    return Set::Ok;
                }
                if style.is_some_and(|s| s.bold || s.italic) {
                    return Set::NotText;
                }
                let color = style.map(|s| s.color);
                match key {
                    $($key => match color {
                        Some(color) => self.$field = color,
                        None => return Set::Required,
                    },)*
                    $($okey => self.$ofield = color,)*
                    _ => return Set::UnknownKey,
                }
                Set::Ok
            }
        }
    };
}

/// The outcome of [`Theme::set`].
enum Set {
    Ok,
    UnknownKey,
    /// The key can't be unset.
    Required,
    /// The key is an interface color, which can't be bold or italic.
    NotText,
}

/// The token kind a `syntax-` key styles.
fn syntax_kind(key: &str) -> Option<TokenKind> {
    let name = key.strip_prefix("syntax-")?;
    TokenKind::ALL.into_iter().find(|kind| kind.name() == name)
}

impl Theme {
    /// The style for a kind of token, following parents for kinds the
    /// theme doesn't style and ending at `view-text`.
    pub fn syntax(&self, kind: TokenKind) -> TextStyle {
        let mut current = Some(kind);
        while let Some(kind) = current {
            if let Some(style) = self.syntax[kind.index()] {
                return style;
            }
            current = kind.parent();
        }
        TextStyle::plain(self.view_text)
    }
}

theme_colors! {
required {
    view_background => "view-background",
    view_text => "view-text",
    /// The background of selected text in the editor.
    selection_background => "selection-background",
    inactive_line_number => "inactive-line-number",
    /// The number of the line the cursor is on.
    active_line_number => "active-line-number",
    /// The vertical guide between the gutter and the text.
    gutter_guide => "gutter-guide",
    inactive_tab_background => "inactive-tab-background",
    active_tab_background => "active-tab-background",
    inactive_tab_text => "inactive-tab-text",
    active_tab_text => "active-tab-text",
    inactive_tab_close => "inactive-tab-close",
    active_tab_close => "active-tab-close",
    scroll_bar_track => "scroll-bar-track",
    /// The scrollbar thumb.
    scroll_bar_color => "scroll-bar-color",
    status_bar_background => "status-bar-background",
    status_bar_filename_text => "status-bar-filename-text",
    status_bar_position_text => "status-bar-position-text",
    status_bar_project_text => "status-bar-project-text",
    command_palette_background => "command-palette-background",
    /// The border around the palette.
    command_palette_box_color => "command-palette-box-color",
    command_palette_placeholder_text => "command-palette-placeholder-text",
    command_palette_input_background => "command-palette-input-background",
    command_palette_input_text => "command-palette-input-text",
    command_palette_result_text => "command-palette-result-text",
    /// The secondary text of a result, such as a file's directory.
    command_palette_result_context_text => "command-palette-result-context-text",
    command_palette_selection_background => "command-palette-selection-background",
    command_palette_selection_text => "command-palette-selection-text",
}
optional {
    /// Selected text in the editor. When set it replaces whatever color
    /// the text would otherwise have (syntax highlighting included) so
    /// the selection stays readable; when unset the text keeps its color
    /// and the theme's content colors must work over the selection
    /// background.
    selection_text => "selection-text",
}
}

/// Why a theme could not be loaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThemeError(String);

impl fmt::Display for ThemeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ThemeError {}

static DEFAULT: LazyLock<Theme> = LazyLock::new(|| {
    Theme::parse_over(DEFAULT_THEME, Theme::uniform(Color::Reset))
        .expect("the embedded default theme is valid")
});

impl Default for Theme {
    /// The theme compiled into the binary.
    fn default() -> Theme {
        *DEFAULT
    }
}

impl Theme {
    /// Parse a theme file. Colors it doesn't set come from the default
    /// theme.
    pub fn parse(text: &str) -> Result<Theme, ThemeError> {
        Theme::parse_over(text, Theme::default())
    }

    /// Read and parse a theme file.
    pub fn load(path: &Path) -> Result<Theme, ThemeError> {
        let text = std::fs::read_to_string(path)
            .map_err(|err| ThemeError(format!("could not read {}: {err}", path.display())))?;
        Theme::parse(&text)
    }

    /// Parse a theme file, taking any colors it doesn't set from `base`.
    fn parse_over(text: &str, base: Theme) -> Result<Theme, ThemeError> {
        let table: toml::Table = text.parse().map_err(|err: toml::de::Error| {
            ThemeError(format!("invalid theme: {}", err.message()))
        })?;

        let mut names = HashMap::new();
        if let Some(value) = table.get("names") {
            let entries = value
                .as_table()
                .ok_or_else(|| ThemeError("`names` must be a table of colors".to_owned()))?;
            for (name, value) in entries {
                let text = value
                    .as_str()
                    .ok_or_else(|| ThemeError(format!("named color `{name}` must be a string")))?;
                let color = parse_rgb(text).ok_or_else(|| {
                    ThemeError(format!(
                        "named color `{name}` must be an RGB color like #1a2b3c, not `{text}`"
                    ))
                })?;
                names.insert(name.as_str(), color);
            }
        }

        let mut theme = base;
        for (key, value) in &table {
            if key == "names" {
                continue;
            }
            let text = value
                .as_str()
                .ok_or_else(|| ThemeError(format!("`{key}` must be a color string")))?;
            let style = if text.is_empty() {
                None
            } else {
                let (bold, italic, color_text) = split_modifiers(text);
                match parse_rgb(color_text).or_else(|| names.get(color_text).copied()) {
                    Some(color) => Some(TextStyle {
                        color,
                        bold,
                        italic,
                    }),
                    None => {
                        return Err(ThemeError(format!(
                            "`{key}` is `{text}`, which is neither an RGB color like #1a2b3c nor a named color"
                        )));
                    }
                }
            };
            match theme.set(key, style) {
                Set::Ok => {}
                Set::UnknownKey => {
                    return Err(ThemeError(format!("`{key}` is not a theme color")));
                }
                Set::Required => {
                    return Err(ThemeError(format!(
                        "`{key}` is not optional and can't be empty"
                    )));
                }
                Set::NotText => {
                    return Err(ThemeError(format!(
                        "`{key}` is an interface color and can't be bold or italic; only `syntax-` colors can"
                    )));
                }
            }
        }
        Ok(theme)
    }
}

/// Split the `*` (bold) and `/` (italic) prefixes off a color.
fn split_modifiers(text: &str) -> (bool, bool, &str) {
    let mut bold = false;
    let mut italic = false;
    let mut rest = text;
    loop {
        if let Some(r) = rest.strip_prefix('*') {
            bold = true;
            rest = r;
        } else if let Some(r) = rest.strip_prefix('/') {
            italic = true;
            rest = r;
        } else {
            return (bold, italic, rest);
        }
    }
}

/// Parse `#rrggbb`.
fn parse_rgb(text: &str) -> Option<Color> {
    let hex = text.strip_prefix('#')?;
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some(Color::Rgb(channel(0)?, channel(2)?, channel(4)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_theme_sets_every_color() {
        let table: toml::Table = DEFAULT_THEME.parse().unwrap();
        for key in Theme::REQUIRED_KEYS {
            assert!(table.contains_key(*key), "default theme is missing `{key}`");
        }
        // Every required color resolved to a real color rather than the
        // `Reset` the default is parsed over.
        let theme = Theme::default();
        assert_ne!(theme, Theme::uniform(Color::Reset));
        let text = format!("{theme:?}");
        assert!(!text.contains("Reset"), "{text}");
        assert_eq!(theme.selection_text, None);
    }

    #[test]
    fn optional_colors() {
        let theme = Theme::parse("selection-text = \"#010203\"\n").unwrap();
        assert_eq!(theme.selection_text, Some(Color::Rgb(1, 2, 3)));
        let base = "selection-text = \"#010203\"\n";
        let theme = Theme::parse_over("selection-text = \"\"\n", Theme::parse(base).unwrap());
        assert_eq!(theme.unwrap().selection_text, None);
        let error = Theme::parse("view-text = \"\"\n").unwrap_err().to_string();
        assert!(error.contains("not optional"), "{error}");
    }

    #[test]
    fn rgb_and_named_colors() {
        let theme = Theme::parse(
            "view-text = \"#FF0080\"\nview-background = \"bg\"\n\n[names]\nbg = \"#010203\"\n",
        )
        .unwrap();
        assert_eq!(theme.view_text, Color::Rgb(0xff, 0x00, 0x80));
        assert_eq!(theme.view_background, Color::Rgb(1, 2, 3));
    }

    #[test]
    fn unset_colors_keep_the_default() {
        let theme = Theme::parse("view-text = \"#ffffff\"\n").unwrap();
        let default = Theme::default();
        assert_eq!(theme.view_text, Color::Rgb(0xff, 0xff, 0xff));
        assert_eq!(theme.view_background, default.view_background);
        assert_eq!(theme.active_tab_text, default.active_tab_text);
    }

    #[test]
    fn empty_theme_is_the_default() {
        assert_eq!(Theme::parse("").unwrap(), Theme::default());
    }

    #[test]
    fn rejects_bad_input() {
        let error = |text: &str| Theme::parse(text).unwrap_err().to_string();
        assert!(error("view-text = \"nope\"").contains("nor a named color"));
        assert!(error("view-txt = \"#000000\"").contains("not a theme color"));
        assert!(error("view-text = \"#12345\"").contains("nor a named color"));
        assert!(error("view-text = \"#gggggg\"").contains("nor a named color"));
        assert!(error("view-text = 7").contains("must be a color string"));
        assert!(error("[names]\nbad = \"red\"").contains("must be an RGB color"));
        assert!(error("names = 3").contains("must be a table"));
        assert!(error("view-text = ").contains("invalid theme"));
    }

    #[test]
    fn syntax_styles_and_modifiers() {
        // Over a blank base, so the default theme's own syntax colors
        // don't get in the way of checking fallbacks.
        let theme = Theme::parse_over(
            "syntax-comment = \"/#010203\"\nsyntax-keyword = \"*/blue\"\nsyntax-type = \"/*#040506\"\n\n[names]\nblue = \"#0000ff\"\n",
            Theme::uniform(Color::Rgb(9, 9, 9)),
        )
        .unwrap();
        assert_eq!(
            theme.syntax(TokenKind::Comment),
            TextStyle {
                color: Color::Rgb(1, 2, 3),
                bold: false,
                italic: true
            }
        );
        assert_eq!(
            theme.syntax(TokenKind::Keyword),
            TextStyle {
                color: Color::Rgb(0, 0, 0xff),
                bold: true,
                italic: true
            }
        );
        assert!(theme.syntax(TokenKind::Type).bold && theme.syntax(TokenKind::Type).italic);
        // Unstyled kinds follow their parents; doc comments are comments.
        assert_eq!(
            theme.syntax(TokenKind::DocComment),
            theme.syntax(TokenKind::Comment)
        );
        // Unsetting a kind in a derived theme returns it to its parent.
        let derived = Theme::parse_over("syntax-doc-comment = \"\"\n", theme).unwrap();
        assert_eq!(
            derived.syntax(TokenKind::DocComment),
            theme.syntax(TokenKind::Comment)
        );
        // Root kinds fall back to the view text color.
        assert_eq!(
            theme.syntax(TokenKind::Text),
            TextStyle::plain(Color::Rgb(9, 9, 9))
        );
        assert_eq!(
            theme.syntax(TokenKind::Function),
            TextStyle::plain(Color::Rgb(9, 9, 9))
        );

        let style = TextStyle {
            color: Color::Rgb(1, 2, 3),
            bold: true,
            italic: false,
        }
        .apply(Style::default().bg(Color::Black));
        assert_eq!(style.fg, Some(Color::Rgb(1, 2, 3)));
        assert_eq!(style.bg, Some(Color::Black));
        assert!(style.add_modifier.contains(Modifier::BOLD));
        assert!(!style.add_modifier.contains(Modifier::ITALIC));

        let error = |text: &str| Theme::parse(text).unwrap_err().to_string();
        assert!(error("view-text = \"*#000000\"").contains("can't be bold or italic"));
        assert!(error("syntax-nope = \"#000000\"").contains("not a theme color"));
        assert!(error("syntax-comment = \"*\"").contains("nor a named color"));
    }

    #[test]
    fn default_theme_styles_every_root_kind() {
        let theme = Theme::default();
        for kind in TokenKind::ALL {
            if kind.parent().is_none() && kind != TokenKind::Text && kind != TokenKind::Identifier {
                assert!(
                    theme.syntax[kind.index()].is_some(),
                    "default theme lacks syntax-{}",
                    kind.name()
                );
            }
        }
    }

    #[test]
    fn parse_rgb_forms() {
        assert_eq!(parse_rgb("#000000"), Some(Color::Rgb(0, 0, 0)));
        assert_eq!(parse_rgb("#AbCdEf"), Some(Color::Rgb(0xab, 0xcd, 0xef)));
        assert_eq!(parse_rgb("000000"), None);
        assert_eq!(parse_rgb("#fff"), None);
        assert_eq!(parse_rgb("#ffffff0"), None);
        assert_eq!(parse_rgb("#ffffé"), None);
    }
}
