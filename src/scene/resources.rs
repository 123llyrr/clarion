//! Platform-owned retained scene resources and weak adapter capabilities.

use super::closing_window::ClosingResources;
use super::mapped_resources::MappedResources;
use super::monitor_resources::MonitorResources;
use super::retained::{ResourceAllocator, ResourceRegistry};
use super::window_resources::{WindowResourceAllocator, WindowResourceRegistry};
use super::workspace_resources::WorkspaceResources;

/// One owner is kept beside the desktop runtime by each platform instance.
/// Drop layout registrations before this owner during normal or panic teardown.
#[derive(Debug)]
pub struct SceneResources {
    windows: WindowResourceRegistry,
    closing: ResourceRegistry<ClosingResources>,
    monitors: ResourceRegistry<MonitorResources>,
    mapped: ResourceRegistry<MappedResources>,
    workspaces: ResourceRegistry<WorkspaceResources>,
}

impl SceneResources {
    #[cfg(test)]
    pub(crate) fn registration_counts(&self) -> [usize; 4] {
        [
            self.windows.len(),
            self.closing.len(),
            self.monitors.len(),
            self.mapped.len(),
        ]
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn workspace_registration_count(&self) -> usize {
        self.workspaces.len()
    }

    pub fn new() -> Self {
        Self {
            windows: WindowResourceRegistry::new(),
            closing: ResourceRegistry::new(),
            monitors: ResourceRegistry::new(),
            mapped: ResourceRegistry::new(),
            workspaces: ResourceRegistry::new(),
        }
    }

    /// Scene failures request one owner wakeup without traversing windows or
    /// exposing an event loop to composition.
    pub(crate) fn take_window_effect_wakeup(&self) -> bool {
        self.windows.take_effect_wakeup()
    }

    pub fn allocator(&self) -> SceneResourceAllocator {
        SceneResourceAllocator {
            windows: self.windows.allocator(),
            closing: self.closing.allocator(),
            monitors: self.monitors.allocator(),
            mapped: self.mapped.allocator(),
            workspaces: self.workspaces.allocator(),
        }
    }
}

impl Default for SceneResources {
    fn default() -> Self {
        Self::new()
    }
}

/// Cloneable weak access cannot extend a scene or GPU device lifetime.
#[derive(Debug, Clone)]
pub struct SceneResourceAllocator {
    pub(crate) windows: WindowResourceAllocator,
    pub(crate) closing: ResourceAllocator<ClosingResources>,
    pub(crate) monitors: ResourceAllocator<MonitorResources>,
    pub(crate) mapped: ResourceAllocator<MappedResources>,
    pub(crate) workspaces: ResourceAllocator<WorkspaceResources>,
}
