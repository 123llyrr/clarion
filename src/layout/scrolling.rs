use std::cmp::max;
use std::iter::{self, zip};
use std::rc::Rc;
use std::time::Duration;

use clarion_config::utils::MergeWith as _;
use clarion_config::{CenterFocusedColumn, PresetSize, Struts};
use clarion_core::layout::LiveLayoutPolicy;
use clarion_core::scrolling::{
    ColumnArrangement, ColumnDisplay, ColumnOrder, ColumnPolicy, DimensionChange, HeightContext,
    WidthContext,
};
pub use clarion_core::scrolling::{ColumnWidth, WindowHeight};
use clarion_ipc::{ColumnDisplay as IpcColumnDisplay, SizeChange, WindowLayout};
use ordered_float::NotNan;
use smithay::utils::{Logical, Point, Rectangle, Scale, Serial, Size};

use super::monitor::InsertPosition;
use super::tile::{Tile, TileRenderElement};
use super::wobble::WobbleAnchor;
use super::workspace::ResolvedSize;
use super::{ConfigureIntent, HitType, InteractiveResizeData, LayoutElement, Options, RemovedTile};
use crate::animation::{Animation, Clock};
use crate::clarion_render_elements;
use crate::layout::{RenderLayer, SizingMode};
use crate::render_helpers::renderer::{AnyRenderer, ClarionRenderer};
use crate::render_helpers::xray::XrayPos;
use crate::render_helpers::RenderCtx;
use crate::scene::closing_window::{ClosingWindow, ClosingWindowRenderElement};
use crate::scene::resources::SceneResourceAllocator;
use crate::scene::tab_indicator::{TabIndicator, TabIndicatorRenderElement, TabInfo};
use crate::utils::transaction::{Transaction, TransactionBlocker};
use crate::utils::ResizeEdge;
use crate::window::ResolvedWindowRules;

/// A scrollable-tiling space for windows.
#[derive(Debug)]
pub struct ScrollingSpace<W: LayoutElement> {
    scene_resources: SceneResourceAllocator,
    /// Columns of windows on this space.
    columns: Vec<Column<W>>,

    /// Extra per-column data.
    data: Vec<ColumnData>,

    /// Device-independent column identity/order/focus policy.
    order: ColumnOrder,

    /// Offset of the view computed from the active column.
    ///
    /// Any gaps, including left padding from work area left exclusive zone, is handled
    /// with this view offset (rather than added as a constant elsewhere in the code). This allows
    /// for natural handling of fullscreen windows, which must ignore work area padding.
    view_offset: ViewOffset,

    /// Windows in the closing animation.
    pub(super) closing_windows: Vec<ClosingWindow>,

    /// View size for this space.
    view_size: Size<f64, Logical>,

    /// Working area for this space.
    ///
    /// Takes into account layer-shell exclusive zones and clarion struts.
    working_area: Rectangle<f64, Logical>,

    /// Working area for this space excluding struts.
    ///
    /// Used for popup unconstraining. Popups can go over struts, but they shouldn't go over
    /// the layer-shell top layer (which renders on top of popups).
    parent_area: Rectangle<f64, Logical>,

    /// Scale of the output the space is on (and rounds its sizes to).
    scale: f64,

    /// Clock for driving animations.
    clock: Clock,

    /// Configurable properties of the layout.
    options: Rc<Options>,
}

clarion_render_elements! {
    ScrollingSpaceRenderElement<R> => {
        Tile = TileRenderElement<R>,
        ClosingWindow = ClosingWindowRenderElement,
        TabIndicator = TabIndicatorRenderElement,
    }
}

/// Extra per-column data.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ColumnData {
    /// Cached actual column width.
    width: f64,
}

/// Clock/configuration adapter. All logical offsets, trajectory endpoints,
/// gesture state and lifecycle clocks belong to the single pure policy.
#[derive(Debug)]
pub(super) struct ViewOffset {
    policy: clarion_core::viewport::Viewport,
    clock: Clock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ColumnId(u64);

impl ColumnId {
    fn next() -> ColumnId {
        ColumnId(clarion_core::scrolling::allocate_column_id())
    }

    pub fn get(self) -> u64 {
        self.0
    }

    pub fn specific(id: u64) -> Self {
        assert!(
            clarion_core::scrolling::reserve_column_id(id),
            "column identity exhausted"
        );
        Self(id)
    }
}

#[derive(Debug)]
pub struct Column<W: LayoutElement> {
    /// Tiles in this column.
    ///
    /// Must be non-empty.
    tiles: Vec<Tile<W>>,

    /// Extra per-tile data.
    ///
    /// Must have the same number of elements as `tiles`.
    data: Vec<TileData>,

    /// Authoritative ID-only row/focus/size policy; tiles/data hold physical payloads.
    policy: ColumnPolicy<u64>,

    /// 2D relative offsets for tiles when in Dwindle display mode.
    dwindle_offsets: Rc<Vec<Point<f64, Logical>>>,

    /// Tab indicator for the tabbed display mode.
    tab_indicator: TabIndicator,

    /// Animation of the render offset during window swapping.
    move_x_animation: Option<MoveAnimation>,

    /// Animation of a column visually moving vertically.
    move_y_animation: Option<MoveAnimation>,

    /// Latest known view size for this column's workspace.
    view_size: Size<f64, Logical>,

    /// Latest known working area for this column's workspace.
    working_area: Rectangle<f64, Logical>,

    /// Working area for this column's workspace excluding struts.
    ///
    /// Used for maximize-to-edges.
    parent_area: Rectangle<f64, Logical>,

    /// Scale of the output the column is on (and rounds its sizes to).
    scale: f64,

    /// Clock for driving animations.
    clock: Clock,

    /// Configurable properties of the layout.
    options: Rc<Options>,
}

/// Extra per-tile data.
#[derive(Debug, Clone, Copy, PartialEq)]
struct TileData {
    /// Cached actual size of the tile.
    size: Size<f64, Logical>,

    /// Cached whether the tile is being interactively resized by its left edge.
    interactively_resizing_by_left_edge: bool,
}

/// Horizontal direction for an operation.
///
/// As operations often have a symmetrical counterpart, e.g. focus-right/focus-left, methods
/// on `Scrolling` can sometimes be factored using the direction of the operation as a parameter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScrollDirection {
    Left,
    Right,
}

#[derive(Debug)]
struct MoveAnimation {
    anim: Animation,
    from: f64,
    /// Whether this animation is for moving the tile between workspaces.
    ///
    /// Controls whether the tile is rendered uncropped and above others.
    is_between_workspaces: bool,
}

impl MoveAnimation {
    fn retarget(
        previous: Option<Self>,
        offset: f64,
        clock: Clock,
        config: clarion_config::Animation,
    ) -> Option<Self> {
        // Moving the other axis must not restart this axis, especially when a
        // custom easing curve is already in its slow final part.
        if offset == 0. {
            return previous;
        }

        let (current, velocity, is_between_workspaces) =
            previous.map_or((0., 0., false), |previous| {
                (
                    previous.from * previous.anim.value(),
                    previous.from * previous.anim.velocity(),
                    previous.is_between_workspaces,
                )
            });
        let from = offset + current;
        // Keep an independent amplitude: the new offset may be zero while the
        // column still has momentum, or too small to safely normalize speed.
        let amplitude = from.abs().max(1.);
        Some(Self {
            anim: Animation::new(clock, from / amplitude, 0., velocity / amplitude, config),
            from: amplitude,
            is_between_workspaces,
        })
    }

    fn offset_current(&mut self, offset: f64) {
        // If the anim is almost done, there's little point trying to offset it;
        // we can let things jump rather than amplify a tiny remaining value.
        let value = self.anim.value();
        if value.abs() > 0.001 {
            self.from += offset / value;
        }
    }
}

impl<W: LayoutElement> ScrollingSpace<W> {
    pub fn new(
        view_size: Size<f64, Logical>,
        parent_area: Rectangle<f64, Logical>,
        scale: f64,
        clock: Clock,
        options: Rc<Options>,
        scene_resources: SceneResourceAllocator,
    ) -> Self {
        let working_area = compute_working_area(parent_area, scale, options.layout.struts);

        Self {
            scene_resources,
            columns: Vec::new(),
            data: Vec::new(),
            order: ColumnOrder::default(),
            view_offset: ViewOffset::new(clock.clone(), 0.),
            closing_windows: Vec::new(),
            view_size,
            working_area,
            parent_area,
            scale,
            clock,
            options,
        }
    }

    pub fn update_config(
        &mut self,
        view_size: Size<f64, Logical>,
        parent_area: Rectangle<f64, Logical>,
        scale: f64,
        options: Rc<Options>,
    ) {
        let working_area = compute_working_area(parent_area, scale, options.layout.struts);

        for (column, data) in zip(&mut self.columns, &mut self.data) {
            column.update_config(view_size, working_area, parent_area, scale, options.clone());
            data.update(column);
        }

        self.view_size = view_size;
        self.working_area = working_area;
        self.parent_area = parent_area;
        self.scale = scale;
        self.options = options;

        // Apply always-center and such right away.
        if !self.columns.is_empty() && !self.view_offset.is_gesture() {
            self.animate_view_offset_to_column(None, self.order.active_index(), None);
        }
    }

    pub fn update_shaders(&mut self) {
        for col in &mut self.columns {
            col.update_shaders();
        }
    }

    pub fn advance_animations(&mut self) {
        self.view_offset.advance();

        for col in &mut self.columns {
            col.advance_animations();
        }

        self.closing_windows.retain_mut(|closing| {
            closing.advance_animations();
            closing.are_animations_ongoing()
        });
    }

    pub fn are_animations_ongoing(&self) -> bool {
        self.view_offset.is_animation_ongoing()
            || self.columns.iter().any(Column::are_animations_ongoing)
            || !self.closing_windows.is_empty()
    }

    pub fn are_transitions_ongoing(&self) -> bool {
        self.view_offset.is_animation_ongoing()
            || self.columns.iter().any(Column::are_transitions_ongoing)
            || !self.closing_windows.is_empty()
    }

    pub fn has_closing_windows(&self) -> bool {
        !self.closing_windows.is_empty()
    }

    pub fn take_closing_windows(&mut self) -> Vec<ClosingWindow> {
        std::mem::take(&mut self.closing_windows)
    }

    pub fn extend_closing_windows(&mut self, closing: Vec<ClosingWindow>) {
        self.closing_windows.extend(closing);
    }

    pub(super) fn policy_snapshot(&self) -> clarion_core::policy_memory::ScrollingSnapshot<u64> {
        use clarion_core::policy_memory::{
            ColumnDisplay as Display, ColumnSnapshot, Height, ScrollingSnapshot, Width,
        };
        ScrollingSnapshot {
            columns: self
                .columns
                .iter()
                .map(|column| ColumnSnapshot {
                    id: column.policy.id,
                    windows: column
                        .tiles
                        .iter()
                        .map(|tile| tile.window().core_id())
                        .collect(),
                    heights: column
                        .policy
                        .rows()
                        .iter()
                        .map(|row| match row.height {
                            WindowHeight::Auto { weight } => Height::Auto(weight),
                            WindowHeight::Fixed(height) => Height::Fixed(height),
                            WindowHeight::Preset(index) => Height::Preset(index),
                        })
                        .collect(),
                    width: match column.policy.width {
                        ColumnWidth::Fixed(width) => Width::Fixed(width),
                        ColumnWidth::Proportion(width) => Width::Proportion(width),
                    },
                    preset_width: column.policy.preset_width_idx,
                    full_width: column.policy.is_full_width,
                    fullscreen: column.policy.is_pending_fullscreen,
                    maximized: column.policy.is_pending_maximized,
                    display: match column.policy.display_mode {
                        ColumnDisplay::Normal => Display::Normal,
                        ColumnDisplay::Tabbed => Display::Tabbed,
                        ColumnDisplay::Dwindle => Display::Dwindle,
                    },
                    active: column
                        .tiles
                        .get(column.policy.active_index())
                        .map(|tile| tile.window().core_id()),
                })
                .collect(),
            active: self.active_window().map(|window| window.core_id()),
            active_column: self
                .columns
                .get(self.order.active_index())
                .map(|column| column.policy.id),
            view_offset: self.view_offset.stationary(),
            view_offset_to_restore: self.order.view_offset_to_restore,
        }
    }

    /// Restore ID-only policy history using the current physical tile handles.
    /// Closed windows are pruned; newly mapped windows append in input order.
    pub(super) fn restore_policy(
        &mut self,
        snapshot: &clarion_core::policy_memory::ScrollingSnapshot<u64>,
        tiles: Vec<Tile<W>>,
        active: Option<u64>,
    ) {
        use std::collections::{HashMap, HashSet};

        use clarion_core::policy_memory::{ColumnDisplay as Display, Height, Width};
        let order: Vec<_> = tiles.iter().map(|tile| tile.window().core_id()).collect();
        let snapshot = snapshot.retain(&order.iter().cloned().collect::<HashSet<_>>());
        let mut available: HashMap<_, _> = tiles
            .into_iter()
            .map(|tile| (tile.window().core_id(), tile))
            .collect();
        self.drain_tiles();
        for saved in &snapshot.columns {
            let mut tiles = saved.windows.iter().filter_map(|id| available.remove(id));
            let Some(first) = tiles.next() else {
                continue;
            };
            let width = match saved.width {
                Width::Fixed(width) => ColumnWidth::Fixed(width),
                Width::Proportion(width) => ColumnWidth::Proportion(width),
            };
            let mut column = Column::new_with_tile(
                first,
                self.view_size,
                self.working_area,
                self.parent_area,
                self.scale,
                width,
                saved.full_width,
            );
            // Normalize inherited expanded state before restoring grouped rows.
            column.set_fullscreen(false);
            column.set_maximized(false);
            column.policy.display_mode = match saved.display {
                Display::Normal => ColumnDisplay::Normal,
                Display::Tabbed => ColumnDisplay::Tabbed,
                Display::Dwindle => ColumnDisplay::Dwindle,
            };
            for tile in tiles {
                column.add_tile_at(column.tiles.len(), tile);
            }
            column.policy.adopt_id(saved.id);
            column.policy.preset_width_idx = saved
                .preset_width
                .filter(|index| *index < self.options.layout.preset_column_widths.len());
            for (index, row) in column.policy.rows_mut().iter_mut().enumerate() {
                row.height = match saved
                    .heights
                    .get(index)
                    .copied()
                    .unwrap_or(Height::Auto(1.0))
                {
                    Height::Auto(weight) => WindowHeight::Auto { weight },
                    Height::Fixed(height) => WindowHeight::Fixed(height),
                    Height::Preset(index)
                        if index < self.options.layout.preset_window_heights.len() =>
                    {
                        WindowHeight::Preset(index)
                    }
                    Height::Preset(_) => WindowHeight::auto_1(),
                };
            }
            if column.data.len() == 1 {
                if let WindowHeight::Auto { weight } = &mut column.policy.rows_mut()[0].height {
                    *weight = 1.0;
                }
            }
            column.policy.activate(
                saved
                    .active
                    .as_ref()
                    .and_then(|id| {
                        column
                            .tiles
                            .iter()
                            .position(|tile| tile.window().core_id() == *id)
                    })
                    .unwrap_or(0),
            );
            let expandable =
                column.tiles.len() == 1 || column.policy.display_mode == ColumnDisplay::Tabbed;
            column.set_maximized(saved.maximized && expandable);
            column.set_fullscreen(saved.fullscreen && expandable);
            column.update_tile_sizes(false);
            self.data.push(ColumnData::new(&column));
            assert!(self.order.insert(self.columns.len(), column.policy.id));
            self.columns.push(column);
        }
        let default_width = self
            .options
            .layout
            .default_column_width
            .map(column_width_from_preset)
            .unwrap_or(ColumnWidth::Proportion(0.5));
        for id in &order {
            if let Some(tile) = available.remove(id) {
                self.add_tile(
                    Some(self.columns.len()),
                    tile,
                    false,
                    default_width,
                    false,
                    None,
                );
            }
        }
        if self.columns.is_empty() {
            return;
        }
        let focused = active
            .as_ref()
            .filter(|id| {
                self.columns.iter().any(|column| {
                    column
                        .tiles
                        .iter()
                        .any(|tile| tile.window().core_id() == **id)
                })
            })
            .or(snapshot.active.as_ref());
        let active_index = focused
            .and_then(|id| {
                self.columns.iter().position(|column| {
                    column
                        .tiles
                        .iter()
                        .any(|tile| tile.window().core_id() == *id)
                })
            })
            .or_else(|| {
                snapshot.active_column.and_then(|id| {
                    self.columns
                        .iter()
                        .position(|column| column.policy.id == id)
                })
            })
            .unwrap_or(0);
        self.order.activate(active_index);
        if let Some(id) = focused {
            if let Some(index) = self.columns[self.order.active_index()]
                .tiles
                .iter()
                .position(|tile| tile.window().core_id() == *id)
            {
                self.columns[self.order.active_index()]
                    .policy
                    .activate(index);
            }
        }
        self.view_offset = if focused == snapshot.active.as_ref() {
            ViewOffset::new(self.clock.clone(), snapshot.view_offset)
        } else {
            ViewOffset::new(
                self.clock.clone(),
                self.compute_new_view_offset_for_column(None, self.order.active_index(), None),
            )
        };
        self.order.view_offset_to_restore = snapshot.view_offset_to_restore;
    }

    /// Take all tiles out of the space, leaving it empty.
    ///
    /// Used to migrate windows to a different tiling layout when switching tiling modes.
    pub fn drain_tiles(&mut self) -> Vec<Tile<W>> {
        let mut tiles = Vec::new();
        for mut column in self.columns.drain(..) {
            tiles.append(&mut column.tiles);
        }

        self.data.clear();
        self.order.clear();
        self.order.resize = None;
        self.view_offset = ViewOffset::new(self.clock.clone(), 0.);
        self.order.activate_previous_on_removal = None;
        self.order.view_offset_to_restore = None;

        tiles
    }

    pub fn update_render_elements(&mut self, is_active: bool, layer: RenderLayer) {
        let view_pos = Point::from((self.view_pos(), 0.));
        let view_size = self.view_size;
        let active_idx = self.order.active_index();
        for (col_idx, (col, col_x)) in self.columns_mut().enumerate() {
            // Skip columns belonging to a different render layer.
            if layer.is_normal() == col.is_moving_between_workspaces() {
                continue;
            }

            let is_active = is_active && col_idx == active_idx;
            let col_off = Point::from((col_x, 0.));
            let col_pos = view_pos - col_off - col.render_offset();
            let view_rect = Rectangle::new(col_pos, view_size);
            col.update_render_elements(is_active, view_rect);
        }
    }

