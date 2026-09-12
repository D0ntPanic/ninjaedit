//! What one cell of a terminal screen holds: its text and the attributes
//! set by SGR escape sequences.
//!
//! A cell's text is one grapheme cluster, so a base character and its
//! combining marks live together, as they do in the editor. A character
//! two columns wide takes two cells: the first holds the text and is
//! [`Cell::wide`], and the second is a [`Cell::spacer`] with no text of
//! its own. A frontend skips spacers when drawing.
//!
//! Colors are kept as the program asked for them ([`Color::Indexed`] for
//! the 256-color palette, [`Color::Rgb`] for direct color) so that a
//! frontend can map the palette through its theme.

use compact_str::CompactString;

/// A terminal color as an escape sequence names it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Color {
    /// The default foreground or background, whatever the frontend makes
    /// it.
    #[default]
    Default,
    /// One of the 256 palette colors: 0-7 the standard colors, 8-15 their
    /// bright versions, 16-231 a 6x6x6 color cube, 232-255 a grey ramp.
    Indexed(u8),
    Rgb(u8, u8, u8),
}

impl Color {
    /// The sRGB value of a palette entry from 16 up, which every terminal
    /// agrees on. The first sixteen are the frontend's to choose, so this
    /// gives `None` for them (and for the default color).
    pub fn fixed_rgb(self) -> Option<(u8, u8, u8)> {
        match self {
            Color::Rgb(r, g, b) => Some((r, g, b)),
            Color::Indexed(i) if i >= 232 => {
                let level = 8 + 10 * (i - 232);
                Some((level, level, level))
            }
            Color::Indexed(i) if i >= 16 => {
                let i = i - 16;
                let channel = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
                Some((channel(i / 36), channel(i / 6 % 6), channel(i % 6)))
            }
            _ => None,
        }
    }
}

/// The kinds of underline SGR 4 can ask for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Underline {
    #[default]
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

/// The attributes a cell is drawn with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Style {
    pub fg: Color,
    pub bg: Color,
    /// The underline color, when set separately from the text (SGR 58).
    pub underline_color: Color,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: Underline,
    pub blink: bool,
    /// Foreground and background swapped.
    pub inverse: bool,
    /// Drawn as blank.
    pub hidden: bool,
    pub strikethrough: bool,
}

/// One cell of the screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    /// The grapheme cluster shown, or a single space when the cell is
    /// blank. Empty for a spacer.
    pub text: CompactString,
    pub style: Style,
    /// Whether the text is two columns wide, spilling into the next cell.
    pub wide: bool,
    /// Whether this is the second column of a wide character.
    pub spacer: bool,
}

impl Default for Cell {
    fn default() -> Cell {
        Cell::blank(Style::default())
    }
}

impl Cell {
    /// A blank cell: a space in a style. An erase keeps the current
    /// background color, so blank cells carry a style too.
    pub fn blank(style: Style) -> Cell {
        Cell {
            text: CompactString::const_new(" "),
            style,
            wide: false,
            spacer: false,
        }
    }

    /// The second column of a wide character.
    pub fn spacer(style: Style) -> Cell {
        Cell {
            text: CompactString::const_new(""),
            style,
            wide: false,
            spacer: true,
        }
    }

    /// Whether the cell is a space in the default style: what trimming
    /// from the end of a row can drop without changing what's shown.
    pub fn is_default(&self) -> bool {
        !self.wide && !self.spacer && self.text == " " && self.style == Style::default()
    }

    /// The columns the cell takes: two when wide, none for a spacer.
    pub fn width(&self) -> usize {
        if self.spacer {
            0
        } else if self.wide {
            2
        } else {
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_colors() {
        assert_eq!(Color::Default.fixed_rgb(), None);
        assert_eq!(Color::Indexed(1).fixed_rgb(), None);
        assert_eq!(Color::Indexed(15).fixed_rgb(), None);
        assert_eq!(Color::Indexed(16).fixed_rgb(), Some((0, 0, 0)));
        assert_eq!(Color::Indexed(21).fixed_rgb(), Some((0, 0, 255)));
        assert_eq!(Color::Indexed(196).fixed_rgb(), Some((255, 0, 0)));
        assert_eq!(Color::Indexed(231).fixed_rgb(), Some((255, 255, 255)));
        assert_eq!(Color::Indexed(232).fixed_rgb(), Some((8, 8, 8)));
        assert_eq!(Color::Indexed(255).fixed_rgb(), Some((238, 238, 238)));
        assert_eq!(Color::Rgb(1, 2, 3).fixed_rgb(), Some((1, 2, 3)));
    }

    #[test]
    fn cell_widths() {
        let blank = Cell::default();
        assert!(blank.is_default());
        assert_eq!(blank.width(), 1);
        let spacer = Cell::spacer(Style::default());
        assert_eq!(spacer.width(), 0);
        assert!(!spacer.is_default());
        let mut wide = Cell::default();
        wide.text = "한".into();
        wide.wide = true;
        assert_eq!(wide.width(), 2);
    }
}
