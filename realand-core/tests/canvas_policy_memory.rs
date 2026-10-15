use realand_core::policy_memory::{
    ColumnDisplay, ColumnSnapshot, Height, ScrollingSnapshot, Width,
};
use realand_core::stage_drag::{StageCapture, StageDragAction, StageRestorePosition};
use realand_core::workspace_state::{CanvasPolicyMemory, MinimizeRestore};
use realand_core::{CoreEffect, CoreEvent, DesktopCore, Layer, Rect, SpaceId};

fn snapshot() -> ScrollingSnapshot<u64> {
    ScrollingSnapshot {
        columns: vec![ColumnSnapshot {
            id: 31,
            windows: vec![1],
            heights: vec![Height::Fixed(333.)],
            width: Width::Proportion(0.73),
            preset_width: None,
            full_width: true,
            fullscreen: false,
            maximized: false,
            display: ColumnDisplay::Tabbed,
            active: Some(1),
        }],
        active: Some(1),
        active_column: Some(31),
        view_offset: 42.,
        view_offset_to_restore: None,
    }
}
fn desktop() -> DesktopCore<u64> {
    let mut core = DesktopCore::new();
    core.apply(CoreEvent::OutputRegistered {
        connector: "LEFT".into(),
        bounds: Rect::new(0., 0., 1000., 800.),
    });
    core.add_window(
        1,
        SpaceId(10),
        Some("LEFT".into()),
        Rect::new(0., 0., 300., 200.),
    );
    core.apply(CoreEvent::AttachSpace {
        id: SpaceId(10),
        connector: "LEFT".into(),
        index: 0,
    });
    core.apply(CoreEvent::CaptureCanvasPolicy {
        space: SpaceId(10),
        policy: CanvasPolicyMemory::Scrolling(snapshot()),
    });
    core.apply(CoreEvent::SetCanvasMode {
        space: SpaceId(10),
        enabled: true,
    });
    core.apply(CoreEvent::SetLayer {
        id: 1,
        layer: Layer::Above,
    });
    core.apply(CoreEvent::SetCanvasOrigin { id: 1, tiled: true });
    core
}
#[test]
fn entry_policy_is_not_overwritten_during_canvas_and_moves_once_into_exit_intent() {
    let mut core = desktop();
    let mut changed = snapshot();
    changed.columns[0].width = Width::Fixed(999.);
    core.apply(CoreEvent::CaptureCanvasPolicy {
        space: SpaceId(10),
        policy: CanvasPolicyMemory::Scrolling(changed),
    });
    let exit = core.apply(CoreEvent::SetCanvasMode {
        space: SpaceId(10),
        enabled: false,
    });
    let transition = exit
        .into_iter()
        .find_map(|effect| match effect {
            CoreEffect::CanvasChanged { transition, .. } => Some(transition),
            _ => None,
        })
        .unwrap();
    assert_eq!(transition.restore_tiled, [1]);
    assert_eq!(
        transition.restore_policy,
        Some(CanvasPolicyMemory::Scrolling(snapshot()))
    );
    assert!(core
        .space(SpaceId(10))
        .unwrap()
        .interaction
        .canvas_policy()
        .is_none());
    assert!(core
        .apply(CoreEvent::SetCanvasMode {
            space: SpaceId(10),
            enabled: false
        })
        .is_empty());
}
#[test]
fn minimized_original_tiled_window_keeps_proportional_full_width_and_display_after_canvas_exit() {
    let mut core = desktop();
    let hint = MinimizeRestore::capture(&1, true, 300., &[], None);
    core.apply(CoreEvent::MinimizeWindow {
        id: 1,
        restoration: hint,
    });
    let memory = core.window(&1).unwrap().restoration.as_ref().unwrap();
    assert_eq!(memory.width, Width::Proportion(0.73));
    assert!(memory.full_width);
    assert_eq!(memory.display, Some(ColumnDisplay::Tabbed));
    core.apply(CoreEvent::SetCanvasMode {
        space: SpaceId(10),
        enabled: false,
    });
    let effects = core.apply(CoreEvent::RestoreWindow {
        id: 1,
        columns: Vec::new(),
    });
    let restore = effects
        .into_iter()
        .find_map(|effect| match effect {
            CoreEffect::MinimizeIntent(intent) => intent.restore,
            _ => None,
        })
        .unwrap();
    assert_eq!(restore.width, Width::Proportion(0.73));
    assert!(restore.full_width);
    assert_eq!(restore.display, Some(ColumnDisplay::Tabbed));
    assert!(!restore.is_floating());
}
#[test]
fn stage_capture_reads_original_policy_before_drained_floating_metadata_can_replace_it() {
    let mut core = desktop();
    core.apply(CoreEvent::StageDrag(StageDragAction::Capture(
        StageCapture {
            window: 1,
            columns: Vec::new(),
            bsp: None,
            floating_position: None,
            floating_size: Some([300, 200]),
            floating_width: 300.,
            restore_to_floating: false,
            fullscreen: false,
            maximized: false,
        },
    )));
    core.detach_window(&1);
    core.apply(CoreEvent::SetCanvasMode {
        space: SpaceId(10),
        enabled: false,
    });
    core.apply(CoreEvent::StageDrag(StageDragAction::Resolve {
        window: 1,
        destination: SpaceId(10),
        columns: Vec::new(),
    }));
    let restore = core
        .apply(CoreEvent::StageDrag(StageDragAction::Take { window: 1 }))
        .into_iter()
        .find_map(|effect| match effect {
            CoreEffect::StageRestoreIntent(intent) => Some(intent),
            _ => None,
        })
        .unwrap();
    assert_eq!(restore.position, StageRestorePosition::NewColumn(0));
    assert_eq!(restore.width, Width::Proportion(0.73));
    assert!(restore.full_width);
    assert_eq!(restore.display, Some(ColumnDisplay::Tabbed));
    assert!(!restore.floating);
}
#[test]
fn canvas_preference_restore_keeps_current_expanded_mode_with_original_dimensions() {
    let saved = CanvasPolicyMemory::Scrolling(snapshot());
    let mut current = snapshot();
    current.columns[0].width = Width::Fixed(300.);
    current.columns[0].full_width = false;
    current.columns[0].display = ColumnDisplay::Normal;
    current.columns[0].maximized = true;
    let CanvasPolicyMemory::Scrolling(restored) =
        saved.preserve_runtime(&CanvasPolicyMemory::Scrolling(current))
    else {
        unreachable!()
    };
    assert!(restored.columns[0].maximized);
    assert_eq!(restored.columns[0].width, Width::Proportion(0.73));
    assert!(restored.columns[0].full_width);
    assert_eq!(restored.columns[0].display, ColumnDisplay::Tabbed);
}

