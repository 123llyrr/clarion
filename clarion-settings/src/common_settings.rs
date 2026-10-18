//! Typed controls backed by the compositor's effective KDL configuration.
//!
//! Only changed fields are written. Whole-group overrides first copy their
//! effective source, and the merged candidate must equal the intended config.
use std::path::PathBuf;

use anyhow::{Context, Result};
use clarion_config::animations::{Animation, Animations, Curve, Kind, WindowOpenEffect};
use clarion_config::{Config, PresetSize, WindowCloseEffect};
use knuffel::ast::SpannedNode;
use knuffel::span::Span;

use crate::config_manager::ConfigManager;
use crate::kdl_edit::Editor;

#[derive(Clone, Debug, PartialEq)]
pub struct CommonSettings {
    pub animations_enabled: bool,
    /// The compositor's duration multiplier: smaller means faster.
    pub animation_speed: f64,
    pub window_open_enabled: bool,
    pub window_close_enabled: bool,
    pub window_movement_enabled: bool,
    pub window_resize_enabled: bool,
    pub workspace_switch_enabled: bool,
    pub window_open_effect: String,
    pub window_close_effect: String,
    /// Read from the effective shader; true -> false is explicit replacement.
    pub open_custom_shader: bool,
    pub close_custom_shader: bool,
    pub wobble_enabled: bool,
    pub cube_enabled: bool,
    pub glide_enabled: bool,
    pub auto_stack: bool,
    pub max_windows_per_column: u32,
    pub default_width: Option<PresetSize>,
    pub gaps: f64,
    pub center_column: String,
    pub center_single: bool,
    pub border_enabled: bool,
    pub focus_ring_enabled: bool,
    pub shadow_enabled: bool,
    pub border_width: f64,
    /// Rotation of the active border gradient; inactive/urgent stay independent.
    pub border_gradient_enabled: bool,
    pub border_gradient_speed: f64,
    pub focus_ring_width: f64,
    pub rounding: f64,
    pub blur_enabled: bool,
    pub blur_passes: u8,
    pub blur_offset: f64,
    pub touchpad_tap: bool,
    pub touchpad_natural_scroll: bool,
    pub touchpad_dwt: bool,
    pub touchpad_speed: f64,
    pub touchpad_scroll_factor: f64,
    pub mouse_natural_scroll: bool,
    pub mouse_speed: f64,
    pub keyboard_numlock: bool,
    pub keyboard_repeat_rate: u8,
    pub keyboard_repeat_delay: u16,
}

impl Default for CommonSettings {
    fn default() -> Self {
        Self::from_config(&Config::default())
    }
}

impl CommonSettings {
    pub fn read(manager: &ConfigManager) -> Result<Self> {
        read(manager)
    }

    pub fn apply(manager: &mut ConfigManager, before: &Self, after: &Self) -> Result<()> {
        apply(manager, before, after)
    }

    fn from_config(config: &Config) -> Self {
        let a = &config.animations;
        let l = &config.layout;
        let t = &config.input.touchpad;
        let m = &config.input.mouse;
        let k = &config.input.keyboard;
        Self {
            animations_enabled: !a.off,
            animation_speed: a.slowdown,
            window_open_enabled: !a.window_open.anim.off,
            window_close_enabled: !a.window_close.anim.off,
            window_movement_enabled: !a.window_movement.0.off,
            window_resize_enabled: !a.window_resize.anim.off,
            workspace_switch_enabled: !a.workspace_switch.0.off,
            window_open_effect: match a.window_open.effect {
                WindowOpenEffect::Fade => "fade",
                WindowOpenEffect::Reveal => "reveal",
            }
            .into(),
            window_close_effect: match a.window_close.effect {
                WindowCloseEffect::Explosion => "explosion",
                WindowCloseEffect::Implode => "implode",
                WindowCloseEffect::Fade => "fade",
            }
            .into(),
            open_custom_shader: a.window_open.custom_shader.is_some(),
            close_custom_shader: a.window_close.custom_shader.is_some(),
            wobble_enabled: !a.window_wobble.off,
            cube_enabled: !a.workspace_switch.1.off,
            // Opening and closing have independent Glide settings. This control
            // changes opening only, preserving independently tuned closing.
            glide_enabled: !a.window_open.glide.off,
            auto_stack: l.auto_stack_new_windows,
            max_windows_per_column: l.max_windows_per_column,
            default_width: l.default_column_width,
            gaps: l.gaps,
            center_column: match l.center_focused_column {
                clarion_config::layout::CenterFocusedColumn::Never => "never",
                clarion_config::layout::CenterFocusedColumn::Always => "always",
                clarion_config::layout::CenterFocusedColumn::OnOverflow => "on-overflow",
            }
            .into(),
            center_single: l.always_center_single_column,
            border_enabled: !l.border.off,
            focus_ring_enabled: !l.focus_ring.off,
            shadow_enabled: l.shadow.on,
            border_width: l.border.width,
            border_gradient_enabled: l.border.active_gradient.is_some_and(|gradient| {
                gradient
                    .rotate_deg_per_sec
                    .is_some_and(|speed| speed.0 != 0.)
            }),
            border_gradient_speed: l
                .border
                .active_gradient
                .and_then(|gradient| gradient.rotate_deg_per_sec)
                .filter(|speed| speed.0 != 0.)
                .map_or(45., |speed| speed.0),
            focus_ring_width: l.focus_ring.width,
            rounding: l.rounding,
            blur_enabled: !config.blur.off,
            blur_passes: config.blur.passes,
            blur_offset: config.blur.offset,
            touchpad_tap: t.tap,
            touchpad_natural_scroll: t.natural_scroll,
            touchpad_dwt: t.dwt,
            touchpad_speed: t.accel_speed.0,
            touchpad_scroll_factor: t.scroll_factor.and_then(|f| f.base).map_or(1., |f| f.0),
            mouse_natural_scroll: m.natural_scroll,
            mouse_speed: m.accel_speed.0,
            keyboard_numlock: k.numlock,
            keyboard_repeat_rate: k.repeat_rate,
            keyboard_repeat_delay: k.repeat_delay,
        }
    }
}

pub fn read(manager: &ConfigManager) -> Result<CommonSettings> {
    let config = manager.effective_config()?;
    read_with_sources(
        &config,
        manager.raw_content(),
        &manager.effective_source_contents()?,
    )
}

