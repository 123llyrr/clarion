//! Scene-owned retained resources for workspace background and overview shadows.
//! The workspace layout owner keeps only a weak registration handle.

use std::cell::RefCell;

use super::shadow::Shadow;
use crate::render_helpers::solid_color::SolidColorBuffer;

#[derive(Debug)]
pub struct WorkspaceResources {
    pub(crate) shadow: RefCell<Shadow>,
    pub(crate) background_buffer: RefCell<SolidColorBuffer>,
}

impl WorkspaceResources {
    pub fn new(shadow: Shadow, background_buffer: SolidColorBuffer) -> Self {
        Self {
            shadow: RefCell::new(shadow),
            background_buffer: RefCell::new(background_buffer),
        }
    }
}

impl Default for WorkspaceResources {
    fn default() -> Self {
        Self {
            shadow: RefCell::new(Shadow::new(Default::default())),
            background_buffer: RefCell::new(SolidColorBuffer::default()),
        }
    }
}
