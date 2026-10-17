//! Scene-owned images captured immediately before a client commit is applied.
//! The mapped protocol adapter keeps only a weak registration. Taking a
//! snapshot transfers its image/buffer leases into the scene's resize cache.

use std::cell::RefCell;

use crate::layout::LayoutElementRenderSnapshot;
use crate::render_helpers::vulkan::VulkanTexture;

#[derive(Debug, Default)]
pub(crate) struct MappedResources {
    pub(crate) gles_snapshot: RefCell<Option<LayoutElementRenderSnapshot>>,
    pub(crate) native_snapshot: RefCell<Option<LayoutElementRenderSnapshot<VulkanTexture>>>,
}
