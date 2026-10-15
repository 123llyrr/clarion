//! Reconnection applies the core's stable-space return affinity; device names
//! retain the public configuration matcher's existing alias behavior.
use super::*;

fn output(connector: &str, serial: &str) -> Output {
    let output = Output::new(
        connector.into(),
        PhysicalProperties {
            size: (1280, 720).into(),
            subpixel: Subpixel::Unknown,
            make: "Maker".into(),
            model: "Screen".into(),
            serial_number: serial.into(),
        },
    );
    output.change_current_state(
        Some(Mode {
            size: (1280, 720).into(),
            refresh: 60000,
        }),
        None,
        None,
        None,
    );
    output.user_data().insert_if_missing(|| OutputName {
        connector: connector.into(),
        make: Some("Maker".into()),
        model: Some("Screen".into()),
        serial: Some(serial.into()),
    });
    output
}

fn named_origin() -> (Layout<TestWindow>, Output, Output, WorkspaceId) {
    let mut layout = Layout::with_options(Clock::with_time(Duration::ZERO), Options::default());
    let left = output("DP-1", "LEFT");
    let right = output("DP-2", "RIGHT");
    layout.add_output(left.clone(), None);
    layout.add_output(right.clone(), None);
    layout.focus_output(&left);
    check_ops_on_layout(
        &mut layout,
        [
            Op::SetWorkspaceName {
                new_ws_name: 1,
                ws_name: None,
            },
            Op::AddWindow {
                params: TestWindowParams::new(1),
            },
        ],
    );
    let space = layout.active_workspace().unwrap().id();
    assert_eq!(
        layout
            .core
            .borrow()
            .space(realand_core::SpaceId(space.get()))
            .unwrap()
            .output_preference,
        "Maker Screen LEFT"
    );
    (layout, left, right, space)
}

fn owner(layout: &Layout<TestWindow>, space: WorkspaceId) -> Option<String> {
    layout
        .workspaces()
        .find(|(_, _, workspace)| workspace.id() == space)
        .and_then(|(monitor, _, _)| monitor)
        .map(|monitor| monitor.output().name())
}

#[test]
fn pure_output_alias_matcher_agrees_with_the_existing_configuration_oracle() {
    for make in [None, Some("Maker"), Some("Maker With Spaces"), Some("厂商")] {
        for model in [None, Some("Screen"), Some("Wide Screen"), Some("屏幕")] {
            for serial in [None, Some("ABC"), Some(""), Some("序号")] {
                let oracle = OutputName {
                    connector: "DP-1".into(),
                    make: make.map(str::to_owned),
                    model: model.map(str::to_owned),
                    serial: serial.map(str::to_owned),
                };
                let pure = realand_core::output_identity::OutputIdentity {
                    connector: oracle.connector.clone(),
                    make: oracle.make.clone(),
                    model: oracle.model.clone(),
                    serial: oracle.serial.clone(),
                };
                assert_eq!(
                    pure.canonical_name(),
                    oracle.format_make_model_serial_or_connector()
                );
                let alias = oracle.format_make_model_serial();
                for selector in [
                    "DP-1".to_owned(),
                    "dp-1".to_owned(),
                    "OTHER".to_owned(),
                    alias.clone(),
                    alias.to_ascii_lowercase(),
                    format!("{alias} "),
                    format!(" {alias}"),
                    "Unknown Unknown Unknown".to_owned(),
                    "厂厂".to_owned(),
                    "Ma屏".to_owned(),
                    String::new(),
                ] {
                    assert_eq!(
                        pure.matches(&selector),
                        oracle.matches(&selector),
                        "make={make:?} model={model:?} serial={serial:?} selector={selector:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn actual_hotplug_returns_named_desktop_by_serial_after_the_connector_changes() {
    let (mut layout, left, right, space) = named_origin();
    layout.remove_output(&left);
    assert_eq!(owner(&layout, space), Some(right.name()));
    let replacement = output("HDMI-9", "LEFT");
    layout.add_output(replacement.clone(), None);
    assert_eq!(owner(&layout, space), Some(replacement.name()));
    assert!(layout
        .workspaces()
        .find(|(_, _, workspace)| workspace.id() == space)
        .unwrap()
        .2
        .has_window(&1));
    layout.verify_invariants();
}

#[test]
fn actual_no_output_interval_keeps_return_affinity_and_rejects_same_connector_with_foreign_serial()
{
    let (mut layout, left, right, space) = named_origin();
    layout.remove_output(&left);
    layout.remove_output(&right);
    assert_eq!(owner(&layout, space), None);
    let foreign = output("DP-1", "FOREIGN");
    layout.add_output(foreign.clone(), None);
    assert_eq!(owner(&layout, space), Some(foreign.name()));
    assert_eq!(
        layout
            .core
            .borrow()
            .space(realand_core::SpaceId(space.get()))
            .unwrap()
            .output_preference,
        "Maker Screen LEFT"
    );
    let replacement = output("DP-9", "LEFT");
    layout.add_output(replacement.clone(), None);
    assert_eq!(owner(&layout, space), Some(replacement.name()));
    layout.verify_invariants();
}

#[test]
fn actual_explicit_move_to_current_output_changes_the_core_return_preference_once() {
    let (mut layout, left, right, space) = named_origin();
    layout.remove_output(&left);
    let index = layout
        .workspaces()
        .find(|(_, _, workspace)| workspace.id() == space)
        .unwrap()
        .1;
    assert!(!layout.move_workspace_to_output_by_id(index, Some(right.clone()), &right));
    assert_eq!(
        layout
            .core
            .borrow()
            .space(realand_core::SpaceId(space.get()))
            .unwrap()
            .output_preference,
        "Maker Screen RIGHT"
    );
    let replacement = output("HDMI-9", "LEFT");
    layout.add_output(replacement, None);
    assert_eq!(owner(&layout, space), Some(right.name()));
    layout.verify_invariants();
}