fn read_with_sources(
    config: &Config,
    source: &str,
    sources: &[(PathBuf, Option<Vec<u8>>)],
) -> Result<CommonSettings> {
    let mut settings = CommonSettings::from_config(config);
    if !settings.border_gradient_enabled {
        if let Some(gradient) = config.layout.border.active_gradient {
            for candidate in std::iter::once(source.to_owned()).chain(
                sources
                    .iter()
                    .filter_map(|(_, bytes)| bytes.as_ref())
                    .map(|bytes| String::from_utf8_lossy(bytes).into_owned()),
            ) {
                if let Some((_, Some(remembered))) = matching_gradient(&candidate, gradient)? {
                    settings.border_gradient_speed = remembered;
                    break;
                }
            }
        }
    }
    Ok(settings)
}

/// Produce a checked draft. Saving and IPC application remain the caller's job.
pub fn apply(
    manager: &mut ConfigManager,
    before: &CommonSettings,
    after: &CommonSettings,
) -> Result<()> {
    if before == after {
        return Ok(());
    }
    let sources = manager.effective_source_contents()?;
    let mut desired = manager.effective_config()?;
    let fresh = read_with_sources(&desired, manager.raw_content(), &sources)?;
    let mut content = manager.raw_content().to_owned();

    // Replaying a pending UI edit may coexist with changes to unrelated fields,
    // but it must not replace a newer edit of the same field.
    macro_rules! changed {
        ($field:ident) => {{
            let changed = before.$field != after.$field;
            anyhow::ensure!(
                !changed || before.$field == fresh.$field,
                "设置来源已改变（{}），请重新载入后再修改",
                stringify!($field)
            );
            changed
        }};
    }
    macro_rules! scalar {
        ($field:ident, $path:expr, $value:expr, $target:expr) => {
            if changed!($field) {
                content = set_scalar(&content, $path, &$value.to_string())?;
                $target = after.$field;
            }
        };
    }
    macro_rules! pair {
        ($field:ident, $path:expr, $target:expr) => {
            if changed!($field) {
                content = set_switch(&content, $path, after.$field)?;
                $target = !after.$field;
            }
        };
    }

    let replace_open_shader = changed!(open_custom_shader);
    let replace_close_shader = changed!(close_custom_shader);
    anyhow::ensure!(
        !replace_open_shader || !after.open_custom_shader,
        "不能通过内置效果选择创建自定义着色器"
    );
    anyhow::ensure!(
        !replace_close_shader || !after.close_custom_shader,
        "不能通过内置效果选择创建自定义着色器"
    );
    let open = changed!(window_open_enabled)
        || changed!(window_open_effect)
        || changed!(glide_enabled)
        || replace_open_shader;
    let close =
        changed!(window_close_enabled) || changed!(window_close_effect) || replace_close_shader;
    let movement = changed!(window_movement_enabled);
    let resize = changed!(window_resize_enabled);
    let workspace = changed!(workspace_switch_enabled) || changed!(cube_enabled);
    let wobble = changed!(wobble_enabled);
    for (modified, name) in [
        (open, "window-open"),
        (close, "window-close"),
        (movement, "window-movement"),
        (resize, "window-resize"),
        (workspace, "workspace-switch"),
        (wobble, "window-wobble"),
    ] {
        if modified {
            content = seed_group(&content, &sources, &["animations", name], &desired)?;
        }
    }
    for (modified, name) in [
        (
            changed!(touchpad_tap)
                || changed!(touchpad_natural_scroll)
                || changed!(touchpad_dwt)
                || changed!(touchpad_speed)
                || changed!(touchpad_scroll_factor),
            "touchpad",
        ),
        (
            changed!(mouse_natural_scroll) || changed!(mouse_speed),
            "mouse",
        ),
    ] {
        if modified {
            content = seed_group(&content, &sources, &["input", name], &desired)?;
        }
    }

    pair!(animations_enabled, &["animations"], desired.animations.off);
    scalar!(
        animation_speed,
        &["animations", "slowdown"],
        after.animation_speed,
        desired.animations.slowdown
    );
    for (modified, name, enabled) in [
        (
            changed!(window_open_enabled),
            "window-open",
            after.window_open_enabled,
        ),
        (
            changed!(window_close_enabled),
            "window-close",
            after.window_close_enabled,
        ),
        (movement, "window-movement", after.window_movement_enabled),
        (resize, "window-resize", after.window_resize_enabled),
        (
            changed!(workspace_switch_enabled),
            "workspace-switch",
            after.workspace_switch_enabled,
        ),
    ] {
        if modified {
            content = set_bare_flag(&content, &["animations", name, "off"], !enabled)?;
        }
    }
    if changed!(window_open_enabled) {
        desired.animations.window_open.anim.off = !after.window_open_enabled;
    }
    if changed!(window_close_enabled) {
        desired.animations.window_close.anim.off = !after.window_close_enabled;
    }
    if movement {
        desired.animations.window_movement.0.off = !after.window_movement_enabled;
    }
    if resize {
        desired.animations.window_resize.anim.off = !after.window_resize_enabled;
    }
    if changed!(workspace_switch_enabled) {
        desired.animations.workspace_switch.0.off = !after.workspace_switch_enabled;
    }
    if changed!(window_open_effect) || replace_open_shader {
        desired.animations.window_open.effect = after
            .window_open_effect
            .parse()
            .map_err(anyhow::Error::msg)?;
        content = set_scalar(
            &content,
            &["animations", "window-open", "effect"],
            &serde_json::to_string(&after.window_open_effect)?,
        )?;
        // A shader takes precedence over the built-in effect. Preserve it for
        // enable/disable; an explicit effect selection intentionally replaces it.
        content = remove(&content, &["animations", "window-open", "custom-shader"])?;
        desired.animations.window_open.custom_shader = None;
    }
    if changed!(window_close_effect) || replace_close_shader {
        // An effect-only close node also selects that effect's implicit timing.
        // Keep the currently effective duration/curve when this UI changes only
        // the visual effect, including when removing a previous custom shader.
        content = preserve_close_timing(&content, desired.animations.window_close.anim)?;
        desired.animations.window_close.effect = after
            .window_close_effect
            .parse()
            .map_err(anyhow::Error::msg)?;
        content = set_scalar(
            &content,
            &["animations", "window-close", "effect"],
            &serde_json::to_string(&after.window_close_effect)?,
        )?;
        content = remove(&content, &["animations", "window-close", "custom-shader"])?;
        desired.animations.window_close.custom_shader = None;
    }
    pair!(
        wobble_enabled,
        &["animations", "window-wobble"],
        desired.animations.window_wobble.off
    );
    pair!(
        cube_enabled,
        &["animations", "workspace-switch", "cube"],
        desired.animations.workspace_switch.1.off
    );
    if changed!(glide_enabled) {
        content = set_switch(
            &content,
            &["animations", "window-open", "glide"],
            after.glide_enabled,
        )?;
        desired.animations.window_open.glide.off = !after.glide_enabled;
    }

    scalar!(
        auto_stack,
        &["layout", "auto-stack-new-windows"],
        after.auto_stack,
        desired.layout.auto_stack_new_windows
    );
    scalar!(
        max_windows_per_column,
        &["layout", "max-windows-per-column"],
        after.max_windows_per_column,
        desired.layout.max_windows_per_column
    );
    scalar!(gaps, &["layout", "gaps"], after.gaps, desired.layout.gaps);
    scalar!(
        center_single,
        &["layout", "always-center-single-column"],
        after.center_single,
        desired.layout.always_center_single_column
    );
    scalar!(
        border_width,
        &["layout", "border", "width"],
        after.border_width,
        desired.layout.border.width
    );
    scalar!(
        focus_ring_width,
        &["layout", "focus-ring", "width"],
        after.focus_ring_width,
        desired.layout.focus_ring.width
    );
    scalar!(
        rounding,
        &["layout", "rounding"],
        after.rounding,
        desired.layout.rounding
    );
    pair!(
        border_enabled,
        &["layout", "border"],
        desired.layout.border.off
    );
    if changed!(border_gradient_enabled) || changed!(border_gradient_speed) {
        let speed = after.border_gradient_speed;
        anyhow::ensure!(
            speed.is_finite() && (-3600.0..=3600.0).contains(&speed) && speed != 0.,
            "渐变旋转速度必须是 -3600 到 3600 之间的非零值；停止旋转请关闭开关"
        );
        content = prepare_gradient(
            &content,
            &sources,
            &mut desired,
            after.border_gradient_enabled,
        )?;
        let actual_speed = if after.border_gradient_enabled {
            speed
        } else {
            0.
        };
        content = set_gradient_rotation(&content, actual_speed, speed)?;
        desired
            .layout
            .border
            .active_gradient
            .as_mut()
            .context("请先启用活动边框渐变")?
            .rotate_deg_per_sec = Some(clarion_config::FloatOrInt(actual_speed));
    }
    pair!(
        focus_ring_enabled,
        &["layout", "focus-ring"],
        desired.layout.focus_ring.off
    );
    if changed!(shadow_enabled) {
        content = set_switch(&content, &["layout", "shadow"], after.shadow_enabled)?;
        desired.layout.shadow.on = after.shadow_enabled;
    }
    if changed!(center_column) {
        desired.layout.center_focused_column = match after.center_column.as_str() {
            "never" => clarion_config::layout::CenterFocusedColumn::Never,
            "always" => clarion_config::layout::CenterFocusedColumn::Always,
            "on-overflow" => clarion_config::layout::CenterFocusedColumn::OnOverflow,
            _ => anyhow::bail!("窗口居中选项无效"),
        };
        content = set_scalar(
            &content,
            &["layout", "center-focused-column"],
            &serde_json::to_string(&after.center_column)?,
        )?;
    }
    if changed!(default_width) {
        let node = match after.default_width {
            Some(PresetSize::Proportion(value)) => {
                anyhow::ensure!(value.is_finite() && value > 0., "默认窗口宽度必须大于零");
                format!("default-column-width {{ proportion {value}; }}")
            }
            Some(PresetSize::Fixed(value)) => {
                anyhow::ensure!(value > 0, "默认窗口宽度必须大于零");
                format!("default-column-width {{ fixed {value}; }}")
            }
            None => "default-column-width {}".into(),
        };
        ensure_unique(&content, &["layout", "default-column-width"])?;
        let mut editor = Editor::new(&content)?;
        editor.upsert(&["layout"], "default-column-width", &node)?;
        content = editor.finish();
        desired.layout.default_column_width = after.default_width;
    }
    pair!(blur_enabled, &["blur"], desired.blur.off);
    scalar!(
        blur_passes,
        &["blur", "passes"],
        after.blur_passes,
        desired.blur.passes
    );
    scalar!(
        blur_offset,
        &["blur", "offset"],
        after.blur_offset,
        desired.blur.offset
    );

    for (modified, name, value) in [
        (changed!(touchpad_tap), "tap", after.touchpad_tap),
        (
            changed!(touchpad_natural_scroll),
            "natural-scroll",
            after.touchpad_natural_scroll,
        ),
        (changed!(touchpad_dwt), "dwt", after.touchpad_dwt),
    ] {
        if modified {
            content = set_bare_flag(&content, &["input", "touchpad", name], value)?;
        }
    }
    if changed!(touchpad_tap) {
        desired.input.touchpad.tap = after.touchpad_tap;
    }
    if changed!(touchpad_natural_scroll) {
        desired.input.touchpad.natural_scroll = after.touchpad_natural_scroll;
    }
    if changed!(touchpad_dwt) {
        desired.input.touchpad.dwt = after.touchpad_dwt;
    }
    scalar!(
        touchpad_speed,
        &["input", "touchpad", "accel-speed"],
        after.touchpad_speed,
        desired.input.touchpad.accel_speed.0
    );
    scalar!(
        mouse_speed,
        &["input", "mouse", "accel-speed"],
        after.mouse_speed,
        desired.input.mouse.accel_speed.0
    );
    if changed!(mouse_natural_scroll) {
        content = set_bare_flag(
            &content,
            &["input", "mouse", "natural-scroll"],
            after.mouse_natural_scroll,
        )?;
        desired.input.mouse.natural_scroll = after.mouse_natural_scroll;
    }
    if changed!(touchpad_scroll_factor) {
        // Keep axis-specific overrides; this control changes the common base.
        content = set_scalar(
            &content,
            &["input", "touchpad", "scroll-factor"],
            &after.touchpad_scroll_factor.to_string(),
        )?;
        desired
            .input
            .touchpad
            .scroll_factor
            .get_or_insert_default()
            .base = Some(clarion_config::FloatOrInt(after.touchpad_scroll_factor));
    }
    scalar!(
        keyboard_numlock,
        &["input", "keyboard", "numlock"],
        after.keyboard_numlock,
        desired.input.keyboard.numlock
    );
    scalar!(
        keyboard_repeat_rate,
        &["input", "keyboard", "repeat-rate"],
        after.keyboard_repeat_rate,
        desired.input.keyboard.repeat_rate
    );
    scalar!(
        keyboard_repeat_delay,
        &["input", "keyboard", "repeat-delay"],
        after.keyboard_repeat_delay,
        desired.input.keyboard.repeat_delay
    );

    // At the root, Clarion's legacy decoder implicitly enables a newly added
    // `border { ... }` block. Merely changing its width/gradient must preserve
    // an inherited or default disabled border, rather than turn it on.
    if desired.layout.border.off
        && !Editor::new(manager.raw_content())?.exists(&["layout", "border"])
        && Editor::new(&content)?.exists(&["layout", "border"])
    {
        content = set_bare_flag(&content, &["layout", "border", "off"], true)?;
    }
    let candidate = manager.merged_config(&content)?;
    anyhow::ensure!(
        candidate == desired,
        "设置被后续包含文件覆盖或修改会重置其他参数，原草稿已保留；请检查配置来源与载入顺序"
    );
    anyhow::ensure!(
        manager.effective_source_contents()? == sources,
        "包含文件在修改期间发生变化，原草稿已保留，请重新载入"
    );
    manager.set_raw_content(content);
    Ok(())
}