    pub fn tiles(&self) -> impl Iterator<Item = &Tile<W>> + '_ {
        self.columns.iter().flat_map(|col| col.tiles.iter())
    }

    pub fn tiles_mut(&mut self) -> impl Iterator<Item = &mut Tile<W>> + '_ {
        self.columns.iter_mut().flat_map(|col| col.tiles.iter_mut())
    }

    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }

    pub fn active_window(&self) -> Option<&W> {
        if self.columns.is_empty() {
            return None;
        }

        let col = &self.columns[self.order.active_index()];
        Some(col.tiles[col.policy.active_index()].window())
    }

    pub fn active_window_mut(&mut self) -> Option<&mut W> {
        if self.columns.is_empty() {
            return None;
        }

        let col = &mut self.columns[self.order.active_index()];
        Some(col.tiles[col.policy.active_index()].window_mut())
    }

    pub fn active_tile_mut(&mut self) -> Option<&mut Tile<W>> {
        if self.columns.is_empty() {
            return None;
        }

        let col = &mut self.columns[self.order.active_index()];
        Some(&mut col.tiles[col.policy.active_index()])
    }

    pub fn is_active_pending_fullscreen(&self) -> bool {
        if self.columns.is_empty() {
            return false;
        }

        let col = &self.columns[self.order.active_index()];
        col.pending_sizing_mode().is_fullscreen()
    }

    pub fn active_column_is_normal(&self) -> bool {
        self.columns
            .get(self.order.active_index())
            .is_none_or(|col| col.pending_sizing_mode().is_normal())
    }

    pub fn can_auto_stack_new_window(&self) -> bool {
        self.options.layout.auto_stack_new_windows
            && !self.is_empty()
            && self.active_column_tile_count() < self.options.layout.max_windows_per_column as usize
            && self.active_column_is_normal()
    }

    /// Predict the cell used by automatic insertion before the client draws its
    /// first buffer. Use the same column constraints and spiral as the eventual
    /// arrangement, without changing membership, focus or animation state.
    pub fn auto_stack_new_window_size(
        &self,
        rules: &ResolvedWindowRules,
        (min_size, max_size): (Size<i32, Logical>, Size<i32, Logical>),
    ) -> Option<Size<i32, Logical>> {
        if !self.can_auto_stack_new_window() {
            return None;
        }
        let column = &self.columns[self.order.active_index()];
        // Tabbed insertion keeps its display mode and has a separate indicator
        // extent; only normal columns turn into the spiral arrangement.
        if column.policy.display_mode == ColumnDisplay::Tabbed {
            return None;
        }

        let border = self.options.layout.border.merged_with(&rules.border);
        let decoration = if border.off {
            0.
        } else {
            crate::utils::round_logical_in_physical_max1(self.scale, border.width) * 2.
        };
        let minimum = f64::from(min_size.w.max(1)) + decoration;
        let maximum = if max_size.w > 0 {
            f64::from(max_size.w) + decoration
        } else {
            0.
        };
        let constraints = column.tiles.iter().map(|tile| {
            (
                tile.min_size_nonfullscreen().w.max(1.),
                tile.max_size_nonfullscreen().w,
            )
        });
        let width = column.policy.arranged_width(
            self.working_area.size.w,
            self.options.layout.gaps,
            0.,
            constraints.chain(iter::once((minimum, maximum))),
        );
        let height = (self.working_area.size.h - self.options.layout.gaps * 2.).max(1.);
        let mut cells = Vec::new();
        clarion_core::geometry::spiral_rects_into(
            clarion_core::Rect::new(0., 0., width, height),
            column.tiles.len() + 1,
            self.options.layout.gaps,
            &mut cells,
        );
        let cell = cells.last().unwrap();
        // Match Tile::request_tile_size, including fractional border rounding
        // and the protocol's one-pixel floor for exhausted work areas.
        Some(
            Size::from((
                (cell.width.max(1.) - decoration).max(1.),
                (cell.height.max(1.) - decoration).max(1.),
            ))
            .to_i32_floor(),
        )
    }

    pub fn new_window_toplevel_bounds(&self, rules: &ResolvedWindowRules) -> Size<i32, Logical> {
        let border_config = self.options.layout.border.merged_with(&rules.border);

        let display_mode = rules
            .default_column_display
            .unwrap_or(self.options.layout.default_column_display);
        let will_tab = display_mode == IpcColumnDisplay::Tabbed;
        let extra_size = if will_tab {
            TabIndicator::new(self.options.layout.tab_indicator).extra_size(1, self.scale)
        } else {
            Size::from((0., 0.))
        };

        compute_toplevel_bounds(
            border_config,
            self.working_area.size,
            extra_size,
            self.options.layout.gaps,
        )
    }

    pub fn new_window_size(
        &self,
        width: Option<PresetSize>,
        height: Option<PresetSize>,
        rules: &ResolvedWindowRules,
    ) -> Size<i32, Logical> {
        let border = self.options.layout.border.merged_with(&rules.border);

        let display_mode = rules
            .default_column_display
            .unwrap_or(self.options.layout.default_column_display);
        let will_tab = display_mode == IpcColumnDisplay::Tabbed;
        let extra = if will_tab {
            TabIndicator::new(self.options.layout.tab_indicator).extra_size(1, self.scale)
        } else {
            Size::from((0., 0.))
        };

        let working_size = self.working_area.size;

        let width = if let Some(size) = width {
            let size = match resolve_preset_size(size, &self.options, working_size.w, extra.w) {
                ResolvedSize::Tile(mut size) => {
                    if !border.off {
                        size -= border.width * 2.;
                    }
                    size
                }
                ResolvedSize::Window(size) => size,
            };

            max(1, size.floor() as i32)
        } else {
            0
        };

        let mut full_height = self.working_area.size.h - self.options.layout.gaps * 2.;
        if !border.off {
            full_height -= border.width * 2.;
        }

        let height = if let Some(height) = height {
            let height = match resolve_preset_size(height, &self.options, working_size.h, extra.h) {
                ResolvedSize::Tile(mut size) => {
                    if !border.off {
                        size -= border.width * 2.;
                    }
                    size
                }
                ResolvedSize::Window(size) => size,
            };
            f64::min(height, full_height)
        } else {
            full_height
        };

        Size::from((width, max(height.floor() as i32, 1)))
    }

    pub fn is_centering_focused_column(&self) -> bool {
        clarion_core::viewport::focus_columns_centered(
            self.viewport_focus_context(),
            self.columns.len(),
        )
    }

    fn viewport_focus_context(&self) -> clarion_core::viewport::FocusContext {
        use clarion_core::viewport::{FocusCentering, FocusContext};
        let rect = |area: Rectangle<f64, Logical>| {
            clarion_core::Rect::new(area.loc.x, area.loc.y, area.size.w, area.size.h)
        };
        FocusContext {
            working: rect(self.working_area),
            parent: rect(self.parent_area),
            gaps: self.options.layout.gaps,
            centering: match self.options.layout.center_focused_column {
                CenterFocusedColumn::Always => FocusCentering::Always,
                CenterFocusedColumn::OnOverflow => FocusCentering::OnOverflow,
                CenterFocusedColumn::Never => FocusCentering::Never,
            },
            always_center_single: self.options.layout.always_center_single_column,
        }
    }

    fn viewport_column(&self, index: usize) -> clarion_core::viewport::PositionedColumn {
        let column = &self.columns[index];
        clarion_core::viewport::PositionedColumn {
            x: self.column_x(index),
            width: column.width(),
            sizing: core_column_sizing(column.sizing_mode()),
        }
    }

    fn compute_new_view_offset_fit(
        &self,
        target_x: Option<f64>,
        col_x: f64,
        width: f64,
        mode: SizingMode,
    ) -> f64 {
        clarion_core::viewport::fit_column_offset(
            target_x.unwrap_or_else(|| self.target_view_pos()),
            clarion_core::viewport::PositionedColumn {
                x: col_x,
                width,
                sizing: core_column_sizing(mode),
            },
            self.viewport_focus_context(),
        )
        .unwrap_or(0.)
    }

    fn compute_new_view_offset_centered(
        &self,
        target_x: Option<f64>,
        col_x: f64,
        width: f64,
        mode: SizingMode,
    ) -> f64 {
        clarion_core::viewport::center_column_offset(
            target_x.unwrap_or_else(|| self.target_view_pos()),
            clarion_core::viewport::PositionedColumn {
                x: col_x,
                width,
                sizing: core_column_sizing(mode),
            },
            self.viewport_focus_context(),
        )
        .unwrap_or(0.)
    }

    fn compute_new_view_offset_for_column_centered(
        &self,
        target_x: Option<f64>,
        index: usize,
    ) -> f64 {
        clarion_core::viewport::center_column_offset(
            target_x.unwrap_or_else(|| self.target_view_pos()),
            self.viewport_column(index),
            self.viewport_focus_context(),
        )
        .unwrap_or(0.)
    }

    fn compute_new_view_offset_for_column(
        &self,
        target_x: Option<f64>,
        index: usize,
        previous: Option<usize>,
    ) -> f64 {
        clarion_core::viewport::focus_column_offset(
            target_x.unwrap_or_else(|| self.target_view_pos()),
            index,
            previous,
            self.columns.len(),
            self.viewport_focus_context(),
            |index| self.viewport_column(index),
        )
        .unwrap_or(0.)
    }

    fn animate_view_offset(&mut self, idx: usize, new_view_offset: f64) {
        self.animate_view_offset_with_config(
            idx,
            new_view_offset,
            self.options.animations.horizontal_view_movement.0,
        );
    }

    fn animate_view_offset_with_config(
        &mut self,
        idx: usize,
        new_view_offset: f64,
        config: clarion_config::Animation,
    ) {
        let new_col_x = self.column_x(idx);
        let old_col_x = self.column_x(self.order.active_index());
        let offset_delta = old_col_x - new_col_x;
        self.view_offset.offset(offset_delta);

        self.view_offset
            .animate_to(new_view_offset, 1. / self.scale, config);
    }

    fn animate_view_offset_to_column_centered(
        &mut self,
        target_x: Option<f64>,
        idx: usize,
        config: clarion_config::Animation,
    ) {
        let new_view_offset = self.compute_new_view_offset_for_column_centered(target_x, idx);
        self.animate_view_offset_with_config(idx, new_view_offset, config);
    }

    fn animate_view_offset_to_column_with_config(
        &mut self,
        target_x: Option<f64>,
        idx: usize,
        prev_idx: Option<usize>,
        config: clarion_config::Animation,
    ) {
        let new_view_offset = self.compute_new_view_offset_for_column(target_x, idx, prev_idx);
        self.animate_view_offset_with_config(idx, new_view_offset, config);
    }

    fn animate_view_offset_to_column(
        &mut self,
        target_x: Option<f64>,
        idx: usize,
        prev_idx: Option<usize>,
    ) {
        self.animate_view_offset_to_column_with_config(
            target_x,
            idx,
            prev_idx,
            self.options.animations.horizontal_view_movement.0,
        )
    }

    fn activate_column(&mut self, idx: usize) {
        self.activate_column_with_anim_config(
            idx,
            self.options.animations.horizontal_view_movement.0,
        );
    }

    fn activate_column_with_anim_config(&mut self, idx: usize, config: clarion_config::Animation) {
        self.activate_column_from_with_anim_config(idx, config, self.order.active_index());
    }

    fn activate_column_from_with_anim_config(
        &mut self,
        idx: usize,
        config: clarion_config::Animation,
        previous: usize,
    ) {
        if previous == idx
            // During a DnD scroll, animate even when activating the same window, for DnD hold.
            && (self.columns.is_empty() || !self.view_offset.is_dnd_scroll())
        {
            return;
        }

        // Core insert/remove/reorder repairs focus before the adapter animates.
        // Rebase the relative camera to that new anchor, preserving the previous
        // absolute view position used by the animation's initial frame.
        if previous != idx && self.order.active_index() == idx {
            self.view_offset
                .offset(self.column_x(previous) - self.column_x(idx));
        }

        self.animate_view_offset_to_column_with_config(None, idx, Some(previous), config);

        if self.order.activate_from(idx, previous) {
            self.order.resize = None;
        }
    }

    pub(super) fn insert_position(&self, pos: Point<f64, Logical>) -> InsertPosition {
        if self.columns.is_empty() {
            return InsertPosition::NewColumn(0);
        }

        let x = pos.x + self.view_pos();

        // Aim for the center of the gap.
        let x = x + self.options.layout.gaps / 2.;
        let y = pos.y + self.options.layout.gaps / 2.;

        // Insert position is before the first column.
        if x < 0. {
            return InsertPosition::NewColumn(0);
        }

        // Find the closest gap between columns.
        let (closest_col_idx, col_x) = self
            .column_xs(self.data.iter().copied())
            .enumerate()
            .min_by_key(|(_, col_x)| NotNan::new((col_x - x).abs()).unwrap())
            .unwrap();

        // Find the column containing the position.
        let (col_idx, _) = self
            .column_xs(self.data.iter().copied())
            .enumerate()
            .take_while(|(_, col_x)| *col_x <= x)
            .last()
            .unwrap_or((0, 0.));

        // Insert position is past the last column.
        if col_idx == self.columns.len() {
            return InsertPosition::NewColumn(closest_col_idx);
        }

        // Find the closest gap between tiles.
        let col = &self.columns[col_idx];

        let (closest_tile_idx, tile_y) = if col.policy.display_mode == ColumnDisplay::Tabbed {
            // In tabbed mode, there's only one tile visible, and we want to check its top and
            // bottom.
            let top = col.tile_offsets().nth(col.policy.active_index()).unwrap().y;
            let bottom = top + col.data[col.policy.active_index()].size.h;
            if (top - y).abs() <= (bottom - y).abs() {
                (col.policy.active_index(), top)
            } else {
                (col.policy.active_index() + 1, bottom)
            }
        } else {
            col.tile_offsets()
                .map(|tile_off| tile_off.y)
                .enumerate()
                .min_by_key(|(_, tile_y)| NotNan::new((tile_y - y).abs()).unwrap())
                .unwrap()
        };

        // Return the closest among the vertical and the horizontal gap.
        let vert_dist = (col_x - x).abs();
        let hor_dist = (tile_y - y).abs();
        if vert_dist <= hor_dist {
            InsertPosition::NewColumn(closest_col_idx)
        } else {
            InsertPosition::InColumn(col_idx, closest_tile_idx)
        }
    }

    pub fn add_tile(
        &mut self,
        col_idx: Option<usize>,
        tile: Tile<W>,
        activate: bool,
        width: ColumnWidth,
        is_full_width: bool,
        anim: Option<clarion_config::Animation>,
    ) {
        let column = Column::new_with_tile(
            tile,
            self.view_size,
            self.working_area,
            self.parent_area,
            self.scale,
            width,
            is_full_width,
        );

        self.add_column(col_idx, column, activate, anim);
    }

    pub fn add_tile_to_column(
        &mut self,
        col_idx: usize,
        tile_idx: Option<usize>,
        tile: Tile<W>,
        activate: bool,
    ) {
        let prev_next_x = self.column_x(col_idx + 1);

        let target_column = &mut self.columns[col_idx];
        let tile_idx = tile_idx.unwrap_or(target_column.tiles.len());
        let mut prev_active_tile_idx = target_column.policy.active_index();

        target_column.add_tile_at(tile_idx, tile);
        self.data[col_idx].update(target_column);

        if tile_idx <= prev_active_tile_idx {
            prev_active_tile_idx += 1;
        }

        if activate {
            target_column.activate_idx(tile_idx);
            if self.order.active_index() != col_idx {
                self.activate_column(col_idx);
            }
        }

        let target_column = &mut self.columns[col_idx];
        if target_column.policy.display_mode == ColumnDisplay::Tabbed {
            if target_column.policy.active_index() == tile_idx {
                // Fade out the previously active tile.
                let tile = &mut target_column.tiles[prev_active_tile_idx];
                tile.animate_alpha(1., 0., self.options.animations.window_movement.0);
            } else {
                // Fade out when adding into a tabbed column into the background.
                let tile = &mut target_column.tiles[tile_idx];
                tile.animate_alpha(1., 0., self.options.animations.window_movement.0);
            }
        }

        // Adding a wider window into a column increases its width now (even if the window will
        // shrink later). Move the columns to account for this.
        let offset = self.column_x(col_idx + 1) - prev_next_x;
        if self.order.active_index() <= col_idx {
            for col in &mut self.columns[col_idx + 1..] {
                col.animate_move_x_from(-offset);
            }
        } else {
            for col in &mut self.columns[..=col_idx] {
                col.animate_move_x_from(offset);
            }
        }

        // In dwindle display mode the 2D geometry depends on the tile count, so recompute it
        // after adding the new tile (otherwise the new tile has no dwindle offset and the
        // existing tiles don't reflow).
        if self.columns[col_idx].policy.display_mode == ColumnDisplay::Dwindle {
            let column = &mut self.columns[col_idx];
            column.update_tile_sizes(true);
            self.data[col_idx].update(column);
        }
    }

    /// Number of tiles in the active column (0 if there are no columns).
    pub fn active_column_tile_count(&self) -> usize {
        self.columns
            .get(self.order.active_index())
            .map_or(0, |col| col.tiles.len())
    }

    /// Add a new tile into the active column for the auto-stack behavior.
    ///
    /// The column keeps its current width; if its display mode is Normal, it is switched to
    /// Dwindle so the stacked windows form a Hyprland-style 2D puzzle (with a smooth reflow
    /// animation). Tabbed columns stay tabbed, Dwindle columns are already correct.
    pub fn add_tile_to_active_column(&mut self, tile: Tile<W>, activate: bool) {
        if self.columns.is_empty() {
            let width = self
                .options
                .layout
                .default_column_width
                .map(column_width_from_preset)
                .unwrap_or(ColumnWidth::Proportion(0.5));
            self.add_tile(None, tile, activate, width, false, None);
            return;
        }

        let col_idx = self.order.active_index();
        self.add_tile_to_column(col_idx, None, tile, activate);

        if self.columns[col_idx].policy.display_mode == ColumnDisplay::Normal {
            self.set_column_display(IpcColumnDisplay::Dwindle);
        }
    }

    pub fn add_tile_right_of(
        &mut self,
        right_of: &W::Id,
        tile: Tile<W>,
        activate: bool,
        width: ColumnWidth,
        is_full_width: bool,
        anim: Option<clarion_config::Animation>,
    ) {
        let Some(right_of_idx) = self.columns.iter().position(|col| col.contains(right_of)) else {
            self.add_tile(None, tile, activate, width, is_full_width, anim);
            return;
        };
        let col_idx = right_of_idx + 1;

        self.add_tile(Some(col_idx), tile, activate, width, is_full_width, anim);
    }

    pub fn add_column(
        &mut self,
        idx: Option<usize>,
        mut column: Column<W>,
        activate: bool,
        anim_config: Option<clarion_config::Animation>,
    ) {
        let was_empty = self.columns.is_empty();

        let idx = self.order.insertion_index(idx);

        column.update_config(
            self.view_size,
            self.working_area,
            self.parent_area,
            self.scale,
            self.options.clone(),
        );
        self.data.insert(idx, ColumnData::new(&column));
        assert!(self.order.insert(idx, column.policy.id));
        self.columns.insert(idx, column);

        // Compute the initial 2D geometry for dwindle-display columns (the window's opening
        // size doesn't match the dwindle area otherwise).
        if self.columns[idx].policy.display_mode == ColumnDisplay::Dwindle {
            let column = &mut self.columns[idx];
            column.update_tile_sizes(false);
            self.data[idx].update(column);
        }

        // Animate movement of other columns.
        let offset = self.column_x(idx + 1) - self.column_x(idx);
        let config = anim_config.unwrap_or(self.options.animations.window_movement.0);
        if self.order.active_index() <= idx {
            for col in &mut self.columns[idx + 1..] {
                col.animate_move_x_from_with_config(-offset, config);
            }
        } else {
            for col in &mut self.columns[..idx] {
                col.animate_move_x_from_with_config(offset, config);
            }
        }

        if activate {
            // If this is the first window on an empty workspace, remove the effect of whatever
            // view_offset was left over and skip the animation.
            if was_empty {
                self.view_offset = ViewOffset::new(self.clock.clone(), 0.);
                self.view_offset = ViewOffset::new(
                    self.clock.clone(),
                    self.compute_new_view_offset_for_column(None, idx, None),
                );
            }

            let prev_offset = (!was_empty && idx == self.order.active_index() + 1)
                .then(|| self.view_offset.stationary());

            // This camera movement belongs to the same layout change as the
            // columns above. A faster focus-navigation spring pulls the view
            // ahead of the windows when an opening window creates a new column.
            self.activate_column_with_anim_config(idx, config);
            self.order.activate_previous_on_removal = prev_offset;
        }
    }

    pub fn remove_tile(&mut self, window: &W::Id, transaction: Transaction) -> RemovedTile<W> {
        let column_idx = self
            .columns
            .iter()
            .position(|col| col.contains(window))
            .unwrap();
        let column = &self.columns[column_idx];

        let tile_idx = column.position(window).unwrap();
        self.remove_tile_by_idx(column_idx, tile_idx, transaction, None)
    }

    pub fn remove_tile_by_idx(
        &mut self,
        column_idx: usize,
        tile_idx: usize,
        transaction: Transaction,
        anim_config: Option<clarion_config::Animation>,
    ) -> RemovedTile<W> {
        // If this is the only tile in the column, remove the whole column.
        if self.columns[column_idx].tiles.len() == 1 {
            let mut column = self.remove_column_by_idx(column_idx, anim_config);
            column.policy.remove(tile_idx);
            return RemovedTile {
                tile: column.tiles.remove(tile_idx),
                width: column.policy.width,
                is_full_width: column.policy.is_full_width,
                is_floating: false,
            };
        }

        let column = &mut self.columns[column_idx];
        let prev_width = self.data[column_idx].width;

        let movement_config = anim_config.unwrap_or(self.options.animations.window_movement.0);

        // Animate movement of other tiles.
        // FIXME: tiles can move by X too, in a centered or resizing layout with one window smaller
        // than the others.
        //
        // In dwindle display mode the tiles are laid out in 2D, so the vertical offset between
        // consecutive tiles doesn't describe the reflow motion; the position animation is handled
        // by update_tile_sizes_with_transaction() instead.
        if column.policy.display_mode != ColumnDisplay::Dwindle {
            let offset_y = column.tile_offset(tile_idx + 1).y - column.tile_offset(tile_idx).y;
            for tile in &mut column.tiles[tile_idx + 1..] {
                tile.animate_move_y_from(offset_y);
            }
        }

        if column.policy.display_mode == ColumnDisplay::Tabbed
            && tile_idx != column.policy.active_index()
        {
            // Fade in when removing background tab from a tabbed column.
            let tile = &mut column.tiles[tile_idx];
            tile.animate_alpha(0., 1., movement_config);
        }

        let was_normal = column.sizing_mode().is_normal();

        let (_, removal) = column.policy.remove(tile_idx);
        let tile = column.tiles.remove(tile_idx);
        column.data.remove(tile_idx);
        if column.dwindle_offsets.len() > tile_idx {
            Rc::make_mut(&mut column.dwindle_offsets).remove(tile_idx);
        }

        // If an active column became non-fullscreen after removing the tile, clear the stored
        // unfullscreen offset.
        if column_idx == self.order.active_index()
            && !was_normal
            && column.sizing_mode().is_normal()
        {
            self.order.view_offset_to_restore = None;
        }

        // Stop interactive resize.
        if let Some(resize) = &self.order.resize {
            if tile.window().core_id() == resize.id {
                self.order.resize = None;
            }
        }

        let tile = RemovedTile {
            tile,
            width: column.policy.width,
            is_full_width: column.policy.is_full_width,
            is_floating: false,
        };

        if removal.removed_active {
            column.tiles[removal.new_active].ensure_alpha_animates_to_1();
        }

        column.update_tile_sizes_with_transaction(true, true, transaction);
        self.data[column_idx].update(column);
        let offset = prev_width - column.width();

        // Animate movement of the other columns.
        if self.order.active_index() <= column_idx {
            for col in &mut self.columns[column_idx + 1..] {
                col.animate_move_x_from_with_config(offset, movement_config);
            }
        } else {
            for col in &mut self.columns[..=column_idx] {
                col.animate_move_x_from_with_config(-offset, movement_config);
            }
        }

        tile
    }

    pub fn active_column(&self) -> Option<&Column<W>> {
        if self.columns.is_empty() {
            return None;
        }

        Some(&self.columns[self.order.active_index()])
    }

    pub fn remove_active_column(&mut self) -> Option<Column<W>> {
        if self.columns.is_empty() {
            return None;
        }

        Some(self.remove_column_by_idx(self.order.active_index(), None))
    }

    pub fn remove_column_by_idx(
        &mut self,
        column_idx: usize,
        anim_config: Option<clarion_config::Animation>,
    ) -> Column<W> {
        // Animate movement of the other columns.
        let movement_config = anim_config.unwrap_or(self.options.animations.window_movement.0);
        let offset = self.column_x(column_idx + 1) - self.column_x(column_idx);
        if self.order.active_index() <= column_idx {
            for col in &mut self.columns[column_idx + 1..] {
                col.animate_move_x_from_with_config(offset, movement_config);
            }
        } else {
            for col in &mut self.columns[..column_idx] {
                col.animate_move_x_from_with_config(-offset, movement_config);
            }
        }

        let previous = self.order.active_index();
        let restore_previous = self.order.activate_previous_on_removal;
        let column = self.columns.remove(column_idx);
        let removal = self.order.remove(column_idx);
        self.data.remove(column_idx);

        // Stop interactive resize.
        if let Some(resize) = &self.order.resize {
            if column
                .tiles
                .iter()
                .any(|tile| tile.window().core_id() == resize.id)
            {
                self.order.resize = None;
            }
        }

        if self.columns.is_empty() {
            return column;
        }

        let view_config = anim_config.unwrap_or(self.options.animations.horizontal_view_movement.0);

        if column_idx >= previous {
            self.activate_column_from_with_anim_config(removal.new_active, view_config, previous);
            if removal.removed_active && column_idx > 0 {
                if let Some(prev_offset) = restore_previous {
                    self.animate_view_offset_with_config(
                        removal.new_active,
                        prev_offset,
                        view_config,
                    );
                    self.animate_view_offset_to_column_with_config(
                        None,
                        removal.new_active,
                        None,
                        view_config,
                    );
                }
            }
        }

        column
    }

    pub fn update_window(&mut self, window: &W::Id, serial: Option<Serial>) {
        let (col_idx, column) = self
            .columns
            .iter_mut()
            .enumerate()
            .find(|(_, col)| col.contains(window))
            .unwrap();
        let was_normal = column.sizing_mode().is_normal();
        let prev_origin = column.tiles_origin();

        let (tile_idx, tile) = column
            .tiles
            .iter_mut()
            .enumerate()
            .find(|(_, tile)| tile.window().id() == window)
            .unwrap();

        let resize = tile.window_mut().interactive_resize_data();

        // Do this before calling update_window() so it can get up-to-date info.
        if let Some(serial) = serial {
            tile.window_mut().on_commit(serial);
        }

        let prev_width = self.data[col_idx].width;

        column.update_window(window);
        self.data[col_idx].update(column);
        column.update_tile_sizes(false);

        let offset = prev_width - self.data[col_idx].width;

        // Move other columns in tandem with resizing.
        let ongoing_resize_anim = column.tiles[tile_idx].resize_animation().is_some();
        if offset != 0. {
            if self.order.active_index() <= col_idx {
                for col in &mut self.columns[col_idx + 1..] {
                    // If there's a resize animation on the tile (that may have just started in
                    // column.update_window()), then the apparent size change is smooth with no
                    // sudden jumps. This corresponds to adding an X animation to adjacent columns.
                    //
                    // There could also be no resize animation with nonzero offset. This could
                    // happen for example:
                    // - if the window resized on its own, which we don't animate
                    // - if the window resized by less than 10 px (the resize threshold)
                    //
                    // The latter case could also cancel an ongoing resize animation.
                    //
                    // Now, stationary columns shouldn't react to this offset change in any way,
                    // i.e. their apparent X position should jump together with the resize.
                    // However, adjacent columns that are already animating an X movement should
                    // offset their animations to avoid the jump.
                    //
                    // Notably, this is necessary to fix the animation jump when resizing width back
                    // and forth in quick succession (in a way that cancels the resize animation).
                    if ongoing_resize_anim {
                        col.animate_move_x_from_with_config(
                            offset,
                            self.options.animations.window_resize.anim,
                        );
                    } else {
                        col.offset_move_anim_current(offset);
                    }
                }
            } else {
                for col in &mut self.columns[..=col_idx] {
                    if ongoing_resize_anim {
                        col.animate_move_x_from_with_config(
                            -offset,
                            self.options.animations.window_resize.anim,
                        );
                    } else {
                        col.offset_move_anim_current(-offset);
                    }
                }
            }
        }

        // When a column goes between fullscreen and non-fullscreen, the tiles origin can change.
        // The change comes from things like ignoring struts and hiding the tab indicator in
        // fullscreen, so both in X and Y directions.
        let column = &mut self.columns[col_idx];
        let new_origin = column.tiles_origin();
        let origin_delta = prev_origin - new_origin;
        if origin_delta != Point::new(0., 0.) {
            for (tile, _pos) in column.tiles_mut() {
                tile.animate_move_from(origin_delta);
            }
        }

        if col_idx == self.order.active_index() {
            // If offset == 0, then don't mess with the view or the gesture. Some clients (Firefox,
            // Chromium, Electron) currently don't commit after the ack of a configure that drops
            // the Resizing state, which can trigger this code path for a while.
            let resize = if offset != 0. { resize } else { None };
            if let Some(resize) = resize {
                // Don't bother with the gesture.
                self.view_offset.cancel_gesture();

                // If this is an interactive resize commit of an active window, then we need to
                // either preserve the view offset or adjust it accordingly.
                let centered = self.is_centering_focused_column();

                let width = self.data[col_idx].width;
                let offset = if centered {
                    // FIXME: when view_offset becomes fractional, this can be made additive too.
                    let new_offset =
                        -(self.working_area.size.w - width) / 2. - self.working_area.loc.x;
                    new_offset - self.view_offset.target()
                } else if resize.edges.contains(ResizeEdge::LEFT) {
                    -offset
                } else {
                    0.
                };

                self.view_offset.offset(offset);
            }

            // When the active column goes fullscreen, store the view offset to restore later.
            let is_normal = self.columns[col_idx].sizing_mode().is_normal();
            if was_normal && !is_normal {
                self.order.view_offset_to_restore = Some(self.view_offset.stationary());
            }

            // Upon unfullscreening, restore the view offset.
            //
            // In tabbed display mode, there can be multiple tiles in a fullscreen column. They
            // will unfullscreen one by one, and the column width will shrink only when the
            // last tile unfullscreens. This is when we want to restore the view offset,
            // otherwise it will immediately reset back by the animate_view_offset below.
            let unfullscreen_offset = if !was_normal && is_normal {
                // Take the value unconditionally, even if the view is currently frozen by
                // a view gesture. It shouldn't linger around because it's only valid for this
                // particular unfullscreen.
                self.order.view_offset_to_restore.take()
            } else {
                None
            };

            // We might need to move the view to ensure the resized window is still visible. But
            // only do it when the view isn't frozen by an interactive resize or a view gesture.
            if self.order.resize.is_none() && !self.view_offset.is_gesture() {
                // Synchronize the horizontal view movement with the resize so that it looks nice.
                // This is especially important for always-centered view.
                let config = if ongoing_resize_anim {
                    self.options.animations.window_resize.anim
                } else {
                    self.options.animations.horizontal_view_movement.0
                };

                // Restore the view offset upon unfullscreening if needed.
                if let Some(prev_offset) = unfullscreen_offset {
                    self.animate_view_offset_with_config(col_idx, prev_offset, config);
                }

                // FIXME: we will want to skip the animation in some cases here to make continuously
                // resizing windows not look janky.
                self.animate_view_offset_to_column_with_config(None, col_idx, None, config);
            }
        }
    }

    pub fn scroll_amount_to_activate(&self, window: &W::Id) -> f64 {
        let column_idx = self
            .columns
            .iter()
            .position(|col| col.contains(window))
            .unwrap();

        if self.order.active_index() == column_idx {
            return 0.;
        }

        // Consider the end of an ongoing animation because that's what compute to fit does too.
        let target_x = self.target_view_pos();
        let new_view_offset = self.compute_new_view_offset_for_column(
            Some(target_x),
            column_idx,
            Some(self.order.active_index()),
        );

        let new_col_x = self.column_x(column_idx);
        let from_view_offset = target_x - new_col_x;

        (from_view_offset - new_view_offset).abs() / self.working_area.size.w
    }

    pub fn activate_window(&mut self, window: &W::Id) -> bool {
        let column_idx = self.columns.iter().position(|col| col.contains(window));
        let Some(column_idx) = column_idx else {
            return false;
        };
        let column = &mut self.columns[column_idx];

        column.activate_window(window);
        self.activate_column(column_idx);

        true
    }

    pub fn start_close_animation_for_window(
        &mut self,
        renderer: &mut AnyRenderer<'_>,
        window: &W::Id,
        blocker: TransactionBlocker,
    ) {
        let closing_res = {
            let Some((tile, mut tile_pos, _)) = self
                .tiles_with_render_positions()
                .find(|(tile, _, _)| tile.window().id() == window)
            else {
                return;
            };

            let Some((col_idx, tile_idx)) =
                self.columns.iter().enumerate().find_map(|(col_idx, col)| {
                    col.tiles
                        .iter()
                        .position(|tile| tile.window().id() == window)
                        .map(move |tile_idx| (col_idx, tile_idx))
                })
            else {
                return;
            };

            let col = &self.columns[col_idx];
            let removing_last = col.tiles.len() == 1;

            // Skip closing animation for invisible tiles in a tabbed column.
            if col.policy.display_mode == ColumnDisplay::Tabbed
                && tile_idx != col.policy.active_index()
            {
                return;
            }

            tile_pos.x += self.view_pos();

            if col_idx < self.order.active_index() {
                let offset = if removing_last {
                    self.column_x(col_idx + 1) - self.column_x(col_idx)
                } else {
                    self.data[col_idx].width
                        - col
                            .data
                            .iter()
                            .enumerate()
                            .filter_map(|(idx, data)| {
                                (idx != tile_idx).then_some(NotNan::new(data.size.w).unwrap())
                            })
                            .max()
                            .map(NotNan::into_inner)
                            .unwrap()
                };
                tile_pos.x -= offset;
            }

            let anim = Animation::new(
                self.clock.clone(),
                0.,
                1.,
                0.,
                self.options.animations.window_close.anim,
            );

            let blocker = if self.options.disable_transactions {
                TransactionBlocker::completed()
            } else {
                blocker
            };

            let scale = Scale::from(self.scale);
            tile.create_closing_window(
                renderer,
                scale,
                tile_pos,
                blocker,
                anim,
                &self.options.animations.window_close,
                self.scene_resources.closing.clone(),
            )
        };

        match closing_res {
            Ok(closing) => {
                self.closing_windows.push(closing);
            }
            Err(err) => {
                warn!("error creating a closing window animation: {err:?}");
            }
        }
    }

    pub fn start_close_animation_for_window_at(
        &mut self,
        renderer: &mut AnyRenderer<'_>,
        window: &W::Id,
        tile_pos: Point<f64, Logical>,
        blocker: TransactionBlocker,
    ) {
        let closing_res = {
            let Some((tile, _, _)) = self
                .tiles_with_render_positions()
                .find(|(tile, _, _)| tile.window().id() == window)
            else {
                return;
            };

            let anim = Animation::new(
                self.clock.clone(),
                0.,
                1.,
                0.,
                self.options.animations.window_close.anim,
            );

            let blocker = if self.options.disable_transactions {
                TransactionBlocker::completed()
            } else {
                blocker
            };

            let scale = Scale::from(self.scale);
            tile.create_closing_window(
                renderer,
                scale,
                tile_pos,
                blocker,
                anim,
                &self.options.animations.window_close,
                self.scene_resources.closing.clone(),
            )
        };

        match closing_res {
            Ok(closing) => {
                self.closing_windows.push(closing);
            }
            Err(err) => {
                warn!("error creating a closing window animation: {err:?}");
            }
        }
    }

    pub fn start_open_animation(&mut self, id: &W::Id) -> bool {
        self.columns
            .iter_mut()
            .any(|col| col.start_open_animation(id))
    }

    /// The close animation ignores background tabs, so capturing them would only consume GPU
    /// time and memory. Return `None` for windows that are not in this scrolling space.
    pub fn should_capture_close_snapshot(&self, window: &W::Id) -> Option<bool> {
        self.columns.iter().find_map(|col| {
            col.tiles
                .iter()
                .position(|tile| tile.window().id() == window)
                .map(|idx| {
                    col.policy.display_mode != ColumnDisplay::Tabbed
                        || idx == col.policy.active_index()
                })
        })
    }

    pub fn focus_left(&mut self) -> bool {
        if let Some(col) = self.columns.get_mut(self.order.active_index()) {
            if col.policy.display_mode == ColumnDisplay::Dwindle
                && col.focus_dwindle_horizontal(true)
            {
                return true;
            }
        }

        let Some(new_idx) = self.order.adjacent(false) else {
            return false;
        };
        self.activate_column(new_idx);
        if let Some(col) = self.columns.get_mut(new_idx) {
            if col.policy.display_mode == ColumnDisplay::Dwindle {
                col.activate_rightmost_dwindle_tile();
            }
        }
        true
    }

    pub fn focus_right(&mut self) -> bool {
        if let Some(col) = self.columns.get_mut(self.order.active_index()) {
            if col.policy.display_mode == ColumnDisplay::Dwindle
                && col.focus_dwindle_horizontal(false)
            {
                return true;
            }
        }

        let Some(new_idx) = self.order.adjacent(true) else {
            return false;
        };
        self.activate_column(new_idx);
        if let Some(col) = self.columns.get_mut(new_idx) {
            if col.policy.display_mode == ColumnDisplay::Dwindle {
                col.activate_leftmost_dwindle_tile();
            }
        }
        true
    }

    pub fn focus_column_first(&mut self) {
        self.activate_column(0);
    }

    pub fn focus_column_last(&mut self) {
        if self.columns.is_empty() {
            return;
        }

        self.activate_column(self.columns.len() - 1);
    }

    pub fn focus_column(&mut self, index: usize) {
        if self.columns.is_empty() {
            return;
        }

        if let Some(index) = self.order.one_based_index(index) {
            self.activate_column(index);
        }
    }

    pub fn focus_window_in_column(&mut self, index: u8) {
        if self.columns.is_empty() {
            return;
        }

        self.columns[self.order.active_index()].focus_index(index);
    }

    pub fn focus_down(&mut self) -> bool {
        if self.columns.is_empty() {
            return false;
        }

        self.columns[self.order.active_index()].focus_down()
    }

    pub fn focus_up(&mut self) -> bool {
        if self.columns.is_empty() {
            return false;
        }

        self.columns[self.order.active_index()].focus_up()
    }

    pub fn focus_down_or_left(&mut self) {
        if self.columns.is_empty() {
            return;
        }

        let column = &mut self.columns[self.order.active_index()];
        if !column.focus_down() {
            self.focus_left();
        }
    }

    pub fn focus_down_or_right(&mut self) {
        if self.columns.is_empty() {
            return;
        }

        let column = &mut self.columns[self.order.active_index()];
        if !column.focus_down() {
            self.focus_right();
        }
    }

    pub fn focus_up_or_left(&mut self) {
        if self.columns.is_empty() {
            return;
        }

        let column = &mut self.columns[self.order.active_index()];
        if !column.focus_up() {
            self.focus_left();
        }
    }

    pub fn focus_up_or_right(&mut self) {
        if self.columns.is_empty() {
            return;
        }

        let column = &mut self.columns[self.order.active_index()];
        if !column.focus_up() {
            self.focus_right();
        }
    }

    pub fn focus_top(&mut self) {
        if self.columns.is_empty() {
            return;
        }

        self.columns[self.order.active_index()].focus_top()
    }

    pub fn focus_bottom(&mut self) {
        if self.columns.is_empty() {
            return;
        }

        self.columns[self.order.active_index()].focus_bottom()
    }

    pub fn move_column_to_index(&mut self, index: usize) {
        if self.columns.is_empty() {
            return;
        }

        self.move_column_to(index.saturating_sub(1).min(self.columns.len() - 1));
    }

    fn move_column_to(&mut self, new_idx: usize) {
        if self.order.active_index() == new_idx {
            return;
        }

        let previous = self.order.active_index();
        let current_col_x = self.column_x(previous);
        let next_col_x = self.column_x(previous + 1);

        let mut column = self.columns.remove(previous);
        let data = self.data.remove(previous);
        cancel_resize_for_column(&mut self.order.resize, &mut column);
        self.columns.insert(new_idx, column);
        self.data.insert(new_idx, data);

        // Preserve the camera position when moving to the left.
        let view_offset_delta = -self.column_x(previous) + current_col_x;
        self.view_offset.offset(view_offset_delta);

        // The column we just moved is offset by the difference between its new and old position.
        let new_col_x = self.column_x(new_idx);
        self.columns[new_idx].animate_move_x_from(current_col_x - new_col_x);

        // All columns in between moved by the width of the column that we just moved.
        let others_x_offset = next_col_x - current_col_x;
        if previous < new_idx {
            for col in &mut self.columns[previous..new_idx] {
                col.animate_move_x_from(others_x_offset);
            }
        } else {
            for col in &mut self.columns[new_idx + 1..=previous] {
                col.animate_move_x_from(-others_x_offset);
            }
        }

        self.order.move_active_to(new_idx);
        self.activate_column_from_with_anim_config(
            new_idx,
            self.options.animations.window_movement.0,
            previous,
        );
    }

    pub fn move_left(&mut self) -> bool {
        if self.order.active_index() == 0 {
            return false;
        }

        self.move_column_to(self.order.active_index() - 1);
        true
    }

    pub fn move_right(&mut self) -> bool {
        let new_idx = self.order.active_index() + 1;
        if new_idx >= self.columns.len() {
            return false;
        }

        self.move_column_to(new_idx);
        true
    }

    pub fn move_column_to_first(&mut self) {
        self.move_column_to(0);
    }

    pub fn move_column_to_last(&mut self) {
        if self.columns.is_empty() {
            return;
        }

        let new_idx = self.columns.len() - 1;
        self.move_column_to(new_idx);
    }

    pub fn move_down(&mut self) -> bool {
        if self.columns.is_empty() {
            return false;
        }

        self.columns[self.order.active_index()].move_down()
    }

    pub fn move_up(&mut self) -> bool {
        if self.columns.is_empty() {
            return false;
        }

        self.columns[self.order.active_index()].move_up()
    }

    pub fn consume_or_expel_window_left(&mut self, window: Option<&W::Id>) {
        if self.columns.is_empty() {
            return;
        }

        let (source_col_idx, source_tile_idx) = if let Some(window) = window {
            self.columns
                .iter_mut()
                .enumerate()
                .find_map(|(col_idx, col)| {
                    col.tiles
                        .iter()
                        .position(|tile| tile.window().id() == window)
                        .map(|tile_idx| (col_idx, tile_idx))
                })
                .unwrap()
        } else {
            let source_col_idx = self.order.active_index();
            let source_tile_idx = self.columns[self.order.active_index()]
                .policy
                .active_index();
            (source_col_idx, source_tile_idx)
        };

        let source_column = &self.columns[source_col_idx];
        let prev_off = source_column.tile_offset(source_tile_idx);

        let source_tile_was_active = self.order.active_index() == source_col_idx
            && source_column.policy.active_index() == source_tile_idx;

        if source_column.tiles.len() == 1 {
            if source_col_idx == 0 {
                return;
            }

            // Move into adjacent column.
            let target_column_idx = source_col_idx - 1;

            let offset = if self.order.active_index() <= source_col_idx {
                // Tiles to the right animate from the following column.
                self.column_x(source_col_idx) - self.column_x(target_column_idx)
            } else {
                // Tiles to the left animate to preserve their right edge position.
                f64::max(
                    0.,
                    self.data[target_column_idx].width - self.data[source_col_idx].width,
                )
            };
            let mut offset = Point::from((offset, 0.));

            if source_tile_was_active {
                // Make sure the previous (target) column is activated so the animation looks right.
                //
                // However, if it was already going to be activated, leave the offset as is. This
                // improves the workflow that has become common with tabbed columns: open a new
                // window, then immediately consume it left as a new tab.
                self.order
                    .activate_previous_on_removal
                    .get_or_insert(self.view_offset.stationary() + offset.x);
            }

            offset += self.columns[source_col_idx].render_offset();
            let RemovedTile { tile, .. } = self.remove_tile_by_idx(
                source_col_idx,
                0,
                Transaction::new(),
                Some(self.options.animations.window_movement.0),
            );
            self.add_tile_to_column(target_column_idx, None, tile, source_tile_was_active);

            let target_column = &mut self.columns[target_column_idx];
            offset -= target_column.render_offset();
            offset += prev_off - target_column.tile_offset(target_column.tiles.len() - 1);

            let new_tile = target_column.tiles.last_mut().unwrap();
            new_tile.animate_move_from(offset);
        } else {
            // Move out of column.
            let mut offset = source_column.render_offset();

            let removed =
                self.remove_tile_by_idx(source_col_idx, source_tile_idx, Transaction::new(), None);

            // We're inserting into the source column position.
            let target_column_idx = source_col_idx;

            self.add_tile(
                Some(target_column_idx),
                removed.tile,
                source_tile_was_active,
                removed.width,
                removed.is_full_width,
                Some(self.options.animations.window_movement.0),
            );

            if source_tile_was_active {
                // We added to the left, don't activate even further left on removal.
                self.order.activate_previous_on_removal = None;
            }

            if target_column_idx <= self.order.active_index() {
                // Tiles to the left animate from the following column.
                offset.x += self.column_x(target_column_idx + 1) - self.column_x(target_column_idx);
            }

            let new_col = &mut self.columns[target_column_idx];
            offset += prev_off - new_col.tile_offset(0);
            new_col.tiles[0].animate_move_from(offset);
        }
    }

    pub fn consume_or_expel_window_right(&mut self, window: Option<&W::Id>) {
        if self.columns.is_empty() {
            return;
        }

        let (source_col_idx, source_tile_idx) = if let Some(window) = window {
            self.columns
                .iter_mut()
                .enumerate()
                .find_map(|(col_idx, col)| {
                    col.tiles
                        .iter()
                        .position(|tile| tile.window().id() == window)
                        .map(|tile_idx| (col_idx, tile_idx))
                })
                .unwrap()
        } else {
            let source_col_idx = self.order.active_index();
            let source_tile_idx = self.columns[self.order.active_index()]
                .policy
                .active_index();
            (source_col_idx, source_tile_idx)
        };

        let cur_x = self.column_x(source_col_idx);

        let source_column = &self.columns[source_col_idx];
        let mut offset = source_column.render_offset();
        let prev_off = source_column.tile_offset(source_tile_idx);

        let source_tile_was_active = self.order.active_index() == source_col_idx
            && source_column.policy.active_index() == source_tile_idx;

        if source_column.tiles.len() == 1 {
            if source_col_idx + 1 == self.columns.len() {
                return;
            }

            // Move into adjacent column.
            let target_column_idx = source_col_idx;

            offset.x += cur_x - self.column_x(source_col_idx + 1);
            offset -= self.columns[source_col_idx + 1].render_offset();

            if source_tile_was_active {
                // Make sure the target column gets activated.
                self.order.activate_previous_on_removal = None;
            }

            let RemovedTile { tile, .. } = self.remove_tile_by_idx(
                source_col_idx,
                0,
                Transaction::new(),
                Some(self.options.animations.window_movement.0),
            );
            self.add_tile_to_column(target_column_idx, None, tile, source_tile_was_active);

            let target_column = &mut self.columns[target_column_idx];
            offset += prev_off - target_column.tile_offset(target_column.tiles.len() - 1);

            let new_tile = target_column.tiles.last_mut().unwrap();
            new_tile.animate_move_from(offset);
        } else {
            // Move out of column.
            let prev_width = self.data[source_col_idx].width;

            let removed =
                self.remove_tile_by_idx(source_col_idx, source_tile_idx, Transaction::new(), None);

            let target_column_idx = source_col_idx + 1;

            self.add_tile(
                Some(target_column_idx),
                removed.tile,
                source_tile_was_active,
                removed.width,
                removed.is_full_width,
                Some(self.options.animations.window_movement.0),
            );

            offset.x += if self.order.active_index() <= target_column_idx {
                // Tiles to the right animate to the following column.
                cur_x - self.column_x(target_column_idx)
            } else {
                // Tiles to the left animate for a change in width.
                -f64::max(0., prev_width - self.data[target_column_idx].width)
            };

            let new_col = &mut self.columns[target_column_idx];
            offset += prev_off - new_col.tile_offset(0);
            new_col.tiles[0].animate_move_from(offset);
        }
    }

    pub fn consume_into_column(&mut self) {
        if self.columns.len() < 2 {
            return;
        }

        if self.order.active_index() == self.columns.len() - 1 {
            return;
        }

        let target_column_idx = self.order.active_index();
        let source_column_idx = self.order.active_index() + 1;

        let mut offset = self.columns[source_column_idx].render_offset();
        offset.x += self.column_x(source_column_idx);
        offset.x -= self.column_x(target_column_idx);
        let prev_off = self.columns[source_column_idx].tile_offset(0);

        let removed = self.remove_tile_by_idx(source_column_idx, 0, Transaction::new(), None);
        self.add_tile_to_column(target_column_idx, None, removed.tile, false);

        let target_column = &mut self.columns[target_column_idx];
        offset += prev_off - target_column.tile_offset(target_column.tiles.len() - 1);
        offset -= target_column.render_offset();

        let new_tile = target_column.tiles.last_mut().unwrap();
        new_tile.animate_move_from(offset);
    }

    pub fn expel_from_column(&mut self) {
        if self.columns.is_empty() {
            return;
        }

        let source_col_idx = self.order.active_index();
        let target_col_idx = self.order.active_index() + 1;
        let cur_x = self.column_x(source_col_idx);

        let source_column = &self.columns[self.order.active_index()];
        if source_column.tiles.len() == 1 {
            return;
        }

        let source_tile_idx = source_column.tiles.len() - 1;

        let mut offset = source_column.render_offset();
        let prev_off = source_column.tile_offset(source_tile_idx);

        let removed =
            self.remove_tile_by_idx(source_col_idx, source_tile_idx, Transaction::new(), None);

        self.add_tile(
            Some(target_col_idx),
            removed.tile,
            false,
            removed.width,
            removed.is_full_width,
            Some(self.options.animations.window_movement.0),
        );

        offset.x += cur_x - self.column_x(target_col_idx);

        let new_col = &mut self.columns[target_col_idx];
        offset += prev_off - new_col.tile_offset(0);
        new_col.tiles[0].animate_move_from(offset);
    }

    pub fn swap_window_in_direction(&mut self, direction: ScrollDirection) {
        if self.columns.is_empty() {
            return;
        }

        // if this is the first (resp. last column), then this operation is equivalent
        // to an `consume_or_expel_window_left` (resp. `consume_or_expel_window_right`)
        match direction {
            ScrollDirection::Left => {
                if self.order.active_index() == 0 {
                    return;
                }
            }
            ScrollDirection::Right => {
                if self.order.active_index() == self.columns.len() - 1 {
                    return;
                }
            }
        }

        let source_column_idx = self.order.active_index();
        let target_column_idx = self
            .order
            .active_index()
            .wrapping_add_signed(match direction {
                ScrollDirection::Left => -1,
                ScrollDirection::Right => 1,
            });

        // if both source and target columns contain a single tile, then the operation is equivalent
        // to a simple column move
        if self.columns[source_column_idx].tiles.len() == 1
            && self.columns[target_column_idx].tiles.len() == 1
        {
            return self.move_column_to(target_column_idx);
        }

        let source_tile_idx = self.columns[source_column_idx].policy.active_index();
        let target_tile_idx = self.columns[target_column_idx].policy.active_index();
        let source_column_drained = self.columns[source_column_idx].tiles.len() == 1;

        // capture the original positions of the tiles
        let (mut source_pt, mut target_pt) = (
            self.columns[source_column_idx].render_offset()
                + self.columns[source_column_idx].tile_offset(source_tile_idx),
            self.columns[target_column_idx].render_offset()
                + self.columns[target_column_idx].tile_offset(target_tile_idx),
        );
        source_pt.x += self.column_x(source_column_idx);
        target_pt.x += self.column_x(target_column_idx);

        let transaction = Transaction::new();

        // If the source column contains a single tile, this will also remove the column.
        // When this happens `source_column_drained` will be set and the column will need to be
        // recreated with `add_tile`
        let source_removed = self.remove_tile_by_idx(
            source_column_idx,
            source_tile_idx,
            transaction.clone(),
            None,
        );

        {
            // special case when the source column disappears after removing its last tile
            let adjusted_target_column_idx =
                if direction == ScrollDirection::Right && source_column_drained {
                    target_column_idx - 1
                } else {
                    target_column_idx
                };

            self.add_tile_to_column(
                adjusted_target_column_idx,
                Some(target_tile_idx),
                source_removed.tile,
                false,
            );

            let RemovedTile {
                tile: target_tile, ..
            } = self.remove_tile_by_idx(
                adjusted_target_column_idx,
                target_tile_idx + 1,
                transaction.clone(),
                None,
            );

            if source_column_drained {
                // recreate the drained column with only the target tile
                self.add_tile(
                    Some(source_column_idx),
                    target_tile,
                    true,
                    source_removed.width,
                    source_removed.is_full_width,
                    None,
                )
            } else {
                // simply add the removed target tile to the source column
                self.add_tile_to_column(
                    source_column_idx,
                    Some(source_tile_idx),
                    target_tile,
                    false,
                );
            }
        }

        // update the active tile in the modified columns
        self.columns[source_column_idx]
            .policy
            .activate(source_tile_idx);
        self.columns[target_column_idx]
            .policy
            .activate(target_tile_idx);

        // Animations
        self.columns[target_column_idx].tiles[target_tile_idx]
            .animate_move_from(source_pt - target_pt);
        self.columns[target_column_idx].tiles[target_tile_idx].ensure_alpha_animates_to_1();

        // FIXME: this stop_move_animations() causes the target tile animation to "reset" when
        // swapping. It's here as a workaround to stop the unwanted animation of moving the source
        // tile down when adding the target tile above it. This code needs to be written in some
        // other way not to trigger that animation, or to cancel it properly, so that swap doesn't
        // cancel all ongoing target tile animations.
        self.columns[source_column_idx].tiles[source_tile_idx].stop_move_animations();
        self.columns[source_column_idx].tiles[source_tile_idx]
            .animate_move_from(target_pt - source_pt);
        self.columns[source_column_idx].tiles[source_tile_idx].ensure_alpha_animates_to_1();

        self.activate_column(target_column_idx);
    }

    pub fn toggle_column_tabbed_display(&mut self) {
        if self.columns.is_empty() {
            return;
        }

        let col = &mut self.columns[self.order.active_index()];
        let display = match col.policy.display_mode {
            ColumnDisplay::Normal => ColumnDisplay::Tabbed,
            ColumnDisplay::Tabbed => ColumnDisplay::Normal,
            ColumnDisplay::Dwindle => ColumnDisplay::Normal,
        };

        self.set_column_display(column_display_to_ipc(display));
    }

    pub fn toggle_column_dwindle_display(&mut self) {
        if self.columns.is_empty() {
            return;
        }

        let col = &mut self.columns[self.order.active_index()];
        let display = match col.policy.display_mode {
            ColumnDisplay::Dwindle => ColumnDisplay::Normal,
            _ => ColumnDisplay::Dwindle,
        };

        self.set_column_display(column_display_to_ipc(display));
    }

    pub fn set_column_display(&mut self, display: IpcColumnDisplay) {
        if self.columns.is_empty() {
            return;
        }
        self.set_column_display_at(self.order.active_index(), display);
    }

    /// Restore a specific window's column display without selecting its column
    /// or replacing the active view/focus. Both routes use the same adaptation.
    pub(super) fn set_window_column_display(&mut self, id: u64, display: IpcColumnDisplay) {
        if let Some(index) = self
            .columns
            .iter()
            .position(|column| column.policy.rows().iter().any(|row| *row.id() == id))
        {
            self.set_column_display_at(index, display);
        }
    }

    fn set_column_display_at(&mut self, index: usize, display: IpcColumnDisplay) {
        let display = column_display_from_ipc(display);
        let col = &mut self.columns[index];
        if col.policy.display_mode == display {
            return;
        }

        cancel_resize_for_column(&mut self.order.resize, col);
        col.set_column_display(display);

        // With place_within_column, the tab indicator changes the column size immediately.
        self.data[index].update(col);
        col.update_tile_sizes(true);

        // Disable fullscreen if needed.
        if col.policy.display_mode != ColumnDisplay::Tabbed && col.tiles.len() > 1 {
            let window = col.tiles[col.policy.active_index()].window().id().clone();
            self.set_fullscreen(&window, false);
            self.set_maximized(&window, false);
        }
    }

    pub fn center_column(&mut self) {
        if self.columns.is_empty() {
            return;
        }

        self.animate_view_offset_to_column_centered(
            None,
            self.order.active_index(),
            self.options.animations.horizontal_view_movement.0,
        );

        let col = &mut self.columns[self.order.active_index()];
        cancel_resize_for_column(&mut self.order.resize, col);
    }

    pub fn center_window(&mut self, window: Option<&W::Id>) {
        if self.columns.is_empty() {
            return;
        }

        let col_idx = if let Some(window) = window {
            self.columns
                .iter()
                .position(|col| col.contains(window))
                .unwrap()
        } else {
            self.order.active_index()
        };

        // We can reasonably center only the active column.
        if col_idx != self.order.active_index() {
            return;
        }

        self.center_column();
    }

    pub fn center_visible_columns(&mut self) {
        if self.columns.is_empty() {
            return;
        }

        if self.is_centering_focused_column() {
            return;
        }

        // Consider the end of an ongoing animation because that's what compute to fit does too.
        let view_x = self.target_view_pos();
        let working_x = self.working_area.loc.x;
        let working_w = self.working_area.size.w;

        // Count all columns that are fully visible inside the working area.
        let mut width_taken = 0.;
        let mut leftmost_col_x = None;
        let mut active_col_x = None;

        let gap = self.options.layout.gaps;
        let col_xs = self.column_xs(self.data.iter().copied());
        for (idx, col_x) in col_xs.take(self.columns.len()).enumerate() {
            if col_x < view_x + working_x + gap {
                // Column goes off-screen to the left.
                continue;
            }

            leftmost_col_x.get_or_insert(col_x);

            let width = self.data[idx].width;
            if view_x + working_x + working_w < col_x + width + gap {
                // Column goes off-screen to the right. We can stop here.
                break;
            }

            if idx == self.order.active_index() {
                active_col_x = Some(col_x);
            }

            width_taken += width + gap;
        }

        if active_col_x.is_none() {
            // The active column wasn't fully on screen, so we can't meaningfully do anything.
            return;
        }

        let col = &mut self.columns[self.order.active_index()];
        cancel_resize_for_column(&mut self.order.resize, col);

        let free_space = working_w - width_taken + gap;
        let new_view_x = leftmost_col_x.unwrap() - free_space / 2. - working_x;

        self.animate_view_offset(
            self.order.active_index(),
            new_view_x - active_col_x.unwrap(),
        );
        // Just in case.
        self.animate_view_offset_to_column(None, self.order.active_index(), None);
    }

    pub fn view_pos(&self) -> f64 {
        self.column_x(self.order.active_index()) + self.view_offset.current()
    }

    pub fn target_view_pos(&self) -> f64 {
        self.column_x(self.order.active_index()) + self.view_offset.target()
    }

    // HACK: pass a self.data iterator in manually as a workaround for the lack of method partial
    // borrowing. Note that this method's return value does not borrow the entire &Self!
    fn column_xs(&self, data: impl Iterator<Item = ColumnData>) -> impl Iterator<Item = f64> {
        // Include the end origin for insertion after the last column.
        let widths = data.map(|data| data.width).chain(iter::once(0.));
        clarion_core::layout::column_origins(0., 0., self.options.layout.gaps, widths)
    }

    fn column_x(&self, column_idx: usize) -> f64 {
        self.column_xs(self.data.iter().copied())
            .nth(column_idx)
            .unwrap()
    }

    fn column_xs_in_render_order(
        &self,
        data: impl Iterator<Item = ColumnData>,
    ) -> impl Iterator<Item = f64> {
        let active_idx = self.order.active_index();
        let active_pos = self.column_x(active_idx);
        let offsets = self
            .column_xs(data)
            .enumerate()
            .filter_map(move |(idx, pos)| (idx != active_idx).then_some(pos));
        iter::once(active_pos).chain(offsets)
    }

    /// Numeric placement preferences from the live pure column owner.
    /// Used only at the input mutation boundary, without allocating a snapshot.
    pub(super) fn move_preferences(&self, id: u64) -> Option<(ColumnWidth, bool)> {
        self.columns
            .iter()
            .find(|column| column.policy.position(&id).is_some())
            .map(|column| (column.policy.width, column.policy.is_full_width))
    }

    pub fn columns(&self) -> impl Iterator<Item = &Column<W>> {
        self.columns.iter()
    }

    /// Commit settled policy geometry without animation offsets or a renderer.
    pub(super) fn with_layout_geometry(
        &self,
        f: &mut impl FnMut(&Tile<W>, Rectangle<f64, Logical>),
    ) {
        let view = self.target_view_pos();
        for (column, x) in zip(&self.columns, self.column_xs(self.data.iter().copied())) {
            for (tile, offset) in column.tiles() {
                let pos = offset + Point::from((x - view, 0.));
                f(
                    tile,
                    Rectangle::new(pos, tile.tile_expected_or_current_size()),
                );
            }
        }
    }

    fn columns_mut(&mut self) -> impl Iterator<Item = (&mut Column<W>, f64)> + '_ {
        let offsets = self.column_xs(self.data.iter().copied());
        zip(&mut self.columns, offsets)
    }

    fn columns_in_render_order(&self) -> impl Iterator<Item = (&Column<W>, f64)> + '_ {
        let offsets = self.column_xs_in_render_order(self.data.iter().copied());

        let (first, active, rest) = if self.columns.is_empty() {
            (&[][..], &[][..], &[][..])
        } else {
            let (first, rest) = self.columns.split_at(self.order.active_index());
            let (active, rest) = rest.split_at(1);
            (first, active, rest)
        };

        let columns = active.iter().chain(first).chain(rest);
        zip(columns, offsets)
    }

    fn columns_in_render_order_mut(&mut self) -> impl Iterator<Item = (&mut Column<W>, f64)> + '_ {
        let offsets = self.column_xs_in_render_order(self.data.iter().copied());

        let (first, active, rest) = if self.columns.is_empty() {
            (&mut [][..], &mut [][..], &mut [][..])
        } else {
            let (first, rest) = self.columns.split_at_mut(self.order.active_index());
            let (active, rest) = rest.split_at_mut(1);
            (first, active, rest)
        };

        let columns = active.iter_mut().chain(first).chain(rest);
        zip(columns, offsets)
    }

    pub fn columns_with_render_positions(
        &self,
    ) -> impl Iterator<Item = (&Column<W>, Point<f64, Logical>)> {
        let view_off = Point::from((-self.view_pos(), 0.));
        self.columns_in_render_order().map(move |(col, col_x)| {
            let col_off = Point::from((col_x, 0.));
            let col_render_off = col.render_offset();
            let pos = view_off + col_off + col_render_off;
            (col, pos)
        })
    }

    pub fn columns_with_render_positions_mut(
        &mut self,
    ) -> impl Iterator<Item = (&mut Column<W>, Point<f64, Logical>)> {
        let view_off = Point::from((-self.view_pos(), 0.));
        self.columns_in_render_order_mut().map(move |(col, col_x)| {
            let col_off = Point::from((col_x, 0.));
            let col_render_off = col.render_offset();
            let pos = view_off + col_off + col_render_off;
            (col, pos)
        })
    }

    pub fn tiles_with_render_positions(
        &self,
    ) -> impl Iterator<Item = (&Tile<W>, Point<f64, Logical>, bool)> {
        let scale = self.scale;
        self.columns_with_render_positions()
            .flat_map(move |(col, col_pos)| {
                col.tiles_in_render_order()
                    .map(move |(tile, tile_off, visible)| {
                        let pos = col_pos + tile_off + tile.render_offset();
                        // Round to physical pixels.
                        let pos = pos.to_physical_precise_round(scale).to_logical(scale);
                        (tile, pos, visible)
                    })
            })
    }

    pub fn tiles_with_render_positions_mut(
        &mut self,
        round: bool,
    ) -> impl Iterator<Item = (&mut Tile<W>, Point<f64, Logical>)> {
        let scale = self.scale;
        self.columns_with_render_positions_mut()
            .flat_map(move |(col, col_pos)| {
                col.tiles_in_render_order_mut()
                    .map(move |(tile, tile_off)| {
                        let mut pos = col_pos + tile_off + tile.render_offset();
                        // Round to physical pixels.
                        if round {
                            pos = pos.to_physical_precise_round(scale).to_logical(scale);
                        }
                        (tile, pos)
                    })
            })
    }

    pub fn tiles_with_ipc_layouts(&self) -> impl Iterator<Item = (&Tile<W>, WindowLayout)> {
        self.columns
            .iter()
            .enumerate()
            .flat_map(move |(col_idx, col)| {
                col.tiles().enumerate().map(move |(tile_idx, (tile, _))| {
                    let layout = WindowLayout {
                        // Our indices are 1-based, consistent with the actions.
                        pos_in_scrolling_layout: Some((col_idx + 1, tile_idx + 1)),
                        ..tile.ipc_layout_template()
                    };
                    (tile, layout)
                })
            })
    }

    pub(super) fn insert_hint_area(
        &self,
        position: InsertPosition,
    ) -> Option<Rectangle<f64, Logical>> {
        let mut hint_area = match position {
            InsertPosition::NewColumn(column_index) => {
                if column_index == 0 || column_index == self.columns.len() {
                    let size = Size::from((
                        300.,
                        (self.working_area.size.h - self.options.layout.gaps * 2.).max(1.),
                    ));
                    let mut loc = Point::from((
                        self.column_x(column_index),
                        self.working_area.loc.y + self.options.layout.gaps,
                    ));
                    if column_index == 0 && !self.columns.is_empty() {
                        loc.x -= size.w + self.options.layout.gaps;
                    }
                    Rectangle::new(loc, size)
                } else if column_index > self.columns.len() {
                    error!("insert hint column index is out of range");
                    return None;
                } else {
                    let size = Size::from((
                        300.,
                        (self.working_area.size.h - self.options.layout.gaps * 2.).max(1.),
                    ));
                    let loc = Point::from((
                        self.column_x(column_index) - size.w / 2. - self.options.layout.gaps / 2.,
                        self.working_area.loc.y + self.options.layout.gaps,
                    ));
                    Rectangle::new(loc, size)
                }
            }
            InsertPosition::InColumn(column_index, tile_index) => {
                if column_index > self.columns.len() {
                    error!("insert hint column index is out of range");
                    return None;
                }

                let col = &self.columns[column_index];
                if tile_index > col.tiles.len() {
                    error!("insert hint tile index is out of range");
                    return None;
                }

                let is_tabbed = col.policy.display_mode == ColumnDisplay::Tabbed;

                let (height, y) = if is_tabbed {
                    // In tabbed mode, there's only one tile visible, and we want to draw the hint
                    // at its top or bottom.
                    let top = col.tile_offset(col.policy.active_index()).y;
                    let bottom = top + col.data[col.policy.active_index()].size.h;

                    if tile_index <= col.policy.active_index() {
                        (150., top)
                    } else {
                        (150., bottom - 150.)
                    }
                } else {
                    let top = col.tile_offset(tile_index).y;

                    if tile_index == 0 {
                        (150., top)
                    } else if tile_index == col.tiles.len() {
                        (150., top - self.options.layout.gaps - 150.)
                    } else {
                        (300., top - self.options.layout.gaps / 2. - 150.)
                    }
                };

                // Adjust for place-within-column tab indicator.
                let origin_x = col.tiles_origin().x;
                let extra_w = if is_tabbed && col.sizing_mode().is_normal() {
                    col.tab_indicator.extra_size(col.tiles.len(), col.scale).w
                } else {
                    0.
                };

                let size = Size::from((self.data[column_index].width - extra_w, height));
                let loc = Point::from((self.column_x(column_index) + origin_x, y));
                Rectangle::new(loc, size)
            }
            InsertPosition::Floating => return None,
        };

        // First window on an empty workspace will cancel out any view offset. Replicate this
        // effect here.
        if self.columns.is_empty() {
            let view_offset = if self.is_centering_focused_column() {
                self.compute_new_view_offset_centered(
                    Some(0.),
                    0.,
                    hint_area.size.w,
                    SizingMode::Normal,
                )
            } else {
                self.compute_new_view_offset_fit(Some(0.), 0., hint_area.size.w, SizingMode::Normal)
            };
            hint_area.loc.x -= view_offset;
        } else {
            hint_area.loc.x -= self.view_pos();
        }

        Some(hint_area)
    }

    /// Returns the geometry of the active window relative to and clamped to the view.
    ///
    /// During animations, assumes the final view position.
    pub fn active_window_visual_rectangle(&self) -> Option<Rectangle<f64, Logical>> {
        let col = self.columns.get(self.order.active_index())?;

        let final_view_offset = self.view_offset.target();
        let view_off = Point::from((-final_view_offset, 0.));

        let (tile, tile_off) = col.tiles().nth(col.policy.active_index()).unwrap();

        let window_pos = view_off + tile_off + tile.window_loc();
        let window_size = tile.window_size();
        let window_rect = Rectangle::new(window_pos, window_size);

        let view = Rectangle::from_size(self.view_size);
        view.intersection(window_rect)
    }

    pub fn popup_target_rect(&self, id: &W::Id) -> Option<Rectangle<f64, Logical>> {
        for col in &self.columns {
            for (tile, pos) in col.tiles() {
                if tile.window().id() == id {
                    // In the scrolling layout, we try to position popups horizontally within the
                    // window geometry (so they remain visible even if the window scrolls flush with
                    // the left/right edge of the screen), and vertically within the whole parent
                    // working area.
                    let width = tile.window_size().w;
                    let height = self.parent_area.size.h;

                    let mut target = Rectangle::from_size(Size::from((width, height)));
                    target.loc.y += self.parent_area.loc.y;
                    target.loc.y -= pos.y;
                    target.loc.y -= tile.window_loc().y;

                    return Some(target);
                }
            }
        }
        None
    }

    pub fn toggle_width(&mut self, forwards: bool) {
        if self.columns.is_empty() {
            return;
        }

        let col = &mut self.columns[self.order.active_index()];
        col.toggle_width(None, forwards);

        cancel_resize_for_column(&mut self.order.resize, col);
    }

    pub fn toggle_full_width(&mut self) {
        if self.columns.is_empty() {
            return;
        }

        let col = &mut self.columns[self.order.active_index()];
        col.toggle_full_width();

        cancel_resize_for_column(&mut self.order.resize, col);
    }

    pub fn set_window_width(&mut self, window: Option<&W::Id>, change: SizeChange) {
        if self.columns.is_empty() {
            return;
        }

        let (col, tile_idx) = if let Some(window) = window {
            self.columns
                .iter_mut()
                .find_map(|col| {
                    col.tiles
                        .iter()
                        .position(|tile| tile.window().id() == window)
                        .map(|tile_idx| (col, Some(tile_idx)))
                })
                .unwrap()
        } else {
            (&mut self.columns[self.order.active_index()], None)
        };

        col.set_column_width(change, tile_idx, true);

        cancel_resize_for_column(&mut self.order.resize, col);
    }

    pub fn set_window_height(&mut self, window: Option<&W::Id>, change: SizeChange) {
        if self.columns.is_empty() {
            return;
        }

        let (col, tile_idx) = if let Some(window) = window {
            self.columns
                .iter_mut()
                .find_map(|col| {
                    col.tiles
                        .iter()
                        .position(|tile| tile.window().id() == window)
                        .map(|tile_idx| (col, Some(tile_idx)))
                })
                .unwrap()
        } else {
            (&mut self.columns[self.order.active_index()], None)
        };

        col.set_window_height(change, tile_idx, true);

        cancel_resize_for_column(&mut self.order.resize, col);
    }

    pub fn reset_window_height(&mut self, window: Option<&W::Id>) {
        if self.columns.is_empty() {
            return;
        }

        let (col, tile_idx) = if let Some(window) = window {
            self.columns
                .iter_mut()
                .find_map(|col| {
                    col.tiles
                        .iter()
                        .position(|tile| tile.window().id() == window)
                        .map(|tile_idx| (col, Some(tile_idx)))
                })
                .unwrap()
        } else {
            (&mut self.columns[self.order.active_index()], None)
        };

        col.reset_window_height(tile_idx);

        cancel_resize_for_column(&mut self.order.resize, col);
    }

    pub fn toggle_window_width(&mut self, window: Option<&W::Id>, forwards: bool) {
        if self.columns.is_empty() {
            return;
        }

        let (col, tile_idx) = if let Some(window) = window {
            self.columns
                .iter_mut()
                .find_map(|col| {
                    col.tiles
                        .iter()
                        .position(|tile| tile.window().id() == window)
                        .map(|tile_idx| (col, Some(tile_idx)))
                })
                .unwrap()
        } else {
            (&mut self.columns[self.order.active_index()], None)
        };

        col.toggle_width(tile_idx, forwards);

        cancel_resize_for_column(&mut self.order.resize, col);
    }

    pub fn toggle_window_height(&mut self, window: Option<&W::Id>, forwards: bool) {
        if self.columns.is_empty() {
            return;
        }

        let (col, tile_idx) = if let Some(window) = window {
            self.columns
                .iter_mut()
                .find_map(|col| {
                    col.tiles
                        .iter()
                        .position(|tile| tile.window().id() == window)
                        .map(|tile_idx| (col, Some(tile_idx)))
                })
                .unwrap()
        } else {
            (&mut self.columns[self.order.active_index()], None)
        };

        col.toggle_window_height(tile_idx, forwards);

        cancel_resize_for_column(&mut self.order.resize, col);
    }

    pub fn expand_column_to_available_width(&mut self) {
        if self.columns.is_empty() {
            return;
        }

        let col = &mut self.columns[self.order.active_index()];
        if !col.pending_sizing_mode().is_normal() || col.policy.is_full_width {
            return;
        }

        if self.is_centering_focused_column() {
            // Always-centered mode is different since the active window position cannot be
            // controlled (it's always at the center). I guess you could come up with different
            // logic here that computes the width in such a way so as to leave nearby columns fully
            // on screen while taking into account that the active column will remain centered
            // after resizing. But I'm not sure it's that useful? So let's do the simple thing.
            let col = &mut self.columns[self.order.active_index()];
            col.toggle_full_width();
            cancel_resize_for_column(&mut self.order.resize, col);
            return;
        }

        // NOTE: This logic won't work entirely correctly with small fixed-size maximized windows
        // (they have a different area and padding).

        // Consider the end of an ongoing animation because that's what compute to fit does too.
        let view_x = self.target_view_pos();
        let working_x = self.working_area.loc.x;
        let working_w = self.working_area.size.w;

        // Count all columns that are fully visible inside the working area.
        let mut width_taken = 0.;
        let mut leftmost_col_x = None;
        let mut active_col_x = None;
        let mut counted_non_active_column = false;

        let gap = self.options.layout.gaps;
        let col_xs = self.column_xs(self.data.iter().copied());
        for (idx, col_x) in col_xs.take(self.columns.len()).enumerate() {
            if col_x < view_x + working_x + gap {
                // Column goes off-screen to the left.
                continue;
            }

            leftmost_col_x.get_or_insert(col_x);

            let width = self.data[idx].width;
            if view_x + working_x + working_w < col_x + width + gap {
                // Column goes off-screen to the right. We can stop here.
                break;
            }

            if idx == self.order.active_index() {
                active_col_x = Some(col_x);
            } else {
                counted_non_active_column = true;
            }

            width_taken += width + gap;
        }

        if active_col_x.is_none() {
            // The active column wasn't fully on screen, so we can't meaningfully do anything.
            return;
        }

        let col = &mut self.columns[self.order.active_index()];

        let available_width = working_w - gap - width_taken - col.extra_size().w;
        if available_width <= 0. {
            // Nowhere to expand.
            return;
        }

        cancel_resize_for_column(&mut self.order.resize, col);

        if !counted_non_active_column {
            // Only the active column was fully on-screen (maybe it's the only column), so we're
            // about to set its width to 100% of the working area. Let's do it via
            // toggle_full_width() as it lets you back out of it more intuitively.
            col.toggle_full_width();
            return;
        }

        let active_width = self.data[self.order.active_index()].width;
        col.policy.width = ColumnWidth::Fixed(active_width + available_width);
        col.policy.preset_width_idx = None;
        col.policy.is_full_width = false;
        col.update_tile_sizes(true);

        // Put the leftmost window into the view.
        let new_view_x = leftmost_col_x.unwrap() - gap - working_x;
        self.animate_view_offset(
            self.order.active_index(),
            new_view_x - active_col_x.unwrap(),
        );
        // Just in case.
        self.animate_view_offset_to_column(None, self.order.active_index(), None);
    }

    pub fn set_fullscreen(&mut self, window: &W::Id, is_fullscreen: bool) -> bool {
        let mut col_idx = self
            .columns
            .iter()
            .position(|col| col.contains(window))
            .unwrap();

        if is_fullscreen == self.columns[col_idx].policy.is_pending_fullscreen {
            return false;
        }

        let mut col = &mut self.columns[col_idx];
        let is_tabbed = col.policy.display_mode == ColumnDisplay::Tabbed;

        cancel_resize_for_column(&mut self.order.resize, col);

        if is_fullscreen && (col.tiles.len() > 1 && !is_tabbed) {
            // This wasn't the only window in its column; extract it into a separate column.
            self.consume_or_expel_window_right(Some(window));
            col_idx += 1;
            col = &mut self.columns[col_idx];
        }

        col.set_fullscreen(is_fullscreen);

        // With place_within_column, the tab indicator changes the column size immediately.
        self.data[col_idx].update(col);

        true
    }

    pub fn set_maximized(&mut self, window: &W::Id, maximize: bool) -> bool {
        let mut col_idx = self
            .columns
            .iter()
            .position(|col| col.contains(window))
            .unwrap();

        if maximize == self.columns[col_idx].policy.is_pending_maximized {
            return false;
        }

        let mut col = &mut self.columns[col_idx];
        let is_tabbed = col.policy.display_mode == ColumnDisplay::Tabbed;

        cancel_resize_for_column(&mut self.order.resize, col);

        if maximize && (col.tiles.len() > 1 && !is_tabbed) {
            // This wasn't the only window in its column; extract it into a separate column.
            self.consume_or_expel_window_right(Some(window));
            col_idx += 1;
            col = &mut self.columns[col_idx];
        }

        col.set_maximized(maximize);

        // With place_within_column, the tab indicator changes the column size immediately.
        self.data[col_idx].update(col);

        true
    }

    pub fn render_above_top_layer(&self) -> bool {
        // Render above the top layer if we're on a fullscreen window and the view is stationary.
        if self.columns.is_empty() {
            return false;
        }

        if !self.view_offset.is_static() {
            return false;
        }

        self.columns[self.order.active_index()]
            .sizing_mode()
            .is_fullscreen()
    }

    pub(crate) fn scene_items<'a>(
        &'a self,
        normal_visible: bool,
        focus: bool,
        items: &mut Vec<crate::scene::workspace_composition::OrderedItem<'a, W>>,
    ) {
        use crate::layout::scene_source::{ClosingRenderSource, TabRenderSource, TileRenderSource};
        use crate::scene::workspace_composition::{Channel, Item, OrderedItem, TileSpace};
        let scale = Scale::from(self.scale);
        if normal_visible {
            let view = Rectangle::new(Point::from((self.view_pos(), 0.)), self.view_size);
            for closing in self.closing_windows.iter().rev() {
                items.push(OrderedItem {
                    channel: Channel::TilingNormal,
                    item: Item::Closing {
                        source: ClosingRenderSource::new(closing),
                        view,
                        scale,
                        space: TileSpace::Scrolling,
                    },
                });
            }
        }
        let mut first_normal = true;
        let mut first_moving = true;
        for (column, column_position) in self.columns_with_render_positions() {
            let moving = column.is_moving_between_workspaces();
            let (channel, first) = if moving {
                first_normal = false;
                (Channel::TilingMoving, &mut first_moving)
            } else {
                first_moving = false;
                (Channel::TilingNormal, &mut first_normal)
            };
            if !moving && !normal_visible {
                continue;
            }
            items.push(OrderedItem {
                channel,
                item: Item::Tab {
                    source: TabRenderSource::new(&column.tab_indicator),
                    position: column_position
                        .to_physical_precise_round(scale)
                        .to_logical(scale),
                },
            });
            for (tile, tile_offset, visible) in column.tiles_in_render_order() {
                let focused = focus && *first;
                *first = false;
                // Hidden tabs remain present while their alpha is animated.
                if !visible && tile.presentation.alpha_animation.is_none() {
                    continue;
                }
                let position = (column_position + tile_offset + tile.render_offset())
                    .to_physical_precise_round(scale)
                    .to_logical(scale);
                items.push(OrderedItem {
                    channel,
                    item: Item::Tile {
                        source: TileRenderSource::new(tile),
                        position,
                        focused,
                        space: TileSpace::Scrolling,
                    },
                });
            }
        }
    }

    pub fn render<R: ClarionRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        xray_pos: XrayPos,
        focus_ring: bool,
        layer: RenderLayer,
        push: &mut dyn FnMut(ScrollingSpaceRenderElement<R>),
    ) {
        let scale = Scale::from(self.scale);

        // Draw the closing windows on top of the other windows.
        if layer.is_normal() {
            let view_rect = Rectangle::new(Point::from((self.view_pos(), 0.)), self.view_size);
            for closing in self.closing_windows.iter().rev() {
                for elem in closing.render(ctx.r(), view_rect, scale) {
                    push(elem.into());
                }
            }
        }

        if self.columns.is_empty() {
            return;
        }

        let mut first = true;

        // This matches self.tiles_with_render_positions().
        for (col, col_pos) in self.columns_with_render_positions() {
            // Skip columns belonging to a different render layer.
            if layer.is_normal() == col.is_moving_between_workspaces() {
                first = false;
                continue;
            }

            // Draw the tab indicator on top.
            {
                let pos = col_pos.to_physical_precise_round(scale).to_logical(scale);
                col.tab_indicator
                    .render(ctx.renderer, pos, &mut |elem| push(elem.into()));
            }

            for (tile, tile_off, visible) in col.tiles_in_render_order() {
                let tile_pos = col_pos + tile_off + tile.render_offset();
                // Round to physical pixels.
                let tile_pos = tile_pos.to_physical_precise_round(scale).to_logical(scale);

                // And now the drawing logic.

                // For the active tile (which comes first), draw the focus ring.
                let focus_ring = focus_ring && first;
                first = false;

                // In the scrolling layout, we currently use visible only for hidden tabs in the
                // tabbed mode. We want to animate their opacity when going in and out of tabbed
                // mode, so we don't want to apply "visible" immediately. However, "visible" is
                // also used for input handling, and there we *do* want to apply it immediately.
                // So, let's just selectively ignore "visible" here when animating alpha.
                let visible = visible || tile.presentation.alpha_animation.is_some();
                if !visible {
                    continue;
                }

                let xray_pos = xray_pos.offset(tile_pos);
                tile.scene()
                    .render(ctx.r(), tile_pos, xray_pos, focus_ring, &mut |elem| {
                        push(elem.into())
                    });
            }
        }
    }

    pub fn window_under(&self, pos: Point<f64, Logical>) -> Option<(&W, HitType)> {
        // This matches self.tiles_with_render_positions().
        let scale = self.scale;
        for (col, col_pos) in self.columns_with_render_positions() {
            // Hit the tab indicator.
            if col.policy.display_mode == ColumnDisplay::Tabbed && col.sizing_mode().is_normal() {
                let col_pos = col_pos.to_physical_precise_round(scale).to_logical(scale);

                if let Some(idx) = col.tab_indicator.hit(
                    col.tab_indicator_area(),
                    col.tiles.len(),
                    scale,
                    pos - col_pos,
                ) {
                    let hit = HitType::Activate {
                        is_tab_indicator: true,
                    };
                    return Some((col.tiles[idx].window(), hit));
                }
            }

            for (tile, tile_off, visible) in col.tiles_in_render_order() {
                if !visible {
                    continue;
                }

                let tile_pos = col_pos + tile_off + tile.render_offset();
                // Round to physical pixels.
                let tile_pos = tile_pos.to_physical_precise_round(scale).to_logical(scale);

                if let Some(rv) = HitType::hit_tile(tile, tile_pos, pos) {
                    return Some(rv);
                }
            }
        }

        None
    }

    pub fn view_offset_gesture_begin(&mut self, is_touchpad: bool) {
        if self.columns.is_empty() || self.order.resize.is_some() {
            return;
        }
        let kind = if is_touchpad {
            clarion_core::viewport::GestureKind::Touchpad
        } else {
            clarion_core::viewport::GestureKind::Pointer
        };
        self.view_offset.begin(
            kind,
            self.working_area.size.w,
            self.options.animations.horizontal_view_movement.0,
        );
    }

    pub fn dnd_scroll_gesture_begin(&mut self) {
        if !self.view_offset.begin(
            clarion_core::viewport::GestureKind::Dnd,
            self.working_area.size.w,
            self.options.animations.horizontal_view_movement.0,
        ) {
            return;
        }
        self.order.resize = None;
    }

    pub fn view_offset_gesture_update(
        &mut self,
        delta_x: f64,
        timestamp: Duration,
        is_touchpad: bool,
    ) -> Option<bool> {
        self.view_offset
            .policy
            .update(delta_x, timestamp, is_touchpad, self.working_area.size.w)
    }

    pub fn dnd_scroll_gesture_scroll(&mut self, delta: f64) -> bool {
        let area = self.working_area;
        let bounds = clarion_core::viewport::dnd_bounds(
            self.columns.iter().map(Column::width),
            self.order.active_index(),
            self.options.layout.gaps,
            clarion_core::Rect::new(area.loc.x, area.loc.y, area.size.w, area.size.h),
        );
        let config = &self.options.gestures.dnd_edge_view_scroll;
        self.view_offset.policy.dnd_scroll(
            delta,
            self.clock.now_unadjusted(),
            Duration::from_millis(u64::from(config.delay_ms)),
            config.max_speed,
            bounds,
        )
    }

    pub fn view_offset_gesture_cancel(&mut self, is_touchpad: Option<bool>) -> bool {
        let time = self.view_offset.time();
        let clock = self.clock.clone();
        let config = self.options.animations.horizontal_view_movement.0;
        self.view_offset
            .policy
            .cancel(is_touchpad, self.working_area.size.w, time, |intent| {
                convert_viewport_motion(clock, config, intent)
            })
    }

    pub fn view_offset_gesture_end(&mut self, is_touchpad: Option<bool>) -> bool {
        use clarion_core::viewport::{ColumnGeometry, SnapContext};
        let time = self.view_offset.time();
        let Some(release) =
            self.view_offset
                .policy
                .release(is_touchpad, self.working_area.size.w, time)
        else {
            return false;
        };
        if self.columns.is_empty() {
            self.view_offset.policy.set_static(release.current);
            return true;
        }
        // Gesture-release metadata is built once at this input boundary. Render
        // frames never clone/rebuild the column topology or viewport intent.
        let columns: Vec<_> = self
            .columns
            .iter()
            .map(|column| {
                let sizing = column.sizing_mode();
                ColumnGeometry {
                    width: column.width(),
                    sizing: core_column_sizing(sizing),
                }
            })
            .collect();
        let rect = |area: Rectangle<f64, Logical>| {
            clarion_core::Rect::new(area.loc.x, area.loc.y, area.size.w, area.size.h)
        };
        let context = SnapContext {
            working: rect(self.working_area),
            parent: rect(self.parent_area),
            view_width: self.view_size.w,
            gaps: self.options.layout.gaps,
            centered: self.is_centering_focused_column(),
            center_on_overflow: matches!(
                self.options.layout.center_focused_column,
                CenterFocusedColumn::OnOverflow
            ),
        };
        let Some(decision) = clarion_core::viewport::snap_gesture(
            &columns,
            self.order.active_index(),
            context,
            release,
        ) else {
            self.view_offset.policy.set_static(release.current);
            return true;
        };
        if self.order.active_index() != decision.active {
            self.order.view_offset_to_restore = None;
        }
        self.order.activate(decision.active);
        self.view_offset.settle(
            decision.motion,
            self.options.animations.horizontal_view_movement.0,
        );
        // Retain the established fit correction for oversized/window-rule slots.
        self.animate_view_offset_to_column(None, decision.active, None);
        true
    }

    pub fn dnd_scroll_gesture_end(&mut self) {
        if self.view_offset.policy.finish_unscrolled_dnd() {
            if !self.columns.is_empty() {
                self.animate_view_offset_to_column(None, self.order.active_index(), None);
            }
            return;
        }
        self.view_offset_gesture_end(None);
    }

    pub fn interactive_resize_begin(&mut self, window: W::Id, edges: ResizeEdge) -> bool {
        if self.order.resize.is_some() {
            return false;
        }

        let Some(col) = self.columns.iter_mut().find(|col| col.contains(&window)) else {
            return false;
        };

        if !col.pending_sizing_mode().is_normal() {
            return false;
        }

        let Some(tile) = col
            .tiles
            .iter_mut()
            .find(|tile| tile.window().id() == &window)
        else {
            return false;
        };

        let original_window_size = tile.window_size();
        tile.start_wobble(WobbleAnchor::from_resize_edges(edges));

        let resize = clarion_core::resize::Resize::new(
            tile.window().core_id(),
            clarion_core::Size::new(original_window_size.w, original_window_size.h),
            edges.bits(),
        );
        self.order.resize = Some(resize);

        self.view_offset.stop_anim_and_gesture();

        true
    }

    pub fn interactive_resize_update(
        &mut self,
        window: &W::Id,
        delta: Point<f64, Logical>,
    ) -> bool {
        let Some(resize) = self.order.resize else {
            return false;
        };
        let Some(id) = self
            .tiles()
            .find(|tile| tile.window().id() == window)
            .map(|tile| tile.window().core_id())
        else {
            return false;
        };
        if id != resize.id {
            return false;
        }
        let centered = self.is_centering_focused_column();
        let col = self
            .columns
            .iter_mut()
            .find(|col| col.policy.position(&id).is_some())
            .unwrap();
        let tile_idx = col.policy.position(&id).unwrap();
        let Some(intent) = col
            .policy
            .resize_intent(resize, [delta.x, delta.y], centered)
        else {
            return false;
        };
        if let Some(width) = intent.width {
            col.set_column_width(SizeChange::SetFixed(width), Some(tile_idx), false);
        }
        if let Some(height) = intent.height {
            col.set_window_height(SizeChange::SetFixed(height), Some(tile_idx), false);
        }
        true
    }

    pub fn interactive_resize_end(&mut self, window: Option<&W::Id>) {
        let Some(resize) = &self.order.resize else {
            return;
        };
        let resized_window = resize.id;

        if let Some(window) = window {
            if !self.tiles().any(|tile| {
                tile.window().id() == window && tile.window().core_id() == resized_window
            }) {
                return;
            }

            // Animate the active window into view right away.
            if self.columns[self.order.active_index()].contains(window) {
                self.animate_view_offset_to_column(None, self.order.active_index(), None);
            }
        }

        if let Some(tile) = self.columns.iter_mut().find_map(|column| {
            column
                .tiles
                .iter_mut()
                .find(|tile| tile.window().core_id() == resized_window)
        }) {
            tile.finish_wobble();
        }
        self.order.resize = None;
    }

    pub fn refresh(&mut self, is_active: bool, is_focused: bool) {
        for (col_idx, col) in self.columns.iter_mut().enumerate() {
            let mut col_resize_data = None;
            if let Some(resize) = &self.order.resize {
                if col.policy.position(&resize.id).is_some() {
                    col_resize_data = Some(InteractiveResizeData {
                        edges: ResizeEdge::from_bits_retain(resize.edges),
                    });
                }
            }

            let is_tabbed = col.policy.display_mode == ColumnDisplay::Tabbed;
            let extra_size = col.extra_size();

            // If transactions are disabled, also disable combined throttling, for more intuitive
            // behavior. In tabbed display mode, only one window is visible, so individual
            // throttling makes more sense.
            let individual_throttling = self.options.disable_transactions || is_tabbed;

            let intent = if self.options.disable_resize_throttling {
                ConfigureIntent::CanSend
            } else if individual_throttling {
                // In this case, we don't use combined throttling, but rather compute throttling
                // individually below.
                ConfigureIntent::CanSend
            } else {
                col.tiles
                    .iter()
                    .fold(ConfigureIntent::NotNeeded, |intent, tile| {
                        match (intent, tile.window().configure_intent()) {
                            (_, ConfigureIntent::ShouldSend) => ConfigureIntent::ShouldSend,
                            (ConfigureIntent::NotNeeded, tile_intent) => tile_intent,
                            (ConfigureIntent::CanSend, ConfigureIntent::Throttled) => {
                                ConfigureIntent::Throttled
                            }
                            (intent, _) => intent,
                        }
                    })
            };

            for (tile_idx, tile) in col.tiles.iter_mut().enumerate() {
                let win = tile.window_mut();

                let active_in_column = col.policy.active_index() == tile_idx;
                win.set_active_in_column(active_in_column);
                win.set_floating(false);

                let mut active = is_active && self.order.active_index() == col_idx;
                if self.options.deactivate_unfocused_windows {
                    active &= active_in_column && is_focused;
                } else {
                    // In tabbed mode, all tabs have activated state to reduce unnecessary
                    // animations when switching tabs.
                    active &= active_in_column || is_tabbed;
                }
                win.set_activated(active);

                win.set_interactive_resize(col_resize_data);

                let border_config = self.options.layout.border.merged_with(&win.rules().border);
                let bounds = compute_toplevel_bounds(
                    border_config,
                    self.working_area.size,
                    extra_size,
                    self.options.layout.gaps,
                );
                win.set_bounds(bounds);

                let intent = if individual_throttling {
                    win.configure_intent()
                } else {
                    intent
                };

                if matches!(
                    intent,
                    ConfigureIntent::CanSend | ConfigureIntent::ShouldSend
                ) {
                    win.send_pending_configure();
                }

                win.refresh();
            }
        }
    }

    #[cfg(test)]
    pub fn view_size(&self) -> Size<f64, Logical> {
        self.view_size
    }

    #[cfg(test)]
    pub fn parent_area(&self) -> Rectangle<f64, Logical> {
        self.parent_area
    }

    #[cfg(test)]
    pub fn clock(&self) -> &Clock {
        &self.clock
    }

    #[cfg(test)]
    pub fn options(&self) -> &Rc<Options> {
        &self.options
    }

    #[cfg(test)]
    pub fn active_column_idx(&self) -> usize {
        self.order.active_index()
    }

    #[cfg(test)]
    pub(super) fn view_offset(&self) -> &ViewOffset {
        &self.view_offset
    }

    #[cfg(test)]
    pub fn verify_invariants(&self) {
        assert!(self.view_size.w > 0.);
        assert!(self.view_size.h > 0.);
        assert!(self.scale > 0.);
        assert!(self.scale.is_finite());
        assert_eq!(self.columns.len(), self.data.len());
        assert_eq!(self.columns.len(), self.order.ids().len());
        for (column, id) in zip(&self.columns, self.order.ids()) {
            assert_eq!(
                column.policy.id, *id,
                "physical column order must project core IDs"
            );
        }
        assert_eq!(
            self.working_area,
            compute_working_area(self.parent_area, self.scale, self.options.layout.struts)
        );

        if !self.columns.is_empty() {
            assert!(self.order.active_index() < self.columns.len());

            for (column, data) in zip(&self.columns, &self.data) {
                assert!(Rc::ptr_eq(&self.options, &column.options));
                assert_eq!(self.clock, column.clock);
                assert_eq!(self.scale, column.scale);
                column.verify_invariants();

                let mut data2 = *data;
                data2.update(column);
                assert_eq!(data, &data2, "column data must be up to date");
            }

            let col = &self.columns[self.order.active_index()];

            if self.order.view_offset_to_restore.is_some() {
                assert!(
                    !col.sizing_mode().is_normal(),
                    "when view_offset_to_restore is set, \
                     the active column must be fullscreen or maximized"
                );
            }
        }

        if let Some(resize) = &self.order.resize {
            assert!(
                self.columns
                    .iter()
                    .flat_map(|col| &col.tiles)
                    .any(|tile| tile.window().core_id() == resize.id),
                "interactive resize window must be present in the layout"
            );
        }
    }
}

