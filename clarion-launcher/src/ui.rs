use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use gpui::{
    actions, div, img, point, prelude::*, px, rgba, App, Context, Entity, Focusable, KeyBinding,
    Render, ScrollAnchor, ScrollHandle, Window, WindowAppearance,
};

use crate::{
    catalog::{AppCatalog, AppCategory, DesktopApp},
    preferences::{AppsViewState, DisplayMode, ViewMode},
    spike_input::{QueryChanged, SpikeInput},
};

actions!(apps, [PreviousApp, NextApp, LaunchSelected, Dismiss]);

#[derive(Clone)]
pub struct Message {
    pub epoch: u64,
    pub request: Request,
}

#[derive(Clone)]
pub enum Request {
    Hide,
    Launch(String),
    View(AppsViewState),
}

pub fn bind_keys(cx: &mut App) {
    SpikeInput::bind_keys(cx);
    cx.bind_keys([
        KeyBinding::new("escape", Dismiss, Some("AppsPanel")),
        KeyBinding::new("enter", LaunchSelected, Some("AppsPanel")),
        KeyBinding::new("up", PreviousApp, Some("AppsPanel")),
        KeyBinding::new("down", NextApp, Some("AppsPanel")),
        KeyBinding::new("alt-left", PreviousApp, Some("AppsPanel")),
        KeyBinding::new("alt-right", NextApp, Some("AppsPanel")),
    ]);
}

pub struct AppsPanel {
    catalog: Arc<AppCatalog>,
    input: Entity<SpikeInput>,
    query: String,
    selected: Option<String>,
    expanded: HashSet<String>,
    pub view: AppsViewState,
    theme: String,
    accent: u32,
    recent: Vec<String>,
    sender: async_channel::Sender<Message>,
    epoch: u64,
    scroll: ScrollHandle,
    anchors: HashMap<String, ScrollAnchor>,
    error: Option<String>,
    launching: bool,
}

