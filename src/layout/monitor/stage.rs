//! Live workspace cards for Realand's spatial overview.
//!
//! The indexed geometry is the single source of truth for rendering, input, and
//! window dragging. Workspace IDs keep selection and captured poses stable when
//! empty workspaces disappear or the workspace order changes.
use super::*;
use crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement;
use crate::render_helpers::texture::TextureRenderElement;

pub(super) type StageGeometryCache = Option<(
    [u64; 14],
    Vec<WorkspaceId>,
    Rc<Vec<Rectangle<f64, Logical>>>,
)>;

type StageLayerRenderer<'a, R> = dyn FnMut(
        RenderCtx<R>,
        WorkspaceId,
        Rectangle<f64, Logical>,
        bool,
        &mut dyn FnMut(crate::layer::mapped::LayerSurfaceRenderElement<R>),
    ) + 'a;

#[derive(Debug)]
struct StageCapture {
    time: Duration,
    progress: f64,
    rects: HashMap<WorkspaceId, Rectangle<f64, Logical>>,
    depths: HashMap<WorkspaceId, f64>,
    highlights: HashMap<WorkspaceId, f64>,
    rail_first: f64,
    momentum: Option<StageMomentum>,
}

#[derive(Debug)]
struct StageTransition {
    start_progress: f64,
    target_progress: f64,
    is_gesture: bool,
    anchor: WorkspaceId,
    rects: HashMap<WorkspaceId, Rectangle<f64, Logical>>,
    depths: HashMap<WorkspaceId, f64>,
    highlights: HashMap<WorkspaceId, f64>,
    rail_first: f64,
    momentum: Option<StageMomentum>,
}

#[derive(Debug)]
struct SelectionTransition {
    animation: Animation,
    momentum: Option<StageMomentum>,
    rects: HashMap<WorkspaceId, Rectangle<f64, Logical>>,
    depths: HashMap<WorkspaceId, f64>,
    highlights: HashMap<WorkspaceId, f64>,
    rail_first: f64,
}

#[derive(Debug)]
struct StageMomentum {
    /// Zero displacement with unit launch velocity; multiplied by each card's
    /// velocity this carries its presentation motion into the next spring.
    animation: Animation,
    velocities: HashMap<WorkspaceId, CardVelocity>,
    end_time: Option<Duration>,
}

impl StageMomentum {
    fn is_done(&self, now: Duration) -> bool {
        self.animation.is_done() || self.end_time.is_some_and(|end| now >= end)
    }

    fn value_at(&self, at: Duration) -> f64 {
        let envelope = self.end_time.map_or(1., |end| {
            let duration = end
                .saturating_sub(self.animation.start_time())
                .as_secs_f64();
            if duration == 0. {
                return 0.;
            }
            let t = (at.saturating_sub(self.animation.start_time()).as_secs_f64() / duration)
                .clamp(0., 1.);
            // Finish with zero displacement and slope before the parent drops
            // overview geometry. At launch the tail is at zero, so this smooth
            // deadline envelope leaves the captured initial velocity intact.
            1. - t * t * (3. - 2. * t)
        });
        self.animation.value_at(at) * envelope
    }
}

#[derive(Debug)]
struct CardVelocity {
    loc: Point<f64, Logical>,
    // Size subtraction saturates at zero. A signed point also represents cards
    // which are shrinking, without constructing an invalid negative Size.
    size: Point<f64, Logical>,
}

impl SelectionTransition {
    fn is_done(&self) -> bool {
        self.animation.is_done() && self.momentum.as_ref().is_none_or(|m| m.animation.is_done())
    }
}

#[derive(Debug, Default)]
pub(super) struct StageState {
    transition: Option<StageTransition>,
    selection: Option<SelectionTransition>,
    pending: Option<StageCapture>,
    revision: u64,
}

#[derive(Debug, Clone)]
struct RailItem {
    target: Option<InsertWorkspace>,
    navigation: Option<isize>,
    geo: Rectangle<f64, Logical>,
    clip: Rectangle<f64, Logical>,
    text: String,
    subtitle: Option<String>,
    highlight: f64,
    enabled: bool,
}

impl<W: LayoutElement> Monitor<W> {
    pub(in crate::layout) fn stage_drag_source(&self) -> Option<WorkspaceId> {
        self.core
            .borrow()
            .stage_drag_source()
            .map(|id| WorkspaceId::specific(id.0))
    }

    pub fn is_stage_visible(&self) -> bool {
        self.overview_mode() == OverviewMode::Stage
            && (self.is_overview_open() || self.overview_progress.is_some())
    }

    pub(in crate::layout) fn stage_selected(&self) -> WorkspaceId {
        self.core
            .borrow()
            .overview()
            .stage(&self.output_name)
            .and_then(|state| state.selected)
            .map(|id| WorkspaceId::specific(id.0))
            .filter(|id| self.idx_of_ws(*id).is_some())
            .unwrap_or_else(|| self.active_workspace_ref().id())
    }

    pub(in crate::layout) fn reset_stage(&mut self) {
        self.stage_capture();
        let id = self.active_workspace_ref().id();
        self.core
            .borrow_mut()
            .overview_action(OverviewAction::StageReset {
                connector: self.output_name.clone(),
                space: realand_core::SpaceId(id.get()),
            });
        self.stage.selection = None;
        self.stage.transition = None;
        self.invalidate_stage_geometry();
    }