fn same_group(path: &[&str], left: &Config, right: &Config) -> bool {
    match path {
        ["input", "touchpad"] => left.input.touchpad == right.input.touchpad,
        ["input", "mouse"] => left.input.mouse == right.input.mouse,
        ["animations", name] => same_animation(name, &left.animations, &right.animations),
        _ => false,
    }
}

fn same_animation(name: &str, left: &Animations, right: &Animations) -> bool {
    match name {
        "window-open" => left.window_open == right.window_open,
        "window-close" => left.window_close == right.window_close,
        "window-movement" => left.window_movement == right.window_movement,
        "window-resize" => left.window_resize == right.window_resize,
        "workspace-switch" => left.workspace_switch == right.workspace_switch,
        "window-wobble" => left.window_wobble == right.window_wobble,
        _ => false,
    }
}

fn seed_group(
    source: &str,
    sources: &[(PathBuf, Option<Vec<u8>>)],
    path: &[&str],
    effective: &Config,
) -> Result<String> {
    ensure_unique(source, path)?;
    let mut matching = None;
    for candidate in std::iter::once(source.to_owned()).chain(
        sources
            .iter()
            .filter_map(|(_, bytes)| bytes.as_ref())
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned()),
    ) {
        let document = knuffel::parse_ast::<Span>("config.kdl", &candidate)
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        for parent in document.nodes.iter().filter(|n| &**n.node_name == path[0]) {
            for node in parent.children().filter(|n| &**n.node_name == path[1]) {
                let span = node.span();
                let block = &candidate[span.0..span.1];
                let isolated = Config::parse_mem(&format!("{} {{\n{block}\n}}", path[0]))?;
                if same_group(path, &isolated, effective) {
                    matching = Some(block.to_owned());
                    break;
                }
            }
            if matching.is_some() {
                break;
            }
        }
        if matching.is_some() {
            break;
        }
    }
    let Some(block) = matching else {
        anyhow::ensure!(
            same_group(path, &Config::default(), effective),
            "{} 的完整配置来源无法确定，未覆盖其他参数",
            path.join(".")
        );
        return Ok(source.to_owned());
    };
    let mut editor = Editor::new(source)?;
    if let Some(current) = node_text(source, path)? {
        if current == block {
            return Ok(source.to_owned());
        }
    }
    editor.upsert(&path[..path.len() - 1], path[path.len() - 1], &block)?;
    Ok(editor.finish())
}