fn convert_viewport_motion(
    clock: Clock,
    config: clarion_config::Animation,
    intent: clarion_core::viewport::MotionIntent,
) -> clarion_motion::trajectory::Trajectory {
    Animation::new(clock, intent.from, intent.to, intent.velocity, config).into_trajectory()
}

impl ViewOffset {
    fn new(clock: Clock, offset: f64) -> Self {
        Self {
            clock,
            policy: clarion_core::viewport::Viewport::new(offset),
        }
    }
    fn time(&self) -> clarion_core::viewport::ViewportTime {
        clarion_core::viewport::ViewportTime {
            animation: self.clock.now(),
            event: self.clock.now_unadjusted(),
            rate: self.clock.rate(),
            complete: self.clock.should_complete_instantly(),
        }
    }
    pub fn current(&self) -> f64 {
        self.policy.current_at(self.time())
    }
    pub fn target(&self) -> f64 {
        self.policy.target()
    }
    fn stationary(&self) -> f64 {
        self.policy.stationary()
    }
    pub fn is_static(&self) -> bool {
        self.policy.is_static()
    }
    pub fn is_gesture(&self) -> bool {
        self.policy.is_gesture()
    }
    pub fn is_dnd_scroll(&self) -> bool {
        self.policy.is_dnd()
    }
    pub fn is_animation_ongoing(&self) -> bool {
        self.policy.is_animating()
    }
    pub fn offset(&mut self, delta: f64) {
        self.policy.translate(delta);
    }
    fn advance(&mut self) {
        let time = self.time();
        self.policy.advance(time);
    }
    fn begin(
        &mut self,
        kind: clarion_core::viewport::GestureKind,
        width: f64,
        config: clarion_config::Animation,
    ) -> bool {
        let time = self.time();
        let clock = self.clock.clone();
        self.policy.begin(kind, width, time, |intent| {
            convert_viewport_motion(clock, config, intent)
        })
    }
    fn settle(
        &mut self,
        intent: clarion_core::viewport::MotionIntent,
        config: clarion_config::Animation,
    ) {
        let clock = self.clock.clone();
        self.policy.settle(intent, |intent| {
            convert_viewport_motion(clock, config, intent)
        });
    }
    fn animate_to(&mut self, to: f64, pixel: f64, config: clarion_config::Animation) {
        let time = self.time();
        let clock = self.clock.clone();
        self.policy.animate_to(to, pixel, time, |intent| {
            convert_viewport_motion(clock, config, intent)
        });
    }
    pub fn cancel_gesture(&mut self) {
        let time = self.time();
        self.policy.freeze_gesture(time);
    }
    pub fn stop_anim_and_gesture(&mut self) {
        let time = self.time();
        self.policy.freeze(time);
    }
}

