// SPDX-License-Identifier: GPL-3.0-or-later
//! One native desktop, one visual language, one GPUI event loop.
mod app;
mod backend;
mod desktop;
mod input;
mod preferences;
mod services;
mod view;

use clap::Parser;

#[derive(Parser, Debug, Clone)]
#[command(version=concat!(env!("CARGO_PKG_VERSION")," (",env!("CLARION_SHELL_REVISION"),")"), about = "Clarion native desktop shell")]
pub struct Options {
    /// Use an ordinary window without observing the real clipboard or D-Bus.
    #[arg(long)]
    pub preview: bool,
    /// Alias for a safe preview with sample data.
    #[arg(long)]
    pub demo: bool,
    /// Compatibility with the desktop service. All outputs share one process.
    #[arg(long, default_value_t = 0)]
    pub monitor: usize,
    #[arg(long, alias = "output")]
    pub output_name: Option<String>,
    /// Diagnostics: close only this process after a bounded interval.
    #[arg(long, value_parser=clap::value_parser!(u64).range(1..=3600))]
    pub quit_after: Option<u64>,
    /// Read configuration and print a capability report without opening windows.
    #[arg(long)]
    pub check: bool,
    #[arg(long)]
    pub no_wallpaper: bool,
}

fn main() -> anyhow::Result<()> {
    let options = Options::parse();
    if options.check {
        let env = std::env::vars().collect();
        let prefs = preferences::Preferences::load(&env, options.preview || options.demo)?;
        println!(
            "{}",
            serde_json::json!({"toolkit":"gpui", "language":"rust", "design":"macos", "styles":["macos"], "bar_height":prefs.bar_height, "dock_size":prefs.dock_size, "preview":options.preview||options.demo})
        );
        return Ok(());
    }
    app::run(options)
}
