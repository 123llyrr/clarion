//! Display controls which edit only the selected output's configuration.
//!
//! Output matching is first-match, by connector or make/model/serial; there is
//! no wildcard matching. A connector-specific root override precedes inherited
//! matches, preserving the complete winning block and every other output.
use std::ops::Range;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clarion_config::output::{Mode, Output, OutputName, Vrr};
use clarion_config::{Config, FloatOrInt};
use clarion_ipc::{ConfiguredMode, Transform};
use knuffel::ast::SpannedNode;
use knuffel::span::Span;

use crate::config_manager::ConfigManager;
use crate::kdl_edit::Editor;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VrrMode {
    #[default]
    Off,
    Always,
    OnDemand,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OutputSettings {
    pub enabled: bool,
    pub hdr: bool,
    pub vrr: VrrMode,
    pub mode: Option<ConfiguredMode>,
    /// None uses the compositor's automatic scale.
    pub scale: Option<f64>,
    pub transform: Transform,
    /// None inherits the native defaults (100%, 203 nits and EDID peak).
    pub hdr_brightness: Option<f64>,
    pub sdr_white_level: Option<f64>,
    pub hdr_peak_luminance: Option<f64>,
    /// Read from the effective block. Selecting a standard mode may clear it;
    /// the simple controls cannot synthesize a custom modeline.
    pub custom_modeline: bool,
}

impl Default for OutputSettings {
    fn default() -> Self {
        Self::from_output(&Output::default())
    }
}

impl OutputSettings {
    fn from_output(output: &Output) -> Self {
        Self {
            enabled: !output.off,
            hdr: output.hdr,
            vrr: match &output.variable_refresh_rate {
                None => VrrMode::Off,
                Some(vrr) if vrr.on_demand => VrrMode::OnDemand,
                Some(_) => VrrMode::Always,
            },
            mode: output.mode.map(|mode| mode.mode),
            scale: output.scale.map(|value| value.0),
            transform: output.transform,
            hdr_brightness: output.hdr_brightness.map(|value| value.0),
            sdr_white_level: output.sdr_white_level.map(|value| value.0),
            hdr_peak_luminance: output.hdr_peak_luminance.map(|value| value.0),
            custom_modeline: output.modeline.is_some(),
        }
    }

    pub fn read(manager: &ConfigManager, target: &OutputName) -> Result<Self> {
        let config = manager.effective_config()?;
        Ok(config
            .outputs
            .find(target)
            .map(Self::from_output)
            .unwrap_or_default())
    }

    /// Create a checked draft; the caller owns output-preview confirmation and
    /// persistence. No files or compositor state are changed here.
    pub fn apply(
        manager: &mut ConfigManager,
        target: &OutputName,
        before: &Self,
        after: &Self,
    ) -> Result<()> {
        if before == after {
            return Ok(());
        }
        anyhow::ensure!(
            manager.config_path() == manager.root_path(),
            "显示器控件需要编辑主配置文件，请返回主配置后重试"
        );
        anyhow::ensure!(
            !target.connector.is_empty(),
            "显示器连接器名称为空，未修改配置"
        );
        let sources = manager.effective_source_contents()?;
        let mut desired = manager.effective_config()?;
        let selected = desired
            .outputs
            .0
            .iter()
            .position(|output| target.matches(&output.name));
        let original = selected
            .map(|index| desired.outputs.0[index].clone())
            .unwrap_or_else(|| Output {
                name: target.connector.clone(),
                ..Default::default()
            });
        let fresh = Self::from_output(&original);
        let expected = changed_output(&original, &fresh, before, after)?;
        let source = manager.raw_content();

        // Prefer editing an existing connector block in place. Its configured
        // order must actually own this output, established by full-config
        // comparison rather than by guessing which include was loaded first.
        for (span, block, configured) in output_blocks(source)? {
            if configured.name.eq_ignore_ascii_case(&target.connector) && configured == original {
                let edited = patch_block(&block, &fresh, before, after)?;
                let mut candidate_source = source.to_owned();
                candidate_source.replace_range(span, &edited);
                if let Some(index) = selected {
                    desired.outputs.0[index] = expected.clone();
                    if manager.merged_config(&candidate_source)? == desired {
                        anyhow::ensure!(
                            manager.effective_source_contents()? == sources,
                            "包含文件在修改显示设置期间发生变化，原草稿已保留"
                        );
                        manager.set_raw_content(candidate_source);
                        return Ok(());
                    }
                    desired.outputs.0[index] = original.clone();
                }
            }
        }

        // An inherited or make/model/serial block may also serve another
        // monitor. Keep it intact and add a precise connector override first.
        let inherited = if selected.is_some() {
            winning_block(source, &sources, &original)?
                .context("无法找到完整显示器配置来源，未重置其他参数")?
        } else {
            format!(
                "output {} {{\n}}\n",
                serde_json::to_string(&target.connector)?
            )
        };
        let isolated = set_argument(
            &inherited,
            &["output"],
            &serde_json::to_string(&target.connector)?,
        )?;
        let edited = patch_block(&isolated, &fresh, before, after)?;
        let candidate_source = format!("{edited}\n{source}");
        let mut override_output = expected;
        override_output.name = target.connector.clone();
        desired.outputs.0.insert(0, override_output);
        anyhow::ensure!(
            manager.merged_config(&candidate_source)? == desired,
            "显示器覆盖无法安全表达，原配置已保留；请检查包含文件与输出匹配规则"
        );
        anyhow::ensure!(
            manager.effective_source_contents()? == sources,
            "包含文件在修改显示设置期间发生变化，原草稿已保留"
        );
        manager.set_raw_content(candidate_source);
        Ok(())
    }
}

fn changed_output(
    original: &Output,
    fresh: &OutputSettings,
    before: &OutputSettings,
    after: &OutputSettings,
) -> Result<Output> {
    let mut desired = original.clone();
    macro_rules! changed {
        ($field:ident) => {{
            let changed = before.$field != after.$field;
            anyhow::ensure!(
                !changed || fresh.$field == before.$field,
                "显示设置 {} 已被其他程序修改，请重新载入",
                stringify!($field)
            );
            changed
        }};
    }
    if changed!(enabled) {
        desired.off = !after.enabled;
    }
    if changed!(hdr) {
        desired.hdr = after.hdr;
    }
    if changed!(vrr) {
        desired.variable_refresh_rate = match after.vrr {
            VrrMode::Off => None,
            VrrMode::Always => Some(Vrr { on_demand: false }),
            VrrMode::OnDemand => Some(Vrr { on_demand: true }),
        };
    }
    let clear_modeline = changed!(custom_modeline);
    if clear_modeline {
        anyhow::ensure!(
            !after.custom_modeline,
            "请选择标准显示模式；常用设置不能创建自定义modeline"
        );
        desired.modeline = None;
        // Selecting a standard mode owns the entire mode choice even if its
        // dimensions equal the previous custom mode. Do not replay a stale
        // selection over an externally changed mode.
        anyhow::ensure!(
            fresh.mode == before.mode,
            "显示模式已被其他程序修改，请重新载入"
        );
    }
    if changed!(mode) || clear_modeline {
        anyhow::ensure!(!after.custom_modeline, "请选择标准模式以替换自定义modeline");
        if let Some(mode) = after.mode {
            anyhow::ensure!(
                mode.width > 0
                    && mode.height > 0
                    && mode.refresh.is_none_or(|hz| hz.is_finite() && hz > 0.),
                "显示模式的分辨率和刷新率必须大于零"
            );
        }
        desired.mode = after.mode.map(|mode| Mode {
            custom: false,
            mode,
        });
    }
    if changed!(scale) {
        anyhow::ensure!(
            after
                .scale
                .is_none_or(|value| value.is_finite() && value > 0. && value <= 10.),
            "缩放必须大于0且不超过10"
        );
        desired.scale = after.scale.map(FloatOrInt);
    }
    if changed!(transform) {
        desired.transform = after.transform;
    }
    macro_rules! number {
        ($field:ident, $minimum:literal, $maximum:literal) => {
            if changed!($field) {
                anyhow::ensure!(
                    after.$field.is_none_or(
                        |value| value.is_finite() && ($minimum..=$maximum).contains(&value)
                    ),
                    "显示器参数 {} 超出支持范围",
                    stringify!($field)
                );
                desired.$field = after.$field.map(FloatOrInt);
            }
        };
    }
    number!(hdr_brightness, 1.0, 100.0);
    number!(sdr_white_level, 80.0, 500.0);
    number!(hdr_peak_luminance, 80.0, 10000.0);
    Ok(desired)
}

fn patch_block(
    source: &str,
    _fresh: &OutputSettings,
    before: &OutputSettings,
    after: &OutputSettings,
) -> Result<String> {
    let mut result = source.to_owned();
    if before.enabled != after.enabled {
        result = set_flag(&result, "off", !after.enabled)?;
    }
    if before.hdr != after.hdr {
        result = set_flag(&result, "hdr", after.hdr)?;
    }
    if before.vrr != after.vrr {
        result = match after.vrr {
            VrrMode::Off => remove(&result, "variable-refresh-rate")?,
            VrrMode::Always => {
                set_property(&result, "variable-refresh-rate", "on-demand", "false")?
            }
            VrrMode::OnDemand => {
                set_property(&result, "variable-refresh-rate", "on-demand", "true")?
            }
        };
    }
    if before.custom_modeline != after.custom_modeline {
        result = remove(&result, "modeline")?;
    }
    if before.mode != after.mode || before.custom_modeline != after.custom_modeline {
        if let Some(mode) = after.mode {
            let value = match mode.refresh {
                Some(hz) => format!("{}x{}@{hz}", mode.width, mode.height),
                None => format!("{}x{}", mode.width, mode.height),
            };
            result = set_argument(
                &result,
                &["output", "mode"],
                &serde_json::to_string(&value)?,
            )?;
            result = set_property(&result, "mode", "custom", "false")?;
        } else {
            result = remove(&result, "mode")?;
        }
    }
    if before.scale != after.scale {
        result = set_optional(&result, "scale", after.scale)?;
    }
    if before.transform != after.transform {
        let value = match after.transform {
            Transform::Normal => "normal",
            Transform::_90 => "90",
            Transform::_180 => "180",
            Transform::_270 => "270",
            Transform::Flipped => "flipped",
            Transform::Flipped90 => "flipped-90",
            Transform::Flipped180 => "flipped-180",
            Transform::Flipped270 => "flipped-270",
        };
        result = set_argument(
            &result,
            &["output", "transform"],
            &serde_json::to_string(value)?,
        )?;
    }
    for (name, previous, next) in [
        (
            "hdr-brightness",
            before.hdr_brightness,
            after.hdr_brightness,
        ),
        (
            "sdr-white-level",
            before.sdr_white_level,
            after.sdr_white_level,
        ),
        (
            "hdr-peak-luminance",
            before.hdr_peak_luminance,
            after.hdr_peak_luminance,
        ),
    ] {
        if previous != next {
            result = set_optional(&result, name, next)?;
        }
    }
    Ok(result)
}

fn output_blocks(source: &str) -> Result<Vec<(Range<usize>, String, Output)>> {
    let document = knuffel::parse_ast::<Span>("config.kdl", source)
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    document
        .nodes
        .iter()
        .filter(|node| &**node.node_name == "output")
        .map(|node| {
            let span = node.span();
            let block = source[span.0..span.1].to_owned();
            let configured = Config::parse_mem(&block)?
                .outputs
                .0
                .into_iter()
                .next()
                .context("输出块为空")?;
            Ok((span.0..span.1, block, configured))
        })
        .collect()
}

fn winning_block(
    source: &str,
    sources: &[(PathBuf, Option<Vec<u8>>)],
    wanted: &Output,
) -> Result<Option<String>> {
    for candidate in std::iter::once(source.to_owned()).chain(
        sources
            .iter()
            .filter_map(|(_, bytes)| bytes.as_ref())
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned()),
    ) {
        for (_, block, configured) in output_blocks(&candidate)? {
            if &configured == wanted {
                return Ok(Some(block));
            }
        }
    }
    Ok(None)
}

