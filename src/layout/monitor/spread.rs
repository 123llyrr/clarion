use super::*;
use crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement;
use crate::render_helpers::texture::TextureRenderElement;

#[derive(Debug, Clone, Copy)]
pub(super) struct SpreadPose {
    loc: Point<f64, Logical>,
    zoom: f64,
}

impl SpreadPose {
    fn interpolate(self, target: Self, fraction: f64, view_size: Size<f64, Logical>) -> Self {
        if fraction == 0. || !fraction.is_finite() {
            return self;
        }
        if fraction == 1. {
            return target;
        }
        // A reversal first follows its incoming velocity. Its progress relative
        // to the captured pose can be negative, so clamping to that pose would
        // freeze every window until the trajectory turns around. Bound that
        // continuation by visible size and one viewport beyond its endpoints.
        let mut fraction = fraction;
        if !(0. ..=1.).contains(&fraction) {
            for (start, end, min, max) in [
                (
                    self.zoom,
                    target.zoom,
                    0.,
                    1_f64.max(self.zoom).max(target.zoom),
                ),
                (
                    self.loc.x,
                    target.loc.x,
                    self.loc.x.min(target.loc.x) - view_size.w,
                    self.loc.x.max(target.loc.x) + view_size.w,
                ),
                (
                    self.loc.y,
                    target.loc.y,
                    self.loc.y.min(target.loc.y) - view_size.h,
                    self.loc.y.max(target.loc.y) + view_size.h,
                ),
            ] {
                let delta = end - start;
                if delta > 0. {
                    fraction = fraction.clamp((min - start) / delta, (max - start) / delta);
                } else if delta < 0. {
                    fraction = fraction.clamp((max - start) / delta, (min - start) / delta);
                }
            }
        }
        Self {
            loc: self.loc + (target.loc - self.loc).upscale(fraction),
            zoom: (self.zoom + (target.zoom - self.zoom) * fraction).max(0.),
        }
    }
}

#[derive(Debug)]
struct SpreadCapture<I> {
    time: Duration,
    progress: f64,
    poses: Vec<(I, SpreadPose)>,
}

#[derive(Debug)]
pub(super) struct SpreadTransition<I> {
    start_progress: f64,
    target_progress: f64,
    is_gesture: bool,
    start_poses: Vec<(I, SpreadPose)>,
    pending_poses: Option<SpreadCapture<I>>,
    closing_focus: Option<I>,
}

#[derive(Debug)]
pub(super) struct SpreadClosing {
    workspace: WorkspaceId,
    start_progress: f64,
    start: SpreadPose,
    grid: SpreadPose,
    source_pos: Point<f64, Logical>,
}

impl<W: LayoutElement> Monitor<W> {
    pub(in crate::layout) fn reconcile_spread_candidates(&self, force_metadata: bool) {
        use realand_core::spread::{SpreadAction, SpreadCandidate};
        let core = self.core.borrow();
        let output = core.spread().output(&self.output_name);
        let metadata = force_metadata || output.is_some_and(|output| !output.query().is_empty());
        if !metadata
            && output.is_some_and(|output| {
                output.order().iter().copied().eq(self
                    .workspaces
                    .iter()
                    .flat_map(|workspace| workspace.tiles())
                    .map(|tile| tile.window().core_id()))
            })
        {
            return;
        }
        drop(core);
        let windows = self
            .workspaces
            .iter()
            .flat_map(|workspace| workspace.tiles())
            .map(|tile| SpreadCandidate {
                id: tile.window().core_id(),
                search_text: metadata.then(|| {
                    let (app, title) = tile.window().session_identity();
                    format!("{app} {title}")
                }),
            })
            .collect();
        self.core
            .borrow_mut()
            .spread_action(SpreadAction::Candidates {
                connector: self.output_name.clone(),
                windows,
            });
    }

    /// Search results determine the grid and keyboard selection.
    /// Logical tile order also includes windows hidden behind a fullscreen Dwindle tile,
    /// and does not change when a tab becomes active.
    pub(in crate::layout) fn spread_matching_tiles(&self) -> impl Iterator<Item = &Tile<W>> {
        let core = self.core.borrow();
        let output_name = &self.output_name;
        self.workspaces
            .iter()
            .flat_map(|ws| ws.tiles())
            .filter(move |tile| {
                core.spread()
                    .output(output_name)
                    .is_none_or(|output| output.matches(&tile.window().core_id()))
            })
    }

