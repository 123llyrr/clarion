//! Idle policy: turn inactivity into a lock or a suspend request.
//!
//! Callers supply monotonic time and the current inhibit/lock conditions; this
//! module owns no clocks, processes or protocol objects, so the whole decision
//! is testable without a compositor. Each idle period yields at most one action,
//! and any activity re-arms it.

use std::time::{Duration, Instant};

use clarion_config::Idle as IdleConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleAction {
    Lock,
    Suspend,
    Blank,
}

#[derive(Debug)]
pub struct IdlePolicy {
    off: bool,
    timeout: Option<Duration>,
    blank_timeout: Option<Duration>,
    action: clarion_config::IdleAction,
    lock_command: Option<Vec<String>>,
    lock_before_suspend: bool,
    last_activity: Instant,
    acted: bool,
    blanked: bool,
}

impl IdlePolicy {
    pub fn new(config: &IdleConfig, now: Instant) -> Self {
        Self {
            off: config.off,
            timeout: (config.timeout > 0).then(|| Duration::from_secs(config.timeout.into())),
            blank_timeout: (config.blank_timeout > 0)
                .then(|| Duration::from_secs(config.blank_timeout.into())),
            action: config.action,
            lock_command: config
                .lock_command
                .as_ref()
                .map(|command| command.split_whitespace().map(str::to_owned).collect())
                .filter(|command: &Vec<String>| !command.is_empty()),
            lock_before_suspend: config.lock_before_suspend,
            last_activity: now,
            acted: false,
            blanked: false,
        }
    }

    /// Records input. An idle period that already acted starts over. Returns
    /// whether the panels were blanked and must be switched back on.
    pub fn activity(&mut self, now: Instant) -> bool {
        self.last_activity = now;
        self.acted = false;
        std::mem::take(&mut self.blanked)
    }

    /// The command that provides the lock surface, if one is configured.
    pub fn lock_command(&self) -> Option<Vec<String>> {
        self.lock_command.clone()
    }

    pub fn lock_before_suspend(&self) -> bool {
        self.lock_before_suspend
    }

