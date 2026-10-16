use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use clarion_config::Config;
use ordered_float::NotNan;
use pangocairo::cairo::{self, ImageSurface};
use pangocairo::pango::FontDescription;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::{ContextId, ImportMem, Texture};
use smithay::output::Output;
use smithay::reexports::gbm::Format as Fourcc;
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Point, Transform};

use crate::animation::{Animation, Clock};
use crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement;
use crate::render_helpers::renderer::ClarionRenderer;
use crate::render_helpers::texture::{TextureBuffer, TextureRenderElement};
use crate::utils::{output_size, to_physical_precise_round};

const PADDING: i32 = 8;
use crate::ui::theme::{FONT, ON_SURFACE, SURFACE};
const BORDER: i32 = 4;

pub struct ConfigErrorNotification {
    state: State,
    buffers: UserDataMap,

    // If set, this is a "Created config at {path}" notification. If unset, this is a config error
    // notification.
    created_path: Option<PathBuf>,

    clock: Clock,
    config: Rc<RefCell<Config>>,
}

enum State {
    Hidden,
    Showing(Animation),
    Shown(Duration),
    Hiding(Animation),
}

impl ConfigErrorNotification {
    pub fn new(clock: Clock, config: Rc<RefCell<Config>>) -> Self {
        Self {
            state: State::Hidden,
            buffers: UserDataMap::new(),
            created_path: None,
            clock,
            config,
        }
    }

    fn animation(&self, to: f64) -> Animation {
        let c = self.config.borrow();
        let velocity = match &self.state {
            State::Showing(anim) | State::Hiding(anim) => anim.velocity(),
            State::Hidden | State::Shown(_) => 0.,
        };
        Animation::new(
            self.clock.clone(),
            self.value(),
            to,
            velocity,
            c.animations.config_notification_open_close.0,
        )
    }

    pub fn show_created(&mut self, created_path: &Path) {
        if self.created_path.as_deref() != Some(created_path) {
            self.created_path = Some(created_path.to_owned());
            self.buffers = UserDataMap::new();
        }

        self.state = State::Showing(self.animation(1.));
    }

    pub fn show(&mut self) {
        let c = self.config.borrow();
        if c.config_notification.disable_failed {
            return;
        }

        if self.created_path.is_some() {
            self.created_path = None;
            self.buffers = UserDataMap::new();
        }

        // Updating a message must not make the existing feedback disappear.
        self.state = State::Showing(self.animation(1.));
    }

    fn value(&self) -> f64 {
        match &self.state {
            State::Hidden => 0.,
            State::Showing(anim) | State::Hiding(anim) => anim.value(),
            State::Shown(_) => 1.,
        }
    }

    pub fn hide(&mut self) {
        if matches!(self.state, State::Hidden | State::Hiding(_)) {
            return;
        }

        self.state = State::Hiding(self.animation(0.));
    }

    pub fn advance_animations(&mut self) {
        match &mut self.state {
            State::Hidden => (),
            State::Showing(anim) => {
                if anim.is_done() {
                    let duration = if self.created_path.is_some() {
                        // Make this quite a bit longer because it comes with a monitor modeset
                        // (can take a while) and an important hotkeys popup diverting the
                        // attention.
                        Duration::from_secs(8)
                    } else {
                        Duration::from_secs(4)
                    };
                    self.state = State::Shown(self.clock.now_unadjusted() + duration);
                }
            }
            State::Shown(deadline) => {
                if self.clock.now_unadjusted() >= *deadline {
                    self.hide();
                }
            }
            State::Hiding(anim) => {
                if anim.is_clamped_done() {
                    self.state = State::Hidden;
                }
            }
        }
    }

    pub fn are_animations_ongoing(&self) -> bool {
        !matches!(self.state, State::Hidden)
    }

    pub(crate) fn on_native_renderer_replaced(&self) {
        if let Some(cache) = self
            .buffers
            .get::<NotificationTextures<crate::render_helpers::vulkan::VulkanTexture>>()
        {
            cache.0.borrow_mut().clear();
        }
    }

    pub fn render<R: ClarionRenderer>(
        &self,
        renderer: &mut R,
        output: &Output,
    ) -> Option<PrimaryGpuTextureRenderElement> {
        if let Some(gles) = renderer.as_gles_renderer() {
            self.render_with(gles, output)
                .map(PrimaryGpuTextureRenderElement::Gles)
        } else if let Some(native) = renderer.as_native_renderer() {
            self.render_with(native, output)
                .map(PrimaryGpuTextureRenderElement::Vulkan)
        } else {
            None
        }
    }

