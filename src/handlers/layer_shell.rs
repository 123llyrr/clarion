use smithay::desktop::{layer_map_for_output, LayerSurface, PopupKind, WindowSurfaceType};
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::compositor::{add_pre_commit_hook, get_parent, with_states, HookId};
use smithay::wayland::shell::wlr_layer::{
    self, Layer, LayerSurface as WlrLayerSurface, LayerSurfaceCachedState, LayerSurfaceData,
    WlrLayerShellHandler, WlrLayerShellState,
};
use smithay::wayland::shell::xdg::PopupSurface;

use crate::compositor::State;
use crate::layer::{MappedLayer, ResolvedLayerRules};
use crate::utils::{is_mapped, output_size, send_scale_transform};

impl WlrLayerShellHandler for State {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.clarion.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: WlrLayerSurface,
        wl_output: Option<WlOutput>,
        _layer: Layer,
        namespace: String,
    ) {
        let output = if let Some(wl_output) = &wl_output {
            self.clarion.output_from_resource(wl_output)
        } else {
            self.clarion.layout.active_output().cloned()
        };
        let Some(output) = output else {
            warn!("no output for new layer surface, closing");
            surface.send_close();
            return;
        };

        let wl_surface = surface.wl_surface().clone();
        let is_new = self.clarion.unmapped_layer_surfaces.insert(wl_surface);
        assert!(is_new);

        let layer = LayerSurface::new(surface, namespace);
        let scale = output.current_scale();
        let transform = output.current_transform();
        layer.with_surfaces(|surface, data| {
            send_scale_transform(surface, data, scale, transform);
        });
        // map_layer/arrange may send the initial configure themselves, so
        // initialize the output state before entering that path.
        let mut map = layer_map_for_output(&output);
        map.map_layer(&layer).unwrap();
    }

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        let wl_surface = surface.wl_surface();
        self.clarion.unmapped_layer_surfaces.remove(wl_surface);

        let output = if let Some((output, mut map, layer)) =
            self.clarion.layout.outputs().find_map(|o| {
                let map = layer_map_for_output(o);
                let layer = map
                    .layers()
                    .find(|&layer| layer.layer_surface() == &surface)
                    .cloned();
                layer.map(|layer| (o.clone(), map, layer))
            }) {
            map.unmap_layer(&layer);
            self.clarion.mapped_layer_surfaces.remove(&layer);
            Some(output)
        } else {
            None
        };
        if let Some(output) = output {
            self.clarion.output_resized(&output);
        }
    }

    fn new_popup(&mut self, _parent: WlrLayerSurface, popup: PopupSurface) {
        self.unconstrain_popup(&PopupKind::Xdg(popup));
    }
}

