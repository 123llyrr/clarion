use std::collections::{HashMap, HashSet};
use std::mem;
use std::os::fd::OwnedFd;
use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, bail, Context};
use clarion_config::OutputName;
use libc::dev_t;
use smithay::backend::allocator::format::FormatSet;
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice};
use smithay::backend::drm::compositor::DrmCompositor;
use smithay::backend::drm::exporter::gbm::GbmFramebufferExporter;
use smithay::backend::drm::{
    DrmDevice, DrmDeviceFd, DrmEvent, DrmEventMetadata, DrmEventTime, DrmNode, NodeType, VrrSupport,
};
use smithay::backend::renderer::{DebugFlags, ImportDma, ImportEgl};
use smithay::backend::session::{Event as SessionEvent, Session};
use smithay::backend::udev::UdevEvent;
use smithay::output::{Mode, Output, OutputModeSource, PhysicalProperties};
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::drm::control::{connector, crtc, ModeFlags, ModeTypeFlags};
use smithay::reexports::gbm::Modifier;
use smithay::reexports::rustix::fs::OFlags;
use smithay::reexports::wayland_protocols;
use smithay::utils::DeviceFd;
use smithay::wayland::dmabuf::DmabufFeedbackBuilder;
use smithay::wayland::drm_lease::DrmLeaseState;
use smithay::wayland::presentation::Refresh;
use smithay_drm_extras::drm_scanner::{DrmScanEvent, DrmScanner};
use wayland_protocols::wp::presentation_time::server::wp_presentation_feedback;

use crate::backend::OutputId;
use crate::compositor::{Clarion, State};
use crate::render_helpers::renderer::AsGlesRenderer;
use crate::utils::{
    get_monotonic_time, is_laptop_panel, logical_output, next_presentation_seq, PanelOrientation,
};

use crate::output_schedule::EstimatedVblankAction;

use crate::backend::tty::renderer_init::initialize_device_renderer;
use crate::backend::tty::*;

impl Tty {
    fn platform_init(&mut self, clarion: &mut Clarion) {
        // If the session is inactive, skip initialization because we won't be able to do much with
        // the devices anyway. We'll get ActivateSession and add the devices there instead.
        //
        // This can happen when starting clarion while having a different TTY active (e.g. via tmux).
        if !self.session.is_active() {
            return;
        }

        // Initialize the ignored nodes.
        self.ignored_nodes = self.compute_ignored_nodes();

        let udev = self.udev_inventory.dispatcher.clone();
        let udev = udev.as_source_ref();

        // Initialize the primary node first as later nodes might depend on the primary render node
        // being available.
        if let Some((primary_device_id, primary_device_path)) = udev
            .device_list()
            .find(|&(device_id, _)| device_id == self.primary_node.dev_id())
        {
            if let Err(err) =
                self.platform_device_added(primary_device_id, primary_device_path, clarion)
            {
                warn!(
                    "error adding primary node device, display-only devices may not work: {err:?}"
                );
            }
        } else {
            warn!("primary node is missing, display-only devices may not work");
        };

        for (device_id, path) in udev.device_list() {
            if device_id == self.primary_node.dev_id() {
                continue;
            }

            if let Err(err) = self.platform_device_added(device_id, path, clarion) {
                warn!("error adding device: {err:?}");
            }
        }
    }

    fn platform_on_udev_event(&mut self, clarion: &mut Clarion, event: UdevEvent) {
        let _span = clarion_tracy_span!("Tty::on_udev_event");

        match event {
            UdevEvent::Added { device_id, path } => {
                if !self.session.is_active() {
                    debug!("skipping UdevEvent::Added as session is inactive");
                    return;
                }

                // Recompute ignored nodes to resolve symlinks (like /dev/dri/by-path/...) to their
                // new underlying device IDs.
                self.ignored_nodes = self.compute_ignored_nodes();

                if let Err(err) = self.platform_device_added(device_id, &path, clarion) {
                    warn!("error adding device: {err:?}");
                }
            }
            UdevEvent::Changed { device_id } => {
                if !self.session.is_active() {
                    debug!("skipping UdevEvent::Changed as session is inactive");
                    return;
                }

                self.platform_device_changed(device_id, clarion, false)
            }
            UdevEvent::Removed { device_id } => {
                if !self.session.is_active() {
                    debug!("skipping UdevEvent::Removed as session is inactive");
                    return;
                }

                self.platform_device_removed(device_id, clarion)
            }
        }
    }

    fn platform_on_session_event(&mut self, clarion: &mut Clarion, event: SessionEvent) {
        let _span = clarion_tracy_span!("Tty::on_session_event");

        match event {
            SessionEvent::PauseSession => {
                debug!("pausing session");

                self.libinput.suspend();

                for device in self.devices.values_mut() {
                    device.drm.pause();

                    if let Some(lease_state) = &mut device.drm_lease_state {
                        lease_state.suspend();
                    }
                }
            }
            SessionEvent::ActivateSession => {
                debug!("resuming session");

                if self.libinput.resume().is_err() {
                    warn!("error resuming libinput");
                }

                // While the session was suspended, GPUs could have been added, so
                // /dev/dri/by-path/... symlinks need to be re-resolved.
                self.ignored_nodes = self.compute_ignored_nodes();

                let mut device_list = self
                    .udev_inventory
                    .dispatcher
                    .as_source_ref()
                    .device_list()
                    .map(|(device_id, path)| (device_id, path.to_owned()))
                    .collect::<HashMap<_, _>>();

                let removed_devices = self
                    .devices
                    .keys()
                    .filter(|node| {
                        !device_list.contains_key(&node.dev_id())
                            || self.ignored_nodes.contains(node)
                    })
                    .copied()
                    .collect::<Vec<_>>();

                let remained_devices = self
                    .devices
                    .keys()
                    .filter(|node| {
                        device_list.contains_key(&node.dev_id())
                            && !self.ignored_nodes.contains(node)
                    })
                    .copied()
                    .collect::<Vec<_>>();

                // Remove removed devices.
                for node in removed_devices {
                    device_list.remove(&node.dev_id());
                    self.platform_device_removed(node.dev_id(), clarion);
                }

                // Update remained devices.
                for node in remained_devices {
                    device_list.remove(&node.dev_id());

                    // It hasn't been removed, update its state as usual.
                    let device = self.devices.get_mut(&node).unwrap();

                    // Someone on an old device hit what seems to be a driver bug without this.
                    let force_disable = self
                        .config
                        .as_ref()
                        .debug
                        .force_disable_connectors_on_resume;

                    if let Err(err) = device.drm.activate(force_disable) {
                        warn!("error activating DRM device: {err:?}");
                    }
                    if let Some(lease_state) = &mut device.drm_lease_state {
                        lease_state.resume::<State>();
                    }

                    // Refresh the connectors.
                    self.platform_device_changed(node.dev_id(), clarion, true);

                    // Apply pending gamma changes and restore our existing gamma.
                    let device = self.devices.get_mut(&node).unwrap();
                    for (crtc, surface) in device.surfaces.iter_mut() {
                        if let Ok(mut props) =
                            ConnectorProperties::try_new(&device.drm, surface.connector)
                        {
                            let max_bpc = self
                                .config
                                .as_ref()
                                .outputs
                                .find(&surface.name)
                                .and_then(|o| o.max_bpc);
                            set_connector_properties(&mut props, max_bpc, false);
                        } else {
                            warn!("failed to get connector properties");
                        }

                        if let Some(ramp) = surface.pending_gamma_change.take() {
                            let ramp = ramp.as_deref();
                            let res = if let Some(gamma_props) = &mut surface.gamma_props {
                                gamma_props.set_gamma(&device.drm, ramp)
                            } else {
                                set_gamma_for_crtc(&device.drm, *crtc, ramp)
                            };
                            if let Err(err) = res {
                                warn!("error applying pending gamma change: {err:?}");
                            }
                        } else if let Some(gamma_props) = &surface.gamma_props {
                            if let Err(err) = gamma_props.restore_gamma(&device.drm) {
                                warn!("error restoring gamma: {err:?}");
                            }
                        }
                    }
                }

                // Add new devices.
                //
                // Add the primary node first as later nodes might depend on the primary render
                // node being available.
                let primary_device_id = self.primary_node.dev_id();
                let primary_device_path = device_list.remove(&primary_device_id);
                let primary = primary_device_path.map(|path| (primary_device_id, path));

                for (device_id, path) in primary.into_iter().chain(device_list) {
                    if let Err(err) = self.platform_device_added(device_id, &path, clarion) {
                        warn!("error adding device: {err:?}");
                    }
                }

                if self.update_output_config_on_resume {
                    self.platform_on_output_config_changed(clarion);
                }

                self.platform_refresh_ipc_outputs(clarion);

                clarion.notify_activity();
                clarion.monitors_active = true;
                self.set_monitors_active(true);
                reset_kms_submission_backoff(clarion);
                clarion.queue_redraw_all();
            }
        }
    }

