use clarion_ipc::management::SessionTree;

use super::*;
use crate::layout::dwindle::DwindleSpace;
use crate::scene::resources::SceneResources;

fn fixture() -> (
    SceneResources,
    DwindleSpace<TestWindow>,
    Size<f64, Logical>,
    Clock,
    Rc<Options>,
) {
    let view = Size::from((1280., 720.));
    let area = Rectangle::new(Point::from((17.25, 11.5)), Size::from((1001., 601.)));
    let clock = Clock::with_time(Duration::ZERO);
    let mut options = Options::default();
    options.layout.gaps = 15.;
    options.layout.border.off = false;
    options.layout.border.width = 2.;
    let options = Rc::new(options);
    let scene_resources = SceneResources::new();
    let space = DwindleSpace::new(
        view,
        area,
        1.25,
        clock.clone(),
        options.clone(),
        scene_resources.allocator(),
    );
    (scene_resources, space, view, clock, options)
}

fn tile(
    id: usize,
    view: Size<f64, Logical>,
    clock: &Clock,
    options: &Rc<Options>,
    scene_resources: &SceneResources,
) -> Tile<TestWindow> {
    Tile::new(
        TestWindow::new(TestWindowParams::new(id)),
        view,
        1.25,
        clock.clone(),
        options.clone(),
        scene_resources.allocator().windows.allocate(),
    )
}

fn nested_tree() -> SessionTree {
    SessionTree::Split {
        vertical: false,
        ratio: 0.37,
        first: Box::new(SessionTree::Split {
            vertical: true,
            ratio: 0.4,
            first: Box::new(SessionTree::Leaf(0)),
            second: Box::new(SessionTree::Leaf(1)),
        }),
        second: Box::new(SessionTree::Leaf(2)),
    }
}

fn requested(space: &DwindleSpace<TestWindow>, id: usize) -> Size<i32, Logical> {
    space
        .tiles()
        .find(|tile| *tile.window().id() == id)
        .unwrap()
        .window()
        .requested_size()
        .unwrap()
}

#[test]
fn dwindle_core_nested_slots_preserve_internal_resize_geometry() {
    let (scene_resources, mut space, view, clock, options) = fixture();
    for id in 0..3 {
        space.add_tile(
            tile(id, view, &clock, &options, &scene_resources),
            ActivateWindow::Yes,
        );
    }
    let tree = nested_tree();
    space.restore_session_tree(&tree, &[Some(0), Some(1), Some(2)]);
    assert_eq!(
        serde_json::to_value(space.session_tree(&|id| *id)).unwrap(),
        serde_json::to_value(Some(tree)).unwrap()
    );
    assert_eq!(
        space.tiled_positions(),
        vec![
            (0, (17.25, 11.5).into()),
            (1, (17.25, 260.5).into()),
            (2, (397.25, 11.5).into())
        ]
    );
    // At 1.25 scale the 2-logical-pixel border is rounded to 3 physical
    // pixels (2.4 logical); subtract 4.8 once, then floor the client request.
    assert_eq!(requested(&space, 0), (360, 229).into());
    assert_eq!(requested(&space, 1), (360, 347).into());
    assert_eq!(requested(&space, 2), (616, 596).into());

    assert!(space.interactive_resize_begin(0, ResizeEdge::RIGHT | ResizeEdge::BOTTOM));
    assert!(space.interactive_resize_update(&0, (50., 30.).into()));
    space.interactive_resize_end(Some(&0));
    assert_eq!(
        space.tiled_positions(),
        vec![
            (0, (17.25, 11.5).into()),
            (1, (17.25, 290.5).into()),
            (2, (447.25, 11.5).into())
        ]
    );
    assert_eq!(requested(&space, 0), (410, 259).into());
    assert_eq!(requested(&space, 1), (410, 317).into());
    assert_eq!(requested(&space, 2), (566, 596).into());
}

#[test]
fn dwindle_core_client_hints_do_not_change_reserved_slots_or_double_subtract_borders() {
    let (scene_resources, mut space, view, clock, options) = fixture();
    let params = TestWindowParams {
        min_max_size: ((900, 900).into(), (950, 950).into()),
        ..TestWindowParams::new(0)
    };
    space.add_tile(
        Tile::new(
            TestWindow::new(params),
            view,
            1.25,
            clock.clone(),
            options.clone(),
            scene_resources.allocator().windows.allocate(),
        ),
        ActivateWindow::Yes,
    );
    space.add_tile(
        tile(1, view, &clock, &options, &scene_resources),
        ActivateWindow::Yes,
    );
    assert_eq!(
        space.tiled_positions(),
        vec![(0, (17.25, 11.5).into()), (1, (525.25, 11.5).into())]
    );
    assert_eq!(requested(&space, 0), (488, 596).into());
    assert_eq!(requested(&space, 1), (488, 596).into());
    // Expanded client requests bypass normal tile borders, while the hidden
    // allocation remains available when the window returns to normal sizing.
    assert!(space.set_fullscreen(&0, true));
    assert_eq!(requested(&space, 0), (1280, 720).into());
    let mut intended = None;
    space.with_layout_geometry(&mut |tile, geometry| {
        if *tile.window().id() == 0 {
            intended = Some(geometry);
        }
    });
    assert_eq!(intended, Some(Rectangle::from_size(view)));
    assert!(space.set_fullscreen(&0, false));
    assert_eq!(requested(&space, 0), (488, 596).into());
    space.with_layout_geometry(&mut |tile, geometry| {
        if *tile.window().id() == 0 {
            intended = Some(geometry);
        }
    });
    assert_eq!(
        intended,
        Some(Rectangle::new((17.25, 11.5).into(), (493., 601.).into()))
    );
    assert!(space.set_maximized(&0, true));
    assert_eq!(requested(&space, 0), (1001, 601).into());
    space.with_layout_geometry(&mut |tile, geometry| {
        if *tile.window().id() == 0 {
            intended = Some(geometry);
        }
    });
    assert_eq!(
        intended,
        Some(Rectangle::new((17.25, 11.5).into(), (1001., 601.).into()))
    );
}

