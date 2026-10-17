//! Scene-owned output presentation caches. Output/layout adapters keep only a
//! weak registration token; output removal retires these resources without
//! discarding workspaces transferred to another output.

use std::cell::RefCell;

use smithay::backend::renderer::element::utils::CropRenderElement;
use smithay::backend::renderer::element::Id;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::utils::CommitCounter;
use smithay::backend::renderer::ErasedContextId;
use smithay::utils::Size;

use crate::clarion_render_elements;
use crate::layout::workspace::{WorkspaceId, WorkspaceRenderElement};
use crate::render_helpers::border::BorderRenderElement;
use crate::render_helpers::offscreen::{NativeOffscreenBuffer, OffscreenBuffer, OffscreenData};
use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
use crate::ui::mru::MruTexture;

type TextTextureCache = (String, u64, ErasedContextId, MruTexture);

clarion_render_elements! {
    CubeFaceRenderElement<R> => {
        Workspace = CropRenderElement<WorkspaceRenderElement<R>>,
        SolidColor = SolidColorRenderElement,
    }
}

/// Render-cache fingerprint; this tracks mesh damage identity, not animation
/// clocks, workspace selection or input policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CubeFaceState {
    pub(crate) workspace_idx: usize,
    pub(crate) source_commit: CommitCounter,
    pub(crate) source_texture: u64,
    pub(crate) angle_bits: u64,
    pub(crate) perspective_bits: u64,
    pub(crate) width_bits: u64,
    pub(crate) height_bits: u64,
    pub(crate) scale_bits: u64,
    pub(crate) focus_ring: bool,
}

/// Retained targets, pixel elements and presentation-cache identity. Layout
/// animation clocks, workspace selection and input policy stay in their owners.
#[derive(Debug)]
pub struct MonitorResources {
    pub(crate) cube_ids: [Id; 2],
    pub(crate) cube_commits: RefCell<[CommitCounter; 2]>,
    pub(crate) cube_face_states: RefCell<[Option<CubeFaceState>; 2]>,
    pub(crate) cube_visible_workspaces: RefCell<[Option<WorkspaceId>; 2]>,
    pub(crate) cube_background: RefCell<SolidColorBuffer>,
    pub(crate) cube_feedback: RefCell<[Option<(WorkspaceId, OffscreenData)>; 2]>,
    pub(crate) cube_buffers: [OffscreenBuffer; 2],
    pub(crate) cube_native_buffers: [NativeOffscreenBuffer; 2],
    pub(crate) cube_elements: [RefCell<Vec<CubeFaceRenderElement<GlesRenderer>>>; 2],
    pub(crate) spread_header: RefCell<Option<TextTextureCache>>,
    pub(crate) stage_rail_text: RefCell<Vec<Option<TextTextureCache>>>,
    pub(crate) stage_rail_buffers: RefCell<Vec<SolidColorBuffer>>,
    pub(crate) stage_borders: RefCell<Vec<[SolidColorBuffer; 4]>>,
    pub(crate) stage_glass_borders: RefCell<Vec<[BorderRenderElement; 2]>>,
    pub(crate) expo_borders: RefCell<Vec<[SolidColorBuffer; 4]>>,
    pub(crate) insert_hint_element: RefCell<super::insert_hint_element::InsertHintElement>,
}

impl Default for MonitorResources {
    fn default() -> Self {
        Self {
            cube_ids: [Id::new(), Id::new()],
            cube_commits: RefCell::new([CommitCounter::default(); 2]),
            cube_face_states: RefCell::new([None, None]),
            cube_visible_workspaces: RefCell::new([None, None]),
            cube_background: RefCell::new(SolidColorBuffer::new(Size::from((0., 0.)), [0.; 4])),
            cube_feedback: RefCell::new([None, None]),
            cube_buffers: std::array::from_fn(|_| OffscreenBuffer::default()),
            cube_native_buffers: std::array::from_fn(|_| NativeOffscreenBuffer::default()),
            cube_elements: std::array::from_fn(|_| RefCell::new(Vec::new())),
            spread_header: RefCell::new(None),
            stage_rail_text: RefCell::new(Vec::new()),
            stage_rail_buffers: RefCell::new(Vec::new()),
            stage_borders: RefCell::new(Vec::new()),
            stage_glass_borders: RefCell::new(Vec::new()),
            expo_borders: RefCell::new(Vec::new()),
            insert_hint_element: RefCell::new(Default::default()),
        }
    }
}

impl MonitorResources {
    pub(crate) fn clear_cube(&self) {
        for buffer in &self.cube_buffers {
            buffer.clear();
        }
        for buffer in &self.cube_native_buffers {
            buffer.clear();
        }
        for elements in &self.cube_elements {
            elements.borrow_mut().clear();
        }
        *self.cube_face_states.borrow_mut() = [None, None];
        *self.cube_feedback.borrow_mut() = [None, None];
    }

    pub(crate) fn clear_native(&self) {
        *self.cube_face_states.borrow_mut() = [None, None];
        *self.cube_feedback.borrow_mut() = [None, None];
        for buffer in &self.cube_native_buffers {
            buffer.clear();
        }
        for elements in &self.cube_elements {
            elements.borrow_mut().clear();
        }
        let mut header = self.spread_header.borrow_mut();
        if header
            .as_ref()
            .is_some_and(|(_, _, _, texture)| matches!(texture, MruTexture::Vulkan(_)))
        {
            *header = None;
        }
        for text in self.stage_rail_text.borrow_mut().iter_mut() {
            if text
                .as_ref()
                .is_some_and(|(_, _, _, texture)| matches!(texture, MruTexture::Vulkan(_)))
            {
                *text = None;
            }
        }
    }
}