    /// Render using the selected backend's own texture type and context.
    pub fn render_with<R>(
        &self,
        renderer: &mut R,
        output: &Output,
    ) -> Option<TextureRenderElement<R::TextureId>>
    where
        R: ImportMem,
        R::TextureId: Clone + 'static,
        R::Error: std::error::Error + Send + Sync + 'static,
    {
        if matches!(self.state, State::Hidden) {
            return None;
        }

        let scale = output.current_scale().fractional_scale();
        let output_size = output_size(output);
        let path = self.created_path.as_deref();

        let cache = self
            .buffers
            .get_or_insert(|| NotificationTextures::<R::TextureId>(RefCell::new(HashMap::new())));
        let mut buffers = cache.0.borrow_mut();
        let buffer = buffers
            .entry((renderer.context_id(), NotNan::new(scale).ok()?))
            .or_insert_with(move || render(renderer, scale, path).ok());
        let buffer = buffer.clone()?;

        let size = buffer.logical_size();
        let y_range = size.h + f64::from(PADDING) * 2.;

        let x = (output_size.w - size.w).max(0.) / 2.;
        let y = match &self.state {
            State::Hidden => unreachable!(),
            State::Showing(anim) | State::Hiding(anim) => -size.h + anim.value() * y_range,
            State::Shown(_) => f64::from(PADDING) * 2.,
        };

        let location = Point::from((x, y));
        let location = location.to_physical_precise_round(scale).to_logical(scale);

        let elem = TextureRenderElement::from_texture_buffer(
            buffer,
            location,
            1.,
            None,
            None,
            Kind::Unspecified,
        );
        Some(elem)
    }
}

type NotificationTextureMap<T> = HashMap<(ContextId<T>, NotNan<f64>), Option<TextureBuffer<T>>>;
struct NotificationTextures<T: Texture>(RefCell<NotificationTextureMap<T>>);

fn render<R>(
    renderer: &mut R,
    scale: f64,
    created_path: Option<&Path>,
) -> anyhow::Result<TextureBuffer<R::TextureId>>
where
    R: ImportMem,
    R::Error: std::error::Error + Send + Sync + 'static,
{
    let _span = clarion_tracy_span!("config_error_notification::render");

    let padding: i32 = to_physical_precise_round(scale, PADDING);

    let mut text = error_text(true);
    let mut border_color = (1., 0.3, 0.3);
    if let Some(path) = created_path {
        text = format!(
            "Created a default config file at \
             <span face='monospace' bgcolor='#000000'>{path:?}</span>",
        );
        border_color = (0.5, 1., 0.5);
    };

    let mut font = FontDescription::from_string(FONT);
    font.set_absolute_size(to_physical_precise_round(scale, font.size()));

    let surface = ImageSurface::create(cairo::Format::ARgb32, 0, 0)?;
    let cr = cairo::Context::new(&surface)?;
    let layout = pangocairo::functions::create_layout(&cr);
    layout.context().set_round_glyph_positions(false);
    layout.set_font_description(Some(&font));
    layout.set_markup(&text);

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
    layout.set_markup(&text);

    cr.set_source_rgb(ON_SURFACE.0, ON_SURFACE.1, ON_SURFACE.2);
    pangocairo::functions::show_layout(&cr, &layout);

    cr.move_to(0., 0.);
    cr.line_to(width.into(), 0.);
    cr.line_to(width.into(), height.into());
    cr.line_to(0., height.into());
    cr.line_to(0., 0.);
    cr.set_source_rgb(border_color.0, border_color.1, border_color.2);
    // Keep the border width even to avoid blurry edges.
    cr.set_line_width((f64::from(BORDER) / 2. * scale).round() * 2.);
    cr.stroke()?;
    drop(cr);

    let data = surface.take_data()?;
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

pub fn error_text(markup: bool) -> String {
    let command = if markup {
        "<span face='monospace' bgcolor='#000000'>clarion validate</span>"
    } else {
        "clarion validate"
    };

    format!("Failed to parse the config file. Please run {command} to see the errors.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_feedback_and_reversal_preserve_visible_motion() {
        let mut clock = Clock::with_time(Duration::ZERO);
        let mut notification =
            ConfigErrorNotification::new(clock.clone(), Rc::new(RefCell::new(Config::default())));
        notification.show();
        clock.set_unadjusted(Duration::from_millis(40));
        let before = notification.value();
        let State::Showing(before_anim) = &notification.state else {
            panic!()
        };
        let velocity = before_anim.velocity();
        assert!(before > 0. && velocity > 0.);
        notification.show();
        let State::Showing(after_anim) = &notification.state else {
            panic!()
        };
        assert!((after_anim.value() - before).abs() < 1e-10);
        assert!((after_anim.velocity() - velocity).abs() < 1e-10);
        notification.hide();
        let State::Hiding(after_anim) = &notification.state else {
            panic!()
        };
        assert!((after_anim.value() - before).abs() < 1e-10);
        assert!((after_anim.velocity() - velocity).abs() < 1e-10);
        let start = after_anim.start_time();
        clock.set_unadjusted(Duration::from_millis(80));
        notification.hide();
        let State::Hiding(after_anim) = &notification.state else {
            panic!()
        };
        assert_eq!(after_anim.start_time(), start);
        clock.set_unadjusted(Duration::from_secs(2));
        notification.advance_animations();
        assert!(matches!(notification.state, State::Hidden));
    }

    #[test]
    fn changed_notification_content_does_not_restart_from_invisible() {
        let mut clock = Clock::with_time(Duration::ZERO);
        let mut notification =
            ConfigErrorNotification::new(clock.clone(), Rc::new(RefCell::new(Config::default())));
        notification.show();
        clock.set_unadjusted(Duration::from_millis(60));
        let before = notification.value();
        notification.show_created(Path::new("/test/config.kdl"));
        assert!((notification.value() - before).abs() < 1e-10);
        clock.set_unadjusted(Duration::from_secs(1));
        notification.advance_animations();
        assert_eq!(notification.value(), 1.);
        notification.show();
        assert_eq!(notification.value(), 1.);
        assert!(notification.created_path.is_none());
    }
}