    fn platform_device_added(
        &mut self,
        device_id: dev_t,
        path: &Path,
        clarion: &mut Clarion,
    ) -> anyhow::Result<()> {
        debug!("adding device: {device_id} {path:?}");

        let node = DrmNode::from_dev_id(device_id)?;

        if node == self.primary_node {
            debug!("this is the primary node");
        }

        // Only consider primary node on udev event
        // https://gitlab.freedesktop.org/wlroots/wlroots/-/commit/768fbaad54027f8dd027e7e015e8eeb93cb38c52
        if node.ty() != NodeType::Primary {
            debug!("not a primary node, skipping");
            return Ok(());
        }

        if self.ignored_nodes.contains(&node) {
            debug!("node is ignored, skipping");
            return Ok(());
        }

        let _span = clarion_tracy_span!("Tty::device_added");

        let open_flags = OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK;
        let fd = {
            let _span = clarion_tracy_span!("LibSeatSession::open");
            self.session.open(path, open_flags)
        }?;
        let device_fd = DrmDeviceFd::new(DeviceFd::from(fd));

        let (drm, drm_notifier) = {
            let _span = clarion_tracy_span!("DrmDevice::new");
            DrmDevice::new(device_fd.clone(), false)
        }?;
        let syncobj_fd = device_fd.clone();
        let gbm = {
            let _span = clarion_tracy_span!("GbmDevice::new");
            GbmDevice::new(device_fd)
        }?;

        let is_primary_gpu = is_primary_gpu_node(node, self.primary_node, self.primary_render_node);
        if is_primary_gpu && self.dmabuf_global.is_none() && self.requested_renderer.is_vulkan() {
            self.native_renderer = super::native_renderer_with_wakeup(
                self.primary_render_node,
                self.config.as_ref(),
                &clarion.event_loop,
            );
        }

        // DRM topology is sufficient for native Vulkan. Display-only nodes keep
        // None and use the existing primary allocator; no EGL discovery is needed.
        let drm_render_node = node
            .node_with_type(NodeType::Render)
            .and_then(Result::ok)
            .or_else(|| is_primary_gpu.then_some(self.primary_render_node));
        let render_node = initialize_device_renderer(
            self.native_renderer.is_some(),
            drm_render_node,
            &mut self.gpu_manager,
            |manager, render_node| manager.register_deferred(render_node, gbm.clone()),
            |manager| match manager.probe_and_register(node, gbm.clone()) {
                Ok(render_node) => Some(render_node),
                Err(err) => {
                    debug!(
                        "failed to initialize EGL renderer, falling back to primary gpu: {err:?}"
                    );
                    None
                }
            },
        );
        if let Some(render_node) = render_node {
            debug!("got render node: {render_node}");
        }

        if is_primary_gpu && self.dmabuf_global.is_none() {
            let vulkan_node = self.primary_render_node;
            debug!("initializing the primary renderer");

            let primary_formats = if let Some(native) = self.native_renderer.as_ref() {
                info!(
                    "Using standalone native Vulkan renderer without creating GLES scene renderer"
                );
                Some(native.dmabuf_formats())
            } else if let Some(render_node) = render_node {
                let mut renderer = self
                    .gpu_manager
                    .single_renderer(&render_node)
                    .context("error creating renderer")?;

                if let Err(err) = renderer.bind_wl_display(&clarion.display_handle) {
                    // wl_drm is on its way out so this is expected on most modern distros.
                    trace!("error binding legacy EGL to wl_display: {err}");
                } else {
                    debug!("bound legacy EGL to wl_display");
                }

                if let Some(gles_renderer) = renderer.as_gles_renderer() {
                    Self::init_gles_renderer(&self.config, gles_renderer);
                }

                Some(renderer.dmabuf_formats())
            } else {
                warn!("failed to initialize the primary renderer");
                None
            };

            if let Some(primary_formats) = primary_formats {
                clarion.update_shaders();

                // Create the dmabuf global.
                let default_feedback =
                    DmabufFeedbackBuilder::new(vulkan_node.dev_id(), primary_formats.clone())
                        .build()
                        .context("error building default dmabuf feedback")?;
                let dmabuf_global = clarion
                    .dmabuf_state
                    .create_global_with_default_feedback::<State>(
                        &clarion.display_handle,
                        &default_feedback,
                    );
                assert!(self.dmabuf_global.replace(dmabuf_global).is_none());
                if self.native_renderer.is_some() {
                    clarion.enable_drm_syncobj(syncobj_fd);
                }

                // Update the dmabuf feedbacks for all surfaces.
                for (node, device) in self.devices.iter_mut() {
                    for surface in device.surfaces.values_mut() {
                        match surface_dmabuf_feedback(
                            &surface.compositor,
                            primary_formats.clone(),
                            self.primary_render_node,
                            device.render_node,
                            self.primary_node,
                            *node,
                        ) {
                            Ok(feedback) => {
                                surface.dmabuf_feedback = Some(feedback);
                            }
                            Err(err) => {
                                warn!("error building dmabuf feedback: {err:?}");
                            }
                        }
                    }
                }
            }
        }

        let allocator_gbm = if render_node.is_some() || is_primary_gpu {
            gbm.clone()
        } else if let Some(primary_device) = self.devices.get(&self.primary_node) {
            primary_device.gbm.clone()
        } else {
            bail!("no allocator available for device");
        };
        let gbm_flags = GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT;
        let allocator = GbmAllocator::new(allocator_gbm, gbm_flags);

        let token = clarion
            .event_loop
            .insert_source(drm_notifier, move |event, meta, state| {
                let tty = state.backend.tty();
                match event {
                    DrmEvent::VBlank(crtc) => {
                        let meta = meta.expect("VBlank events must have metadata");
                        tty.platform_on_vblank(&mut state.clarion, node, crtc, meta);
                    }
                    DrmEvent::Error(error) => warn!("DRM error: {error}"),
                };
            })
            .unwrap();

        let drm_lease_state = DrmLeaseState::new::<State>(&clarion.display_handle, &node)
            .map_err(|err| warn!("error initializing DRM leasing for {node}: {err:?}"))
            .ok();

        let device = OutputDevice {
            token,
            registered_render_node: render_node,
            render_node,
            gles_classification_pending: self.native_renderer.is_some(),
            drm,
            gbm,
            allocator,
            drm_scanner: DrmScanner::new(),
            surfaces: HashMap::new(),
            known_crtcs: HashMap::new(),
            drm_lease_state,
            active_leases: Vec::new(),
            non_desktop_connectors: HashSet::new(),
        };
        assert!(self.devices.insert(node, device).is_none());

        self.platform_device_changed(device_id, clarion, true);

        Ok(())
    }

