//! Swipe gesture from scroll events.
//!
//! Tracks when to begin, update, and end a swipe gesture from pointer axis events, also whether
//! the gesture is vertical or horizontal. Necessary because libinput only provides touchpad swipe
//! gesture events for 3+ fingers.

#[derive(Debug)]
pub struct ScrollSwipeGesture {
    ongoing: bool,
    vertical: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    BeginUpdate,
    Update,
    End,
}

impl ScrollSwipeGesture {
    pub const fn new() -> Self {
        Self {
            ongoing: false,
            vertical: false,
        }
    }

    pub fn update(&mut self, dx: f64, dy: f64) -> Action {
        if dx == 0. && dy == 0. {
            self.ongoing = false;
            Action::End
        } else if !self.ongoing {
            self.ongoing = true;
            self.vertical = dy.abs() >= dx.abs();
            Action::BeginUpdate
        } else {
            Action::Update
        }
    }

    pub fn reset(&mut self) -> bool {
        if self.ongoing {
            self.ongoing = false;
            true
        } else {
            false
        }
    }

    pub fn is_vertical(&self) -> bool {
        self.vertical
    }
}

impl Default for ScrollSwipeGesture {
    fn default() -> Self {
        Self::new()
    }
}

impl Action {
    pub fn begin(self) -> bool {
        self == Action::BeginUpdate
    }

    pub fn end(self) -> bool {
        self == Action::End
    }
}

#[cfg(test)]
mod tests {
    use super::{Action, ScrollSwipeGesture};

    #[test]
    fn diagonal_scroll_uses_the_dominant_axis() {
        for (dx, dy, vertical) in [
            (12., 0.1, false),
            (-12., 0.1, false),
            (12., -0.1, false),
            (0.1, 12., true),
            (0.1, -12., true),
            (-0.1, 12., true),
            (12., 0., false),
            (0., 12., true),
        ] {
            let mut gesture = ScrollSwipeGesture::new();
            assert_eq!(gesture.update(dx, dy), Action::BeginUpdate);
            assert_eq!(gesture.is_vertical(), vertical, "delta = ({dx}, {dy})");
        }
    }

    #[test]
    fn axis_stays_locked_until_the_scroll_ends() {
        let mut gesture = ScrollSwipeGesture::new();
        assert_eq!(gesture.update(12., 0.1), Action::BeginUpdate);
        assert!(!gesture.is_vertical());
        assert_eq!(gesture.update(0.1, 30.), Action::Update);
        assert!(!gesture.is_vertical());
        assert_eq!(gesture.update(-20., -0.1), Action::Update);
        assert!(!gesture.is_vertical());

        assert_eq!(gesture.update(0., 0.), Action::End);
        assert_eq!(gesture.update(0.1, 12.), Action::BeginUpdate);
        assert!(gesture.is_vertical());
        assert_eq!(gesture.update(30., 0.1), Action::Update);
        assert!(gesture.is_vertical());
    }

    #[test]
    fn idle_samples_do_not_select_the_next_scroll_axis() {
        let mut gesture = ScrollSwipeGesture::new();
        assert_eq!(gesture.update(0., 0.), Action::End);
        assert!(!gesture.reset());
        assert_eq!(gesture.update(0., 12.), Action::BeginUpdate);
        assert!(gesture.is_vertical());
        assert_eq!(gesture.update(0., 0.), Action::End);
        assert_eq!(gesture.update(0., 0.), Action::End);
        assert_eq!(gesture.update(12., 0.1), Action::BeginUpdate);
        assert!(!gesture.is_vertical());
    }

    #[test]
    fn reset_allows_the_next_scroll_to_choose_its_axis() {
        let mut gesture = ScrollSwipeGesture::new();
        assert_eq!(gesture.update(0.1, 12.), Action::BeginUpdate);
        assert!(gesture.is_vertical());
        assert!(gesture.reset());
        assert!(!gesture.reset());
        assert_eq!(gesture.update(12., 0.1), Action::BeginUpdate);
        assert!(!gesture.is_vertical());
    }
}