    pub(in crate::layout) fn stage_capture(&mut self) {
        // Only visible desktop workspaces have a presented origin. Capturing the
        // whole vertical strip makes unseen workspaces fly across the output at
        // desktop size before becoming Stage cards. Missing origins enter from
        // their collapsed card poses instead. In Stage, retain every pose so
        // reversing an in-flight selection stays continuous.
        let rects = if self.is_stage_visible() {
            self.workspaces
                .iter()
                .map(Workspace::id)
                .zip(self.workspaces_render_geo())
                .collect()
        } else {
            self.workspaces_with_render_geo()
                .map(|(workspace, geo)| (workspace.id(), geo))
                .collect()
        };
        let momentum = if self.is_stage_visible()
            && (self.stage.selection.is_some()
                || self
                    .stage
                    .transition
                    .as_ref()
                    .is_some_and(|t| t.momentum.is_some()))
        {
            self.stage_capture_momentum()
        } else {
            None
        };
        self.stage.pending = Some(StageCapture {
            time: self.clock.now_unadjusted(),
            progress: self.overview_backdrop_progress(),
            rects,
            depths: self.stage_depths(),
            highlights: self
                .workspaces
                .iter()
                .map(|ws| (ws.id(), self.stage_highlight(ws.id())))
                .collect(),
            rail_first: self.stage_rail_first(),
            momentum,
        });
    }

    pub(in crate::layout) fn stage_prepare_close(&mut self, id: WorkspaceId) {
        // Capturing before the parent activates a workspace preserves the visible
        // frame even if its normal workspace-switch geometry changes immediately.
        self.stage_capture();
        self.core
            .borrow_mut()
            .overview_action(OverviewAction::StageCloseAnchor {
                connector: self.output_name.clone(),
                space: Some(realand_core::SpaceId(id.get())),
            });
        self.invalidate_stage_geometry();
    }

    pub(in crate::layout) fn stage_select(&mut self, id: WorkspaceId) -> bool {
        if self.idx_of_ws(id).is_none() || self.stage_selected() == id {
            return false;
        }
        let momentum = self.stage_capture_momentum();
        let rects = self
            .workspaces
            .iter()
            .map(Workspace::id)
            .zip(self.workspaces_render_geo())
            .collect();
        let depths = self.stage_depths();
        let highlights = self
            .workspaces
            .iter()
            .map(|workspace| (workspace.id(), self.stage_highlight(workspace.id())))
            .collect();
        let rail_first = self.stage_rail_first();
        self.core
            .borrow_mut()
            .overview_action(OverviewAction::StageSelect {
                connector: self.output_name.clone(),
                space: realand_core::SpaceId(id.get()),
            });
        self.stage.selection = Some(SelectionTransition {
            animation: Animation::new(
                self.clock.clone(),
                0.,
                1.,
                0.,
                self.options.animations.overview_open_close.0,
            ),
            momentum,
            rects,
            depths,
            highlights,
            rail_first,
        });
        self.invalidate_stage_geometry();
        true
    }

    fn stage_capture_momentum(&self) -> Option<StageMomentum> {
        let config = self.options.animations.overview_open_close.0;
        if config.off || !matches!(config.kind, realand_config::animations::Kind::Spring(_)) {
            return None;
        }
        let overview_moving = matches!(self.overview_progress.as_ref(), Some(OverviewProgress::Animation(a)) if !a.is_clamped_done());
        let tail_moving = self
            .stage
            .transition
            .as_ref()
            .and_then(|t| t.momentum.as_ref())
            .is_some_and(|m| m.value_at(self.clock.now()) != 0.);
        if !overview_moving
            && !tail_moving
            && self
                .stage
                .selection
                .as_ref()
                .is_none_or(SelectionTransition::is_done)
        {
            return None;
        }
        // Sample unrounded presentation geometry, without advancing the shared
        // animation clock. Pixel-rounded positions would turn slow motion into
        // alternating zero and enormous velocities, especially at scale 1.25.
        let now = self.clock.now();
        let dt = Duration::from_micros(1);
        let sample = |at| {
            let clamped = |animation: &Animation| {
                if animation.is_clamped_done() {
                    animation.to()
                } else {
                    animation.value_at(at).clamp(0., 1.)
                }
            };
            let p = match self.overview_progress.as_ref() {
                Some(OverviewProgress::Animation(animation)) => clamped(animation),
                Some(OverviewProgress::Value(value)) => value.clamp(0., 1.),
                None => 0.,
            };
            let selection = self.stage.selection.as_ref();
            let fraction = selection.map_or(1., |s| clamped(&s.animation));
            let momentum = selection
                .and_then(|s| s.momentum.as_ref())
                .map_or(0., |m| m.value_at(at));
            let transition_momentum = self
                .stage
                .transition
                .as_ref()
                .and_then(|t| t.momentum.as_ref())
                .map_or(0., |m| m.value_at(at));
            self.stage_compute_rects(p, fraction, momentum, transition_momentum)
        };
        let before = sample(now);
        let after = sample(now.saturating_add(dt));
        let rate = self.clock.rate() / dt.as_secs_f64();
        let velocities: HashMap<_, _> = self
            .workspaces
            .iter()
            .map(Workspace::id)
            .zip(
                before
                    .into_iter()
                    .zip(after)
                    .map(|(before, after)| CardVelocity {
                        loc: (after.loc - before.loc).upscale(rate),
                        size: (after.size.to_point() - before.size.to_point()).upscale(rate),
                    }),
            )
            .collect();
        velocities
            .values()
            .any(|v| {
                [v.loc.x, v.loc.y, v.size.x, v.size.y]
                    .into_iter()
                    .any(|v| v.abs() > 0.001)
            })
            .then(|| StageMomentum {
                animation: Animation::new(self.clock.clone(), 0., 0., 1., config),
                velocities,
                end_time: None,
            })
    }

    pub(in crate::layout) fn stage_navigate(&mut self, delta: isize) -> bool {
        let idx = self.idx_of_ws(self.stage_selected()).unwrap();
        let target = idx
            .saturating_add_signed(delta)
            .min(self.workspaces.len() - 1);
        self.stage_select(self.workspaces[target].id())
    }

