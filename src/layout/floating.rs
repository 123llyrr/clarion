use std::iter::zip;
use std::rc::Rc;

use clarion_config::utils::MergeWith as _;
use clarion_config::{PresetSize, RelativeTo};
use clarion_core::floating::{Direction, FloatingPolicy, Placement, PlacementChange};
use clarion_core::layout::LiveLayoutPolicy;
use clarion_ipc::{PositionChange, SizeChange, WindowLayout};
use smithay::backend::renderer::element::Id;
use smithay::utils::{Logical, Point, Rectangle, Scale, Serial, Size};

use super::scrolling::ColumnWidth;
use super::tile::{Tile, TileRenderElement};
use super::wobble::WobbleAnchor;
use super::{
    ConfigureIntent, InteractiveResizeData, LayoutElement, Options, RemovedTile, SizeFrac,
};
use crate::animation::{Animation, Clock};
use crate::clarion_render_elements;
use crate::layout::RenderLayer;
use crate::render_helpers::renderer::{AnyRenderer, ClarionRenderer};
use crate::render_helpers::xray::XrayPos;
use crate::render_helpers::RenderCtx;
use crate::scene::closing_window::{ClosingWindow, ClosingWindowRenderElement};
use crate::scene::resources::SceneResourceAllocator;
use crate::utils::transaction::TransactionBlocker;
use crate::utils::ResizeEdge;
use crate::window::ResolvedWindowRules;

/// By how many logical pixels the directional move commands move floating windows.
pub const DIRECTIONAL_MOVE_PX: f64 = 50.;

/// Space for floating windows.
#[derive(Debug)]
pub struct FloatingSpace<W: LayoutElement> {
    scene_resources: SceneResourceAllocator,
    /// Tiles in top-to-bottom order.
    tiles: Vec<Tile<W>>,
    /// Includes fading snapshots, so destroying a background window cannot raise it.
    stacking: FloatingStack,
    /// Sole numeric camera owner; the adapter only converts coordinates/time.
    canvas: Option<clarion_core::canvas::CanvasPan>,

    /// Live stable-ID placement, stacking, focus and resize policy.
    policy: FloatingPolicy,

    /// Windows in the closing animation.
    pub(super) closing_windows: Vec<ClosingWindow>,

    /// View size for this space.
    view_size: Size<f64, Logical>,

    /// Working area for this space.
    working_area: Rectangle<f64, Logical>,

    /// Scale of the output the space is on (and rounds its sizes to).
    scale: f64,

    /// Clock for driving animations.
    clock: Clock,

    /// Configurable properties of the layout.
    options: Rc<Options>,
}

clarion_render_elements! {
    FloatingSpaceRenderElement<R> => {
        Tile = TileRenderElement<R>,
        ClosingWindow = ClosingWindowRenderElement,
    }
}

#[derive(Debug, PartialEq, Eq)]
enum FloatingStackEntry {
    Live(u64),
    Closing(Id),
}

#[derive(Debug, Default)]
struct FloatingStack {
    entries: Vec<FloatingStackEntry>,
}

impl FloatingStack {
    fn place_live(&mut self, id: u64, next_live: Option<u64>, top: bool) {
        self.remove_live(id);
        let index = if top {
            0
        } else {
            next_live
                .and_then(|next| {
                    self.entries
                        .iter()
                        .position(|entry| *entry == FloatingStackEntry::Live(next))
                })
                .unwrap_or(self.entries.len())
        };
        self.entries.insert(index, FloatingStackEntry::Live(id));
    }

    fn remove_live(&mut self, id: u64) {
        self.entries
            .retain(|entry| *entry != FloatingStackEntry::Live(id));
    }

    fn close(&mut self, source: u64, snapshot: Id) {
        let index = self
            .entries
            .iter()
            .position(|entry| *entry == FloatingStackEntry::Live(source))
            .unwrap_or(0);
        self.entries
            .insert(index, FloatingStackEntry::Closing(snapshot));
    }

    fn retain_closing(&mut self, closing: &[ClosingWindow]) {
        self.entries.retain(|entry| match entry {
            FloatingStackEntry::Live(_) => true,
            FloatingStackEntry::Closing(id) => {
                closing.iter().any(|window| window.stable_id() == id)
            }
        });
    }
}