    /// Returning desktop windows remain visible even if search excluded them from the grid.
    /// Keep captured windows during a reversal too, so they can shrink back out continuously.
    pub(super) fn spread_render_tiles(&self) -> impl Iterator<Item = &Tile<W>> {
        let core = self.core.borrow();
        let output_name = &self.output_name;
        let p = self.overview_backdrop_progress();
        let transition = self.spread_transition.as_ref();
        let returning = transition.is_some_and(|t| {
            if t.is_gesture {
                p < t.start_progress
            } else {
                t.target_progress == 0.
            }
        });
        // Look up captured poses once instead of scanning them for every tile.
        let captured: std::collections::HashSet<&W::Id> = if p < 1. {
            transition
                .map(|t| {
                    t.start_poses
                        .iter()
                        .filter(|(_, pose)| pose.zoom > 0.)
                        .map(|(id, _)| id)
                        .collect()
                })
                .unwrap_or_default()
        } else {
            Default::default()
        };
        self.workspaces
            .iter()
            .flat_map(|ws| ws.tiles())
            .filter(move |tile| {
                returning
                    || core
                        .spread()
                        .output(output_name)
                        .is_none_or(|output| output.matches(&tile.window().core_id()))
                    || captured.contains(tile.window().id())
            })
    }

    fn spread_grid_tiles(&self) -> Vec<(&Tile<W>, Rectangle<f64, Logical>, f64)> {
        let tiles: Vec<_> = self.spread_matching_tiles().collect();
        let cols = ((tiles.len() as f64 * self.view_size.w / self.view_size.h.max(1.))
            .sqrt()
            .ceil() as usize)
            .max(1);
        let rows = tiles.len().div_ceil(cols).max(1);
        let cw = ((self.view_size.w - 24.) / cols as f64).max(1.);
        let ch = ((self.view_size.h - 80.) / rows as f64).max(1.);
        tiles
            .into_iter()
            .enumerate()
            .map(|(i, tile)| {
                let size = tile.tile_size();
                let zoom = ((cw - 24.).max(1.) / size.w.max(1.))
                    .min((ch - 24.).max(1.) / size.h.max(1.))
                    .min(1.);
                let size = size.upscale(zoom);
                let loc = Point::from((
                    12. + (i % cols) as f64 * cw + (cw - size.w) / 2.,
                    68. + (i / cols) as f64 * ch + (ch - size.h) / 2.,
                ));
                (tile, Rectangle::new(loc, size), zoom)
            })
            .collect()
    }

    pub(super) fn is_spread_visible(&self) -> bool {
        self.overview_mode() == OverviewMode::Windows
            && (self.is_overview_open() || self.overview_progress.is_some())
    }

    /// Positions in the desktop scene, including hidden tabs and off-screen workspaces.
    /// Closing uses the live, unzoomed workspace switch so its final frame agrees with
    /// the ordinary desktop even when focus changes while the overview is closing.
    fn spread_desktop_poses(&self, current_scene: bool) -> Vec<(W::Id, SpreadPose)> {
        let render_idx = self.workspace_switch_idx();
        let geometry = self.workspaces_render_geo();
        self.workspaces
            .iter()
            .zip(geometry)
            .enumerate()
            .flat_map(|(idx, (ws, geo))| {
                let zoom = if current_scene {
                    self.workspace_render_zoom(geo)
                } else {
                    1.
                };
                let loc = if current_scene {
                    geo.loc
                } else {
                    Point::from((
                        0.,
                        (idx as f64 - render_idx) * (self.view_size.h + self.workspace_gap(1.)),
                    ))
                    .to_physical_precise_round(self.scale.fractional_scale())
                    .to_logical(self.scale.fractional_scale())
                };
                ws.tiles_with_render_positions()
                    .map(move |(tile, pos, visible)| {
                        let mut pose = SpreadPose {
                            loc: loc + pos.upscale(zoom),
                            zoom,
                        };
                        if !visible {
                            pose.loc += tile.animated_tile_size().to_point().upscale(zoom / 2.);
                            pose.zoom = 0.;
                        }
                        (tile.window().id().clone(), pose)
                    })
            })
            .collect()
    }