    pub(in crate::layout) fn stage_restore(&mut self) -> bool {
        let initial = self
            .core
            .borrow()
            .overview()
            .stage(&self.output_name)
            .and_then(|state| state.initial);
        let target = initial
            .map(|id| WorkspaceId::specific(id.0))
            .filter(|id| self.idx_of_ws(*id).is_some())
            .unwrap_or_else(|| self.active_workspace_ref().id());
        self.stage_select(target)
    }

    fn invalidate_stage_geometry(&mut self) {
        self.stage.revision = self.stage.revision.wrapping_add(1);
        *self.stage_geometry.borrow_mut() = None;
    }

    pub(super) fn advance_stage_animations(&mut self) {
        if self
            .stage
            .selection
            .as_ref()
            .is_some_and(SelectionTransition::is_done)
        {
            self.stage.selection = None;
            self.invalidate_stage_geometry();
        }
        if let Some(transition) = &mut self.stage.transition {
            if transition
                .momentum
                .as_ref()
                .is_some_and(|m| m.is_done(self.clock.now()))
            {
                transition.momentum = None;
                self.invalidate_stage_geometry();
            }
        }
    }

    pub(super) fn stage_animations_ongoing(&self) -> bool {
        self.is_stage_visible()
            && self
                .stage
                .selection
                .as_ref()
                .is_some_and(|selection| !selection.is_done())
    }

    pub(super) fn set_stage_progress(&mut self, progress: Option<&super::super::OverviewProgress>) {
        if self.overview_mode() != OverviewMode::Stage || progress.is_none() {
            self.stage.transition = None;
            self.stage.selection = None;
            self.stage.pending = None;
            // Close-anchor lifecycle is cleared by core owner events, never by
            // this per-output presentation update (which may forecast time).
            self.invalidate_stage_geometry();
            return;
        }
        let target_progress = if self.is_overview_open() { 1. } else { 0. };
        let is_gesture = matches!(progress, Some(super::super::OverviewProgress::Gesture));
        let anchor = if target_progress == 0. {
            self.core
                .borrow()
                .overview()
                .stage(&self.output_name)
                .and_then(|state| state.close_anchor)
                .map(|id| WorkspaceId::specific(id.0))
                .unwrap_or_else(|| self.active_workspace_ref().id())
        } else {
            self.stage
                .transition
                .as_ref()
                .map_or_else(|| self.active_workspace_ref().id(), |t| t.anchor)
        };
        let changed = self.stage.transition.as_ref().is_none_or(|t| {
            t.target_progress != target_progress
                || t.is_gesture != is_gesture
                || (!is_gesture && target_progress == 0. && t.anchor != anchor)
        });
        if changed {
            let pending_valid = self.stage.pending.as_ref().is_some_and(|capture| {
                capture.time == self.clock.now_unadjusted()
                    && capture.progress == self.overview_backdrop_progress()
            });
            if !pending_valid {
                self.stage_capture();
            }
            let capture = self.stage.pending.take().unwrap();
            let mut momentum = capture.momentum;
            // A close immediately reversed at fully-open progress has no scalar
            // progress distance left to animate. The card poses can still be in
            // flight after preview navigation, so settle them independently.
            let selection =
                (capture.progress == 1. && target_progress == 1.).then(|| SelectionTransition {
                    animation: Animation::new(
                        self.clock.clone(),
                        0.,
                        1.,
                        0.,
                        self.options.animations.overview_open_close.0,
                    ),
                    momentum: momentum.take(),
                    rects: capture.rects.clone(),
                    depths: capture.depths.clone(),
                    highlights: capture.highlights.clone(),
                    rail_first: capture.rail_first,
                });
            self.stage.transition = Some(StageTransition {
                start_progress: capture.progress,
                target_progress,
                is_gesture,
                anchor,
                rects: capture.rects,
                depths: capture.depths,
                highlights: capture.highlights,
                rail_first: capture.rail_first,
                momentum: None,
            });
            self.stage.selection = selection;
            if !is_gesture {
                if let Some(mut momentum) = momentum {
                    // Carry only the velocity missing from the new scalar path;
                    // open/close reversals already preserve its own momentum.
                    let now = self.clock.now();
                    let dt = Duration::from_micros(1);
                    let next_p = match progress.unwrap() {
                        super::super::OverviewProgress::Animation(animation) => {
                            momentum.end_time = Some(animation.end_time());
                            if animation.is_clamped_done() {
                                animation.to()
                            } else {
                                animation.value_at(now.saturating_add(dt)).clamp(0., 1.)
                            }
                        }
                        super::super::OverviewProgress::Open => 1.,
                        super::super::OverviewProgress::Gesture => self
                            .core
                            .borrow()
                            .overview()
                            .gesture_value()
                            .unwrap_or(0.)
                            .clamp(0., 1.),
                    };
                    let origins = self.stage_compute_rects(capture.progress, 1., 0., 0.);
                    let next = self.stage_compute_rects(next_p, 1., 0., 0.);
                    let rate = self.clock.rate() / dt.as_secs_f64();
                    for ((workspace, origin), next) in self.workspaces.iter().zip(origins).zip(next)
                    {
                        if let Some(velocity) = momentum.velocities.get_mut(&workspace.id()) {
                            velocity.loc -= (next.loc - origin.loc).upscale(rate);
                            velocity.size -=
                                (next.size.to_point() - origin.size.to_point()).upscale(rate);
                        }
                    }
                    self.stage.transition.as_mut().unwrap().momentum = Some(momentum);
                }
            }
            self.invalidate_stage_geometry();
        }
    }

