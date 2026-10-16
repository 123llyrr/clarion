use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Mutex;

use clarion_config::Config;
use ordered_float::NotNan;
use pangocairo::cairo::{self, ImageSurface};
use pangocairo::pango::{Alignment, FontDescription};
use smithay::backend::renderer::element::utils::RescaleRenderElement;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::{ContextId, ImportMem, Texture};
use smithay::output::Output;
use smithay::reexports::gbm::Format as Fourcc;
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Point, Transform};

use crate::animation::{Animation, Clock};
use crate::clarion_render_elements;
use crate::render_helpers::memory::MemoryBuffer;
use crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement;
use crate::render_helpers::renderer::ClarionRenderer;
use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
use crate::render_helpers::texture::{TextureBuffer, TextureRenderElement};
use crate::utils::{output_size, to_physical_precise_round};

const KEY_NAME: &str = "Enter";
const PADDING: i32 = 16;
use crate::ui::theme::{ERROR, FONT, ON_SURFACE, SURFACE};
const BORDER: i32 = 8;
const BACKDROP_COLOR: [f32; 4] = [0., 0., 0., 0.4];

pub struct ExitConfirmDialog {
    state: State,
    buffers: RefCell<HashMap<NotNan<f64>, Option<MemoryBuffer>>>,

    textures: UserDataMap,
    clock: Clock,
    config: Rc<RefCell<Config>>,
}

clarion_render_elements! {
    ExitConfirmDialogRenderElement => {
        Texture = RescaleRenderElement<PrimaryGpuTextureRenderElement>,
        SolidColor = SolidColorRenderElement,
    }
}

smithay::backend::renderer::element::render_elements! {
    pub NativeExitDialogRenderElement<R>;
    Texture = RescaleRenderElement<TextureRenderElement<R::TextureId>>,
    SolidColor = SolidColorRenderElement,
}

type DialogTextureMap<T> = HashMap<(ContextId<T>, NotNan<f64>), TextureBuffer<T>>;
struct DialogTextures<T: Texture>(RefCell<DialogTextureMap<T>>);

struct DialogScene<T: Texture> {
    texture: TextureRenderElement<T>,
    origin: Point<i32, smithay::utils::Physical>,
    scale: f64,
    backdrop: SolidColorRenderElement,
}

struct OutputData {
    backdrop: SolidColorBuffer,
}

enum State {
    Hidden,
    Showing(Animation),
    Visible,
    Hiding(Animation),
}

impl ExitConfirmDialog {
    pub fn new(clock: Clock, config: Rc<RefCell<Config>>) -> Self {
        let buffer = match render(1.) {
            Ok(x) => Some(x),
            Err(err) => {
                warn!("error creating the exit confirm dialog: {err:?}");
                None
            }
        };

        Self {
            state: State::Hidden,
            buffers: RefCell::new(HashMap::from([(NotNan::new(1.).unwrap(), buffer)])),
            textures: UserDataMap::new(),
            clock,
            config,
        }
    }

    pub fn can_show(&self) -> bool {
        let buffers = self.buffers.borrow();
        let fallback = &buffers[&NotNan::new(1.).unwrap()];
        fallback.is_some()
    }

    fn animation(&self, from: f64, to: f64) -> Animation {
        let c = self.config.borrow();
        let velocity = match &self.state {
            State::Showing(anim) | State::Hiding(anim) => anim.velocity(),
            State::Hidden | State::Visible => 0.,
        };
        Animation::new(
            self.clock.clone(),
            from,
            to,
            velocity,
            c.animations.exit_confirmation_open_close.0,
        )
    }

    fn value(&self) -> f64 {
        match &self.state {
            State::Hidden => 0.,
            State::Showing(anim) | State::Hiding(anim) => anim.value(),
            State::Visible => 1.,
        }
    }

    /// Returns true if the dialog will be shown (even if it is already shown).
    pub fn show(&mut self) -> bool {
        if !self.can_show() {
            return false;
        }

        if matches!(self.state, State::Showing(_) | State::Visible) {
            return true;
        }

        self.state = State::Showing(self.animation(self.value(), 1.));
        true
    }

    /// Returns true if started the hide animation.
    pub fn hide(&mut self) -> bool {
        if matches!(self.state, State::Hidden | State::Hiding(_)) {
            return false;
        }

        self.state = State::Hiding(self.animation(self.value(), 0.));
        true
    }

    pub fn is_open(&self) -> bool {
        !matches!(self.state, State::Hidden)
    }

    pub fn advance_animations(&mut self) {
        match &mut self.state {
            State::Hidden => (),
            State::Showing(anim) => {
                if anim.is_done() {
                    self.state = State::Visible;
                }
            }
            State::Visible => (),
            State::Hiding(anim) => {
                if anim.is_clamped_done() {
                    self.state = State::Hidden;
                }
            }
        }
    }