    pub(in crate::layout) fn spread_tiles(&self) -> Vec<(&Tile<W>, Rectangle<f64, Logical>, f64)> {
        let grid = self.spread_grid_tiles();
        let Some(transition) = &self.spread_transition else {
            return grid;
        };
        let p = self
            .overview_progress
            .as_ref()
            .map_or(1., OverviewProgress::clamped_value)
            .clamp(0., 1.);
        if !transition.is_gesture && transition.target_progress == 1. && p >= 1. {
            return grid;
        }
        let target_progress = if transition.is_gesture {
            if p >= transition.start_progress {
                1.
            } else {
                0.
            }
        } else {
            transition.target_progress
        };
        let fraction = if transition.start_progress == target_progress {
            1.
        } else {
            (p - transition.start_progress) / (target_progress - transition.start_progress)
        };
        let desktop = self.spread_desktop_poses(false);
        // Index all three pose sources once; per-tile linear finds make every
        // transition frame quadratic in the window count.
        let desktop_map: std::collections::HashMap<&W::Id, SpreadPose> =
            desktop.iter().map(|(id, pose)| (id, *pose)).collect();
        let grid_map: std::collections::HashMap<&W::Id, SpreadPose> = grid
            .iter()
            .map(|(tile, geo, zoom)| {
                (
                    tile.window().id(),
                    SpreadPose {
                        loc: geo.loc,
                        zoom: *zoom,
                    },
                )
            })
            .collect();
        let start_map: std::collections::HashMap<&W::Id, SpreadPose> = transition
            .start_poses
            .iter()
            .map(|(id, pose)| (id, *pose))
            .collect();
        self.spread_render_tiles()
            .map(|tile| {
                let id = tile.window().id();
                let grid_pose = grid_map.get(id).copied();
                // Parked minimized windows remain searchable but have no
                // desktop pose. Appear from and return to a zero-sized preview
                // instead of requiring ordinary layout geometry for a hidden ID.
                let desktop_pose = desktop_map.get(id).copied().unwrap_or_else(|| {
                    let center =
                        grid_pose.map_or(self.view_size.to_point().downscale(2.), |pose| {
                            pose.loc + tile.animated_tile_size().to_point().upscale(pose.zoom / 2.)
                        });
                    SpreadPose {
                        loc: center,
                        zoom: 0.,
                    }
                });
                let target = if target_progress == 1. {
                    grid_pose.unwrap_or(SpreadPose {
                        loc: desktop_pose.loc + tile.animated_tile_size().to_point().downscale(2.),
                        zoom: 0.,
                    })
                } else {
                    desktop_pose
                };
                let start = start_map.get(id).copied().unwrap_or(SpreadPose {
                    loc: target.loc
                        + tile
                            .animated_tile_size()
                            .to_point()
                            .upscale(target.zoom / 2.),
                    zoom: 0.,
                });
                let pose = start.interpolate(target, fraction, self.view_size);
                let loc = pose
                    .loc
                    .to_physical_precise_round(self.scale.fractional_scale())
                    .to_logical(self.scale.fractional_scale());
                (
                    tile,
                    Rectangle::new(loc, tile.animated_tile_size().upscale(pose.zoom)),
                    pose.zoom,
                )
            })
            .collect()
    }

    pub(super) fn spread_draw_tiles(&self) -> Vec<(&Tile<W>, Rectangle<f64, Logical>, f64)> {
        let mut tiles = self.spread_tiles();
        if let Some(transition) = &self.spread_transition {
            let positions: std::collections::HashMap<&W::Id, usize> = transition
                .start_poses
                .iter()
                .enumerate()
                .map(|(index, (id, _))| (id, index))
                .collect();
            tiles.sort_by_cached_key(|(tile, _, _)| {
                positions
                    .get(tile.window().id())
                    .copied()
                    .unwrap_or(usize::MAX)
            });
        }
        tiles
    }