    fn stage_transition_fraction(&self) -> f64 {
        let Some(transition) = self.stage.transition.as_ref() else {
            return 1.;
        };
        let p = self.overview_backdrop_progress();
        let target = if transition.is_gesture {
            if p < transition.start_progress {
                0.
            } else {
                1.
            }
        } else {
            transition.target_progress
        };
        if target == transition.start_progress {
            1.
        } else {
            (p - transition.start_progress) / (target - transition.start_progress)
        }
    }

    fn stage_interpolate_value(&self, id: WorkspaceId, target: f64, highlight: bool) -> f64 {
        let values = |depths: &HashMap<WorkspaceId, f64>,
                      highlights: &HashMap<WorkspaceId, f64>| {
            if highlight {
                highlights.get(&id)
            } else {
                depths.get(&id)
            }
            .copied()
        };
        if let Some(selection) = self.stage.selection.as_ref() {
            let from = values(&selection.depths, &selection.highlights).unwrap_or(target);
            return from + (target - from) * selection.animation.clamped_value();
        }
        if let Some(transition) = self.stage.transition.as_ref() {
            let from = values(&transition.depths, &transition.highlights).unwrap_or(target);
            // Closing cards retain their current depth and emphasis while fading.
            let target = if transition.target_progress == 0. {
                from
            } else {
                target
            };
            return from + (target - from) * self.stage_transition_fraction();
        }
        target
    }

    // Preserve current depth and selection emphasis when retargeting a preview.
    // Rendering, hits and drops all sort this same continuously animated depth.
    fn stage_depths(&self) -> HashMap<WorkspaceId, f64> {
        let selected = self.idx_of_ws(self.stage_selected()).unwrap();
        self.workspaces
            .iter()
            .enumerate()
            .map(|(idx, ws)| {
                (
                    ws.id(),
                    self.stage_interpolate_value(ws.id(), idx.abs_diff(selected) as f64, false)
                        .max(0.),
                )
            })
            .collect()
    }

    fn stage_highlight(&self, id: WorkspaceId) -> f64 {
        self.stage_interpolate_value(id, f64::from(id == self.stage_selected()), true)
            .clamp(0., 1.)
    }

    fn stage_rail_first(&self) -> f64 {
        let selected = self.idx_of_ws(self.stage_selected()).unwrap();
        let count = self.workspaces.len().min(7);
        let target = selected
            .saturating_sub(3)
            .min(self.workspaces.len() - count) as f64;
        if let Some(selection) = self.stage.selection.as_ref() {
            return selection.rail_first
                + (target - selection.rail_first) * selection.animation.clamped_value();
        }
        if let Some(transition) = self.stage.transition.as_ref() {
            let target = if transition.target_progress == 0. {
                transition.rail_first
            } else {
                target
            };
            return transition.rail_first
                + (target - transition.rail_first) * self.stage_transition_fraction();
        }
        target
    }

    pub(super) fn stage_front_order(&self) -> Vec<usize> {
        let depths = self.stage_depths();
        let mut indices: Vec<_> = (0..self.workspaces.len()).collect();
        indices.sort_by(|a, b| {
            depths[&self.workspaces[*a].id()]
                .total_cmp(&depths[&self.workspaces[*b].id()])
                .then_with(|| a.cmp(b))
        });
        indices
    }

    fn stage_available_area(&self) -> Rectangle<f64, Logical> {
        // Layer-shell exclusive zones are output-local. Keep all interactive
        // Stage controls inside that visible area, including on small outputs
        // where an oversized panel consumes the entire working area.
        self.working_area
            .intersection(Rectangle::from_size(self.view_size))
            .unwrap_or_else(|| {
                Rectangle::new(
                    Point::from((
                        self.working_area.loc.x.clamp(0., self.view_size.w),
                        self.working_area.loc.y.clamp(0., self.view_size.h),
                    )),
                    Size::from((0., 0.)),
                )
            })
    }

    fn stage_grid_rects(&self) -> Vec<Rectangle<f64, Logical>> {
        let selected = self.idx_of_ws(self.stage_selected()).unwrap();
        let area = self.stage_available_area();
        // Preserve the configured zoom across the supported overview range.
        // An epsilon also keeps pointer/drag transforms finite on tiny outputs.
        let requested_zoom = if self.options.overview.zoom.is_finite() {
            self.options.overview.zoom.clamp(0., 1.)
        } else {
            0.5
        };
        let zoom = requested_zoom
            .min(
                (area.size.h - self.stage_rail_height() - 24.).max(f64::EPSILON)
                    / self.view_size.h.max(f64::EPSILON),
            )
            .min((area.size.w - 48.).max(f64::EPSILON) / self.view_size.w.max(f64::EPSILON))
            .max(f64::EPSILON);
        // The workspace keeps its original aspect ratio; the reserved area
        // changes the layout center and maximum zoom, not the card contents.
        let center_size = self.view_size.upscale(zoom);
        let center = Point::<f64, Logical>::from((
            area.loc.x + area.size.w / 2.,
            area.loc.y + (area.size.h - self.stage_rail_height()) / 2.,
        ));
        let remaining = (area.size.w - center_size.w) / 2.;
        (0..=self.workspaces.len())
            .map(|idx| {
                let depth = idx.abs_diff(selected);
                let direction = if idx < selected { -1. } else { 1. };
                let ratio = match depth {
                    0 => 1.,
                    1 => 0.75,
                    2 => 0.60,
                    3 => 0.48,
                    _ => 0.,
                };
                let size = center_size.upscale(ratio);
                let card_center = if depth == 0 {
                    center
                } else if depth <= 3 {
                    // Rear cards rise above the nearer card so all three
                    // depth layers retain a visible edge within the output.
                    let gap = (center_size.h * 0.05).min(18.);
                    let top = center.y - center_size.h * 0.375 - (depth - 1) as f64 * gap;
                    Point::from((
                        center.x + direction * (center_size.w / 2. + remaining * depth as f64 / 4.),
                        top + size.h / 2.,
                    ))
                } else {
                    Point::from((
                        if direction < 0. {
                            area.loc.x
                        } else {
                            area.loc.x + area.size.w
                        },
                        center.y,
                    ))
                };
                Rectangle::new(card_center - size.to_point().downscale(2.), size)
            })
            .collect()
    }

