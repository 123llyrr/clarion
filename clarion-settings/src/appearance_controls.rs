//! Explicit, lossless appearance overrides for the current configuration file.
//!
//! UI inspiration: Denial's settings_animations_page.dart and
//! settings_appearance_page.dart at 0fccf2c68ea560402153491a4efeabe6caefaa4e.
//! Independently implemented for Clarion's KDL schema; no Denial source is copied.
use anyhow::{Context, Result};
use clarion_config::animations::{Curve, Kind, WindowCloseAnim};
use clarion_config::{OverviewStyle, WindowCloseEffect};

use crate::kdl_edit::Editor;

const CLOSE: &[&str] = &["animations", "window-close"];

pub fn overview_style_label(style: OverviewStyle) -> &'static str {
    match style {
        OverviewStyle::Stage => "空间舞台",
        OverviewStyle::Strip => "经典纵向概览",
    }
}

pub fn read_overview_style(source: &str) -> Result<Option<OverviewStyle>> {
    ensure_unique_path(source, &["overview"], &["style"])?;
    Editor::new(source)?
        .argument(&["overview", "style"])
        .map(|value| match value.as_str() {
            "stage" => Ok(OverviewStyle::Stage),
            "strip" => Ok(OverviewStyle::Strip),
            _ => anyhow::bail!("概览风格无效，请在全部设置修正 overview.style"),
        })
        .transpose()
}