    /// Capture before focus activation changes a tab, fullscreen pane or workspace.
    pub(in crate::layout) fn capture_spread_frame(&mut self) {
        let p = self.overview_backdrop_progress();
        let closing_poses: Vec<_> = self
            .spread_closing
            .iter()
            .map(|(id, origin)| (id.clone(), self.closing_pose(origin)))
            .collect();
        for (id, pose) in closing_poses {
            let origin = self.spread_closing.get_mut(&id).unwrap();
            origin.start_progress = p;
            origin.start = pose;
        }
        let poses = self
            .spread_tiles()
            .into_iter()
            .map(|(tile, rect, zoom)| {
                (
                    tile.window().id().clone(),
                    SpreadPose {
                        loc: rect.loc,
                        zoom,
                    },
                )
            })
            .collect();
        if let Some(transition) = &mut self.spread_transition {
            transition.pending_poses = Some(SpreadCapture {
                time: self.clock.now_unadjusted(),
                progress: p,
                poses,
            });
        }
    }

    pub(in crate::layout) fn prepare_spread_close(&mut self) {
        if self.spread_transition.as_ref().is_some_and(|t| {
            t.pending_poses.as_ref().is_none_or(|capture| {
                capture.time != self.clock.now_unadjusted()
                    || capture.progress != self.overview_backdrop_progress()
            })
        }) {
            self.capture_spread_frame();
        }
    }

    pub(super) fn set_spread_progress(
        &mut self,
        progress: Option<&super::super::OverviewProgress>,
    ) {
        if self.overview_mode() != OverviewMode::Windows || progress.is_none() {
            self.spread_transition = None;
            self.spread_closing.clear();
            return;
        }
        let target_progress = if self.is_overview_open() { 1. } else { 0. };
        let is_gesture = matches!(progress, Some(super::super::OverviewProgress::Gesture));
        let focus = self.active_window().map(|window| window.id().clone());
        if self.spread_transition.is_none() {
            self.spread_transition = Some(SpreadTransition {
                start_progress: 0.,
                target_progress,
                is_gesture,
                start_poses: self.spread_desktop_poses(true),
                pending_poses: None,
                closing_focus: focus,
            });
        } else if self.spread_transition.as_ref().is_some_and(|transition| {
            transition.target_progress != target_progress
                || transition.is_gesture != is_gesture
                || (!is_gesture && target_progress == 0. && transition.closing_focus != focus)
        }) {
            self.prepare_spread_close();
            let p = self
                .overview_progress
                .as_ref()
                .map_or(1., OverviewProgress::clamped_value)
                .clamp(0., 1.);
            let transition = self.spread_transition.as_mut().unwrap();
            transition.start_poses = transition.pending_poses.take().unwrap().poses;
            transition.start_progress = p;
            transition.target_progress = target_progress;
            transition.is_gesture = is_gesture;
            transition.closing_focus = focus;
        }
    }

    fn closing_pose(&self, closing: &SpreadClosing) -> SpreadPose {
        let p = self.overview_backdrop_progress();
        let target_progress = if self
            .spread_transition
            .as_ref()
            .is_some_and(|t| t.is_gesture)
        {
            if p >= closing.start_progress {
                1.
            } else {
                0.
            }
        } else {
            self.spread_transition
                .as_ref()
                .map_or(1., |t| t.target_progress)
        };
        let render_idx = self.workspace_switch_idx();
        let idx = self
            .idx_of_ws(closing.workspace)
            .unwrap_or(self.active_workspace_idx());
        let desktop = SpreadPose {
            loc: closing.source_pos
                + Point::from((
                    0.,
                    (idx as f64 - render_idx) * (self.view_size.h + self.workspace_gap(1.)),
                )),
            zoom: 1.,
        };
        let fraction = if closing.start_progress == target_progress {
            1.
        } else {
            (p - closing.start_progress) / (target_progress - closing.start_progress)
        };
        closing.start.interpolate(
            if target_progress == 1. {
                closing.grid
            } else {
                desktop
            },
            fraction,
            self.view_size,
        )
    }

    fn spread_close_origin(&self, window: &W::Id) -> Option<(usize, SpreadClosing)> {
        if !self.is_spread_visible() {
            return None;
        }
        let (_, rect, zoom) = self
            .spread_tiles()
            .into_iter()
            .find(|(tile, _, zoom)| tile.window().id() == window && *zoom > 0.)?;
        let (idx, ws) = self
            .workspaces
            .iter()
            .enumerate()
            .find(|(_, ws)| ws.has_window(window))?;
        let (_, source_pos, _) = ws
            .tiles_with_render_positions()
            .find(|(tile, _, _)| tile.window().id() == window)?;
        let grid = self
            .spread_grid_tiles()
            .into_iter()
            .find(|(tile, _, _)| tile.window().id() == window)
            .map(|(_, geo, zoom)| SpreadPose { loc: geo.loc, zoom })
            .unwrap_or(SpreadPose {
                loc: rect.loc + rect.size.to_point().downscale(2.),
                zoom: 0.,
            });
        Some((
            idx,
            SpreadClosing {
                workspace: ws.id(),
                start_progress: self.overview_backdrop_progress(),
                start: SpreadPose {
                    loc: rect.loc,
                    zoom,
                },
                grid,
                source_pos,
            },
        ))
    }