fn find<'a>(
    nodes: &'a [SpannedNode<Span>],
    path: &[&str],
) -> Result<Option<&'a SpannedNode<Span>>> {
    let Some((name, rest)) = path.split_first() else {
        return Ok(None);
    };
    let mut matching = nodes.iter().filter(|node| &**node.node_name == *name);
    let Some(node) = matching.next() else {
        return Ok(None);
    };
    anyhow::ensure!(
        matching.next().is_none(),
        "显示器字段 {} 重复，未修改配置",
        path.join(".")
    );
    if rest.is_empty() {
        Ok(Some(node))
    } else {
        find(
            node.children.as_deref().map(Vec::as_slice).unwrap_or(&[]),
            rest,
        )
    }
}

fn set_argument(source: &str, path: &[&str], value: &str) -> Result<String> {
    let document = knuffel::parse_ast::<Span>("output.kdl", source)
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    if let Some(node) = find(&document.nodes, path)? {
        anyhow::ensure!(
            node.arguments.len() <= 1,
            "显示器字段 {} 有多个参数",
            path.join(".")
        );
        let mut result = source.to_owned();
        if let Some(argument) = node.arguments.first() {
            let span = argument.literal.span();
            result.replace_range(span.0..span.1, value);
        } else {
            result.insert_str(node.node_name.span().1, &format!(" {value}"));
        }
        return Ok(result);
    }
    let (name, parent) = path.split_last().context("缺少输出字段")?;
    let mut editor = Editor::new(source)?;
    editor.upsert(parent, name, &format!("{name} {value}"))?;
    Ok(editor.finish())
}