enum FloatingRenderItem<'a, W: LayoutElement> {
    Tile(&'a Tile<W>, Point<f64, Logical>),
    Closing(&'a ClosingWindow),
}

/// This bridge projects numeric placement into the platform's logical types.
/// Target placement and relative/unbounded behavior belong to the pure policy.
trait PlacementBridge {
    fn from_tile<W: LayoutElement>(
        area: Rectangle<f64, Logical>,
        tile: &Tile<W>,
        position: Point<f64, Logical>,
        unbounded: bool,
    ) -> Self;
    fn logical_pos(&self) -> Point<f64, Logical>;
    fn relative_pos(&self) -> Point<f64, SizeFrac>;
    fn update_from_tile<W: LayoutElement>(&mut self, tile: &Tile<W>);
}

fn core_area(area: Rectangle<f64, Logical>) -> clarion_core::Rect {
    clarion_core::Rect::new(area.loc.x, area.loc.y, area.size.w, area.size.h)
}

impl PlacementBridge for Placement {
    fn from_tile<W: LayoutElement>(
        area: Rectangle<f64, Logical>,
        tile: &Tile<W>,
        position: Point<f64, Logical>,
        unbounded: bool,
    ) -> Self {
        let size = tile.tile_size();
        Self::new(
            core_area(area),
            clarion_core::Size::new(size.w, size.h),
            [position.x, position.y],
            unbounded,
        )
    }
    fn logical_pos(&self) -> Point<f64, Logical> {
        (self.rectangle.x, self.rectangle.y).into()
    }
    fn relative_pos(&self) -> Point<f64, SizeFrac> {
        (self.relative[0], self.relative[1]).into()
    }
    fn update_from_tile<W: LayoutElement>(&mut self, tile: &Tile<W>) {
        let size = tile.tile_size();
        self.set_size(clarion_core::Size::new(size.w, size.h));
    }
}

impl<W: LayoutElement> FloatingSpace<W> {
    pub fn new(
        view_size: Size<f64, Logical>,
        working_area: Rectangle<f64, Logical>,
        scale: f64,
        clock: Clock,
        options: Rc<Options>,
        scene_resources: SceneResourceAllocator,
    ) -> Self {
        Self {
            scene_resources,
            tiles: Vec::new(),
            stacking: FloatingStack::default(),
            canvas: None,
            policy: FloatingPolicy::new(),
            closing_windows: Vec::new(),
            view_size,
            working_area,
            scale,
            clock,
            options,
        }
    }

    pub fn canvas_enabled(&self) -> bool {
        self.canvas.is_some()
    }

    pub fn set_canvas_enabled(&mut self, enabled: bool) {
        if enabled == self.canvas_enabled() {
            return;
        }
        if enabled {
            self.canvas = Some(Default::default());
        } else {
            self.reset_canvas();
            self.canvas = None;
        }
        self.policy.set_unbounded(enabled);
    }

    fn translate_canvas(&mut self, delta: Point<f64, Logical>) {
        self.policy.pan([delta.x, delta.y]);
    }

    pub fn pan_canvas(&mut self, delta: Point<f64, Logical>, kinetic: bool) {
        if let Some(pan) = &mut self.canvas {
            let applied = pan.pan(
                [delta.x, delta.y],
                self.clock.now(),
                kinetic && !self.options.animations.off && !self.clock.should_complete_instantly(),
            );
            self.translate_canvas((applied[0], applied[1]).into());
        }
    }

    pub fn stop_canvas_pan(&mut self) {
        if let Some(pan) = &mut self.canvas {
            pan.stop();
        }
    }

    pub fn reset_canvas(&mut self) {
        if let Some(pan) = &mut self.canvas {
            let delta = pan.reset();
            self.translate_canvas((delta[0], delta[1]).into());
        }
    }

    pub fn update_config(
        &mut self,
        view_size: Size<f64, Logical>,
        working_area: Rectangle<f64, Logical>,
        scale: f64,
        options: Rc<Options>,
    ) {
        if self.options.layout.preset_column_widths != options.layout.preset_column_widths {
            self.policy.clear_presets(clarion_core::Axis::Horizontal);
        }
        if self.options.layout.preset_window_heights != options.layout.preset_window_heights {
            self.policy.clear_presets(clarion_core::Axis::Vertical);
        }
        for (tile, data) in zip(&mut self.tiles, &mut self.policy) {
            tile.update_config(view_size, scale, options.clone());
            data.update_from_tile(tile);
        }
        // Arrangement updates the retained authoritative placements. Rendering
        // and Workspace geometry commits consume those same targets directly.
        self.policy.arrange(core_area(working_area), |_, _| {});

        self.view_size = view_size;
        self.working_area = working_area;
        self.scale = scale;
        self.options = options;
    }

    pub fn update_shaders(&mut self) {
        for tile in &mut self.tiles {
            tile.update_shaders();
        }
    }

    /// Return whether the accepted camera tick changed placement targets.
    pub fn advance_animations(&mut self) -> bool {
        let delta = self
            .canvas
            .as_mut()
            .map(|pan| pan.tick(self.clock.now()))
            .unwrap_or_default();
        self.translate_canvas((delta[0], delta[1]).into());
        for tile in &mut self.tiles {
            tile.advance_animations();
        }

        self.closing_windows.retain_mut(|closing| {
            closing.advance_animations();
            closing.are_animations_ongoing()
        });
        self.stacking.retain_closing(&self.closing_windows);
        delta != [0.; 2]
    }

    pub fn are_animations_ongoing(&self) -> bool {
        self.canvas
            .as_ref()
            .is_some_and(clarion_core::canvas::CanvasPan::ongoing)
            || self.tiles.iter().any(Tile::are_animations_ongoing)
            || !self.closing_windows.is_empty()
    }

    pub fn are_transitions_ongoing(&self) -> bool {
        self.canvas
            .as_ref()
            .is_some_and(clarion_core::canvas::CanvasPan::ongoing)
            || self.tiles.iter().any(Tile::are_transitions_ongoing)
            || !self.closing_windows.is_empty()
    }

    pub fn has_closing_windows(&self) -> bool {
        !self.closing_windows.is_empty()
    }

    pub fn update_render_elements(
        &mut self,
        is_active: bool,
        view_rect: Rectangle<f64, Logical>,
        layer: RenderLayer,
    ) {
        let active = self.active_window_id().cloned();
        let camera_forecast = self.canvas_render_delta();
        for (tile, offset) in self.tiles_with_offsets_mut() {
            // Skip tiles belonging to a different render layer.
            if layer.is_normal() == tile.is_moving_between_workspaces() {
                continue;
            }

            let id = tile.window().id();
            let is_active = is_active && Some(id) == active.as_ref();

            let mut tile_view_rect = view_rect;
            tile_view_rect.loc -= offset + camera_forecast + tile.render_offset();
            tile.update_render_elements(is_active, tile_view_rect);
        }
    }

    pub fn tiles(&self) -> impl Iterator<Item = &Tile<W>> + '_ {
        self.tiles.iter()
    }

    pub fn tiles_mut(&mut self) -> impl Iterator<Item = &mut Tile<W>> + '_ {
        self.tiles.iter_mut()
    }

    pub fn tiles_with_offsets(&self) -> impl Iterator<Item = (&Tile<W>, Point<f64, Logical>)> + '_ {
        let offsets = self.policy.placements().map(|d| d.logical_pos());
        zip(&self.tiles, offsets)
    }

    pub fn tiles_with_offsets_mut(
        &mut self,
    ) -> impl Iterator<Item = (&mut Tile<W>, Point<f64, Logical>)> + '_ {
        let offsets = self.policy.placements().map(|d| d.logical_pos());
        zip(&mut self.tiles, offsets)
    }

    pub fn tiles_with_render_positions(
        &self,
    ) -> impl Iterator<Item = (&Tile<W>, Point<f64, Logical>)> {
        let scale = self.scale;
        let camera_forecast = self.canvas_render_delta();
        self.tiles_with_offsets().map(move |(tile, offset)| {
            let pos = offset + camera_forecast + tile.render_offset();
            // Round to physical pixels.
            let pos = pos.to_physical_precise_round(scale).to_logical(scale);
            (tile, pos)
        })
    }

    pub fn tiles_with_render_positions_mut(
        &mut self,
        round: bool,
    ) -> impl Iterator<Item = (&mut Tile<W>, Point<f64, Logical>)> {
        let scale = self.scale;
        let camera_forecast = self.canvas_render_delta();
        self.tiles_with_offsets_mut().map(move |(tile, offset)| {
            let mut pos = offset + camera_forecast + tile.render_offset();
            // Round to physical pixels.
            if round {
                pos = pos.to_physical_precise_round(scale).to_logical(scale);
            }
            (tile, pos)
        })
    }

    pub fn tiles_with_ipc_layouts(&self) -> impl Iterator<Item = (&Tile<W>, WindowLayout)> {
        let scale = self.scale;
        self.tiles_with_offsets().map(move |(tile, offset)| {
            // Do not include animated render offset here to avoid IPC spam.
            let pos = offset;
            // Round to physical pixels.
            let pos = pos.to_physical_precise_round(scale).to_logical(scale);

            let layout = WindowLayout {
                tile_pos_in_workspace_view: Some(pos.into()),
                ..tile.ipc_layout_template()
            };
            (tile, layout)
        })
    }

    fn canvas_render_delta(&self) -> Point<f64, Logical> {
        let delta = self
            .canvas
            .as_ref()
            .map(|pan| pan.forecast_delta(self.clock.now()))
            .unwrap_or_default();
        // Camera displacement is opposite scene displacement, matching the
        // accepted FloatingPolicy::pan tick. Predicting the other sign makes
        // the scene jump back across the forecast at the next owner tick.
        (-delta[0], -delta[1]).into()
    }

    pub fn new_window_toplevel_bounds(&self, rules: &ResolvedWindowRules) -> Size<i32, Logical> {
        let border_config = self.options.layout.border.merged_with(&rules.border);
        compute_toplevel_bounds(border_config, self.working_area.size)
    }

    /// Returns the geometry of the active window relative to and clamped to the working area.
    ///
    /// During animations, assumes the final tile position.
    pub fn active_window_visual_rectangle(&self) -> Option<Rectangle<f64, Logical>> {
        let active_id = self.active_window_id()?;
        let (tile, offset) = self
            .tiles_with_offsets()
            .find(|(tile, _)| tile.window().id() == active_id)?;

        let window_pos = offset + tile.window_loc();
        let window_size = tile.window_size();
        let window_rect = Rectangle::new(window_pos, window_size);

        self.working_area.intersection(window_rect)
    }

    pub fn popup_target_rect(&self, id: &W::Id) -> Option<Rectangle<f64, Logical>> {
        for (tile, pos) in self.tiles_with_offsets() {
            if tile.window().id() == id {
                // Position within the working area.
                let mut target = self.working_area;
                target.loc -= pos;
                target.loc -= tile.window_loc();

                return Some(target);
            }
        }
        None
    }

    fn active_window_id(&self) -> Option<&W::Id> {
        let id = self.policy.active()?;
        self.tiles
            .iter()
            .find(|tile| tile.window().core_id() == id)
            .map(|tile| tile.window().id())
    }

    fn physical_id(&self, core_id: u64) -> Option<W::Id> {
        self.tiles
            .iter()
            .find(|tile| tile.window().core_id() == core_id)
            .map(|tile| tile.window().id().clone())
    }

    fn idx_of(&self, id: &W::Id) -> Option<usize> {
        let id = self
            .tiles
            .iter()
            .find(|tile| tile.window().id() == id)?
            .window()
            .core_id();
        self.policy.index(id)
    }

    #[cfg(test)]
    fn contains(&self, id: &W::Id) -> bool {
        self.idx_of(id).is_some()
    }

    pub fn active_window(&self) -> Option<&W> {
        let id = self.policy.active()?;
        self.tiles
            .iter()
            .find(|tile| tile.window().core_id() == id)
            .map(Tile::window)
    }

    pub fn active_window_mut(&mut self) -> Option<&mut W> {
        let id = self.policy.active()?;
        self.tiles
            .iter_mut()
            .find(|tile| tile.window().core_id() == id)
            .map(Tile::window_mut)
    }

    pub fn has_window(&self, id: &W::Id) -> bool {
        self.idx_of(id).is_some()
    }

    pub fn is_empty(&self) -> bool {
        self.policy.is_empty()
    }

    pub fn add_tile(&mut self, tile: Tile<W>, activate: bool) {
        self.add_tile_at(0, tile, activate);
    }

    fn add_tile_at(&mut self, mut idx: usize, mut tile: Tile<W>, activate: bool) {
        tile.cancel_requested_resize();
        tile.update_config(self.view_size, self.scale, self.options.clone());

        // Restore the previous floating window size, and in case the tile is fullscreen,
        // unfullscreen it.
        let floating_size = tile.floating_window_size.map(|size| [size.w, size.h]);
        let win = tile.window_mut();
        let expected = win.expected_size().map(|size| [size.w, size.h]);
        let minimum = win.min_size();
        let maximum = win.max_size();
        let size = clarion_core::floating::restored_client_size(
            floating_size,
            !win.pending_sizing_mode().is_normal(),
            expected,
            [minimum.w, minimum.h],
            [maximum.w, maximum.h],
        );
        win.request_size_once((size[0], size[1]).into(), true);

        let ancestors: Vec<_> = self
            .tiles
            .iter()
            .filter(|parent| win.is_child_of(parent.window()))
            .map(|parent| parent.window().core_id())
            .collect();
        let core_id = win.core_id();
        let pos = self.stored_or_default_tile_pos(&tile).unwrap_or_else(|| {
            let size = tile.tile_size();
            let position = clarion_core::floating::centered_position(
                core_area(self.working_area),
                clarion_core::Size::new(size.w, size.h),
            );
            (position[0], position[1]).into()
        });
        let placement = Placement::from_tile(self.working_area, &tile, pos, self.canvas.is_some());
        idx = self
            .policy
            .insert(core_id, placement, idx, activate, &ancestors)
            .expect("floating identities are unique");
        let presets = [
            tile.floating_preset_width_idx.take(),
            tile.floating_preset_height_idx.take(),
        ];
        self.policy.restore_presets(core_id, presets);
        self.tiles.insert(idx, tile);
        self.place_stack_tile(idx);

        self.bring_up_descendants_of(idx);
    }

    pub fn add_tile_above(&mut self, above: &W::Id, mut tile: Tile<W>, activate: bool) {
        let idx = self.idx_of(above).unwrap();
        let size = tile.tile_size();
        let position = self
            .policy
            .position_above(
                self.tiles[idx].window().core_id(),
                clarion_core::Size::new(size.w, size.h),
            )
            .unwrap();
        tile.floating_pos = Some(self.logical_to_size_frac((position[0], position[1]).into()));
        self.add_tile_at(idx, tile, activate);
    }

    fn bring_up_descendants_of(&mut self, idx: usize) {
        // Protocol relationships are translated once at this mutation boundary.
        // The independent policy chooses and orders the family raises.
        for tile in &self.tiles {
            let id = tile.window().core_id();
            let parents = self
                .tiles
                .iter()
                .filter(|parent| tile.window().is_child_of(parent.window()))
                .map(|parent| parent.window().core_id())
                .collect();
            self.policy.set_parents(id, parents);
        }
        let id = self.tiles[idx].window().core_id();
        for change in self.policy.raise_descendants(id) {
            let tile = self.tiles.remove(change.from);
            self.tiles.insert(change.to, tile);
            self.place_stack_tile(change.to);
        }
    }

    pub fn remove_tile(&mut self, id: &W::Id) -> RemovedTile<W> {
        let idx = self.idx_of(id).unwrap();
        self.remove_tile_by_idx(idx)
    }

    fn remove_tile_by_idx(&mut self, idx: usize) -> RemovedTile<W> {
        let id = self.tiles[idx].window().core_id();
        let (index, member) = self.policy.remove(id).unwrap();
        let mut tile = self.tiles.remove(index);
        self.stacking.remove_live(id);
        tile.floating_preset_width_idx = member.presets[0];
        tile.floating_preset_height_idx = member.presets[1];
        if let Some(size) = tile.window().expected_size() {
            tile.floating_window_size = Some(size);
        }
        tile.floating_pos = Some(member.placement.relative_pos());
        let width = ColumnWidth::Fixed(tile.tile_expected_or_current_size().w);
        RemovedTile {
            tile,
            width,
            is_full_width: false,
            is_floating: true,
        }
    }

    pub fn start_close_animation_for_dragged_tile(
        &mut self,
        renderer: &mut AnyRenderer<'_>,
        tile: &Tile<W>,
        tile_pos: Point<f64, Logical>,
        blocker: TransactionBlocker,
    ) {
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
        match tile.create_closing_window(
            renderer,
            scale,
            tile_pos,
            blocker,
            anim,
            &self.options.animations.window_close,
            self.scene_resources.closing.clone(),
        ) {
            Ok(closing) => {
                self.stacking
                    .close(tile.window().core_id(), closing.stable_id().clone());
                self.closing_windows.push(closing);
            }
            Err(err) => {
                warn!("error creating a closing window animation: {err:?}");
            }
        }
    }

    pub fn start_close_animation_for_window(
        &mut self,
        renderer: &mut AnyRenderer<'_>,
        id: &W::Id,
        blocker: TransactionBlocker,
    ) {
        let closing_res = {
            let Some((tile, tile_pos)) = self
                .tiles_with_render_positions()
                .find(|(tile, _)| tile.window().id() == id)
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
                let source = self
                    .tiles()
                    .find(|tile| tile.window().id() == id)
                    .unwrap()
                    .window()
                    .core_id();
                self.stacking.close(source, closing.stable_id().clone());
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
        id: &W::Id,
        tile_pos: Point<f64, Logical>,
        blocker: TransactionBlocker,
    ) {
        let closing_res = {
            let Some(tile) = self.tiles().find(|tile| tile.window().id() == id) else {
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
                let source = self
                    .tiles()
                    .find(|tile| tile.window().id() == id)
                    .unwrap()
                    .window()
                    .core_id();
                self.stacking.close(source, closing.stable_id().clone());
                self.closing_windows.push(closing);
            }
            Err(err) => {
                warn!("error creating a closing window animation: {err:?}");
            }
        }
    }

    pub fn activate_window_without_raising(&mut self, id: &W::Id) -> bool {
        let Some(index) = self.idx_of(id) else {
            return false;
        };
        self.policy.focus(self.tiles[index].window().core_id())
    }

    pub fn activate_window(&mut self, id: &W::Id) -> bool {
        let Some(idx) = self.idx_of(id) else {
            return false;
        };

        if let Some(pan) = &mut self.canvas {
            pan.stop();
        }
        if let Some(delta) = self.policy.reveal_delta(self.tiles[idx].window().core_id()) {
            self.pan_canvas((delta[0], delta[1]).into(), false);
        }
        self.raise_window(idx, 0);
        self.policy
            .focus(self.tiles[self.idx_of(id).unwrap()].window().core_id());
        self.bring_up_descendants_of(0);

        true
    }

    fn raise_window(&mut self, from_idx: usize, to_idx: usize) {
        assert!(to_idx <= from_idx);
        let id = self.tiles[from_idx].window().core_id();
        let change = self.policy.raise(id, to_idx).unwrap();
        let tile = self.tiles.remove(change.from);
        self.tiles.insert(change.to, tile);
        self.place_stack_tile(change.to);
    }

    fn place_stack_tile(&mut self, index: usize) {
        let id = self.tiles[index].window().core_id();
        let next = self
            .tiles
            .get(index + 1)
            .map(|tile| tile.window().core_id());
        self.stacking.place_live(id, next, index == 0);
    }

    fn stacked_items(&self) -> impl Iterator<Item = FloatingRenderItem<'_, W>> {
        let mut tiles = self.tiles_with_render_positions();
        self.stacking
            .entries
            .iter()
            .filter_map(move |entry| match entry {
                FloatingStackEntry::Live(id) => {
                    let (tile, position) = tiles.next()?;
                    debug_assert_eq!(tile.window().core_id(), *id);
                    Some(FloatingRenderItem::Tile(tile, position))
                }
                FloatingStackEntry::Closing(id) => self
                    .closing_windows
                    .iter()
                    .find(|closing| closing.stable_id() == id)
                    .map(FloatingRenderItem::Closing),
            })
    }

    fn sizing_input(&self, index: usize) -> clarion_core::floating::SizingInput {
        let tile = &self.tiles[index];
        let window = tile.window();
        let current = window.expected_size().unwrap_or_else(|| window.size());
        let expected = window.expected_size().unwrap_or_default();
        let tile_size = tile.tile_expected_or_current_size();
        let minimum = window.min_size();
        let maximum = window.max_size();
        clarion_core::floating::SizingInput {
            available: clarion_core::Size::new(self.working_area.size.w, self.working_area.size.h),
            current_client: [current.w, current.h],
            expected_client: [expected.w, expected.h],
            current_tile: clarion_core::Size::new(tile_size.w, tile_size.h),
            decoration: clarion_core::Size::new(
                -tile.window_width_for_tile_width(0.),
                -tile.window_height_for_tile_height(0.),
            ),
            minimum: [minimum.w, minimum.h],
            maximum: [maximum.w, maximum.h],
        }
    }

    fn toggle_window_size(&mut self, id: Option<&W::Id>, axis: clarion_core::Axis, forwards: bool) {
        let Some(id) = id.or(self.active_window_id()).cloned() else {
            return;
        };
        let index = self.idx_of(&id).unwrap();
        let input = self.sizing_input(index);
        let core_id = self.tiles[index].window().core_id();
        let presets = match axis {
            clarion_core::Axis::Horizontal => &self.options.layout.preset_column_widths,
            clarion_core::Axis::Vertical => &self.options.layout.preset_window_heights,
        };
        let Some(size) = self.policy.cycle_preset(
            core_id,
            axis,
            forwards,
            presets.iter().copied().map(core_preset),
            input,
        ) else {
            return;
        };
        self.tiles[index]
            .window_mut()
            .request_size_once((size[0], size[1]).into(), true);
        self.interactive_resize_end(Some(&id));
    }

    fn set_window_size(
        &mut self,
        id: Option<&W::Id>,
        axis: clarion_core::Axis,
        change: SizeChange,
        animate: bool,
    ) {
        let Some(id) = id.or(self.active_window_id()) else {
            return;
        };
        let index = self.idx_of(id).unwrap();
        let input = self.sizing_input(index);
        let change = match change {
            SizeChange::SetFixed(value) => clarion_core::floating::SizeChange::SetFixed(value),
            SizeChange::SetProportion(value) => {
                clarion_core::floating::SizeChange::SetProportion(value)
            }
            SizeChange::AdjustFixed(value) => {
                clarion_core::floating::SizeChange::AdjustFixed(value)
            }
            SizeChange::AdjustProportion(value) => {
                clarion_core::floating::SizeChange::AdjustProportion(value)
            }
        };
        let size = self
            .policy
            .change_size(self.tiles[index].window().core_id(), axis, change, input)
            .unwrap();
        self.tiles[index]
            .window_mut()
            .request_size_once((size[0], size[1]).into(), animate);
    }

    pub fn toggle_window_width(&mut self, id: Option<&W::Id>, forwards: bool) {
        self.toggle_window_size(id, clarion_core::Axis::Horizontal, forwards);
    }

    pub fn start_open_animation(&mut self, id: &W::Id) -> bool {
        let Some(idx) = self.idx_of(id) else {
            return false;
        };

        self.tiles[idx].start_open_animation();
        true
    }

    pub fn toggle_window_height(&mut self, id: Option<&W::Id>, forwards: bool) {
        self.toggle_window_size(id, clarion_core::Axis::Vertical, forwards);
    }

    pub fn set_window_width(&mut self, id: Option<&W::Id>, change: SizeChange, animate: bool) {
        self.set_window_size(id, clarion_core::Axis::Horizontal, change, animate);
    }

    pub fn set_window_height(&mut self, id: Option<&W::Id>, change: SizeChange, animate: bool) {
        self.set_window_size(id, clarion_core::Axis::Vertical, change, animate);
    }

    fn focus_directional(&mut self, direction: Direction) -> bool {
        let Some(id) = self.policy.focus_directional(direction) else {
            return false;
        };
        let id = self.physical_id(id).unwrap();
        self.activate_window(&id)
    }

    pub fn focus_left(&mut self) -> bool {
        self.focus_directional(Direction::Left)
    }

    pub fn focus_right(&mut self) -> bool {
        self.focus_directional(Direction::Right)
    }

    pub fn focus_up(&mut self) -> bool {
        self.focus_directional(Direction::Up)
    }

    pub fn focus_down(&mut self) -> bool {
        self.focus_directional(Direction::Down)
    }

    pub fn focus_leftmost(&mut self) {
        if let Some(id) = self
            .policy
            .focus_extreme(clarion_core::Axis::Horizontal, false)
        {
            let id = self.physical_id(id).unwrap();
            self.activate_window(&id);
        }
    }

    pub fn focus_rightmost(&mut self) {
        if let Some(id) = self
            .policy
            .focus_extreme(clarion_core::Axis::Horizontal, true)
        {
            let id = self.physical_id(id).unwrap();
            self.activate_window(&id);
        }
    }

    pub fn focus_topmost(&mut self) {
        if let Some(id) = self
            .policy
            .focus_extreme(clarion_core::Axis::Vertical, false)
        {
            let id = self.physical_id(id).unwrap();
            self.activate_window(&id);
        }
    }

    pub fn focus_bottommost(&mut self) {
        if let Some(id) = self
            .policy
            .focus_extreme(clarion_core::Axis::Vertical, true)
        {
            let id = self.physical_id(id).unwrap();
            self.activate_window(&id);
        }
    }

    fn move_by(&mut self, amount: Point<f64, Logical>) {
        let Some(id) = self.policy.active() else {
            return;
        };
        let change = self.policy.move_by(id, [amount.x, amount.y]).unwrap();
        let index = self.policy.index(id).unwrap();
        self.apply_placement_change(index, change, true);
        self.interactive_resize_end(None);
    }

    pub fn move_left(&mut self) {
        self.move_by(Point::from((-DIRECTIONAL_MOVE_PX, 0.)));
    }

    pub fn move_right(&mut self) {
        self.move_by(Point::from((DIRECTIONAL_MOVE_PX, 0.)));
    }

    pub fn move_up(&mut self) {
        self.move_by(Point::from((0., -DIRECTIONAL_MOVE_PX)));
    }

    pub fn move_down(&mut self) {
        self.move_by(Point::from((0., DIRECTIONAL_MOVE_PX)));
    }

    pub fn move_window(
        &mut self,
        id: Option<&W::Id>,
        x: PositionChange,
        y: PositionChange,
        animate: bool,
    ) {
        let id = if let Some(id) = id {
            self.tiles[self.idx_of(id).unwrap()].window().core_id()
        } else {
            let Some(id) = self.policy.active() else {
                return;
            };
            id
        };
        let convert = |change| match change {
            PositionChange::SetFixed(value) => {
                clarion_core::floating::PositionChange::SetFixed(value)
            }
            PositionChange::SetProportion(value) => {
                clarion_core::floating::PositionChange::SetProportion(value)
            }
            PositionChange::AdjustFixed(value) => {
                clarion_core::floating::PositionChange::AdjustFixed(value)
            }
            PositionChange::AdjustProportion(value) => {
                clarion_core::floating::PositionChange::AdjustProportion(value)
            }
        };
        let change = self
            .policy
            .move_with_changes(id, convert(x), convert(y))
            .unwrap();
        let index = self.policy.index(id).unwrap();
        self.apply_placement_change(index, change, animate);
        self.interactive_resize_end(None);
    }

    pub fn center_window(&mut self, id: Option<&W::Id>) {
        let Some(id) = id.or(self.active_window_id()) else {
            return;
        };
        let index = self.idx_of(id).unwrap();
        let change = self
            .policy
            .center_window(self.tiles[index].window().core_id())
            .unwrap();
        self.apply_placement_change(index, change, true);
        self.interactive_resize_end(None);
    }

    pub fn descendants_added(&mut self, id: &W::Id) -> bool {
        let Some(idx) = self.idx_of(id) else {
            return false;
        };

        self.bring_up_descendants_of(idx);
        true
    }

    pub fn update_window(&mut self, id: &W::Id, serial: Option<Serial>) -> bool {
        let Some(index) = self.idx_of(id) else {
            return false;
        };
        let tile = &mut self.tiles[index];
        let resize = tile.window_mut().interactive_resize_data();
        if let Some(serial) = serial {
            tile.window_mut().on_commit(serial);
        }
        tile.update_window();
        let size = tile.tile_size();
        self.policy
            .commit_tile_size(
                tile.window().core_id(),
                clarion_core::Size::new(size.w, size.h),
                resize.map(|resize| resize.edges.bits()),
            )
            .unwrap();
        true
    }

    /// Capture this space once; scene code owns channel iteration and drawing.
    pub(crate) fn scene_items<'a>(
        &'a self,
        normal_visible: bool,
        focus: bool,
        items: &mut Vec<crate::scene::workspace_composition::OrderedItem<'a, W>>,
    ) {
        use crate::layout::scene_source::{ClosingRenderSource, TileRenderSource};
        use crate::scene::workspace_composition::{Channel, Item, OrderedItem, TileSpace};
        let active = self.active_window_id();
        for item in self.stacked_items() {
            match item {
                FloatingRenderItem::Closing(closing) if normal_visible => {
                    items.push(OrderedItem {
                        channel: Channel::FloatingNormal,
                        item: Item::Closing {
                            source: ClosingRenderSource::new(closing),
                            view: Rectangle::from_size(self.view_size),
                            scale: Scale::from(self.scale),
                            space: TileSpace::Floating,
                        },
                    });
                }
                FloatingRenderItem::Closing(_) => (),
                FloatingRenderItem::Tile(tile, position) => {
                    let moving = tile.is_moving_between_workspaces();
                    if !moving && !normal_visible {
                        continue;
                    }
                    items.push(OrderedItem {
                        channel: if moving {
                            Channel::FloatingMoving
                        } else {
                            Channel::FloatingNormal
                        },
                        item: Item::Tile {
                            source: TileRenderSource::new(tile),
                            position,
                            focused: focus && Some(tile.window().id()) == active,
                            space: TileSpace::Floating,
                        },
                    });
                }
            }
        }
    }

    pub fn render<R: ClarionRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        xray_pos: XrayPos,
        view_rect: Rectangle<f64, Logical>,
        focus_ring: bool,
        layer: RenderLayer,
        push: &mut dyn FnMut(FloatingSpaceRenderElement<R>),
    ) {
        let scale = Scale::from(self.scale);

        let active = self.active_window_id().cloned();
        for item in self.stacked_items() {
            match item {
                FloatingRenderItem::Closing(closing) if layer.is_normal() => {
                    for elem in closing.render(ctx.r(), view_rect, scale) {
                        push(elem.into());
                    }
                }
                FloatingRenderItem::Closing(_) => (),
                FloatingRenderItem::Tile(tile, tile_pos) => {
                    if layer.is_normal() == tile.is_moving_between_workspaces() {
                        continue;
                    }
                    let focus_ring = focus_ring && Some(tile.window().id()) == active.as_ref();
                    let xray_pos = xray_pos.offset(tile_pos);
                    tile.scene()
                        .render(ctx.r(), tile_pos, xray_pos, focus_ring, &mut |elem| {
                            push(elem.into())
                        });
                }
            }
        }
    }

    pub fn interactive_resize_begin(&mut self, window: W::Id, edges: ResizeEdge) -> bool {
        if self.policy.resize().is_some() {
            return false;
        }
        let tile = self
            .tiles
            .iter_mut()
            .find(|tile| tile.window().id() == &window)
            .unwrap();
        let size = tile.window_size();
        if !self.policy.begin_resize(
            tile.window().core_id(),
            clarion_core::Size::new(size.w, size.h),
            edges.bits(),
        ) {
            return false;
        }
        tile.start_wobble(WobbleAnchor::from_resize_edges(edges));
        true
    }

    pub fn interactive_resize_update(
        &mut self,
        window: &W::Id,
        delta: Point<f64, Logical>,
    ) -> bool {
        let Some(index) = self.idx_of(window) else {
            return false;
        };
        let id = self.tiles[index].window().core_id();
        let Some(intent) = self.policy.resize_intent(id, [delta.x, delta.y]) else {
            return false;
        };
        if let Some(width) = intent.width {
            self.set_window_width(Some(window), SizeChange::SetFixed(width), false);
        }
        if let Some(height) = intent.height {
            self.set_window_height(Some(window), SizeChange::SetFixed(height), false);
        }
        true
    }

    pub fn interactive_resize_end(&mut self, window: Option<&W::Id>) {
        let id = match window {
            Some(window) => {
                let Some(index) = self.idx_of(window) else {
                    return;
                };
                Some(self.tiles[index].window().core_id())
            }
            None => None,
        };
        let Some(id) = self.policy.end_resize(id) else {
            return;
        };
        if let Some(tile) = self
            .tiles
            .iter_mut()
            .find(|tile| tile.window().core_id() == id)
        {
            tile.finish_wobble();
        }
    }

    pub fn refresh(&mut self, is_active: bool, is_focused: bool) {
        let active = self.active_window_id().cloned();
        for tile in &mut self.tiles {
            let win = tile.window_mut();

            win.set_active_in_column(true);
            win.set_floating(true);

            let mut is_active = is_active && Some(win.id()) == active.as_ref();
            if self.options.deactivate_unfocused_windows {
                is_active &= is_focused;
            }
            win.set_activated(is_active);

            let resize_data = self
                .policy
                .resize()
                .filter(|resize| resize.id == win.core_id())
                .map(|resize| InteractiveResizeData {
                    edges: ResizeEdge::from_bits_truncate(resize.edges),
                });
            win.set_interactive_resize(resize_data);

            let border_config = self.options.layout.border.merged_with(&win.rules().border);
            let bounds = compute_toplevel_bounds(border_config, self.working_area.size);
            win.set_bounds(bounds);

            // If transactions are disabled, also disable combined throttling, for more
            // intuitive behavior.
            let intent = if self.options.disable_resize_throttling {
                ConfigureIntent::CanSend
            } else {
                win.configure_intent()
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

    pub fn clamp_within_working_area(
        &self,
        pos: Point<f64, Logical>,
        size: Size<f64, Logical>,
    ) -> Point<f64, Logical> {
        let area = self.working_area;
        let rect = clarion_core::geometry::clamp_rect(
            clarion_core::Rect::new(pos.x, pos.y, size.w, size.h),
            clarion_core::Rect::new(area.loc.x, area.loc.y, area.size.w, area.size.h),
        );
        Point::from((rect.x, rect.y))
    }

    pub fn scale_by_working_area(&self, pos: Point<f64, SizeFrac>) -> Point<f64, Logical> {
        let position = Placement::logical_position(core_area(self.working_area), [pos.x, pos.y]);
        (position[0], position[1]).into()
    }

    pub fn logical_to_size_frac(&self, logical_pos: Point<f64, Logical>) -> Point<f64, SizeFrac> {
        let position = Placement::relative_position(
            core_area(self.working_area),
            [logical_pos.x, logical_pos.y],
        );
        (position[0], position[1]).into()
    }

    fn apply_placement_change(&mut self, index: usize, change: PlacementChange, animate: bool) {
        let displacement: Point<f64, Logical> = (
            change.previous.x - change.current.x,
            change.previous.y - change.current.y,
        )
            .into();
        if animate && displacement.x * displacement.x + displacement.y * displacement.y > 100. {
            self.tiles[index].animate_move_from(displacement);
        }
    }

    pub fn new_window_size(
        &self,
        width: Option<PresetSize>,
        height: Option<PresetSize>,
        rules: &ResolvedWindowRules,
    ) -> Size<i32, Logical> {
        let border = self.options.layout.border.merged_with(&rules.border);
        let decoration = if border.off { 0. } else { border.width * 2. };
        let size = clarion_core::floating::initial_client_size(
            width.map(core_preset),
            height.map(core_preset),
            clarion_core::Size::new(self.working_area.size.w, self.working_area.size.h),
            clarion_core::Size::new(decoration, decoration),
        );
        (size[0], size[1]).into()
    }

    pub fn stored_or_default_tile_pos(&self, tile: &Tile<W>) -> Option<Point<f64, Logical>> {
        let stored = tile
            .floating_pos
            .map(|position| self.scale_by_working_area(position));
        stored.or_else(|| {
            tile.window()
                .rules()
                .default_floating_position
                .map(|position| {
                    let anchor = match position.relative_to {
                        RelativeTo::TopLeft => clarion_core::floating::Anchor::TopLeft,
                        RelativeTo::TopRight => clarion_core::floating::Anchor::TopRight,
                        RelativeTo::BottomLeft => clarion_core::floating::Anchor::BottomLeft,
                        RelativeTo::BottomRight => clarion_core::floating::Anchor::BottomRight,
                        RelativeTo::Top => clarion_core::floating::Anchor::Top,
                        RelativeTo::Bottom => clarion_core::floating::Anchor::Bottom,
                        RelativeTo::Left => clarion_core::floating::Anchor::Left,
                        RelativeTo::Right => clarion_core::floating::Anchor::Right,
                    };
                    let size = tile.tile_size();
                    let position = clarion_core::floating::anchored_position(
                        core_area(self.working_area),
                        clarion_core::Size::new(size.w, size.h),
                        anchor,
                        [position.x.0, position.y.0],
                    );
                    (position[0], position[1]).into()
                })
        })
    }

    #[cfg(test)]
    pub fn view_size(&self) -> Size<f64, Logical> {
        self.view_size
    }

    pub fn working_area(&self) -> Rectangle<f64, Logical> {
        self.working_area
    }

    #[cfg(test)]
    pub fn scale(&self) -> f64 {
        self.scale
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
    pub fn verify_invariants(&self) {
        assert!(self.scale > 0.);
        assert!(self.scale.is_finite());
        assert_eq!(self.tiles.len(), self.policy.len());

        for (i, (tile, data)) in zip(&self.tiles, &self.policy).enumerate() {
            use crate::layout::SizingMode;

            assert!(Rc::ptr_eq(&self.options, &tile.presentation.options));
            assert_eq!(self.view_size, tile.view_size());
            assert_eq!(self.clock, tile.presentation.clock);
            assert_eq!(self.scale, tile.scale());
            tile.verify_invariants();

            let presets = self.policy.preset_indices(i);
            assert!(
                tile.floating_preset_width_idx.is_none()
                    && tile.floating_preset_height_idx.is_none(),
                "active floating preferences belong to the independent policy"
            );
            if let Some(idx) = presets[0] {
                assert!(idx < self.options.layout.preset_column_widths.len());
            }
            if let Some(idx) = presets[1] {
                assert!(idx < self.options.layout.preset_window_heights.len());
            }

            assert_eq!(
                tile.window().pending_sizing_mode(),
                SizingMode::Normal,
                "floating windows cannot be maximized or fullscreen"
            );

            assert!(data.is_consistent());

            let mut data2 = *data;
            data2.update_from_tile(tile);
            data2.set_area(core_area(self.working_area));
            assert_eq!(data, &data2, "tile data must be up to date");

            for tile_below in &self.tiles[i + 1..] {
                assert!(
                    !tile_below.window().is_child_of(tile.window()),
                    "children must be stacked above parents"
                );
            }
        }

        if let Some(id) = self.active_window_id() {
            assert!(!self.tiles.is_empty());
            assert!(self.contains(id), "active window must be present in tiles");
        } else {
            assert!(self.tiles.is_empty());
        }

        assert_eq!(
            self.stacking
                .entries
                .iter()
                .filter_map(|entry| match entry {
                    FloatingStackEntry::Live(id) => Some(*id),
                    FloatingStackEntry::Closing(_) => None,
                })
                .collect::<Vec<_>>(),
            self.tiles
                .iter()
                .map(|tile| tile.window().core_id())
                .collect::<Vec<_>>()
        );
        assert!(self.policy.verify_invariants());
        assert_eq!(
            self.policy
                .members()
                .map(|member| member.id)
                .collect::<Vec<_>>(),
            self.tiles
                .iter()
                .map(|tile| tile.window().core_id())
                .collect::<Vec<_>>()
        );
    }
}

fn compute_toplevel_bounds(
    border_config: clarion_config::Border,
    working_area_size: Size<f64, Logical>,
) -> Size<i32, Logical> {
    let border = if border_config.off {
        0.
    } else {
        border_config.width * 2.
    };
    let size = clarion_core::floating::client_bounds(
        clarion_core::Size::new(working_area_size.w, working_area_size.h),
        clarion_core::Size::new(border, border),
    );
    (size[0], size[1]).into()
}

fn core_preset(preset: PresetSize) -> clarion_core::floating::Preset {
    match preset {
        PresetSize::Fixed(value) => clarion_core::floating::Preset::Fixed(value),
        PresetSize::Proportion(value) => clarion_core::floating::Preset::Proportion(value),
    }
}

#[cfg(test)]
mod stacking_tests {
    use super::*;

    fn stack() -> FloatingStack {
        let mut stack = FloatingStack::default();
        stack.place_live(1, None, true);
        stack.place_live(2, Some(1), true);
        stack.place_live(3, Some(2), true);
        stack
    }

    #[test]
    fn background_close_retains_its_slot_between_surviving_windows() {
        let mut stack = stack();
        let closing = Id::new();
        stack.close(2, closing.clone());
        stack.remove_live(2);
        assert_eq!(
            stack.entries,
            vec![
                FloatingStackEntry::Live(3),
                FloatingStackEntry::Closing(closing),
                FloatingStackEntry::Live(1),
            ]
        );
    }

    #[test]
    fn consecutive_closes_keep_original_order_and_new_windows_open_above_them() {
        let mut stack = stack();
        let bottom = Id::new();
        let top = Id::new();
        stack.close(1, bottom.clone());
        stack.remove_live(1);
        stack.close(3, top.clone());
        stack.remove_live(3);
        stack.place_live(4, Some(2), true);
        assert_eq!(
            stack.entries,
            vec![
                FloatingStackEntry::Live(4),
                FloatingStackEntry::Closing(top),
                FloatingStackEntry::Live(2),
                FloatingStackEntry::Closing(bottom),
            ]
        );
    }

    #[test]
    fn activating_a_survivor_raises_it_above_a_closing_snapshot() {
        let mut stack = stack();
        let closing = Id::new();
        stack.close(2, closing.clone());
        stack.remove_live(2);
        stack.place_live(1, Some(3), true);
        assert_eq!(
            stack.entries,
            vec![
                FloatingStackEntry::Live(1),
                FloatingStackEntry::Live(3),
                FloatingStackEntry::Closing(closing),
            ]
        );
    }

    #[test]
    fn closing_a_detached_drag_remains_above_the_workspace() {
        let mut stack = stack();
        let closing = Id::new();
        stack.close(9, closing.clone());
        assert_eq!(
            stack.entries.first(),
            Some(&FloatingStackEntry::Closing(closing))
        );
        stack.retain_closing(&[]);
        assert_eq!(
            stack.entries,
            vec![
                FloatingStackEntry::Live(3),
                FloatingStackEntry::Live(2),
                FloatingStackEntry::Live(1),
            ]
        );
    }
}
