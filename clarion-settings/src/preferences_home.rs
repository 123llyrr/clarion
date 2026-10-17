//! The available preference panes, arranged in the Catalina category grid.
//!
//! Dimensions are engineering values in logical points, not measured NIB frames.
//! This module only returns navigation requests; the app owns saving and history.

use eframe::egui::{self, FontId, Rect, Response, Sense, Stroke, Ui, Vec2};

use crate::app::ActiveTab;
use crate::icons::{self, PreferenceIcon};
use crate::theme::colors;

#[derive(Clone, Copy, Debug)]
pub struct PaneMetadata {
    pub tab: ActiveTab,
    pub title: &'static str,
    pub aliases: &'static [&'static str],
    pub group: usize,
    pub icon: PreferenceIcon,
}

/// Only panes backed by existing settings pages belong in this catalogue.
pub const PANES: [PaneMetadata; 8] = [
    PaneMetadata {
        tab: ActiveTab::Appearance,
        title: "通用",
        aliases: &[
            "general",
            "appearance",
            "通用与外观",
            "外观",
            "动效",
            "动画",
            "animation",
            "边框",
            "border",
            "背景",
            "background",
            "模糊",
            "blur",
            "窗口关闭",
            "close",
        ],
        group: 0,
        icon: PreferenceIcon::General,
    },
    PaneMetadata {
        tab: ActiveTab::Layout,
        title: "窗口布局",
        aliases: &["layout", "tiling", "平铺", "间距", "gap", "画布", "canvas"],
        group: 0,
        icon: PreferenceIcon::WindowLayout,
    },
    PaneMetadata {
        tab: ActiveTab::Windows,
        title: "调度中心",
        aliases: &[
            "windows",
            "workspaces",
            "mission control",
            "窗口与工作区",
            "窗口管理",
            "工作区",
            "分组",
            "移动",
            "热区",
            "触发角",
            "hot corners",
        ],
        group: 0,
        icon: PreferenceIcon::MissionControl,
    },
    PaneMetadata {
        tab: ActiveTab::Outputs,
        title: "显示器",
        aliases: &[
            "displays",
            "outputs",
            "monitor",
            "分辨率",
            "resolution",
            "缩放",
            "scale",
            "刷新率",
            "refresh rate",
            "屏幕",
            "位置",
            "旋转",
            "rotation",
        ],
        group: 1,
        icon: PreferenceIcon::Displays,
    },
    PaneMetadata {
        tab: ActiveTab::InputHotCorners,
        title: "键盘",
        aliases: &[
            "keyboard",
            "input",
            "输入",
            "鼠标",
            "mouse",
            "快捷键",
            "keybind",
        ],
        group: 1,
        icon: PreferenceIcon::Keyboard,
    },
    PaneMetadata {
        tab: ActiveTab::Trackpad,
        title: "触控板",
        aliases: &[
            "trackpad",
            "touchpad",
            "手势",
            "gesture",
            "轻点",
            "tap",
            "自然滚动",
            "natural scrolling",
        ],
        group: 1,
        icon: PreferenceIcon::Trackpad,
    },
    PaneMetadata {
        tab: ActiveTab::FullConfig,
        title: "更多设置",
        aliases: &[
            "advanced",
            "all settings",
            "全部设置",
            "完整参数",
            "高级",
            "窗口规则",
            "rules",
            "启动",
            "startup",
            "快捷键",
            "keybind",
        ],
        group: 2,
        icon: PreferenceIcon::Advanced,
    },
    PaneMetadata {
        tab: ActiveTab::ConfigEditor,
        title: "配置文件",
        aliases: &[
            "config",
            "configuration",
            "editor",
            "code",
            "kdl",
            "源码",
            "编辑",
            "配置",
        ],
        group: 2,
        icon: PreferenceIcon::Code,
    },
];

pub fn pane(tab: ActiveTab) -> Option<&'static PaneMetadata> {
    PANES.iter().find(|pane| pane.tab == tab)
}

pub fn title(tab: ActiveTab) -> &'static str {
    pane(tab).map_or("系统偏好设置", |pane| pane.title)
}

/// Match every whitespace-separated term against a pane's name or aliases.
/// An empty query returns all panes, in their stable category order.
pub fn matching_panes(query: &str) -> Vec<&'static PaneMetadata> {
    let query = query.trim().to_lowercase();
    PANES
        .iter()
        .filter(|pane| {
            query.split_whitespace().all(|term| {
                pane.title.contains(term) || pane.aliases.iter().any(|alias| alias.contains(term))
            })
        })
        .collect()
}