fn set_property(source: &str, name: &str, property: &str, value: &str) -> Result<String> {
    let document = knuffel::parse_ast::<Span>("output.kdl", source)
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    if let Some(node) = find(&document.nodes, &["output", name])? {
        let mut result = source.to_owned();
        if let Some((_, argument)) = node.properties.iter().find(|(key, _)| &****key == property) {
            let span = argument.literal.span();
            result.replace_range(span.0..span.1, value);
        } else {
            result.insert_str(node.node_name.span().1, &format!(" {property}={value}"));
        }
        return Ok(result);
    }
    let mut editor = Editor::new(source)?;
    editor.upsert(&["output"], name, &format!("{name} {property}={value}"))?;
    Ok(editor.finish())
}

fn remove(source: &str, name: &str) -> Result<String> {
    let document = knuffel::parse_ast::<Span>("output.kdl", source)
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    find(&document.nodes, &["output", name])?;
    let mut editor = Editor::new(source)?;
    editor.remove(&["output", name])?;
    Ok(editor.finish())
}

fn set_flag(source: &str, name: &str, enabled: bool) -> Result<String> {
    let document = knuffel::parse_ast::<Span>("output.kdl", source)
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    let present = find(&document.nodes, &["output", name])?.is_some();
    if !enabled {
        return remove(source, name);
    }
    if present {
        return Ok(source.to_owned());
    }
    let mut editor = Editor::new(source)?;
    editor.upsert(&["output"], name, name)?;
    Ok(editor.finish())
}