    pub(super) fn stage_workspace_geometries(&self) -> Rc<Vec<Rectangle<f64, Logical>>> {
        let p = self.overview_backdrop_progress();
        let selection_fraction = self
            .stage
            .selection
            .as_ref()
            .map_or(1., |selection| selection.animation.clamped_value());
        let momentum_fraction = self
            .stage
            .selection
            .as_ref()
            .and_then(|s| s.momentum.as_ref())
            .map_or(0., |m| m.value_at(self.clock.now()));
        let transition_momentum = self
            .stage
            .transition
            .as_ref()
            .and_then(|t| t.momentum.as_ref())
            .map_or(0., |m| m.value_at(self.clock.now()));
        let scale = self.scale.fractional_scale();
        let key = [
            self.view_size.w.to_bits(),
            self.view_size.h.to_bits(),
            scale.to_bits(),
            p.to_bits(),
            selection_fraction.to_bits(),
            momentum_fraction.to_bits(),
            transition_momentum.to_bits(),
            self.options.overview.zoom.to_bits(),
            self.stage.revision,
            self.stage_selected().get(),
            self.working_area.loc.x.to_bits(),
            self.working_area.loc.y.to_bits(),
            self.working_area.size.w.to_bits(),
            self.working_area.size.h.to_bits(),
        ];
        if let Some((cached_key, ids, geometry)) = &*self.stage_geometry.borrow() {
            if *cached_key == key
                && ids
                    .iter()
                    .copied()
                    .eq(self.workspaces.iter().map(Workspace::id))
            {
                return geometry.clone();
            }
        }
        let mut rects = self.stage_compute_rects(
            p,
            selection_fraction,
            momentum_fraction,
            transition_momentum,
        );
        for rect in &mut rects {
            rect.loc = rect.loc.to_physical_precise_round(scale).to_logical(scale);
        }
        let rects = Rc::new(rects);
        *self.stage_geometry.borrow_mut() = Some((
            key,
            self.workspaces.iter().map(Workspace::id).collect(),
            rects.clone(),
        ));
        rects
    }

    fn stage_compute_rects(
        &self,
        p: f64,
        selection_fraction: f64,
        momentum_fraction: f64,
        transition_momentum: f64,
    ) -> Vec<Rectangle<f64, Logical>> {
        let grid = self.stage_grid_rects();
        let transition = self.stage.transition.as_ref();
        let anchor = transition.map_or_else(
            || self.active_workspace_ref().id(),
            |transition| transition.anchor,
        );
        let start_progress = transition.map_or(0., |t| t.start_progress);
        let target_progress = transition.map_or(1., |t| {
            if t.is_gesture {
                if p < start_progress {
                    0.
                } else {
                    1.
                }
            } else {
                t.target_progress
            }
        });
        let fraction = if start_progress == target_progress {
            1.
        } else {
            (p - start_progress) / (target_progress - start_progress)
        };
        grid.into_iter()
            .enumerate()
            .map(|(idx, grid)| {
                let id = self.workspaces.get(idx).map(Workspace::id);
                let desktop = if id == Some(anchor) {
                    Rectangle::from_size(self.view_size)
                } else {
                    Rectangle::new(
                        grid.loc + grid.size.to_point().downscale(2.),
                        Size::from((0., 0.)),
                    )
                };
                let start = transition
                    .and_then(|t| t.rects.get(&id?))
                    .copied()
                    .unwrap_or_else(|| {
                        interpolate_expo_rect(desktop, grid, start_progress, self.view_size)
                    });
                let target = if target_progress == 1. { grid } else { desktop };
                let mut rect = interpolate_expo_rect(start, target, fraction, self.view_size);
                if let Some(velocity) = transition
                    .and_then(|t| t.momentum.as_ref())
                    .and_then(|m| m.velocities.get(&id?))
                {
                    rect = self.stage_apply_momentum(rect, velocity, transition_momentum);
                }
                if let Some(selection) = &self.stage.selection {
                    if let Some(origin) = id.and_then(|id| selection.rects.get(&id)) {
                        rect = interpolate_expo_rect(
                            *origin,
                            rect,
                            selection_fraction,
                            self.view_size,
                        );
                    }
                    if let Some(velocity) = selection
                        .momentum
                        .as_ref()
                        .and_then(|m| m.velocities.get(&id?))
                    {
                        rect = self.stage_apply_momentum(rect, velocity, momentum_fraction);
                    }
                }
                rect
            })
            .collect()
    }

    fn stage_apply_momentum(
        &self,
        mut rect: Rectangle<f64, Logical>,
        velocity: &CardVelocity,
        fraction: f64,
    ) -> Rectangle<f64, Logical> {
        rect.loc += velocity.loc.upscale(fraction);
        rect.size.w = (rect.size.w + velocity.size.x * fraction).clamp(0., self.view_size.w);
        rect.size.h = (rect.size.h + velocity.size.y * fraction).clamp(0., self.view_size.h);
        rect.loc.x = rect.loc.x.clamp(-self.view_size.w, 2. * self.view_size.w);
        rect.loc.y = rect.loc.y.clamp(-self.view_size.h, 2. * self.view_size.h);
        rect
    }

    fn stage_rail_height(&self) -> f64 {
        self.stage_available_area().size.h.min(76.)
    }