impl AppsPanel {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        catalog: Arc<AppCatalog>,
        view: AppsViewState,
        theme: String,
        accent: String,
        recent: Vec<String>,
        sender: async_channel::Sender<Message>,
        epoch: u64,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| SpikeInput::new(cx).with_placeholder("搜索应用"));
        cx.subscribe(&input, |this, _, QueryChanged(query), cx| {
            this.query = query.clone();
            this.selected = this.visible_ids().into_iter().next();
            this.scroll.set_offset(point(px(0.), px(0.)));
            this.error = None;
            cx.notify();
        })
        .detach();
        let mut this = Self {
            catalog,
            input,
            view,
            query: String::new(),
            selected: None,
            expanded: HashSet::new(),
            theme,
            accent: parse_accent(&accent),
            recent,
            sender,
            epoch,
            scroll: ScrollHandle::new(),
            anchors: HashMap::new(),
            error: None,
            launching: false,
        };
        this.selected = this.visible_ids().into_iter().next();
        this
    }

    pub fn set_epoch(&mut self, epoch: u64, cx: &mut Context<Self>) {
        self.epoch = epoch;
        cx.notify();
    }

    fn send(&self, request: Request) {
        let _ = self.sender.try_send(Message {
            epoch: self.epoch,
            request,
        });
    }

    pub fn focus(&self, window: &mut Window, cx: &App) {
        window.focus(&self.input.focus_handle(cx));
    }

    pub fn update_preferences(
        &mut self,
        theme: String,
        accent: String,
        recent: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        self.theme = theme;
        self.accent = parse_accent(&accent);
        self.recent = recent;
        self.reconcile_selection();
        cx.notify();
    }

    pub fn update_catalog(&mut self, catalog: Arc<AppCatalog>, cx: &mut Context<Self>) {
        self.catalog = catalog;
        self.reconcile_selection();
        cx.notify();
    }

    pub fn set_launching(&mut self, launching: bool, cx: &mut Context<Self>) {
        self.launching = launching;
        cx.notify();
    }

    pub fn report_error(&mut self, error: String, cx: &mut Context<Self>) {
        self.launching = false;
        self.error = Some(error);
        cx.notify();
    }

    fn reconcile_selection(&mut self) {
        let ids = self.visible_ids();
        if !self
            .selected
            .as_ref()
            .is_some_and(|selected| ids.contains(selected))
        {
            self.selected = ids.into_iter().next();
        }
    }

    fn visible_ids(&self) -> Vec<String> {
        if !self.query.trim().is_empty() || self.view.mode == ViewMode::Name {
            return self
                .catalog
                .search(&self.query)
                .into_iter()
                .map(|app| app.id.clone())
                .collect();
        }
        let mut ids: Vec<_> = self
            .catalog
            .suggested(&self.recent, 6)
            .into_iter()
            .map(|app| app.id.clone())
            .collect();
        for category in AppCategory::ALL {
            let limit = if self.expanded.contains(category.key()) {
                usize::MAX
            } else {
                8
            };
            ids.extend(
                self.catalog
                    .apps()
                    .iter()
                    .filter(|app| app.category == category)
                    .take(limit)
                    .map(|app| app.id.clone()),
            );
        }
        let mut seen = HashSet::new();
        ids.retain(|id| seen.insert(id.clone()));
        ids
    }

    fn move_selection(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let ids = self.visible_ids();
        self.selected = next_selection(&ids, self.selected.as_deref(), delta);
        if let Some(anchor) = self
            .selected
            .as_ref()
            .and_then(|id| self.anchors.get(&self.navigation_anchor_key(id)))
        {
            anchor.scroll_to(window, cx);
        }
        cx.notify();
    }

    fn navigation_anchor_key(&self, id: &str) -> String {
        let section = if !self.query.trim().is_empty() || self.view.mode == ViewMode::Name {
            "results"
        } else if self
            .catalog
            .suggested(&self.recent, 6)
            .iter()
            .any(|app| app.id == id)
        {
            "suggested"
        } else {
            self.catalog
                .get(id)
                .map(|app| app.category.key())
                .unwrap_or("other")
        };
        format!("{section}-{id}")
    }

    fn launch(&mut self, id: String, cx: &mut Context<Self>) {
        if self.launching {
            return;
        }
        self.launching = true;
        self.selected = Some(id.clone());
        self.error = None;
        self.send(Request::Launch(id));
        cx.notify();
    }

    fn set_mode(&mut self, mode: ViewMode, cx: &mut Context<Self>) {
        self.view.mode = mode;
        self.persist(cx);
    }

    fn set_display(&mut self, display: DisplayMode, cx: &mut Context<Self>) {
        self.view.display = display;
        self.persist(cx);
    }

    fn persist(&mut self, cx: &mut Context<Self>) {
        self.reconcile_selection();
        self.scroll.set_offset(point(px(0.), px(0.)));
        self.send(Request::View(self.view.clone()));
        cx.notify();
    }

    fn tile(
        &mut self,
        app: &DesktopApp,
        section: &str,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let id = app.id.clone();
        let click_id = id.clone();
        let selected = self.selected.as_deref() == Some(id.as_str());
        let anchor = self
            .anchors
            .entry(format!("{section}-{id}"))
            .or_insert_with(|| ScrollAnchor::for_handle(self.scroll.clone()))
            .clone();
        let icon = match app.icon_path.as_ref() {
            Some(path) => img(path.clone())
                .size(px(if self.view.display == DisplayMode::List {
                    32.
                } else {
                    54.
                }))
                .into_any_element(),
            None => div()
                .size(px(if self.view.display == DisplayMode::List {
                    32.
                } else {
                    54.
                }))
                .rounded(px(13.))
                .bg(rgba((self.accent << 8) | 0x48))
                .flex()
                .items_center()
                .justify_center()
                .text_xl()
                .child(app.name.chars().next().unwrap_or('·').to_string())
                .into_any_element(),
        };
        let base = div()
            .id(gpui::SharedString::from(format!("{section}-{id}")))
            .anchor_scroll(Some(anchor))
            .rounded(px(16.))
            .border_1()
            .border_color(rgba(if selected {
                (self.accent << 8) | 0x90
            } else {
                0xffffff00
            }))
            .bg(rgba(if selected {
                (self.accent << 8) | 0x26
            } else {
                0xffffff00
            }))
            .cursor_pointer()
            .hover(|style| style.bg(rgba(0x8f9bb522)))
            .on_click(cx.listener(move |this, _, _, cx| this.launch(click_id.clone(), cx)));
        if self.view.display == DisplayMode::List {
            base.w_full()
                .h(px(56.))
                .px_3()
                .flex()
                .items_center()
                .gap_3()
                .child(icon)
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .child(div().text_sm().text_ellipsis().child(app.name.clone()))
                        .child(
                            div()
                                .text_xs()
                                .opacity(0.55)
                                .text_ellipsis()
                                .child(app.description.clone()),
                        ),
                )
                .child(div().text_xs().opacity(0.45).child(app.category.title()))
                .into_any_element()
        } else {
            base.w(px(104.))
                .h(px(112.))
                .p_2()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .child(icon)
                .child(
                    div()
                        .w_full()
                        .text_center()
                        .text_sm()
                        .line_clamp(2)
                        .child(app.name.clone()),
                )
                .into_any_element()
        }
    }

    fn collection(
        &mut self,
        apps: &[DesktopApp],
        section: &str,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let elements: Vec<_> = apps.iter().map(|app| self.tile(app, section, cx)).collect();
        if self.view.display == DisplayMode::List {
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap_1()
                .children(elements)
                .into_any_element()
        } else {
            div()
                .w_full()
                .flex()
                .flex_wrap()
                .gap_2()
                .children(elements)
                .into_any_element()
        }
    }
}