    pub fn are_animations_ongoing(&self) -> bool {
        matches!(self.state, State::Showing(_) | State::Hiding(_))
    }

    pub(crate) fn on_native_renderer_replaced(&self) {
        if let Some(cache) = self
            .textures
            .get::<DialogTextures<crate::render_helpers::vulkan::VulkanTexture>>()
        {
            cache.0.borrow_mut().clear();
        }
    }

    pub fn render<R: ClarionRenderer>(
        &self,
        renderer: &mut R,
        output: &Output,
        push: &mut dyn FnMut(ExitConfirmDialogRenderElement),
    ) {
        let (elem, backdrop) = if let Some(gles) = renderer.as_gles_renderer() {
            let Some(scene) = self.prepare(gles, output) else {
                return;
            };
            (
                RescaleRenderElement::from_element(
                    PrimaryGpuTextureRenderElement::Gles(scene.texture),
                    scene.origin,
                    scene.scale,
                ),
                scene.backdrop,
            )
        } else if let Some(native) = renderer.as_native_renderer() {
            let Some(scene) = self.prepare(native, output) else {
                return;
            };
            (
                RescaleRenderElement::from_element(
                    PrimaryGpuTextureRenderElement::Vulkan(scene.texture),
                    scene.origin,
                    scene.scale,
                ),
                scene.backdrop,
            )
        } else {
            return;
        };
        push(ExitConfirmDialogRenderElement::Texture(elem));
        push(ExitConfirmDialogRenderElement::SolidColor(backdrop));
    }

    pub fn render_with<R>(
        &self,
        renderer: &mut R,
        output: &Output,
        push: &mut dyn FnMut(NativeExitDialogRenderElement<R>),
    ) where
        R: ImportMem,
        R::TextureId: Clone + 'static,
    {
        let Some(scene) = self.prepare(renderer, output) else {
            return;
        };
        push(NativeExitDialogRenderElement::Texture(
            RescaleRenderElement::from_element(scene.texture, scene.origin, scene.scale),
        ));
        push(NativeExitDialogRenderElement::SolidColor(scene.backdrop));
    }

    fn prepare<R>(&self, renderer: &mut R, output: &Output) -> Option<DialogScene<R::TextureId>>
    where
        R: ImportMem,
        R::TextureId: Clone + 'static,
    {
        let (value, clamped_value) = match &self.state {
            State::Hidden => return None,
            State::Showing(anim) | State::Hiding(anim) => (anim.value(), anim.clamped_value()),
            State::Visible => (1., 1.),
        };
        let _span = clarion_tracy_span!("ExitConfirmDialog::render");

        // Can be out of range when starting from past 0. or 1. from a spring bounce.
        let clamped_value = clamped_value.clamp(0., 1.);

        let scale = output.current_scale().fractional_scale();
        let output_size = output_size(output);

        let mut buffers = self.buffers.borrow_mut();
        let Some(fallback) = buffers[&NotNan::new(1.).unwrap()].clone() else {
            error!("exit confirm dialog opened without fallback buffer");
            return None;
        };

        let buffer = buffers
            .entry(NotNan::new(scale).unwrap())
            .or_insert_with(|| render(scale).ok());
        let buffer = buffer.as_ref().unwrap_or(&fallback);

        let size = buffer.logical_size();
        let textures = self
            .textures
            .get_or_insert(|| DialogTextures::<R::TextureId>(RefCell::new(HashMap::new())));
        let mut textures = textures.0.borrow_mut();
        let key = (renderer.context_id(), NotNan::new(buffer.scale().x).ok()?);
        let buffer = match textures.entry(key) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.get().clone(),
            std::collections::hash_map::Entry::Vacant(entry) => entry
                .insert(TextureBuffer::from_memory_buffer(renderer, buffer).ok()?)
                .clone(),
        };

        let location = (output_size.to_point() - size.to_point()).downscale(2.);
        let mut location = location.to_physical_precise_round(scale).to_logical(scale);
        location.x = f64::max(0., location.x);
        location.y = f64::max(0., location.y);

        let elem = TextureRenderElement::from_texture_buffer(
            buffer,
            location,
            clamped_value as f32,
            None,
            None,
            Kind::Unspecified,
        );
        let texture = elem;
        let origin = (location + size.downscale(2.)).to_physical_precise_round(scale);
        let animation_scale = value.max(0.) * 0.2 + 0.8;

        // Backdrop.
        let data = output.user_data().get_or_insert(|| {
            Mutex::new(OutputData {
                backdrop: SolidColorBuffer::new(output_size, BACKDROP_COLOR),
            })
        });
        let mut data = data.lock().unwrap();
        data.backdrop.resize(output_size);