impl ColumnData {
    pub fn new<W: LayoutElement>(column: &Column<W>) -> Self {
        let mut rv = Self { width: 0. };
        rv.update(column);
        rv
    }

    pub fn update<W: LayoutElement>(&mut self, column: &Column<W>) {
        self.width = column.width();
    }
}

impl TileData {
    pub fn new<W: LayoutElement>(tile: &Tile<W>) -> Self {
        let mut rv = Self {
            size: Size::default(),
            interactively_resizing_by_left_edge: false,
        };
        rv.update(tile);
        rv
    }

    pub fn update<W: LayoutElement>(&mut self, tile: &Tile<W>) {
        self.size = tile.tile_size();
        self.interactively_resizing_by_left_edge = tile
            .window()
            .interactive_resize_data()
            .is_some_and(|data| data.edges.contains(ResizeEdge::LEFT));
    }
}

pub(super) fn column_width_from_preset(value: PresetSize) -> ColumnWidth {
    match value {
        PresetSize::Proportion(p) => ColumnWidth::Proportion(p.clamp(0., 10000.)),
        PresetSize::Fixed(f) => ColumnWidth::Fixed(f64::from(f.clamp(1, 100000))),
    }
}

fn dimension_change(change: SizeChange) -> DimensionChange {
    match change {
        SizeChange::SetFixed(value) => DimensionChange::SetFixed(f64::from(value)),
        SizeChange::SetProportion(value) => DimensionChange::SetProportion(value),
        SizeChange::AdjustFixed(value) => DimensionChange::AdjustFixed(f64::from(value)),
        SizeChange::AdjustProportion(value) => DimensionChange::AdjustProportion(value),
    }
}

