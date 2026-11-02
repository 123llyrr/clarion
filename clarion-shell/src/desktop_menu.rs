// SPDX-License-Identifier: GPL-3.0-or-later
//! The desktop context menu lives in an overlay surface of its own.
//!
//! A card painted into the desktop window can never be frosted: the compositor
//! blurs what lies *behind* a surface, never that surface's own wallpaper and
//! icons. Opening the menu as a full-screen transparent overlay (the same shape
//! the launcher's application menu uses) lets the compositor blur the real
//! desktop underneath the card while outside clicks still land on its backdrop.
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

use gpui::prelude::*;
use gpui::*;

use crate::desktop::{DesktopAction, DesktopView};
use crate::preferences::SurfaceAppearance;

/// The compact fixed card. Every row and divider is a known size, so the frost
/// mask can be requested for the exact painted outline before layout.
pub(crate) const DESKTOP_MENU_WIDTH: f32 = 218.;
pub(crate) const DESKTOP_MENU_ROW_H: f32 = 26.;
pub(crate) const DESKTOP_MENU_SEPARATOR_H: f32 = 9.;
pub(crate) const DESKTOP_MENU_PADDING: f32 = 4.;
pub(crate) const DESKTOP_MENU_RADIUS: f32 = 12.;
pub(crate) const DESKTOP_MENU_ICON: f32 = 16.;

pub(crate) fn context_menu_height(selected: bool, image: bool) -> f32 {
    let (rows, separators) = if selected {
        (10 + usize::from(image), 4)
    } else {
        (5, 1)
    };
    DESKTOP_MENU_PADDING * 2.
        + rows as f32 * DESKTOP_MENU_ROW_H
        + separators as f32 * DESKTOP_MENU_SEPARATOR_H
        // The one-point border is inside the fixed height too.
        + 2.
}

/// Rectangle mask for the menu's rounded outline, in surface-local pixels.
pub(crate) fn menu_frost_regions(menu: Bounds<Pixels>) -> Vec<Bounds<Pixels>> {
    let mut regions = crate::view::rounded_region(
        f32::from(menu.size.width),
        f32::from(menu.size.height),
        DESKTOP_MENU_RADIUS,
    );
    for region in &mut regions {
        region.origin.x += menu.origin.x;
        region.origin.y += menu.origin.y;
    }
    regions
}

/// The tint, ink and rules shared by the context menu and the desktop's inline
/// rename field, so the two controls read as one material.
#[derive(Clone, Copy)]
pub(crate) struct MenuColors {
    appearance: SurfaceAppearance,
}

impl MenuColors {
    pub(crate) fn new(appearance: SurfaceAppearance) -> Self {
        Self { appearance }
    }

    pub(crate) fn surface(&self) -> Hsla {
        let appearance = self.appearance;
        let base: Hsla = if appearance.dark {
            rgb(0x222431).into()
        } else {
            rgb(0xf8f8fd).into()
        };
        let frost = appearance.frost.clamp(0., 1.);
        // With a blur behind it the card can use the same translucency as
        // every other shell surface. Without the compositor effect the fallback
        // stays denser so text never disappears into a busy wallpaper.
        let alpha = if frost > 0.01 {
            appearance.surface_opacity.clamp(0.3, 1.)
        } else {
            (0.55 + 0.45 * appearance.surface_opacity).clamp(0., 1.)
        };
        let wash = Hsla::from(rgb(0xffffff)).opacity(if appearance.dark {
            0.012 + 0.04 * frost
        } else {
            0.03 + 0.06 * frost
        });
        base.opacity(alpha).blend(wash)
    }

    pub(crate) fn text(&self) -> Hsla {
        if self.appearance.dark {
            rgb(0xf4f5fa).into()
        } else {
            rgb(0x23242a).into()
        }
    }

    pub(crate) fn line(&self) -> Hsla {
        Hsla::from(rgb(if self.appearance.dark {
            0xd9e3ff
        } else {
            0x355578
        }))
        .opacity(0.12 + 0.06 * self.appearance.frost)
    }

    pub(crate) fn hover(&self) -> Hsla {
        Hsla::from(rgb(if self.appearance.dark {
            0xffffff
        } else {
            0x344966
        }))
        .opacity(0.07 + 0.04 * self.appearance.frost)
    }

    pub(crate) fn shadow(&self) -> Hsla {
        Hsla::from(rgb(0x000000)).opacity(0.20 + 0.12 * self.appearance.frost)
    }
}