    pub(in crate::layout) fn start_spread_close(
        &mut self,
        renderer: &mut GlesRenderer,
        window: &W::Id,
        blocker: crate::utils::transaction::TransactionBlocker,
    ) -> bool {
        let Some((idx, origin)) = self.spread_close_origin(window) else {
            return false;
        };
        let ws = &mut self.workspaces[idx];
        let tile = ws
            .tiles_mut()
            .find(|tile| tile.window().id() == window)
            .unwrap();
        let Some(snapshot) = tile.take_unmap_snapshot() else {
            return true;
        };
        let size = tile.tile_size();
        let dmabuf = tile.window().dmabuf();
        let before = ws.closing_windows().count();
        ws.start_close_animation_for_tile(
            renderer,
            snapshot,
            size,
            origin.source_pos,
            blocker,
            dmabuf,
        );
        if ws.closing_windows().count() > before {
            self.spread_closing.insert(
                ws.closing_windows().last().unwrap().stable_id().clone(),
                origin,
            );
        }
        true
    }

    pub(in crate::layout) fn start_spread_close_native(
        &mut self,
        renderer: &mut NativeRenderer,
        window: &W::Id,
        blocker: crate::utils::transaction::TransactionBlocker,
    ) -> bool {
        let Some((idx, origin)) = self.spread_close_origin(window) else {
            return false;
        };
        let ws = &mut self.workspaces[idx];
        let tile = ws
            .tiles_mut()
            .find(|tile| tile.window().id() == window)
            .unwrap();
        let snapshot = tile.snapshot_native(renderer).ok();
        let size = tile.tile_size();
        let dmabuf = tile.window().dmabuf();
        let before = ws.closing_windows().count();
        ws.start_close_animation_for_tile_native(
            renderer,
            size,
            origin.source_pos,
            blocker,
            snapshot,
            dmabuf,
        );
        if ws.closing_windows().count() > before {
            self.spread_closing.insert(
                ws.closing_windows().last().unwrap().stable_id().clone(),
                origin,
            );
        }
        true
    }

    /// Opening the grid should preserve the user's current focus on each output.
    pub(in crate::layout) fn reset_spread(&mut self) {
        self.reconcile_spread_candidates(false);
        let preferred = self.active_window().map(LayoutElement::core_id);
        self.core
            .borrow_mut()
            .spread_action(realand_core::spread::SpreadAction::Reset {
                connector: self.output_name.clone(),
                preferred,
            });
        self.spread_transition = Some(SpreadTransition {
            start_progress: 0.,
            target_progress: 1.,
            is_gesture: false,
            start_poses: self.spread_desktop_poses(true),
            pending_poses: None,
            closing_focus: self.active_window().map(|window| window.id().clone()),
        });
    }

    pub(crate) fn spread_selected(&self) -> Option<W::Id> {
        let selected = self
            .core
            .borrow()
            .spread()
            .output(&self.output_name)
            .and_then(|output| output.selected())
            .copied()?;
        self.workspaces
            .iter()
            .flat_map(|workspace| workspace.tiles())
            .find(|tile| tile.window().core_id() == selected)
            .map(|tile| tile.window().id().clone())
    }

    pub(super) fn update_spread_render_elements(&mut self) {
        let selected = self.spread_selected();
        let ids: Vec<_> = self
            .spread_render_tiles()
            .map(|tile| tile.window().id().clone())
            .collect();
        let tiles: Vec<_> = self
            .workspaces
            .iter_mut()
            .flat_map(|ws| ws.tiles_mut())
            .filter(|tile| ids.contains(tile.window().id()))
            .collect();
        for tile in tiles {
            // Workspace decorations are not drawn in the spread. Updating the same tile first
            // in workspace coordinates would change its highlight and damage it again every frame.
            tile.update_render_elements(
                selected.as_ref() == Some(tile.window().id()),
                Rectangle::from_size(tile.tile_size()),
            );
        }
    }

