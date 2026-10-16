mod app;
mod appearance_controls;
mod command_palette;
mod config_manager;
mod config_schema;
mod editor;
mod full_config;
mod icons;
mod instance;
mod ipc;
mod kdl_edit;
mod output_preview;
mod preferences_home;
mod socket_io;
mod storage;
mod theme;
mod window_manager;

use clap::Parser;
use std::{
    env,
    path::{Path, PathBuf},
};

#[derive(Parser)]
#[command(version, about = "Clarion settings")]
struct Cli {
    /// Close the settings window when focused; otherwise focus or open it.
    #[arg(long, conflicts_with = "command_palette")]
    toggle: bool,
    /// Open the searchable settings and desktop command center.
    #[arg(long)]
    command_palette: bool,
}

use app::ClarionSettingsApp;
use eframe::egui::{self, Vec2};

fn is_absent_instance(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
    )
}

fn is_busy_instance(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::AddrInUse
    ) || matches!(
        error.raw_os_error(),
        Some(libc::EWOULDBLOCK | libc::EADDRINUSE)
    )
}

fn take_instance(
    path: &Path,
    toggle: bool,
    command_palette: bool,
) -> std::io::Result<Option<instance::Instance>> {
    for _ in 0..25 {
        let signal = if command_palette {
            instance::signal_command_palette(path)
        } else {
            instance::signal(path, toggle)
        };
        match signal {
            Ok(()) => return Ok(None),
            Err(error) if is_absent_instance(&error) => {}
            Err(error) => return Err(error),
        }
        match instance::Instance::bind(path.to_path_buf()) {
            Ok(instance) => return Ok(Some(instance)),
            Err(error) if is_busy_instance(&error) => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(error) => return Err(error),
        }
    }
    if command_palette {
        instance::signal_command_palette(path)
    } else {
        instance::signal(path, toggle)
    }
    .map(|()| None)
}

fn get_single_instance_socket_path() -> PathBuf {
    if let Ok(runtime_dir) = env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(runtime_dir).join("clarion-settings.sock")
    } else {
        std::env::temp_dir().join(format!("clarion-settings-{}.sock", unsafe {
            libc::getuid()
        }))
    }
}

fn main() -> eframe::Result<()> {
    // Parse help/version before touching sockets or initializing the GUI.
    let args = Cli::parse();
    let path = get_single_instance_socket_path();
    let instance = match take_instance(&path, args.toggle, args.command_palette) {
        Ok(None) => return Ok(()),
        Ok(Some(instance)) => instance,
        Err(error) => return Err(eframe::Error::AppCreation(Box::new(error))),
    };

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("系统偏好设置")
            .with_app_id("clarion-settings")
            .with_inner_size(Vec2::new(700.0, 580.0))
            .with_min_inner_size(Vec2::new(480.0, 360.0))
            .with_decorations(false)
            .with_transparent(false),
        ..Default::default()
    };

    eframe::run_native(
        "Clarion Settings",
        native_options,
        Box::new(|cc| {
            theme::setup_custom_fonts_and_styles(&cc.egui_ctx);
            let toggle_flag = Some(instance.start(cc.egui_ctx.clone())?);
            let mut app = ClarionSettingsApp::new(cc, toggle_flag);
            if args.command_palette {
                app.open_command_palette();
            }
            Ok(Box::new(app))
        }),
    )
}

#[cfg(test)]
mod cli_tests {
    use super::*;

    #[test]
    fn command_palette_cli_opens_explicitly_and_conflicts_with_toggle() {
        let args = Cli::try_parse_from(["clarion-settings", "--command-palette"]).unwrap();
        assert!(args.command_palette);
        assert!(!args.toggle);
        assert!(
            Cli::try_parse_from(["clarion-settings", "--command-palette", "--toggle"]).is_err()
        );
        let args = Cli::try_parse_from(["clarion-settings"]).unwrap();
        assert!(!args.command_palette);
        assert!(!args.toggle);
    }
}