        let elem = SolidColorRenderElement::from_buffer(
            &data.backdrop,
            Point::new(0., 0.),
            clamped_value as f32,
            Kind::Unspecified,
        );
        Some(DialogScene {
            texture,
            origin,
            scale: animation_scale,
            backdrop: elem,
        })
    }
}

fn render(scale: f64) -> anyhow::Result<MemoryBuffer> {
    let _span = clarion_tracy_span!("exit_confirm_dialog::render");

    let markup = text(true);

    let padding: i32 = to_physical_precise_round(scale, PADDING);

    let mut font = FontDescription::from_string(FONT);
    font.set_absolute_size(to_physical_precise_round(scale, font.size()));

    let surface = ImageSurface::create(cairo::Format::ARgb32, 0, 0)?;
    let cr = cairo::Context::new(&surface)?;
    let layout = pangocairo::functions::create_layout(&cr);
    layout.context().set_round_glyph_positions(false);
    layout.set_font_description(Some(&font));
    layout.set_alignment(Alignment::Center);
    layout.set_markup(&markup);

    let (mut width, mut height) = layout.pixel_size();
    width += padding * 2;
    height += padding * 2;

    let surface = ImageSurface::create(cairo::Format::ARgb32, width, height)?;
    let cr = cairo::Context::new(&surface)?;
    cr.set_source_rgb(SURFACE.0, SURFACE.1, SURFACE.2);
    cr.paint()?;

    cr.move_to(padding.into(), padding.into());
    let layout = pangocairo::functions::create_layout(&cr);
    layout.context().set_round_glyph_positions(false);
    layout.set_font_description(Some(&font));
    layout.set_alignment(Alignment::Center);
    layout.set_markup(&markup);

    cr.set_source_rgb(ON_SURFACE.0, ON_SURFACE.1, ON_SURFACE.2);
    pangocairo::functions::show_layout(&cr, &layout);

    cr.move_to(0., 0.);
    cr.line_to(width.into(), 0.);
    cr.line_to(width.into(), height.into());
    cr.line_to(0., height.into());
    cr.line_to(0., 0.);
    cr.set_source_rgb(ERROR.0, ERROR.1, ERROR.2);
    // Keep the border width even to avoid blurry edges.
    cr.set_line_width((f64::from(BORDER) / 2. * scale).round() * 2.);
    cr.stroke()?;
    drop(cr);

    let data = surface.take_data().unwrap();
    let buffer = MemoryBuffer::new(
        data.to_vec(),
        Fourcc::Argb8888,
        (width, height),
        scale,
        Transform::Normal,
    );

    Ok(buffer)
}

fn text(markup: bool) -> String {
    let key = if markup {
        format!("<span face='mono' bgcolor='#2C2C2C'> {KEY_NAME} </span>")
    } else {
        String::from(KEY_NAME)
    };

    format!(
        "Are you sure you want to exit clarion?\n\n\
         Press {key} to confirm."
    )
}

#[cfg(feature = "dbus")]
pub fn a11y_node() -> accesskit::Node {
    let mut node = accesskit::Node::new(accesskit::Role::AlertDialog);
    node.set_label("Exit clarion");
    node.set_description(text(false));
    node.set_modal();
    node
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn dialog_reversal_preserves_motion_and_repeated_requests_are_idempotent() {
        let mut clock = Clock::with_time(Duration::ZERO);
        let mut dialog =
            ExitConfirmDialog::new(clock.clone(), Rc::new(RefCell::new(Config::default())));
        assert!(dialog.show());
        clock.set_unadjusted(Duration::from_millis(50));
        let before = dialog.value();
        let State::Showing(anim) = &dialog.state else {
            panic!()
        };
        let velocity = anim.velocity();
        assert!(before > 0. && velocity > 0.);
        assert!(dialog.hide());
        let State::Hiding(anim) = &dialog.state else {
            panic!()
        };
        assert!((anim.value() - before).abs() < 1e-10);
        assert!((anim.velocity() - velocity).abs() < 1e-10);
        assert!(!dialog.hide());
        clock.set_unadjusted(Duration::from_millis(80));
        let before = dialog.value();
        let State::Hiding(anim) = &dialog.state else {
            panic!()
        };
        let velocity = anim.velocity();
        assert!(dialog.show());
        let State::Showing(anim) = &dialog.state else {
            panic!()
        };
        assert!((anim.value() - before).abs() < 1e-10);
        assert!((anim.velocity() - velocity).abs() < 1e-10);
        let start = anim.start_time();
        clock.set_unadjusted(Duration::from_millis(90));
        assert!(dialog.show());
        let State::Showing(anim) = &dialog.state else {
            panic!()
        };
        assert_eq!(anim.start_time(), start);
    }
}