    /// The action to run now, if any. `inhibited` covers idle inhibit surfaces
    /// and the freedesktop idle-inhibit state; `locked` covers a session lock
    /// already being active, which must not restart itself.
    pub fn poll(&mut self, now: Instant, inhibited: bool, locked: bool) -> Option<IdleAction> {
        if self.off || inhibited {
            return None;
        }
        let elapsed = now.saturating_duration_since(self.last_activity);
        // Blanking is independent of the action and outlives a lock: the panels
        // should switch off while the session stays locked.
        if !self.blanked && self.blank_timeout.is_some_and(|timeout| elapsed >= timeout) {
            self.blanked = true;
            return Some(IdleAction::Blank);
        }
        if self.acted || locked {
            return None;
        }
        let timeout = self.timeout?;
        if elapsed < timeout {
            return None;
        }
        self.acted = true;
        match self.action {
            clarion_config::IdleAction::None => None,
            clarion_config::IdleAction::Lock => Some(IdleAction::Lock),
            clarion_config::IdleAction::Suspend => Some(IdleAction::Suspend),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(timeout: u32, action: clarion_config::IdleAction) -> IdleConfig {
        IdleConfig {
            off: false,
            timeout,
            blank_timeout: 0,
            action,
            lock_command: Some("hyprlock --immediate".to_owned()),
            lock_before_suspend: true,
        }
    }

    #[test]
    fn fires_once_after_the_timeout_and_rearms_on_activity() {
        let start = Instant::now();
        let mut policy = IdlePolicy::new(&config(300, clarion_config::IdleAction::Lock), start);

        assert_eq!(
            policy.poll(start + Duration::from_secs(299), false, false),
            None
        );
        assert_eq!(
            policy.poll(start + Duration::from_secs(300), false, false),
            Some(IdleAction::Lock)
        );
        // One action per idle period, not one per poll.
        assert_eq!(
            policy.poll(start + Duration::from_secs(600), false, false),
            None
        );

        policy.activity(start + Duration::from_secs(601));
        assert_eq!(
            policy.poll(start + Duration::from_secs(660), false, false),
            None
        );
        assert_eq!(
            policy.poll(start + Duration::from_secs(901), false, false),
            Some(IdleAction::Lock)
        );
    }

    #[test]
    fn inhibitors_and_an_active_lock_suspend_the_policy() {
        let start = Instant::now();
        let mut policy = IdlePolicy::new(&config(10, clarion_config::IdleAction::Suspend), start);
        let later = start + Duration::from_secs(60);

        // An inhibited timeout must not burn the idle period: it acts once the
        // inhibitor clears without new input.
        assert_eq!(policy.poll(later, true, false), None);
        assert_eq!(policy.poll(later, false, false), Some(IdleAction::Suspend));

        // A session lock already being up must not ask for another one.
        let mut policy = IdlePolicy::new(&config(10, clarion_config::IdleAction::Lock), start);
        assert_eq!(policy.poll(later, false, true), None);
        assert_eq!(policy.poll(later, false, false), Some(IdleAction::Lock));
    }

    #[test]
    fn zero_timeout_or_off_disables_actions() {
        let start = Instant::now();
        let later = start + Duration::from_secs(3600);

        let mut policy = IdlePolicy::new(&config(0, clarion_config::IdleAction::Lock), start);
        assert_eq!(policy.poll(later, false, false), None);

        let mut disabled = config(10, clarion_config::IdleAction::Lock);
        disabled.off = true;
        let mut policy = IdlePolicy::new(&disabled, start);
        assert_eq!(policy.poll(later, false, false), None);

        // "none" keeps the session awake without acting.
        let mut policy = IdlePolicy::new(&config(10, clarion_config::IdleAction::None), start);
        assert_eq!(policy.poll(later, false, false), None);
    }

    #[test]
    fn blanking_is_independent_of_the_action_and_survives_a_lock() {
        let start = Instant::now();
        let mut blanking = config(600, clarion_config::IdleAction::Lock);
        blanking.blank_timeout = 300;
        let mut policy = IdlePolicy::new(&blanking, start);

        // Blanking is due before the action is.
        assert_eq!(
            policy.poll(start + Duration::from_secs(300), false, false),
            Some(IdleAction::Blank)
        );
        // It happens once per idle period.
        assert_eq!(
            policy.poll(start + Duration::from_secs(400), false, false),
            None
        );
        // A locked session still switches its panels off.
        let mut locked_blanking = config(600, clarion_config::IdleAction::Lock);
        locked_blanking.blank_timeout = 300;
        let mut locked = IdlePolicy::new(&locked_blanking, start);
        assert_eq!(
            locked.poll(start + Duration::from_secs(300), false, true),
            Some(IdleAction::Blank)
        );
        // Input switches them back on and re-arms the whole period.
        assert!(policy.activity(start + Duration::from_secs(301)));
        assert!(!policy.activity(start + Duration::from_secs(302)));
        assert_eq!(
            policy.poll(start + Duration::from_secs(500), false, false),
            None
        );
        assert_eq!(
            policy.poll(start + Duration::from_secs(603), false, false),
            Some(IdleAction::Blank)
        );
    }

    #[test]
    fn an_inhibitor_suppresses_blanking_and_off_disables_it() {
        let start = Instant::now();
        let mut blanking = config(600, clarion_config::IdleAction::Lock);
        blanking.blank_timeout = 60;
        let mut policy = IdlePolicy::new(&blanking, start);
        assert_eq!(
            policy.poll(start + Duration::from_secs(90), true, false),
            None
        );
        assert_eq!(
            policy.poll(start + Duration::from_secs(90), false, false),
            Some(IdleAction::Blank)
        );

        let mut disabled = blanking.clone();
        disabled.off = true;
        let mut policy = IdlePolicy::new(&disabled, start);
        assert_eq!(
            policy.poll(start + Duration::from_secs(90), false, false),
            None
        );
        assert!(!policy.activity(start));
    }

    #[test]
    fn lock_command_is_split_and_suspend_can_lock_first() {
        let start = Instant::now();
        let policy = IdlePolicy::new(&config(10, clarion_config::IdleAction::Suspend), start);
        assert_eq!(
            policy.lock_command(),
            Some(vec!["hyprlock".to_owned(), "--immediate".to_owned()])
        );
        assert!(policy.lock_before_suspend());

        let empty = IdleConfig {
            lock_command: Some("   ".to_owned()),
            ..config(10, clarion_config::IdleAction::Lock)
        };
        let policy = IdlePolicy::new(&empty, start);
        assert_eq!(policy.lock_command(), None);
    }
}
