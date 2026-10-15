use std::collections::HashMap;
use std::rc::Rc;

use realand_config::utils::MergeWith as _;
use realand_core::bsp::{BspArrangement, BspSnapshot, BspTree, Direction, PaneMode};
use realand_core::layout::{BspNode, LiveLayoutPolicy};
use realand_ipc::{SizeChange, WindowLayout};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::utils::{Logical, Point, Rectangle, Scale, Serial, Size};

use super::closing_window::{ClosingWindow, ClosingWindowRenderElement};
use super::scrolling::ColumnWidth;
use super::tile::{Tile, TileRenderElement, TileRenderSnapshot};
use super::{
    ActivateWindow, ConfigureIntent, HitType, InteractiveResizeData, LayoutElement, Options,
    RemovedTile, RenderLayer, SizingMode,
};
use crate::animation::{Animation, Clock};
use crate::realand_render_elements;
use crate::render_helpers::renderer::RealandRenderer;
use crate::render_helpers::xray::XrayPos;
use crate::render_helpers::RenderCtx;
use crate::utils::transaction::Transaction;
use crate::utils::transaction::TransactionBlocker;
use crate::utils::ResizeEdge;

/// Direction to split a container node in dwindle layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDirection {
    /// Split side-by-side (left / right).
    Horizontal,
    /// Split stacked (top / bottom).
    Vertical,
}

impl SplitDirection {
    /// Toggle between horizontal and vertical.
    pub fn toggle(self) -> Self {
        match self {
            Self::Horizontal => Self::Vertical,
            Self::Vertical => Self::Horizontal,
        }
    }
}

// Bound session topology independently of protocol/renderer payloads.
const MAX_SPLIT_DEPTH: usize = realand_ipc::management::MAX_SESSION_TREE_DEPTH;

fn core_rect(rect: Rectangle<f64, Logical>) -> realand_core::Rect {
    realand_core::Rect::new(rect.loc.x, rect.loc.y, rect.size.w, rect.size.h)
}
fn platform_rect(rect: realand_core::Rect) -> Rectangle<f64, Logical> {
    Rectangle::new((rect.x, rect.y).into(), (rect.width, rect.height).into())
}

#[derive(Debug)]
struct TileSlot<W: LayoutElement> {
    tile: Box<Tile<W>>,
    geometry: Rectangle<f64, Logical>,
}

/// Flat device payload storage, keyed by the IDs owned by the core BSP.
/// Order is synchronized only after topology changes, making mutable rendering
/// traversal safe and allocation-free without borrowing several map entries.
#[derive(Debug)]
struct TileStore<W: LayoutElement> {
    slots: Vec<TileSlot<W>>,
    index: HashMap<W::Id, usize>,
    core_index: HashMap<u64, usize>,
}

impl<W: LayoutElement> TileStore<W> {
    fn new() -> Self {
        Self {
            slots: Vec::new(),
            index: HashMap::new(),
            core_index: HashMap::new(),
        }
    }
    fn reindex(&mut self) {
        self.index = self
            .slots
            .iter()
            .enumerate()
            .map(|(i, slot)| (slot.tile.window().id().clone(), i))
            .collect();
        self.core_index = self
            .slots
            .iter()
            .enumerate()
            .map(|(i, slot)| (slot.tile.window().core_id(), i))
            .collect();
    }
    fn reorder(&mut self, tree: &BspTree<u64>) {
        let order: HashMap<_, _> = tree
            .ordered_ids()
            .iter()
            .enumerate()
            .map(|(i, id)| (*id, i))
            .collect();
        self.slots.sort_unstable_by_key(|slot| {
            order
                .get(&slot.tile.window().core_id())
                .copied()
                .unwrap_or(usize::MAX)
        });
        self.reindex();
    }
    fn insert(&mut self, tile: Tile<W>, geometry: Rectangle<f64, Logical>, tree: &BspTree<u64>) {
        self.slots.push(TileSlot {
            tile: Box::new(tile),
            geometry,
        });
        self.reorder(tree);
    }
    fn remove(&mut self, id: &W::Id, tree: &BspTree<u64>) -> Option<Box<Tile<W>>> {
        let idx = *self.index.get(id)?;
        let tile = self.slots.remove(idx).tile;
        self.reorder(tree);
        Some(tile)
    }
    fn find_tile(&self, id: &W::Id) -> Option<&Tile<W>> {
        self.index.get(id).map(|idx| self.slots[*idx].tile.as_ref())
    }
    fn find_core_tile(&self, id: u64) -> Option<&Tile<W>> {
        self.core_index
            .get(&id)
            .map(|idx| self.slots[*idx].tile.as_ref())
    }
    fn find_tile_mut(&mut self, id: &W::Id) -> Option<&mut Tile<W>> {
        let idx = *self.index.get(id)?;
        Some(self.slots[idx].tile.as_mut())
    }
    fn find_leaf(&self, id: &W::Id) -> Option<(&Tile<W>, Rectangle<f64, Logical>)> {
        self.index.get(id).map(|idx| {
            let slot = &self.slots[*idx];
            (slot.tile.as_ref(), slot.geometry)
        })
    }
    fn find_leaf_mut(&mut self, id: &W::Id) -> Option<(&mut Tile<W>, Rectangle<f64, Logical>)> {
        let idx = *self.index.get(id)?;
        let slot = &mut self.slots[idx];
        Some((slot.tile.as_mut(), slot.geometry))
    }
    fn apply_placement(&mut self, id: &u64, bounds: realand_core::Rect) {
        let Some(index) = self.core_index.get(id).copied() else {
            return;
        };
        let slot = &mut self.slots[index];
        let next = platform_rect(bounds);
        if slot.geometry.loc != next.loc && slot.geometry.size.w > 0. {
            slot.tile.animate_move_from(slot.geometry.loc - next.loc);
        }
        slot.geometry = next;
    }
    fn request_sizes(
        &mut self,
        area: Rectangle<f64, Logical>,
        animate: bool,
        transaction: &Transaction,
    ) {
        for slot in &mut self.slots {
            let transaction = Some(transaction.clone());
            match slot.tile.window().pending_sizing_mode() {
                SizingMode::Fullscreen => slot.tile.request_fullscreen(animate, transaction),
                SizingMode::Maximized => {
                    slot.tile.request_maximized(area.size, animate, transaction)
                }
                SizingMode::Normal => {
                    slot.tile
                        .request_tile_size(slot.geometry.size, animate, transaction)
                }
            }
        }
    }
    fn collect_tiles(self, acc: &mut Vec<Tile<W>>) {
        acc.extend(self.slots.into_iter().map(|slot| *slot.tile));
    }
    fn collect_tile_refs<'a>(&'a self, acc: &mut Vec<&'a Tile<W>>) {
        acc.extend(self.slots.iter().map(|slot| slot.tile.as_ref()));
    }
    fn collect_tiles_mut<'a>(&'a mut self, acc: &mut Vec<&'a mut Tile<W>>) {
        acc.extend(self.slots.iter_mut().map(|slot| slot.tile.as_mut()));
    }
    fn collect_leaf_tiles<'a>(&'a self, acc: &mut Vec<(&'a Tile<W>, Rectangle<f64, Logical>)>) {
        acc.extend(
            self.slots
                .iter()
                .map(|slot| (slot.tile.as_ref(), slot.geometry)),
        );
    }
    fn with_tiles(&self, f: &mut impl FnMut(&Tile<W>)) {
        for slot in &self.slots {
            f(&slot.tile);
        }
    }
    fn for_each_tile_mut(&mut self, f: &mut impl FnMut(&mut Tile<W>)) {
        for slot in &mut self.slots {
            f(&mut slot.tile);
        }
    }
    fn for_each_leaf(&self, f: &mut impl FnMut(&Tile<W>, Rectangle<f64, Logical>)) {
        for slot in &self.slots {
            f(&slot.tile, slot.geometry);
        }
    }
    fn advance_animations(&mut self) {
        self.for_each_tile_mut(&mut |tile| tile.advance_animations());
    }
    fn are_animations_ongoing(&self) -> bool {
        self.slots
            .iter()
            .any(|slot| slot.tile.are_animations_ongoing())
    }
    fn are_transitions_ongoing(&self) -> bool {
        self.slots
            .iter()
            .any(|slot| slot.tile.are_transitions_ongoing())
    }
    fn retain_placed(&mut self, tree: &BspTree<u64>, reset: bool) -> Vec<Tile<W>> {
        let mut extra = Vec::new();
        for mut slot in std::mem::take(&mut self.slots) {
            if tree.contains(&slot.tile.window().core_id()) {
                if reset {
                    slot.geometry = Rectangle::default();
                }
                self.slots.push(slot);
            } else {
                extra.push(*slot.tile);
            }
        }
        self.reorder(tree);
        extra
    }
}

