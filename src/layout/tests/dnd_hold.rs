//! Actual hit testing and activation must consume the core's hover intent.
use super::*;

fn fixture() -> (Layout<TestWindow>, Output, Point<f64, Logical>) {
    let mut options = Options::default();
    options.animations.off = true;
    let mut first = TestWindowParams::new(1);
    first.is_floating = true;
    let mut second = TestWindowParams::new(2);
    second.is_floating = true;
    let layout = check_ops_with_options(
        options,
        [
            Op::AddOutput(1),
            Op::AddWindow { params: first },
            Op::Communicate(1),
            Op::AddOutput(2),
            Op::FocusOutput(2),
            Op::AddWindow { params: second },
            Op::Communicate(2),
            Op::CompleteAnimations,
        ],
    );
    let mon = layout
        .monitors()
        .find(|mon| mon.output().name() == "output1")
        .unwrap();
    let (_, geo) = mon.workspaces_with_render_geo().next().unwrap();
    let workspace = &mon.workspaces[0];
    let (tile, point, _) = workspace
        .tiles_with_render_positions()
        .find(|(tile, _, _)| tile.window().id() == &1)
        .unwrap();
    let pointer = geo.loc
        + (point + tile.window_loc() + tile.window_size().to_point().downscale(2.))
            .upscale(mon.workspace_render_zoom(geo));
    assert_eq!(
        mon.window_under(pointer).map(|(window, _)| *window.id()),
        Some(1)
    );
    let output = mon.output().clone();
    (layout, output, pointer)
}
fn tick(layout: &mut Layout<TestWindow>, millis: u64) {
    layout.clock.set_unadjusted(Duration::from_millis(millis));
    layout.advance_animations();
    layout.verify_invariants();
}
#[test]
fn stationary_hover_activates_actual_window_only_at_the_core_deadline() {
    let (mut layout, output, pointer) = fixture();
    layout.dnd_update(output, pointer);
    tick(&mut layout, 0);
    assert_eq!(
        layout.core.borrow().dnd_hold().target(),
        Some(&clarion_core::dnd_hold::DndTarget::Window(1))
    );
    tick(&mut layout, 749);
    assert_eq!(layout.focus().map(LayoutElement::id), Some(&2));
    tick(&mut layout, 750);
    assert_eq!(layout.focus().map(LayoutElement::id), Some(&1));
    assert!(layout.core.borrow().dnd_hold().target().is_none());
    layout.dnd_end();
    assert!(layout.core.borrow().dnd_hold().connection().is_none());
}
#[test]
fn real_pointer_motion_resets_hover_even_when_hit_target_did_not_change() {
    let (mut layout, output, pointer) = fixture();
    layout.dnd_update(output.clone(), pointer);
    tick(&mut layout, 0);
    tick(&mut layout, 600);
    layout.dnd_update(output, pointer);
    tick(&mut layout, 600);
    tick(&mut layout, 1000);
    assert_eq!(layout.focus().map(LayoutElement::id), Some(&2));
    tick(&mut layout, 1350);
    assert_eq!(layout.focus().map(LayoutElement::id), Some(&1));
}
#[test]
fn destroying_hovered_window_clears_the_id_before_any_late_activation() {
    let (mut layout, output, pointer) = fixture();
    layout.dnd_update(output, pointer);
    tick(&mut layout, 0);
    layout.remove_window(&1, Transaction::new()).unwrap();
    assert!(layout.core.borrow().dnd_hold().target().is_none());
    tick(&mut layout, 1000);
    assert!(layout.core.borrow().window(&1).is_none());
    assert_ne!(layout.focus().map(LayoutElement::id), Some(&1));
}
#[test]
fn same_name_output_replacement_rejects_old_device_object_and_old_hover_time() {
    let (mut layout, old, pointer) = fixture();
    layout.dnd_update(old.clone(), pointer);
    tick(&mut layout, 0);
    let generation = layout
        .core
        .borrow()
        .dnd_hold()
        .connection()
        .unwrap()
        .generation;
    Op::RemoveOutput(1).apply(&mut layout);
    assert!(layout.core.borrow().dnd_hold().connection().is_none());
    Op::AddOutput(1).apply(&mut layout);
    layout.dnd_update(old, pointer);
    assert!(layout.core.borrow().dnd_hold().connection().is_none());
    let current = layout
        .outputs()
        .find(|output| output.name() == "output1")
        .unwrap()
        .clone();
    layout.dnd_update(current, pointer);
    assert_ne!(
        layout
            .core
            .borrow()
            .dnd_hold()
            .connection()
            .unwrap()
            .generation,
        generation
    );
    tick(&mut layout, 1000);
    assert!(layout.core.borrow().dnd_hold().target().is_some());
    assert_eq!(layout.focus().map(LayoutElement::id), Some(&2));
    tick(&mut layout, 1749);
    assert_eq!(layout.focus().map(LayoutElement::id), Some(&2));
    tick(&mut layout, 1750);
    assert_eq!(layout.focus().map(LayoutElement::id), Some(&1));
}