    fn stage_rail_items(&self) -> Vec<RailItem> {
        let area = self.stage_available_area();
        let height = (self.stage_rail_height() - 24.).max(0.);
        if height == 0. || area.size.w <= 0. {
            // There is no visible rail target when the panel consumes all of
            // its space. An epsilon-sized target outside the area would still
            // accept clicks or drops behind the panel.
            return Vec::new();
        }
        let first = self.stage_rail_first();
        let count = self.workspaces.len().min(7);
        let selected = self.idx_of_ws(self.stage_selected()).unwrap();
        let margin = area.size.w.min(24.);
        let available = (area.size.w - margin * 2.).max(f64::EPSILON);
        let fit = (available / (44. * 2. + 148. * count as f64 + 144.)).min(1.);
        let arrow_width = 44. * fit;
        let width = 148. * fit;
        let new_width = 144. * fit;
        let total = arrow_width * 2. + width * count as f64 + new_width;
        let x = area.loc.x + (area.size.w - total) / 2.;
        let y = area.loc.y + area.size.h - self.stage_rail_height() + 8.;
        let geo = |offset: f64, width: f64| {
            Rectangle::new(Point::from((x + offset, y)), Size::from((width, height)))
        };
        let clip = |offset, width| geo(offset, width).intersection(area).unwrap_or_default();
        let mut items = vec![RailItem {
            target: None,
            navigation: Some(-1),
            geo: geo(0., arrow_width),
            clip: clip(0., arrow_width),
            text: "‹".into(),
            subtitle: None,
            highlight: 0.,
            enabled: selected > 0,
        }];
        let workspace_clip = Rectangle::new(
            Point::from((x + arrow_width, y)),
            Size::from((count as f64 * width, height)),
        )
        .intersection(area)
        .unwrap_or_default();
        for (idx, ws) in self.workspaces.iter().enumerate() {
            let geo = geo(arrow_width + (idx as f64 - first) * width, width);
            let Some(clip) = geo.intersection(workspace_clip) else {
                continue;
            };
            let name = ws
                .name()
                .map(|name| {
                    name.chars()
                        .filter(|c| !c.is_control())
                        .take(48)
                        .collect::<String>()
                })
                .unwrap_or_else(|| format!("工作区 {}", idx + 1));
            items.push(RailItem {
                target: Some(InsertWorkspace::Existing(ws.id())),
                navigation: None,
                geo,
                clip,
                text: name,
                subtitle: Some(format!("{} 个窗口", ws.windows().count())),
                highlight: self.stage_highlight(ws.id()),
                enabled: true,
            });
        }
        items.push(RailItem {
            target: Some(InsertWorkspace::NewAt(self.workspaces.len())),
            navigation: None,
            geo: geo(arrow_width + count as f64 * width, new_width),
            clip: clip(arrow_width + count as f64 * width, new_width),
            text: "+ 新工作区".into(),
            subtitle: Some("点击进入 · 拖入新建".into()),
            highlight: 0.,
            enabled: true,
        });
        items.push(RailItem {
            target: None,
            navigation: Some(1),
            geo: geo(arrow_width + count as f64 * width + new_width, arrow_width),
            clip: clip(arrow_width + count as f64 * width + new_width, arrow_width),
            text: "›".into(),
            subtitle: None,
            highlight: 0.,
            enabled: selected + 1 < self.workspaces.len(),
        });
        items
    }