fn column_display_from_ipc(display: IpcColumnDisplay) -> ColumnDisplay {
    match display {
        IpcColumnDisplay::Normal => ColumnDisplay::Normal,
        IpcColumnDisplay::Tabbed => ColumnDisplay::Tabbed,
        IpcColumnDisplay::Dwindle => ColumnDisplay::Dwindle,
    }
}

fn column_display_to_ipc(display: ColumnDisplay) -> IpcColumnDisplay {
    match display {
        ColumnDisplay::Normal => IpcColumnDisplay::Normal,
        ColumnDisplay::Tabbed => IpcColumnDisplay::Tabbed,
        ColumnDisplay::Dwindle => IpcColumnDisplay::Dwindle,
    }
}

impl<W: LayoutElement> Column<W> {
    #[allow(clippy::too_many_arguments)]
    fn new_with_tile(
        tile: Tile<W>,
        view_size: Size<f64, Logical>,
        working_area: Rectangle<f64, Logical>,
        parent_area: Rectangle<f64, Logical>,
        scale: f64,
        width: ColumnWidth,
        is_full_width: bool,
    ) -> Self {
        let options = tile.presentation.options.clone();

        let display_mode = column_display_from_ipc(
            tile.window()
                .rules()
                .default_column_display
                .unwrap_or(options.layout.default_column_display),
        );

        // Try to match width to a preset width. Consider the following case: a terminal (foot)
        // sizes itself to the terminal grid. We open it with default-column-width 0.5. It shrinks
        // by a few pixels to evenly match the terminal grid. Then we press
        // switch-preset-column-width intending to go to proportion 0.667, but the preset width
        // matching code picks the proportion 0.5 preset because it's the next smallest width after
        // the current foot's window width. Effectively, this makes the first
        // switch-preset-column-width press ignored.
        //
        // However, here, we do know that width = proportion 0.5 (regardless of what the window
        // opened with), and we can match it to a preset right away, if one exists.
        let preset_width_idx = options
            .layout
            .preset_column_widths
            .iter()
            .position(|preset| width == column_width_from_preset(*preset));

        let mut rv = Self {
            tiles: vec![],
            data: vec![],
            policy: ColumnPolicy::new(
                ColumnId::next().get(),
                width,
                is_full_width,
                display_mode,
                preset_width_idx,
            ),
            dwindle_offsets: Rc::default(),
            tab_indicator: TabIndicator::new(options.layout.tab_indicator),
            move_x_animation: None,
            move_y_animation: None,
            view_size,
            working_area,
            parent_area,
            scale,
            clock: tile.presentation.clock.clone(),
            options,
        };

        let pending_sizing_mode = tile.window().pending_sizing_mode();

        rv.add_tile_at(0, tile);

        match pending_sizing_mode {
            SizingMode::Normal => (),
            SizingMode::Maximized => rv.set_maximized(true),
            SizingMode::Fullscreen => rv.set_fullscreen(true),
        }

        // Animate the tab indicator for new columns.
        if display_mode == ColumnDisplay::Tabbed
            && !rv.options.layout.tab_indicator.hide_when_single_tab
            && rv.sizing_mode().is_normal()
        {
            // Usually new columns are created together with window movement actions. For new
            // windows, we handle that in start_open_animation().
            rv.tab_indicator
                .start_open_animation(rv.clock.clone(), rv.options.animations.window_movement.0);
        }

        rv
    }