#[test]
fn restoring_a_normal_group_partitions_new_expanded_members_and_preserves_row_preferences() {
    let mut saved = snapshot();
    saved.columns[0].windows = vec![1, 2];
    saved.columns[0].heights = vec![Height::Fixed(333.), Height::Auto(2.)];
    saved.columns[0].display = ColumnDisplay::Normal;
    let mut current = snapshot();
    current.columns[0].id = 82;
    current.columns[0].maximized = true;
    current.columns[0].display = ColumnDisplay::Normal;
    let mut normal = current.columns[0].clone();
    normal.id = 83;
    normal.windows = vec![2];
    normal.maximized = false;
    current.columns.push(normal);
    let CanvasPolicyMemory::Scrolling(restored) = CanvasPolicyMemory::Scrolling(saved)
        .preserve_runtime(&CanvasPolicyMemory::Scrolling(current))
    else {
        unreachable!()
    };
    assert_eq!(restored.columns.len(), 2);
    let expanded = &restored.columns[0];
    assert_eq!((expanded.id, expanded.windows.as_slice()), (82, &[1][..]));
    assert!(expanded.maximized);
    assert_eq!(expanded.width, Width::Proportion(0.73));
    assert!(expanded.full_width);
    assert_eq!(expanded.display, ColumnDisplay::Normal);
    assert_eq!(expanded.heights, [Height::Fixed(333.)]);
    let normal = &restored.columns[1];
    assert_eq!((normal.id, normal.windows.as_slice()), (31, &[2][..]));
    assert!(!normal.fullscreen && !normal.maximized);
    assert_eq!(normal.heights, [Height::Auto(2.)]);
    assert_eq!(restored.active_column, Some(82));
}
#[test]
fn expanded_partition_never_steals_another_saved_columns_restore_identity() {
    let mut saved = snapshot();
    saved.columns[0].windows = vec![1, 2];
    saved.columns[0].display = ColumnDisplay::Normal;
    let mut other = saved.columns[0].clone();
    other.id = 32;
    other.windows = vec![3];
    saved.columns.push(other);
    let mut current = snapshot();
    current.columns[0].id = 32;
    current.columns[0].fullscreen = true;
    let CanvasPolicyMemory::Scrolling(restored) = CanvasPolicyMemory::Scrolling(saved)
        .preserve_runtime(&CanvasPolicyMemory::Scrolling(current))
    else {
        unreachable!()
    };
    let ids: std::collections::HashSet<_> =
        restored.columns.iter().map(|column| column.id).collect();
    assert_eq!(ids.len(), restored.columns.len());
    assert_ne!(restored.columns[0].id, 32);
    assert_eq!(
        restored
            .columns
            .iter()
            .find(|column| column.windows == [3])
            .unwrap()
            .id,
        32
    );
}