    pub(in crate::layout) fn stage_rail_under(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<InsertWorkspace> {
        self.is_stage_visible()
            .then(|| self.stage_rail_items())?
            .into_iter()
            .find(|item| item.clip.contains(pos))?
            .target
    }

    pub(in crate::layout) fn stage_rail_navigation(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<isize> {
        let item = self
            .is_stage_visible()
            .then(|| self.stage_rail_items())?
            .into_iter()
            .find(|item| item.clip.contains(pos))?;
        item.navigation
            .map(|delta| if item.enabled { delta } else { 0 })
    }

    pub(in crate::layout) fn stage_drop_target(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(InsertWorkspace, Rectangle<f64, Logical>)> {
        if !self.is_stage_visible() {
            return None;
        }
        let area = self.stage_available_area();
        if area.size.w <= 0. || area.size.h <= 0. {
            return None;
        }
        let geometry = self.stage_workspace_geometries();
        if let Some(target) = self.stage_rail_under(pos) {
            let geo = match target {
                InsertWorkspace::Existing(id) => {
                    let geo = geometry[self.idx_of_ws(id)?];
                    if geo.size.w > 0. && geo.size.h > 0. {
                        geo
                    } else {
                        self.stage_reference_geometry()
                    }
                }
                InsertWorkspace::NewAt(_) => self.stage_reference_geometry(),
            };
            return Some((target, geo));
        }
        // Only displayed cards and explicit rail slots accept a drop. The parent
        // restores the original window when a release falls into empty space.
        self.workspace_under(pos)
            .map(|(ws, geo)| (InsertWorkspace::Existing(ws.id()), geo))
    }

    fn stage_reference_geometry(&self) -> Rectangle<f64, Logical> {
        let geometry = self.stage_workspace_geometries();
        let idx = self.idx_of_ws(self.stage_selected()).unwrap();
        let selected = geometry[idx];
        if selected.size.w > 0. && selected.size.h > 0. {
            return selected;
        }
        // The selected workspace may still be entering from outside the visible
        // three-card range. Use the current front card for a finite drag proxy.
        if let Some(geo) = self
            .stage_front_order()
            .into_iter()
            .map(|idx| geometry[idx])
            .find(|geo| geo.size.w > 0. && geo.size.h > 0.)
        {
            return geo;
        }
        self.stage_grid_rects()[idx]
    }

    pub(in crate::layout) fn stage_card_zoom(&self) -> f64 {
        self.workspace_render_zoom(self.stage_reference_geometry())
            .max(0.01)
    }

    pub(super) fn render_stage<R: RealandRenderer>(
        &self,
        ctx: RenderCtx<R>,
        focus_ring: bool,
        push: &mut dyn FnMut(MonitorRenderElement<R>),
    ) {
        self.render_stage_with_layers(ctx, focus_ring, &mut |_, _, _, _, _| {}, push);
    }

    /// Compose each complete card before proceeding to the card behind it.
    /// Bottom/background layer-shell content uses the card's exact transform and
    /// the same privacy render target as its windows.
    pub fn render_stage_with_layers<R: RealandRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        focus_ring: bool,
        layers: &mut StageLayerRenderer<'_, R>,
        push: &mut dyn FnMut(MonitorRenderElement<R>),
    ) {
        let scale = self.scale.fractional_scale();
        let progress = self.overview_backdrop_progress() as f32;
        self.render_stage_rail(ctx.r(), push);
        // Finish each card before emitting the next card: a rear floating window
        // must never draw above the selected card's tiling window or background.
        for (ws, geo) in self.workspaces_with_render_geo() {
            let zoom = self.workspace_render_zoom(geo);
            let offset = geo.loc.to_physical_precise_round(scale);
            let crop = Rectangle::from_size(ws.view_size().to_physical_precise_round(scale));
            let xray = XrayPos::new(geo.loc, zoom);
            self.render_stage_border(ws.id(), geo, progress, push);
            macro_rules! card_push {
                () => {
                    &mut |elem| {
                        if let Some(elem) = CropRenderElement::from_element(elem, scale, crop) {
                            let elem = MonitorInnerRenderElement::from(elem);
                            push(RelocateRenderElement::from_element(
                                RescaleRenderElement::from_element(elem, Point::default(), zoom),
                                offset,
                                Relocate::Relative,
                            ));
                        }
                    }
                };
            }
            let layer_push = &mut |element| {
                if let Some(element) = CropRenderElement::from_element(element, scale, crop) {
                    let inner = MonitorInnerRenderElement::StageLayer(element);
                    push(RelocateRenderElement::from_element(
                        RescaleRenderElement::from_element(inner, Point::default(), zoom),
                        offset,
                        Relocate::Relative,
                    ));
                }
            };
            layers(ctx.r(), ws.id(), geo, true, layer_push);
            ws.render_floating(
                ctx.r(),
                xray,
                focus_ring,
                RenderLayer::MovingBetweenWorkspaces,
                card_push!(),
            );
            ws.render_floating(ctx.r(), xray, focus_ring, RenderLayer::Normal, card_push!());
            if let Some(loc) = self.insert_hint_render_loc {
                if loc.workspace == InsertWorkspace::Existing(ws.id()) {
                    self.insert_hint_element
                        .render(ctx.renderer, loc.location, card_push!());
                }
            }
            ws.render_scrolling(
                ctx.r(),
                xray,
                focus_ring,
                RenderLayer::MovingBetweenWorkspaces,
                card_push!(),
            );
            ws.render_scrolling(ctx.r(), xray, focus_ring, RenderLayer::Normal, card_push!());
            let layer_push = &mut |element| {
                if let Some(element) = CropRenderElement::from_element(element, scale, crop) {
                    let inner = MonitorInnerRenderElement::StageLayer(element);
                    push(RelocateRenderElement::from_element(
                        RescaleRenderElement::from_element(inner, Point::default(), zoom),
                        offset,
                        Relocate::Relative,
                    ));
                }
            };
            layers(ctx.r(), ws.id(), geo, false, layer_push);
            let background = ws.render_background();
            let element = MonitorInnerRenderElement::SolidColor(background);
            push(RelocateRenderElement::from_element(
                RescaleRenderElement::from_element(element, Point::default(), zoom),
                offset,
                Relocate::Relative,
            ));
            ws.render_shadow(ctx.renderer, &mut |element| {
                let element = MonitorInnerRenderElement::Shadow(element.with_alpha(progress));
                push(RelocateRenderElement::from_element(
                    RescaleRenderElement::from_element(element, Point::default(), zoom),
                    offset,
                    Relocate::Relative,
                ));
            });
        }
    }

    fn render_stage_border<R: RealandRenderer>(
        &self,
        id: WorkspaceId,
        geo: Rectangle<f64, Logical>,
        alpha: f32,
        push: &mut dyn FnMut(MonitorRenderElement<R>),
    ) {
        let idx = self.idx_of_ws(id).unwrap();
        let highlight = self
            .insert_hint
            .as_ref()
            .and_then(|hint| hint.workspace.existing_id())
            .map_or_else(|| self.stage_highlight(id), |hint| f64::from(hint == id));
        let width = 1. + 2. * highlight;
        if geo.size.w <= width * 2. || geo.size.h <= width * 2. {
            return;
        }
        let active_color = self
            .options
            .layout
            .focus_ring
            .active_color
            .to_array_premul();
        let passive_color = [0.25, 0.25, 0.30, 0.8];
        let color = std::array::from_fn(|idx| {
            passive_color[idx] + (active_color[idx] - passive_color[idx]) * highlight as f32
        });
        let mut cache = self.expo_borders.borrow_mut();
        cache.resize_with(self.workspaces.len(), || {
            std::array::from_fn(|_| SolidColorBuffer::new(Size::from((0., 0.)), [0.; 4]))
        });
        let positions = [
            (geo.loc, Size::from((geo.size.w, width))),
            (
                geo.loc + Point::from((0., geo.size.h - width)),
                Size::from((geo.size.w, width)),
            ),
            (
                geo.loc + Point::from((0., width)),
                Size::from((width, geo.size.h - width * 2.)),
            ),
            (
                geo.loc + Point::from((geo.size.w - width, width)),
                Size::from((width, geo.size.h - width * 2.)),
            ),
        ];
        for (buffer, (loc, size)) in cache[idx].iter_mut().zip(positions) {
            buffer.update(size, color);
            let inner = MonitorInnerRenderElement::SolidColor(
                SolidColorRenderElement::from_buffer(buffer, loc, alpha, Kind::Unspecified),
            );
            push(RelocateRenderElement::from_element(
                RescaleRenderElement::from_element(inner, Point::default(), 1.),
                Point::default(),
                Relocate::Relative,
            ));
        }
    }

    fn render_stage_rail<R: RealandRenderer>(
        &self,
        ctx: RenderCtx<R>,
        push: &mut dyn FnMut(MonitorRenderElement<R>),
    ) {
        let scale = self.scale.fractional_scale();
        let alpha = self.overview_backdrop_progress() as f32;
        if alpha <= 0. {
            return;
        }
        let items = self.stage_rail_items();
        let mut texts = self.stage_rail_text.borrow_mut();
        let mut buffers = self.stage_rail_buffers.borrow_mut();
        texts.resize_with(items.len() * 2, || None);
        buffers.resize_with(items.len(), || {
            SolidColorBuffer::new(Size::from((0., 0.)), [0.; 4])
        });
        let renderer_context = ctx.renderer.context_id().erased();
        for (idx, item) in items.iter().enumerate() {
            let alpha = alpha * if item.enabled { 1. } else { 0.4 };
            // Give the count its own line so a long custom workspace name cannot
            // push it beyond the card's clip. Each line retains its GPU texture.
            for (line, text) in [Some(&item.text), item.subtitle.as_ref()]
                .into_iter()
                .enumerate()
            {
                let Some(text) = text else {
                    continue;
                };
                let cache = &mut texts[idx * 2 + line];
                if cache.as_ref().is_none_or(|(old, s, context, _)| {
                    old != text || *s != scale.to_bits() || context != &renderer_context
                }) {
                    *cache = None;
                    if let Ok(texture) =
                        crate::ui::mru::generate_title_texture(ctx.renderer, text, scale)
                    {
                        *cache = Some((
                            text.clone(),
                            scale.to_bits(),
                            renderer_context.clone(),
                            texture,
                        ));
                    }
                }
                let Some((_, _, _, texture)) = cache else {
                    continue;
                };
                let text_size = match texture {
                    crate::ui::mru::MruTexture::Gles(buffer) => buffer.logical_size(),
                    crate::ui::mru::MruTexture::Vulkan(buffer) => buffer.logical_size(),
                }
                .upscale(if line == 0 { 1. } else { 0.85 });
                let y = if item.subtitle.is_some() {
                    7. + line as f64 * 22.
                } else {
                    (item.geo.size.h - text_size.h) / 2.
                };
                let x = if item.navigation.is_some() {
                    (item.geo.size.w - text_size.w) / 2.
                } else {
                    10.
                };
                let loc = item.geo.loc + Point::from((x, y));
                let text_alpha = alpha * if line == 0 { 1. } else { 0.72 };
                let element = match texture {
                    crate::ui::mru::MruTexture::Gles(buffer) => {
                        PrimaryGpuTextureRenderElement::from(
                            TextureRenderElement::from_texture_buffer(
                                buffer.clone(),
                                loc,
                                text_alpha,
                                None,
                                Some(text_size),
                                Kind::Unspecified,
                            ),
                        )
                    }
                    crate::ui::mru::MruTexture::Vulkan(buffer) => {
                        PrimaryGpuTextureRenderElement::from(
                            TextureRenderElement::from_texture_buffer(
                                buffer.clone(),
                                loc,
                                text_alpha,
                                None,
                                Some(text_size),
                                Kind::Unspecified,
                            ),
                        )
                    }
                };
                let crop = Rectangle::new(
                    item.geo.loc + Point::from((4., 0.)),
                    Size::from(((item.geo.size.w - 8.).max(0.), item.geo.size.h)),
                )
                .intersection(item.clip)
                .unwrap_or_default()
                .to_physical_precise_round(scale);
                if let Some(element) = CropRenderElement::from_element(element, scale, crop) {
                    let inner = MonitorInnerRenderElement::StageText(element);
                    push(RelocateRenderElement::from_element(
                        RescaleRenderElement::from_element(inner, Point::default(), 1.),
                        Point::default(),
                        Relocate::Relative,
                    ));
                }
            }
            let highlight = if self
                .insert_hint
                .as_ref()
                .is_some_and(|hint| Some(hint.workspace) == item.target)
            {
                1.
            } else {
                item.highlight as f32
            };
            let active_color = [0.12, 0.25, 0.40, 0.95];
            let passive_color = [0.06, 0.07, 0.09, 0.9];
            let color = std::array::from_fn(|idx| {
                passive_color[idx] + (active_color[idx] - passive_color[idx]) * highlight
            });
            let Some(background) = Rectangle::new(
                item.geo.loc + Point::from((2., 0.)),
                Size::from(((item.geo.size.w - 4.).max(0.), item.geo.size.h)),
            )
            .intersection(item.clip) else {
                continue;
            };
            buffers[idx].update(background.size, color);
            let element = SolidColorRenderElement::from_buffer(
                &buffers[idx],
                background.loc,
                alpha,
                Kind::Unspecified,
            );
            let inner = MonitorInnerRenderElement::SolidColor(element);
            push(RelocateRenderElement::from_element(
                RescaleRenderElement::from_element(inner, Point::default(), 1.),
                Point::default(),
                Relocate::Relative,
            ));
        }
    }
}