    #[allow(non_upper_case_globals)]
    pub(crate) fn spread_key(&mut self, key: smithay::input::keyboard::Keysym) {
        use realand_core::spread::{SpreadAction, SpreadKey};
        use smithay::input::keyboard::keysyms::*;
        // Titles/app IDs are read once at search input. Ordinary unfiltered
        // overview frames and mutation edges do not allocate metadata strings.
        self.reconcile_spread_candidates(true);
        let count = self
            .core
            .borrow()
            .spread()
            .output(&self.output_name)
            .map_or(0, |output| output.matching().len());
        let columns = ((count as f64 * self.view_size.w / self.view_size.h.max(1.))
            .sqrt()
            .ceil() as usize)
            .max(1);
        let key = match key.raw() {
            KEY_Left => Some(SpreadKey::Left),
            KEY_Right => Some(SpreadKey::Right),
            KEY_Up => Some(SpreadKey::Up),
            KEY_Down => Some(SpreadKey::Down),
            KEY_Tab => Some(SpreadKey::Next),
            KEY_BackSpace => Some(SpreadKey::Backspace),
            _ => key
                .key_char()
                .filter(|c| !c.is_control())
                .map(SpreadKey::Character),
        };
        if let Some(key) = key {
            self.core.borrow_mut().spread_action(SpreadAction::Key {
                connector: self.output_name.clone(),
                key,
                columns,
            });
        }
    }

    pub(super) fn render_spread<R: RealandRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        push: &mut dyn FnMut(MonitorRenderElement<R>),
    ) {
        let scale = self.scale.fractional_scale();
        for ws in &self.workspaces {
            for closing in ws.closing_windows() {
                let pose = self
                    .spread_closing
                    .get(closing.stable_id())
                    .map(|origin| self.closing_pose(origin))
                    .unwrap_or(SpreadPose {
                        loc: closing.pos(),
                        zoom: 1.,
                    });
                let origin = closing.pos().to_physical_precise_round(scale);
                for element in
                    closing.render(ctx.r(), Rectangle::from_size(self.view_size), scale.into())
                {
                    let element = RescaleRenderElement::from_element(
                        MonitorInnerRenderElement::SpreadClosing(element),
                        origin,
                        pose.zoom,
                    );
                    push(RelocateRenderElement::from_element(
                        element,
                        pose.loc.to_physical_precise_round(scale) - origin,
                        Relocate::Relative,
                    ));
                }
            }
        }
        let tiles = self.spread_draw_tiles();
        let selected = self.spread_selected();
        let progress = self.overview_backdrop_progress() as f32;
        let title = format!(
            "搜索: {}  |  {} 个窗口  |  方向键选择 · Enter 打开 · Esc 返回",
            self.core
                .borrow()
                .spread()
                .output(&self.output_name)
                .map_or("", |output| output.query()),
            self.spread_matching_tiles().count()
        );
        let renderer_context = ctx.renderer.context_id().erased();
        let mut cache = self.spread_header.borrow_mut();
        if cache.as_ref().is_none_or(|(text, s, context, _)| {
            text != &title || *s != scale.to_bits() || context != &renderer_context
        }) {
            // Never keep rendering a texture from a previous renderer context if
            // regeneration fails (for example while a GPU is being recreated).
            *cache = None;
            if let Ok(texture) = crate::ui::mru::generate_title_texture(ctx.renderer, &title, scale)
            {
                *cache = Some((title, scale.to_bits(), renderer_context, texture));
            }
        }
        if let Some((_, _, _, texture)) = cache.as_ref() {
            let element = match texture {
                crate::ui::mru::MruTexture::Gles(buffer) => {
                    PrimaryGpuTextureRenderElement::from(TextureRenderElement::from_texture_buffer(
                        buffer.clone(),
                        (24., 24.),
                        progress,
                        None,
                        None,
                        Kind::Unspecified,
                    ))
                }
                crate::ui::mru::MruTexture::Vulkan(buffer) => {
                    PrimaryGpuTextureRenderElement::from(TextureRenderElement::from_texture_buffer(
                        buffer.clone(),
                        (24., 24.),
                        progress,
                        None,
                        None,
                        Kind::Unspecified,
                    ))
                }
            };
            push(RelocateRenderElement::from_element(
                RescaleRenderElement::from_element(
                    MonitorInnerRenderElement::SpreadText(element),
                    Point::default(),
                    1.,
                ),
                Point::default(),
                Relocate::Relative,
            ));
        }
        for (tile, geo, zoom) in tiles {
            if zoom <= 0. {
                continue;
            }
            tile.render(
                ctx.r(),
                Point::default(),
                XrayPos::new(geo.loc, zoom),
                selected.as_ref() == Some(tile.window().id()),
                &mut |element| {
                    let element = RescaleRenderElement::from_element(
                        MonitorInnerRenderElement::SpreadTile(element),
                        Point::default(),
                        zoom,
                    );
                    push(RelocateRenderElement::from_element(
                        element,
                        geo.loc.to_physical_precise_round(scale),
                        Relocate::Relative,
                    ));
                },
            );
        }
        // Fade workspace backgrounds with the grid's backdrop, retaining their stable
        // buffer identities and the exact ordinary-desktop scene at either closed endpoint.
        if progress < 1. {
            let render_idx = self.workspace_switch_idx();
            for (idx, ws) in self.workspaces.iter().enumerate() {
                let loc = Point::from((
                    0.,
                    (idx as f64 - render_idx) * (self.view_size.h + self.workspace_gap(1.)),
                ))
                .to_physical_precise_round(scale)
                .to_logical(scale);
                let geo = Rectangle::new(loc, self.view_size);
                if geo
                    .intersection(Rectangle::from_size(self.view_size))
                    .is_none()
                {
                    continue;
                }
                let background = ws.render_background();
                let color: smithay::backend::renderer::Color32F =
                    ws.options.layout.background_color.into();
                let element = SolidColorRenderElement::new(
                    background.id().clone(),
                    geo,
                    background.current_commit(),
                    color * (1. - progress),
                    Kind::Unspecified,
                );
                push(RelocateRenderElement::from_element(
                    RescaleRenderElement::from_element(
                        MonitorInnerRenderElement::SolidColor(element),
                        Point::default(),
                        1.,
                    ),
                    Point::default(),
                    Relocate::Relative,
                ));
            }
        }
    }
}