    fn platform_device_changed(&mut self, device_id: dev_t, clarion: &mut Clarion, cleanup: bool) {
        debug!("device changed: {device_id}");

        let Ok(node) = DrmNode::from_dev_id(device_id) else {
            warn!("error creating DrmNode");
            return;
        };

        if node.ty() != NodeType::Primary {
            debug!("not a primary node, skipping");
            return;
        }

        if self.ignored_nodes.contains(&node) {
            debug!("node is ignored, skipping");
            return;
        }

        let Some(device) = self.devices.get_mut(&node) else {
            if let Some(path) = node.dev_path() {
                warn!("unknown device; trying to add");

                if let Err(err) = self.platform_device_added(device_id, &path, clarion) {
                    warn!("error adding device: {err:?}");
                }
            } else {
                warn!("unknown device");
            }

            return;
        };

        // DrmScanner will preserve any existing connector-CRTC mapping.
        let scan_result = match device.drm_scanner.scan_connectors(&device.drm) {
            Ok(x) => x,
            Err(err) => {
                warn!("error scanning connectors: {err:?}");
                return;
            }
        };

        let mut added = Vec::new();
        let mut removed = Vec::new();
        for event in scan_result {
            match event {
                DrmScanEvent::Connected {
                    connector,
                    crtc: Some(crtc),
                } => {
                    let connector_name = format_connector_name(&connector);
                    let name = make_output_name(&device.drm, connector.handle(), connector_name);
                    debug!(
                        "new connector: {} \"{}\"",
                        &name.connector,
                        name.format_make_model_serial(),
                    );

                    // Assign an id to this crtc.
                    let id = OutputId::next();
                    added.push((crtc, CrtcInfo { id, name }));
                }
                DrmScanEvent::Disconnected {
                    crtc: Some(crtc), ..
                } => {
                    removed.push(crtc);
                }
                // Emitted when the list of connector modes changes at runtime.
                //
                // Some devices, notably USB-C docks with DP-MST/alt-mode, report Connected before
                // the EDID has been read, with an empty mode list. Then, at a later point, the
                // modes will be populated, at which point we'll get this Changed event.
                DrmScanEvent::Changed {
                    connector,
                    crtc: Some(crtc),
                } => {
                    let connector_name = format_connector_name(&connector);
                    let name = make_output_name(&device.drm, connector.handle(), connector_name);
                    debug!(
                        "connector changed: {} \"{}\"",
                        &name.connector,
                        name.format_make_model_serial(),
                    );

                    if !device.known_crtcs.contains_key(&crtc) {
                        // I guess this can happen if the connector initially wasn't mapped to a
                        // CRTC but then got mapped before being changed.
                        warn!("changed connector missing from known crtcs");
                    }

                    // We don't actually need to do anything here; on_output_config_changed() will
                    // take care of picking a new mode if needed.
                }
                _ => (),
            }
        }

        for crtc in &removed {
            self.platform_connector_disconnected(clarion, node, *crtc);
        }

        let Some(device) = self.devices.get_mut(&node) else {
            error!("device disappeared");
            return;
        };

        for crtc in removed {
            if device.known_crtcs.remove(&crtc).is_none() {
                error!("output ID missing for disconnected crtc: {crtc:?}");
            }
        }

        for (crtc, mut info) in added {
            // Make/model/serial can match exactly between different physical monitors. This doesn't
            // happen often, but our Layout does not support such duplicates and will panic.
            //
            // As a workaround, search for duplicates, and unname the new connectors if one is
            // found. Connector names are always unique.
            let name = &mut info.name;
            let formatted = name.format_make_model_serial_or_connector();
            for info in self.devices.values().flat_map(|d| d.known_crtcs.values()) {
                if info.name.matches(&formatted) {
                    let connector = mem::take(&mut name.connector);
                    warn!(
                        "new connector {connector} duplicates make/model/serial \
                         of existing connector {}, unnaming",
                        info.name.connector,
                    );
                    *name = OutputName {
                        connector,
                        make: None,
                        model: None,
                        serial: None,
                    };
                    break;
                }
            }

            // Insert it right away so next added connector will check against this one too.
            let device = self.devices.get_mut(&node).unwrap();
            device.known_crtcs.insert(crtc, info);
        }

        // If the device was just added or resumed, we need to cleanup any disconnected connectors
        // and planes.
        if cleanup {
            let device = self.devices.get(&node).unwrap();

            // Follow the logic in on_output_config_changed().
            let disable_laptop_panels = self.should_disable_laptop_panels(clarion.is_lid_closed);
            let should_disable = |conn: &str| disable_laptop_panels && is_laptop_panel(conn);

            let config = self.config.as_ref();
            let disable_monitor_names = config.debug.disable_monitor_names;

            let should_be_off = |crtc, conn: &connector::Info| {
                let output_name = device.known_crtc_name(&crtc, conn, disable_monitor_names);

                let config = config
                    .outputs
                    .find(&output_name)
                    .cloned()
                    .unwrap_or_default();

                config.off || should_disable(&output_name.connector)
            };

            if let Err(err) = device.cleanup_mismatching_resources(&should_be_off) {
                warn!("error cleaning up connectors: {err:?}");
            }

            let device = self.devices.get_mut(&node).unwrap();
            for surface in device.surfaces.values_mut() {
                // We aren't force-clearing the CRTCs, so we need to make the surfaces read the
                // updated state after a session resume. This also causes a full damage for the
                // next redraw.
                if let Err(err) = surface.compositor.reset_state() {
                    warn!("error resetting DrmCompositor state: {err:?}");
                }
                surface.compositor.reset_buffers();
            }
        }

        // This will connect any new connectors if needed, and apply other changes, such as
        // connecting back the internal laptop monitor once it becomes the only monitor left.
        //
        // It will also call refresh_ipc_outputs(), which will catch the disconnected connectors
        // above.
        self.platform_on_output_config_changed(clarion);
    }

