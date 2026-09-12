//! Turning what the user does into the bytes a program in the terminal
//! reads: key presses, mouse events, pasted text.
//!
//! The encoding is xterm's, which is what every program expects of a
//! terminal calling itself `xterm-256color`: control characters for
//! Ctrl+letter, an escape prefix for Alt, CSI sequences for the special
//! keys with the modifier state as a parameter, and SGR (or, if the
//! program didn't ask for that, X10) mouse reports.
//!
//! Keys are described with the frontend-neutral [`Key`] and [`Modifiers`]
//! types; a frontend maps its own events onto them.

use super::emulator::MouseMode;

/// A key the user pressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// A character, as typed (so with Shift already applied).
    Char(char),
    Enter,
    Tab,
    Backspace,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    /// A function key, F1 being 1.
    F(u8),
}

/// The modifier keys held with a key or mouse button.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

impl Modifiers {
    /// The xterm modifier parameter: 1 plus the bits for Shift, Alt, and
    /// Ctrl, or 1 with nothing held.
    fn parameter(self) -> u8 {
        1 + u8::from(self.shift) + 2 * u8::from(self.alt) + 4 * u8::from(self.ctrl)
    }

    fn any(self) -> bool {
        self.shift || self.alt || self.ctrl
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseEventKind {
    Press(MouseButton),
    Release(MouseButton),
    /// Motion with a button held.
    Drag(MouseButton),
    /// Motion with no button held.
    Move,
    ScrollUp,
    ScrollDown,
    ScrollLeft,
    ScrollRight,
}

/// A mouse event at a cell of the terminal, counting from zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MouseEvent {
    pub kind: MouseEventKind,
    pub col: usize,
    pub row: usize,
    pub modifiers: Modifiers,
}

/// The bytes for a key press. `application_cursor_keys` is DECCKM, which
/// changes what the arrow and Home/End keys send.
pub(crate) fn encode_key(
    key: Key,
    modifiers: Modifiers,
    application_cursor_keys: bool,
) -> Option<Vec<u8>> {
    let m = modifiers.parameter();
    let mut out = Vec::new();
    // Keys with a single byte, which Alt turns into an escape sequence.
    let plain: Option<Vec<u8>> = match key {
        Key::Char(c) => {
            let control = if modifiers.ctrl {
                control_byte(c)
            } else {
                None
            };
            Some(match control {
                Some(byte) => vec![byte],
                None => c.to_string().into_bytes(),
            })
        }
        Key::Enter => Some(b"\r".to_vec()),
        Key::Tab if modifiers.shift => return Some(b"\x1b[Z".to_vec()),
        Key::Tab => Some(b"\t".to_vec()),
        Key::Backspace if modifiers.ctrl => Some(b"\x08".to_vec()),
        Key::Backspace => Some(b"\x7f".to_vec()),
        Key::Escape => Some(b"\x1b".to_vec()),
        _ => None,
    };
    if let Some(bytes) = plain {
        if modifiers.alt {
            out.push(0x1b);
        }
        out.extend(bytes);
        return Some(out);
    }

    // Cursor keys: SS3 in application mode, CSI otherwise, and always CSI
    // with a modifier parameter.
    let cursor = match key {
        Key::Up => Some(b'A'),
        Key::Down => Some(b'B'),
        Key::Right => Some(b'C'),
        Key::Left => Some(b'D'),
        Key::Home => Some(b'H'),
        Key::End => Some(b'F'),
        Key::F(1) => Some(b'P'),
        Key::F(2) => Some(b'Q'),
        Key::F(3) => Some(b'R'),
        Key::F(4) => Some(b'S'),
        _ => None,
    };
    if let Some(letter) = cursor {
        let is_function = matches!(key, Key::F(_));
        if modifiers.any() {
            out.extend(format!("\x1b[1;{m}").into_bytes());
        } else if application_cursor_keys || is_function {
            out.extend(b"\x1bO");
        } else {
            out.extend(b"\x1b[");
        }
        out.push(letter);
        return Some(out);
    }

    // Keys with a numbered CSI ~ sequence.
    let number = match key {
        Key::Insert => 2,
        Key::Delete => 3,
        Key::PageUp => 5,
        Key::PageDown => 6,
        Key::F(5) => 15,
        Key::F(6) => 17,
        Key::F(7) => 18,
        Key::F(8) => 19,
        Key::F(9) => 20,
        Key::F(10) => 21,
        Key::F(11) => 23,
        Key::F(12) => 24,
        _ => return None,
    };
    if modifiers.any() {
        out.extend(format!("\x1b[{number};{m}~").into_bytes());
    } else {
        out.extend(format!("\x1b[{number}~").into_bytes());
    }
    Some(out)
}

/// The control character for Ctrl and a key, if there is one.
fn control_byte(c: char) -> Option<u8> {
    let c = c.to_ascii_lowercase();
    Some(match c {
        'a'..='z' => c as u8 - b'a' + 1,
        ' ' | '@' | '2' => 0,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '7' | '-' => 0x1f,
        '?' | '8' => 0x7f,
        _ => return None,
    })
}

/// The bytes for a mouse event, or `None` when the program's mouse mode
/// doesn't cover it.
pub(crate) fn encode_mouse(event: MouseEvent, mode: MouseMode, sgr: bool) -> Option<Vec<u8>> {
    let wanted = match event.kind {
        MouseEventKind::Press(_) => mode >= MouseMode::X10,
        MouseEventKind::Release(_) => mode >= MouseMode::Normal,
        MouseEventKind::ScrollUp
        | MouseEventKind::ScrollDown
        | MouseEventKind::ScrollLeft
        | MouseEventKind::ScrollRight => mode >= MouseMode::Normal,
        MouseEventKind::Drag(_) => mode >= MouseMode::Button,
        MouseEventKind::Move => mode >= MouseMode::Any,
    };
    if !wanted {
        return None;
    }
    let button = |b: MouseButton| match b {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    };
    let (mut code, release) = match event.kind {
        MouseEventKind::Press(b) => (button(b), false),
        MouseEventKind::Release(b) => (button(b), true),
        MouseEventKind::Drag(b) => (button(b) + 32, false),
        MouseEventKind::Move => (3 + 32, false),
        MouseEventKind::ScrollUp => (64, false),
        MouseEventKind::ScrollDown => (65, false),
        MouseEventKind::ScrollLeft => (66, false),
        MouseEventKind::ScrollRight => (67, false),
    };
    if mode != MouseMode::X10 {
        if event.modifiers.shift {
            code += 4;
        }
        if event.modifiers.alt {
            code += 8;
        }
        if event.modifiers.ctrl {
            code += 16;
        }
    }
    let (col, row) = (event.col + 1, event.row + 1);
    if sgr {
        let suffix = if release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{col};{row}{suffix}").into_bytes());
    }
    // X10 encoding: a release is button 3, and the coordinates are
    // single bytes, so cells past 223 can't be reported at all.
    if release {
        code = (code & !3) | 3;
    }
    let byte = |v: usize| u8::try_from(v + 32).ok();
    Some(vec![
        0x1b,
        b'[',
        b'M',
        byte(code as usize)?,
        byte(col)?,
        byte(row)?,
    ])
}

/// Pasted text as the program reads it.
pub(crate) fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + 12);
    if bracketed {
        out.extend(b"\x1b[200~");
    }
    // Line breaks are sent as the Enter key sends them.
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                out.push(b'\r');
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
            }
            '\n' => out.push(b'\r'),
            c => out.extend(c.to_string().into_bytes()),
        }
    }
    if bracketed {
        out.extend(b"\x1b[201~");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(shift: bool, alt: bool, ctrl: bool) -> Modifiers {
        Modifiers { shift, alt, ctrl }
    }

    fn key(k: Key, m: Modifiers, app: bool) -> String {
        String::from_utf8(encode_key(k, m, app).unwrap()).unwrap()
    }

    #[test]
    fn characters_and_control_keys() {
        let none = Modifiers::default();
        assert_eq!(key(Key::Char('a'), none, false), "a");
        assert_eq!(key(Key::Char('é'), none, false), "é");
        assert_eq!(key(Key::Char('A'), mods(true, false, false), false), "A");
        assert_eq!(key(Key::Char('a'), mods(false, false, true), false), "\x01");
        assert_eq!(key(Key::Char('C'), mods(true, false, true), false), "\x03");
        assert_eq!(key(Key::Char(' '), mods(false, false, true), false), "\0");
        assert_eq!(key(Key::Char('['), mods(false, false, true), false), "\x1b");
        assert_eq!(key(Key::Char('/'), mods(false, false, true), false), "/");
        assert_eq!(
            key(Key::Char('x'), mods(false, true, false), false),
            "\x1bx"
        );
        assert_eq!(
            key(Key::Char('x'), mods(false, true, true), false),
            "\x1b\x18"
        );
        assert_eq!(key(Key::Enter, none, false), "\r");
        assert_eq!(key(Key::Enter, mods(false, true, false), false), "\x1b\r");
        assert_eq!(key(Key::Tab, none, false), "\t");
        assert_eq!(key(Key::Tab, mods(true, false, false), false), "\x1b[Z");
        assert_eq!(key(Key::Backspace, none, false), "\x7f");
        assert_eq!(key(Key::Backspace, mods(false, false, true), false), "\x08");
        assert_eq!(
            key(Key::Backspace, mods(false, true, false), false),
            "\x1b\x7f"
        );
        assert_eq!(key(Key::Escape, none, false), "\x1b");
    }

    #[test]
    fn special_keys_and_modifiers() {
        let none = Modifiers::default();
        assert_eq!(key(Key::Up, none, false), "\x1b[A");
        assert_eq!(key(Key::Up, none, true), "\x1bOA");
        assert_eq!(key(Key::Up, mods(true, false, false), true), "\x1b[1;2A");
        assert_eq!(key(Key::Left, mods(false, false, true), false), "\x1b[1;5D");
        assert_eq!(key(Key::Home, none, false), "\x1b[H");
        assert_eq!(key(Key::End, none, true), "\x1bOF");
        assert_eq!(key(Key::Delete, none, false), "\x1b[3~");
        assert_eq!(key(Key::PageUp, mods(true, true, true), false), "\x1b[5;8~");
        assert_eq!(key(Key::F(1), none, false), "\x1bOP");
        assert_eq!(key(Key::F(1), mods(true, false, false), false), "\x1b[1;2P");
        assert_eq!(key(Key::F(5), none, false), "\x1b[15~");
        assert_eq!(
            key(Key::F(12), mods(false, false, true), false),
            "\x1b[24;5~"
        );
        assert_eq!(encode_key(Key::F(13), none, false), None);
    }

    fn mouse(kind: MouseEventKind, col: usize, row: usize, modifiers: Modifiers) -> MouseEvent {
        MouseEvent {
            kind,
            col,
            row,
            modifiers,
        }
    }

    #[test]
    fn mouse_reports() {
        let none = Modifiers::default();
        let press = mouse(MouseEventKind::Press(MouseButton::Left), 4, 9, none);
        assert_eq!(encode_mouse(press, MouseMode::None, true), None);
        assert_eq!(
            encode_mouse(press, MouseMode::Normal, true),
            Some(b"\x1b[<0;5;10M".to_vec())
        );
        assert_eq!(
            encode_mouse(press, MouseMode::Normal, false),
            Some(b"\x1b[M\x20\x25\x2a".to_vec())
        );
        let release = mouse(MouseEventKind::Release(MouseButton::Right), 0, 0, none);
        assert_eq!(
            encode_mouse(release, MouseMode::Normal, true),
            Some(b"\x1b[<2;1;1m".to_vec())
        );
        assert_eq!(
            encode_mouse(release, MouseMode::Normal, false),
            Some(b"\x1b[M\x23\x21\x21".to_vec())
        );
        assert_eq!(encode_mouse(release, MouseMode::X10, true), None);
        let drag = mouse(
            MouseEventKind::Drag(MouseButton::Left),
            1,
            1,
            mods(true, false, true),
        );
        assert_eq!(encode_mouse(drag, MouseMode::Normal, true), None);
        assert_eq!(
            encode_mouse(drag, MouseMode::Button, true),
            Some(b"\x1b[<52;2;2M".to_vec())
        );
        let motion = mouse(MouseEventKind::Move, 1, 1, none);
        assert_eq!(encode_mouse(motion, MouseMode::Button, true), None);
        assert_eq!(
            encode_mouse(motion, MouseMode::Any, true),
            Some(b"\x1b[<35;2;2M".to_vec())
        );
        let wheel = mouse(MouseEventKind::ScrollDown, 300, 2, none);
        assert_eq!(
            encode_mouse(wheel, MouseMode::Normal, true),
            Some(b"\x1b[<65;301;3M".to_vec())
        );
        assert_eq!(
            encode_mouse(wheel, MouseMode::Normal, false),
            None,
            "X10 can't encode a column that far right"
        );
    }

    #[test]
    fn pastes() {
        assert_eq!(encode_paste("a\nb\r\nc\rd", false), b"a\rb\rc\rd");
        assert_eq!(encode_paste("x", true), b"\x1b[200~x\x1b[201~");
    }
}
