//! Renderer initialization policy, independent of DRM/EGL/Vulkan constructors.

use std::collections::HashMap;
use std::hash::Hash;

/// Resolve hardware rendering without making native Vulkan depend on an EGL probe.
pub(crate) fn initialize_device_renderer<N: Copy, C>(
    native_available: bool,
    drm_render_node: Option<N>,
    context: &mut C,
    register_deferred: impl FnOnce(&mut C, N),
    initialize_egl: impl FnOnce(&mut C) -> Option<N>,
) -> Option<N> {
    if native_available {
        if let Some(node) = drm_render_node {
            register_deferred(context, node);
        }
        drm_render_node
    } else {
        initialize_egl(context)
    }
}

/// Classify a device only when a native output actually needs GLES compatibility.
pub(crate) fn resolve_gles_render_node<N: Copy + Eq, C, E>(
    render_node: Option<N>,
    primary: N,
    context: &mut C,
    mut initialize_egl: impl FnMut(&mut C, N) -> Result<(), E>,
) -> Result<Option<N>, E> {
    let Some(node) = render_node else {
        initialize_egl(context, primary)?;
        return Ok(None);
    };
    match initialize_egl(context, node) {
        Ok(()) => Ok(Some(node)),
        Err(err) if node == primary => Err(err),
        Err(_) => {
            initialize_egl(context, primary)?;
            Ok(None)
        }
    }
}

struct DeferredNode<G> {
    /// Only pending registrations own a resource; the renderer manager retains
    /// its own GBM reference after successful initialization.
    pending: Option<G>,
}

/// Remember GBM handles without running any compatibility renderer constructor.
pub(crate) struct DeferredNodes<N, G> {
    nodes: HashMap<N, DeferredNode<G>>,
}

impl<N, G> Default for DeferredNodes<N, G> {
    fn default() -> Self {
        Self {
            nodes: HashMap::new(),
        }
    }
}

impl<N: Eq + Hash, G> DeferredNodes<N, G> {
    pub(crate) fn register(&mut self, node: N, resource: G, initialized: bool) {
        // Match GbmGlesBackend::add_node: duplicate render nodes retain their
        // first pending GBM handle. Once registered, retaining another clone here
        // would unnecessarily keep a removed primary device's FD alive.
        self.nodes
            .entry(node)
            .and_modify(|entry| {
                if initialized {
                    entry.pending = None;
                }
            })
            .or_insert(DeferredNode {
                pending: (!initialized).then_some(resource),
            });
    }

    pub(crate) fn initialize<E>(
        &mut self,
        node: &N,
        initialize: impl FnOnce(&G) -> Result<(), E>,
    ) -> Result<bool, E> {
        let Some(entry) = self.nodes.get_mut(node) else {
            return Ok(false);
        };
        if let Some(resource) = entry.pending.as_ref() {
            initialize(resource)?;
            entry.pending = None;
        }
        Ok(true)
    }

    pub(crate) fn is_initialized(&self, node: &N) -> bool {
        self.nodes
            .get(node)
            .is_some_and(|entry| entry.pending.is_none())
    }

