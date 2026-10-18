//! Optional surface identity for Clarion's own desktop shortcuts.
//!
//! This is only a Wayland protocol client: no GTK toolkit, D-Bus connection or
//! exported application menus are involved. The narrow namespace allowlist must
//! not turn an arbitrary GPUI application into the session's desktop owner.

#[allow(dead_code, non_camel_case_types, unused_unsafe, unused_variables)]
#[allow(non_upper_case_globals, non_snake_case, unused_imports)]
#[allow(missing_docs, clippy::all)]
pub(crate) mod client {
    use wayland_client;
    use wayland_client::protocol::*;

    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("src/platform/linux/wayland/gtk-shell.xml");
    }
    use self::__interfaces::*;
    wayland_scanner::generate_client_code!("src/platform/linux/wayland/gtk-shell.xml");
}

pub(crate) fn desktop_application_id(namespace: &str) -> Option<&'static str> {
    match namespace {
        "clarion-desktop" | "clarion-desktop-interaction" => {
            Some("io.github.clarion.Desktop.MacOS")
        }
        _ => None,
    }
}

pub(crate) fn destroy_surface(surface: client::gtk_surface1::GtkSurface1) {
    use wayland_client::Proxy;
    // Clarion deliberately advertises metadata-only v1. GTK's release request
    // exists only since v4, so v1 cleanup is the same local proxy destruction
    // used by wl_proxy_destroy; wl_surface teardown invalidates its identity.
    if surface.version() >= 4 {
        surface.release();
    } else if let Some(backend) = surface.backend().upgrade() {
        let _ = backend.destroy_object(&surface.id());
    }
}

#[cfg(test)]
mod tests {
    use super::desktop_application_id;

    #[::core::prelude::v1::test]
    fn only_explicit_desktop_layers_receive_the_shortcut_identity() {
        assert_eq!(
            desktop_application_id("clarion-desktop"),
            Some("io.github.clarion.Desktop.MacOS")
        );
        assert_eq!(
            desktop_application_id("clarion-desktop-interaction"),
            Some("io.github.clarion.Desktop.MacOS")
        );
        for namespace in [
            "clarion-panel",
            "clarion-dock",
            "clarion-bar",
            "clarion-launcher",
            "gpui",
            "",
            "clarion-desktop-other",
        ] {
            assert_eq!(desktop_application_id(namespace), None);
        }
    }
}
