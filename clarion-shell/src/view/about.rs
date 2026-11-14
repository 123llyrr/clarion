// SPDX-License-Identifier: GPL-3.0-or-later
//! The compact "About This Mac" card opened from the system menu.
use super::*;
use crate::system_info::{format_gib, SystemFacts};

impl ShellView {
    pub(super) fn about_card(&self, host: &ViewState, p: Palette) -> AnyElement {
        let facts = &host.system.facts;
        let host_handle = self.host.clone();
        let name = if facts.model.is_empty() {
            if facts.hostname.is_empty() {
                "Clarion".to_owned()
            } else {
                facts.hostname.clone()
            }
        } else {
            facts.model.clone()
        };
        let subtitle = if facts.hostname.is_empty() {
            facts.os_name.clone()
        } else {
            facts.hostname.clone()
        };
        let mut specs = div()
            .w(px(280.))
            .mt(px(18.))
            .flex()
            .flex_col()
            .gap(px(4.));
        for (key, label, value) in about_rows(facts) {
            if value.is_empty() {
                continue;
            }
            specs = specs.child(spec_row(key, label, value, p));
        }

        let close = self.host.clone();
        let mut card = div()
            .id("about-card")
            .debug_selector(|| "about-card".into())
            .size_full()
            .flex()
            .flex_col();
        card = card
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(16.))
                    .pt(px(14.))
                    .child(
                        div()
                            .id("about-close")
                            .debug_selector(|| "about-close".into())
                            .size(px(12.))
                            .rounded_full()
                            .bg(rgb(0xff5f57))
                            .cursor_pointer()
                            .on_click(move |_, _, cx| {
                                close.update(cx, |host, cx| host.handle(Action::Close, cx))
                            }),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .px(px(28.))
                    .pb(px(16.))
                    .child(div().flex_1())
                    .child(laptop_art())
                    .child(
                        div()
                            .id("about-title")
                            .debug_selector(|| "about-title".into())
                            .mt(px(16.))
                            .max_w_full()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(26.))
                            .line_height(px(32.))
                            .font_weight(FontWeight::BOLD)
                            .child(name),
                    )
                    .child(
                        div()
                            .id("about-subtitle")
                            .debug_selector(|| "about-subtitle".into())
                            .mt(px(2.))
                            .text_size(px(12.))
                            .line_height(px(17.))
                            .text_color(p.muted)
                            .child(subtitle),
                    )
                    .child(specs)
                    .child(
                        div()
                            .id("about-more")
                            .debug_selector(|| "about-more".into())
                            .mt(px(20.))
                            .h(px(30.))
                            .px(px(18.))
                            .rounded_full()
                            .bg(p.card)
                            .cursor_pointer()
                            .hover(move |s| s.bg(p.line))
                            .flex()
                            .items_center()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .line_height(px(16.))
                                    .child("更多信息…"),
                            )
                            .on_click(move |_, _, cx| {
                                host_handle.update(cx, |host, cx| {
                                    host.handle(Action::Panel(Panel::Preferences), cx)
                                })
                            }),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(2.))
                            .pt(px(12.))
                            .text_size(px(10.))
                            .line_height(px(13.))
                            .text_color(p.muted)
                            .child("Clarion 桌面环境")
                            .child("© 2026 Clarion · GPL-3.0-or-later"),
                    ),
            );
        card.into_any_element()
    }

    /// The desktop's "get info" card as a top-level modal: the file's artwork
    /// or type mark on top, its metadata below, and a single close control at
    /// the top-left (like the About card).
    pub(super) fn info_card(
        &self,
        path: Option<&std::path::Path>,
        title: &str,
        rows: &[(String, String)],
        p: Palette,
    ) -> AnyElement {
        let close = self.host.clone();
        let is_dir = path.map(|path| path.is_dir()).unwrap_or(true);
        let filename = path
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| title.to_owned());
        let artwork = crate::file_icons::whole_icon(path, is_dir, &[], 140., 0.);
        let mut specs = div().w(px(320.)).flex().flex_col().gap(px(6.));
        for (label, value) in rows {
            specs = specs.child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(12.))
                    .text_size(px(12.))
                    .line_height(px(17.))
                    .child(
                        div()
                            .w(px(56.))
                            .flex_none()
                            .pt(px(1.))
                            .text_right()
                            .text_color(p.muted)
                            .child(label.clone()),
                    )
                    .child({
                        let mut value_row = div()
                            .flex()
                            .items_start()
                            .gap(px(6.))
                            .flex_1()
                            .min_w_0()
                            .child(div().flex_1().min_w_0().child(value.clone()));
                        if label == "位置" {
                            let copy_text = value.clone();
                            value_row = value_row.child(
                                div()
                                    .id("info-copy-path")
                                    .debug_selector(|| "info-copy-path".into())
                                    .size(px(18.))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(5.))
                                    .cursor_pointer()
                                    .hover(move |style| style.bg(p.line))
                                    .child(self.glyph(
                                        &self.themed_glyph_path("edit-copy-symbolic", "clipboard.svg"),
                                        13.,
                                        p,
                                    ))
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                            copy_text.clone(),
                                        ));
                                    }),
                            );
                        }
                        value_row
                    }),
            );
        }
        div()
            .id("info-card")
            .debug_selector(|| "info-card".into())
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .px(px(26.))
            .pt(px(32.))
            .pb(px(28.))
            .child(div().child(artwork))
            .child(
                div()
                    .mt(px(10.))
                    .w_full()
                    .text_center()
                    .text_size(px(15.))
                    .line_height(px(19.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .line_clamp(2)
                    .child(filename),
            )
            .child(div().mt(px(14.)).child(specs))
            .child(
                div()
                    .id("info-close")
                    .debug_selector(|| "info-close".into())
                    .absolute()
                    .left(px(14.))
                    .top(px(12.))
                    .size(px(16.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child(div().size(px(12.)).rounded_full().bg(rgb(0xff5f57)))
                    .on_click(move |_, _, cx| {
                        close.update(cx, |host, cx| host.handle(Action::Close, cx))
                    }),
            )
            .into_any_element()
    }

    /// The native application chooser, opened for a file from any folder.
    pub(super) fn open_with_card(
        &self,
        path: &std::path::Path,
        apps: &[crate::app::OpenWithApp],
        p: Palette,
    ) -> AnyElement {
        let close = self.host.clone();
        let host = self.host.clone();
        let filename = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut rows = div().w(px(340.)).flex().flex_col().gap(px(2.));
        for app in apps {
            let desktop = app.desktop_file.clone();
            let target = path.to_path_buf();
            let host = host.clone();
            let selector = format!("open-with-{}", app.desktop_file.display());
            let mut row = div()
                .id(SharedString::from(selector.clone()))
                .debug_selector(move || selector.clone())
                .h(px(38.))
                .px(px(10.))
                .rounded(px(8.))
                .flex()
                .items_center()
                .gap(px(10.))
                .cursor_pointer()
                .hover(move |row| row.bg(p.line));
            if let Some(icon) = app.icon.clone() {
                row = row.child(img(icon).size(px(26.)).object_fit(ObjectFit::Contain));
            } else {
                row = row.child(div().size(px(26.)));
            }
            rows = rows.child(
                row.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(app.name.clone()),
                )
                .on_click(move |_, _, cx| {
                    host.update(cx, |host, cx| {
                        host.handle(
                            Action::Backend(crate::backend::Command::LaunchDesktopWithPaths(
                                desktop.clone(),
                                vec![target.clone()],
                            )),
                            cx,
                        );
                        host.handle(Action::Close, cx);
                    })
                }),
            );
        }
        div()
            .id("open-with-card")
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .px(px(26.))
            .pt(px(36.))
            .pb(px(26.))
            .child(
                div()
                    .text_size(px(17.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("打开方式"),
            )
            .child(
                div()
                    .mt(px(2.))
                    .text_size(px(11.))
                    .text_color(p.muted)
                    .child(filename),
            )
            .child(div().mt(px(12.)).child(rows))
            .child(
                div()
                    .id("open-with-close")
                    .debug_selector(|| "open-with-close".into())
                    .absolute()
                    .left(px(14.))
                    .top(px(12.))
                    .size(px(16.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child(div().size(px(12.)).rounded_full().bg(rgb(0xff5f57)))
                    .on_click(move |_, _, cx| {
                        close.update(cx, |host, cx| host.handle(Action::Close, cx))
                    }),
            )
            .into_any_element()
    }
}

/// The device illustration at the top of the card: a dark bezel, a blue screen
/// and the wide base, echoing the reference About window without shipping a
/// vendor photo.
fn laptop_art() -> AnyElement {
    div()
        .relative()
        .mt(px(4.))
        .w(px(232.))
        .h(px(146.))
        .child(
            div()
                .absolute()
                .left(px(18.))
                .right(px(18.))
                .top(px(0.))
                .h(px(133.))
                .rounded(px(9.))
                .bg(rgb(0x1b1b1e))
                .p(px(6.))
                .child(
                    div().size_full().rounded(px(4.)).bg(linear_gradient(
                        180.,
                        linear_color_stop(rgb(0x1f6fd0), 0.),
                        linear_color_stop(rgb(0x71c3ff), 1.),
                    )),
                ),
        )
        .child(
            div()
                .absolute()
                .left(px(0.))
                .right(px(0.))
                .bottom(px(0.))
                .h(px(13.))
                .rounded(px(4.))
                .bg(linear_gradient(
                    180.,
                    linear_color_stop(rgb(0xd9d9de), 0.),
                    linear_color_stop(rgb(0xa9a9b0), 1.),
                )),
        )
        .child(
            div()
                .absolute()
                .left(px(96.))
                .bottom(px(3.))
                .w(px(40.))
                .h(px(3.))
                .rounded(px(2.))
                .bg(rgb(0x8a8a90)),
        )
        .into_any_element()
}

fn spec_row(key: &'static str, label: &'static str, value: String, p: Palette) -> AnyElement {
    div()
        .debug_selector(move || format!("about-row-{key}"))
        .flex()
        .gap(px(10.))
        .text_size(px(12.))
        .line_height(px(17.))
        .child(
            div()
                .w(px(64.))
                .flex_none()
                .text_right()
                .text_color(p.muted)
                .child(label),
        )
        .child(div().flex_1().min_w_0().child(value))
        .into_any_element()
}

/// The rows the card can show. Empty values are omitted by the renderer so a
/// machine without, say, a readable serial keeps a clean card.
fn about_rows(facts: &SystemFacts) -> [(&'static str, &'static str, String); 7] {
    let disk = match (facts.disk_free, facts.disk_total) {
        (Some(free), Some(total)) => format!("{} 可用 / {}", format_gib(free), format_gib(total)),
        (None, Some(total)) => format_gib(total),
        _ => String::new(),
    };
    [
        ("cpu", "芯片", facts.cpu.clone()),
        ("gpu", "显卡", facts.gpu.clone()),
        (
            "memory",
            "内存",
            facts.memory_total.map(format_gib).unwrap_or_default(),
        ),
        ("disk", "启动磁盘", disk),
        ("serial", "序列号", facts.serial.clone()),
        ("os", "系统", facts.os_name.clone()),
        ("kernel", "内核", facts.kernel.clone()),
    ]
}

#[cfg(test)]
mod tests {
    use ::core::prelude::v1::test;

    use super::*;

    #[test]
    fn the_about_rows_drop_unavailable_facts() {
        let facts = SystemFacts {
            model: "Blade".into(),
            hostname: "clarion-box".into(),
            os_name: "Arch Linux".into(),
            kernel: "6.17.6-arch1-1".into(),
            cpu: "Intel(R) Core(TM) i7-9750H".into(),
            gpu: "GP106M [GeForce GTX 1060 Mobile]".into(),
            serial: "PF12345678".into(),
            memory_total: Some(16 * 1024 * 1024 * 1024),
            disk_total: Some(512 * 1024 * 1024 * 1024),
            disk_free: Some(128 * 1024 * 1024 * 1024),
        };
        let rows = about_rows(&facts);
        assert_eq!(rows[0], ("cpu", "芯片", "Intel(R) Core(TM) i7-9750H".into()));
        assert_eq!(rows[1], ("gpu", "显卡", "GP106M [GeForce GTX 1060 Mobile]".into()));
        assert_eq!(rows[2].2, "16 GB");
        assert_eq!(rows[3].2, "128 GB 可用 / 512 GB");
        assert_eq!(rows[4].2, "PF12345678");

        let bare = SystemFacts::default();
        let rows = about_rows(&bare);
        assert!(rows[0].2.is_empty());
        assert!(rows[1].2.is_empty());
        assert!(rows[2].2.is_empty());
        assert!(rows[3].2.is_empty());
        assert!(rows[4].2.is_empty());
    }
}