realand_render_elements! {
    DwindleSpaceRenderElement<R> => {
        Tile = TileRenderElement<R>,
        ClosingWindow = ClosingWindowRenderElement,
    }
}

/// Dynamic BSP / Dwindle tiling space (Hyprland style).
#[derive(Debug)]
pub struct DwindleSpace<W: LayoutElement> {
    tree: BspTree<u64>,
    payloads: Option<TileStore<W>>,
    pub(super) closing_windows: Vec<ClosingWindow>,
    view_size: Size<f64, Logical>,
    working_area: Rectangle<f64, Logical>,
    scale: f64,
    clock: Clock,
    options: Rc<Options>,
    interactive_resize: Option<realand_core::bsp::SplitResize>,
}

impl<W: LayoutElement> DwindleSpace<W> {
    pub fn new(
        view_size: Size<f64, Logical>,
        working_area: Rectangle<f64, Logical>,
        scale: f64,
        clock: Clock,
        options: Rc<Options>,
    ) -> Self {
        Self {
            tree: BspTree::new(
                core_rect(working_area),
                options.layout.gaps,
                MAX_SPLIT_DEPTH,
            ),
            payloads: None,
            closing_windows: Vec::new(),
            view_size,
            working_area,
            scale,
            clock,
            options,
            interactive_resize: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.tree.is_empty()
    }
    pub fn len(&self) -> usize {
        self.tree.len()
    }

    fn core_id(&self, id: &W::Id) -> Option<u64> {
        self.payloads
            .as_ref()?
            .find_tile(id)
            .map(|tile| tile.window().core_id())
    }
    fn physical_id(&self, id: u64) -> Option<W::Id> {
        self.payloads
            .as_ref()?
            .find_core_tile(id)
            .map(|tile| tile.window().id().clone())
    }
    pub fn active_window_id(&self) -> Option<&W::Id> {
        Some(self.active_tile()?.window().id())
    }

    pub fn active_tile(&self) -> Option<&Tile<W>> {
        self.payloads.as_ref()?.find_core_tile(*self.tree.active()?)
    }

    pub fn active_tile_mut(&mut self) -> Option<&mut Tile<W>> {
        let index = self
            .payloads
            .as_ref()?
            .core_index
            .get(self.tree.active()?)
            .copied()?;
        Some(&mut self.payloads.as_mut()?.slots[index].tile)
    }

    pub fn set_active_window(&mut self, id: W::Id) {
        if let Some(id) = self.core_id(&id) {
            self.tree.activate(&id);
        }
    }

    pub fn set_fullscreen(&mut self, window: &W::Id, is_fullscreen: bool) -> bool {
        if is_fullscreen {
            self.restore_normal_except(window);
        }
        let Some((tile, geometry)) = self.payloads.as_mut().and_then(|r| r.find_leaf_mut(window))
        else {
            return false;
        };
        let transaction = Some(Transaction::new());
        if is_fullscreen {
            tile.request_fullscreen(true, transaction);
        } else {
            tile.request_tile_size(geometry.size, true, transaction);
        }
        true
    }

    pub fn set_maximized(&mut self, window: &W::Id, is_maximized: bool) -> bool {
        if is_maximized {
            self.restore_normal_except(window);
        }
        let Some((tile, geometry)) = self.payloads.as_mut().and_then(|r| r.find_leaf_mut(window))
        else {
            return false;
        };
        let transaction = Some(Transaction::new());
        if is_maximized {
            tile.request_maximized(self.working_area.size, true, transaction);
        } else {
            tile.request_tile_size(geometry.size, true, transaction);
        }
        true
    }

    pub fn is_active_pending_fullscreen(&self) -> bool {
        self.active_tile()
            .is_some_and(|tile| tile.window().pending_sizing_mode().is_fullscreen())
    }

    pub fn has_window(&self, id: &W::Id) -> bool {
        self.core_id(id)
            .is_some_and(|core_id| self.tree.contains(&core_id))
    }

    fn restore_normal_except(&mut self, keep: &W::Id) {
        let mut restore = Vec::new();
        if let Some(root) = self.payloads.as_ref() {
            root.for_each_leaf(&mut |tile, geom| {
                if tile.window().id() != keep && !tile.window().pending_sizing_mode().is_normal() {
                    restore.push((tile.window().id().clone(), geom.size));
                }
            });
        }
        for (id, size) in restore {
            if let Some((tile, _)) = self
                .payloads
                .as_mut()
                .and_then(|root| root.find_leaf_mut(&id))
            {
                tile.request_tile_size(size, true, Some(Transaction::new()));
            }
        }
    }

    pub fn tiled_positions(&self) -> Vec<(W::Id, Point<f64, Logical>)> {
        let mut acc = Vec::new();
        if let Some(root) = &self.payloads {
            root.collect_leaf_tiles(&mut acc);
        }
        acc.into_iter()
            .filter(|(tile, _)| tile.window().pending_sizing_mode().is_normal())
            .map(|(tile, geom)| (tile.window().id().clone(), geom.loc))
            .collect()
    }

    /// Visit intended layout slots without render offsets or temporary allocations.
    /// Normal slots include decoration space; expanded modes use their target
    /// output/working bounds even before the client commits the new configure.
    pub(super) fn with_layout_geometry(
        &self,
        f: &mut impl FnMut(&Tile<W>, Rectangle<f64, Logical>),
    ) {
        if let Some(root) = &self.payloads {
            root.for_each_leaf(&mut |tile, normal| {
                let geometry = match tile.window().pending_sizing_mode() {
                    SizingMode::Fullscreen => Rectangle::from_size(self.view_size),
                    SizingMode::Maximized => self.working_area,
                    SizingMode::Normal => normal,
                };
                f(tile, geometry);
            });
        }
    }

    fn tile_is_expanded(tile: &Tile<W>) -> bool {
        !tile.window().pending_sizing_mode().is_normal() || !tile.sizing_mode().is_normal()
    }

    fn expanded_origin(
        tile: &Tile<W>,
        working_area_loc: Point<f64, Logical>,
    ) -> Option<Point<f64, Logical>> {
        let pending = tile.window().pending_sizing_mode();
        if pending.is_fullscreen() || tile.sizing_mode().is_fullscreen() {
            Some(Point::from((0., 0.)))
        } else if pending.is_maximized() || tile.sizing_mode().is_maximized() {
            Some(working_area_loc)
        } else {
            None
        }
    }

    fn tile_render_position(
        tile: &Tile<W>,
        geometry: Rectangle<f64, Logical>,
        working_area_loc: Point<f64, Logical>,
    ) -> Point<f64, Logical> {
        // BSP rearrangement animations move normal panes. Expanded panes always align to the
        // view/work area, even while their reserved tree leaf is being moved underneath them.
        Self::expanded_origin(tile, working_area_loc)
            .unwrap_or_else(|| geometry.loc + tile.render_offset())
    }

    fn expanded_render(&self) -> Option<(W::Id, Point<f64, Logical>)> {
        // Focusing another pane reveals it without changing the previous pane's requested
        // fullscreen/maximized state, just as switching away from a scrolling column does.
        let tile = self.active_tile()?;
        let origin = Self::expanded_origin(tile, self.working_area.loc)?;
        Some((tile.window().id().clone(), origin))
    }

    pub fn should_capture_close_snapshot(&self, window: &W::Id) -> Option<bool> {
        let tile = self.payloads.as_ref()?.find_tile(window)?;
        Some(self.expanded_render().map_or_else(
            || !Self::tile_is_expanded(tile),
            |(expanded, _)| &expanded == window,
        ))
    }

    pub fn refresh(&mut self, is_active: bool, is_focused: bool) {
        let active_id = self.tree.active().copied();
        let resize = self.interactive_resize;
        let options = &self.options;
        let working_area_size = self.working_area.size;

        if let Some(ref mut root) = self.payloads {
            root.for_each_tile_mut(&mut |tile| {
                let win = tile.window_mut();

                win.set_active_in_column(true);
                win.set_floating(false);

                let mut is_active = is_active && active_id == Some(win.core_id());
                if options.deactivate_unfocused_windows {
                    is_active &= is_focused;
                }
                win.set_activated(is_active);

                let resize_data = resize
                    .as_ref()
                    .filter(|resize| resize.resize.id == win.core_id())
                    .map(|resize| InteractiveResizeData {
                        edges: ResizeEdge::from_bits_retain(resize.resize.edges),
                    });
                win.set_interactive_resize(resize_data);

                let border_config = options.layout.border.merged_with(&win.rules().border);
                let bounds = compute_toplevel_bounds(
                    border_config,
                    working_area_size,
                    Size::from((0., 0.)),
                    options.layout.gaps,
                );
                win.set_bounds(bounds);

                // If transactions are disabled, also disable combined throttling, for more
                // intuitive behavior.
                let intent = if options.disable_resize_throttling {
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
            });
        }
    }

    pub fn active_window(&self) -> Option<&W> {
        self.active_tile().map(Tile::window)
    }

    pub fn active_window_mut(&mut self) -> Option<&mut W> {
        self.active_tile_mut().map(Tile::window_mut)
    }

    pub fn tiles(&self) -> std::vec::IntoIter<&Tile<W>> {
        let mut acc = Vec::new();
        if let Some(ref root) = self.payloads {
            root.collect_tile_refs(&mut acc);
        }
        acc.into_iter()
    }

    pub(super) fn with_tiles(&self, f: &mut impl FnMut(&Tile<W>)) {
        if let Some(root) = &self.payloads {
            root.with_tiles(f);
        }
    }

    pub fn tiles_mut(&mut self) -> std::vec::IntoIter<&mut Tile<W>> {
        let mut acc = Vec::new();
        if let Some(ref mut root) = self.payloads {
            root.collect_tiles_mut(&mut acc);
        }
        acc.into_iter()
    }

    pub fn tiles_with_render_positions(
        &self,
    ) -> impl Iterator<Item = (&Tile<W>, Point<f64, Logical>, bool)> {
        let scale = self.scale;
        let expanded = self.expanded_render();
        let working_area_loc = self.working_area.loc;
        self.payloads
            .as_ref()
            .into_iter()
            .flat_map(|store| store.slots.iter())
            .map(move |slot| {
                let geom = slot.geometry;
                let tile = slot.tile.as_ref();
                let visible = expanded.as_ref().map_or_else(
                    || !Self::tile_is_expanded(tile),
                    |(id, _)| tile.window().id() == id,
                );
                let pos = Self::tile_render_position(tile, geom, working_area_loc);
                let pos = pos.to_physical_precise_round(scale).to_logical(scale);
                (tile, pos, visible)
            })
    }

    pub fn tiles_with_render_positions_mut(
        &mut self,
        round: bool,
    ) -> impl Iterator<Item = (&mut Tile<W>, Point<f64, Logical>, bool)> {
        let scale = self.scale;
        let expanded = self.expanded_render();
        let working_area_loc = self.working_area.loc;
        self.payloads
            .as_mut()
            .into_iter()
            .flat_map(|store| store.slots.iter_mut())
            .map(move |slot| {
                let geom = slot.geometry;
                let tile = slot.tile.as_mut();
                let visible = expanded.as_ref().map_or_else(
                    || !Self::tile_is_expanded(tile),
                    |(id, _)| tile.window().id() == id,
                );
                let mut pos = Self::tile_render_position(tile, geom, working_area_loc);
                if round {
                    pos = pos.to_physical_precise_round(scale).to_logical(scale);
                }
                (tile, pos, visible)
            })
    }

    pub fn tiles_with_ipc_layouts(&self) -> impl Iterator<Item = (&Tile<W>, WindowLayout)> {
        let scale = self.scale;
        let working_area_loc = self.working_area.loc;
        self.payloads
            .as_ref()
            .into_iter()
            .flat_map(|store| store.slots.iter())
            .map(move |slot| {
                let geom = slot.geometry;
                let tile = slot.tile.as_ref();
                // Report the final placement without temporary movement to avoid IPC spam.
                // Expanded panes use their display origin, rather than their reserved BSP leaf.
                let pos = Self::expanded_origin(tile, working_area_loc).unwrap_or(geom.loc);
                let pos = pos.to_physical_precise_round(scale).to_logical(scale);

                let layout = WindowLayout {
                    tile_pos_in_workspace_view: Some(pos.into()),
                    ..tile.ipc_layout_template()
                };
                (tile, layout)
            })
    }

    /// Returns the geometry of the active window relative to and clamped to the working area.
    pub fn active_window_visual_rectangle(&self) -> Option<Rectangle<f64, Logical>> {
        let active_id = self.tree.active()?;
        let (tile, pos, _) = self
            .tiles_with_render_positions()
            .find(|(tile, _, _)| tile.window().core_id() == *active_id)?;
        let window_pos = pos + tile.window_loc();
        let window_size = tile.window_size();
        let window_rect = Rectangle::new(window_pos, window_size);

        self.working_area.intersection(window_rect)
    }

    pub fn popup_target_rect(&self, id: &W::Id) -> Option<Rectangle<f64, Logical>> {
        let (tile, pos, _) = self
            .tiles_with_render_positions()
            .find(|(tile, _, _)| tile.window().id() == id)?;

        // Position within the working area, like in the floating layout.
        let mut target = self.working_area;
        target.loc -= pos;
        target.loc -= tile.window_loc();
        Some(target)
    }

    pub fn update_render_elements(
        &mut self,
        is_active: bool,
        view_rect: Rectangle<f64, Logical>,
        layer: RenderLayer,
    ) {
        let active_id = self.tree.active().copied();
        for (tile, pos, visible) in self.tiles_with_render_positions_mut(true) {
            if !visible || layer.is_normal() == tile.is_moving_between_workspaces() {
                continue;
            }
            let is_active = is_active && active_id == Some(tile.window().core_id());
            let mut tile_view_rect = view_rect;
            tile_view_rect.loc -= pos;
            tile.update_render_elements(is_active, tile_view_rect);
        }
    }

    pub fn update_window(&mut self, id: &W::Id, serial: Option<Serial>) -> bool {
        let Some(tile) = self
            .payloads
            .as_mut()
            .and_then(|root| root.find_tile_mut(id))
        else {
            return false;
        };

        // Do this before calling update_window() so it can get up-to-date info.
        if let Some(serial) = serial {
            tile.window_mut().on_commit(serial);
        }

        tile.update_window();
        true
    }

    pub fn start_open_animation(&mut self, id: &W::Id) -> bool {
        match self
            .payloads
            .as_mut()
            .and_then(|root| root.find_tile_mut(id))
        {
            Some(tile) => {
                tile.start_open_animation();
                true
            }
            None => false,
        }
    }

    pub fn start_close_animation_for_window(
        &mut self,
        renderer: &mut GlesRenderer,
        window: &W::Id,
        blocker: TransactionBlocker,
    ) {
        let Some((tile, tile_pos, _)) = self
            .tiles_with_render_positions_mut(true)
            .find(|(tile, _, visible)| *visible && tile.window().id() == window)
        else {
            return;
        };

        let Some(snapshot) = tile.take_unmap_snapshot() else {
            return;
        };

        let dmabuf = tile.window().dmabuf();
        let tile_size = tile.tile_size();

        self.start_close_animation_for_tile(
            renderer, snapshot, tile_size, tile_pos, blocker, dmabuf,
        );
    }

    pub fn start_close_animation_for_window_native(
        &mut self,
        renderer: &mut crate::render_helpers::vulkan::native::NativeRenderer,
        window: &W::Id,
        blocker: TransactionBlocker,
    ) {
        let Some((tile, tile_pos, _)) = self
            .tiles_with_render_positions_mut(true)
            .find(|(tile, _, visible)| *visible && tile.window().id() == window)
        else {
            return;
        };
        let dmabuf = tile.window().dmabuf();
        let tile_size = tile.tile_size();
        let snapshot = tile.snapshot_native(renderer).ok();

        self.start_close_animation_for_tile_native(
            renderer, tile_size, tile_pos, blocker, snapshot, dmabuf,
        );
    }

    pub fn start_close_animation_for_tile_native(
        &mut self,
        renderer: &mut crate::render_helpers::vulkan::native::NativeRenderer,
        tile_size: Size<f64, Logical>,
        tile_pos: Point<f64, Logical>,
        blocker: TransactionBlocker,
        snapshot: Option<super::tile::NativeTileSnapshot>,
        dmabuf: Option<smithay::backend::allocator::dmabuf::Dmabuf>,
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
        let res = ClosingWindow::new_native(
            renderer,
            scale,
            tile_size,
            tile_pos,
            blocker,
            anim,
            self.options.animations.window_close.glide,
            self.options.animations.window_close.effect,
            self.options
                .animations
                .window_close
                .custom_shader
                .as_deref(),
            snapshot,
            dmabuf,
        );
        match res {
            Ok(closing) => {
                self.closing_windows.push(closing);
            }
            Err(err) => {
                warn!("error creating a native closing window animation: {err:?}");
            }
        }
    }

    fn start_close_animation_for_tile(
        &mut self,
        renderer: &mut GlesRenderer,
        snapshot: TileRenderSnapshot,
        tile_size: Size<f64, Logical>,
        tile_pos: Point<f64, Logical>,
        blocker: TransactionBlocker,
        dmabuf: Option<smithay::backend::allocator::dmabuf::Dmabuf>,
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
        let is_vulkan = self.options.renderer.is_vulkan();
        let res = ClosingWindow::new(
            renderer,
            snapshot,
            scale,
            tile_size,
            tile_pos,
            blocker,
            anim,
            self.options.animations.window_close.glide,
            self.options.animations.window_close.effect,
            self.options.animations.window_close.custom_shader.is_some(),
            is_vulkan,
            dmabuf,
        );
        match res {
            Ok(closing) => {
                self.closing_windows.push(closing);
            }
            Err(err) => {
                warn!("error creating a closing window animation: {err:?}");
            }
        }
    }

    /// Toggle manual split direction preference for the next split.
    pub fn toggle_split_direction(&mut self) {
        self.tree.toggle_split_direction();
    }

    pub fn update_config(
        &mut self,
        view_size: Size<f64, Logical>,
        working_area: Rectangle<f64, Logical>,
        scale: f64,
        options: Rc<Options>,
    ) {
        self.view_size = view_size;
        self.working_area = working_area;
        self.scale = scale;
        if let Some(root) = &mut self.payloads {
            root.for_each_tile_mut(&mut |tile| {
                tile.update_config(view_size, scale, options.clone());
            });
        }
        self.options = options;
        self.relayout();
    }

    pub fn advance_animations(&mut self) {
        if let Some(ref mut root) = self.payloads {
            root.advance_animations();
        }

        self.closing_windows.retain_mut(|closing| {
            closing.advance_animations();
            closing.are_animations_ongoing()
        });
    }

    pub fn are_animations_ongoing(&self) -> bool {
        self.payloads
            .as_ref()
            .is_some_and(|r| r.are_animations_ongoing())
            || !self.closing_windows.is_empty()
    }

    pub fn are_transitions_ongoing(&self) -> bool {
        self.payloads
            .as_ref()
            .is_some_and(TileStore::are_transitions_ongoing)
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

    /// Recompute geometry for the whole tree according to working area and gaps.
    pub fn relayout(&mut self) {
        self.relayout_with_animation(true);
    }

    fn relayout_with_animation(&mut self, animate: bool) {
        self.arrange_geometry();
        if let Some(payloads) = &mut self.payloads {
            payloads.request_sizes(self.working_area, animate, &Transaction::new());
        }
    }

    fn arrange_geometry(&mut self) {
        let payloads = &mut self.payloads;
        self.tree.arrange(
            BspArrangement {
                bounds: core_rect(self.working_area),
                gap: self.options.layout.gaps,
            },
            |id, rect| {
                if let Some(payloads) = payloads {
                    payloads.apply_placement(id, rect);
                }
            },
        );
    }

    /// Add a new tile into the dwindle tree using smart split.
    pub fn add_tile(&mut self, tile: Tile<W>, activate: ActivateWindow) {
        self.insert_tile(tile, activate, self.tree.active().copied());
        self.relayout();
    }

    /// Split the requested tile without changing focus unless activation was requested.
    pub fn add_tile_next_to(&mut self, next_to: &W::Id, tile: Tile<W>, activate: ActivateWindow) {
        let target = if self.has_window(next_to) {
            self.core_id(next_to)
        } else {
            self.tree.active().copied()
        };
        self.insert_tile(tile, activate, target);
        self.relayout();
    }

    /// Add several tiles with a single resize request pass.
    pub fn add_tiles(
        &mut self,
        tiles: impl IntoIterator<Item = Tile<W>>,
        activate: ActivateWindow,
    ) {
        // Restored session trees start with empty geometries. Smart splitting needs the actual
        // target rectangle even though client size requests remain deferred until the end.
        self.arrange_geometry();
        for tile in tiles {
            self.insert_tile(tile, activate, self.tree.active().copied());
        }
        self.relayout();
    }

    fn insert_tile(&mut self, mut tile: Tile<W>, activate: ActivateWindow, target_id: Option<u64>) {
        tile.update_config(self.view_size, self.scale, self.options.clone());
        let core_id = tile.window().core_id();
        let target_core = target_id;
        let geometry = self
            .tree
            .insertion_bounds(target_core.as_ref())
            .expect("BSP depth bound must leave a split target");
        assert!(
            self.tree.insert(core_id, target_core.as_ref(), None),
            "BSP window IDs must be unique and within depth capacity"
        );
        self.payloads.get_or_insert_with(TileStore::new).insert(
            tile,
            platform_rect(geometry),
            &self.tree,
        );
        if activate == ActivateWindow::Yes {
            self.tree.activate(&core_id);
        }
    }

    /// Core topology decides removal, parent collapse and the nearest sibling focus.
    pub fn remove_window(&mut self, id: &W::Id) -> Option<RemovedTile<W>> {
        let core_id = self.core_id(id)?;
        self.tree.remove(&core_id)?;
        let tile = self.payloads.as_mut()?.remove(id, &self.tree)?;
        if self
            .interactive_resize
            .as_ref()
            .is_some_and(|resize| resize.resize.id == core_id)
        {
            self.interactive_resize = None;
        }
        if self.tree.is_empty() {
            self.payloads = None;
        }
        self.relayout();
        Some(RemovedTile {
            tile: *tile,
            width: ColumnWidth::Proportion(1.0),
            is_full_width: false,
            is_floating: false,
        })
    }

    /// Focus direction: left.
    pub fn focus_left(&mut self) -> bool {
        self.focus_directional(Direction::Left)
    }

    /// Focus direction: right.
    pub fn focus_right(&mut self) -> bool {
        self.focus_directional(Direction::Right)
    }

    /// Focus direction: up.
    pub fn focus_up(&mut self) -> bool {
        self.focus_directional(Direction::Up)
    }

    /// Focus direction: down.
    pub fn focus_down(&mut self) -> bool {
        self.focus_directional(Direction::Down)
    }

    fn focus_directional(&mut self, direction: Direction) -> bool {
        self.tree.focus_neighbor(direction)
    }

    pub fn focus_top(&mut self) {
        self.focus_extreme(Direction::Up);
    }

    pub fn focus_bottom(&mut self) {
        self.focus_extreme(Direction::Down);
    }

    pub fn focus_leftmost(&mut self) {
        self.focus_extreme(Direction::Left);
    }

    pub fn focus_rightmost(&mut self) {
        self.focus_extreme(Direction::Right);
    }

    fn focus_extreme(&mut self, direction: Direction) {
        self.tree.focus_extreme(direction);
    }

    pub fn move_left(&mut self) -> bool {
        self.move_directional(Direction::Left)
    }

    pub fn move_right(&mut self) -> bool {
        self.move_directional(Direction::Right)
    }

    pub fn move_up(&mut self) -> bool {
        self.move_directional(Direction::Up)
    }

    pub fn move_down(&mut self) -> bool {
        self.move_directional(Direction::Down)
    }

    fn move_directional(&mut self, direction: Direction) -> bool {
        let Some(active) = self.tree.active().copied() else {
            return false;
        };
        let Some(target) = self.tree.neighbor(&active, direction) else {
            return false;
        };
        if !self.tree.swap(&active, &target) {
            return false;
        }
        self.payloads.as_mut().unwrap().reorder(&self.tree);
        self.relayout();
        true
    }

    pub fn interactive_resize_begin(&mut self, window: W::Id, edges: ResizeEdge) -> bool {
        if self.interactive_resize.is_some() || !self.has_window(&window) {
            return false;
        }
        if self
            .payloads
            .as_ref()
            .and_then(|root| root.find_tile(&window))
            .is_some_and(|tile| !tile.window().pending_sizing_mode().is_normal())
        {
            return false;
        }
        let id = self.core_id(&window).unwrap();
        let size = self.tree.geometry(&id).unwrap().size();
        self.interactive_resize = Some(realand_core::bsp::SplitResize::new(
            realand_core::resize::Resize::new(id, size, edges.bits()),
        ));

        true
    }

    pub fn interactive_resize_update(
        &mut self,
        window: &W::Id,
        delta: Point<f64, Logical>,
    ) -> bool {
        let Some(id) = self.core_id(window) else {
            return false;
        };
        let Some(resize) = &mut self.interactive_resize else {
            return false;
        };
        if id != resize.resize.id {
            return false;
        }
        let Some(applied) = resize.delta([delta.x, delta.y]) else {
            return false;
        };
        let edges = ResizeEdge::from_bits_retain(resize.resize.edges);
        self.resize_splits(window, edges, Point::from((applied[0], applied[1])))
    }

    pub fn interactive_resize_end(&mut self, window: Option<&W::Id>) {
        if let Some(window) = window {
            if self.core_id(window).is_some_and(|id| {
                self.interactive_resize
                    .as_ref()
                    .is_some_and(|resize| resize.resize.id == id)
            }) {
                self.interactive_resize = None;
            }
        } else {
            self.interactive_resize = None;
        }
    }

    pub fn set_window_width(&mut self, window: Option<&W::Id>, change: SizeChange) {
        let Some(id) = window
            .cloned()
            .or_else(|| self.tree.active().and_then(|id| self.physical_id(*id)))
        else {
            return;
        };
        let Some((_, geom)) = self.payloads.as_ref().and_then(|root| root.find_leaf(&id)) else {
            return;
        };
        let current = geom.size.w;
        let target = match change {
            SizeChange::SetFixed(w) => f64::from(w),
            SizeChange::AdjustFixed(d) => current + f64::from(d),
            SizeChange::SetProportion(p) => self.working_area.size.w * (p / 100.),
            SizeChange::AdjustProportion(p) => current + self.working_area.size.w * (p / 100.),
        };
        let delta = target - current;
        if !self.resize_splits(&id, ResizeEdge::RIGHT, Point::from((delta, 0.))) {
            self.resize_splits(&id, ResizeEdge::LEFT, Point::from((-delta, 0.)));
        }
    }

    pub fn set_window_height(&mut self, window: Option<&W::Id>, change: SizeChange) {
        let Some(id) = window
            .cloned()
            .or_else(|| self.tree.active().and_then(|id| self.physical_id(*id)))
        else {
            return;
        };
        let Some((_, geom)) = self.payloads.as_ref().and_then(|root| root.find_leaf(&id)) else {
            return;
        };
        let current = geom.size.h;
        let target = match change {
            SizeChange::SetFixed(h) => f64::from(h),
            SizeChange::AdjustFixed(d) => current + f64::from(d),
            SizeChange::SetProportion(p) => self.working_area.size.h * (p / 100.),
            SizeChange::AdjustProportion(p) => current + self.working_area.size.h * (p / 100.),
        };
        let delta = target - current;
        if !self.resize_splits(&id, ResizeEdge::BOTTOM, Point::from((0., delta))) {
            self.resize_splits(&id, ResizeEdge::TOP, Point::from((0., -delta)));
        }
    }

    fn resize_splits(&mut self, id: &W::Id, edges: ResizeEdge, delta: Point<f64, Logical>) -> bool {
        let Some(core_id) = self.core_id(id) else {
            return false;
        };
        let applied = self.tree.resize(&core_id, edges.bits(), delta.x, delta.y);
        if applied != 0 {
            self.relayout_with_animation(self.interactive_resize.is_none());
        }
        applied != 0
    }

    /// Extract all tiles to allow migrating to another layout (e.g. scrolling).
    pub fn drain_tiles(&mut self) -> Vec<Tile<W>> {
        let mut tiles = Vec::new();
        if let Some(payloads) = self.payloads.take() {
            payloads.collect_tiles(&mut tiles);
        }
        self.tree.clear();
        self.interactive_resize = None;
        tiles
    }

    /// Find window under logical position.
    pub fn window_under(&self, pos: Point<f64, Logical>) -> Option<(&W, HitType)> {
        // Use the same rounded position as rendering, including normal-pane movement and
        // ignoring stale tree movement for expanded panes.
        if let Some(hit) = self
            .tiles_with_render_positions()
            .filter(|(_, _, visible)| *visible)
            .find_map(|(tile, origin, _)| HitType::hit_tile(tile, origin, pos))
        {
            return Some(hit);
        }
        if let Some((id, origin)) = self.expanded_render() {
            let origin = origin
                .to_physical_precise_round(self.scale)
                .to_logical(self.scale);
            let (tile, _) = self.payloads.as_ref()?.find_leaf(&id)?;
            let size = if tile.window().pending_sizing_mode().is_fullscreen()
                || tile.sizing_mode().is_fullscreen()
            {
                self.view_size
            } else {
                self.working_area.size
            };
            return Rectangle::new(origin, size).contains(pos).then_some((
                tile.window(),
                HitType::Activate {
                    is_tab_indicator: false,
                },
            ));
        }
        None
    }

    /// Render tiles with their decoration elements.
    pub fn render<R: RealandRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        xray_pos: XrayPos,
        _view_rect: Rectangle<f64, Logical>,
        focus_ring: bool,
        layer: RenderLayer,
        push: &mut dyn FnMut(DwindleSpaceRenderElement<R>),
    ) {
        let scale = Scale::from(self.scale);

        // Draw the closing windows on top of the other windows.
        if layer.is_normal() {
            let view_rect = Rectangle::from_size(self.view_size);
            for closing in self.closing_windows.iter().rev() {
                for elem in closing.render(ctx.r(), view_rect, scale) {
                    push(elem.into());
                }
            }
        }

        for (tile, pos, visible) in self.tiles_with_render_positions() {
            if !visible || layer.is_normal() == tile.is_moving_between_workspaces() {
                continue;
            }
            let is_focused =
                focus_ring && Some(tile.window().core_id()) == self.tree.active().copied();
            let xray_pos = xray_pos.offset(pos);
            tile.render(ctx.r(), pos, xray_pos, is_focused, &mut |elem| {
                push(elem.into());
            });
        }
    }
}

fn compute_toplevel_bounds(
    border_config: realand_config::Border,
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

impl<W: LayoutElement> DwindleSpace<W> {
    #[cfg(test)]
    pub(super) fn session_tree(
        &self,
        index: &impl Fn(&W::Id) -> usize,
    ) -> Option<realand_ipc::management::SessionTree> {
        use realand_ipc::management::SessionTree;
        fn encode<I>(node: &BspNode<I>, index: &impl Fn(&I) -> usize) -> SessionTree {
            match node {
                BspNode::Leaf(id) => SessionTree::Leaf(index(id)),
                BspNode::Split {
                    axis,
                    ratio,
                    first,
                    second,
                } => SessionTree::Split {
                    vertical: *axis == realand_core::Axis::Vertical,
                    ratio: *ratio,
                    first: Box::new(encode(first, index)),
                    second: Box::new(encode(second, index)),
                },
            }
        }
        self.tree.topology().map(|tree| {
            encode(&tree, &|id| {
                index(
                    &self
                        .physical_id(*id)
                        .expect("core BSP pane must have a device tile"),
                )
            })
        })
    }

    fn restore_topology<J>(
        &mut self,
        topology: Option<&BspNode<J>>,
        mut resolve: impl FnMut(&J) -> Option<u64>,
        reset_geometry: bool,
    ) -> bool {
        let active = self.tree.active().copied();
        let available: std::collections::HashSet<_> =
            self.tiles().map(|tile| tile.window().core_id()).collect();
        if let Err(error) = self.tree.restore_mapped(topology, |source| {
            resolve(source).filter(|id| available.contains(id))
        }) {
            warn!("ignoring invalid BSP topology: {error:?}");
            return false;
        }
        let extra = self
            .payloads
            .as_mut()
            .map(|payloads| payloads.retain_placed(&self.tree, reset_geometry))
            .unwrap_or_default();
        for tile in extra {
            self.insert_tile(tile, ActivateWindow::No, active);
        }
        if self.tree.is_empty() {
            self.payloads = None;
        }
        true
    }

    pub(super) fn restore_session_tree(
        &mut self,
        tree: &realand_ipc::management::SessionTree,
        handles: &[Option<W::Id>],
    ) {
        use realand_ipc::management::SessionTree;
        fn decode(node: &SessionTree) -> BspNode<usize> {
            match node {
                SessionTree::Leaf(index) => BspNode::Leaf(*index),
                SessionTree::Split {
                    vertical,
                    ratio,
                    first,
                    second,
                } => BspNode::Split {
                    axis: if *vertical {
                        realand_core::Axis::Vertical
                    } else {
                        realand_core::Axis::Horizontal
                    },
                    ratio: *ratio,
                    first: Box::new(decode(first)),
                    second: Box::new(decode(second)),
                },
            }
        }
        let active = self.tree.active().copied();
        let core_ids: HashMap<_, _> = self
            .tiles()
            .map(|tile| (tile.window().id().clone(), tile.window().core_id()))
            .collect();
        if !self.restore_topology(
            Some(&decode(tree)),
            |index| {
                handles
                    .get(*index)?
                    .as_ref()
                    .and_then(|id| core_ids.get(id).copied())
            },
            true,
        ) {
            return;
        }
        self.tree.restore_focus(active.as_ref());
        self.interactive_resize = None;
        self.relayout();
    }

    pub(super) fn policy_snapshot(&self) -> BspSnapshot<u64> {
        BspSnapshot {
            topology: self.tree.topology(),
            active: self.tree.active().copied(),
            expanded: self
                .tiles()
                .filter_map(|tile| match tile.window().pending_sizing_mode() {
                    SizingMode::Normal => None,
                    SizingMode::Fullscreen => Some((tile.window().core_id(), PaneMode::Fullscreen)),
                    SizingMode::Maximized => Some((tile.window().core_id(), PaneMode::Maximized)),
                })
                .collect(),
        }
    }
    /// Restore the Stage drag's numeric topology without reverting expanded
    /// modes changed on other windows while this interaction was active.
    pub(super) fn restore_stage_topology(&mut self, snapshot: &BspSnapshot<u64>) {
        if self.restore_topology(snapshot.topology.as_ref(), |id| Some(*id), false) {
            self.relayout();
        }
    }

    pub(super) fn restore_policy(&mut self, snapshot: &BspSnapshot<u64>) {
        if !self.restore_topology(snapshot.topology.as_ref(), |id| Some(*id), false) {
            return;
        }
        self.tree.restore_focus(snapshot.active.as_ref());
        let mut known = std::collections::HashSet::new();
        let mut pending = snapshot.topology.iter().collect::<Vec<_>>();
        while let Some(node) = pending.pop() {
            match node {
                BspNode::Leaf(id) => {
                    known.insert(id);
                }
                BspNode::Split { first, second, .. } => {
                    pending.push(first);
                    pending.push(second);
                }
            }
        }
        self.interactive_resize = None;
        self.arrange_geometry();
        if let Some(payloads) = &mut self.payloads {
            for slot in &mut payloads.slots {
                let tile = &mut slot.tile;
                if !known.contains(&tile.window().core_id()) {
                    continue;
                }
                let mode = snapshot
                    .expanded
                    .iter()
                    .find(|(id, _)| *id == tile.window().core_id())
                    .map(|(_, mode)| *mode)
                    .unwrap_or(PaneMode::Normal);
                match mode {
                    PaneMode::Normal => {
                        tile.request_tile_size(slot.geometry.size, false, Some(Transaction::new()))
                    }
                    PaneMode::Fullscreen => {
                        tile.request_fullscreen(false, Some(Transaction::new()))
                    }
                    PaneMode::Maximized => tile.request_maximized(
                        self.working_area.size,
                        false,
                        Some(Transaction::new()),
                    ),
                }
            }
        }
        self.relayout_with_animation(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_direction_toggle() {
        let dir = SplitDirection::Horizontal;
        assert_eq!(dir.toggle(), SplitDirection::Vertical);
        assert_eq!(dir.toggle().toggle(), SplitDirection::Horizontal);
    }

    #[test]
    fn test_smart_split_ratio_math() {
        let area: Rectangle<f64, Logical> =
            Rectangle::new(Point::from((0.0, 0.0)), Size::from((1920.0, 1080.0)));
        let gap: f64 = 20.0;
        let total_w = (area.size.w - gap).max(1.0);
        let w1 = (total_w * 0.5).round();
        let w2 = total_w - w1;
        assert_eq!(w1, 950.0);
        assert_eq!(w2, 950.0);

        // Child node has width 950 and height 1080 -> height > width -> Vertical split (top/bottom)
        let total_h = (1080.0_f64 - gap).max(1.0);
        let h1 = (total_h * 0.5).round();
        let h2 = total_h - h1;
        assert_eq!(h1, 530.0);
        assert_eq!(h2, 530.0);
    }
}