pub(crate) struct DesktopMenuView {
    desktop: WindowHandle<DesktopView>,
    position: Point<Pixels>,
    width: f32,
    height: f32,
    reserved: [f32; 4],
    appearance: SurfaceAppearance,
    has_selection: bool,
    has_image: bool,
    selected: Option<PathBuf>,
    focus: FocusHandle,
    /// Theme icons resolved once per appearance change: probing every icon
    /// theme on each repaint would stall the frame for a static glyph.
    icons: RefCell<HashMap<&'static str, PathBuf>>,
    /// The surface region currently frost-requested from the compositor.
    pub(crate) glass: Option<Bounds<Pixels>>,
}

impl DesktopMenuView {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        desktop: WindowHandle<DesktopView>,
        position: Point<Pixels>,
        width: f32,
        height: f32,
        reserved: [f32; 4],
        appearance: SurfaceAppearance,
        has_selection: bool,
        has_image: bool,
        selected: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            desktop,
            position,
            width,
            height,
            reserved,
            appearance,
            has_selection,
            has_image,
            selected,
            focus: cx.focus_handle(),
            icons: RefCell::new(HashMap::new()),
            glass: None,
        }
    }

    pub(crate) fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub(crate) fn set_appearance(&mut self, appearance: SurfaceAppearance, cx: &mut Context<Self>) {
        if self.appearance != appearance {
            self.appearance = appearance;
            self.icons.borrow_mut().clear();
            cx.notify();
        }
    }

    fn colors(&self) -> MenuColors {
        MenuColors::new(self.appearance)
    }

    fn card(&self) -> Bounds<Pixels> {
        let [top, bottom, left, right] = self.reserved;
        let height = context_menu_height(self.has_selection, self.has_image)
            .min((self.height - top - bottom - 16.).max(80.));
        Bounds::new(
            point(
                px(f32::from(self.position.x).clamp(
                    left + 8.,
                    (self.width - right - DESKTOP_MENU_WIDTH - 8.).max(left + 8.),
                )),
                px(f32::from(self.position.y)
                    .clamp(top + 8., (self.height - bottom - height - 8.).max(top + 8.))),
            ),
            size(px(DESKTOP_MENU_WIDTH), px(height)),
        )
    }

    fn dismiss(&self, cx: &mut Context<Self>) {
        let desktop = self.desktop;
        let _ = desktop.update(cx, |desktop, _, cx| desktop.close_menu(cx));
    }

    /// A menu prefix glyph: the desktop theme's symbolic mark when it exists,
    /// otherwise the bundled monochrome fallback.
    fn icon(&self, themed: &'static str, fallback: &'static str) -> AnyElement {
        let cached = { self.icons.borrow().get(themed).cloned() };
        let path = match cached {
            Some(path) => path,
            None => {
                let path = crate::view::resolve_theme_icon(themed)
                    .unwrap_or_else(|| crate::view::image_path(fallback));
                self.icons.borrow_mut().insert(themed, path.clone());
                path
            }
        };
        if path.is_file() {
            svg()
                .path(path.to_string_lossy().into_owned())
                .size(px(DESKTOP_MENU_ICON))
                .text_color(self.colors().text())
                .into_any_element()
        } else {
            div().size(px(DESKTOP_MENU_ICON)).into_any_element()
        }
    }

    fn separator(&self, id: &'static str) -> AnyElement {
        let line = self.colors().line();
        div()
            .debug_selector(move || id.to_owned())
            .h(px(DESKTOP_MENU_SEPARATOR_H))
            .px(px(6.))
            .flex()
            .items_center()
            .child(div().h(px(1.)).w_full().bg(line))
            .into_any_element()
    }

    fn row(
        &self,
        id: &'static str,
        label: &'static str,
        icon: (&'static str, &'static str),
        action: impl Fn(&mut DesktopView, &mut Window, &mut Context<DesktopView>) + 'static,
    ) -> AnyElement {
        let hover = self.colors().hover();
        let desktop = self.desktop;
        div()
            .id(id)
            .debug_selector(move || id.to_owned())
            .h(px(DESKTOP_MENU_ROW_H))
            .px(px(8.))
            .flex()
            .items_center()
            .gap(px(8.))
            .rounded(px(5.))
            .text_size(px(13.))
            .line_height(px(16.))
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .child(
                div()
                    .debug_selector(move || format!("desktop-menu-icon-{id}"))
                    .size(px(DESKTOP_MENU_ICON))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(self.icon(icon.0, icon.1)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(label),
            )
            .on_click(move |_, _, cx| {
                let _ = desktop.update(cx, |desktop, window, cx| action(desktop, window, cx));
            })
            .into_any_element()
    }

    fn card_contents(&self) -> AnyElement {
        let mut menu = div()
            .id("desktop-menu-content")
            .w_full()
            .h_full()
            .overflow_y_scroll()
            .p(px(DESKTOP_MENU_PADDING))
            .flex()
            .flex_col();
        if self.has_selection {
            let selected = self.selected.clone();
            menu = menu
                .child(self.row(
                    "desktop-open",
                    "打开",
                    ("document-open-symbolic", "menu-open.svg"),
                    |desktop, window, cx| desktop.activate(window, cx),
                ))
                .child(self.separator("desktop-menu-separator-0"))
                .child(self.row(
                    "desktop-info",
                    "显示简介",
                    ("dialog-information-symbolic", "menu-info.svg"),
                    |desktop, _, cx| desktop.inspect(false, cx),
                ))
                .child(self.row(
                    "desktop-quick-look",
                    "快速查看",
                    ("zoom-fit-best-symbolic", "menu-eye.svg"),
                    |desktop, _, cx| desktop.inspect(true, cx),
                ))
                .child(self.separator("desktop-menu-separator-1"))
                .child(self.row(
                    "desktop-rename",
                    "重命名…",
                    ("document-edit-symbolic", "menu-pencil.svg"),
                    |desktop, window, cx| desktop.begin_rename(window, cx),
                ))
                .child(self.row(
                    "desktop-trash",
                    "移到废纸篓",
                    ("user-trash-symbolic", "menu-trash.svg"),
                    |desktop, _, cx| desktop.trash(cx),
                ));
            if self.has_image {
                menu = menu.child(self.row(
                    "desktop-wallpaper-file",
                    "设为桌面壁纸",
                    ("preferences-desktop-wallpaper-symbolic", "wallpaper.svg"),
                    move |desktop, _, cx| {
                        if let Some(path) = selected.clone() {
                            desktop.send(DesktopAction::Wallpaper(Some(path)), cx)
                        }
                    },
                ));
            }
            menu = menu.child(self.separator("desktop-menu-separator-2"));
        }
        menu.child(self.row(
            "desktop-new-folder",
            "新建文件夹…",
            ("folder-new-symbolic", "menu-folder-plus.svg"),
            |desktop, window, cx| desktop.begin_new_folder(window, cx),
        ))
        .child(self.row(
            "desktop-refresh",
            "刷新桌面",
            ("view-refresh-symbolic", "menu-refresh.svg"),
            |desktop, _, cx| desktop.refresh(cx),
        ))
        .child(self.row(
            "desktop-open-directory",
            "打开桌面文件夹",
            ("folder-open-symbolic", "menu-folder-open.svg"),
            |desktop, _, cx| desktop.open_desktop_folder(cx),
        ))
        .child(self.separator("desktop-menu-separator-3"))
        .child(self.row(
            "desktop-wallpaper",
            "更换壁纸…",
            ("preferences-desktop-wallpaper-symbolic", "wallpaper.svg"),
            |desktop, _, cx| desktop.send(DesktopAction::Wallpaper(None), cx),
        ))
        .child(self.row(
            "desktop-settings",
            "桌面设置…",
            ("preferences-system-symbolic", "settings.svg"),
            |desktop, _, cx| desktop.send(DesktopAction::Settings, cx),
        ))
        .into_any_element()
    }
}

