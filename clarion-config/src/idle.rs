//! Idle policy: how long the session may stay inactive before it locks, suspends
//! or does nothing. The compositor owns this because the session lock and the
//! sleep request are compositor-level actions; the lock surface itself still
//! comes from a client, which `lock-command` starts.

use crate::utils::{Flag, MergeWith};

#[derive(Debug, Clone, PartialEq)]
pub struct Idle {
    pub off: bool,
    /// Seconds without input before `action` runs. Zero never acts.
    pub timeout: u32,
    pub action: IdleAction,
    /// Command started when the policy requests a lock. Empty leaves locking to
    /// whatever the session runs on its own.
    pub lock_command: Option<String>,
    /// Lock before this policy suspends the system.
    pub lock_before_suspend: bool,
}

#[derive(knuffel::DecodeScalar, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IdleAction {
    #[default]
    None,
    Lock,
    Suspend,
}

impl Default for Idle {
    fn default() -> Self {
        // Inert until configured: no idle action, no command. The shipped
        // default config opts in with a locker command.
        Self {
            off: false,
            timeout: 0,
            action: IdleAction::default(),
            lock_command: None,
            lock_before_suspend: false,
        }
    }
}

#[derive(knuffel::Decode, Debug, PartialEq)]
pub struct IdlePart {
    #[knuffel(child)]
    pub off: Option<Flag>,
    #[knuffel(child, unwrap(argument))]
    pub timeout: Option<u32>,
    #[knuffel(child, unwrap(argument))]
    pub action: Option<IdleAction>,
    #[knuffel(child, unwrap(argument))]
    pub lock_command: Option<String>,
    #[knuffel(child)]
    pub lock_before_suspend: Option<Flag>,
}

impl MergeWith<IdlePart> for Idle {
    fn merge_with(&mut self, part: &IdlePart) {
        merge!((self, part), off);
        merge_clone!((self, part), timeout);
        merge_clone!((self, part), action);
        merge_clone_opt!((self, part), lock_command);
        merge!((self, part), lock_before_suspend);
    }
}