const TILE_WIDTH: f32 = 92.;
const TILE_HEIGHT: f32 = 88.;
const ICON_SIZE: f32 = 40.;
const ICON_TOP: f32 = 8.;
const GROUP_GAP: f32 = 24.;

struct GridLayout {
    tiles: [Rect; PANES.len()],
    separators: [f32; 2],
    size: Vec2,
}

fn grid_layout(width: f32, tile_height: f32) -> GridLayout {
    let width = width.min(TILE_WIDTH * 7.);
    let tile_width = width.min(TILE_WIDTH);
    let columns = (width / TILE_WIDTH).floor().max(1.) as usize;
    let mut tiles = [Rect::NOTHING; PANES.len()];
    let mut separators = [0.; 2];
    let mut y = 0.;
    for group in 0..3 {
        let mut count = 0;
        for (index, pane) in PANES
            .iter()
            .enumerate()
            .filter(|(_, pane)| pane.group == group)
        {
            tiles[index] = Rect::from_min_size(
                egui::pos2(
                    (count % columns) as f32 * tile_width,
                    y + (count / columns) as f32 * tile_height,
                ),
                egui::vec2(tile_width, tile_height),
            );
            // Keep category membership in the catalogue, not duplicated in the layout.
            debug_assert_eq!(pane.group, group);
            count += 1;
        }
        y += count.div_ceil(columns) as f32 * tile_height;
        if let Some(separator) = separators.get_mut(group) {
            *separator = y + GROUP_GAP / 2.;
            y += GROUP_GAP;
        }
    }
    GridLayout {
        tiles,
        separators,
        size: egui::vec2(width, y),
    }
}

/// Draw into the caller's scrolling content area. Matching panes retain their
/// original positions; unmatched panes stay enabled and can still be opened.
pub fn show(ui: &mut Ui, query: &str) -> Option<ActiveTab> {
    show_observed(ui, query, |_, _, _| {})
}