    fn platform_device_removed(&mut self, device_id: dev_t, clarion: &mut Clarion) {
        debug!("removing device: {device_id}");

        let Ok(node) = DrmNode::from_dev_id(device_id) else {
            warn!("error creating DrmNode");
            return;
        };

        if node.ty() != NodeType::Primary {
            debug!("not a primary node, skipping");
            return;
        }

        let Some(device) = self.devices.get_mut(&node) else {
            warn!("unknown device");
            return;
        };

        let crtcs: Vec<_> = device
            .drm_scanner
            .crtcs()
            .map(|(_info, crtc)| crtc)
            .collect();

        for crtc in crtcs {
            self.platform_connector_disconnected(clarion, node, crtc);
        }

        let mut device = self.devices.remove(&node).unwrap();
        let device_fd = device.drm.device_fd().device_fd();

        if let Some(lease_state) = &mut device.drm_lease_state {
            lease_state.disable_global::<State>();
        }

        // Vulkan can initialize from the configured DRM render node even when EGL could not
        // report one. Teardown therefore has to recognize the configured primary DRM device,
        // not only the optional EGL render-node entry.
        let removed_primary =
            is_primary_gpu_node(node, self.primary_node, self.primary_render_node);
        let primary_remains = self
            .devices
            .keys()
            .copied()
            .any(|node| is_primary_gpu_node(node, self.primary_node, self.primary_render_node));
        if removed_primary && !primary_remains {
            // A removed/re-added node can have the same dev_t. Its old worker
            // generation must never be installed into the replacement device.
            self.vulkan_recovery.invalidate();
            self.vulkan_recovery_generation = None;
            self.vulkan_recovery_attempt = 0;
            self.vulkan_recovery_retry_after = Duration::ZERO;
            if let Some(timer) = self.vulkan_recovery_watchdog.take() {
                clarion.event_loop.remove(timer);
            }
            debug!("destroying the primary renderer");

            if let Some(_native) = self.native_renderer.take() {
                debug!("destroyed native Vulkan primary renderer");
            } else if self.gpu_manager.is_initialized(&self.primary_render_node) {
                match self.gpu_manager.single_renderer(&self.primary_render_node) {
                    Ok(mut renderer) => renderer.unbind_wl_display(),
                    Err(err) => {
                        warn!("error creating renderer during device removal: {err}");
                    }
                }
            }

            // Disable and destroy the dmabuf global.
            if let Some(global) = self.dmabuf_global.take() {
                clarion.disable_drm_syncobj();
                clarion
                    .dmabuf_state
                    .disable_global::<State>(&clarion.display_handle, &global);
                clarion
                    .event_loop
                    .insert_source(
                        Timer::from_duration(Duration::from_secs(10)),
                        move |_, _, state| {
                            state
                                .clarion
                                .dmabuf_state
                                .destroy_global::<State>(&state.clarion.display_handle, global);
                            TimeoutAction::Drop
                        },
                    )
                    .unwrap();

                // Clear the dmabuf feedbacks for all surfaces.
                for device in self.devices.values_mut() {
                    for surface in device.surfaces.values_mut() {
                        surface.dmabuf_feedback = None;
                    }
                }
            } else {
                error!("dmabuf global was already missing");
            }
        }

        if let Some(render_node) = device.registered_render_node {
            // Sometimes (Asahi DisplayLink), multiple primary nodes will correspond to the same
            // render node. In this case, we want to keep the render node active until the last
            // primary node that uses it is gone.
            let was_last = !self
                .devices
                .values()
                .any(|device| device.registered_render_node == Some(render_node));

            if was_last {
                self.gpu_manager.remove_node(&render_node);
            }
        }

        clarion.event_loop.remove(device.token);

        self.platform_refresh_ipc_outputs(clarion);

        drop(device);

        match TryInto::<OwnedFd>::try_into(device_fd) {
            Ok(fd) => {
                if let Err(err) = self.session.close(fd) {
                    warn!("error closing DRM device fd: {err:?}");
                }
            }
            Err(_) => {
                error!("unable to close DRM device cleanly: fd has unexpected references");
            }
        }
    }