    pub fn id(&self) -> ColumnId {
        ColumnId::specific(self.policy.id)
    }

    fn update_config(
        &mut self,
        view_size: Size<f64, Logical>,
        working_area: Rectangle<f64, Logical>,
        parent_area: Rectangle<f64, Logical>,
        scale: f64,
        options: Rc<Options>,
    ) {
        let mut update_sizes = false;

        if self.view_size != view_size
            || self.working_area != working_area
            || self.parent_area != parent_area
        {
            update_sizes = true;
        }

        // If preset widths changed, clear our stored preset index.
        if self.options.layout.preset_column_widths != options.layout.preset_column_widths {
            self.policy.preset_width_idx = None;
        }

        // If preset heights changed, make our heights non-preset.
        if self.options.layout.preset_window_heights != options.layout.preset_window_heights {
            self.convert_heights_to_auto();
            update_sizes = true;
        }

        if self.options.layout.gaps != options.layout.gaps {
            update_sizes = true;
        }

        if self.options.layout.border.off != options.layout.border.off
            || self.options.layout.border.width != options.layout.border.width
        {
            update_sizes = true;
        }

        if self.options.layout.tab_indicator != options.layout.tab_indicator {
            update_sizes = true;
        }

        for (tile, data) in zip(&mut self.tiles, &mut self.data) {
            tile.update_config(view_size, scale, options.clone());
            data.update(tile);
        }

        self.tab_indicator
            .update_config(options.layout.tab_indicator);
        self.view_size = view_size;
        self.working_area = working_area;
        self.parent_area = parent_area;
        self.scale = scale;
        self.options = options;

        if update_sizes {
            self.update_tile_sizes(false);
        }
    }

    pub fn update_shaders(&mut self) {
        for tile in &mut self.tiles {
            tile.update_shaders();
        }

        self.tab_indicator.update_shaders();
    }

    pub fn advance_animations(&mut self) {
        if let Some(move_) = &mut self.move_x_animation {
            if move_.anim.is_done() {
                self.move_x_animation = None;
            }
        }
        if let Some(move_) = &mut self.move_y_animation {
            if move_.anim.is_done() {
                self.move_y_animation = None;
            }
        }

        for tile in &mut self.tiles {
            tile.advance_animations();
        }

        self.tab_indicator.advance_animations();
    }

    pub fn are_animations_ongoing(&self) -> bool {
        self.move_x_animation.is_some()
            || self.move_y_animation.is_some()
            || self.tab_indicator.are_animations_ongoing()
            || self.tiles.iter().any(Tile::are_animations_ongoing)
    }

    pub fn are_transitions_ongoing(&self) -> bool {
        self.move_x_animation.is_some()
            || self.move_y_animation.is_some()
            || self.tab_indicator.are_animations_ongoing()
            || self.tiles.iter().any(Tile::are_transitions_ongoing)
    }

    pub fn is_moving_between_workspaces(&self) -> bool {
        self.move_y_animation
            .as_ref()
            .is_some_and(|anim| anim.is_between_workspaces)
            || self
                .move_x_animation
                .as_ref()
                .is_some_and(|anim| anim.is_between_workspaces)
            // When moving a window between workspaces into the scrolling layout, it will be
            // immediately put into a new column, however the Y move animation and the
            // moving-between-workspaces flag will be set on the tile rather than on that new
            // column. Since the scrolling layout rendering checks moving between workspaces
            // per-column rather than per-tile, we need to include the tiles here.
            //
            // This is not always visually correct: for example, interactive-moving a tile into an
            // otherwise stationary column will now cause that entire column to draw unculled. I'm
            // not sure there's a good solution overall for all edge cases here. Consider that the
            // column tab indicator needs to draw on top of the tiles, but in this counterexample
            // above, the column is stationary, so the single moving tile would instead need to draw
            // on top of the tab indicator.
            //
            // I'm going with this simple check for now that should look ok in most cases.
            || self
                .tiles
                .iter()
                .any(|tile| tile.is_moving_between_workspaces())
    }

    pub fn update_render_elements(&mut self, is_active: bool, view_rect: Rectangle<f64, Logical>) {
        let active_idx = self.policy.active_index();
        for (tile_idx, (tile, tile_off)) in self.tiles_mut().enumerate() {
            let is_active = is_active && tile_idx == active_idx;

            let mut tile_view_rect = view_rect;
            tile_view_rect.loc -= tile_off + tile.render_offset();
            tile.update_render_elements(is_active, tile_view_rect);
        }

        let config = self.tab_indicator.config();
        let offsets = self.tile_offsets_iter(self.data.iter().copied());
        let tabs = zip(&self.tiles, offsets)
            .enumerate()
            .map(|(tile_idx, (tile, tile_off))| {
                let is_active = tile_idx == active_idx;
                let is_urgent = tile.window().is_urgent();
                let tile_pos = tile_off + tile.render_offset();
                let geometry = Rectangle::new(tile_pos, tile.animated_tile_size());
                TabInfo::new(
                    &tile.window().rules().tab_indicator,
                    *tile.focus_ring_config(),
                    *tile.border_config(),
                    geometry,
                    is_active,
                    is_urgent,
                    &config,
                )
            });

        // Hide the tab indicator in fullscreen. If you have it configured to overlap the window,
        // you don't want that to happen in fullscreen. Also, laying things out correctly when the
        // tab indicator is within the column and the column goes fullscreen, would require too
        // many changes to the code for too little benefit (it's mostly invisible anyway).
        let enabled =
            self.policy.display_mode == ColumnDisplay::Tabbed && self.sizing_mode().is_normal();

        self.tab_indicator.update_render_elements(
            enabled,
            self.tab_indicator_area(),
            view_rect,
            self.tiles.len(),
            tabs,
            is_active,
            self.scale,
        );
    }

    pub fn is_pending_fullscreen(&self) -> bool {
        self.policy.is_pending_fullscreen
    }

    pub fn is_pending_maximized(&self) -> bool {
        self.policy.is_pending_maximized
    }

    pub fn display_mode(&self) -> IpcColumnDisplay {
        column_display_to_ipc(self.policy.display_mode)
    }

    #[cfg(test)]
    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    pub fn pending_sizing_mode(&self) -> SizingMode {
        if self.policy.is_pending_fullscreen {
            SizingMode::Fullscreen
        } else if self.policy.is_pending_maximized {
            SizingMode::Maximized
        } else {
            SizingMode::Normal
        }
    }

    pub fn render_offset(&self) -> Point<f64, Logical> {
        let mut offset = Point::from((0., 0.));

        if let Some(move_) = &self.move_x_animation {
            offset.x += move_.from * move_.anim.value();
        }
        if let Some(move_) = &self.move_y_animation {
            offset.y += move_.from * move_.anim.value();
        }

        offset
    }

    pub fn animate_move_from(&mut self, from: Point<f64, Logical>) {
        self.animate_move_from_with_config(from, self.options.animations.window_movement.0);
    }

    pub fn animate_move_from_with_config(
        &mut self,
        from: Point<f64, Logical>,
        config: clarion_config::Animation,
    ) {
        self.animate_move_x_from_with_config(from.x, config);
        self.animate_move_y_from_with_config(from.y, config);
    }

    pub fn animate_move_x_from(&mut self, from_x_offset: f64) {
        self.animate_move_x_from_with_config(
            from_x_offset,
            self.options.animations.window_movement.0,
        );
    }

    pub fn animate_move_x_from_with_config(
        &mut self,
        from_x_offset: f64,
        config: clarion_config::Animation,
    ) {
        self.move_x_animation = MoveAnimation::retarget(
            self.move_x_animation.take(),
            from_x_offset,
            self.clock.clone(),
            config,
        );
    }

    pub fn animate_move_y_from(&mut self, from_y_offset: f64) {
        self.animate_move_y_from_with_config(
            from_y_offset,
            self.options.animations.window_movement.0,
        );
    }

    pub fn animate_move_y_from_with_config(
        &mut self,
        from_y_offset: f64,
        config: clarion_config::Animation,
    ) {
        self.move_y_animation = MoveAnimation::retarget(
            self.move_y_animation.take(),
            from_y_offset,
            self.clock.clone(),
            config,
        );
    }

    pub fn offset_move_anim_current(&mut self, offset: f64) {
        if let Some(move_) = self.move_x_animation.as_mut() {
            move_.offset_current(offset);
        }
    }

    pub fn set_anim_y_between_workspaces(&mut self) {
        if let Some(anim) = &mut self.move_y_animation {
            anim.is_between_workspaces = true;
        }
    }

    /// Returns whether this column is currently fullscreen.
    ///
    /// As in, if it contains one currently-fullscreen tile, or in tabbed mode, if it contains at
    /// least one currently-fullscreen tile.
    ///
    /// This will lag behind is_pending_fullscreen, depending on when the tiles actually respond to
    /// the un/fullscreen request. But, it's possible for is_fullscreen() to flip instantly, for
    /// example when consuming a fullscreen tile into a non-pending-fullscreen column.
    ///
    /// This controls things like:
    ///
    /// - whether the column draws at the top of the screen or at the start of the working area
    /// - whether the column draws above the top layer-shell layer
    /// - whether the tab indicator is shown
    /// - restoring view_offset_before_fullscreen
    ///
    /// Edge cases to watch out for:
    ///
    /// - Consuming a fullscreen tile into a non-tabbed column will keep that tile fullscreen until
    ///   it responds to the unfullscreen request. This tile may be anywhere in the column,
    ///   including at the active position.
    ///
    /// - Changing a fullscreen tabbed column into normal mode is an easy way to get randomly
    ///   delayed unfullscreening tiles in a normal column.
    ///
    /// - is_fullscreen() can suddenly change when consuming/expelling a fullscreen tile into/from a
    ///   non-fullscreen column. This can influence the code that saves/restores the unfullscreen
    ///   view offset.
    fn sizing_mode(&self) -> SizingMode {
        // Behaviors that we want:
        //
        // 1. The common case: single tile in a column. Assume no animations. Fullscreening the tile
        //    should make it jump to the top-left of the screen only when the tile finishes
        //    fullscreening. Similarly, unfullscreening should keep it at the top-left until the
        //    tile had unfullscreened.
        //
        // 2. Unfullscreening a tabbed column with multiple tiles should restore the view offset
        //    correctly. This means waiting for *all* tiles to unfullscreen, because otherwise the
        //    restored view offset will immediately get overwritten by the still screen-wide column
        //    (it uses the largest tile's width).
        //
        // 3. Changing a fullscreen tabbed column to normal should probably also restore the view
        //    offset correctly. Same problem as above, but now for normal columns (since display
        //    mode change applies instantly).
        //
        // The logic that satisfies these behaviors is to check if *any* tile is fullscreen.
        let mut any_fullscreen = false;
        let mut any_maximized = false;
        for tile in &self.tiles {
            match tile.sizing_mode() {
                SizingMode::Normal => (),
                SizingMode::Maximized => any_maximized = true,
                SizingMode::Fullscreen => any_fullscreen = true,
            }
        }

        if any_fullscreen {
            SizingMode::Fullscreen
        } else if any_maximized {
            SizingMode::Maximized
        } else {
            SizingMode::Normal
        }
    }

    pub fn contains(&self, window: &W::Id) -> bool {
        self.tiles
            .iter()
            .map(Tile::window)
            .any(|win| win.id() == window)
    }

    pub fn position(&self, window: &W::Id) -> Option<usize> {
        self.tiles
            .iter()
            .map(Tile::window)
            .position(|win| win.id() == window)
    }

    fn activate_idx(&mut self, idx: usize) -> bool {
        if !self.policy.activate(idx) {
            return false;
        }

        self.tiles[idx].ensure_alpha_animates_to_1();

        true
    }

    fn activate_window(&mut self, window: &W::Id) {
        let idx = self.position(window).unwrap();
        self.activate_idx(idx);
    }

    fn add_tile_at(&mut self, idx: usize, mut tile: Tile<W>) {
        tile.update_config(self.view_size, self.scale, self.options.clone());

        // Inserting a tile pushes down all tiles below it, but also in always-centering mode it
        // will affect the X position of all tiles in the column.
        let mut prev_offsets = Vec::with_capacity(self.tiles.len() + 1);
        prev_offsets.extend(self.tile_offsets().take(self.tiles.len()));

        if self.policy.display_mode != ColumnDisplay::Tabbed {
            self.policy.is_pending_fullscreen = false;
            self.policy.is_pending_maximized = false;
        }

        self.data.insert(idx, TileData::new(&tile));
        assert!(self.policy.insert(idx, tile.window().core_id()));
        self.tiles.insert(idx, tile);
        // This insertion owns movement compensation below, where old positions
        // still correspond to window identities. Letting the Dwindle arrangement
        // compensate too would add the same geometry change twice.
        self.update_tile_sizes_with_transaction(true, false, Transaction::new());

        // Animate tiles according to the offset changes.
        prev_offsets.insert(idx, Point::default());
        for (i, ((tile, offset), prev)) in zip(self.tiles_mut(), prev_offsets).enumerate() {
            if i == idx {
                continue;
            }

            tile.animate_move_from(prev - offset);
        }
    }

    fn update_window(&mut self, window: &W::Id) {
        let (tile_idx, tile) = self
            .tiles
            .iter_mut()
            .enumerate()
            .find(|(_, tile)| tile.window().id() == window)
            .unwrap();

        let prev_height = self.data[tile_idx].size.h;

        tile.update_window();
        self.data[tile_idx].update(tile);

        let offset = prev_height - self.data[tile_idx].size.h;

        // Move windows below in tandem with resizing.
        // Dwindle positions already follow the arranged cells. A late client
        // height change does not change those offsets, so compensating it like
        // a vertical stack would move the following panes a second time.
        //
        // FIXME: in always-centering mode, window resizing will affect the offsets of all other
        // windows in the column, so they should all be animated. How should this interact with
        // animated vs. non-animated resizes? For example, an animated +20 resize followed by two
        // non-animated -10 resizes.
        if self.policy.display_mode == ColumnDisplay::Normal && offset != 0. {
            if tile.resize_animation().is_some() {
                // If there's a resize animation (that may have just started in
                // tile.update_window()), then the apparent size change is smooth with no sudden
                // jumps. This corresponds to adding an Y animation to tiles below.
                for tile in &mut self.tiles[tile_idx + 1..] {
                    tile.animate_move_y_from_with_config(
                        offset,
                        self.options.animations.window_resize.anim,
                    );
                }
            } else {
                // There's no resize animation, but the offset is nonzero. This could happen for
                // example:
                // - if the window resized on its own, which we don't animate
                // - if the window resized by less than 10 px (the resize threshold)
                //
                // The latter case could also cancel an ongoing resize animation.
                //
                // Now, stationary tiles below shouldn't react to this offset change in any way,
                // i.e. their apparent Y position should jump together with the resize. However,
                // tiles below that are already animating an Y movement should offset their
                // animations to avoid the jump.
                //
                // Notably, this is necessary to fix the animation jump when resizing height back
                // and forth in quick succession (in a way that cancels the resize animation).
                for tile in &mut self.tiles[tile_idx + 1..] {
                    tile.offset_move_y_anim_current(offset);
                }
            }
        }
    }

    /// Extra size taken up by elements in the column such as the tab indicator.
    fn extra_size(&self) -> Size<f64, Logical> {
        if self.policy.display_mode == ColumnDisplay::Tabbed {
            self.tab_indicator.extra_size(self.tiles.len(), self.scale)
        } else {
            Size::from((0., 0.))
        }
    }

    fn resolve_preset_width(&self, preset: PresetSize) -> ResolvedSize {
        let extra = self.extra_size();
        resolve_preset_size(preset, &self.options, self.working_area.size.w, extra.w)
    }

    fn resolve_preset_height(&self, preset: PresetSize) -> ResolvedSize {
        let extra = self.extra_size();
        resolve_preset_size(preset, &self.options, self.working_area.size.h, extra.h)
    }

    fn update_tile_sizes(&mut self, animate: bool) {
        self.update_tile_sizes_with_transaction(animate, animate, Transaction::new());
    }