fn show_observed(
    ui: &mut Ui,
    query: &str,
    mut on_tile: impl FnMut(&PaneMetadata, &Response, bool),
) -> Option<ActiveTab> {
    let available_width = ui.available_width();
    if !available_width.is_finite() || available_width <= 0. {
        return None;
    }
    let matches = matching_panes(query);
    let searching = !query.trim().is_empty();
    let tile_width = available_width.min(TILE_WIDTH);
    let icon_size = ICON_SIZE.min((tile_width - 12.).max(0.));
    let labels: Vec<_> = PANES
        .iter()
        .map(|pane| {
            ui.painter().layout(
                pane.title.to_owned(),
                FontId::proportional(13.),
                colors::text(),
                (tile_width - 8.).max(1.),
            )
        })
        .collect();
    let tile_height = labels.iter().fold(TILE_HEIGHT, |height, label| {
        height.max(ICON_TOP + icon_size + 5. + label.size().y + 6.)
    });
    let layout = grid_layout(available_width, tile_height);
    let (area, _) =
        ui.allocate_exact_size(egui::vec2(available_width, layout.size.y), Sense::hover());
    let origin = area.min + egui::vec2((available_width - layout.size.x) / 2., 0.);
    for y in layout.separators {
        ui.painter().hline(
            origin.x..=origin.x + layout.size.x,
            origin.y + y,
            Stroke::new(1_f32, colors::border().gamma_multiply(0.55)),
        );
    }

    let mut selected = None;
    for (index, pane) in PANES.iter().enumerate() {
        let rect = layout.tiles[index].translate(origin.to_vec2());
        let response = ui.interact(
            rect,
            ui.make_persistent_id(("preference-pane", index)),
            Sense::click(),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), pane.title)
        });
        let matched = matches.iter().any(|matched| matched.tab == pane.tab);
        let emphasized = response.hovered() || response.has_focus();
        if emphasized {
            ui.painter()
                .rect_filled(rect.shrink(3.), 5., colors::surface_hover());
        }
        if response.has_focus() {
            ui.painter().rect_stroke(
                rect.shrink(3.),
                5.,
                Stroke::new(2_f32, colors::control_accent()),
            );
        }
        let mut paint_ui = ui.new_child(
            egui::UiBuilder::new()
                .id_salt(("preference-paint", index))
                .max_rect(rect),
        );
        if searching && !matched && !emphasized {
            paint_ui.multiply_opacity(0.38);
        }
        let icon_rect = Rect::from_center_size(
            egui::pos2(rect.center().x, rect.top() + ICON_TOP + icon_size / 2.),
            Vec2::splat(icon_size),
        );
        if searching && matched {
            paint_ui.painter().circle_filled(
                icon_rect.center(),
                icon_size / 2. + 7.,
                colors::surface(),
            );
            paint_ui.painter().circle_stroke(
                icon_rect.center(),
                icon_size / 2. + 7.,
                Stroke::new(1_f32, colors::border()),
            );
        }
        icons::paint_preference(&paint_ui, icon_rect, pane.icon);
        let label = &labels[index];
        paint_ui.painter().galley(
            egui::pos2(
                rect.center().x - label.size().x / 2.,
                icon_rect.bottom() + 5.,
            ),
            label.clone(),
            colors::text(),
        );
        on_tile(pane, &response, matched);
        if response.clicked() {
            selected = Some(pane.tab);
        }
    }
    if searching && matches.is_empty() {
        ui.add_space(12.);
        ui.add(
            egui::Label::new(
                egui::RichText::new("未找到匹配的设置。请尝试其他名称或关键词。")
                    .size(13.)
                    .color(colors::muted()),
            )
            .wrap(),
        );
    }
    selected
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tile {
        tab: ActiveTab,
        rect: Rect,
        id: egui::Id,
        matched: bool,
    }

    fn frame(
        context: &egui::Context,
        width: f32,
        scale: f32,
        query: &str,
        events: Vec<egui::Event>,
    ) -> (Option<ActiveTab>, Vec<Tile>, egui::FullOutput) {
        let mut input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, 1000.),
            )),
            events,
            ..Default::default()
        };
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .native_pixels_per_point = Some(scale);
        let mut selected = None;
        let mut tiles = Vec::new();
        let output = context.run(input, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::none())
                .show(ctx, |ui| {
                    selected = show_observed(ui, query, |pane, response, matched| {
                        tiles.push(Tile {
                            tab: pane.tab,
                            rect: response.rect,
                            id: response.id,
                            matched,
                        });
                    });
                });
        });
        (selected, tiles, output)
    }

    fn context() -> egui::Context {
        let context = egui::Context::default();
        crate::theme::setup_custom_fonts_and_styles(&context, false);
        context
    }

    #[test]
    fn catalogue_contains_each_existing_pane_once_and_has_no_fake_destinations() {
        assert!(pane(ActiveTab::Dashboard).is_none());
        assert_eq!(title(ActiveTab::Dashboard), "系统偏好设置");
        for tab in [
            ActiveTab::Appearance,
            ActiveTab::Layout,
            ActiveTab::Windows,
            ActiveTab::Outputs,
            ActiveTab::InputHotCorners,
            ActiveTab::Trackpad,
            ActiveTab::FullConfig,
            ActiveTab::ConfigEditor,
        ] {
            assert_eq!(PANES.iter().filter(|pane| pane.tab == tab).count(), 1);
            assert_eq!(pane(tab).unwrap().title, title(tab));
        }
        assert_eq!(
            PANES.iter().map(|pane| pane.group).collect::<Vec<_>>(),
            [0, 0, 0, 1, 1, 1, 2, 2]
        );
    }

    #[test]
    fn search_matches_chinese_and_case_insensitive_english_across_aliases() {
        for (query, expected) in [
            ("动画", ActiveTab::Appearance),
            (" BLUR ", ActiveTab::Appearance),
            ("通用与外观", ActiveTab::Appearance),
            ("windowS", ActiveTab::Windows),
            ("mission control", ActiveTab::Windows),
            ("窗口与工作区", ActiveTab::Windows),
            ("显示器 缩放", ActiveTab::Outputs),
            ("Refresh RATE", ActiveTab::Outputs),
            ("热区", ActiveTab::Windows),
            ("KEYBOARD", ActiveTab::InputHotCorners),
            ("TRACKPAD touchpad", ActiveTab::Trackpad),
            ("手势", ActiveTab::Trackpad),
            ("平铺", ActiveTab::Layout),
            ("advanced", ActiveTab::FullConfig),
            ("KDL", ActiveTab::ConfigEditor),
        ] {
            assert_eq!(
                matching_panes(query)
                    .iter()
                    .map(|pane| pane.tab)
                    .collect::<Vec<_>>(),
                [expected],
                "{query}"
            );
        }
        assert_eq!(matching_panes(" \t\n ").len(), PANES.len());
        assert_eq!(
            matching_panes("快捷键")
                .iter()
                .map(|pane| pane.tab)
                .collect::<Vec<_>>(),
            [ActiveTab::InputHotCorners, ActiveTab::FullConfig]
        );
        assert!(matching_panes("不存在的选项").is_empty());
        assert!(matching_panes("keyboard blur").is_empty());
        assert!(matching_panes("keyboard trackpad").is_empty());
    }

    #[test]
    fn grid_keeps_all_tiles_inside_the_window_without_overlap_at_fractional_dpi() {
        let context = context();
        for scale in [1., 1.25, 1.5, 2.] {
            for width in [80., 180., 280., 460., 668., 980.] {
                let (_, tiles, output) = frame(&context, width, scale, "", vec![]);
                assert_eq!(tiles.len(), PANES.len());
                for (index, tile) in tiles.iter().enumerate() {
                    assert!(tile.rect.is_finite());
                    assert!(
                        tile.rect.left() >= 0. && tile.rect.right() <= width + 0.01,
                        "{width}/{scale}: {:?}",
                        tile.rect
                    );
                    assert!(tile.rect.width() <= TILE_WIDTH && tile.rect.height() >= TILE_HEIGHT);
                    for other in &tiles[index + 1..] {
                        assert!(!tile.rect.shrink(0.1).intersects(other.rect));
                    }
                }
                // Test painted labels too: checking only allocated hit rectangles
                // would miss long Chinese text spilling into a neighbouring tile.
                for shape in &output.shapes {
                    if let egui::Shape::Text(text) = &shape.shape {
                        let bounds = Rect::from_min_size(text.pos, text.galley.size());
                        assert!(
                            tiles.iter().any(|tile| tile.rect.contains_rect(bounds)),
                            "text {bounds:?} at {width}/{scale}"
                        );
                    }
                }
                let primitives = context.tessellate(output.shapes, output.pixels_per_point);
                assert!(!primitives.is_empty());
                for primitive in primitives {
                    if let egui::epaint::Primitive::Mesh(mesh) = primitive.primitive {
                        assert!(mesh.vertices.iter().all(|vertex| vertex.pos.is_finite()));
                    }
                }
            }
        }
    }

    #[test]
    fn search_highlights_matches_without_reordering_or_removing_other_panes() {
        let context = context();
        let (_, before, _) = frame(&context, 668., 1., "", vec![]);
        let (_, searched, highlighted) = frame(&context, 668., 1., "显示器", vec![]);
        assert_eq!(searched.iter().filter(|tile| tile.matched).count(), 1);
        assert_eq!(highlighted.shapes.iter().filter(|shape| matches!(&shape.shape, egui::Shape::Circle(circle) if circle.radius == ICON_SIZE / 2. + 7. && circle.fill == crate::theme::colors::surface())).count(), 1);
        for (before, searched) in before.iter().zip(&searched) {
            assert_eq!(before.tab, searched.tab);
            assert_eq!(before.rect, searched.rect);
            assert_eq!(before.id, searched.id);
            assert_eq!(searched.matched, searched.tab == ActiveTab::Outputs);
        }
        let (_, empty, output) = frame(&context, 668., 1., "不存在的选项", vec![]);
        assert_eq!(empty.len(), PANES.len());
        assert!(empty.iter().all(|tile| !tile.matched));
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text().contains("未找到匹配的设置"))));
    }

    #[test]
    fn unmatched_tile_remains_clickable_and_keyboard_activation_returns_navigation() {
        let context = context();
        let (_, tiles, _) = frame(&context, 668., 1., "显示器", vec![]);
        let tile = &tiles[0];
        assert!(!tile.matched);
        for pressed in [true, false] {
            let (selected, _, _) = frame(
                &context,
                668.,
                1.,
                "显示器",
                vec![
                    egui::Event::PointerMoved(tile.rect.center()),
                    egui::Event::PointerButton {
                        pos: tile.rect.center(),
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
            assert_eq!(selected, if pressed { None } else { Some(tile.tab) });
        }
        for key in [egui::Key::Enter, egui::Key::Space] {
            context.memory_mut(|memory| memory.request_focus(tiles[4].id));
            let (selected, _, _) = frame(
                &context,
                668.,
                1.,
                "显示器",
                vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
            assert_eq!(selected, Some(ActiveTab::InputHotCorners), "{key:?}");
        }
    }

    #[test]
    fn tab_traversal_visits_all_panes_in_category_order_including_dimmed_results() {
        let context = context();
        let (_, tiles, _) = frame(&context, 280., 1.25, "显示器", vec![]);
        for tile in tiles {
            let (selected, _, _) = frame(
                &context,
                280.,
                1.25,
                "显示器",
                vec![egui::Event::Key {
                    key: egui::Key::Tab,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
            assert_eq!(selected, None);
            assert_eq!(
                context.memory(|memory| memory.focused()),
                Some(tile.id),
                "{:?}",
                tile.tab
            );
            // Supply a release so this represents distinct physical Tab presses.
            frame(
                &context,
                280.,
                1.25,
                "显示器",
                vec![egui::Event::Key {
                    key: egui::Key::Tab,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
        }
    }
}