    pub(crate) fn remove(&mut self, node: &N) {
        self.nodes.remove(node);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use super::*;

    #[test]
    fn native_startup_does_not_probe_egl_for_intel_or_nvidia() {
        for render_node in [128, 129] {
            let egl_calls = Cell::new(0);
            let deferred = Cell::new(None);
            let node = initialize_device_renderer(
                true,
                Some(render_node),
                &mut (),
                |_, node| deferred.set(Some(node)),
                |_| {
                    egl_calls.set(egl_calls.get() + 1);
                    None // Missing or broken EGL must not disable a working Vulkan device.
                },
            );
            assert_eq!(egl_calls.get(), 0, "native startup called EGL");
            assert_eq!(node, Some(render_node));
            assert_eq!(deferred.get(), Some(render_node));
        }
    }

    #[test]
    fn native_display_only_device_needs_neither_egl_nor_a_local_renderer() {
        let node = initialize_device_renderer::<u32, _>(
            true,
            None,
            &mut (),
            |_, _| panic!("display-only node has no local renderer to register"),
            |_| panic!("Vulkan can scan out through the primary allocator without EGL"),
        );
        assert_eq!(node, None);
    }

    #[test]
    fn gles_or_failed_native_startup_retains_egl_discovered_mapping() {
        let node = initialize_device_renderer(
            false,
            Some(128),
            &mut (),
            |_, _| panic!("GLES must validate hardware and preserve its discovered mapping"),
            |_| Some(129),
        );
        assert_eq!(node, Some(129));
        assert_eq!(
            initialize_device_renderer(false, Some(128), &mut (), |_, _| {}, |_| None),
            None
        );
    }

    #[test]
    fn compatibility_registration_is_lazy_deduplicated_and_retryable() {
        let mut nodes = DeferredNodes::default();
        nodes.register(128, "intel", false);
        nodes.register(128, "same render node through another DRM device", false);
        let calls = Cell::new(0);
        assert!(!nodes.is_initialized(&128));
        assert_eq!(
            nodes.initialize(&128, |_| {
                calls.set(calls.get() + 1);
                Err("EGL temporarily unavailable")
            }),
            Err("EGL temporarily unavailable")
        );
        assert!(!nodes.is_initialized(&128));
        assert_eq!(
            nodes.initialize(&128, |resource| {
                assert_eq!(*resource, "intel");
                calls.set(calls.get() + 1);
                Ok::<_, &str>(())
            }),
            Ok(true)
        );
        assert!(nodes.is_initialized(&128));
        assert_eq!(
            nodes.initialize(&128, |_| -> Result<(), &str> { panic!("registered twice") }),
            Ok(true)
        );
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn removing_pending_or_initialized_devices_never_initializes_them_and_allows_readd() {
        let mut nodes = DeferredNodes::default();
        for initialized in [false, true] {
            nodes.register(129, "old NVIDIA fd", initialized);
            nodes.remove(&129);
            assert!(!nodes.is_initialized(&129));
            assert_eq!(
                nodes.initialize(&129, |_| -> Result<(), &str> {
                    panic!("removed device initialized")
                }),
                Ok(false)
            );
            nodes.register(129, "new NVIDIA fd", false);
            assert_eq!(
                nodes.initialize(&129, |resource| {
                    assert_eq!(*resource, "new NVIDIA fd");
                    Ok::<_, &str>(())
                }),
                Ok(true)
            );
            nodes.remove(&129);
        }
    }

    #[test]
    fn activated_nodes_leave_no_redundant_gbm_reference_in_the_pending_cache() {
        let mut nodes = DeferredNodes::default();
        let resource = Rc::new("first primary fd");
        let weak = Rc::downgrade(&resource);
        nodes.register(128, resource, false);
        assert!(weak.upgrade().is_some());
        // A real manager takes any references it needs inside this callback.
        nodes.initialize(&128, |_| Ok::<_, ()>(())).unwrap();
        assert!(nodes.is_initialized(&128));
        assert!(weak.upgrade().is_none());

        let redundant = Rc::new("another primary fd sharing the same render node");
        let weak_redundant = Rc::downgrade(&redundant);
        nodes.register(128, redundant, false);
        assert!(nodes.is_initialized(&128));
        assert!(weak_redundant.upgrade().is_none());

        let pending = Rc::new("pending fd before an eager fallback probe");
        let weak_pending = Rc::downgrade(&pending);
        nodes.register(129, pending, false);
        nodes.register(129, Rc::new("eager manager already owns its fd"), true);
        assert!(nodes.is_initialized(&129));
        assert!(weak_pending.upgrade().is_none());
    }

    #[test]
    fn secondary_without_egl_falls_back_to_primary_display_only_rendering() {
        let mut attempted = Vec::new();
        let resolved = resolve_gles_render_node(Some(129), 128, &mut attempted, |calls, node| {
            calls.push(node);
            if node == 129 {
                Err("secondary EGL is unavailable")
            } else {
                Ok(())
            }
        });
        assert_eq!(resolved, Ok(None));
        assert_eq!(attempted, [129, 128]);
    }

    #[test]
    fn unavailable_primary_does_not_commit_a_secondary_fallback_and_can_retry() {
        let mut attempted = Vec::new();
        let original = Some(129);
        let failed = resolve_gles_render_node(original, 128, &mut attempted, |calls, node| {
            calls.push(node);
            Err(if node == 129 {
                "secondary EGL"
            } else {
                "primary EGL"
            })
        });
        assert_eq!(failed, Err("primary EGL"));
        assert_eq!(attempted, [129, 128]);
        let retried = resolve_gles_render_node(original, 128, &mut (), |_, _| Ok::<_, ()>(()));
        assert_eq!(retried, Ok(original));
    }

    #[test]
    fn working_secondary_and_primary_failures_preserve_the_original_classification() {
        assert_eq!(
            resolve_gles_render_node(Some(129), 128, &mut (), |_, _| Ok::<_, ()>(())),
            Ok(Some(129))
        );
        let mut calls = 0;
        assert_eq!(
            resolve_gles_render_node(Some(128), 128, &mut calls, |calls, _| {
                *calls += 1;
                Err("primary EGL")
            }),
            Err("primary EGL")
        );
        assert_eq!(calls, 1);
    }
}