impl Render for AppsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let light = self.theme == "light"
            || (self.theme == "system"
                && matches!(
                    window.appearance(),
                    WindowAppearance::Light | WindowAppearance::VibrantLight
                ));
        let foreground = rgba(if light { 0x1b2535ff } else { 0xf4f6ffff });
        let selected_button = rgba((self.accent << 8) | 0x40);
        let mut sections = Vec::new();
        if !self.query.trim().is_empty() || self.view.mode == ViewMode::Name {
            let apps: Vec<_> = self
                .catalog
                .search(&self.query)
                .into_iter()
                .cloned()
                .collect();
            sections.push(
                div()
                    .text_sm()
                    .opacity(0.65)
                    .child(if self.query.trim().is_empty() {
                        format!("全部应用 · {}", apps.len())
                    } else {
                        format!("搜索结果 · {}", apps.len())
                    })
                    .into_any_element(),
            );
            if apps.is_empty() {
                sections.push(
                    div()
                        .w_full()
                        .py_12()
                        .text_center()
                        .opacity(0.65)
                        .child("没有找到匹配的应用")
                        .into_any_element(),
                );
            } else {
                sections.push(self.collection(&apps, "results", cx));
            }
        } else {
            let suggested: Vec<_> = self
                .catalog
                .suggested(&self.recent, 6)
                .into_iter()
                .cloned()
                .collect();
            if !suggested.is_empty() {
                sections.push(div().text_lg().child("建议").into_any_element());
                sections.push(self.collection(&suggested, "suggested", cx));
            }
            for category in AppCategory::ALL {
                let all: Vec<_> = self
                    .catalog
                    .apps()
                    .iter()
                    .filter(|app| app.category == category)
                    .cloned()
                    .collect();
                if all.is_empty() {
                    continue;
                }
                let expanded = self.expanded.contains(category.key());
                let key = category.key().to_owned();
                let header = div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .mt_3()
                    .child(div().text_lg().child(category.title()));
                sections.push(if all.len() > 8 {
                    header
                        .child(
                            div()
                                .id(gpui::SharedString::from(format!("more-{key}")))
                                .px_3()
                                .py_1()
                                .rounded_full()
                                .text_xs()
                                .bg(rgba(0x8f9bb522))
                                .cursor_pointer()
                                .child(if expanded { "收起" } else { "显示更多" })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if !this.expanded.remove(&key) {
                                        this.expanded.insert(key.clone());
                                    }
                                    this.reconcile_selection();
                                    cx.notify();
                                })),
                        )
                        .into_any_element()
                } else {
                    header.into_any_element()
                });
                let shown = if expanded {
                    &all[..]
                } else {
                    &all[..all.len().min(8)]
                };
                sections.push(self.collection(shown, category.key(), cx));
            }
            if sections.is_empty() {
                sections.push(
                    div()
                        .w_full()
                        .py_12()
                        .text_center()
                        .opacity(0.65)
                        .child("尚未发现可用的应用")
                        .into_any_element(),
                );
            }
        }
        let category = self.view.mode == ViewMode::Category;
        let icons = self.view.display == DisplayMode::Icons;
        let controls = div()
            .flex()
            .items_center()
            .gap_2()
            .text_xs()
            .child(
                div()
                    .id("category-mode")
                    .px_3()
                    .py_2()
                    .rounded_full()
                    .bg(if category {
                        selected_button
                    } else {
                        rgba(0x8f9bb518)
                    })
                    .cursor_pointer()
                    .child("按类别")
                    .on_click(cx.listener(|this, _, _, cx| this.set_mode(ViewMode::Category, cx))),
            )
            .child(
                div()
                    .id("name-mode")
                    .px_3()
                    .py_2()
                    .rounded_full()
                    .bg(if !category {
                        selected_button
                    } else {
                        rgba(0x8f9bb518)
                    })
                    .cursor_pointer()
                    .child("按名称")
                    .on_click(cx.listener(|this, _, _, cx| this.set_mode(ViewMode::Name, cx))),
            )
            .child(div().w(px(1.)).h(px(18.)).mx_1().bg(rgba(0x8f9bb544)))
            .child(
                div()
                    .id("icons-mode")
                    .px_3()
                    .py_2()
                    .rounded_full()
                    .bg(if icons {
                        selected_button
                    } else {
                        rgba(0x8f9bb518)
                    })
                    .cursor_pointer()
                    .child("图标")
                    .on_click(
                        cx.listener(|this, _, _, cx| this.set_display(DisplayMode::Icons, cx)),
                    ),
            )
            .child(
                div()
                    .id("list-mode")
                    .px_3()
                    .py_2()
                    .rounded_full()
                    .bg(if !icons {
                        selected_button
                    } else {
                        rgba(0x8f9bb518)
                    })
                    .cursor_pointer()
                    .child("列表")
                    .on_click(
                        cx.listener(|this, _, _, cx| this.set_display(DisplayMode::List, cx)),
                    ),
            );
        let close = self.sender.clone();
        let close_epoch = self.epoch;
        div()
            .id("apps-panel-root")
            .key_context("AppsPanel")
            .size_full()
            .p_4()
            .text_color(foreground)
            .on_action(
                cx.listener(|this, _: &PreviousApp, window, cx| {
                    this.move_selection(-1, window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &NextApp, window, cx| this.move_selection(1, window, cx)),
            )
            .on_action(cx.listener(|this, _: &LaunchSelected, _, cx| {
                if let Some(id) = this.selected.clone() {
                    this.launch(id, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Dismiss, _, _| {
                this.send(Request::Hide);
            }))
            .child(
                div()
                    .size_full()
                    .rounded(px(28.))
                    .bg(rgba(if light { 0xf0f3faed } else { 0x142031ed }))
                    .border_1()
                    .border_color(rgba(if light { 0xffffffd0 } else { 0xffffff38 }))
                    .shadow_lg()
                    .p_6()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(div().text_2xl().child("Apps"))
                            .child(
                                div()
                                    .id("close-apps")
                                    .px_3()
                                    .py_1()
                                    .rounded_full()
                                    .bg(rgba(0x8f9bb522))
                                    .text_sm()
                                    .cursor_pointer()
                                    .child("关闭 · Esc")
                                    .on_click(move |_, _, _| {
                                        let _ = close.try_send(Message {
                                            epoch: close_epoch,
                                            request: Request::Hide,
                                        });
                                    }),
                            ),
                    )
                    .child(self.input.clone())
                    .child(controls)
                    .children(self.error.as_ref().map(|error| {
                        div()
                            .rounded_lg()
                            .p_3()
                            .text_sm()
                            .bg(rgba(0xe85d7533))
                            .child(error.clone())
                    }))
                    .children(
                        self.launching
                            .then(|| div().text_xs().opacity(0.65).child("正在打开应用…")),
                    )
                    .child(
                        div()
                            .id("apps-scroll")
                            .flex_1()
                            .min_h(px(0.))
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                            .flex()
                            .flex_col()
                            .gap_3()
                            .pr_2()
                            .children(sections),
                    ),
            )
    }
}

fn parse_accent(value: &str) -> u32 {
    value
        .strip_prefix('#')
        .filter(|value| value.len() == 6)
        .and_then(|value| u32::from_str_radix(value, 16).ok())
        .unwrap_or(0x86b4ff)
}

fn next_selection(ids: &[String], current: Option<&str>, delta: isize) -> Option<String> {
    if ids.is_empty() {
        return None;
    }
    let current = current.and_then(|id| ids.iter().position(|candidate| candidate == id));
    let index = match current {
        Some(index) => (index as isize + delta).rem_euclid(ids.len() as isize) as usize,
        None => {
            if delta < 0 {
                ids.len() - 1
            } else {
                0
            }
        }
    };
    Some(ids[index].clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_recovers_after_filter_or_reload_and_wraps() {
        let ids = vec!["中文.desktop".into(), "org.test.desktop".into()];
        assert_eq!(
            next_selection(&ids, Some("removed.desktop"), 1).as_deref(),
            Some("中文.desktop")
        );
        assert_eq!(
            next_selection(&ids, Some("中文.desktop"), -1).as_deref(),
            Some("org.test.desktop")
        );
        assert!(next_selection(&[], None, 1).is_none());
    }
}

#[cfg(test)]
#[path = "ui_headless_tests.rs"]
mod headless_tests;
