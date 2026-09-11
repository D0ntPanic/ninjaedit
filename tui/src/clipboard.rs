//! The system clipboard.
//!
//! Copy and cut go to the operating system's clipboard so text can move
//! between the editor and other programs. The [`Clipboard`] is created once
//! and kept for the life of the process: on Linux the clipboard owner is
//! asked for the contents whenever another program pastes, and `arboard`
//! serves those requests only while its handle is alive. Dropping it would
//! make the copied text vanish.
//!
//! When there is no system clipboard to talk to (no display server, an SSH
//! session, tests), the clipboard falls back to a plain string so copy and
//! paste still work within the editor.

/// The application's clipboard, backed by the system clipboard when one is
/// available.
pub struct Clipboard {
    system: Option<arboard::Clipboard>,
    /// Text kept in-process, used when the system clipboard is unavailable
    /// or refused the last write.
    local: Option<String>,
}

impl Clipboard {
    /// Connect to the system clipboard, or fall back to an in-process
    /// clipboard if none is available.
    pub fn new() -> Clipboard {
        Clipboard {
            system: arboard::Clipboard::new().ok(),
            local: None,
        }
    }

    /// A clipboard with no system backing, for tests, so that they don't
    /// touch (or depend on) the real clipboard.
    #[cfg(test)]
    pub fn local_only() -> Clipboard {
        Clipboard {
            system: None,
            local: None,
        }
    }

    /// Place `text` on the clipboard.
    pub fn set(&mut self, text: String) {
        if let Some(system) = &mut self.system
            && system.set_text(text.clone()).is_ok()
        {
            // The system clipboard is authoritative now: a stale local copy
            // must not shadow later changes made by other programs.
            self.local = None;
            return;
        }
        self.local = Some(text);
    }

    /// The text on the clipboard, if any.
    pub fn get(&mut self) -> Option<String> {
        if let Some(system) = &mut self.system
            && let Ok(text) = system.get_text()
        {
            return Some(text);
        }
        self.local.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_fallback_round_trips() {
        let mut clipboard = Clipboard::local_only();
        assert_eq!(clipboard.get(), None);
        clipboard.set("hello".to_string());
        assert_eq!(clipboard.get().as_deref(), Some("hello"));
        clipboard.set("again".to_string());
        assert_eq!(clipboard.get().as_deref(), Some("again"));
    }

    /// Round-trips text through the real system clipboard, then restores
    /// whatever was there. Ignored by default: it needs a display and
    /// briefly changes the user's clipboard.
    #[test]
    #[ignore]
    fn system_round_trip() {
        let mut clipboard = Clipboard::new();
        assert!(clipboard.system.is_some(), "no system clipboard available");
        let previous = clipboard.get();
        clipboard.set("ninjaedit clipboard test".to_string());
        assert_eq!(clipboard.get().as_deref(), Some("ninjaedit clipboard test"));
        assert_eq!(clipboard.local, None);
        if let Some(previous) = previous {
            clipboard.set(previous);
        }
    }
}
