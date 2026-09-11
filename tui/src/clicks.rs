//! Double-click detection. The terminal reports each button press on its
//! own, so a double-click is two presses on the same cell within
//! [`DOUBLE_CLICK`] of each other.

use std::time::{Duration, Instant};

/// The most time between two presses that still makes a double-click.
pub const DOUBLE_CLICK: Duration = Duration::from_millis(500);

/// Counts consecutive presses on one cell.
#[derive(Clone, Copy, Debug, Default)]
pub struct ClickTracker {
    last: Option<(Instant, u16, u16)>,
    count: u32,
}

impl ClickTracker {
    /// Record a press at a screen position now. Returns 1 for a single
    /// click and 2 for the second press of a double-click; a third press
    /// starts over, so clicking on and on alternates the two.
    pub fn press(&mut self, x: u16, y: u16) -> u32 {
        self.press_at(x, y, Instant::now())
    }

    /// [`press`](Self::press) at a given time.
    pub fn press_at(&mut self, x: u16, y: u16, now: Instant) -> u32 {
        let repeat = match self.last {
            Some((then, lx, ly)) => {
                (lx, ly) == (x, y)
                    && now.saturating_duration_since(then) <= DOUBLE_CLICK
                    && self.count < 2
            }
            None => false,
        };
        self.count = if repeat { self.count + 1 } else { 1 };
        self.last = Some((now, x, y));
        self.count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_quick_presses_on_one_cell() {
        let start = Instant::now();
        let mut clicks = ClickTracker::default();
        assert_eq!(clicks.press_at(3, 4, start), 1);
        assert_eq!(clicks.press_at(3, 4, start + Duration::from_millis(100)), 2);
        // A third press starts over rather than making a triple.
        assert_eq!(clicks.press_at(3, 4, start + Duration::from_millis(200)), 1);
        assert_eq!(clicks.press_at(3, 4, start + Duration::from_millis(300)), 2);
        // Too slow, or elsewhere, is a single click again.
        assert_eq!(clicks.press_at(3, 4, start + Duration::from_secs(2)), 1);
        assert_eq!(clicks.press_at(3, 5, start + Duration::from_secs(2)), 1);
        assert_eq!(clicks.press_at(3, 5, start + Duration::from_secs(2)), 2);
    }
}