fn find_node<'a>(
    nodes: &'a [SpannedNode<Span>],
    path: &[&str],
) -> Result<Option<&'a SpannedNode<Span>>> {
    let Some((name, rest)) = path.split_first() else {
        return Ok(None);
    };
    let mut matches = nodes.iter().filter(|n| &**n.node_name == *name);
    let Some(node) = matches.next() else {
        return Ok(None);
    };
    anyhow::ensure!(
        matches.next().is_none(),
        "配置路径 {} 有重复节点，请在全部配置中合并",
        path.join(".")
    );
    if rest.is_empty() {
        Ok(Some(node))
    } else {
        find_node(
            node.children.as_deref().map(Vec::as_slice).unwrap_or(&[]),
            rest,
        )
    }
}

fn ensure_unique(source: &str, path: &[&str]) -> Result<()> {
    let document =
        knuffel::parse_ast::<Span>("config.kdl", source).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    find_node(&document.nodes, path)?;
    Ok(())
}

fn node_text(source: &str, path: &[&str]) -> Result<Option<String>> {
    let document =
        knuffel::parse_ast::<Span>("config.kdl", source).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    Ok(find_node(&document.nodes, path)?.map(|n| {
        let span = n.span();
        source[span.0..span.1].to_owned()
    }))
}

/// Replace only the argument token: properties, comments and exact unrelated
/// numeric precision survive slider edits (including scroll-factor axis props).
fn set_scalar(source: &str, path: &[&str], value: &str) -> Result<String> {
    let document =
        knuffel::parse_ast::<Span>("config.kdl", source).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    if let Some(node) = find_node(&document.nodes, path)? {
        anyhow::ensure!(
            node.arguments.len() <= 1,
            "设置 {} 有多个参数",
            path.join(".")
        );
        let mut result = source.to_owned();
        if let Some(arg) = node.arguments.first() {
            let span = arg.literal.span();
            result.replace_range(span.0..span.1, value);
        } else {
            let end = node.node_name.span().1;
            result.insert_str(end, &format!(" {value}"));
        }
        return Ok(result);
    }
    let (name, parent) = path.split_last().context("缺少设置路径")?;
    let mut editor = Editor::new(source)?;
    editor.upsert(parent, name, &format!("{name} {value}"))?;
    Ok(editor.finish())
}

fn remove(source: &str, path: &[&str]) -> Result<String> {
    ensure_unique(source, path)?;
    let mut editor = Editor::new(source)?;
    editor.remove(path)?;
    Ok(editor.finish())
}

fn set_bare_flag(source: &str, path: &[&str], value: bool) -> Result<String> {
    ensure_unique(source, path)?;
    let mut editor = Editor::new(source)?;
    if value {
        let (name, parent) = path.split_last().context("缺少开关路径")?;
        if !editor.exists(path) {
            editor.upsert(parent, name, name)?;
        }
    } else {
        editor.remove(path)?;
    }
    Ok(editor.finish())
}