    fn platform_connector_connected(
        &mut self,
        clarion: &mut Clarion,
        node: DrmNode,
        connector: connector::Info,
        crtc: crtc::Handle,
    ) -> anyhow::Result<()> {
        let connector_name = format_connector_name(&connector);
        debug!("connecting connector: {connector_name}");

        // Every native scene texture belongs to the primary Vulkan context. On a secondary
        // card we must allocate the swapchain on that same GPU, then let the output card import
        // the DMA-BUF for scanout. Smithay tests that import and the KMS modeset in
        // DrmCompositor::new; if it fails we keep this output on the GLES copy path.
        let primary_allocator = self.primary_allocator();
        let device = self.devices.get_mut(&node).context("missing device")?;

        let disable_monitor_names = self.config.as_ref().debug.disable_monitor_names;
        let output_name = device.known_crtc_name(&crtc, &connector, disable_monitor_names);

        let non_desktop = find_drm_property(&device.drm, connector.handle(), "non-desktop")
            .and_then(|(_, info, value)| info.value_type().convert_value(value).as_boolean())
            .unwrap_or(false);

        if non_desktop {
            debug!("output is non desktop");
            let description = output_name.format_description();
            if let Some(lease_state) = &mut device.drm_lease_state {
                lease_state.add_connector::<State>(connector.handle(), connector_name, description);
            }
            device
                .non_desktop_connectors
                .insert((connector.handle(), crtc));
            return Ok(());
        }

        let config = self
            .config
            .as_ref()
            .outputs
            .find(&output_name)
            .cloned()
            .unwrap_or_default();

        for m in connector.modes() {
            trace!("{m:?}");
        }

        let mut mode = None;
        if let Some(modeline) = &config.modeline {
            match calculate_drm_mode_from_modeline(modeline) {
                Ok(x) => mode = Some(x),
                Err(err) => {
                    warn!("invalid custom modeline; falling back to advertised modes: {err:?}");
                }
            }
        }

        let (mode, fallback) = match mode {
            Some(x) => (x, false),
            None => pick_mode(&connector, config.mode).ok_or_else(|| anyhow!("no mode"))?,
        };

        if fallback {
            let target = config.mode.unwrap();
            warn!(
                "configured mode {}x{}{} could not be found, falling back to preferred",
                target.mode.width,
                target.mode.height,
                if let Some(refresh) = target.mode.refresh {
                    format!("@{refresh}")
                } else {
                    String::new()
                },
            );
        }

        debug!("picking mode: {mode:?}");

        let mut orientation = None;
        if let Ok(mut props) = ConnectorProperties::try_new(&device.drm, connector.handle()) {
            set_connector_properties(&mut props, config.max_bpc, config.hdr);

            match props.get_panel_orientation() {
                Ok(x) => orientation = Some(x),
                Err(err) => {
                    trace!("couldn't get panel orientation: {err:?}");
                }
            }
        } else {
            warn!("failed to get connector properties");
        }

        let mut gamma_props = GammaProps::new(&device.drm, crtc)
            .map_err(|err| debug!("couldn't get gamma properties: {err:?}"))
            .ok();

        // Reset gamma in case it was set before.
        let res = if let Some(gamma_props) = &mut gamma_props {
            gamma_props.set_gamma(&device.drm, None)
        } else {
            set_gamma_for_crtc(&device.drm, crtc, None)
        };
        if let Err(err) = res {
            debug!("couldn't reset gamma: {err:?}");
        }

        let surface = device
            .drm
            .create_surface(crtc, mode, &[connector.handle()])?;

        // Try to enable VRR if requested.
        match surface.vrr_supported(connector.handle()) {
            Ok(VrrSupport::Supported | VrrSupport::RequiresModeset) => {
                // Even if on-demand, we still disable it until later checks.
                let vrr = config.is_vrr_always_on();
                let word = if vrr { "enabling" } else { "disabling" };

                if let Err(err) = surface.use_vrr(vrr) {
                    warn!("error {} VRR: {err:?}", word);
                }
            }
            Ok(VrrSupport::NotSupported) => {
                if !config.is_vrr_always_off() {
                    warn!("cannot enable VRR because connector does not support it");
                }

                // Try to disable it anyway to work around a bug where resetting DRM state causes
                // vrr_capable to be reset to 0, potentially leaving VRR_ENABLED at 1.
                let _ = surface.use_vrr(false);
            }
            Err(err) => {
                warn!("error querying for VRR support: {err:?}");
            }
        }

        // Update the output mode.
        let (physical_width, physical_height) = connector.size().unwrap_or((0, 0));

        let output = Output::new(
            connector_name.clone(),
            PhysicalProperties {
                size: (physical_width as i32, physical_height as i32).into(),
                subpixel: connector.subpixel().into(),
                model: output_name.model.as_deref().unwrap_or("Unknown").to_owned(),
                make: output_name.make.as_deref().unwrap_or("Unknown").to_owned(),
                serial_number: output_name
                    .serial
                    .as_deref()
                    .unwrap_or("Unknown")
                    .to_owned(),
            },
        );

        let wl_mode = Mode::from(mode);
        output.change_current_state(Some(wl_mode), None, None, None);
        output.set_preferred(wl_mode);

        output
            .user_data()
            .insert_if_missing(|| TtyOutputState { node, crtc });
        output.user_data().insert_if_missing(|| output_name.clone());
        if let Some(x) = orientation {
            output.user_data().insert_if_missing(|| PanelOrientation(x));
        }

        let mut use_native_renderer = output_uses_native_renderer(
            self.native_renderer.is_some(),
            primary_allocator.is_some(),
        );
        if self.native_renderer.is_some() && !use_native_renderer {
            warn!(
                "output {connector_name} cannot use Vulkan: no GBM allocator for primary render node {:?}; using GLES",
                self.primary_render_node,
            );
        }
        let cross_gpu = node != self.primary_node;
        let mut render_formats = if use_native_renderer {
            let native = self.native_renderer.as_ref().unwrap();
            let vk = native.vulkan();
            vulkan_scanout_formats(&native.dmabuf_formats(), cross_gpu, |format| {
                vk.supports_target_format(format)
            })
        } else {
            prepare_gles_device(
                &mut self.gpu_manager,
                &mut device.render_node,
                &mut device.gles_classification_pending,
                &mut device.allocator,
                self.primary_render_node,
                primary_allocator.as_ref(),
            )?;
            let renderer = self
                .gpu_manager
                .single_renderer(&device.render_node.unwrap_or(self.primary_render_node))?;
            let egl_context = renderer.as_ref().egl_context();
            gles_scanout_formats(
                egl_context.dmabuf_render_formats(),
                device.render_node.is_none(),
            )
        };

        let color_formats = if self.config.as_ref().debug.disable_10bit_output {
            &SUPPORTED_COLOR_FORMATS[..]
        } else {
            &SUPPORTED_COLOR_FORMATS_10BIT[..]
        }
        .iter()
        .copied();

        let cursor_size = device.drm.cursor_size();
        let create_compositor = |device: &OutputDevice, surface, formats: FormatSet, allocator| {
            DrmCompositor::new(
                OutputModeSource::Auto(output.downgrade()),
                surface,
                None,
                allocator,
                GbmFramebufferExporter::new(device.gbm.clone(), device.render_node.into()),
                color_formats.clone(),
                formats,
                cursor_size,
                Some(device.gbm.clone()),
            )
        };
        let swapchain_allocator = if use_native_renderer {
            primary_allocator
                .clone()
                .expect("native output must have a primary GPU allocator")
        } else {
            device.allocator.clone()
        };
        let mut res =
            create_compositor(device, surface, render_formats.clone(), swapchain_allocator);
        let vulkan_probe_error = if cross_gpu && use_native_renderer {
            res.as_ref().ok().and_then(|compositor| {
                probe_vulkan_swapchain_target(
                    self.native_renderer.as_ref().unwrap(),
                    primary_allocator.as_ref().unwrap(),
                    mode.size().0 as u32,
                    mode.size().1 as u32,
                    compositor.format(),
                    compositor.modifiers(),
                )
                .err()
            })
        } else {
            None
        };
        if use_native_renderer && (res.is_err() || vulkan_probe_error.is_some()) {
            warn!(
                "Vulkan-compatible KMS swapchain unavailable for {connector_name}: KMS: {:?}, Vulkan target import: {:?}; retrying with GLES formats",
                res.as_ref().err(),
                vulkan_probe_error,
            );
            use_native_renderer = false;
            let surface = device
                .drm
                .create_surface(crtc, mode, &[connector.handle()])?;
            prepare_gles_device(
                &mut self.gpu_manager,
                &mut device.render_node,
                &mut device.gles_classification_pending,
                &mut device.allocator,
                self.primary_render_node,
                primary_allocator.as_ref(),
            )?;
            let renderer = self
                .gpu_manager
                .single_renderer(&device.render_node.unwrap_or(self.primary_render_node))?;
            let egl_context = renderer.as_ref().egl_context();
            render_formats = gles_scanout_formats(
                egl_context.dmabuf_render_formats(),
                device.render_node.is_none(),
            );
            res = create_compositor(
                device,
                surface,
                render_formats.clone(),
                device.allocator.clone(),
            );
        }
        let mut compositor = match res {
            Ok(x) => x,
            Err(err) => {
                warn!("error creating DRM compositor, will try with invalid modifier: {err:?}");
                let invalid_formats = render_formats
                    .iter()
                    .copied()
                    .filter(|format| format.modifier == Modifier::Invalid)
                    .collect::<FormatSet>();
                let surface = device
                    .drm
                    .create_surface(crtc, mode, &[connector.handle()])?;
                create_compositor(device, surface, invalid_formats, device.allocator.clone())
                    .context("error creating DRM compositor")?
            }
        };

        if use_native_renderer {
            info!(
                "output {connector_name} uses Vulkan composition on render node {:?}, KMS scanout on {:?}",
                self.primary_render_node,
                node,
            );
        } else if self.native_renderer.is_some() {
            warn!("output {connector_name} uses GLES KMS fallback while the primary renderer is Vulkan");
        }

        // Compile animation pipelines for the actual scanout format before the output's first
        // frame. A first window close should not compile a Vulkan graphics pipeline while the
        // compositor is rendering that frame.
        if use_native_renderer {
            if let Some(native) = self.native_renderer.as_mut() {
                if let Err(err) = native.prepare_output_format(compositor.format()) {
                    warn!("error preparing Vulkan animation pipelines for {connector_name}: {err}");
                }
            }
        }

        if self.debug_tint {
            compositor.set_debug_flags(DebugFlags::TINT);
        }

        let mut dmabuf_feedback = None;
        let primary_formats_opt = if use_native_renderer && self.native_renderer.is_some() {
            self.native_renderer
                .as_ref()
                .map(|native| native.dmabuf_formats())
        } else {
            self.gpu_manager
                .single_renderer(&self.primary_render_node)
                .ok()
                .map(|primary_renderer| primary_renderer.dmabuf_formats())
        };
        if let Some(primary_formats) = primary_formats_opt {
            match surface_dmabuf_feedback(
                &compositor,
                primary_formats,
                self.primary_render_node,
                device.render_node,
                self.primary_node,
                node,
            ) {
                Ok(feedback) => {
                    dmabuf_feedback = Some(feedback);
                }
                Err(err) => {
                    warn!("error building dmabuf feedback: {err:?}");
                }
            }
        }

        // Some buggy monitors replug upon powering off, so powering on here would prevent such
        // monitors from powering off. Therefore, we avoid unconditionally powering on.
        if !clarion.monitors_active {
            if let Err(err) = compositor.clear() {
                warn!("error clearing drm surface: {err:?}");
            }
        }

        let vrr_enabled = compositor.vrr_enabled();

        let names = *self
            .tracy_output_names
            .entry(connector_name.clone())
            .or_insert_with(|| TracyOutputNames {
                vblank_frame: tracy_client::FrameName::new_leak(format!(
                    "vblank on {connector_name}"
                )),
                time_since_presentation: tracy_client::PlotName::new_leak(format!(
                    "{connector_name} time since presentation, ms"
                )),
                presentation_misprediction: tracy_client::PlotName::new_leak(format!(
                    "{connector_name} presentation misprediction, ms"
                )),
                sequence_delta: tracy_client::PlotName::new_leak(format!(
                    "{connector_name} sequence delta"
                )),
            });

        let surface = Surface {
            name: output_name,
            connector: connector.handle(),
            compositor,
            native_swapchain: use_native_renderer,
            last_gles_migration_warning: None,
            next_gles_migration_attempt: Duration::ZERO,
            dmabuf_feedback,
            gamma_props,
            pending_gamma_change: None,
            vblank_frame: None,
            vblank_frame_name: names.vblank_frame,
            time_since_presentation_plot_name: names.time_since_presentation,
            presentation_misprediction_plot_name: names.presentation_misprediction,
            sequence_delta_plot_name: names.sequence_delta,
        };

        let res = device.surfaces.insert(crtc, surface);
        assert!(res.is_none(), "crtc must not have already existed");

        clarion.add_output(
            output.clone(),
            Some(refresh_interval(mode)),
            vrr_enabled,
            true,
        );

        if clarion.monitors_active {
            // Redraw the new monitor.
            clarion.event_loop.insert_idle(move |state| {
                // Guard against output disconnecting before the idle has a chance to run.
                if state.clarion.output_state.contains_key(&output) {
                    state.clarion.queue_redraw(&output);
                }
            });
        }

        Ok(())
    }

