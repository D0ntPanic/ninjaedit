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

use ratatui::style::Color;
use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::sync::LazyLock;

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
                }
            }

            /// Set (or, with `None`, unset) the color for a theme file key.
            fn set(&mut self, key: &str, color: Option<Color>) -> Set {
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
            let color = if text.is_empty() {
                None
            } else {
                match parse_rgb(text).or_else(|| names.get(text).copied()) {
                    Some(color) => Some(color),
                    None => {
                        return Err(ThemeError(format!(
                            "`{key}` is `{text}`, which is neither an RGB color like #1a2b3c nor a named color"
                        )));
                    }
                }
            };
            match theme.set(key, color) {
                Set::Ok => {}
                Set::UnknownKey => {
                    return Err(ThemeError(format!("`{key}` is not a theme color")));
                }
                Set::Required => {
                    return Err(ThemeError(format!(
                        "`{key}` is not optional and can't be empty"
                    )));
                }
            }
        }
        Ok(theme)
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
    fn parse_rgb_forms() {
        assert_eq!(parse_rgb("#000000"), Some(Color::Rgb(0, 0, 0)));
        assert_eq!(parse_rgb("#AbCdEf"), Some(Color::Rgb(0xab, 0xcd, 0xef)));
        assert_eq!(parse_rgb("000000"), None);
        assert_eq!(parse_rgb("#fff"), None);
        assert_eq!(parse_rgb("#ffffff0"), None);
        assert_eq!(parse_rgb("#ffffé"), None);
    }
}