    fn update_tile_sizes_with_transaction(
        &mut self,
        animate: bool,
        animate_movement: bool,
        transaction: Transaction,
    ) {
        let sizing_mode = self.pending_sizing_mode();
        if matches!(sizing_mode, SizingMode::Fullscreen | SizingMode::Maximized) {
            let bounds = if sizing_mode == SizingMode::Fullscreen {
                clarion_core::Rect::new(0., 0., self.view_size.w, self.view_size.h)
            } else {
                clarion_core::Rect::new(
                    self.parent_area.loc.x,
                    self.parent_area.loc.y,
                    self.parent_area.size.w,
                    self.parent_area.size.h,
                )
            };
            let active = self.policy.active_index();
            let tabbed = self.policy.display_mode == ColumnDisplay::Tabbed;
            let mut tiles = self.tiles.iter_mut().enumerate();
            self.policy
                .arrange(ColumnArrangement::Expanded(bounds), |id, rect| {
                    let (tile_idx, tile) = tiles.next().unwrap();
                    debug_assert_eq!(*id, tile.window().core_id());
                    // In tabbed mode, only the visible window participates in the transaction.
                    let is_active = tile_idx == active;
                    let transaction = if tabbed && !is_active {
                        None
                    } else {
                        Some(transaction.clone())
                    };

                    if matches!(sizing_mode, SizingMode::Fullscreen) {
                        tile.request_fullscreen(animate, transaction);
                    } else {
                        tile.request_maximized(
                            (rect.width, rect.height).into(),
                            animate,
                            transaction,
                        );
                    }
                });
            return;
        }

        let is_tabbed = self.policy.display_mode == ColumnDisplay::Tabbed;

        let min_size: Vec<_> = self
            .tiles
            .iter()
            .map(Tile::min_size_nonfullscreen)
            .map(|mut size| {
                size.w = size.w.max(1.);
                size.h = size.h.max(1.);
                size
            })
            .collect();
        let max_size: Vec<_> = self
            .tiles
            .iter()
            .map(Tile::max_size_nonfullscreen)
            .collect();

        let working_size = self.working_area.size;
        let extra_size = self.extra_size();
        let width = self.policy.arranged_width(
            working_size.w,
            self.options.layout.gaps,
            extra_size.w,
            zip(&min_size, &max_size).map(|(minimum, maximum)| (minimum.w, maximum.w)),
        );
        let max_tile_height = working_size.h - self.options.layout.gaps * 2. - extra_size.h;
        let arrangement = ColumnArrangement::Normal {
            width,
            working_height: working_size.h,
            gap: self.options.layout.gaps,
            tabbed_limit: max_tile_height,
        };

        if self.policy.display_mode == ColumnDisplay::Dwindle {
            // Geometry snapshots are shared by render iterators. Update the
            // retained buffer at dirty edges; COW is needed only if a prior
            // iterator still owns its immutable snapshot.
            let old_len = self.dwindle_offsets.len();
            let offsets = Rc::make_mut(&mut self.dwindle_offsets);
            offsets.resize(self.tiles.len(), Point::default());
            let mut tiles = self.tiles.iter_mut().enumerate();
            self.policy.arrange(arrangement, |id, rect| {
                let (tile_idx, tile) = tiles.next().unwrap();
                debug_assert_eq!(*id, tile.window().core_id());
                let position = Point::from((rect.x, rect.y));
                if animate_movement && tile_idx < old_len {
                    let delta = offsets[tile_idx] - position;
                    if delta != Point::from((0., 0.)) {
                        tile.animate_move_from(delta);
                    }
                }
                offsets[tile_idx] = position;

                let transaction = Some(transaction.clone());
                // Empty cells are valid clipping geometry on exhausted outputs,
                // but Wayland size zero means "client chooses", not one pixel.
                tile.request_tile_size_with_geometry_animation(
                    (rect.width.max(1.), rect.height.max(1.)).into(),
                    animate,
                    transaction,
                );
            });
            return;
        }

        let max_non_auto_window_height = self
            .policy
            .nonautomatic_height_cap(
                max_tile_height,
                self.options.layout.gaps,
                min_size.iter().map(|size| size.h),
            )
            .map(|(index, height_left)| {
                f64::max(
                    1.,
                    self.tiles[index]
                        .window_height_for_tile_height(height_left)
                        .round(),
                )
            });

        // Compute the tile heights. Start by converting window heights to tile heights.
        let mut constraints = std::mem::take(&mut self.policy.height_constraints);
        constraints.clear();
        let heights = zip(&self.tiles, self.policy.rows()).map(|(tile, row)| match row.height {
            auto @ WindowHeight::Auto { .. } => auto,
            WindowHeight::Fixed(height) => {
                let mut window_height = height.round().max(1.);
                if let Some(max) = max_non_auto_window_height {
                    window_height = f64::min(window_height, max);
                } else {
                    // In any case, clamp to the working area height.
                    let max = tile.window_height_for_tile_height(max_tile_height).round();
                    window_height = f64::min(window_height, max);
                }

                WindowHeight::Fixed(tile.tile_height_for_window_height(window_height))
            }
            WindowHeight::Preset(idx) => {
                let preset = self.options.layout.preset_window_heights[idx];
                let window_height = match self.resolve_preset_height(preset) {
                    ResolvedSize::Tile(h) => tile.window_height_for_tile_height(h),
                    ResolvedSize::Window(h) => h,
                };

                let mut window_height = window_height.round().clamp(1., 100000.);
                if let Some(max) = max_non_auto_window_height {
                    window_height = f64::min(window_height, max);
                }

                let tile_height = tile.tile_height_for_window_height(window_height);
                WindowHeight::Fixed(tile_height)
            }
        });

        // Pure policy resolves height allocation; the adapter has already resolved
        // presets and decoration sizes, and applies protocol requests below.
        constraints.extend(zip(zip(&self.tiles, heights), zip(min_size, max_size)).map(
            |((tile, height), (minimum, maximum))| {
                let height = match height {
                    WindowHeight::Auto { weight } => {
                        clarion_core::column::Height::Automatic { weight }
                    }
                    WindowHeight::Fixed(height) => clarion_core::column::Height::Fixed(height),
                    WindowHeight::Preset(_) => unreachable!(),
                };
                clarion_core::column::HeightConstraint {
                    height,
                    minimum: minimum.h,
                    maximum: maximum.h,
                    decoration: tile.tile_height_for_window_height(0.),
                }
            },
        ));
        self.policy.height_constraints = constraints;
        let active = self.policy.active_index();
        let mut tiles = self.tiles.iter_mut().enumerate();
        self.policy.arrange(arrangement, |id, rect| {
            let (tile_idx, tile) = tiles.next().unwrap();
            debug_assert_eq!(*id, tile.window().core_id());
            let size = Size::from((rect.width, rect.height));

            // In tabbed mode, only the visible window participates in the transaction.
            let is_active = tile_idx == active;
            let transaction = if is_tabbed && !is_active {
                None
            } else {
                Some(transaction.clone())
            };

            tile.request_tile_size(size, animate, transaction);
        });
    }

    fn width(&self) -> f64 {
        let mut tiles_width = self
            .data
            .iter()
            .map(|data| NotNan::new(data.size.w).unwrap())
            .max()
            .map(NotNan::into_inner)
            .unwrap();

        if self.policy.display_mode == ColumnDisplay::Tabbed && self.sizing_mode().is_normal() {
            let extra_size = self.tab_indicator.extra_size(self.tiles.len(), self.scale);
            tiles_width += extra_size.w;
        }

        tiles_width
    }

    fn focus_index(&mut self, index: u8) {
        if let Some(index) = self.policy.one_based_index(usize::from(index)) {
            self.activate_idx(index);
        }
    }

    fn focus_up(&mut self) -> bool {
        if self.policy.display_mode == ColumnDisplay::Dwindle {
            return self.focus_dwindle_vertical(false);
        }
        self.policy
            .adjacent(false)
            .is_some_and(|index| self.activate_idx(index))
    }

    fn focus_down(&mut self) -> bool {
        if self.policy.display_mode == ColumnDisplay::Dwindle {
            return self.focus_dwindle_vertical(true);
        }
        self.policy
            .adjacent(true)
            .is_some_and(|index| self.activate_idx(index))
    }

    /// Nearest tile whose center lies in the requested 2D direction.
    ///
    /// `vertical` selects Y vs X. `positive` is down/right; `false` is up/left.
    fn dwindle_neighbor_idx(&self, vertical: bool, positive: bool) -> Option<usize> {
        let index = self.policy.active_index();
        let active = self.tile_offsets().nth(index)?;
        let size = self.data.get(index)?.size;
        let active = clarion_core::Rect::new(active.x, active.y, size.w, size.h);
        self.policy
            .directional_neighbor(active, self.policy_geometry(), vertical, positive)
    }

    fn policy_geometry(&self) -> impl Iterator<Item = clarion_core::Rect> + '_ {
        zip(self.tile_offsets(), &self.data).map(|(offset, data)| {
            clarion_core::Rect::new(offset.x, offset.y, data.size.w, data.size.h)
        })
    }

    /// 2D directional focus navigation between tiles of a dwindle column.
    ///
    /// Finds the nearest tile whose center lies strictly above (down=false) or below (down=true)
    /// the active tile's center.
    fn focus_dwindle_vertical(&mut self, down: bool) -> bool {
        match self.dwindle_neighbor_idx(true, down) {
            Some(idx) => self.activate_idx(idx),
            None => false,
        }
    }

    /// 2D directional focus navigation horizontally between tiles of a dwindle column.
    ///
    /// Finds the nearest tile whose center lies strictly to the left (left=true) or right
    /// (left=false) of the active tile's center.
    pub fn focus_dwindle_horizontal(&mut self, left: bool) -> bool {
        match self.dwindle_neighbor_idx(false, !left) {
            Some(idx) => self.activate_idx(idx),
            None => false,
        }
    }

    /// Select the leftmost tile within a dwindle column.
    pub fn activate_leftmost_dwindle_tile(&mut self) {
        if let Some(index) = self
            .policy
            .horizontal_extreme(self.policy_geometry(), false)
        {
            self.activate_idx(index);
        }
    }

    /// Select the rightmost tile within a dwindle column.
    pub fn activate_rightmost_dwindle_tile(&mut self) {
        if let Some(index) = self.policy.horizontal_extreme(self.policy_geometry(), true) {
            self.activate_idx(index);
        }
    }

    fn focus_top(&mut self) {
        self.activate_idx(0);
    }

    fn focus_bottom(&mut self) {
        self.activate_idx(self.tiles.len() - 1);
    }

    fn move_dwindle(&mut self, vertical: bool, positive: bool) -> bool {
        let Some(new_idx) = self.dwindle_neighbor_idx(vertical, positive) else {
            return false;
        };
        let active = self.policy.active_index();
        if active == new_idx {
            return false;
        }

        self.policy.swap(active, new_idx, true);
        self.tiles.swap(active, new_idx);
        self.data.swap(active, new_idx);
        if active < self.dwindle_offsets.len() && new_idx < self.dwindle_offsets.len() {
            Rc::make_mut(&mut self.dwindle_offsets).swap(active, new_idx);
        }
        self.update_tile_sizes(true);
        true
    }

    fn move_up(&mut self) -> bool {
        if self.policy.display_mode == ColumnDisplay::Dwindle {
            return self.move_dwindle(true, false);
        }

        let Some(new_idx) = self.policy.adjacent(false) else {
            return false;
        };

        let mut ys = self.tile_offsets().skip(self.policy.active_index());
        let active_y = ys.next().unwrap().y;
        let next_y = ys.next().unwrap().y;
        drop(ys);

        let previous = self.policy.active_index();
        self.policy.swap(previous, new_idx, true);
        self.tiles.swap(previous, new_idx);
        self.data.swap(previous, new_idx);

        // Animate the movement.
        let new_active_y = self.tile_offset(new_idx).y;
        self.tiles[new_idx].animate_move_y_from(active_y - new_active_y);
        self.tiles[new_idx + 1].animate_move_y_from(active_y - next_y);

        true
    }

    fn move_down(&mut self) -> bool {
        if self.policy.display_mode == ColumnDisplay::Dwindle {
            return self.move_dwindle(true, true);
        }

        let Some(new_idx) = self.policy.adjacent(true) else {
            return false;
        };

        let mut ys = self.tile_offsets().skip(self.policy.active_index());
        let active_y = ys.next().unwrap().y;
        let next_y = ys.next().unwrap().y;
        drop(ys);

        let previous = self.policy.active_index();
        self.policy.swap(previous, new_idx, true);
        self.tiles.swap(previous, new_idx);
        self.data.swap(previous, new_idx);

        // Animate the movement.
        let new_active_y = self.tile_offset(new_idx).y;
        self.tiles[new_idx].animate_move_y_from(active_y - new_active_y);
        self.tiles[new_idx - 1].animate_move_y_from(next_y - active_y);

        true
    }

    fn toggle_width(&mut self, tile_idx: Option<usize>, forwards: bool) {
        use clarion_core::scrolling::PresetExtent;
        let tile_idx = tile_idx.unwrap_or(self.policy.active_index());
        let tile = &self.tiles[tile_idx];
        let presets: Vec<_> = self
            .options
            .layout
            .preset_column_widths
            .iter()
            .map(|preset| match self.resolve_preset_width(*preset) {
                ResolvedSize::Tile(value) => PresetExtent::Tile(value),
                ResolvedSize::Window(value) => PresetExtent::Window(value),
            })
            .collect();
        let Some(preset_idx) = self.policy.next_width_preset(
            forwards,
            &presets,
            tile.window_expected_or_current_size().w,
            tile.tile_expected_or_current_size().w,
        ) else {
            return;
        };
        let preset = self.options.layout.preset_column_widths[preset_idx];
        self.set_column_width(SizeChange::from(preset), Some(tile_idx), true);
        self.policy.select_width_preset(preset_idx);
    }

    fn toggle_full_width(&mut self) {
        self.policy.toggle_full_width();
        self.update_tile_sizes(true);
    }

    fn set_column_width(&mut self, change: SizeChange, tile_idx: Option<usize>, animate: bool) {
        let tile = &self.tiles[tile_idx.unwrap_or(self.policy.active_index())];
        let context = WidthContext {
            working_width: self.working_area.size.w,
            gap: self.options.layout.gaps,
            extra_width: self.extra_size().w,
            decoration: tile.tile_width_for_window_width(0.),
        };
        self.policy.set_width(dimension_change(change), context);
        self.update_tile_sizes(animate);
    }

    fn set_window_height(&mut self, change: SizeChange, tile_idx: Option<usize>, animate: bool) {
        let tile_idx = tile_idx.unwrap_or(self.policy.active_index());
        // Converting an automatic row must preserve the apparent heights of the
        // other rows. Fixed/preset rows keep their existing relative weights.
        if matches!(
            self.policy.rows()[tile_idx].height,
            WindowHeight::Auto { .. }
        ) {
            self.convert_heights_to_auto();
        }
        let tile = &self.tiles[tile_idx];
        let other_minimum_tile_height = self
            .tiles
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != tile_idx)
            .map(|(_, tile)| {
                f64::max(1., tile.min_size_nonfullscreen().h) + self.options.layout.gaps
            })
            .sum();
        let context = HeightContext {
            working_height: self.working_area.size.h,
            gap: self.options.layout.gaps,
            extra_height: self.extra_size().h,
            decoration: tile.tile_height_for_window_height(0.),
            current_window_height: tile.window_size().h,
            other_minimum_tile_height,
            minimum_window_height: f64::from(tile.window().min_size().h),
            maximum_window_height: f64::from(tile.window().max_size().h),
        };
        self.policy
            .set_height(tile_idx, dimension_change(change), context);
        self.update_tile_sizes(animate);
    }

    fn reset_window_height(&mut self, tile_idx: Option<usize>) {
        self.policy
            .reset_height(tile_idx.unwrap_or(self.policy.active_index()));
        self.update_tile_sizes(true);
    }

    fn toggle_window_height(&mut self, tile_idx: Option<usize>, forwards: bool) {
        let tile_idx = tile_idx.unwrap_or(self.policy.active_index());
        if matches!(
            self.policy.rows()[tile_idx].height,
            WindowHeight::Auto { .. }
        ) {
            self.convert_heights_to_auto();
        }
        let tile = &self.tiles[tile_idx];
        let presets: Vec<_> = self
            .options
            .layout
            .preset_window_heights
            .iter()
            .map(|preset| match self.resolve_preset_height(*preset) {
                ResolvedSize::Tile(value) => value,
                ResolvedSize::Window(value) => tile.tile_height_for_window_height(value),
            })
            .collect();
        let Some(preset) = self.policy.next_height_preset(
            tile_idx,
            forwards,
            &presets,
            self.data[tile_idx].size.h,
        ) else {
            return;
        };
        self.policy.select_height_preset(tile_idx, preset);
        self.update_tile_sizes(true);
    }

    /// Converts all heights in the column to automatic, preserving the apparent heights.
    ///
    /// All weights are recomputed to preserve the current tile heights while "centering" the
    /// weights at the median window height (it gets weight = 1).
    ///
    /// One case where apparent heights will not be preserved is when the column is taller than the
    /// working area.
    fn convert_heights_to_auto(&mut self) {
        let heights: Vec<_> = self.tiles.iter().map(|tile| tile.tile_size().h).collect();

        self.policy.automatic_heights(&heights);
    }

    fn set_fullscreen(&mut self, is_fullscreen: bool) {
        if self.policy.set_fullscreen(is_fullscreen) {
            self.update_tile_sizes(true);
        }
    }

    fn set_maximized(&mut self, maximize: bool) {
        if self.policy.set_maximized(maximize) {
            self.update_tile_sizes(true);
        }
    }

    fn set_column_display(&mut self, display: ColumnDisplay) {
        if self.policy.display_mode == display {
            return;
        }

        // Capture exact previous 2D positions of all tiles using SmallVec on the stack (zero heap
        // allocation).
        let prev_positions: smallvec::SmallVec<[Point<f64, Logical>; 8]> =
            self.tile_offsets().collect();

        // Animate the opacity when entering/leaving tabbed display mode.
        for (idx, tile) in self.tiles.iter_mut().enumerate() {
            let is_active = idx == self.policy.active_index();
            if !is_active {
                let (from, to) = if display == ColumnDisplay::Tabbed {
                    (1., 0.)
                } else {
                    (0., 1.)
                };
                tile.animate_alpha(from, to, self.options.animations.window_movement.0);
            }
        }

        // Animate the appearance of the tab indicator.
        if display == ColumnDisplay::Tabbed {
            self.tab_indicator.start_open_animation(
                self.clock.clone(),
                self.options.animations.window_movement.0,
            );
        }

        // Now switch the display mode for real and recompute target geometries.
        self.policy.set_display(display);
        self.update_tile_sizes(true);

        // Animate each tile smoothly from its previous 2D position to its new target position.
        let new_positions: smallvec::SmallVec<[Point<f64, Logical>; 8]> =
            self.tile_offsets().collect();

        for (tile, (prev, new)) in zip(&mut self.tiles, zip(prev_positions, new_positions)) {
            let delta = prev - new;
            if delta != Point::from((0., 0.)) {
                tile.animate_move_from(delta);
            }
        }
    }

    fn tiles_origin(&self) -> Point<f64, Logical> {
        let mut origin = Point::from((0., 0.));

        match self.sizing_mode() {
            SizingMode::Normal => (),
            SizingMode::Maximized => {
                origin.y += self.parent_area.loc.y;
                return origin;
            }
            SizingMode::Fullscreen => return origin,
        }

        origin.y += self.working_area.loc.y + self.options.layout.gaps;

        if self.policy.display_mode == ColumnDisplay::Tabbed {
            origin += self
                .tab_indicator
                .content_offset(self.tiles.len(), self.scale);
        }

        origin
    }

    // HACK: pass a self.data iterator in manually as a workaround for the lack of method partial
    // borrowing. Note that this method's return value does not borrow the entire &Self!
    fn tile_offsets_iter(
        &self,
        data: impl Iterator<Item = TileData>,
    ) -> impl Iterator<Item = Point<f64, Logical>> {
        // FIXME: this should take into account always-center-single-column, which means that
        // Column should somehow know when it is being centered due to being the single column on
        // the workspace or some other reason.
        let center = self.options.layout.center_focused_column == CenterFocusedColumn::Always;
        let gaps = self.options.layout.gaps;
        let tabbed = self.policy.display_mode == ColumnDisplay::Tabbed;
        let dwindle = self.policy.display_mode == ColumnDisplay::Dwindle;
        let dwindle_offsets = dwindle.then(|| self.dwindle_offsets.clone());
        let mut dwindle_idx = 0;

        // Does not include extra size from the tab indicator.
        let tiles_width = self
            .data
            .iter()
            .map(|data| NotNan::new(data.size.w).unwrap())
            .max()
            .map(NotNan::into_inner)
            .unwrap_or(0.);

        let mut origin = self.tiles_origin();

        // Chain with a dummy value to be able to get one past all tiles' Y.
        let dummy = TileData {
            size: Size::default(),
            interactively_resizing_by_left_edge: false,
        };
        let data = data.chain(iter::once(dummy));

        data.map(move |data| {
            if dwindle {
                let off = dwindle_offsets
                    .as_ref()
                    .and_then(|offsets| offsets.get(dwindle_idx))
                    .copied()
                    .unwrap_or_default();
                dwindle_idx += 1;
                return origin + off;
            }

            let mut pos = origin;

            if center {
                pos.x += (tiles_width - data.size.w) / 2.;
            } else if data.interactively_resizing_by_left_edge {
                pos.x += tiles_width - data.size.w;
            }

            if !tabbed {
                origin.y += data.size.h + gaps;
            }

            pos
        })
    }

    fn tile_offsets(&self) -> impl Iterator<Item = Point<f64, Logical>> + '_ {
        self.tile_offsets_iter(self.data.iter().copied())
    }

    fn tile_offset(&self, tile_idx: usize) -> Point<f64, Logical> {
        self.tile_offsets().nth(tile_idx).unwrap()
    }

    fn tile_offsets_in_render_order(
        &self,
        data: impl Iterator<Item = TileData>,
    ) -> impl Iterator<Item = Point<f64, Logical>> {
        let active_idx = self.policy.active_index();
        let active_pos = self.tile_offset(active_idx);
        let offsets = self
            .tile_offsets_iter(data)
            .enumerate()
            .filter_map(move |(idx, pos)| (idx != active_idx).then_some(pos));
        iter::once(active_pos).chain(offsets)
    }

    pub fn tiles(&self) -> impl Iterator<Item = (&Tile<W>, Point<f64, Logical>)> + '_ {
        let offsets = self.tile_offsets_iter(self.data.iter().copied());
        zip(&self.tiles, offsets)
    }

    fn tiles_mut(&mut self) -> impl Iterator<Item = (&mut Tile<W>, Point<f64, Logical>)> + '_ {
        let offsets = self.tile_offsets_iter(self.data.iter().copied());
        zip(&mut self.tiles, offsets)
    }

    fn tiles_in_render_order(
        &self,
    ) -> impl Iterator<Item = (&Tile<W>, Point<f64, Logical>, bool)> + '_ {
        let offsets = self.tile_offsets_in_render_order(self.data.iter().copied());

        let (first, rest) = self.tiles.split_at(self.policy.active_index());
        let (active, rest) = rest.split_at(1);

        let active = active.iter().map(|tile| (tile, true));

        let rest_visible = self.policy.display_mode != ColumnDisplay::Tabbed;
        let rest = first.iter().chain(rest);
        let rest = rest.map(move |tile| (tile, rest_visible));

        let tiles = active.chain(rest);
        zip(tiles, offsets).map(|((tile, visible), pos)| (tile, pos, visible))
    }

    fn tiles_in_render_order_mut(
        &mut self,
    ) -> impl Iterator<Item = (&mut Tile<W>, Point<f64, Logical>)> + '_ {
        let offsets = self.tile_offsets_in_render_order(self.data.iter().copied());

        let (first, rest) = self.tiles.split_at_mut(self.policy.active_index());
        let (active, rest) = rest.split_at_mut(1);

        let tiles = active.iter_mut().chain(first).chain(rest);
        zip(tiles, offsets)
    }

    fn tab_indicator_area(&self) -> Rectangle<f64, Logical> {
        // We'd like to use the active tile's animated size for the tab indicator, however we need
        // to be mindful of the case where the active tile is smaller than some other tile in the
        // column. The column assumes the size of the largest tile.
        //
        // We expect users to mainly resize tabbed columns by width, so matching the animated size
        // is more important here. Besides, we always try to resize all windows in a column to the
        // same width when possible, and also the animation for going into tabbed mode doesn't move
        // tiles horizontally as much.
        //
        // For height though, it's a different story. First, users probably aren't resizing a
        // tabbed column by height. Second, we don't match windows by height, so it's easy to have
        // a smaller active tile than the rest of the column, e.g. by adding a fixed-size dialog.
        // Then, switching to that dialog and back should ideally keep the tab indicator position
        // fixed. Third, the animation for making a column tabbed moves tiles vertically, and using
        // the active tile's animated size in this case only works for the topmost tile, and looks
        // broken otherwise.
        let mut max_height = 0.;
        for tile in &self.tiles {
            max_height = f64::max(max_height, tile.tile_size().h);
        }

        let tile = &self.tiles[self.policy.active_index()];
        let area_size = Size::from((tile.animated_tile_size().w, max_height));

        Rectangle::new(self.tiles_origin(), area_size)
    }

    pub fn start_open_animation(&mut self, id: &W::Id) -> bool {
        for tile in &mut self.tiles {
            if tile.window().id() == id {
                tile.start_open_animation();

                // Animate the appearance of the tab indicator.
                if self.policy.display_mode == ColumnDisplay::Tabbed
                    && self.sizing_mode().is_normal()
                    && self.tiles.len() == 1
                    && !self.tab_indicator.config().hide_when_single_tab
                {
                    self.tab_indicator.start_open_animation(
                        self.clock.clone(),
                        self.options.animations.window_open.anim,
                    );
                }

                return true;
            }
        }

        false
    }

    #[cfg(test)]
    fn verify_invariants(&self) {
        assert!(!self.tiles.is_empty(), "columns can't be empty");
        assert!(self.policy.active_index() < self.tiles.len());
        assert_eq!(self.tiles.len(), self.data.len());

        if !self.pending_sizing_mode().is_normal() {
            assert!(self.tiles.len() == 1 || self.policy.display_mode == ColumnDisplay::Tabbed);
        }

        if let Some(idx) = self.policy.preset_width_idx {
            assert!(idx < self.options.layout.preset_column_widths.len());
        }

        let is_tabbed = self.policy.display_mode == ColumnDisplay::Tabbed;
        let is_dwindle = self.policy.display_mode == ColumnDisplay::Dwindle;

        let tile_count = self.tiles.len();
        if tile_count == 1 {
            if let WindowHeight::Auto { weight } = self.policy.rows()[0].height {
                assert_eq!(
                    weight, 1.,
                    "auto height weight must reset to 1 for a single window"
                );
            }
        }

        let working_size = self.working_area.size;
        let extra_size = self.extra_size();
        let gaps = self.options.layout.gaps;

        let mut found_fixed = false;
        let mut total_height = 0.;
        let mut total_min_height = 0.;
        assert_eq!(self.tiles.len(), self.policy.rows().len());
        for ((tile, data), row) in zip(zip(&self.tiles, &self.data), self.policy.rows()) {
            assert_eq!(tile.window().core_id(), *row.id());
            assert!(Rc::ptr_eq(&self.options, &tile.presentation.options));
            assert_eq!(self.clock, tile.presentation.clock);
            assert_eq!(self.scale, tile.scale());
            assert_eq!(
                self.pending_sizing_mode(),
                tile.window().pending_sizing_mode()
            );
            assert_eq!(self.view_size, tile.view_size());
            tile.verify_invariants();

            let mut data2 = *data;
            data2.update(tile);
            assert_eq!(data, &data2, "tile data must be up to date");

            if matches!(row.height, WindowHeight::Fixed(_)) {
                assert!(
                    !found_fixed,
                    "there can only be one fixed-height window in a column"
                );
                found_fixed = true;
            }

            if let WindowHeight::Preset(idx) = row.height {
                assert!(self.options.layout.preset_window_heights.len() > idx);
            }

            let requested_size = tile.window().requested_size().unwrap();
            let requested_tile_height =
                tile.tile_height_for_window_height(f64::from(requested_size.h));
            let min_tile_height = f64::max(1., tile.min_size_nonfullscreen().h);

            if !is_tabbed
                && self.pending_sizing_mode().is_normal()
                && self.scale.round() == self.scale
                && working_size.h.round() == working_size.h
                && gaps.round() == gaps
            {
                let total_height = requested_tile_height + gaps * 2. + extra_size.h;
                let total_min_height = min_tile_height + gaps * 2. + extra_size.h;
                let max_height = f64::max(total_min_height, working_size.h);
                assert!(
                    total_height <= max_height,
                    "each tile in a column mustn't go beyond working area height \
                     (tile height {total_height} > max height {max_height})"
                );
            }

            total_height += requested_tile_height;
            total_min_height += min_tile_height;
        }

        if !is_tabbed
            && !is_dwindle
            && tile_count > 1
            && self.scale.round() == self.scale
            && working_size.h.round() == working_size.h
            && gaps.round() == gaps
        {
            total_height += gaps * (tile_count + 1) as f64 + extra_size.h;
            total_min_height += gaps * (tile_count + 1) as f64 + extra_size.h;
            let max_height = f64::max(total_min_height, working_size.h);
            assert!(
                total_height <= max_height,
                "multiple tiles in a column mustn't go beyond working area height \
                 (total height {total_height} > max height {max_height})"
            );
        }
    }
}

