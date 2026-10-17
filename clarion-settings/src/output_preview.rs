use std::sync::mpsc::Receiver;
use std::time::Instant;

use clarion_ipc::Response;

pub type Reply = Receiver<Result<Response, String>>;

pub enum Phase {
    Starting { reply: Reply, sent: Instant },
    Ready { id: u64, deadline: Instant },
    Finishing { reply: Reply, keep: bool, id: u64 },
    RollingBack { reply: Reply, error: String },
}

pub struct Preview {
    pub content: String,
    // Missing optional includes are dependencies too: creating one changes
    // the configuration just as modifying an existing include does.
    pub dependencies: Vec<(std::path::PathBuf, Option<Vec<u8>>)>,
    pub phase: Phase,
}