    fn platform_connector_disconnected(
        &mut self,
        clarion: &mut Clarion,
        node: DrmNode,
        crtc: crtc::Handle,
    ) {
        let Some(device) = self.devices.get_mut(&node) else {
            debug!("disconnecting connector for crtc: {crtc:?}");
            error!("missing device");
            return;
        };

        let Some(surface) = device.surfaces.remove(&crtc) else {
            debug!("disconnecting connector for crtc: {crtc:?}");

            if let Some((conn, _)) = device
                .non_desktop_connectors
                .iter()
                .find(|(_, crtc_)| *crtc_ == crtc)
            {
                debug!("withdrawing non-desktop connector from DRM leasing");

                let conn = *conn;
                device.non_desktop_connectors.remove(&(conn, crtc));

                if let Some(lease_state) = &mut device.drm_lease_state {
                    lease_state.withdraw_connector(conn);
                }
            } else {
                debug!("crtc wasn't enabled");
            }

            return;
        };

        debug!("disconnecting connector: {:?}", surface.name.connector);

        let output = clarion
            .global_space
            .outputs()
            .find(|output| {
                let tty_state: &TtyOutputState = output.user_data().get().unwrap();
                tty_state.node == node && tty_state.crtc == crtc
            })
            .cloned();
        if let Some(output) = output {
            clarion.remove_output(&output);
        } else {
            error!("missing output for crtc {crtc:?}");
        };
    }

    fn platform_on_vblank(
        &mut self,
        clarion: &mut Clarion,
        node: DrmNode,
        crtc: crtc::Handle,
        meta: DrmEventMetadata,
    ) {
        let span = clarion_tracy_span!("Tty::on_vblank");

        let now = get_monotonic_time();

        let Some(device) = self.devices.get_mut(&node) else {
            // I've seen it happen.
            error!("missing device in vblank callback for crtc {crtc:?}");
            return;
        };

        let Some(surface) = device.surfaces.get_mut(&crtc) else {
            error!("missing surface in vblank callback for crtc {crtc:?}");
            return;
        };

        // Finish the Tracy frame, if any.
        drop(surface.vblank_frame.take());

        let name = &surface.name.connector;
        trace!("vblank on {name} {meta:?}");
        span.emit_text(name);

        let presentation_time = match meta.time {
            DrmEventTime::Monotonic(time) => time,
            DrmEventTime::Realtime(_) => {
                // Not supported.

                // This value will be ignored in the frame clock code.
                Duration::ZERO
            }
        };
        let presentation_time = if clarion.config.borrow().debug.emulate_zero_presentation_time {
            Duration::ZERO
        } else {
            presentation_time
        };

        let message = if presentation_time.is_zero() {
            format!("vblank on {name}, presentation time unknown")
        } else if presentation_time > now {
            let diff = presentation_time - now;
            tracy_client::Client::running().unwrap().plot(
                surface.time_since_presentation_plot_name,
                -diff.as_secs_f64() * 1000.,
            );
            format!("vblank on {name}, presentation is {diff:?} later")
        } else {
            let diff = now - presentation_time;
            tracy_client::Client::running().unwrap().plot(
                surface.time_since_presentation_plot_name,
                diff.as_secs_f64() * 1000.,
            );
            format!("vblank on {name}, presentation was {diff:?} ago")
        };
        tracy_client::Client::running()
            .unwrap()
            .message(&message, 0);

        let Some(output) = clarion
            .global_space
            .outputs()
            .find(|output| {
                let tty_state: &TtyOutputState = output.user_data().get().unwrap();
                tty_state.node == node && tty_state.crtc == crtc
            })
            .cloned()
        else {
            error!("missing output in global space for {name}");
            return;
        };

        let Some(output_state) = clarion.output_state.get_mut(&output) else {
            error!("missing output state for {name}");
            return;
        };

        let refresh_interval = output_state.schedule.clock().refresh_interval();

        let time = if presentation_time.is_zero() {
            now
        } else {
            presentation_time
        };

        if output_state
            .vblank_throttle
            .throttle(refresh_interval, time, move |state| {
                let meta = DrmEventMetadata {
                    sequence: meta.sequence,
                    time: DrmEventTime::Monotonic(Duration::ZERO),
                };

                let tty = state.backend.tty();
                tty.platform_on_vblank(&mut state.clarion, node, crtc, meta);
            })
        {
            // Throttled.
            return;
        }

        let redraw_needed = output_state
            .schedule
            .hardware_vblank(&clarion.event_loop, name);

        // Mark the last frame as submitted.
        let mut submitted_target = None;
        match surface.compositor.frame_submitted() {
            Ok(Some((mut feedback, target_presentation_time))) => {
                submitted_target = Some(target_presentation_time);
                output_state
                    .schedule
                    .record_presentation_feedback(presentation_time, target_presentation_time);
                let refresh = match refresh_interval {
                    Some(refresh) => {
                        if output_state.schedule.clock().vrr() {
                            Refresh::Variable(refresh)
                        } else {
                            Refresh::Fixed(refresh)
                        }
                    }
                    None => Refresh::Unknown,
                };

                // wp_presentation requires the sequence to be monotonically
                // increasing per feedback object. A compositor-wide counter
                // satisfies that even when a surface moves between outputs or
                // CRTCs, unlike raw DRM vblank counters which restart per CRTC.
                let seq = next_presentation_seq();
                let mut flags = wp_presentation_feedback::Kind::Vsync
                    | wp_presentation_feedback::Kind::HwCompletion;

                if !presentation_time.is_zero() {
                    flags.insert(wp_presentation_feedback::Kind::HwClock);
                }

                feedback.presented::<_, smithay::utils::Monotonic>(time, refresh, seq, flags);

                if !presentation_time.is_zero() {
                    let misprediction_s =
                        presentation_time.as_secs_f64() - target_presentation_time.as_secs_f64();
                    tracy_client::Client::running().unwrap().plot(
                        surface.presentation_misprediction_plot_name,
                        misprediction_s * 1000.,
                    );
                }
            }
            Ok(None) => (),
            Err(err) => {
                warn!("error marking frame as submitted: {err}");
            }
        }

        if let Some(delta) = output_state.schedule.record_drm_sequence(meta.sequence) {
            tracy_client::Client::running()
                .unwrap()
                .plot(surface.sequence_delta_plot_name, delta as f64);
        }
        output_state
            .schedule
            .presented(presentation_time, submitted_target);

        if redraw_needed || output_state.schedule.animations_remain() {
            let vblank_frame = tracy_client::Client::running()
                .unwrap()
                .non_continuous_frame(surface.vblank_frame_name);
            surface.vblank_frame = Some(vblank_frame);

            clarion.queue_redraw(&output);
        } else {
            clarion.send_frame_callbacks(&output);
        }
    }