#[cfg(test)]
mod interpolation_tests {
    use super::*;

    #[test]
    fn negative_progress_retains_incoming_pose_velocity() {
        let view = Size::from((1920., 1080.));
        let start = SpreadPose {
            loc: Point::from((40., 20.)),
            zoom: 0.8,
        };
        let target = SpreadPose {
            loc: Point::from((0., 0.)),
            zoom: 1.,
        };
        let dt = 1e-6;
        // At 40% opening progress, a reversal toward zero first keeps the
        // incoming +1 progress/s velocity. The captured pose follows it.
        let pose = start.interpolate(target, dt / -0.4, view);
        assert!(((pose.loc.x - start.loc.x) / dt - 100.).abs() < 1e-7);
        assert!(((pose.loc.y - start.loc.y) / dt - 50.).abs() < 1e-7);
        assert!(((pose.zoom - start.zoom) / dt + 0.5).abs() < 1e-7);
    }

    #[test]
    fn extreme_extrapolation_stays_finite_with_bounded_zoom_and_location() {
        let view = Size::from((1920., 1080.));
        let start = SpreadPose {
            loc: Point::from((40., 20.)),
            zoom: 0.8,
        };
        let target = SpreadPose {
            loc: Point::from((0., 0.)),
            zoom: 1.,
        };
        for fraction in [-f64::MAX, -1e12, 1e12, f64::MAX] {
            let pose = start.interpolate(target, fraction, view);
            assert!(pose.loc.x.is_finite() && pose.loc.y.is_finite() && pose.zoom.is_finite());
            assert!((-1920. ..=1960.).contains(&pose.loc.x));
            assert!((-1080. ..=1100.).contains(&pose.loc.y));
            assert!((0. ..=1.).contains(&pose.zoom));
        }
        for fraction in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let pose = start.interpolate(target, fraction, view);
            assert_eq!(pose.loc, start.loc);
            assert_eq!(pose.zoom, start.zoom);
        }
    }
}