fn core_column_sizing(mode: SizingMode) -> clarion_core::viewport::ColumnSizing {
    use clarion_core::viewport::ColumnSizing;
    if mode.is_fullscreen() {
        ColumnSizing::Fullscreen
    } else if mode.is_maximized() {
        ColumnSizing::Maximized
    } else {
        ColumnSizing::Normal
    }
}

fn compute_working_area(
    parent_area: Rectangle<f64, Logical>,
    scale: f64,
    struts: Struts,
) -> Rectangle<f64, Logical> {
    let mut working_area = parent_area;

    // Add struts.
    working_area.size.w = f64::max(0., working_area.size.w - struts.left.0 - struts.right.0);
    working_area.loc.x += struts.left.0;

    working_area.size.h = f64::max(0., working_area.size.h - struts.top.0 - struts.bottom.0);
    working_area.loc.y += struts.top.0;

    // Round location to start at a physical pixel.
    let loc = working_area
        .loc
        .to_physical_precise_ceil(scale)
        .to_logical(scale);

    let mut size_diff = (loc - working_area.loc).to_size();
    size_diff.w = f64::min(working_area.size.w, size_diff.w);
    size_diff.h = f64::min(working_area.size.h, size_diff.h);

    working_area.size -= size_diff;
    working_area.loc = loc;

    working_area
}

fn compute_toplevel_bounds(
    border_config: clarion_config::Border,
    working_area_size: Size<f64, Logical>,
    extra_size: Size<f64, Logical>,
    gaps: f64,
) -> Size<i32, Logical> {
    let mut border = 0.;
    if !border_config.off {
        border = border_config.width * 2.;
    }

    Size::from((
        f64::max(working_area_size.w - gaps * 2. - extra_size.w - border, 1.),
        f64::max(working_area_size.h - gaps * 2. - extra_size.h - border, 1.),
    ))
    .to_i32_floor()
}

fn cancel_resize_for_column<W: LayoutElement>(
    interactive_resize: &mut Option<clarion_core::resize::Resize>,
    column: &mut Column<W>,
) {
    if let Some(resize) = interactive_resize {
        if column.policy.position(&resize.id).is_some() {
            *interactive_resize = None;
        }
    }

    for tile in &mut column.tiles {
        tile.window_mut().cancel_interactive_resize();
    }
}

fn resolve_preset_size(
    preset: PresetSize,
    options: &Options,
    view_size: f64,
    extra_size: f64,
) -> ResolvedSize {
    match preset {
        PresetSize::Proportion(proportion) => ResolvedSize::Tile(
            (view_size - options.layout.gaps) * proportion - options.layout.gaps - extra_size,
        ),
        PresetSize::Fixed(width) => ResolvedSize::Window(f64::from(width)),
    }
}

impl<W: LayoutElement> Column<W> {
    pub(super) fn session_sizing(&self, row: usize) -> clarion_ipc::management::SessionSizing {
        use clarion_ipc::management::{SessionHeight, SessionSizing};
        let (width, proportional) = match self.policy.width {
            ColumnWidth::Fixed(w) => (w, false),
            ColumnWidth::Proportion(w) => (w, true),
        };
        let height = match self.policy.rows()[row].height {
            WindowHeight::Auto { weight } => SessionHeight::Auto(weight),
            WindowHeight::Fixed(h) => SessionHeight::Fixed(h),
            WindowHeight::Preset(i) => SessionHeight::Preset(i),
        };
        SessionSizing {
            width,
            proportional,
            full_width: self.policy.is_full_width,
            height,
        }
    }
}
impl<W: LayoutElement> ScrollingSpace<W> {
    pub(super) fn restore_session_sizing(
        &mut self,
        sizing: &[(W::Id, clarion_ipc::management::SessionSizing)],
    ) {
        use clarion_ipc::management::SessionHeight;
        for col in &mut self.columns {
            let mut updated = false;
            for (index, tile) in col.tiles.iter().enumerate() {
                if let Some((_, saved)) = sizing.iter().find(|(id, _)| id == tile.window().id()) {
                    if !updated {
                        col.policy.width = if saved.proportional {
                            ColumnWidth::Proportion(saved.width)
                        } else {
                            ColumnWidth::Fixed(saved.width)
                        };
                        col.policy.is_full_width = saved.full_width;
                    }
                    col.policy.rows_mut()[index].height = match saved.height {
                        SessionHeight::Auto(weight) => WindowHeight::Auto { weight },
                        SessionHeight::Fixed(h) => WindowHeight::Fixed(h),
                        SessionHeight::Preset(i)
                            if i < self.options.layout.preset_window_heights.len() =>
                        {
                            WindowHeight::Preset(i)
                        }
                        SessionHeight::Preset(_) => WindowHeight::auto_1(),
                    };
                    updated = true;
                }
            }
            if updated {
                // A saved column may now have only one live window. Match the normal
                // removal path so future additions start with the default auto weight.
                if col.data.len() == 1 {
                    if let WindowHeight::Auto { weight } = &mut col.policy.rows_mut()[0].height {
                        *weight = 1.;
                    }
                }
                col.update_tile_sizes(false);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use clarion_config::animations::{Curve, EasingParams, Kind, WindowMovementAnim};
    use clarion_config::FloatOrInt;

    use super::*;
    use crate::utils::round_logical_in_physical;

    // Keep the existing platform coordinate fixtures, using the sole pure
    // production geometry implementation instead of a legacy solver copy.
    fn compute_dwindle_geometries(
        area: Rectangle<f64, Logical>,
        count: usize,
        gap: f64,
    ) -> Vec<Rectangle<f64, Logical>> {
        let mut result = Vec::new();
        clarion_core::geometry::spiral_rects_into(
            clarion_core::Rect::new(area.loc.x, area.loc.y, area.size.w, area.size.h),
            count,
            gap,
            &mut result,
        );
        result
            .into_iter()
            .map(|rect| Rectangle::new((rect.x, rect.y).into(), (rect.width, rect.height).into()))
            .collect()
    }

    #[test]
    fn column_reordering_preserves_position_and_velocity_across_rates() {
        for rate in [0.25, 0.73, 1., 2.] {
            for hz in [60., 120., 144., 240., 360.] {
                for direction in [-1., 1.] {
                    let mut clock = Clock::with_time(Duration::ZERO);
                    clock.set_rate(rate);
                    let config = WindowMovementAnim::default().0;
                    let movement =
                        MoveAnimation::retarget(None, direction * 400., clock.clone(), config)
                            .unwrap();
                    let at = Duration::from_secs_f64(5. / hz);
                    clock.set_unadjusted(at);
                    let origin = 500.;
                    let before = origin + movement.from * movement.anim.value();
                    let velocity = movement.from * movement.anim.velocity();
                    assert!(velocity.abs() > 1.);

                    // move_column_to() passes old origin minus new origin;
                    // the visible position and speed must survive that change.
                    let offset = direction * -600.;
                    let origin = origin - offset;
                    let movement =
                        MoveAnimation::retarget(Some(movement), offset, clock.clone(), config)
                            .unwrap();
                    let after = origin + movement.from * movement.anim.value();
                    assert!((after - before).abs() < 1e-10);
                    assert!((movement.from * movement.anim.velocity() - velocity).abs() < 1e-10);
                    let delta = Duration::from_nanos(100);
                    clock.set_unadjusted(at + delta);
                    let next = origin + movement.from * movement.anim.value();
                    clock.set_unadjusted(at + delta * 2);
                    let later = origin + movement.from * movement.anim.value();
                    // Remove the finite sample's acceleration term. At 2x speed
                    // a sharp reversal can legitimately change velocity by 0.1
                    // px/s even during this 100 ns sample.
                    let slope =
                        (4. * (next - after) - (later - after)) / (2. * delta.as_secs_f64());
                    assert!(
                        (slope - velocity).abs() < 0.05,
                        "rate={rate}, hz={hz}, direction={direction}, velocity={velocity}, sampled={slope}"
                    );
                    clock.set_unadjusted(Duration::from_secs(5));
                    assert!(movement.anim.is_done());
                    assert_eq!(movement.from * movement.anim.value(), 0.);
                }
            }
        }
    }

    #[test]
    fn column_zero_offset_keeps_existing_curve_and_idle_axis() {
        let mut clock = Clock::with_time(Duration::ZERO);
        let config = clarion_config::Animation {
            off: false,
            kind: Kind::Easing(EasingParams {
                duration_ms: 400,
                curve: Curve::EaseOutCubic,
            }),
        };
        assert!(MoveAnimation::retarget(None, 0., clock.clone(), config).is_none());
        let mut movement = MoveAnimation::retarget(None, -700., clock.clone(), config).unwrap();
        movement.is_between_workspaces = true;
        clock.set_unadjusted(Duration::from_millis(300));
        let start = movement.anim.start_time();
        let end = movement.anim.end_time();
        let before = movement.from * movement.anim.value();
        let velocity = movement.from * movement.anim.velocity();
        let movement = MoveAnimation::retarget(Some(movement), 0., clock.clone(), config).unwrap();
        assert_eq!(movement.anim.start_time(), start);
        assert_eq!(movement.anim.end_time(), end);
        assert_eq!(movement.from * movement.anim.value(), before);
        assert_eq!(movement.from * movement.anim.velocity(), velocity);
        assert!(movement.is_between_workspaces);
        clock.set_unadjusted(Duration::from_millis(400));
        assert!(movement.anim.is_done());
    }

    #[test]
    fn column_zero_remaining_distance_keeps_momentum_and_workspace_flag() {
        let mut clock = Clock::with_time(Duration::ZERO);
        let config = WindowMovementAnim::default().0;
        let mut movement = MoveAnimation::retarget(None, 800., clock.clone(), config).unwrap();
        movement.is_between_workspaces = true;
        clock.set_unadjusted(Duration::from_millis(60));
        let current = movement.from * movement.anim.value();
        let velocity = movement.from * movement.anim.velocity();
        let movement =
            MoveAnimation::retarget(Some(movement), -current, clock.clone(), config).unwrap();
        assert_eq!(movement.from * movement.anim.value(), 0.);
        assert!((movement.from * movement.anim.velocity() - velocity).abs() < 1e-10);
        assert!(movement.is_between_workspaces);
        clock.set_unadjusted(Duration::from_millis(61));
        assert!(movement.from * movement.anim.value() < 0.);
        clock.set_unadjusted(Duration::from_secs(3));
        assert!(movement.anim.is_done());
    }

    #[test]
    fn column_retarget_uses_requested_easing_and_disabled_config() {
        let mut clock = Clock::with_time(Duration::ZERO);
        let initial = WindowMovementAnim::default().0;
        let movement = MoveAnimation::retarget(None, -800., clock.clone(), initial).unwrap();
        clock.set_unadjusted(Duration::from_millis(60));
        let current = movement.from * movement.anim.value();
        let velocity = movement.from * movement.anim.velocity();
        let linear = clarion_config::Animation {
            off: false,
            kind: Kind::Easing(EasingParams {
                duration_ms: 600,
                curve: Curve::Linear,
            }),
        };
        let movement =
            MoveAnimation::retarget(Some(movement), 100., clock.clone(), linear).unwrap();
        assert_eq!(movement.anim.duration(), Duration::from_millis(600));
        assert_eq!(movement.from * movement.anim.value(), current + 100.);
        assert!((movement.from * movement.anim.velocity() - velocity).abs() < 1e-10);
        clock.set_unadjusted(Duration::from_millis(360));
        // The requested duration remains, while a live easing interruption now
        // uses the core's C1 Hermite continuation. At half-time its independent
        // endpoint/velocity equation is p0/2 + v0 * duration/8 (p1 = v1 = 0).
        let expected = (current + 100.) / 2. + velocity * 0.6 / 8.;
        assert!((movement.from * movement.anim.value() - expected).abs() < 1e-10);
        let disabled = clarion_config::Animation {
            off: true,
            ..linear
        };
        let movement =
            MoveAnimation::retarget(Some(movement), -200., clock.clone(), disabled).unwrap();
        assert!(movement.anim.is_done());
        clock.set_unadjusted(Duration::from_millis(361));
        assert_eq!(movement.from * movement.anim.value(), 0.);
        assert_eq!(movement.anim.velocity(), 0.);
    }

    #[test]
    fn column_resize_compensation_handles_negative_progress() {
        let mut clock = Clock::with_time(Duration::ZERO);
        let mut movement =
            MoveAnimation::retarget(None, -600., clock.clone(), WindowMovementAnim::default().0)
                .unwrap();
        clock.set_unadjusted(Duration::from_millis(60));
        assert!(movement.anim.value() < -0.001);
        let before = movement.from * movement.anim.value();
        let end = movement.anim.end_time();
        movement.offset_current(24.);
        assert!((movement.from * movement.anim.value() - (before + 24.)).abs() < 1e-10);
        assert_eq!(movement.anim.end_time(), end);
    }

    #[test]
    fn working_area_starts_at_physical_pixel() {
        let struts = Struts {
            left: FloatOrInt(0.5),
            right: FloatOrInt(1.),
            top: FloatOrInt(0.75),
            bottom: FloatOrInt(1.),
        };

        let parent_area = Rectangle::from_size(Size::from((1280., 720.)));
        let area = compute_working_area(parent_area, 1., struts);

        assert_eq!(round_logical_in_physical(1., area.loc.x), area.loc.x);
        assert_eq!(round_logical_in_physical(1., area.loc.y), area.loc.y);
    }

    #[test]
    fn large_fractional_strut() {
        let struts = Struts {
            left: FloatOrInt(0.),
            right: FloatOrInt(0.),
            top: FloatOrInt(50000.5),
            bottom: FloatOrInt(0.),
        };

        let parent_area = Rectangle::from_size(Size::from((1280., 720.)));
        compute_working_area(parent_area, 1., struts);
    }

    #[test]
    fn dwindle_geometries_single_tile_fills_area() {
        let area = Rectangle::new(Point::from((100., 50.)), Size::from((800., 600.)));
        let geoms = compute_dwindle_geometries(area, 1, 10.);
        assert_eq!(geoms.len(), 1);
        assert_eq!(geoms[0], area);
    }

    #[test]
    fn dwindle_geometries_two_tiles_split_long_axis() {
        // Wide area: splits left/right.
        let area = Rectangle::new(Point::from((0., 0.)), Size::from((1920., 600.)));
        let geoms = compute_dwindle_geometries(area, 2, 20.);
        assert_eq!(geoms.len(), 2);
        assert_eq!(geoms[0].size.w, 950.);
        assert_eq!(geoms[1].loc.x, 970.);
        assert_eq!(geoms[0].size.h, 600.);

        // Tall area: splits top/bottom.
        let area = Rectangle::new(Point::from((0., 0.)), Size::from((600., 1920.)));
        let geoms = compute_dwindle_geometries(area, 2, 20.);
        assert_eq!(geoms[0].size.h, 950.);
        assert_eq!(geoms[1].loc.y, 970.);
        assert_eq!(geoms[0].size.w, 600.);
    }

    #[test]
    fn dwindle_geometries_three_tiles_spiral() {
        let area = Rectangle::new(Point::from((0., 0.)), Size::from((1920., 1080.)));
        let gap = 20.;
        let geoms = compute_dwindle_geometries(area, 3, gap);
        assert_eq!(geoms.len(), 3);

        // Spiral: first window keeps half the area, the other two split the second half.
        assert_eq!(
            geoms[0],
            Rectangle::new(Point::from((0., 0.)), Size::from((950., 1080.)))
        );
        assert_eq!(
            geoms[1],
            Rectangle::new(Point::from((970., 0.)), Size::from((950., 530.)))
        );
        assert_eq!(
            geoms[2],
            Rectangle::new(Point::from((970., 550.)), Size::from((950., 530.)))
        );

        // First window is strictly larger than the other two combined.
        let others: f64 = geoms[1..].iter().map(|g| g.size.w * g.size.h).sum();
        assert!(geoms[0].size.w * geoms[0].size.h > others);

        // Total area = input minus the gap strips lost to each of the two splits
        // (20 x 1080 for the first split, 20 x 950 for the second).
        let total: f64 = geoms.iter().map(|g| g.size.w * g.size.h).sum();
        let expected = 1920. * 1080. - gap * 1080. - gap * 950.;
        assert!((total - expected).abs() < 1., "total {total} vs {expected}");
    }

    #[test]
    fn dwindle_geometries_tiles_never_overlap() {
        let area = Rectangle::new(Point::from((0., 0.)), Size::from((1000., 700.)));
        for count in 1..=8 {
            let geoms = compute_dwindle_geometries(area, count, 10.);
            assert_eq!(geoms.len(), count);
            for (i, a) in geoms.iter().enumerate() {
                for b in &geoms[i + 1..] {
                    let overlap_x = a.loc.x < b.loc.x + b.size.w && b.loc.x < a.loc.x + a.size.w;
                    let overlap_y = a.loc.y < b.loc.y + b.size.h && b.loc.y < a.loc.y + a.size.h;
                    assert!(
                        !(overlap_x && overlap_y),
                        "tiles {i} and {} overlap at count={count}",
                        geoms.iter().position(|g| g == b).unwrap()
                    );
                }
            }
        }
    }
}