    fn platform_on_estimated_vblank_timer(clarion: &mut Clarion, output: Output) {
        clarion.release_fifo_after_estimated_vblank(&output);
        let span = clarion_tracy_span!("Tty::on_estimated_vblank_timer");

        let name = output.name();
        span.emit_text(&name);

        let Some(output_state) = clarion.output_state.get_mut(&output) else {
            error!("missing output state for {name}");
            return;
        };

        match output_state.schedule.estimated_vblank() {
            EstimatedVblankAction::SendCallbacks => clarion.send_frame_callbacks(&output),
            EstimatedVblankAction::QueueRedraw => clarion.queue_redraw(&output),
            EstimatedVblankAction::RedrawAlreadyQueued => (),
        }
    }

    fn platform_refresh_ipc_outputs(&self, clarion: &mut Clarion) {
        let _span = clarion_tracy_span!("Tty::refresh_ipc_outputs");

        let mut ipc_outputs = HashMap::new();
        let disable_monitor_names = self.config.as_ref().debug.disable_monitor_names;

        for (node, device) in &self.devices {
            for (connector, crtc) in device.drm_scanner.crtcs() {
                let connector_name = format_connector_name(connector);
                let physical_size = connector.size();
                let output_name = device.known_crtc_name(&crtc, connector, disable_monitor_names);

                let surface = device.surfaces.get(&crtc);
                let current_crtc_mode = surface.map(|surface| surface.compositor.pending_mode());
                let mut current_mode = None;
                let mut is_custom_mode = false;

                let mut modes: Vec<clarion_ipc::Mode> = connector
                    .modes()
                    .iter()
                    .filter(|m| !m.flags().contains(ModeFlags::INTERLACE))
                    .enumerate()
                    .map(|(idx, m)| {
                        if Some(*m) == current_crtc_mode {
                            current_mode = Some(idx);
                        }

                        clarion_ipc::Mode {
                            width: m.size().0,
                            height: m.size().1,
                            refresh_rate: Mode::from(*m).refresh as u32,
                            is_preferred: m.mode_type().contains(ModeTypeFlags::PREFERRED),
                        }
                    })
                    .collect();

                if let Some(crtc_mode) = current_crtc_mode {
                    // Custom mode
                    if crtc_mode.mode_type().contains(ModeTypeFlags::USERDEF) {
                        modes.insert(
                            0,
                            clarion_ipc::Mode {
                                width: crtc_mode.size().0,
                                height: crtc_mode.size().1,
                                refresh_rate: Mode::from(crtc_mode).refresh as u32,
                                is_preferred: false,
                            },
                        );
                        current_mode = Some(0);
                        is_custom_mode = true;
                    }

                    if current_mode.is_none() {
                        if crtc_mode.flags().contains(ModeFlags::INTERLACE) {
                            warn!("connector mode list missing current mode (interlaced)");
                        } else {
                            error!("connector mode list missing current mode");
                        }
                    }
                }

                let vrr_supported = surface
                    .map(|surface| {
                        matches!(
                            surface.compositor.vrr_supported(connector.handle()),
                            Ok(VrrSupport::Supported | VrrSupport::RequiresModeset)
                        )
                    })
                    .unwrap_or_else(|| {
                        is_vrr_capable(&device.drm, connector.handle()) == Some(true)
                    });
                let vrr_enabled = surface.is_some_and(|surface| surface.compositor.vrr_enabled());

                let logical = clarion
                    .global_space
                    .outputs()
                    .find(|output| {
                        let tty_state: &TtyOutputState = output.user_data().get().unwrap();
                        tty_state.node == *node && tty_state.crtc == crtc
                    })
                    .map(logical_output);

                let id = device.known_crtcs.get(&crtc).map(|info| info.id);
                let id = id.unwrap_or_else(|| {
                    error!("crtc for connector {connector_name} missing from known");
                    OutputId::next()
                });

                let props = ConnectorProperties::try_new(&device.drm, connector.handle()).ok();
                let max_bpc = props.as_ref().and_then(|p| p.find(c"max bpc").ok());
                let max_bpc = max_bpc.and_then(|(info, value)| {
                    info.value_type()
                        .convert_value(*value)
                        .as_unsigned_range()
                        .map(|v| v as u8)
                });
                let hdr_active = props
                    .as_ref()
                    .and_then(|p| p.find(c"HDR_OUTPUT_METADATA").ok())
                    .is_some_and(|(_, value)| *value != 0);

                let ipc_output = clarion_ipc::Output {
                    name: connector_name,
                    make: output_name.make.unwrap_or_else(|| "Unknown".into()),
                    model: output_name.model.unwrap_or_else(|| "Unknown".into()),
                    serial: output_name.serial,
                    physical_size,
                    modes,
                    current_mode,
                    is_custom_mode,
                    vrr_supported,
                    vrr_enabled,
                    logical,
                    max_bpc,
                    hdr_active,
                };

                ipc_outputs.insert(id, ipc_output);
            }
        }

        let mut guard = self.ipc_outputs.lock().unwrap();
        *guard = ipc_outputs;
        clarion.ipc_outputs_changed = true;
    }

    fn platform_set_output_on_demand_vrr(
        &mut self,
        clarion: &mut Clarion,
        output: &Output,
        enable_vrr: bool,
    ) {
        let _span = clarion_tracy_span!("Tty::set_output_on_demand_vrr");

        let current_vrr = {
            let output_state = clarion.output_state.get_mut(output).unwrap();
            output_state.on_demand_vrr_enabled = enable_vrr;
            output_state.schedule.clock().vrr()
        };
        if current_vrr == enable_vrr {
            return;
        }
        for (&node, device) in self.devices.iter_mut() {
            for (&crtc, surface) in device.surfaces.iter_mut() {
                let tty_state: &TtyOutputState = output.user_data().get().unwrap();
                if tty_state.node == node && tty_state.crtc == crtc {
                    let word = if enable_vrr { "enabling" } else { "disabling" };
                    if let Err(err) = surface.compositor.use_vrr(enable_vrr) {
                        warn!(
                            "output {:?}: error {} VRR: {err:?}",
                            surface.name.connector, word
                        );
                    }
                    // A pending latch timer was calculated using the previous VRR state.
                    clarion.cancel_latch_wait(output);
                    clarion
                        .output_state
                        .get_mut(output)
                        .unwrap()
                        .schedule
                        .set_vrr(surface.compositor.vrr_enabled());

                    self.platform_refresh_ipc_outputs(clarion);
                    return;
                }
            }
        }
    }

    fn platform_update_ignored_nodes_config(&mut self, clarion: &mut Clarion) {
        let _span = clarion_tracy_span!("Tty::update_ignored_nodes_config");

        // If we're inactive, we can't do anything, but we'll recompute in ActivateSession.
        if !self.session.is_active() {
            return;
        }

        let ignored_nodes = self.compute_ignored_nodes();
        if ignored_nodes == self.ignored_nodes {
            return;
        }
        self.ignored_nodes = ignored_nodes;

        let mut device_list = self
            .udev_inventory
            .dispatcher
            .as_source_ref()
            .device_list()
            .map(|(device_id, path)| (device_id, path.to_owned()))
            .collect::<HashMap<_, _>>();

        let removed_devices = self
            .devices
            .keys()
            .filter(|node| {
                self.ignored_nodes.contains(node) || !device_list.contains_key(&node.dev_id())
            })
            .copied()
            .collect::<Vec<_>>();

        for node in removed_devices {
            device_list.remove(&node.dev_id());
            self.platform_device_removed(node.dev_id(), clarion);
        }

        for node in self.devices.keys() {
            device_list.remove(&node.dev_id());
        }

        for (device_id, path) in device_list {
            if let Err(err) = self.platform_device_added(device_id, &path, clarion) {
                warn!("error adding device {path:?}: {err:?}");
            }
        }
    }