impl State {
    pub fn layer_shell_handle_commit(&mut self, surface: &WlSurface) -> bool {
        let mut root_surface = surface.clone();
        while let Some(parent) = get_parent(&root_surface) {
            root_surface = parent;
        }

        let output = self
            .clarion
            .layout
            .outputs()
            .find(|o| {
                let map = layer_map_for_output(o);
                map.layer_for_surface(&root_surface, WindowSurfaceType::TOPLEVEL)
                    .is_some()
            })
            .cloned();
        let Some(output) = output else {
            return false;
        };

        if surface != &root_surface {
            // This is an unsync layer-shell subsurface.
            self.clarion.queue_redraw(&output);
            return true;
        }

        let mut map = layer_map_for_output(&output);

        // Arrange the layers before sending the initial configure to respect any size the
        // client may have sent.
        let previous_working_area = map.non_exclusive_zone();
        map.arrange();
        let working_area_changed = map.non_exclusive_zone() != previous_working_area;

        let layer = map
            .layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
            .unwrap();

        let mut privacy_changed = false;
        if is_mapped(surface) {
            let was_unmapped = self.clarion.unmapped_layer_surfaces.remove(surface);

            // Resolve rules for newly mapped layer surfaces.
            if was_unmapped {
                let config = self.clarion.config.borrow();

                let rules = &config.layer_rules;
                let rules = ResolvedLayerRules::compute(rules, layer, self.clarion.is_at_startup);
                // A remapped layer may have been visible in a captured frame
                // before it changed to a protected layer while unmapped.
                privacy_changed = rules.block_out_from.is_some();

                let output_size = output_size(&output);
                let scale = output.current_scale().fractional_scale();

                let hook = add_mapped_layer_pre_commit_hook(layer);
                let mapped = MappedLayer::new(
                    layer.clone(),
                    hook,
                    rules,
                    output_size,
                    scale,
                    self.clarion.clock.clone(),
                    &config,
                );

                let prev = self
                    .clarion
                    .mapped_layer_surfaces
                    .insert(layer.clone(), mapped);
                if prev.is_some() {
                    error!("MappedLayer was present for an unmapped surface");
                }
            } else {
                // The surface remains mapped.
                if let Some(mapped) = self.clarion.mapped_layer_surfaces.get_mut(layer) {
                    // Check if the layer changed.
                    if mapped.take_recompute_rules_on_commit() {
                        let config = self.clarion.config.borrow();
                        let previous_privacy = mapped.rules().block_out_from;
                        if mapped
                            .recompute_layer_rules(&config.layer_rules, self.clarion.is_at_startup)
                        {
                            privacy_changed = previous_privacy != mapped.rules().block_out_from;
                            mapped.update_config(&config);
                        }
                    }
                } else {
                    error!("MappedLayer missing for a mapped surface");
                }
            }

            // Give focus to newly mapped on-demand surfaces. Some launchers like lxqt-runner rely
            // on this behavior. While this behavior doesn't make much sense for other clients like
            // panels, the consensus seems to be that it's not a big deal since panels generally
            // only open once at the start of the session.
            //
            // Note that:
            // 1) Exclusive layer surfaces already get focus automatically in
            //    update_keyboard_focus().
            // 2) Same-layer exclusive layer surfaces are already preferred to on-demand surfaces in
            //    update_keyboard_focus(), so we don't need to check for that here.
            let on_demand = layer.cached_state().keyboard_interactivity
                == wlr_layer::KeyboardInteractivity::OnDemand;
            // The persistent desktop maps when the shell starts or an output is
            // connected. It must wait for user interaction rather than taking
            // focus away from the current application.
            if was_unmapped && on_demand && layer.namespace() != "clarion-desktop" {
                // I guess it'd make sense to check that no higher-layer on-demand surface
                // has focus, but Smithay's Layer doesn't implement Ord so this would be a
                // little annoying.
                self.clarion.layer_shell_on_demand_focus = Some(layer.clone());
            }
        } else {
            // The surface is unmapped.
            if self.clarion.mapped_layer_surfaces.remove(layer).is_some() {
                // A mapped surface got unmapped via a null commit. Now it needs to do a new
                // initial commit again.
                self.clarion.unmapped_layer_surfaces.insert(surface.clone());
            } else {
                // An unmapped surface remains unmapped. If we haven't sent an initial configure
                // yet, we should do so.
                let initial_configure_sent = with_states(surface, |states| {
                    states
                        .data_map
                        .get::<LayerSurfaceData>()
                        .unwrap()
                        .lock()
                        .unwrap()
                        .initial_configure_sent
                });
                if !initial_configure_sent {
                    let scale = output.current_scale();
                    let transform = output.current_transform();
                    // A client may create children before assigning the root its
                    // layer role, when new_subsurface cannot resolve an output.
                    // Initialize the whole tree once; ordinary buffer commits do
                    // not need to resend output state.
                    layer.with_surfaces(|surface, data| {
                        send_scale_transform(surface, data, scale, transform);
                    });

                    layer.layer_surface().send_configure();
                }
                // If we already sent an initial configure, then map.arrange() above had just sent
                // it a new configure, if needed.
            }
        }

        drop(map);

        if privacy_changed {
            // Ordinary repaints retain handoffs, but a privacy rule change must
            // never retain pixels captured under the previous rules.
            self.clarion.clear_screen_transition_snapshots();
            self.clarion.queue_redraw_all();
        }

        // Buffer-only commits (video wallpaper, meters, clocks) need a redraw,
        // not a full output refresh. Arrange already updates the layer geometry;
        // only a changed exclusive zone requires relaying out the workspaces.
        // Actual mode/scale/transform changes still go through output_resized().
        if working_area_changed {
            self.clarion.layout.update_output_size(&output);
        }
        self.clarion.queue_redraw(&output);

        true
    }
}

fn add_mapped_layer_pre_commit_hook(layer: &LayerSurface) -> HookId {
    add_pre_commit_hook::<State, _>(layer.wl_surface(), move |state, _dh, surface| {
        let layer_changed = with_states(surface, |states| {
            let mut guard = states.cached_state.get::<LayerSurfaceCachedState>();
            let pending_layer = guard.pending().layer;
            let current_layer = guard.current().layer;
            pending_layer != current_layer
        });

        if layer_changed {
            for mapped in state.clarion.mapped_layer_surfaces.values_mut() {
                if mapped.surface().wl_surface() == surface {
                    mapped.set_recompute_rules_on_commit();
                    break;
                }
            }
        }
    })
}
