//! Clarion's independent Tahoe-style Apps launcher.
mod daemon;
mod ipc;
mod spike;
mod spike_input;
mod ui;

use clap::Parser;
use clarion_launcher::{catalog, launch, preferences};

#[derive(Parser)]
#[command(version, about = "Clarion Apps launcher")]
struct Options {
    /// Start hidden and wait for Shell IPC.
    #[arg(long)]
    serve: bool,
    /// Wayland output name, for example eDP-1.
    #[arg(long)]
    output: Option<String>,
    /// Exit with this parent Shell process.
    #[arg(long)]
    parent_pid: Option<u32>,
    /// Run the isolated Phase 0 input/layer probe instead of the Apps catalog.
    #[arg(long)]
    spike: bool,
    /// Probe same-process last-window drop and reopening.
    #[arg(long, requires = "spike")]
    lifecycle_probe: bool,
    /// Automatically quit after this many seconds (diagnostics).
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..=3600))]
    quit_after: Option<u64>,
}

fn main() -> anyhow::Result<()> {
    let options = Options::parse();
    if options.spike {
        spike::run(spike::Options {
            output: options.output,
            lifecycle_probe: options.lifecycle_probe,
            quit_after: options.quit_after,
        })
    } else {
        daemon::run(daemon::Options {
            serve: options.serve,
            output: options.output,
            parent_pid: options.parent_pid,
            quit_after: options.quit_after,
        })
    }
}