fn set_switch(source: &str, path: &[&str], enabled: bool) -> Result<String> {
    let mut opposite = path.to_vec();
    opposite.push(if enabled { "off" } else { "on" });
    let content = remove(source, &opposite)?;
    let mut selected = path.to_vec();
    selected.push(if enabled { "on" } else { "off" });
    set_bare_flag(&content, &selected, true)
}

const GRADIENT_MEMORY: &str = "clarion-settings: active-border-gradient-speed=";
const GRADIENT_PATH: &[&str] = &["layout", "border", "active-gradient"];

/// Match the complete effective gradient before copying a local override.
/// Inactive and urgent gradients, solid colors and border enablement stay intact.
fn matching_gradient(
    source: &str,
    effective: clarion_config::Gradient,
) -> Result<Option<(String, Option<f64>)>> {
    let document =
        knuffel::parse_ast::<Span>("config.kdl", source).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    for layout in document
        .nodes
        .iter()
        .filter(|node| &**node.node_name == "layout")
    {
        for border in layout
            .children()
            .filter(|node| &**node.node_name == "border")
        {
            for node in border
                .children()
                .filter(|node| &**node.node_name == "active-gradient")
            {
                let span = node.span();
                let block = &source[span.0..span.1];
                let isolated = Config::parse_mem(&format!("layout {{ border {{ {block}\n }}; }}"))?;
                if isolated.layout.border.active_gradient == Some(effective) {
                    let remembered =
                        gradient_memory(source, node.node_name.span().0, border.node_name.span().0)
                            .map(|(_, speed)| speed);
                    return Ok(Some((block.to_owned(), remembered)));
                }
            }
        }
    }
    Ok(None)
}

fn prepare_gradient(
    source: &str,
    sources: &[(PathBuf, Option<Vec<u8>>)],
    desired: &mut Config,
    enabled: bool,
) -> Result<String> {
    ensure_unique(source, GRADIENT_PATH)?;
    let (block, remembered) = if let Some(gradient) = desired.layout.border.active_gradient {
        let mut found = None;
        for candidate in std::iter::once(source.to_owned()).chain(
            sources
                .iter()
                .filter_map(|(_, bytes)| bytes.as_ref())
                .map(|bytes| String::from_utf8_lossy(bytes).into_owned()),
        ) {
            if let Some(matching) = matching_gradient(&candidate, gradient)? {
                found = Some(matching);
                break;
            }
        }
        found.context("活动边框渐变来源无法确定，未修改原配置")?
    } else {
        anyhow::ensure!(enabled, "请先启用活动边框渐变，再调整旋转速度");
        fn color(color: clarion_config::Color) -> String {
            format!(
                "rgba({}, {}, {}, {})",
                f64::from(color.r) * 255.,
                f64::from(color.g) * 255.,
                f64::from(color.b) * 255.,
                color.a
            )
        }
        let block = format!(
            "active-gradient from={} to={} angle=180 rotate-deg-per-sec=0",
            serde_json::to_string(&color(desired.layout.border.active_color))?,
            serde_json::to_string(&color(desired.layout.border.inactive_color))?
        );
        let isolated = Config::parse_mem(&format!("layout {{ border {{ {block}; }}; }}"))?;
        desired.layout.border.active_gradient = isolated.layout.border.active_gradient;
        (block, None)
    };
    if node_text(source, GRADIENT_PATH)?.as_deref() == Some(block.as_str()) {
        return Ok(source.to_owned());
    }
    let mut editor = Editor::new(source)?;
    editor.upsert(&["layout", "border"], "active-gradient", &block)?;
    let mut content = editor.finish();
    // Include-source metadata belongs to the copied gradient, not to unrelated
    // local comments. It remains data, and is never executed or interpreted.
    if let Some(speed) = remembered {
        content = set_gradient_memory(&content, speed)?;
    }
    Ok(content)
}

fn set_gradient_rotation(source: &str, rotation: f64, remembered: f64) -> Result<String> {
    let document =
        knuffel::parse_ast::<Span>("config.kdl", source).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let node = find_node(&document.nodes, GRADIENT_PATH)?.context("找不到活动边框渐变")?;
    let mut content = source.to_owned();
    if let Some((_, value)) = node
        .properties
        .iter()
        .find(|(name, _)| &****name == "rotate-deg-per-sec")
    {
        let span = value.literal.span();
        content.replace_range(span.0..span.1, &rotation.to_string());
    } else {
        content.insert_str(
            node.node_name.span().1,
            &format!(" rotate-deg-per-sec={rotation}"),
        );
    }
    set_gradient_memory(&content, remembered)
}

fn set_gradient_memory(source: &str, speed: f64) -> Result<String> {
    let document =
        knuffel::parse_ast::<Span>("config.kdl", source).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let node = find_node(&document.nodes, GRADIENT_PATH)?.context("找不到活动边框渐变")?;
    let border = find_node(&document.nodes, &["layout", "border"])?.context("找不到边框设置")?;
    let start = node.node_name.span().0;
    let mut content = source.to_owned();
    if let Some((span, _)) = gradient_memory(source, start, border.node_name.span().0) {
        content.replace_range(span, &format!("// {GRADIENT_MEMORY}{speed}"));
    } else {
        let line_start = source[..start].rfind('\n').map_or(0, |n| n + 1);
        let prefix = &source[line_start..start];
        let indent = if prefix.trim().is_empty() {
            prefix
        } else {
            "    "
        };
        content.insert_str(
            start,
            &format!("\n{indent}// {GRADIENT_MEMORY}{speed}\n{indent}"),
        );
    }
    Ok(content)
}

fn gradient_memory(
    source: &str,
    gradient_start: usize,
    border_start: usize,
) -> Option<(std::ops::Range<usize>, f64)> {
    real_line_comments(source)
        .into_iter()
        .rev()
        .find_map(|span| {
            if span.start < border_start
                || span.end > gradient_start
                || !source[span.end..gradient_start].trim().is_empty()
            {
                return None;
            }
            let text = source[span.start + 2..span.end].trim();
            let speed = text.strip_prefix(GRADIENT_MEMORY)?.parse::<f64>().ok()?;
            (speed.is_finite() && speed != 0. && (-3600.0..=3600.0).contains(&speed))
                .then_some((span, speed))
        })
}