impl Render for DesktopMenuView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let card = self.card();
        let desired = (self.appearance.frost > 0.01).then_some(card);
        if self.glass != desired {
            if let Some(menu) = desired {
                // Set the shape before asking for the effect so the compositor
                // never blurs the whole output for one frame.
                window.set_glass_region(Some(&menu_frost_regions(menu)));
                window.set_background_appearance(WindowBackgroundAppearance::Blurred);
            } else {
                window.set_glass_region(Some(&[]));
                window.set_background_appearance(WindowBackgroundAppearance::Transparent);
            }
            self.glass = desired;
        }
        let colors = self.colors();
        let backdrop = div()
            .id("desktop-menu-backdrop")
            .absolute()
            .inset_0()
            // A barely-there wash so the card reads as modal without dimming
            // the desktop it belongs to.
            .bg(rgba(0x00000001))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.dismiss(cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| this.dismiss(cx)),
            );
        let menu = div()
            .id("desktop-context-menu")
            .debug_selector(|| "desktop-context-menu".into())
            .absolute()
            .left(card.origin.x)
            .top(card.origin.y)
            .w(card.size.width)
            .h(card.size.height)
            .rounded(px(DESKTOP_MENU_RADIUS))
            .bg(colors.surface())
            .text_color(colors.text())
            .border_1()
            .border_color(colors.line())
            .shadow(vec![BoxShadow {
                color: colors.shadow(),
                offset: point(px(0.), px(8.)),
                blur_radius: px(24.),
                spread_radius: px(0.),
            }])
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .child(self.card_contents());
        div()
            .id("desktop-menu-layer")
            .size_full()
            .key_context("ClarionDesktopMenu")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.dismiss(cx);
                }
            }))
            .child(backdrop)
            .child(menu)
            .into_any_element()
    }
}
