//! A repeat belongs to one key press and must not survive loss of its window.

use std::time::Duration;
use xkbcommon::xkb::Keycode;

pub(super) struct KeyRepeat {
    pub(super) characters_per_second: u32,
    pub(super) delay: Duration,
    current_id: u64,
    current_keycode: Option<Keycode>,
}

impl Default for KeyRepeat {
    fn default() -> Self {
        Self {
            characters_per_second: 16,
            delay: Duration::from_millis(500),
            current_id: 0,
            current_keycode: None,
        }
    }
}

impl KeyRepeat {
    pub(super) fn pressed(&mut self, keycode: Keycode) -> u64 {
        self.current_id = self.current_id.wrapping_add(1);
        self.current_keycode = Some(keycode);
        self.current_id
    }

    pub(super) fn released(&mut self, keycode: Keycode) {
        if self.current_keycode == Some(keycode) {
            self.cancel();
        }
    }

    pub(super) fn cancel(&mut self) {
        self.current_id = self.current_id.wrapping_add(1);
        self.current_keycode = None;
    }

    pub(super) fn is_active(&self, id: u64) -> bool {
        id == self.current_id && self.current_keycode.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_key_stays_active_across_repeat_ticks_until_its_release() {
        let mut repeat = KeyRepeat::default();
        let key = Keycode::from(38_u32);
        let id = repeat.pressed(key);
        for _ in 0..100 {
            assert!(repeat.is_active(id));
        }
        repeat.released(key);
        assert!(!repeat.is_active(id));
    }

    #[test]
    fn losing_focus_or_destroying_a_window_cancels_the_old_press() {
        let mut repeat = KeyRepeat::default();
        let escape = Keycode::from(9_u32);
        let old = repeat.pressed(escape);
        repeat.cancel();
        assert!(!repeat.is_active(old));
        let new = repeat.pressed(escape);
        assert!(!repeat.is_active(old));
        assert!(repeat.is_active(new));
    }

    #[test]
    fn a_release_without_a_focused_window_still_invalidates_its_timer() {
        let mut repeat = KeyRepeat::default();
        let key = Keycode::from(9_u32);
        let id = repeat.pressed(key);
        repeat.released(key);
        assert!(!repeat.is_active(id));
        repeat.cancel();
        assert!(!repeat.is_active(id));
    }

    #[test]
    fn another_key_press_invalidates_the_old_timer_but_not_the_new_key() {
        let mut repeat = KeyRepeat::default();
        let first = Keycode::from(38_u32);
        let second = Keycode::from(56_u32);
        let old = repeat.pressed(first);
        let current = repeat.pressed(second);
        repeat.released(first);
        assert!(!repeat.is_active(old));
        assert!(repeat.is_active(current));
        repeat.released(second);
        assert!(!repeat.is_active(current));
    }
}
