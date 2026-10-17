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
}

#[derive(Debug)]
pub struct IdlePolicy {
    off: bool,
    timeout: Option<Duration>,
    action: clarion_config::IdleAction,
    lock_command: Option<Vec<String>>,
    lock_before_suspend: bool,
    last_activity: Instant,
    acted: bool,
}

impl IdlePolicy {
    pub fn new(config: &IdleConfig, now: Instant) -> Self {
        Self {
            off: config.off,
            timeout: (config.timeout > 0).then(|| Duration::from_secs(config.timeout.into())),
            action: config.action,
            lock_command: config
                .lock_command
                .as_ref()
                .map(|command| command.split_whitespace().map(str::to_owned).collect())
                .filter(|command: &Vec<String>| !command.is_empty()),
            lock_before_suspend: config.lock_before_suspend,
            last_activity: now,
            acted: false,
        }
    }

    /// Records input. An idle period that already acted starts over.
    pub fn activity(&mut self, now: Instant) {
        self.last_activity = now;
        self.acted = false;
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
        if self.off || self.acted || inhibited || locked {
            return None;
        }
        let timeout = self.timeout?;
        if now.saturating_duration_since(self.last_activity) < timeout {
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
            action,
            lock_command: Some("hyprlock --immediate".to_owned()),
            lock_before_suspend: true,
        }
    }

    #[test]
    fn fires_once_after_the_timeout_and_rearms_on_activity() {
        let start = Instant::now();
        let mut policy = IdlePolicy::new(&config(300, clarion_config::IdleAction::Lock), start);

        assert_eq!(policy.poll(start + Duration::from_secs(299), false, false), None);
        assert_eq!(
            policy.poll(start + Duration::from_secs(300), false, false),
            Some(IdleAction::Lock)
        );
        // One action per idle period, not one per poll.
        assert_eq!(policy.poll(start + Duration::from_secs(600), false, false), None);

        policy.activity(start + Duration::from_secs(601));
        assert_eq!(policy.poll(start + Duration::from_secs(660), false, false), None);
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