    fn platform_on_output_config_changed(&mut self, clarion: &mut Clarion) {
        let _span = clarion_tracy_span!("Tty::on_output_config_changed");

        // If we're inactive, we can't do anything, so just set a flag for later.
        if !self.session.is_active() {
            self.update_output_config_on_resume = true;
            return;
        }
        self.update_output_config_on_resume = false;

        // Figure out if we should disable laptop panels.
        let disable_laptop_panels = self.should_disable_laptop_panels(clarion.is_lid_closed);
        let should_disable = |connector: &str| disable_laptop_panels && is_laptop_panel(connector);

        let mut to_disconnect = vec![];
        let mut to_connect = vec![];

        for (&node, device) in &mut self.devices {
            for (&crtc, surface) in device.surfaces.iter_mut() {
                let config = self
                    .config
                    .as_ref()
                    .outputs
                    .find(&surface.name)
                    .cloned()
                    .unwrap_or_default();
                if config.off || should_disable(&surface.name.connector) {
                    to_disconnect.push((node, crtc));
                    continue;
                }

                // Check if we need to change the mode.
                let Some(connector) = device.drm_scanner.connectors().get(&surface.connector)
                else {
                    error!("missing enabled connector in drm_scanner");
                    continue;
                };

                let mut mode = None;
                if let Some(modeline) = &config.modeline {
                    match calculate_drm_mode_from_modeline(modeline) {
                        Ok(x) => mode = Some(x),
                        Err(err) => {
                            warn!(
                                "output {:?}: invalid custom modeline; \
                                 falling back to advertised modes: {err:?}",
                                surface.name.connector
                            );
                        }
                    }
                }

                let (mode, fallback) = match mode {
                    Some(x) => (x, false),
                    None => match pick_mode(connector, config.mode) {
                        Some(result) => result,
                        None => {
                            warn!("couldn't pick mode for enabled connector");
                            continue;
                        }
                    },
                };

                if let Ok(mut props) = ConnectorProperties::try_new(&device.drm, surface.connector)
                {
                    // PQ encoding only exists on the native Vulkan path;
                    // sending HDR10 metadata for a GLES-rendered output would
                    // misrender SDR content as PQ.
                    let hdr = config.hdr && surface.native_swapchain;
                    if config.hdr && !surface.native_swapchain {
                        debug!(
                            "HDR requested on a GLES-rendered output; \
                             keeping HDR metadata disabled"
                        );
                    }
                    set_connector_properties(&mut props, config.max_bpc, hdr);
                } else {
                    warn!("failed to get connector properties");
                }

                let change_mode = surface.compositor.pending_mode() != mode;

                let vrr_enabled = surface.compositor.vrr_enabled();
                let change_always_vrr = vrr_enabled != config.is_vrr_always_on();
                let is_on_demand_vrr = config.is_vrr_on_demand();

                if !change_mode && !change_always_vrr && !is_on_demand_vrr {
                    continue;
                }

                let output = clarion
                    .global_space
                    .outputs()
                    .find(|output| {
                        let tty_state: &TtyOutputState = output.user_data().get().unwrap();
                        tty_state.node == node && tty_state.crtc == crtc
                    })
                    .cloned();
                let Some(output) = output else {
                    error!("missing output for crtc: {crtc:?}");
                    continue;
                };
                let Some(on_demand_vrr_enabled) = clarion
                    .output_state
                    .get(&output)
                    .map(|state| state.on_demand_vrr_enabled)
                else {
                    error!("missing state for output {:?}", surface.name.connector);
                    continue;
                };

                let change_vrr = (is_on_demand_vrr && vrr_enabled != on_demand_vrr_enabled)
                    || (!is_on_demand_vrr && change_always_vrr);
                if change_mode || change_vrr {
                    // Both changes invalidate a deadline calculated from the previous refresh
                    // interval or VRR state. Keep the damage queued for immediate rescheduling.
                    clarion.cancel_latch_wait(&output);
                }

                let output_state = clarion.output_state.get_mut(&output).unwrap();
                if change_vrr {
                    let vrr = !vrr_enabled;
                    let word = if vrr { "enabling" } else { "disabling" };
                    if let Err(err) = surface.compositor.use_vrr(vrr) {
                        warn!(
                            "output {:?}: error {} VRR: {err:?}",
                            surface.name.connector, word
                        );
                    }
                    output_state
                        .schedule
                        .set_vrr(surface.compositor.vrr_enabled());
                }

                if change_mode {
                    if fallback {
                        let target = config.mode.unwrap();
                        warn!(
                            "output {:?}: configured mode {}x{}{} could not be found, \
                             falling back to preferred",
                            surface.name.connector,
                            target.mode.width,
                            target.mode.height,
                            if let Some(refresh) = target.mode.refresh {
                                format!("@{refresh}")
                            } else {
                                String::new()
                            },
                        );
                    }

                    debug!(
                        "output {:?}: picking mode: {mode:?}",
                        surface.name.connector
                    );
                    if let Err(err) = surface.compositor.use_mode(mode) {
                        warn!("error changing mode: {err:?}");
                        continue;
                    }

                    let wl_mode = Mode::from(mode);
                    output.change_current_state(Some(wl_mode), None, None, None);
                    output.set_preferred(wl_mode);
                    output_state.schedule.reset_timing(
                        Some(refresh_interval(mode)),
                        surface.compositor.vrr_enabled(),
                    );
                    clarion.output_resized(&output);
                }
            }

            let config = self.config.as_ref();
            let disable_monitor_names = config.debug.disable_monitor_names;

            for (connector, crtc) in device.drm_scanner.crtcs() {
                // Check if connected.
                if connector.state() != connector::State::Connected {
                    continue;
                }

                // Check if already enabled.
                if device.surfaces.contains_key(&crtc)
                    || device
                        .non_desktop_connectors
                        .contains(&(connector.handle(), crtc))
                {
                    continue;
                }

                let output_name = device.known_crtc_name(&crtc, connector, disable_monitor_names);

                let config = config
                    .outputs
                    .find(&output_name)
                    .cloned()
                    .unwrap_or_default();

                if !(config.off || should_disable(&output_name.connector)) {
                    to_connect.push((node, connector.clone(), crtc, output_name));
                }
            }
        }

        for (node, crtc) in to_disconnect {
            self.platform_connector_disconnected(clarion, node, crtc);
        }

        // Sort by output name to get more predictable first focused output at initial compositor
        // startup, when multiple connectors appear at once.
        to_connect.sort_unstable_by(|a, b| a.3.compare(&b.3));

        for (node, connector, crtc, _name) in to_connect {
            if let Err(err) = self.platform_connector_connected(clarion, node, connector, crtc) {
                warn!("error connecting connector: {err:?}");
            }
        }

        self.platform_refresh_ipc_outputs(clarion);
    }
}

pub(super) fn initialize(device: &mut Tty, clarion: &mut Clarion) {
    device.platform_init(clarion);
}
pub(super) fn set_vrr(device: &mut Tty, clarion: &mut Clarion, output: &Output, enabled: bool) {
    device.platform_set_output_on_demand_vrr(clarion, output, enabled);
}
pub(super) fn update_ignored_nodes(device: &mut Tty, clarion: &mut Clarion) {
    device.platform_update_ignored_nodes_config(clarion);
}
pub(super) fn configure_outputs(device: &mut Tty, clarion: &mut Clarion) {
    device.platform_on_output_config_changed(clarion);
}
pub(super) fn estimated_vblank(clarion: &mut Clarion, output: Output) {
    Tty::platform_on_estimated_vblank_timer(clarion, output);
}
pub(super) fn on_udev(state: &mut State, event: UdevEvent) {
    state
        .backend
        .tty()
        .platform_on_udev_event(&mut state.clarion, event);
}
pub(super) fn on_session(state: &mut State, event: SessionEvent) {
    state
        .backend
        .tty()
        .platform_on_session_event(&mut state.clarion, event);
}