/// Identify actual line comments, skipping quoted/raw KDL strings and nested
/// block comments. A shader or startup argument containing a fake marker must
/// never become remembered settings data.
fn real_line_comments(source: &str) -> Vec<std::ops::Range<usize>> {
    let bytes = source.as_bytes();
    let mut found = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i = (i + 2).min(bytes.len());
                } else if bytes[i] == b'"' {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
        } else if bytes[i] == b'r' {
            let mut quote = i + 1;
            while quote < bytes.len() && bytes[quote] == b'#' {
                quote += 1;
            }
            if bytes.get(quote) == Some(&b'"') {
                let terminator = format!("\"{}", "#".repeat(quote - i - 1));
                i = source[quote + 1..]
                    .find(&terminator)
                    .map_or(bytes.len(), |end| quote + 1 + end + terminator.len());
            } else {
                i += 1;
            }
        } else if bytes[i..].starts_with(b"/*") {
            i += 2;
            let mut depth = 1;
            while i < bytes.len() && depth > 0 {
                if bytes[i..].starts_with(b"/*") {
                    depth += 1;
                    i += 2;
                } else if bytes[i..].starts_with(b"*/") {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
        } else if bytes[i..].starts_with(b"//") {
            let start = i;
            i = source[i..].find('\n').map_or(bytes.len(), |end| i + end);
            found.push(start..i);
        } else {
            i += 1;
        }
    }
    found
}