#[test]
fn dwindle_core_batch_smart_insertion_matches_incremental_topology_and_sizes() {
    let (scene_resources, mut incremental, view, clock, options) = fixture();
    let (batch_scene_resources, mut batch, _, _, _) = fixture();
    for id in 0..12 {
        incremental.add_tile(
            tile(id, view, &clock, &options, &scene_resources),
            ActivateWindow::Yes,
        );
    }
    batch.add_tiles(
        (0..12).map(|id| tile(id, view, &clock, &options, &batch_scene_resources)),
        ActivateWindow::Yes,
    );
    assert_eq!(
        serde_json::to_value(batch.session_tree(&|id| *id)).unwrap(),
        serde_json::to_value(incremental.session_tree(&|id| *id)).unwrap()
    );
    assert_eq!(batch.tiled_positions(), incremental.tiled_positions());
    for id in 0..12 {
        assert_eq!(requested(&batch, id), requested(&incremental, id));
    }
}

#[test]
fn dwindle_core_split_cache_keeps_zero_width_leaves_and_fractional_origins() {
    let bounds = clarion_core::Rect::new(17.25, 11.5, 8., 6.);
    let mut tree = clarion_core::bsp::BspTree::new(bounds, 15., 32);
    assert!(tree.insert(0, None, None));
    assert!(tree.insert(1, Some(&0), Some(clarion_core::Axis::Horizontal)));
    assert_eq!(
        tree.geometry(&0),
        Some(clarion_core::Rect::new(17.25, 11.5, 0., 6.))
    );
    assert_eq!(
        tree.geometry(&1),
        Some(clarion_core::Rect::new(25.25, 11.5, 0., 6.))
    );
}

#[test]
fn dwindle_policy_memory_prunes_closed_ids_inserts_new_ids_and_preserves_tile_handles() {
    let (scene_resources, mut space, view, clock, options) = fixture();
    for id in 0..3 {
        space.add_tile(
            tile(id, view, &clock, &options, &scene_resources),
            ActivateWindow::Yes,
        );
    }
    space.restore_session_tree(&nested_tree(), &[Some(0), Some(1), Some(2)]);
    space.set_active_window(0);
    assert!(space.set_fullscreen(&0, true));
    let snapshot = space.policy_snapshot();
    let original = space
        .tiles()
        .find(|tile| *tile.window().id() == 0)
        .unwrap()
        .window()
        .clone();
    space.remove_window(&1).unwrap();
    space.add_tile(
        tile(3, view, &clock, &options, &scene_resources),
        ActivateWindow::Yes,
    );
    assert!(space.set_maximized(&3, true));
    space.set_fullscreen(&0, false);
    space.restore_policy(&snapshot);
    assert_eq!(space.active_window_id(), Some(&0));
    assert!(!space.has_window(&1));
    assert!(space.has_window(&3));
    assert_eq!(
        space
            .tiles()
            .map(|tile| *tile.window().id())
            .collect::<Vec<_>>(),
        vec![0, 3, 2]
    );
    let restored = space
        .tiles()
        .find(|tile| *tile.window().id() == 0)
        .unwrap()
        .window();
    assert!(Rc::ptr_eq(&original.0, &restored.0));
    assert_eq!(restored.pending_sizing_mode(), SizingMode::Fullscreen);
    assert_eq!(requested(&space, 0), (1280, 720).into());
    assert_eq!(
        space
            .tiles()
            .find(|tile| *tile.window().id() == 3)
            .unwrap()
            .window()
            .pending_sizing_mode(),
        SizingMode::Maximized
    );
    assert_eq!(
        serde_json::to_value(space.session_tree(&|id| *id)).unwrap()["Split"]["ratio"],
        serde_json::json!(0.37)
    );
}

#[test]
fn dwindle_policy_memory_normal_state_clears_later_fullscreen_and_invalid_restore_is_atomic() {
    let (scene_resources, mut space, view, clock, options) = fixture();
    for id in 0..2 {
        space.add_tile(
            tile(id, view, &clock, &options, &scene_resources),
            ActivateWindow::Yes,
        );
    }
    let normal = space.policy_snapshot();
    space.set_fullscreen(&0, true);
    space.restore_policy(&normal);
    assert_eq!(
        space
            .tiles()
            .find(|tile| *tile.window().id() == 0)
            .unwrap()
            .window()
            .pending_sizing_mode(),
        SizingMode::Normal
    );
    assert_eq!(requested(&space, 0), (488, 596).into());
    let before = serde_json::to_value(space.session_tree(&|id| *id)).unwrap();
    let mut invalid = normal.clone();
    invalid.active = Some(0);
    invalid.expanded = vec![(0, clarion_core::bsp::PaneMode::Fullscreen)];
    if let Some(clarion_core::layout::BspNode::Split { ratio, .. }) = &mut invalid.topology {
        *ratio = f64::NAN;
    }
    space.restore_policy(&invalid);
    assert_eq!(space.active_window_id(), Some(&1));
    assert_eq!(
        serde_json::to_value(space.session_tree(&|id| *id)).unwrap(),
        before
    );
    assert_eq!(
        space
            .tiles()
            .find(|tile| *tile.window().id() == 0)
            .unwrap()
            .window()
            .pending_sizing_mode(),
        SizingMode::Normal
    );
}
