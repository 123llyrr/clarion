use std::cell::RefCell;
use std::cmp::{max, min};
use std::collections::HashMap;
use std::f64::consts::TAU;
use std::iter::zip;
use std::rc::Rc;

use anyhow::Context;
use arrayvec::ArrayVec;
use clarion_config::{Action, Config};
use clarion_ipc::SizeChange;
use pango::{Alignment, FontDescription};
use pangocairo::cairo::{self, ImageSurface};
use smithay::backend::allocator::Fourcc;
use smithay::backend::input::TouchSlot;
use smithay::backend::renderer::element::utils::{Relocate, RelocateRenderElement};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::Texture as _;
use smithay::input::keyboard::{Keysym, ModifiersState};
use smithay::output::{Output, WeakOutput};
use smithay::utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Size, Transform};

use crate::animation::{Animation, Clock};
use crate::clarion_render_elements;
use crate::layout::floating::DIRECTIONAL_MOVE_PX;
use crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement;
use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
use crate::render_helpers::texture::{TextureBuffer, TextureRenderElement};
use crate::render_helpers::{render_to_texture, RenderTarget};
use crate::utils::to_physical_precise_round;

const SELECTION_BORDER: i32 = 2;

const PADDING: i32 = 8;
const RADIUS: i32 = 16;
use crate::ui::theme::{FONT, ON_SURFACE, OUTLINE, SURFACE};
const BORDER: i32 = 4;
const TEXT_HIDE_P: &str =
    "Drag to select an area, then press <span face='mono' bgcolor='#2C2C2C'> Space </span> to save.\n\
     Press <span face='mono' bgcolor='#2C2C2C'> P </span> to hide the pointer.";
const TEXT_SHOW_P: &str =
    "Drag to select an area, then press <span face='mono' bgcolor='#2C2C2C'> Space </span> to save.\n\
     Press <span face='mono' bgcolor='#2C2C2C'> P </span> to show the pointer.";

// Ideally the screenshot UI should support cross-output selections. However, that poses some
// technical challenges when the outputs have different scales and such. So, this implementation
// allows only single-output selections for now.
//
// As a consequence of this, selection coordinates are in output-local coordinate space.
#[allow(clippy::large_enum_variant)]
pub enum ScreenshotUi {
    Closed {
        last_selection: Option<(WeakOutput, Rectangle<i32, Physical>)>,
        clock: Clock,
        config: Rc<RefCell<Config>>,
    },
    Open {
        selection: (Output, Point<i32, Physical>, Point<i32, Physical>),
        // An output/anchor is needed to route input even before a crop exists.
        // Remembered geometry never implicitly becomes a visible selection.
        has_selection: bool,
        output_data: HashMap<Output, OutputData>,
        button: Button,
        show_pointer: bool,
        open_anim: Animation,
        clock: Clock,
        config: Rc<RefCell<Config>>,
        path: Option<String>,
    },
}

/// State for moving the selection (as opposed to just drawing).
pub struct MoveState {
    // Cursor offset from selection.1 when starting the move.
    pointer_offset: Point<i32, Physical>,
    // If the move is initiated by a touch, this is the slot. If `None`, the move was initiated by
    // holding Space.
    touch_slot: Option<TouchSlot>,
}

pub enum Button {
    Up,
    Down {
        touch_slot: Option<TouchSlot>,
        on_capture_button: bool,
        last_pos: (Output, Point<i32, Physical>),
        move_state: Option<MoveState>,
    },
}

pub struct OutputData {
    size: Size<i32, Physical>,
    scale: f64,
    transform: Transform,
    // Output, screencast, screen capture.
    screenshot: [OutputScreenshot; 3],
    buffers: [SolidColorBuffer; 8],
    locations: [Point<i32, Physical>; 8],
    panel: Option<(ScreenshotPanel, ScreenshotPanel)>,
}

#[derive(Debug, Clone)]
pub enum ScreenshotPanel {
    Gles(TextureBuffer<GlesTexture>),
    Vulkan(TextureBuffer<crate::render_helpers::vulkan::VulkanTexture>),
}

impl ScreenshotPanel {
    pub fn size(&self) -> Size<i32, Buffer> {
        match self {
            Self::Gles(b) => b.texture().size(),
            Self::Vulkan(b) => b.texture().size(),
        }
    }