fn preserve_close_timing(source: &str, animation: Animation) -> Result<String> {
    let mut editor = Editor::new(source)?;
    let parent = &["animations", "window-close"];
    if ["duration-ms", "curve", "spring"].iter().any(|name| {
        let mut path = parent.to_vec();
        path.push(name);
        editor.exists(&path)
    }) {
        return Ok(source.to_owned());
    }
    match animation.kind {
        Kind::Easing(params) => {
            editor.upsert(
                parent,
                "duration-ms",
                &format!("duration-ms {}", params.duration_ms),
            )?;
            let curve = match params.curve {
                Curve::Linear => "\"linear\"".into(),
                Curve::EaseOutQuad => "\"ease-out-quad\"".into(),
                Curve::EaseOutCubic => "\"ease-out-cubic\"".into(),
                Curve::EaseOutExpo => "\"ease-out-expo\"".into(),
                Curve::CubicBezier(a, b, c, d) => format!("\"cubic-bezier\" {a} {b} {c} {d}"),
            };
            editor.upsert(parent, "curve", &format!("curve {curve}"))?;
        }
        Kind::Spring(params) => editor.upsert(
            parent,
            "spring",
            &format!(
                "spring damping-ratio={} stiffness={} epsilon={}",
                params.damping_ratio, params.stiffness, params.epsilon,
            ),
        )?,
        Kind::WorkspaceFeedback => anyhow::bail!("工作区反馈不能用作窗口关闭动画"),
    }
    Ok(editor.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn manager(source: &str) -> (tempfile::TempDir, ConfigManager) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.kdl");
        fs::write(&path, source).unwrap();
        let manager = ConfigManager::from_path(&path).unwrap();
        (dir, manager)
    }

    #[test]
    fn no_change_keeps_every_byte_and_precision() {
        let source = "// exact user settings\nlayout { gaps 6.500000000003; border { width 2.1234567890123; }; }\n";
        let (_dir, mut manager) = manager(source);
        let before = read(&manager).unwrap();
        apply(&mut manager, &before, &before).unwrap();
        assert_eq!(manager.raw_content(), source);
        assert!(!manager.is_dirty());
    }

    #[test]
    fn only_changed_scalar_is_written_and_inline_comment_survives() {
        let source =
            "layout { gaps /* fine */ 6.500000000003; border { width 2.1234567890123; }; }\n";
        let (_dir, mut manager) = manager(source);
        let before = read(&manager).unwrap();
        let mut after = before.clone();
        after.gaps = 9.;
        apply(&mut manager, &before, &after).unwrap();
        assert_eq!(manager.raw_content(), source.replace("6.500000000003", "9"));
        assert_eq!(read(&manager).unwrap(), after);
    }

    #[test]
    fn inherited_animation_toggle_keeps_shader_curve_and_reenables() {
        let (dir, mut manager) = manager("include \"base.kdl\"\n");
        fs::write(dir.path().join("base.kdl"), "animations { window-open { // retained curve\n duration-ms 713; curve \"cubic-bezier\" 0.12 0.234567890123 0.8 1.0; popin 0.91; custom-shader \"kept shader\"; glide { off; tilt-deg 7; }; }; }\n").unwrap();
        let original = manager.effective_config().unwrap().animations.window_open;
        let before = read(&manager).unwrap();
        let mut off = before.clone();
        off.window_open_enabled = false;
        apply(&mut manager, &before, &off).unwrap();
        assert!(manager.raw_content().contains("// retained curve"));
        assert!(manager.raw_content().contains("0.234567890123"));
        apply(&mut manager, &off, &before).unwrap();
        assert_eq!(
            manager.effective_config().unwrap().animations.window_open,
            original
        );
    }

    #[test]
    fn inherited_touchpad_and_mouse_keep_advanced_fields() {
        let (dir, mut manager) = manager("include \"input.kdl\"\n");
        fs::write(dir.path().join("input.kdl"), "input { touchpad { tap; dwt; natural-scroll; accel-speed 0.1234567890123; click-method \"clickfinger\"; left-handed; scroll-factor 1.1 horizontal=0.8 vertical=1.4; }; mouse { natural-scroll; middle-emulation; accel-profile \"flat\"; accel-speed 0.234567890123; }; }\n").unwrap();
        let before_config = manager.effective_config().unwrap();
        let before = read(&manager).unwrap();
        let mut after = before.clone();
        after.touchpad_tap = false;
        after.mouse_natural_scroll = false;
        apply(&mut manager, &before, &after).unwrap();
        let actual = manager.effective_config().unwrap();
        let mut desired_touchpad = before_config.input.touchpad;
        desired_touchpad.tap = false;
        let mut desired_mouse = before_config.input.mouse;
        desired_mouse.natural_scroll = false;
        assert_eq!(actual.input.touchpad, desired_touchpad);
        assert_eq!(actual.input.mouse, desired_mouse);
        assert!(manager.raw_content().contains("0.1234567890123"));
    }

    #[test]
    fn flag_false_overrides_inherited_true_without_other_overrides() {
        let (dir, mut manager) = manager("include \"base.kdl\"\n");
        fs::write(dir.path().join("base.kdl"), "layout { auto-stack-new-windows; always-center-single-column; }\ninput { keyboard { numlock; }; }\n").unwrap();
        let before = read(&manager).unwrap();
        let mut after = before.clone();
        after.auto_stack = false;
        after.center_single = false;
        after.keyboard_numlock = false;
        apply(&mut manager, &before, &after).unwrap();
        for flag in [
            "auto-stack-new-windows false",
            "always-center-single-column false",
            "numlock false",
        ] {
            assert!(manager.raw_content().contains(flag));
        }
        assert_eq!(read(&manager).unwrap(), after);
    }

    #[test]
    fn later_include_override_is_error_without_draft_mutation() {
        let (dir, mut manager) = manager("layout { gaps 5; }\ninclude \"later.kdl\"\n");
        fs::write(dir.path().join("later.kdl"), "layout { gaps 9; }\n").unwrap();
        let original = manager.raw_content().to_owned();
        let before = read(&manager).unwrap();
        let mut after = before.clone();
        after.gaps = 12.;
        assert!(apply(&mut manager, &before, &after).is_err());
        assert_eq!(manager.raw_content(), original);
    }

    #[test]
    fn stale_changed_field_rejected_but_unrelated_external_field_kept() {
        let (dir, mut manager) = manager("include \"base.kdl\"\n");
        let included = dir.path().join("base.kdl");
        fs::write(&included, "layout { gaps 5; rounding 12; }\n").unwrap();
        let before = read(&manager).unwrap();
        let mut after = before.clone();
        after.gaps = 12.;
        fs::write(&included, "layout { gaps 7; rounding 12; }\n").unwrap();
        assert!(apply(&mut manager, &before, &after).is_err());
        fs::write(&included, "layout { gaps 5; rounding 22; }\n").unwrap();
        apply(&mut manager, &before, &after).unwrap();
        assert_eq!(manager.effective_config().unwrap().layout.rounding, 22.);
    }

    #[test]
    fn scroll_base_preserves_per_axis_overrides() {
        let (_dir, mut manager) =
            manager("input { touchpad { scroll-factor 1.1 horizontal=0.8 vertical=1.4; }; }\n");
        let before = read(&manager).unwrap();
        let mut after = before.clone();
        after.touchpad_scroll_factor = 1.7;
        apply(&mut manager, &before, &after).unwrap();
        assert!(manager
            .raw_content()
            .contains("scroll-factor 1.7 horizontal=0.8 vertical=1.4"));
    }

    #[test]
    fn invalid_values_keep_original_draft() {
        let (_dir, mut manager) = manager("// unchanged\n");
        let before = read(&manager).unwrap();
        for value in [f64::NAN, f64::INFINITY, -3.] {
            let mut after = before.clone();
            after.gaps = value;
            assert!(apply(&mut manager, &before, &after).is_err());
            assert_eq!(manager.raw_content(), "// unchanged\n");
        }
    }

    #[test]
    fn switching_builtin_effect_keeps_current_timing() {
        let (_dir, mut manager) = manager("animations { window-close { effect \"implode\"; }; }\n");
        let original = manager
            .effective_config()
            .unwrap()
            .animations
            .window_close
            .anim;
        let before = read(&manager).unwrap();
        let mut after = before.clone();
        after.window_close_effect = "fade".into();
        apply(&mut manager, &before, &after).unwrap();
        assert_eq!(
            manager
                .effective_config()
                .unwrap()
                .animations
                .window_close
                .anim,
            original
        );
        assert_eq!(read(&manager).unwrap(), after);
    }

    #[test]
    fn common_controls_round_trip_without_resetting_other_sections() {
        let (_dir, mut manager) = manager("// keep startup command\nspawn-at-startup \"foot\"\nlayout { background-color \"#112233\"; }\ninput { keyboard { xkb { layout \"us\"; }; }; }\n");
        let before = read(&manager).unwrap();
        let mut after = before.clone();
        after.animations_enabled = false;
        after.animation_speed = 0.8;
        after.window_open_enabled = false;
        after.window_close_enabled = false;
        after.window_movement_enabled = false;
        after.window_resize_enabled = false;
        after.workspace_switch_enabled = false;
        after.window_open_effect = "reveal".into();
        after.window_close_effect = "fade".into();
        after.wobble_enabled = false;
        after.cube_enabled = false;
        after.glide_enabled = true;
        after.auto_stack = false;
        after.max_windows_per_column = 5;
        after.default_width = Some(PresetSize::Fixed(900));
        after.gaps = 8.5;
        after.center_column = "on-overflow".into();
        after.center_single = true;
        after.border_enabled = true;
        after.focus_ring_enabled = false;
        after.shadow_enabled = true;
        after.border_width = 3.;
        after.focus_ring_width = 2.;
        after.rounding = 12.;
        after.blur_enabled = false;
        after.blur_passes = 4;
        after.blur_offset = 2.;
        after.touchpad_tap = true;
        after.touchpad_natural_scroll = true;
        after.touchpad_dwt = true;
        after.touchpad_speed = 0.25;
        after.touchpad_scroll_factor = 1.25;
        after.mouse_natural_scroll = true;
        after.mouse_speed = -0.25;
        after.keyboard_numlock = true;
        after.keyboard_repeat_rate = 35;
        after.keyboard_repeat_delay = 300;
        apply(&mut manager, &before, &after).unwrap();
        assert_eq!(read(&manager).unwrap(), after);
        assert!(manager.raw_content().contains("spawn-at-startup \"foot\""));
        assert!(manager.raw_content().contains("// keep startup command"));
        assert!(manager
            .raw_content()
            .contains("background-color \"#112233\""));
        assert!(manager.raw_content().contains("layout \"us\""));
        // Exercise the opposite direction as well: explicit false Flags and
        // paired on/off nodes must restore, rather than latch the disabled state.
        apply(&mut manager, &after, &before).unwrap();
        assert_eq!(read(&manager).unwrap(), before);
    }

    #[test]
    fn duplicate_edited_blocks_fail_without_touching_draft() {
        let source = "layout { gaps 5; }\nlayout { rounding 12; }\n";
        let (_dir, mut manager) = manager(source);
        assert!(read(&manager).is_err());
        assert!(ensure_unique(source, &["layout", "gaps"]).is_err());
        // The compositor rejects duplicate singleton blocks before structured
        // edits are possible. A stale previously valid form must also fail.
        let before = CommonSettings::default();
        let mut after = before.clone();
        after.gaps = 9.;
        assert!(apply(&mut manager, &before, &after).is_err());
        assert_eq!(manager.raw_content(), source);
    }

    #[test]
    fn same_builtin_effect_can_explicitly_replace_shader_and_false_to_true_is_rejected() {
        let source = "animations { window-open { effect \"fade\"; custom-shader \"custom open\"; }; window-close { effect \"fade\"; custom-shader \"custom close\"; duration-ms 763; }; }\n";
        let (_dir, mut manager) = manager(source);
        let before = read(&manager).unwrap();
        assert!(before.open_custom_shader && before.close_custom_shader);
        let mut after = before.clone();
        after.open_custom_shader = false;
        after.close_custom_shader = false;
        apply(&mut manager, &before, &after).unwrap();
        assert_eq!(read(&manager).unwrap(), after);
        assert!(manager.raw_content().contains("duration-ms 763"));
        let replaced = manager.raw_content().to_owned();
        assert!(apply(&mut manager, &after, &before).is_err());
        assert_eq!(manager.raw_content(), replaced);
    }

    #[test]
    fn gradient_toggle_across_fresh_managers_preserves_custom_colors_and_speed() {
        let source = "layout { border {\n // custom active gradient\n active-gradient from=\"rgba(12.125, 34.5, 56.75, 0.731)\" to=\"#abcdef91\" angle=271 relative-to=\"workspace-view\" in=\"oklch longer hue\" rotate-deg-per-sec=-63.123456789\n inactive-gradient from=\"#111111\" to=\"#222222\" rotate-deg-per-sec=17\n urgent-color \"#ff9955\"\n }; }\n";
        let (dir, mut manager) = manager(source);
        let original = manager.effective_config().unwrap().layout.border;
        let before = read(&manager).unwrap();
        assert!(before.border_gradient_enabled);
        let mut off = before.clone();
        off.border_gradient_enabled = false;
        apply(&mut manager, &before, &off).unwrap();
        let mut stopped = original;
        stopped.active_gradient.as_mut().unwrap().rotate_deg_per_sec =
            Some(clarion_config::FloatOrInt(0.));
        assert_eq!(manager.effective_config().unwrap().layout.border, stopped);
        assert!(manager.raw_content().contains("// custom active gradient"));
        assert!(manager
            .raw_content()
            .contains("from=\"rgba(12.125, 34.5, 56.75, 0.731)\""));
        let path = dir.path().join("config.kdl");
        fs::write(&path, manager.raw_content()).unwrap();
        let mut fresh = ConfigManager::from_path(&path).unwrap();
        let remembered = read(&fresh).unwrap();
        assert_eq!(remembered, off);
        apply(&mut fresh, &remembered, &before).unwrap();
        assert_eq!(fresh.effective_config().unwrap().layout.border, original);
        assert_eq!(read(&fresh).unwrap(), before);
    }

    #[test]
    fn inherited_gradient_override_keeps_other_border_values_and_fresh_memory() {
        let (dir, mut manager) = manager("include \"base.kdl\"\n");
        fs::write(dir.path().join("base.kdl"), "layout { border { off; width 2.123456789; active-gradient from=\"#123456\" to=\"#654321\" angle=75 relative-to=\"workspace-view\" in=\"srgb-linear\" rotate-deg-per-sec=120; inactive-color \"#13579b\"; }; }\n").unwrap();
        let original = manager.effective_config().unwrap().layout.border;
        let before = read(&manager).unwrap();
        let mut after = before.clone();
        after.border_gradient_enabled = false;
        apply(&mut manager, &before, &after).unwrap();
        let actual = manager.effective_config().unwrap().layout.border;
        assert_eq!(actual.active_color, original.active_color);
        assert_eq!(actual.inactive_color, original.inactive_color);
        assert_eq!(actual.width, original.width);
        assert!(actual.off);
        fs::write(dir.path().join("config.kdl"), manager.raw_content()).unwrap();
        let mut fresh = ConfigManager::from_path(&dir.path().join("config.kdl")).unwrap();
        assert_eq!(read(&fresh).unwrap(), after);
        apply(&mut fresh, &after, &before).unwrap();
        assert_eq!(fresh.effective_config().unwrap().layout.border, original);
    }

    #[test]
    fn generated_gradient_uses_effective_colors_without_enabling_border() {
        let (_dir, mut manager) = manager("layout { border { off; active-color \"rgba(17.125, 33.5, 91.25, 0.713)\"; inactive-color \"#12345687\"; urgent-gradient from=\"#f00\" to=\"#0f0\"; }; }\n");
        let original = manager.effective_config().unwrap().layout.border;
        let before = read(&manager).unwrap();
        let mut after = before.clone();
        after.border_gradient_enabled = true;
        apply(&mut manager, &before, &after).unwrap();
        let actual = manager.effective_config().unwrap().layout.border;
        assert!(actual.off);
        let gradient = actual.active_gradient.unwrap();
        assert_eq!(gradient.from, original.active_color);
        assert_eq!(gradient.to, original.inactive_color);
        assert_eq!(actual.urgent_gradient, original.urgent_gradient);
        assert_eq!(actual.active_color, original.active_color);
        assert_eq!(actual.inactive_color, original.inactive_color);
        assert_eq!(read(&manager).unwrap(), after);
    }

    #[test]
    fn gradient_memory_only_reads_real_adjacent_kdl_line_comment() {
        let source = r###"spawn-at-startup r##"// clarion-settings: active-border-gradient-speed=999"##
animations { window-open { custom-shader "// clarion-settings: active-border-gradient-speed=888"; }; }
/* nested /* // clarion-settings: active-border-gradient-speed=777 */ comment */
layout { border {
// clarion-settings: active-border-gradient-speed=-23.456789
active-gradient from="#123" to="#456" rotate-deg-per-sec=0
}; }
"###;
        let (_dir, manager) = manager(source);
        let settings = read(&manager).unwrap();
        assert_eq!(settings.border_gradient_speed, -23.456789);
        assert!(!settings.border_gradient_enabled);
        assert_eq!(real_line_comments(source).len(), 1);
    }

    #[test]
    fn materializing_border_width_keeps_inherited_disabled_state() {
        let (dir, mut manager) = manager("include \"base.kdl\"\n");
        fs::write(
            dir.path().join("base.kdl"),
            "layout { border { off; active-color \"#123456\"; }; }\n",
        )
        .unwrap();
        let before = read(&manager).unwrap();
        let mut after = before.clone();
        after.border_width = 7.5;
        apply(&mut manager, &before, &after).unwrap();
        assert_eq!(read(&manager).unwrap(), after);
        let actual = manager.effective_config().unwrap().layout.border;
        assert!(actual.off);
        assert_eq!(actual.width, 7.5);
        assert_eq!(actual.active_color, "#123456".parse().unwrap());
    }
}