fn set_optional(source: &str, name: &str, value: Option<f64>) -> Result<String> {
    match value {
        Some(value) => set_argument(source, &["output", name], &value.to_string()),
        None => remove(source, name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn target() -> OutputName {
        OutputName {
            connector: "DP-1".into(),
            make: Some("Maker".into()),
            model: Some("Panel".into()),
            serial: Some("123".into()),
        }
    }
    fn manager(source: &str) -> (tempfile::TempDir, ConfigManager) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.kdl");
        fs::write(&path, source).unwrap();
        let manager = ConfigManager::from_path(&path).unwrap();
        (directory, manager)
    }

    #[test]
    fn untouched_output_source_and_custom_fields_remain_byte_identical() {
        let source = "// keep\noutput \"DP-1\" { mode custom=true \"2560x1440@119.98765\"; hdr; sdr-white-level 203.123456789; variable-refresh-rate on-demand=true; position x=42 y=-99; }\noutput \"DP-2\" { off; }\n";
        let (_directory, mut manager) = manager(source);
        let target = target();
        let before = OutputSettings::read(&manager, &target).unwrap();
        OutputSettings::apply(&mut manager, &target, &before, &before).unwrap();
        assert_eq!(manager.raw_content(), source);
        assert!(!manager.is_dirty());
        let mut after = before.clone();
        after.hdr = false;
        OutputSettings::apply(&mut manager, &target, &before, &after).unwrap();
        assert_eq!(manager.raw_content(), source.replacen("hdr;", "", 1));
        assert_eq!(OutputSettings::read(&manager, &target).unwrap(), after);
        assert!(
            manager
                .effective_config()
                .unwrap()
                .outputs
                .find(&target)
                .unwrap()
                .mode
                .unwrap()
                .custom
        );
    }

    #[test]
    fn connector_first_match_in_place_does_not_change_other_outputs() {
        let source = "output \"dp-1\" { scale /* exact */ 1.123456789; variable-refresh-rate on-demand=true; }\noutput \"Maker Panel 123\" { hdr; }\noutput \"DP-2\" { transform \"90\"; }\n";
        let (_directory, mut manager) = manager(source);
        let target = target();
        let original = manager.effective_config().unwrap().outputs.0;
        let before = OutputSettings::read(&manager, &target).unwrap();
        let mut after = before.clone();
        after.scale = Some(1.5);
        OutputSettings::apply(&mut manager, &target, &before, &after).unwrap();
        assert_eq!(manager.raw_content(), source.replace("1.123456789", "1.5"));
        let actual = manager.effective_config().unwrap().outputs.0;
        assert_eq!(&actual[1..], &original[1..]);
        assert_eq!(actual.len(), 3);
        assert_eq!(after.vrr, VrrMode::OnDemand);
    }

    #[test]
    fn inherited_first_match_gets_full_connector_override_before_include() {
        let source =
            "include \"monitors.kdl\"\noutput \"DP-1\" { hdr; }\noutput \"DP-2\" { scale 2; }\n";
        let (directory, mut manager) = manager(source);
        let target = target();
        let included = "// inherited\noutput \"Maker Panel 123\" { mode custom=true \"3840x2160@59.987654\"; scale 1.23456789; position x=70 y=80; variable-refresh-rate on-demand=true; max-bpc 10; background-color \"#123456\"; }\n";
        fs::write(directory.path().join("monitors.kdl"), included).unwrap();
        let original = manager.effective_config().unwrap().outputs.0;
        let before = OutputSettings::read(&manager, &target).unwrap();
        let mut after = before.clone();
        after.hdr = true;
        OutputSettings::apply(&mut manager, &target, &before, &after).unwrap();
        let actual = manager.effective_config().unwrap().outputs.0;
        assert_eq!(&actual[1..], &original);
        assert_eq!(actual[0].name, "DP-1");
        assert_eq!(actual[0].position, original[0].position);
        assert_eq!(actual[0].mode, original[0].mode);
        assert!(manager.raw_content().ends_with(source));
        assert_eq!(
            fs::read_to_string(directory.path().join("monitors.kdl")).unwrap(),
            included
        );
        assert_eq!(OutputSettings::read(&manager, &target).unwrap(), after);
    }

    #[test]
    fn mms_override_targets_only_this_connector() {
        let source = "output \"Maker Panel 123\" { scale 2; hdr; }\n";
        let (_directory, mut manager) = manager(source);
        let target = target();
        let before = OutputSettings::read(&manager, &target).unwrap();
        let mut after = before.clone();
        after.hdr = false;
        OutputSettings::apply(&mut manager, &target, &before, &after).unwrap();
        let mut sibling = target.clone();
        sibling.connector = "DP-2".into();
        let config = manager.effective_config().unwrap();
        assert!(!config.outputs.find(&target).unwrap().hdr);
        assert!(config.outputs.find(&sibling).unwrap().hdr);
        assert!(manager.raw_content().ends_with(source));
    }

    #[test]
    fn standard_mode_replaces_modeline_only_after_explicit_selection() {
        let source = "output \"DP-1\" { modeline 173.0 1920 2048 2248 2576 1080 1083 1088 1120 \"-hsync\" \"+vsync\"; scale 1.25; }\n";
        let (_directory, mut manager) = manager(source);
        let target = target();
        let before = OutputSettings::read(&manager, &target).unwrap();
        assert!(before.custom_modeline);
        let mut after = before.clone();
        after.mode = Some(ConfiguredMode {
            width: 2560,
            height: 1440,
            refresh: Some(119.987),
        });
        assert!(OutputSettings::apply(&mut manager, &target, &before, &after).is_err());
        assert_eq!(manager.raw_content(), source);
        after.custom_modeline = false;
        OutputSettings::apply(&mut manager, &target, &before, &after).unwrap();
        let output = manager.effective_config().unwrap().outputs.0.remove(0);
        assert!(output.modeline.is_none());
        assert_eq!(output.mode.unwrap().mode, after.mode.unwrap());
        assert_eq!(OutputSettings::read(&manager, &target).unwrap(), after);
    }

    #[test]
    fn all_controls_roundtrip_with_unrelated_values_preserved() {
        let source = "output \"DP-1\" { focus-at-startup; position x=14 y=27; backdrop-color \"#223344\"; }\n";
        let (_directory, mut manager) = manager(source);
        let target = target();
        let before = OutputSettings::read(&manager, &target).unwrap();
        let mut after = before.clone();
        after.enabled = false;
        after.hdr = true;
        after.vrr = VrrMode::OnDemand;
        after.mode = Some(ConfiguredMode {
            width: 3840,
            height: 2400,
            refresh: Some(60.),
        });
        after.scale = Some(2.);
        after.transform = Transform::Flipped90;
        after.hdr_brightness = Some(77.5);
        after.sdr_white_level = Some(200.25);
        after.hdr_peak_luminance = Some(800.);
        OutputSettings::apply(&mut manager, &target, &before, &after).unwrap();
        assert_eq!(OutputSettings::read(&manager, &target).unwrap(), after);
        OutputSettings::apply(&mut manager, &target, &after, &before).unwrap();
        assert_eq!(OutputSettings::read(&manager, &target).unwrap(), before);
        assert!(manager.raw_content().contains("focus-at-startup"));
        assert!(manager.raw_content().contains("position x=14 y=27"));
    }

    #[test]
    fn stale_changed_field_and_invalid_luminance_preserve_draft() {
        let (directory, mut manager) = manager("include \"monitors.kdl\"\n");
        let target = target();
        let included = directory.path().join("monitors.kdl");
        fs::write(&included, "output \"DP-1\" { scale 1.5; }\n").unwrap();
        let before = OutputSettings::read(&manager, &target).unwrap();
        let mut after = before.clone();
        after.scale = Some(2.);
        fs::write(&included, "output \"DP-1\" { scale 1.75; }\n").unwrap();
        assert!(OutputSettings::apply(&mut manager, &target, &before, &after).is_err());
        assert_eq!(manager.raw_content(), "include \"monitors.kdl\"\n");
        let before = OutputSettings::read(&manager, &target).unwrap();
        let mut after = before.clone();
        after.sdr_white_level = Some(50.);
        assert!(OutputSettings::apply(&mut manager, &target, &before, &after).is_err());
        after.sdr_white_level = Some(f64::NAN);
        assert!(OutputSettings::apply(&mut manager, &target, &before, &after).is_err());
    }

    #[test]
    fn absent_config_gets_precise_draft_only_and_no_wildcard_match() {
        let source = "output \"*\" { off; }\n// preserved\n";
        let (directory, mut manager) = manager(source);
        let target = target();
        let before = OutputSettings::read(&manager, &target).unwrap();
        assert!(before.enabled);
        let mut after = before.clone();
        after.hdr = true;
        OutputSettings::apply(&mut manager, &target, &before, &after).unwrap();
        assert_eq!(manager.effective_config().unwrap().outputs.0.len(), 2);
        assert_eq!(
            fs::read_to_string(directory.path().join("config.kdl")).unwrap(),
            source
        );
        assert!(!directory.path().join("config.kdl.bak").exists());
    }

    #[test]
    fn same_standard_mode_explicitly_clears_modeline_and_custom_mode_flag() {
        let source = "output \"DP-1\" { mode /* keep mode comment */ custom=true \"1920x1080@60\"; modeline 173.0 1920 2048 2248 2576 1080 1083 1088 1120 \"-hsync\" \"+vsync\"; position x=14 y=27; sdr-white-level 203.123456789; }\n";
        let (_directory, mut manager) = manager(source);
        let target = target();
        let before = OutputSettings::read(&manager, &target).unwrap();
        assert!(before.custom_modeline);
        let mut after = before.clone();
        after.custom_modeline = false;
        assert_eq!(after.mode, before.mode);
        OutputSettings::apply(&mut manager, &target, &before, &after).unwrap();
        let config = manager.effective_config().unwrap();
        let output = config.outputs.find(&target).unwrap();
        assert!(output.modeline.is_none());
        assert!(!output.mode.unwrap().custom);
        assert_eq!(output.mode.unwrap().mode, before.mode.unwrap());
        assert_eq!(OutputSettings::read(&manager, &target).unwrap(), after);
        assert!(manager.raw_content().contains("/* keep mode comment */"));
        assert!(manager.raw_content().contains("position x=14 y=27"));
        assert!(manager
            .raw_content()
            .contains("sdr-white-level 203.123456789"));
    }
}