    pub fn to_render_element(
        &self,
        location: Point<f64, Logical>,
        alpha: f32,
    ) -> PrimaryGpuTextureRenderElement {
        match self {
            Self::Gles(b) => {
                PrimaryGpuTextureRenderElement::from(TextureRenderElement::from_texture_buffer(
                    b.clone(),
                    location,
                    alpha,
                    None,
                    None,
                    Kind::Unspecified,
                ))
            }
            Self::Vulkan(b) => {
                PrimaryGpuTextureRenderElement::from(TextureRenderElement::from_texture_buffer(
                    b.clone(),
                    location,
                    alpha,
                    None,
                    None,
                    Kind::Unspecified,
                ))
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum ScreenshotTexture {
    Gles(GlesTexture),
    Vulkan(crate::render_helpers::vulkan::VulkanTexture),
}

/// Selected physical size, retained capture texture, and its buffer-coordinate region.
type PreparedCapture<T> = (Size<i32, Physical>, T, Rectangle<i32, Buffer>);

pub struct OutputScreenshot {
    texture: ScreenshotTexture,
    buffer: PrimaryGpuTextureRenderElement,
    pointer: Option<PrimaryGpuTextureRenderElement>,
}

clarion_render_elements! {
    ScreenshotUiRenderElement => {
        Screenshot = PrimaryGpuTextureRenderElement,
        SolidColor = SolidColorRenderElement,
    }
}

impl Button {
    fn is_down(&self) -> bool {
        matches!(self, Self::Down { .. })
    }

    fn is_dragging_selection(&self) -> bool {
        matches!(
            self,
            Self::Down {
                on_capture_button: false,
                ..
            }
        )
    }
}

impl ScreenshotUi {
    pub fn new(clock: Clock, config: Rc<RefCell<Config>>) -> Self {
        Self::Closed {
            last_selection: None,
            clock,
            config,
        }
    }

    pub fn open(
        &mut self,
        renderer: &mut GlesRenderer,
        // Output, screencast, screen capture.
        screenshots: HashMap<Output, [OutputScreenshot; 3]>,
        default_output: Output,
        show_pointer: bool,
        path: Option<String>,
    ) -> bool {
        if screenshots.is_empty() {
            return false;
        }

        let Self::Closed {
            last_selection,
            clock,
            config,
        } = self
        else {
            return false;
        };

        let Some(selection) = initial_selection(&screenshots, default_output, last_selection)
        else {
            return false;
        };

        let selection = (
            selection.0,
            selection.1.loc,
            selection.1.loc + selection.1.size - Size::from((1, 1)),
        );

        let output_data = screenshots
            .into_iter()
            .map(|(output, screenshot)| {
                let transform = output.current_transform();
                let output_mode = output.current_mode().unwrap();
                let size = transform.transform_size(output_mode.size);
                let scale = output.current_scale().fractional_scale();
                let buffers = [
                    SolidColorBuffer::new((0., 0.), [1., 1., 1., 1.]),
                    SolidColorBuffer::new((0., 0.), [1., 1., 1., 1.]),
                    SolidColorBuffer::new((0., 0.), [1., 1., 1., 1.]),
                    SolidColorBuffer::new((0., 0.), [1., 1., 1., 1.]),
                    SolidColorBuffer::new((0., 0.), [0., 0., 0., 0.5]),
                    SolidColorBuffer::new((0., 0.), [0., 0., 0., 0.5]),
                    SolidColorBuffer::new((0., 0.), [0., 0., 0., 0.5]),
                    SolidColorBuffer::new((0., 0.), [0., 0., 0., 0.5]),
                ];
                let locations = [Default::default(); 8];

                let mut render_panel_ = |text| {
                    render_panel(renderer, scale, text)
                        .map(ScreenshotPanel::Gles)
                        .map_err(|err| warn!("error rendering help panel: {err:?}"))
                        .ok()
                };
                let panel_show = render_panel_(TEXT_SHOW_P);
                let panel_hide = render_panel_(TEXT_HIDE_P);
                let panel = Option::zip(panel_show, panel_hide);

                let data = OutputData {
                    size,
                    scale,
                    transform,
                    screenshot,
                    buffers,
                    locations,
                    panel,
                };
                (output, data)
            })
            .collect();

        let open_anim = {
            let c = config.borrow();
            Animation::new(clock.clone(), 0., 1., 0., c.animations.screenshot_ui_open.0)
        };

        *self = Self::Open {
            selection,
            has_selection: false,
            output_data,
            button: Button::Up,
            show_pointer,
            open_anim,
            clock: clock.clone(),
            config: config.clone(),
            path,
        };

        self.update_buffers();

        true
    }

    pub fn open_native(
        &mut self,
        renderer: &mut crate::render_helpers::vulkan::native::NativeRenderer,
        screenshots: HashMap<Output, [OutputScreenshot; 3]>,
        default_output: Output,
        show_pointer: bool,
        path: Option<String>,
    ) -> bool {
        if screenshots.is_empty() {
            return false;
        }

        let Self::Closed {
            last_selection,
            clock,
            config,
        } = self
        else {
            return false;
        };

        let Some(selection) = initial_selection(&screenshots, default_output, last_selection)
        else {
            return false;
        };

        let selection = (
            selection.0,
            selection.1.loc,
            selection.1.loc + selection.1.size - Size::from((1, 1)),
        );

        let output_data = screenshots
            .into_iter()
            .map(|(output, screenshot)| {
                let transform = output.current_transform();
                let output_mode = output.current_mode().unwrap();
                let size = transform.transform_size(output_mode.size);
                let scale = output.current_scale().fractional_scale();
                let buffers = [
                    SolidColorBuffer::new((0., 0.), [1., 1., 1., 1.]),
                    SolidColorBuffer::new((0., 0.), [1., 1., 1., 1.]),
                    SolidColorBuffer::new((0., 0.), [1., 1., 1., 1.]),
                    SolidColorBuffer::new((0., 0.), [1., 1., 1., 1.]),
                    SolidColorBuffer::new((0., 0.), [0., 0., 0., 0.5]),
                    SolidColorBuffer::new((0., 0.), [0., 0., 0., 0.5]),
                    SolidColorBuffer::new((0., 0.), [0., 0., 0., 0.5]),
                    SolidColorBuffer::new((0., 0.), [0., 0., 0., 0.5]),
                ];
                let locations = [Default::default(); 8];

                let mut render_panel_ = |text| {
                    render_panel(renderer, scale, text)
                        .map(ScreenshotPanel::Vulkan)
                        .map_err(|err| warn!("error rendering help panel: {err:?}"))
                        .ok()
                };
                let panel_show = render_panel_(TEXT_SHOW_P);
                let panel_hide = render_panel_(TEXT_HIDE_P);
                let panel = Option::zip(panel_show, panel_hide);

                let data = OutputData {
                    size,
                    scale,
                    transform,
                    screenshot,
                    buffers,
                    locations,
                    panel,
                };
                (output, data)
            })
            .collect();

        let open_anim = {
            let c = config.borrow();
            Animation::new(clock.clone(), 0., 1., 0., c.animations.screenshot_ui_open.0)
        };

        *self = Self::Open {
            selection,
            has_selection: false,
            output_data,
            button: Button::Up,
            show_pointer,
            open_anim,
            clock: clock.clone(),
            config: config.clone(),
            path,
        };

        self.update_buffers();

        true
    }

    /// Open the UI with screenshots captured by different output renderers.
    ///
    /// A TTY can have Vulkan KMS outputs alongside outputs that migrated to
    /// GLES. Their screenshot and help-panel textures must stay tagged with their producing
    /// renderer, so the panel map is kept per output as well.
    pub fn open_mixed(
        &mut self,
        screenshots: HashMap<Output, [OutputScreenshot; 3]>,
        panels: HashMap<Output, Option<(ScreenshotPanel, ScreenshotPanel)>>,
        default_output: Output,
        show_pointer: bool,
        path: Option<String>,
    ) -> bool {
        if screenshots.is_empty() {
            return false;
        }

        let Self::Closed {
            last_selection,
            clock,
            config,
        } = self
        else {
            return false;
        };

        let Some(selection) = initial_selection(&screenshots, default_output, last_selection)
        else {
            return false;
        };

        let selection = (
            selection.0,
            selection.1.loc,
            selection.1.loc + selection.1.size - Size::from((1, 1)),
        );

        let output_data = screenshots
            .into_iter()
            .map(|(output, screenshot)| {
                let transform = output.current_transform();
                let output_mode = output.current_mode().unwrap();
                let size = transform.transform_size(output_mode.size);
                let scale = output.current_scale().fractional_scale();
                let buffers = [
                    SolidColorBuffer::new((0., 0.), [1., 1., 1., 1.]),
                    SolidColorBuffer::new((0., 0.), [1., 1., 1., 1.]),
                    SolidColorBuffer::new((0., 0.), [1., 1., 1., 1.]),
                    SolidColorBuffer::new((0., 0.), [1., 1., 1., 1.]),
                    SolidColorBuffer::new((0., 0.), [0., 0., 0., 0.5]),
                    SolidColorBuffer::new((0., 0.), [0., 0., 0., 0.5]),
                    SolidColorBuffer::new((0., 0.), [0., 0., 0., 0.5]),
                    SolidColorBuffer::new((0., 0.), [0., 0., 0., 0.5]),
                ];
                let locations = [Default::default(); 8];
                let panel = panels.get(&output).cloned().flatten();

                let data = OutputData {
                    size,
                    scale,
                    transform,
                    screenshot,
                    buffers,
                    locations,
                    panel,
                };
                (output, data)
            })
            .collect();

        let open_anim = {
            let c = config.borrow();
            Animation::new(clock.clone(), 0., 1., 0., c.animations.screenshot_ui_open.0)
        };

        *self = Self::Open {
            selection,
            has_selection: false,
            output_data,
            button: Button::Up,
            show_pointer,
            open_anim,
            clock: clock.clone(),
            config: config.clone(),
            path,
        };

        self.update_buffers();

        true
    }

    /// Build the help panel textures with the renderer that owns an output.
    pub fn panels_gles(
        renderer: &mut GlesRenderer,
        scale: f64,
    ) -> Option<(ScreenshotPanel, ScreenshotPanel)> {
        let show = render_panel(renderer, scale, TEXT_SHOW_P)
            .map(ScreenshotPanel::Gles)
            .map_err(|err| warn!("error rendering help panel: {err:?}"))
            .ok();
        let hide = render_panel(renderer, scale, TEXT_HIDE_P)
            .map(ScreenshotPanel::Gles)
            .map_err(|err| warn!("error rendering help panel: {err:?}"))
            .ok();
        show.zip(hide)
    }

    /// Build the help panel textures with the native Vulkan renderer that owns an output.
    pub fn panels_native(
        renderer: &mut crate::render_helpers::vulkan::native::NativeRenderer,
        scale: f64,
    ) -> Option<(ScreenshotPanel, ScreenshotPanel)> {
        let show = render_panel(renderer, scale, TEXT_SHOW_P)
            .map(ScreenshotPanel::Vulkan)
            .map_err(|err| warn!("error rendering help panel: {err:?}"))
            .ok();
        let hide = render_panel(renderer, scale, TEXT_HIDE_P)
            .map(ScreenshotPanel::Vulkan)
            .map_err(|err| warn!("error rendering help panel: {err:?}"))
            .ok();
        show.zip(hide)
    }

    pub fn close(&mut self) -> bool {
        let Self::Open {
            selection,
            has_selection,
            clock,
            config,
            ..
        } = self
        else {
            return false;
        };

        let last_selection = has_selection.then(|| {
            (
                selection.0.downgrade(),
                rect_from_corner_points(selection.1, selection.2),
            )
        });

        *self = Self::Closed {
            last_selection,
            clock: clock.clone(),
            config: config.clone(),
        };

        true
    }

    pub fn toggle_pointer(&mut self) {
        if let Self::Open { show_pointer, .. } = self {
            *show_pointer = !*show_pointer;
        }
    }

    pub fn is_open(&self) -> bool {
        matches!(self, ScreenshotUi::Open { .. })
    }

    pub fn has_selection(&self) -> bool {
        matches!(
            self,
            Self::Open {
                has_selection: true,
                ..
            }
        )
    }

    pub fn can_confirm(&self) -> bool {
        matches!(
            self,
            Self::Open {
                has_selection: true,
                button: Button::Up,
                ..
            }
        )
    }

    pub fn set_space_down(&mut self, down: bool) {
        if let Self::Open {
            selection,
            has_selection: true,
            button:
                Button::Down {
                    move_state,
                    last_pos,
                    ..
                },
            ..
        } = self
        {
            if down {
                if move_state.is_none() {
                    *move_state = Some(MoveState {
                        pointer_offset: last_pos.1 - selection.1,
                        touch_slot: None,
                    });
                }
            } else {
                // Only clear if moving with Space.
                if let Some(MoveState {
                    touch_slot: None, ..
                }) = move_state
                {
                    *move_state = None;
                }
            }
        }
    }

    pub fn move_left(&mut self) {
        let Self::Open {
            selection: (output, a, b),
            has_selection: true,
            output_data,
            ..
        } = self
        else {
            return;
        };

        let data = &output_data[output];

        let delta: i32 = to_physical_precise_round(data.scale, DIRECTIONAL_MOVE_PX);
        let delta = min(delta, min(a.x, b.x));
        a.x -= delta;
        b.x -= delta;

        self.update_buffers();
    }

    pub fn move_right(&mut self) {
        let Self::Open {
            selection: (output, a, b),
            has_selection: true,
            output_data,
            ..
        } = self
        else {
            return;
        };

        let data = &output_data[output];

        let delta: i32 = to_physical_precise_round(data.scale, DIRECTIONAL_MOVE_PX);
        let delta = min(delta, data.size.w - max(a.x, b.x) - 1);
        a.x += delta;
        b.x += delta;

        self.update_buffers();
    }

    pub fn move_up(&mut self) {
        let Self::Open {
            selection: (output, a, b),
            has_selection: true,
            output_data,
            ..
        } = self
        else {
            return;
        };

        let data = &output_data[output];

        let delta: i32 = to_physical_precise_round(data.scale, DIRECTIONAL_MOVE_PX);
        let delta = min(delta, min(a.y, b.y));
        a.y -= delta;
        b.y -= delta;

        self.update_buffers();
    }

    pub fn move_down(&mut self) {
        let Self::Open {
            selection: (output, a, b),
            has_selection: true,
            output_data,
            ..
        } = self
        else {
            return;
        };

        let data = &output_data[output];

        let delta: i32 = to_physical_precise_round(data.scale, DIRECTIONAL_MOVE_PX);
        let delta = min(delta, data.size.h - max(a.y, b.y) - 1);
        a.y += delta;
        b.y += delta;

        self.update_buffers();
    }

    /// Moves the screenshot selection to a different output.
    ///
    /// This preserves the relative position while keeping logical size. It is (intentionally) very
    /// similar to how floating windows move across monitors, but with one difference: floating
    /// windows can go partially outside the view, while the screenshot selection cannot. So, we
    /// clamp it to new output bounds, trying to preserve the size if possible.
    pub fn move_to_output(&mut self, new_output: Output) {
        let Self::Open {
            selection,
            output_data,
            ..
        } = self
        else {
            return;
        };

        let (current_output, current_a, current_b) = selection;

        if current_output == &new_output {
            return;
        }

        let Some(target_data) = output_data.get(&new_output) else {
            return;
        };

        let current_data = &output_data[current_output];

        let current_rect: Rectangle<_, Physical> = Rectangle::new(
            Point::from((current_a.x.min(current_b.x), current_a.y.min(current_b.y))),
            Size::from((
                (current_a.x.max(current_b.x) - current_a.x.min(current_b.x) + 1),
                (current_a.y.max(current_b.y) - current_a.y.min(current_b.y) + 1),
            )),
        );
        let current_rect = current_rect.to_f64();

        let rel_x = current_rect.loc.x / current_data.size.w as f64;
        let rel_y = current_rect.loc.y / current_data.size.h as f64;

        let factor = target_data.scale / current_data.scale;
        let mut new_width = (current_rect.size.w * factor).round() as i32;
        let mut new_height = (current_rect.size.h * factor).round() as i32;

        new_width = new_width.clamp(1, target_data.size.w);
        new_height = new_height.clamp(1, target_data.size.h);

        let new_x = (rel_x * target_data.size.w as f64).round() as i32;
        let new_y = (rel_y * target_data.size.h as f64).round() as i32;

        let max_x = target_data.size.w - new_width;
        let max_y = target_data.size.h - new_height;
        let new_x = new_x.clamp(0, max_x);
        let new_y = new_y.clamp(0, max_y);

        let new_rect = Rectangle::new(
            Point::from((new_x, new_y)),
            Size::from((new_width, new_height)),
        );

        *selection = (
            new_output,
            new_rect.loc,
            new_rect.loc + new_rect.size - Size::from((1, 1)),
        );

        self.update_buffers();
    }

    pub fn set_width(&mut self, change: SizeChange) {
        let Self::Open {
            selection: (output, a, b),
            has_selection: true,
            output_data,
            ..
        } = self
        else {
            return;
        };

        let data = &output_data[output];

        let available_size = f64::from(data.size.w);
        let current_size = max(a.x, b.x) + 1 - min(a.x, b.x);

        let new_size = match change {
            SizeChange::SetFixed(fixed) => to_physical_precise_round(data.scale, fixed),
            SizeChange::SetProportion(prop) => {
                let prop = (prop / 100.).clamp(0., 1.);
                (available_size * prop).round() as i32
            }
            SizeChange::AdjustFixed(delta) => {
                let delta = to_physical_precise_round(data.scale, delta);
                current_size.saturating_add(delta)
            }
            SizeChange::AdjustProportion(delta) => {
                let current_prop = f64::from(current_size) / available_size;
                let prop = (current_prop + delta / 100.).clamp(0., 1.);
                (available_size * prop).round() as i32
            }
        };
        let new_size = new_size.clamp(1, data.size.w - min(a.x, b.x)) - 1;
        a.x = min(a.x, b.x);
        b.x = a.x + new_size;

        self.update_buffers();
    }

    pub fn set_height(&mut self, change: SizeChange) {
        let Self::Open {
            selection: (output, a, b),
            has_selection: true,
            output_data,
            ..
        } = self
        else {
            return;
        };

        let data = &output_data[output];

        let available_size = f64::from(data.size.h);
        let current_size = max(a.y, b.y) + 1 - min(a.y, b.y);

        let new_size = match change {
            SizeChange::SetFixed(fixed) => to_physical_precise_round(data.scale, fixed),
            SizeChange::SetProportion(prop) => {
                let prop = (prop / 100.).clamp(0., 1.);
                (available_size * prop).round() as i32
            }
            SizeChange::AdjustFixed(delta) => {
                let delta = to_physical_precise_round(data.scale, delta);
                current_size.saturating_add(delta)
            }
            SizeChange::AdjustProportion(delta) => {
                let current_prop = f64::from(current_size) / available_size;
                let prop = (current_prop + delta / 100.).clamp(0., 1.);
                (available_size * prop).round() as i32
            }
        };
        let new_size = new_size.clamp(1, data.size.h - min(a.y, b.y)) - 1;
        a.y = min(a.y, b.y);
        b.y = a.y + new_size;

        self.update_buffers();
    }

    pub fn advance_animations(&mut self) {}

    pub fn are_animations_ongoing(&self) -> bool {
        let Self::Open { open_anim, .. } = self else {
            return false;
        };

        // Rendering stops at the first arrival at full opacity. An underdamped
        // spring can keep settling after that without changing a single pixel.
        !open_anim.is_clamped_done()
    }

    fn update_buffers(&mut self) {
        let Self::Open {
            selection,
            has_selection,
            output_data,
            ..
        } = self
        else {
            panic!("screenshot UI must be open to update buffers");
        };

        if !*has_selection {
            for data in output_data.values_mut() {
                for buffer in &mut data.buffers {
                    buffer.resize((0., 0.));
                }
                data.locations.fill(Default::default());
            }
            return;
        }

        let (selection_output, a, b) = selection;
        let mut rect = rect_from_corner_points(*a, *b);

        for (output, data) in output_data {
            let buffers = &mut data.buffers;
            let locations = &mut data.locations;
            let size = data.size;
            let scale = data.scale;

            if output == selection_output {
                // Check if the selection is still valid. If not, reset it back to default.
                if !Rectangle::from_size(size).contains_rect(rect) {
                    rect = Rectangle::new(
                        Point::from((size.w / 4, size.h / 4)),
                        Size::from((size.w / 2, size.h / 2)),
                    );
                    *a = rect.loc;
                    *b = rect.loc + rect.size - Size::from((1, 1));
                }

                let border = to_physical_precise_round(scale, SELECTION_BORDER);

                let resize = move |buffer: &mut SolidColorBuffer, w: i32, h: i32| {
                    let size = Size::<_, Physical>::from((w, h));
                    buffer.resize(size.to_f64().to_logical(scale));
                };

                resize(&mut buffers[0], rect.size.w + border * 2, border);
                resize(&mut buffers[1], rect.size.w + border * 2, border);
                resize(&mut buffers[2], border, rect.size.h);
                resize(&mut buffers[3], border, rect.size.h);

                resize(&mut buffers[4], size.w, rect.loc.y);
                resize(&mut buffers[5], size.w, size.h - rect.loc.y - rect.size.h);
                resize(&mut buffers[6], rect.loc.x, rect.size.h);
                resize(
                    &mut buffers[7],
                    size.w - rect.loc.x - rect.size.w,
                    rect.size.h,
                );

                locations[0] = Point::from((rect.loc.x - border, rect.loc.y - border));
                locations[1] = Point::from((rect.loc.x - border, rect.loc.y + rect.size.h));
                locations[2] = Point::from((rect.loc.x - border, rect.loc.y));
                locations[3] = Point::from((rect.loc.x + rect.size.w, rect.loc.y));

                locations[5] = Point::from((0, rect.loc.y + rect.size.h));
                locations[6] = Point::from((0, rect.loc.y));
                locations[7] = Point::from((rect.loc.x + rect.size.w, rect.loc.y));
            } else {
                buffers[0].resize((0., 0.));
                buffers[1].resize((0., 0.));
                buffers[2].resize((0., 0.));
                buffers[3].resize((0., 0.));

                buffers[4].resize(size.to_f64().to_logical(data.scale));
                buffers[5].resize((0., 0.));
                buffers[6].resize((0., 0.));
                buffers[7].resize((0., 0.));
            }
        }
    }

    pub fn render_output(
        &self,
        output: &Output,
        target: RenderTarget,
        push: &mut dyn FnMut(ScreenshotUiRenderElement),
    ) {
        let _span = clarion_tracy_span!("ScreenshotUi::render_output");

        let Self::Open {
            output_data,
            has_selection,
            show_pointer,
            button,
            open_anim,
            ..
        } = self
        else {
            panic!("screenshot UI must be open to render it");
        };

        let Some(output_data) = output_data.get(output) else {
            return;
        };

        let scale = output_data.scale;
        let progress = open_anim.clamped_value().clamp(0., 1.) as f32;

        // The help panel goes on top.
        if let Some((show, hide)) = &output_data.panel {
            let buffer = if *show_pointer { hide } else { show };
            let alpha = if button.is_dragging_selection() {
                0.3
            } else {
                0.9
            };
            let location = panel_location(output_data, buffer.size())
                .to_f64()
                .to_logical(scale);

            let elem = buffer.to_render_element(location, alpha * progress);
            push(elem.into());
        }

        if *has_selection {
            for (buffer, loc) in zip(&output_data.buffers, &output_data.locations) {
                let elem = SolidColorRenderElement::from_buffer(
                    buffer,
                    loc.to_f64().to_logical(scale),
                    progress,
                    Kind::Unspecified,
                );
                push(elem.into());
            }
        }

        // The screenshot itself goes last.
        let index = match target {
            RenderTarget::Output => 0,
            RenderTarget::Screencast => 1,
            RenderTarget::ScreenCapture => 2,
        };
        let screenshot = &output_data.screenshot[index];

        if *show_pointer {
            if let Some(pointer) = screenshot.pointer.clone() {
                push(pointer.into());
            }
        }
        push(screenshot.buffer.clone().into());
    }

    pub(crate) fn prepare_capture(
        &self,
        renderer: &mut GlesRenderer,
    ) -> anyhow::Result<PreparedCapture<GlesTexture>> {
        let _span = clarion_tracy_span!("ScreenshotUi::capture");

        let Self::Open {
            selection,
            has_selection: true,
            output_data,
            show_pointer,
            ..
        } = self
        else {
            anyhow::bail!("screenshot UI has no selected region");
        };

        let data = output_data
            .get(&selection.0)
            .context("screenshot output disappeared")?;
        let rect = rect_from_corner_points(selection.1, selection.2);

        let screenshot = &data.screenshot[0];
        if !matches!(&screenshot.texture, ScreenshotTexture::Gles(_)) {
            anyhow::bail!("selected screenshot was captured by the Vulkan renderer");
        }

        // Composite the pointer on top if needed.
        let mut tex_rect = None;
        if *show_pointer {
            if let Some(pointer) = screenshot.pointer.clone() {
                let scale = pointer.texture_scale();
                let offset = rect.loc.upscale(-1);

                let mut elements = ArrayVec::<_, 2>::new();
                elements.push(pointer);
                elements.push(screenshot.buffer.clone());
                let elements = elements.iter().rev().map(|elem| {
                    RelocateRenderElement::from_element(elem, offset, Relocate::Relative)
                });

                let res = render_to_texture(
                    renderer,
                    rect.size,
                    scale,
                    Transform::Normal,
                    Fourcc::Abgr8888,
                    elements,
                );
                match res {
                    Ok((texture, _)) => {
                        tex_rect = Some((texture, Rectangle::from_size(rect.size)));
                    }
                    Err(err) => {
                        warn!("error compositing pointer onto screenshot: {err:?}");
                    }
                }
            }
        }

        let (texture, rect) = if let Some(tex_rect) = tex_rect {
            tex_rect
        } else {
            match &screenshot.texture {
                ScreenshotTexture::Gles(t) => (t.clone(), rect),
                ScreenshotTexture::Vulkan(_) => {
                    anyhow::bail!("selected screenshot is not GLES-compatible")
                }
            }
        };
        // The size doesn't actually matter because we're not transforming anything.
        let buf_rect = rect
            .to_logical(1)
            .to_buffer(1, Transform::Normal, &Size::from((1, 1)));

        Ok((rect.size, texture, buf_rect))
    }

    #[cfg(test)]
    pub fn capture_native(
        &self,
        renderer: &mut crate::render_helpers::vulkan::native::NativeRenderer,
    ) -> anyhow::Result<(Size<i32, Physical>, Vec<u8>)> {
        use smithay::backend::renderer::ExportMem;
        let (size, texture, region) = self.prepare_capture_native(renderer)?;
        let mapping = renderer
            .copy_texture(&texture, region, Fourcc::Abgr8888)
            .context("error copying texture")?;
        let pixels = crate::render_helpers::mapping_to_vec(renderer, &mapping)?;
        Ok((size, pixels))
    }

    pub(crate) fn prepare_capture_native(
        &self,
        renderer: &mut crate::render_helpers::vulkan::native::NativeRenderer,
    ) -> anyhow::Result<PreparedCapture<crate::render_helpers::vulkan::VulkanTexture>> {
        renderer.set_completion_export_required(false);
        let _span = clarion_tracy_span!("ScreenshotUi::capture_native");

        let Self::Open {
            selection,
            has_selection: true,
            output_data,
            show_pointer,
            ..
        } = self
        else {
            anyhow::bail!("screenshot UI has no selected region");
        };

        let data = output_data
            .get(&selection.0)
            .context("screenshot output disappeared")?;
        let rect = rect_from_corner_points(selection.1, selection.2);

        let screenshot = &data.screenshot[0];
        if !matches!(&screenshot.texture, ScreenshotTexture::Vulkan(_)) {
            anyhow::bail!("selected screenshot was captured by the GLES renderer");
        }

        // Composite the pointer on top if needed.
        let mut tex_rect = None;
        if *show_pointer {
            if let Some(pointer) = screenshot.pointer.clone() {
                let scale = pointer.texture_scale();
                let offset = rect.loc.upscale(-1);

                let mut elements = ArrayVec::<_, 2>::new();
                elements.push(pointer);
                elements.push(screenshot.buffer.clone());
                let elements = elements.iter().rev().map(|elem| {
                    RelocateRenderElement::from_element(elem, offset, Relocate::Relative)
                });

                let res = crate::render_helpers::render_to_texture(
                    renderer,
                    rect.size,
                    scale,
                    Transform::Normal,
                    Fourcc::Abgr8888,
                    elements,
                );
                match res {
                    Ok((texture, _)) => {
                        tex_rect = Some((texture, Rectangle::from_size(rect.size)));
                    }
                    Err(err) => {
                        warn!("error compositing pointer onto screenshot: {err:?}");
                    }
                }
            }
        }

        let (texture, rect) = if let Some(tex_rect) = tex_rect {
            tex_rect
        } else {
            match &screenshot.texture {
                ScreenshotTexture::Vulkan(t) => (t.clone(), rect),
                ScreenshotTexture::Gles(_) => {
                    anyhow::bail!("selected screenshot is not Vulkan-compatible")
                }
            }
        };

        let buf_rect = rect
            .to_logical(1)
            .to_buffer(1, Transform::Normal, &Size::from((1, 1)));

        Ok((rect.size, texture, buf_rect))
    }

    pub fn action(&self, raw: Keysym, mods: ModifiersState) -> Option<Action> {
        let Self::Open { .. } = self else {
            return None;
        };

        // Keep confirmation keys intercepted even with no crop or during a
        // drag. The confirmation entry point checks can_confirm(); Space's
        // drag-origin handling is applied separately by set_space_down().
        action(raw, mods)
    }

    pub fn selection_output(&self) -> Option<&Output> {
        if let Self::Open {
            selection: (output, _, _),
            ..
        } = self
        {
            Some(output)
        } else {
            None
        }
    }

    pub fn selection_uses_native(&self) -> Option<bool> {
        let Self::Open {
            selection: (output, _, _),
            output_data,
            ..
        } = self
        else {
            return None;
        };

        Some(matches!(
            &output_data.get(output)?.screenshot[0].texture,
            ScreenshotTexture::Vulkan(_)
        ))
    }

    pub fn output_size(&self, output: &Output) -> Option<(Size<i32, Physical>, f64, Transform)> {
        if let Self::Open { output_data, .. } = self {
            let data = output_data.get(output)?;
            Some((data.size, data.scale, data.transform))
        } else {
            None
        }
    }

    /// The pointer has moved to `point` relative to the current selection output.
    ///
    /// The point may be outside output bounds.
    pub fn pointer_motion(&mut self, point: Point<i32, Physical>, slot: Option<TouchSlot>) {
        let Self::Open {
            selection,
            has_selection,
            output_data,
            button:
                Button::Down {
                    touch_slot,
                    on_capture_button,
                    last_pos,
                    move_state,
                },
            ..
        } = self
        else {
            return;
        };

        if *touch_slot != slot {
            return;
        }

        last_pos.1 = point;

        if *on_capture_button {
            return;
        }

        if let Some(move_state) = move_state {
            // The cursor offset is relative to selection.1.
            let delta = point - (selection.1 + move_state.pointer_offset);

            let desired = rect_from_corner_points(selection.1 + delta, selection.2 + delta);
            let bounds = Rectangle::from_size(output_data[&selection.0].size - desired.size);
            let clamped_loc = desired.loc.constrain(bounds);

            let delta = clamped_loc - rect_from_corner_points(selection.1, selection.2).loc;
            selection.1 += delta;
            selection.2 += delta;
        } else {
            let size = output_data[&selection.0].size;
            selection.2 = Point::new(point.x.clamp(0, size.w - 1), point.y.clamp(0, size.h - 1));
            *has_selection = selection.1 != selection.2;
        }

        self.update_buffers();
    }

    pub fn pointer_down(
        &mut self,
        output: Output,
        point: Point<i32, Physical>,
        slot: Option<TouchSlot>,
        move_existing: bool,
    ) -> bool {
        let Self::Open {
            selection,
            has_selection,
            output_data,
            show_pointer,
            button,
            ..
        } = self
        else {
            return false;
        };

        // Check if this is a second touch (different slot) while already dragging.
        if let Some(new_slot) = slot {
            if let Button::Down {
                on_capture_button: false,
                move_state,
                last_pos,
                ..
            } = button
            {
                if *has_selection && move_state.is_none() {
                    *move_state = Some(MoveState {
                        pointer_offset: last_pos.1 - selection.1,
                        touch_slot: Some(new_slot),
                    });
                }
            }
        }

        if button.is_down() {
            return false;
        }

        if move_existing {
            if !*has_selection || output != selection.0 {
                return false;
            }

            *button = Button::Down {
                touch_slot: slot,
                on_capture_button: false,
                last_pos: (output, point),
                move_state: Some(MoveState {
                    pointer_offset: point - selection.1,
                    touch_slot: slot,
                }),
            };
            return true;
        }

        let Some(output_data) = output_data.get(&output) else {
            return false;
        };

        if let Some((show, hide)) = &output_data.panel {
            let buffer = if *show_pointer { hide } else { show };
            let panel_size = buffer.size();
            let location = panel_location(output_data, panel_size);

            if is_within_capture_button(output_data.scale, panel_size, point - location) {
                *button = Button::Down {
                    touch_slot: slot,
                    on_capture_button: true,
                    last_pos: (output, point),
                    move_state: None,
                };
                return false;
            }
        }

        *button = Button::Down {
            touch_slot: slot,
            on_capture_button: false,
            last_pos: (output.clone(), point),
            move_state: None,
        };

        let point = Point::new(
            point.x.clamp(0, output_data.size.w - 1),
            point.y.clamp(0, output_data.size.h - 1),
        );
        *selection = (output, point, point);
        *has_selection = false;

        self.update_buffers();

        true
    }

    pub fn pointer_up(&mut self, slot: Option<TouchSlot>) -> Option<bool> {
        let Self::Open {
            has_selection,
            output_data,
            button,
            show_pointer,
            ..
        } = self
        else {
            return None;
        };

        let Button::Down {
            touch_slot,
            on_capture_button,
            ref last_pos,
            ref mut move_state,
            ..
        } = *button
        else {
            return None;
        };

        if touch_slot != slot {
            // This is not our main touch, but it might be the move touch. If so, stop the move.
            if let Some(state) = move_state {
                if state.touch_slot.is_some_and(|m_slot| Some(m_slot) == slot) {
                    *move_state = None;
                }
            };

            return None;
        }

        let last_pos = last_pos.clone();
        *button = Button::Up;

        // Check if we released still on the capture button.
        if on_capture_button {
            let (output, point) = last_pos;

            #[allow(clippy::question_mark)]
            let Some(output_data) = output_data.get(&output) else {
                return None;
            };

            if let Some((show, hide)) = &output_data.panel {
                let buffer = if *show_pointer { hide } else { show };
                let panel_size = buffer.size();
                let location = panel_location(output_data, panel_size);

                if *has_selection
                    && is_within_capture_button(output_data.scale, panel_size, point - location)
                {
                    return Some(true);
                }
            }
        }

        self.update_buffers();

        Some(false)
    }
}

impl OutputScreenshot {
    pub fn from_textures(
        renderer: &mut GlesRenderer,
        scale: Scale<f64>,
        texture: GlesTexture,
        pointer: Option<(GlesTexture, Rectangle<i32, Physical>)>,
    ) -> Self {
        let buffer =
            PrimaryGpuTextureRenderElement::from(TextureRenderElement::from_texture_buffer(
                TextureBuffer::from_texture(
                    renderer,
                    texture.clone(),
                    scale,
                    Transform::Normal,
                    Vec::new(),
                ),
                (0., 0.),
                1.,
                None,
                None,
                Kind::Unspecified,
            ));

        let pointer = pointer.map(|(texture, geo)| {
            PrimaryGpuTextureRenderElement::from(TextureRenderElement::from_texture_buffer(
                TextureBuffer::from_texture(
                    renderer,
                    texture,
                    scale,
                    Transform::Normal,
                    Vec::new(),
                ),
                geo.to_f64().to_logical(scale).loc,
                1.,
                None,
                None,
                Kind::Unspecified,
            ))
        });

        Self {
            texture: ScreenshotTexture::Gles(texture),
            buffer,
            pointer,
        }
    }

    pub fn from_native_textures(
        renderer: &mut crate::render_helpers::vulkan::native::NativeRenderer,
        scale: Scale<f64>,
        texture: crate::render_helpers::vulkan::VulkanTexture,
        pointer: Option<(
            crate::render_helpers::vulkan::VulkanTexture,
            Rectangle<i32, Physical>,
        )>,
    ) -> Self {
        let buffer =
            PrimaryGpuTextureRenderElement::from(TextureRenderElement::from_texture_buffer(
                TextureBuffer::from_texture(
                    renderer,
                    texture.clone(),
                    scale,
                    Transform::Normal,
                    Vec::new(),
                ),
                (0., 0.),
                1.,
                None,
                None,
                Kind::Unspecified,
            ));

        let pointer = pointer.map(|(texture, geo)| {
            PrimaryGpuTextureRenderElement::from(TextureRenderElement::from_texture_buffer(
                TextureBuffer::from_texture(
                    renderer,
                    texture,
                    scale,
                    Transform::Normal,
                    Vec::new(),
                ),
                geo.to_f64().to_logical(scale).loc,
                1.,
                None,
                None,
                Kind::Unspecified,
            ))
        });

        Self {
            texture: ScreenshotTexture::Vulkan(texture),
            buffer,
            pointer,
        }
    }
}

/// Capture can fail on one monitor, and a remembered selection can outlive a mode change.
/// Only select an output with captured pixels and keep the crop inside its current bounds.
fn initial_selection(
    screenshots: &HashMap<Output, [OutputScreenshot; 3]>,
    preferred: Output,
    last_selection: &mut Option<(WeakOutput, Rectangle<i32, Physical>)>,
) -> Option<(Output, Rectangle<i32, Physical>)> {
    let previous = last_selection
        .take()
        .and_then(|(weak, rect)| weak.upgrade().map(|output| (output, rect)))
        .filter(|(output, _)| screenshots.contains_key(output));
    let (output, rect) = if let Some((output, rect)) = previous {
        (output, Some(rect))
    } else {
        let output = if screenshots.contains_key(&preferred) {
            preferred
        } else {
            screenshots
                .keys()
                .min_by_key(|output| output.name())?
                .clone()
        };
        (output, None)
    };
    let size = output
        .current_transform()
        .transform_size(output.current_mode()?.size);
    let bounds = Rectangle::from_size(size);
    let rect = rect
        .and_then(|rect| rect.intersection(bounds))
        .filter(|rect| !rect.is_empty())
        .unwrap_or_else(|| {
            Rectangle::new(
                (size.w / 4, size.h / 4).into(),
                (max(1, size.w / 2), max(1, size.h / 2)).into(),
            )
        });
    Some((output, rect))
}

fn action(raw: Keysym, mods: ModifiersState) -> Option<Action> {
    if raw == Keysym::Escape {
        return Some(Action::CancelScreenshot);
    }

    if mods.alt || mods.shift {
        return None;
    }

    if !mods.ctrl && (raw == Keysym::space || raw == Keysym::Return) {
        return Some(Action::ConfirmScreenshot {
            write_to_disk: true,
        });
    }
    if mods.ctrl && raw == Keysym::c {
        return Some(Action::ConfirmScreenshot {
            write_to_disk: false,
        });
    }

    if !mods.ctrl && raw == Keysym::p {
        return Some(Action::ScreenshotTogglePointer);
    }

    None
}

pub fn rect_from_corner_points(
    a: Point<i32, Physical>,
    b: Point<i32, Physical>,
) -> Rectangle<i32, Physical> {
    let x1 = min(a.x, b.x);
    let y1 = min(a.y, b.y);
    let x2 = max(a.x, b.x);
    let y2 = max(a.y, b.y);
    // We're adding + 1 because the pointer is clamped to output size - 1, so to get the full
    // screen worth of selection we must add back that + 1.
    Rectangle::from_extremities((x1, y1), (x2 + 1, y2 + 1))
}

fn panel_location(output_data: &OutputData, panel_size: Size<i32, Buffer>) -> Point<i32, Physical> {
    let scale = output_data.scale;
    let padding: i32 = to_physical_precise_round(scale, PADDING);
    let x = max(0, (output_data.size.w - panel_size.w) / 2);
    let y = max(0, output_data.size.h - panel_size.h - padding * 2);
    Point::from((x, y))
}

fn is_within_capture_button(
    scale: f64,
    panel_size: Size<i32, Buffer>,
    pos_within_panel: Point<i32, Physical>,
) -> bool {
    let padding: i32 = to_physical_precise_round(scale, PADDING);
    let radius = to_physical_precise_round::<i32>(scale, RADIUS) - 2;

    let xc = padding + radius;
    let yc = panel_size.h / 2;
    let pos = pos_within_panel;

    (pos.x - xc) * (pos.x - xc) + (pos.y - yc) * (pos.y - yc) <= radius * radius
}

fn render_panel<R>(
    renderer: &mut R,
    scale: f64,
    text: &str,
) -> anyhow::Result<TextureBuffer<R::TextureId>>
where
    R: smithay::backend::renderer::Renderer + smithay::backend::renderer::ImportMem,
    R::TextureId: smithay::backend::renderer::Texture + Clone + 'static,
    R::Error: std::error::Error + Send + Sync + 'static,
{
    let _span = clarion_tracy_span!("screenshot_ui::render_panel");

    let padding: i32 = to_physical_precise_round(scale, PADDING);
    // Keep the border width even to avoid blurry edges.
    let border_width = (f64::from(BORDER) / 2. * scale).round() * 2.;
    let half_border_width = (border_width / 2.) as i32;
    let radius: i32 = to_physical_precise_round(scale, RADIUS);
    let circle_stroke: f64 = to_physical_precise_round(scale, 2.);

    // Add 2 px of spacing to separate the backgrounds of the "Space" and "P" keys.
    let spacing = to_physical_precise_round::<i32>(scale, 2) * 1024;

    let mut font = FontDescription::from_string(FONT);
    font.set_absolute_size(to_physical_precise_round(scale, font.size()));

    let surface = ImageSurface::create(cairo::Format::ARgb32, 0, 0)?;
    let cr = cairo::Context::new(&surface)?;
    let layout = pangocairo::functions::create_layout(&cr);
    layout.context().set_round_glyph_positions(false);
    layout.set_font_description(Some(&font));
    layout.set_alignment(Alignment::Left);
    layout.set_markup(text);
    layout.set_spacing(spacing);

    let (mut width, mut height) = layout.pixel_size();

    width += padding + radius * 2 + padding - half_border_width + padding;
    height = max(height, radius * 2);
    height += padding * 2;

    let surface = ImageSurface::create(cairo::Format::ARgb32, width, height)?;
    let cr = cairo::Context::new(&surface)?;
    cr.set_source_rgb(SURFACE.0, SURFACE.1, SURFACE.2);
    cr.paint()?;

    let padding = f64::from(padding);
    let half_border_width = f64::from(half_border_width);
    let r = f64::from(radius);

    let yc = f64::from(height / 2);

    cr.new_sub_path();
    cr.arc(padding + r, yc, r, 0., TAU);
    cr.set_source_rgb(ON_SURFACE.0, ON_SURFACE.1, ON_SURFACE.2);
    cr.fill()?;

    cr.new_sub_path();
    cr.arc(padding + r, yc, r - circle_stroke, 0., TAU);
    cr.set_source_rgb(SURFACE.0, SURFACE.1, SURFACE.2);
    cr.fill()?;

    cr.new_sub_path();
    cr.arc(padding + r, yc, r - circle_stroke * 2., 0., TAU);
    cr.set_source_rgb(ON_SURFACE.0, ON_SURFACE.1, ON_SURFACE.2);
    cr.fill()?;

    cr.move_to(padding + r * 2. + padding - half_border_width, padding);

    let layout = pangocairo::functions::create_layout(&cr);
    layout.context().set_round_glyph_positions(false);
    layout.set_font_description(Some(&font));
    layout.set_alignment(Alignment::Left);
    layout.set_markup(text);
    layout.set_spacing(spacing);

    cr.set_source_rgb(ON_SURFACE.0, ON_SURFACE.1, ON_SURFACE.2);
    pangocairo::functions::show_layout(&cr, &layout);

    cr.move_to(0., 0.);
    cr.line_to(width.into(), 0.);
    cr.line_to(width.into(), height.into());
    cr.line_to(0., height.into());
    cr.line_to(0., 0.);
    cr.set_source_rgb(OUTLINE.0, OUTLINE.1, OUTLINE.2);
    cr.set_line_width(border_width);
    cr.stroke()?;
    drop(cr);

    let data = surface.take_data().unwrap();
    let buffer = TextureBuffer::from_memory(
        renderer,
        &data,
        Fourcc::Argb8888,
        (width, height),
        false,
        scale,
        Transform::Normal,
        Vec::new(),
    )?;

    Ok(buffer)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use clarion_config::animations::{Kind, ScreenshotUiOpenAnim, WindowMovementAnim};
    use smithay::output::{PhysicalProperties, Subpixel};

    use super::*;

    fn selection_fixture() -> (crate::tests::Fixture, Output) {
        let mut config = Config::default();
        config.animations.off = true;
        config.hotkey_overlay.skip_at_startup = true;
        let mut fixture = crate::tests::Fixture::with_config(config);
        fixture.init_gles_renderer();
        fixture.add_output(1, (320, 240));
        let output = fixture.clarion_output(1);
        fixture
            .clarion_state()
            .do_action(Action::Screenshot(false, None), false);
        assert!(fixture.clarion().screenshot_ui.is_open());
        (fixture, output)
    }

    fn selected_rect(ui: &ScreenshotUi) -> Rectangle<i32, Physical> {
        let ScreenshotUi::Open { selection, .. } = ui else {
            panic!("selection UI should be open");
        };
        rect_from_corner_points(selection.1, selection.2)
    }

    fn assert_no_selection_overlay(ui: &ScreenshotUi, output: &Output) {
        assert!(!ui.has_selection());
        assert!(!ui.can_confirm());
        let ScreenshotUi::Open { output_data, .. } = ui else {
            panic!("selection UI should be open");
        };
        for data in output_data.values() {
            assert!(data.buffers.iter().all(|buffer| buffer.size().is_empty()));
        }
        let mut mask_elements = 0;
        ui.render_output(output, RenderTarget::Output, &mut |element| {
            if matches!(element, ScreenshotUiRenderElement::SolidColor(_)) {
                mask_elements += 1;
            }
        });
        assert_eq!(mask_elements, 0, "an empty crop has no frame or dim mask");
    }

    #[test]
    fn initial_selection_has_no_frame_and_confirmation_keys_remain_consumed() {
        let (mut fixture, output) = selection_fixture();
        let ui = &mut fixture.clarion().screenshot_ui;
        assert_no_selection_overlay(ui, &output);
        for key in [Keysym::space, Keysym::Return] {
            assert!(matches!(
                ui.action(key, ModifiersState::default()),
                Some(Action::ConfirmScreenshot { .. })
            ));
        }
        assert!(matches!(
            ui.action(
                Keysym::c,
                ModifiersState {
                    ctrl: true,
                    ..Default::default()
                }
            ),
            Some(Action::ConfirmScreenshot {
                write_to_disk: false
            })
        ));
        ui.move_left();
        ui.move_right();
        ui.move_up();
        ui.move_down();
        ui.set_width(SizeChange::SetFixed(100));
        ui.set_height(SizeChange::SetFixed(100));
        assert!(!ui.pointer_down(output.clone(), (10, 10).into(), None, true));
        assert_no_selection_overlay(ui, &output);
    }

    #[test]
    fn a_drag_reveals_the_frame_only_after_motion_and_release_retains_it() {
        let (mut fixture, output) = selection_fixture();
        let ui = &mut fixture.clarion().screenshot_ui;
        assert!(ui.pointer_down(output.clone(), (10, 20).into(), None, false));
        assert_no_selection_overlay(ui, &output);
        ui.pointer_motion((40, 60).into(), None);
        assert!(ui.has_selection());
        assert!(
            !ui.can_confirm(),
            "holding the pointer keeps Space in move mode"
        );
        assert_eq!(
            selected_rect(ui),
            Rectangle::new((10, 20).into(), (31, 41).into())
        );
        assert_eq!(ui.pointer_up(None), Some(false));
        assert!(ui.can_confirm());
        let ScreenshotUi::Open { output_data, .. } = ui else {
            unreachable!()
        };
        assert!(!output_data[&output].buffers[0].size().is_empty());
    }

    #[test]
    fn a_click_does_not_invent_a_default_crop() {
        let (mut fixture, output) = selection_fixture();
        let ui = &mut fixture.clarion().screenshot_ui;
        assert!(ui.pointer_down(output.clone(), (5, 5).into(), None, false));
        assert_eq!(ui.pointer_up(None), Some(false));
        assert_no_selection_overlay(ui, &output);
        // Moving outside the monitor at the same clamped anchor is still empty.
        assert!(ui.pointer_down(output.clone(), (-5, -5).into(), None, false));
        ui.pointer_motion((-50, -50).into(), None);
        assert_eq!(ui.pointer_up(None), Some(false));
        assert_no_selection_overlay(ui, &output);
    }

    #[test]
    fn reverse_drag_clamps_to_output_edges_without_losing_the_last_pixel() {
        let (mut fixture, output) = selection_fixture();
        let ui = &mut fixture.clarion().screenshot_ui;
        assert!(ui.pointer_down(output, (-10, 260).into(), None, false));
        ui.pointer_motion((999, -200).into(), None);
        assert_eq!(ui.pointer_up(None), Some(false));
        assert_eq!(selected_rect(ui), Rectangle::from_size((320, 240).into()));
        assert!(ui.can_confirm());
    }

    #[test]
    fn space_moves_an_existing_drag_without_confirming_or_resizing_it() {
        let (mut fixture, output) = selection_fixture();
        let ui = &mut fixture.clarion().screenshot_ui;
        assert!(ui.pointer_down(output, (10, 20).into(), None, false));
        ui.pointer_motion((40, 60).into(), None);
        let old_size = selected_rect(ui).size;
        ui.set_space_down(true);
        ui.pointer_motion((100, 120).into(), None);
        assert_eq!(selected_rect(ui).size, old_size);
        assert_eq!(selected_rect(ui).loc, (70, 80).into());
        assert!(!ui.can_confirm());
        assert!(matches!(
            ui.action(Keysym::space, ModifiersState::default()),
            Some(Action::ConfirmScreenshot { .. })
        ));
        ui.set_space_down(false);
        assert_eq!(ui.pointer_up(None), Some(false));
        assert!(ui.can_confirm());
    }

    #[test]
    fn reopening_does_not_reveal_the_previous_crop() {
        let (mut fixture, output) = selection_fixture();
        let ui = &mut fixture.clarion().screenshot_ui;
        assert!(ui.pointer_down(output.clone(), (10, 20).into(), None, false));
        ui.pointer_motion((40, 60).into(), None);
        assert_eq!(ui.pointer_up(None), Some(false));
        assert!(ui.close());
        fixture
            .clarion_state()
            .do_action(Action::Screenshot(false, None), false);
        assert_no_selection_overlay(&fixture.clarion().screenshot_ui, &output);
    }

    #[test]
    fn screenshot_fade_stops_redrawing_when_its_visible_motion_finishes() {
        for rate in [0.25, 0.73, 1., 2.] {
            for hz in [60., 120., 144., 240., 360.] {
                for spring in [false, true] {
                    let mut clock = Clock::with_time(Duration::ZERO);
                    clock.set_rate(rate);
                    let mut animation = if spring {
                        WindowMovementAnim::default().0
                    } else {
                        ScreenshotUiOpenAnim::default().0
                    };
                    if let Kind::Spring(params) = &mut animation.kind {
                        params.damping_ratio = 0.2;
                    }
                    let open_anim = Animation::new(clock.clone(), 0., 1., 0., animation);
                    let output = Output::new(
                        "screenshot-test".into(),
                        PhysicalProperties {
                            size: (0, 0).into(),
                            subpixel: Subpixel::Unknown,
                            make: String::new(),
                            model: String::new(),
                            serial_number: String::new(),
                        },
                    );
                    let ui = ScreenshotUi::Open {
                        selection: (output, (0, 0).into(), (10, 10).into()),
                        has_selection: true,
                        output_data: HashMap::new(),
                        button: Button::Up,
                        show_pointer: true,
                        open_anim,
                        clock: clock.clone(),
                        config: Rc::new(RefCell::new(Config::default())),
                        path: None,
                    };
                    let ScreenshotUi::Open { open_anim, .. } = &ui else {
                        unreachable!()
                    };
                    assert!(ui.are_animations_ongoing());
                    let mut completed = false;
                    for frame in 1..=(hz * 10.) as usize {
                        clock.set_unadjusted(Duration::from_secs_f64(frame as f64 / hz));
                        if open_anim.clamped_value() < 1. {
                            assert!(ui.are_animations_ongoing());
                            continue;
                        }
                        assert_eq!(open_anim.clamped_value(), 1.);
                        assert!(!ui.are_animations_ongoing());
                        if spring {
                            assert!(!open_anim.is_done(), "exercise the invisible spring tail");
                        } else {
                            assert!(open_anim.is_done(), "retain the configured easing duration");
                        }
                        completed = true;
                        break;
                    }
                    assert!(completed, "rate={rate}, hz={hz}, spring={spring}");
                    clock.set_unadjusted(Duration::from_secs(10));
                    assert_eq!(open_anim.clamped_value(), 1.);
                    assert!(!ui.are_animations_ongoing());
                }
            }
        }
    }
}
