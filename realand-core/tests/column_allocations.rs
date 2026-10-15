//! Verify retained buffers avoid allocator churn during steady policy updates.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use realand_core::column::{resolve_heights_into, Height, HeightConstraint, HeightScratch};

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

struct Allocator;

fn record_allocation() {
    // TLS may be unavailable while a test thread is being destroyed.
    let _ = COUNTING.try_with(|counting| {
        if counting.get() {
            let _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
        }
    });
}

// The wrapper forwards every allocation to System without changing its contract.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record_allocation();
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record_allocation();
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record_allocation();
        unsafe { System.realloc(pointer, layout, size) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

#[test]
fn repeated_dirty_updates_with_warm_buffers_allocate_nothing() {
    let windows = [HeightConstraint {
        height: Height::Automatic { weight: 1. },
        minimum: 1.,
        maximum: 0.,
        decoration: 0.,
    }; 8];
    let mut scratch = HeightScratch::default();
    let mut heights = Vec::new();
    resolve_heights_into(1024., 8., false, 0., &windows, &mut heights, &mut scratch);
    ALLOCATIONS.with(|count| count.set(0));
    COUNTING.with(|count| count.set(true));
    for step in 0..1000 {
        resolve_heights_into(
            1024. + (step % 2) as f64,
            8.,
            false,
            0.,
            &windows,
            &mut heights,
            &mut scratch,
        );
    }
    COUNTING.with(|count| count.set(false));
    assert_eq!(ALLOCATIONS.with(Cell::get), 0);
    assert_eq!(heights.len(), 8);
    assert_eq!(heights.iter().sum::<f64>(), 1025. - 8. * 9.);
}

#[test]
fn geometry_effect_buffer_reuse_adds_no_per_event_container_allocation() {
    use realand_core::{CoreEvent, DesktopCore, Rect, SpaceId};
    let mut core = DesktopCore::new();
    core.apply(CoreEvent::OutputRegistered {
        connector: "DP-1".into(),
        bounds: Rect::new(0., 0., 1920., 1080.),
    });
    core.add_window(
        1u64,
        SpaceId(1),
        Some("DP-1".into()),
        Rect::new(0., 0., 100., 80.),
    );
    let mut effects = Vec::with_capacity(4);
    ALLOCATIONS.with(|count| count.set(0));
    COUNTING.with(|count| count.set(true));
    for step in 0..1000 {
        effects.clear();
        core.apply_into(
            CoreEvent::CommitGeometry {
                id: 1,
                geometry: Rect::new((step % 2 + 1) as f64, 0., 100., 80.),
            },
            &mut effects,
        );
        assert_eq!(effects.len(), 1);
    }
    COUNTING.with(|count| count.set(false));
    // Owned connector names currently need one allocation per changed event.
    // A fresh Vec per geometry would double that; retained storage must not.
    assert!(ALLOCATIONS.with(Cell::get) <= 1000);
}

#[test]
fn all_live_layout_modes_reuse_warm_geometry_without_snapshot_allocations() {
    use realand_core::bsp::{BspArrangement, BspTree};
    use realand_core::floating::{FloatingPolicy, Placement};
    use realand_core::layout::LiveLayoutPolicy;
    use realand_core::scrolling::{ColumnArrangement, ColumnDisplay, ColumnPolicy, ColumnWidth};
    use realand_core::{Rect, Size};
    let bounds = Rect::new(0., 0., 1920., 1080.);
    let mut bsp = BspTree::new(bounds, 16., 32);
    let mut floating = FloatingPolicy::new();
    let mut column = ColumnPolicy::new(
        100,
        ColumnWidth::Fixed(640.),
        false,
        ColumnDisplay::Normal,
        None,
    );
    for id in 1..=16u64 {
        bsp.insert(id, Some(&(id - 1)), None);
        floating.insert(
            id,
            Placement::new(
                bounds,
                Size::new(100., 80.),
                [id as f64 * 10., id as f64 * 5.],
                false,
            ),
            floating.len(),
            false,
            &[],
        );
        column.insert(column.rows().len(), id);
        column.height_constraints.push(HeightConstraint {
            height: Height::Automatic { weight: 1. },
            minimum: 1.,
            maximum: 0.,
            decoration: 4.,
        });
    }
    let request = |height| ColumnArrangement::Normal {
        width: 640.,
        working_height: height,
        gap: 16.,
        tabbed_limit: height - 32.,
    };
    for mode in [
        ColumnDisplay::Normal,
        ColumnDisplay::Tabbed,
        ColumnDisplay::Dwindle,
    ] {
        column.set_display(mode);
        column.arrange(request(1080.), |id, rect| {
            std::hint::black_box((*id, rect));
        });
    }
    let mut emitted = 0;
    ALLOCATIONS.with(|count| count.set(0));
    COUNTING.with(|count| count.set(true));
    for step in 0..1000 {
        let bounds = Rect::new(0., 0., 1920. + (step % 2) as f64, 1080.);
        let mut consume = |id: &u64, rect| {
            emitted += 1;
            std::hint::black_box((*id, rect));
        };
        bsp.arrange(BspArrangement { bounds, gap: 16. }, &mut consume);
        floating.arrange(bounds, &mut consume);
        for mode in [
            ColumnDisplay::Normal,
            ColumnDisplay::Tabbed,
            ColumnDisplay::Dwindle,
        ] {
            column.set_display(mode);
            column.arrange(request(1080. + (step % 2) as f64), &mut consume);
        }
    }
    COUNTING.with(|count| count.set(false));
    assert_eq!(ALLOCATIONS.with(Cell::get), 0);
    assert_eq!(emitted, 1000 * 16 * 5);
}