/// Change only the style argument; retain comments inside the existing node.
/// Overview merges individual fields, so inherited zoom and shadows stay intact.
pub fn set_overview_style(source: &str, style: Option<OverviewStyle>) -> Result<String> {
    let before = read_overview_style(source)?;
    if before == style {
        return Ok(source.into());
    }
    let Some(style) = style else {
        let mut editor = Editor::new(source)?;
        editor.remove(&["overview", "style"])?;
        return Ok(editor.finish());
    };
    let value = match style {
        OverviewStyle::Stage => "\"stage\"",
        OverviewStyle::Strip => "\"strip\"",
    };
    if before.is_some() {
        let document = knuffel::parse_ast::<knuffel::span::Span>("config.kdl", source)
            .map_err(|error| anyhow::anyhow!("{error:?}"))?;
        let node = document
            .nodes
            .iter()
            .find(|node| &**node.node_name == "overview")
            .and_then(|node| node.children().find(|node| &**node.node_name == "style"))
            .context("找不到概览风格字段")?;
        let span = node.arguments[0].literal.span();
        let mut next = source.to_owned();
        next.replace_range(span.0..span.1, value);
        return Ok(next);
    }
    let mut editor = Editor::new(source)?;
    editor.upsert(&["overview"], "style", &format!("style {value}"))?;
    Ok(editor.finish())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseEffect {
    Explosion,
    Implode,
    Fade,
    Disabled,
}

impl CloseEffect {
    pub const ALL: [Self; 4] = [Self::Explosion, Self::Implode, Self::Fade, Self::Disabled];

    pub fn label(self) -> &'static str {
        match self {
            Self::Explosion => "粒子爆散",
            Self::Implode => "向内收缩",
            Self::Fade => "淡出",
            Self::Disabled => "无关闭动画",
        }
    }

    fn value(self) -> &'static str {
        match self {
            Self::Explosion => "explosion",
            Self::Implode => "implode",
            Self::Fade => "fade",
            Self::Disabled => unreachable!("disabled is represented by window-close/off"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CloseState {
    pub selected: Option<CloseEffect>,
    pub custom_shader: bool,
    pub local_config: bool,
}

impl CloseState {
    pub fn read(source: &str) -> Result<Self> {
        ensure_unique_path(source, CLOSE, &["effect", "off", "custom-shader"])?;
        let editor = Editor::new(source)?;
        let custom_shader = editor.exists(&["animations", "window-close", "custom-shader"]);
        let selected = if editor.exists(&["animations", "window-close", "off"]) {
            Some(CloseEffect::Disabled)
        } else if custom_shader {
            None
        } else {
            editor
                .argument(&["animations", "window-close", "effect"])
                .map(|value| value.parse::<WindowCloseEffect>())
                .transpose()
                .map_err(anyhow::Error::msg)?
                .map(|effect| match effect {
                    WindowCloseEffect::Explosion => CloseEffect::Explosion,
                    WindowCloseEffect::Implode => CloseEffect::Implode,
                    WindowCloseEffect::Fade => CloseEffect::Fade,
                })
        };
        Ok(Self {
            selected,
            custom_shader,
            local_config: editor.exists(CLOSE),
        })
    }
}

pub fn set_close_effect_preserving_inheritance(
    source: &str,
    choice: CloseEffect,
    inherited: Option<&WindowCloseAnim>,
) -> Result<String> {
    let state = CloseState::read(source)?;
    if state.local_config {
        return set_close_effect(source, choice);
    }
    let Some(inherited) = inherited else {
        return set_close_effect(source, choice);
    };

    // Clarion merges window-close as a whole rather than field by field. A new
    // local effect-only node would reset the inherited timing, shader and Glide.
    let mut editor = Editor::new(source)?;
    editor.upsert(
        &["animations"],
        "window-close",
        &serialize_close(inherited)?,
    )?;
    set_close_effect(&editor.finish(), choice)
}

pub fn contains_close_config(source: &str) -> Result<bool> {
    let document = knuffel::parse_ast::<knuffel::span::Span>("config.kdl", source)
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    Ok(document.nodes.iter().any(|node| {
        &**node.node_name == "animations"
            && node
                .children()
                .any(|child| &**child.node_name == "window-close")
    }))
}

/// Preserve an inherited animation's complete source, including comments,
/// shader strings, independent Glide values and arbitrary CSS color precision.
pub(crate) fn animation_blocks<'a>(source: &'a str, name: &str) -> Result<Vec<&'a str>> {
    let document = knuffel::parse_ast::<knuffel::span::Span>("config.kdl", source)
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    Ok(document
        .nodes
        .iter()
        .filter(|node| &**node.node_name == "animations")
        .flat_map(|node| node.children())
        .filter(|node| &**node.node_name == name)
        .map(|node| {
            let span = node.span();
            &source[span.0..span.1]
        })
        .collect())
}

fn serialize_close(close: &WindowCloseAnim) -> Result<String> {
    let mut result = String::from("window-close {\n");
    if close.anim.off {
        result.push_str("    off\n");
    }
    match close.anim.kind {
        Kind::WorkspaceFeedback => {
            anyhow::bail!("workspace motion cannot be used for window closing")
        }
        Kind::Easing(params) => {
            result.push_str(&format!("    duration-ms {}\n", params.duration_ms));
            let curve = match params.curve {
                Curve::Linear => "\"linear\"".into(),
                Curve::EaseOutQuad => "\"ease-out-quad\"".into(),
                Curve::EaseOutCubic => "\"ease-out-cubic\"".into(),
                Curve::EaseOutExpo => "\"ease-out-expo\"".into(),
                Curve::CubicBezier(a, b, c, d) => format!("\"cubic-bezier\" {a} {b} {c} {d}"),
            };
            result.push_str(&format!("    curve {curve}\n"));
        }
        Kind::Spring(params) => result.push_str(&format!(
            "    spring damping-ratio={} stiffness={} epsilon={}\n",
            params.damping_ratio, params.stiffness, params.epsilon,
        )),
    }
    let effect = match close.effect {
        WindowCloseEffect::Explosion => "explosion",
        WindowCloseEffect::Implode => "implode",
        WindowCloseEffect::Fade => "fade",
    };
    result.push_str(&format!("    effect \"{effect}\"\n"));
    if let Some(shader) = &close.custom_shader {
        result.push_str(&format!(
            "    custom-shader {}\n",
            serde_json::to_string(shader)?
        ));
    }
    result.push_str("    glide {\n");
    if close.glide.off {
        result.push_str("        off\n");
    }
    result.push_str(&format!(
        "        tilt-deg {}\n        depth {}\n    }}\n}}",
        close.glide.tilt_deg, close.glide.depth
    ));
    Ok(result)
}

pub fn set_close_effect(source: &str, choice: CloseEffect) -> Result<String> {
    let state = CloseState::read(source)?;
    if state.selected == Some(choice) {
        return Ok(source.into());
    }
    let mut editor = Editor::new(source)?;
    if choice == CloseEffect::Disabled {
        // Keep the shader, timing, and built-in choice so disabling is reversible.
        editor.upsert(CLOSE, "off", "off")?;
    } else {
        editor.remove(&["animations", "window-close", "off"])?;
        // Custom shaders take priority over built-in effects. The UI explains
        // this replacement before the user selects a built-in effect.
        editor.remove(&["animations", "window-close", "custom-shader"])?;
        editor.upsert(CLOSE, "effect", &format!("effect \"{}\"", choice.value()))?;
    }
    Ok(editor.finish())
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlurOverrides {
    pub enabled: Option<bool>,
    pub passes: Option<u8>,
    pub offset: Option<f64>,
    pub noise: Option<f64>,
    pub saturation: Option<f64>,
}

impl BlurOverrides {
    pub fn read(source: &str) -> Result<Self> {
        ensure_unique_path(
            source,
            &["blur"],
            &["on", "off", "passes", "offset", "noise", "saturation"],
        )?;
        let editor = Editor::new(source)?;
        let number = |name: &str| -> Result<Option<f64>> {
            editor
                .argument(&["blur", name])
                .map(|value| {
                    value
                        .parse()
                        .with_context(|| format!("blur/{name} 需要数值"))
                })
                .transpose()
        };
        Ok(Self {
            enabled: if editor.exists(&["blur", "on"]) {
                Some(true)
            } else if editor.exists(&["blur", "off"]) {
                Some(false)
            } else {
                None
            },
            passes: editor
                .argument(&["blur", "passes"])
                .map(|value| value.parse().context("blur/passes 需要 0 至 255 的整数"))
                .transpose()?,
            offset: number("offset")?,
            noise: number("noise")?,
            saturation: number("saturation")?,
        })
    }

    pub fn apply(&self, source: &str) -> Result<String> {
        let previous = Self::read(source)?;
        let mut editor = Editor::new(source)?;
        if self.enabled != previous.enabled {
            editor.remove(&["blur", "on"])?;
            editor.remove(&["blur", "off"])?;
            if let Some(enabled) = self.enabled {
                let name = if enabled { "on" } else { "off" };
                editor.upsert(&["blur"], name, name)?;
            }
        }
        update_number(&mut editor, "passes", previous.passes, self.passes)?;
        for (name, before, after) in [
            ("offset", previous.offset, self.offset),
            ("noise", previous.noise, self.noise),
            ("saturation", previous.saturation, self.saturation),
        ] {
            update_number(&mut editor, name, before, after)?;
        }
        Ok(editor.finish())
    }
}

fn update_number<T: PartialEq + std::fmt::Display>(
    editor: &mut Editor,
    name: &str,
    before: Option<T>,
    after: Option<T>,
) -> Result<()> {
    if before != after {
        if let Some(value) = after {
            editor.upsert(&["blur"], name, &format!("{name} {value}"))?;
        } else {
            editor.remove(&["blur", name])?;
        }
    }
    Ok(())
}

// KDL allows repeated top-level sections for ordered merges. A simple control
// must not silently edit the first of several conflicting overrides.
fn ensure_unique_path(source: &str, path: &[&str], fields: &[&str]) -> Result<()> {
    let document = knuffel::parse_ast::<knuffel::span::Span>("config.kdl", source)
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    let mut nodes = document.nodes.as_slice();
    for name in path {
        let mut matches = nodes.iter().filter(|node| &**node.node_name == *name);
        let Some(node) = matches.next() else {
            return Ok(());
        };
        anyhow::ensure!(
            matches.next().is_none(),
            "本文件有多处 {name} 配置，请在全部配置中选择要编辑的项"
        );
        nodes = node
            .children
            .as_ref()
            .map(|children| children.as_slice())
            .unwrap_or_default();
    }
    for name in fields {
        anyhow::ensure!(
            nodes
                .iter()
                .filter(|node| &**node.node_name == *name)
                .count()
                <= 1,
            "本文件有多处 {name} 配置，请在全部配置中选择要编辑的项"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use clarion_config::Config;

    use super::*;

    #[test]
    fn overview_choice_preserves_inline_comments_animation_and_other_fields() {
        let source = "// retained header\noverview { style /* retain rationale */ \"strip\"; zoom 0.37123456789; workspace-shadow { softness 42.5; }; }\nanimations { off; overview-open-close { duration-ms 713; }; window-close { custom-shader r#\"shader { }\"#; }; }\n";
        let before = Config::parse_mem(source).unwrap();
        let next = set_overview_style(source, Some(OverviewStyle::Stage)).unwrap();
        assert_eq!(next, source.replacen("\"strip\"", "\"stage\"", 1));
        assert_eq!(
            set_overview_style(&next, Some(OverviewStyle::Stage)).unwrap(),
            next
        );
        let after = Config::parse_mem(&next).unwrap();
        assert_eq!(after.overview.style, OverviewStyle::Stage);
        assert_eq!(after.overview.zoom, before.overview.zoom);
        assert_eq!(
            after.overview.workspace_shadow,
            before.overview.workspace_shadow
        );
        assert_eq!(after.animations, before.animations);
        let inherited = set_overview_style(&next, None).unwrap();
        assert_eq!(read_overview_style(&inherited).unwrap(), None);
        assert!(inherited.contains("zoom 0.37123456789"));
        assert!(inherited.contains("// retained header"));
        assert_eq!(
            Config::parse_mem(&inherited).unwrap().animations,
            before.animations
        );
    }

    #[test]
    fn overview_choice_rejects_ambiguous_or_invalid_local_nodes() {
        for source in [
            "overview { style \"strip\"; }\noverview { zoom 0.4; }\n",
            "overview { style \"strip\"; style \"stage\"; }\n",
            "overview { style \"expo\"; }\n",
        ] {
            assert!(set_overview_style(source, Some(OverviewStyle::Stage)).is_err());
        }
    }

    #[test]
    fn built_in_close_choice_preserves_timing_glide_open_and_comments() {
        let source = "// original\nanimations { off; window-open { duration-ms 321; custom-shader r#\"open() { }\"#; }; window-close { off; /* timing */ duration-ms 431; curve \"ease-out-cubic\"; custom-shader r#\"close() { }\"#; glide { tilt-deg 17.123456789; }; }; }\n";
        for choice in [
            CloseEffect::Explosion,
            CloseEffect::Implode,
            CloseEffect::Fade,
        ] {
            let next = set_close_effect(source, choice).unwrap();
            let config = Config::parse_mem(&next).unwrap();
            assert!(config.animations.off);
            assert!(!config.animations.window_close.anim.off);
            assert!(config.animations.window_close.custom_shader.is_none());
            assert_eq!(
                config.animations.window_open.custom_shader.as_deref(),
                Some("open() { }")
            );
            assert_eq!(config.animations.window_close.glide.tilt_deg, 17.123456789);
            assert!(next.contains("/* timing */ duration-ms 431; curve \"ease-out-cubic\""));
            assert!(next.contains("// original"));
            assert_eq!(CloseState::read(&next).unwrap().selected, Some(choice));
            assert_eq!(set_close_effect(&next, choice).unwrap(), next);
        }
    }

    #[test]
    fn disabling_close_preserves_shader_and_previous_effect() {
        let source = "animations { window-close { effect \"implode\"; custom-shader \"shader\"; glide { depth 0.23456789; }; }; }\n";
        let next = set_close_effect(source, CloseEffect::Disabled).unwrap();
        let config = Config::parse_mem(&next).unwrap();
        assert!(config.animations.window_close.anim.off);
        assert_eq!(
            config.animations.window_close.effect,
            WindowCloseEffect::Implode
        );
        assert_eq!(
            config.animations.window_close.custom_shader.as_deref(),
            Some("shader")
        );
        assert_eq!(config.animations.window_close.glide.depth, 0.23456789);
        assert_eq!(
            CloseState::read(&next).unwrap().selected,
            Some(CloseEffect::Disabled)
        );
    }

    #[test]
    fn close_state_does_not_guess_the_compositors_default() {
        assert_eq!(
            CloseState::read("include \"base.kdl\"\n").unwrap().selected,
            None
        );
        let custom = CloseState::read(
            "animations { window-close { effect \"fade\"; custom-shader \"shader\"; }; }",
        )
        .unwrap();
        assert_eq!(custom.selected, None);
        assert!(custom.custom_shader);
        assert_eq!(
            CloseState::read("animations { window-close { effect \"shrink\"; }; }")
                .unwrap()
                .selected,
            Some(CloseEffect::Implode)
        );
    }

    #[test]
    fn repeated_sections_require_structured_editor_and_never_lose_overrides() {
        let source = "animations { window-close { effect \"fade\"; }; }\nanimations { window-close { effect \"implode\"; }; }\n";
        assert!(set_close_effect(source, CloseEffect::Explosion).is_err());
        assert!(BlurOverrides::read("blur { passes 2; }\nblur { passes 4; }\n").is_err());
    }

    #[test]
    fn repeated_effect_fields_cannot_hide_the_last_override() {
        let source = "animations { window-close { effect \"fade\"; effect \"implode\"; }; }\n";
        assert!(set_close_effect(source, CloseEffect::Explosion).is_err());
        let source =
            "animations { window-close { custom-shader \"first\"; custom-shader \"last\"; }; }\n";
        assert!(set_close_effect(source, CloseEffect::Fade).is_err());
        assert!(BlurOverrides::read("blur { saturation 1; saturation 2; }").is_err());
    }

    #[test]
    fn changing_one_blur_field_keeps_inherited_fields_and_source_precision() {
        let source = "// untouched\ninclude \"base.kdl\"\nblur { /* retained */ offset 4.123456789; noise 0.123456789; saturation 9.123456789; }\nlayout { gaps 8.123456789; }\n";
        let mut draft = BlurOverrides::read(source).unwrap();
        assert_eq!(draft.apply(source).unwrap(), source);
        draft.enabled = Some(true);
        let next = draft.apply(source).unwrap();
        assert!(next.contains(
            "/* retained */ offset 4.123456789; noise 0.123456789; saturation 9.123456789;"
        ));
        assert!(next.contains("layout { gaps 8.123456789; }"));
        assert!(next.contains("include \"base.kdl\""));
        assert!(!Editor::new(&next).unwrap().exists(&["blur", "passes"]));
        assert_eq!(BlurOverrides::read(&next).unwrap(), draft);
    }

    #[test]
    fn clearing_blur_overrides_restores_included_values() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("config.kdl");
        std::fs::write(
            dir.path().join("base.kdl"),
            "blur { off; passes 5; offset 7.25; noise 0.07; saturation 0.9; }\n",
        )
        .unwrap();
        let source = "include \"base.kdl\"\nblur { /* keep comment */ on; passes 2; offset 1; noise 0.01; saturation 2; }\n";
        let next = BlurOverrides::default().apply(source).unwrap();
        let config = Config::parse(&root, &next).config.unwrap();
        assert!(config.blur.off);
        assert_eq!(config.blur.passes, 5);
        assert_eq!(config.blur.offset, 7.25);
        assert_eq!(config.blur.noise, 0.07);
        assert_eq!(config.blur.saturation, 0.9);
        assert!(next.contains("/* keep comment */"));
    }
}
