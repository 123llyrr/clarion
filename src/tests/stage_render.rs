use std::time::Duration;

use realand_config::{BlockOutFrom, Config, LayerRule, Workspace, WorkspaceName};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::Element;
use smithay::output::Scale as OutputScale;
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::Layer, zwlr_layer_surface_v1::Anchor,
};
use smithay::utils::{Logical, Physical, Point, Rectangle, Scale};
use wayland_client::protocol::{wl_region::WlRegion, wl_subsurface::WlSubsurface};

use super::client::{ClientId, LayerConfigureProps};
use super::Fixture;
use crate::render_helpers::{render_to_vec, RenderCtx, RenderTarget};

wayland_client::delegate_noop!(super::client::State: ignore WlRegion);

/// The native shell retains a 40-pixel exclusive zone when its transparent
/// parent grows for a tooltip or panel. Only the child bar accepts input.
pub(super) struct StageTestBar {
    client: ClientId,
    surface: wayland_client::protocol::wl_surface::WlSurface,
    child: wayland_client::protocol::wl_surface::WlSurface,
    subsurface: WlSubsurface,
    viewport:
        smithay::reexports::wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport,
    acknowledged: Option<u32>,
}

impl StageTestBar {
    pub(super) fn new(f: &mut Fixture) -> Self {
        let client = f.add_client();
        let c = f.client(client);
        let surface = c
            .create_layer(None, Layer::Top, "stage-test-bar")
            .surface
            .clone();
        let child = c
            .state
            .compositor
            .as_ref()
            .unwrap()
            .create_surface(&c.qh, ());
        let subsurface =
            c.state
                .subcompositor
                .as_ref()
                .unwrap()
                .get_subsurface(&child, &surface, &c.qh, ());
        let viewport = c
            .state
            .viewporter
            .as_ref()
            .unwrap()
            .get_viewport(&child, &c.qh, ());
        let empty = c
            .state
            .compositor
            .as_ref()
            .unwrap()
            .create_region(&c.qh, ());
        surface.set_input_region(Some(&empty));
        empty.destroy();
        let buffer = c.state.spbm.as_ref().unwrap().create_u32_rgba_buffer(
            u32::MAX,
            0,
            0,
            u32::MAX,
            &c.qh,
            (),
        );
        child.attach(Some(&buffer), 0, 0);
        Self {
            client,
            surface,
            child,
            subsurface,
            viewport,
            acknowledged: None,
        }
    }

    pub(super) fn configure(&mut self, f: &mut Fixture, bottom: bool, height: u32, layer: Layer) {
        let panel = f.client(self.client).layer(&self.surface);
        panel.set_configure_props(LayerConfigureProps {
            size: Some((0, height)),
            anchor: Some(
                Anchor::Left | Anchor::Right | if bottom { Anchor::Bottom } else { Anchor::Top },
            ),
            exclusive_zone: Some(40),
            layer: Some(layer),
            ..Default::default()
        });
        panel.commit();
        f.double_roundtrip(self.client);
        let panel = f.client(self.client).layer(&self.surface);
        let (serial, configured) = *panel.configures_received.last().unwrap();
        let (width, height) = configured.size;
        self.viewport.set_destination(width as i32, 40);
        self.subsurface
            .set_position(0, if bottom { height as i32 - 40 } else { 0 });
        self.child.commit();
        panel.attach_new_buffer();
        panel.set_size(width as u16, height as u16);
        if self.acknowledged != Some(serial) {
            panel.ack_last();
            self.acknowledged = Some(serial);
        }
        panel.commit();
        f.double_roundtrip(self.client);
    }
}

fn stage_rail_backgrounds(f: &mut Fixture, scale: f64) -> Vec<Rectangle<i32, Physical>> {
    let output = f.realand_output(1);
    let state = f.realand_state();
    let realand = &mut state.realand;
    realand.update_render_elements(None);
    let monitor = realand.layout.monitor_for_output(&output).unwrap();
    let bottom = monitor.working_area().loc.y + monitor.working_area().size.h;
    macro_rules! geometry {
        ($renderer:expr) => {{
            let mut rail = Vec::new();
            monitor.render_stage_with_layers(
                RenderCtx {
                    renderer: $renderer,
                    target: RenderTarget::Output,
                    xray: None,
                },
                false,
                &mut |_, _, _, _, _| {},
                &mut |element| {
                    let geo = element.geometry(scale.into());
                    // Rail backgrounds are 52 pixels high; text and workspace
                    // cards have different heights in this 1280x800 fixture.
                    if (f64::from(geo.size.h) - 52. * scale).abs() <= 1.
                        && f64::from(geo.loc.y) > (bottom - 100.) * scale
                    {
                        rail.push(geo);
                    }
                },
            );
            rail
        }};
    }
    state
        .backend
        .with_renderer(|renderer| match renderer {
            crate::render_helpers::renderer::AnyRenderer::Gles(renderer) => geometry!(renderer),
            crate::render_helpers::renderer::AnyRenderer::Native(renderer) => geometry!(renderer),
        })
        .unwrap()
}

pub(super) fn pixels(f: &mut Fixture, target: RenderTarget) -> Vec<u8> {
    let output = f.realand_output(1);
    let state = f.realand_state();
    let realand = &mut state.realand;
    realand.update_render_elements(None);
    macro_rules! draw {
        ($renderer:expr) => {{
            let renderer = $renderer;
            let elements = realand.render_to_vec(
                RenderCtx {
                    renderer,
                    target,
                    xray: None,
                },
                &output,
                false,
            );
            render_to_vec(
                renderer,
                output.current_mode().unwrap().size,
                Scale::from(output.current_scale().fractional_scale()),
                output.current_transform(),
                Fourcc::Abgr8888,
                elements.iter().rev(),
            )
            .unwrap()
        }};
    }
    state
        .backend
        .with_renderer(|renderer| match renderer {
            crate::render_helpers::renderer::AnyRenderer::Gles(renderer) => draw!(renderer),
            crate::render_helpers::renderer::AnyRenderer::Native(renderer) => draw!(renderer),
        })
        .unwrap()
}

fn test_stage_layers(native: bool) {
    for scale in [1., 1.25, 1.5] {
        let mut config = Config::default();
        config.hotkey_overlay.skip_at_startup = true;
        config.animations.overview_open_close.0.kind =
            realand_config::animations::Kind::Easing(realand_config::animations::EasingParams {
                duration_ms: 1000,
                curve: realand_config::animations::Curve::Linear,
            });
        config.workspaces = ["first", "second", "third"]
            .into_iter()
            .map(|name| Workspace {
                name: WorkspaceName(name.into()),
                open_on_output: None,
                layout: None,
            })
            .collect();
        config.layer_rules.push(LayerRule {
            matches: vec![realand_config::layer_rule::Match {
                layer: Some(realand_ipc::Layer::Bottom),
                ..Default::default()
            }],
            block_out_from: Some(BlockOutFrom::ScreenCapture),
            ..Default::default()
        });
        let mut f = Fixture::with_config(config);
        if native {
            let mut headless = crate::backend::Headless::new(
                f.realand().config.clone(),
                crate::backend::RendererBackendType::Vulkan,
            );
            headless.add_renderer().unwrap();
            headless.init(&mut crate::platform::device_bridge::DeviceCoordinator::new(
                f.realand(),
            ));
            f.realand_state().backend = crate::backend::Backend::Headless(headless);
        } else {
            f.init_gles_renderer();
        }
        if native {
            f.realand_state()
                .backend
                .with_renderer(|renderer| match renderer {
                    crate::render_helpers::renderer::AnyRenderer::Native(renderer) => eprintln!(
                        "HARDWARE: compositor Stage native {}",
                        renderer.vulkan().info.device_name
                    ),
                    crate::render_helpers::renderer::AnyRenderer::Gles(_) => {
                        panic!("Stage native regression must not fall back to GLES")
                    }
                })
                .unwrap();
        }
        f.add_output(1, (1280, 800));
        let output = f.realand_output(1);
        output.change_current_state(None, None, Some(OutputScale::Fractional(scale)), None);
        f.realand().output_resized(&output);
        let client = f.add_client();
        for (layer, namespace, size, color) in [
            (
                Layer::Background,
                "stage-wallpaper",
                (0, 0),
                [0, u32::MAX / 5 * 4, u32::MAX / 5, u32::MAX],
            ),
            (
                Layer::Bottom,
                "stage-bottom",
                (120, 80),
                [u32::MAX / 10 * 9, u32::MAX / 10, u32::MAX / 10, u32::MAX],
            ),
        ] {
            let panel = f.client(client).create_layer(None, layer, namespace);
            let surface = panel.surface.clone();
            let anchor = if layer == Layer::Background {
                Anchor::Top | Anchor::Right | Anchor::Bottom | Anchor::Left
            } else {
                Anchor::Top | Anchor::Left
            };
            panel.set_configure_props(LayerConfigureProps {
                size: Some(size),
                anchor: Some(anchor),
                ..Default::default()
            });
            panel.commit();
            f.double_roundtrip(client);
            let panel = f.client(client).layer(&surface);
            let buffer = panel.spbm.create_u32_rgba_buffer(
                color[0],
                color[1],
                color[2],
                color[3],
                &panel.qh,
                (),
            );
            panel.surface.attach(Some(&buffer), 0, 0);
            let configured = panel.configures_received.last().unwrap().1.size;
            panel.set_size(configured.0 as u16, configured.1 as u16);
            panel.ack_last_and_commit();
            f.double_roundtrip(client);
        }
        f.realand_complete_animations();
        let desktop = pixels(&mut f, RenderTarget::Output);
        f.realand().layout.open_overview();
        assert_eq!(pixels(&mut f, RenderTarget::Output), desktop, "Stage opening must retain the actual wallpaper and bottom layer at scale={scale}, native={native}");
        f.realand_complete_animations();
        let monitor = f.realand().layout.monitor_for_output(&output).unwrap();
        let (workspace, card) = monitor.workspaces_with_render_geo().next().unwrap();
        assert_eq!(workspace.id(), monitor.active_workspace_ref().id());
        let zoom = card.size.w / monitor.view_size().w;
        let to_pixel = |local: Point<f64, Logical>| {
            (card.loc + local.upscale(zoom)).to_physical_precise_round::<f64, i32>(scale)
        };
        let bottom = to_pixel((60., 40.).into());
        let wallpaper =
            to_pixel((monitor.view_size().w * 0.75, monitor.view_size().h * 0.5).into());
        let output_pixels = pixels(&mut f, RenderTarget::Output);
        let at = |image: &[u8], point: Point<i32, smithay::utils::Physical>| {
            let offset = (point.y as usize * 1280 + point.x as usize) * 4;
            image[offset..offset + 4].to_vec()
        };
        let bottom_color = at(&output_pixels, bottom);
        let desktop_bottom = at(
            &desktop,
            Point::from((60., 40.)).to_physical_precise_round::<f64, i32>(scale),
        );
        assert_eq!(
            bottom_color, desktop_bottom,
            "card layer color matches its normal desktop source"
        );
        assert!(
            bottom_color[0] > 200 && bottom_color[1] < 50 && bottom_color[2] < 50,
            "Bottom Layer-shell must be inside its workspace card: {bottom_color:?}"
        );
        let wallpaper_color = at(&output_pixels, wallpaper);
        assert!(
            wallpaper_color[0] < 10 && wallpaper_color[1] > 180 && wallpaper_color[2] < 80,
            "The live wallpaper must remain inside each card: {wallpaper_color:?}"
        );
        let capture = pixels(&mut f, RenderTarget::ScreenCapture);
        assert!(
            at(&capture, bottom)[..3].iter().all(|c| *c < 10),
            "Layer privacy must use the ScreenCapture target for each card"
        );
        assert_eq!(at(&capture, wallpaper), wallpaper_color);
        if let Some(directory) = std::env::var_os("REALAND_STAGE_PNG_DIR") {
            let file = std::fs::File::create(std::path::Path::new(&directory).join(format!(
                "realand-stage-layers-{scale}-{}.png",
                if native { "vulkan" } else { "gles" }
            )))
            .unwrap();
            let mut encoder = png::Encoder::new(file, 1280, 800);
            encoder.set_color(png::ColorType::Rgba);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&output_pixels)
                .unwrap();
        }
        f.realand().layout.close_overview();
        assert_eq!(
            pixels(&mut f, RenderTarget::Output),
            output_pixels,
            "Stage closing must retain its composed first frame"
        );
        let now = f.realand().clock.now_unadjusted();
        f.realand()
            .clock
            .set_unadjusted(now + Duration::from_millis(250));
        f.realand().advance_animations();
        let closing = pixels(&mut f, RenderTarget::Output);
        f.realand().layout.open_overview();
        assert_eq!(
            pixels(&mut f, RenderTarget::Output),
            closing,
            "Stage reversal retains wallpaper and Bottom surfaces"
        );
        f.realand().layout.close_overview();
        f.realand_complete_animations();
        assert_eq!(
            pixels(&mut f, RenderTarget::Output),
            desktop,
            "The final desktop must preserve its wallpaper and Bottom surfaces"
        );
    }
}

#[test]
fn stage_wallpaper_and_bottom_layers_are_live_private_and_continuous_on_gles() {
    test_stage_layers(false);
}

#[test]
#[ignore = "requires a Vulkan render node; never acquires display master"]
fn stage_wallpaper_and_bottom_layers_are_live_private_and_continuous_on_vulkan() {
    test_stage_layers(true);
}

fn test_stage_rail_with_native_bar(native: bool) {
    for scale in [1., 1.25, 1.5, 2.] {
        let mut config = Config::default();
        config.hotkey_overlay.skip_at_startup = true;
        config.workspaces = ["first", "second", "third"]
            .into_iter()
            .map(|name| Workspace {
                name: WorkspaceName(name.into()),
                open_on_output: None,
                layout: None,
            })
            .collect();
        let mut f = Fixture::with_config(config);
        if native {
            let mut headless = crate::backend::Headless::new(
                f.realand().config.clone(),
                crate::backend::RendererBackendType::Vulkan,
            );
            headless.add_renderer().unwrap();
            headless.init(&mut crate::platform::device_bridge::DeviceCoordinator::new(
                f.realand(),
            ));
            f.realand_state().backend = crate::backend::Backend::Headless(headless);
            f.realand_state()
                .backend
                .with_renderer(|renderer| match renderer {
                    crate::render_helpers::renderer::AnyRenderer::Native(renderer) => eprintln!(
                        "HARDWARE: Stage bar reservation native {} scale={scale}",
                        renderer.vulkan().info.device_name
                    ),
                    crate::render_helpers::renderer::AnyRenderer::Gles(_) => {
                        panic!("Stage bar reservation must not fall back to GLES")
                    }
                })
                .unwrap();
        } else {
            f.init_gles_renderer();
        }
        f.add_output(1, (1280, 800));
        let output = f.realand_output(1);
        output.change_current_state(None, None, Some(OutputScale::Fractional(scale)), None);
        f.realand().output_resized(&output);
        let mut bar = StageTestBar::new(&mut f);
        f.realand().layout.open_overview();
        f.realand_complete_animations();
        let logical_height = f
            .realand()
            .layout
            .monitor_for_output(&output)
            .unwrap()
            .view_size()
            .h;
        for (bottom, height, layer) in [
            (false, 40, Layer::Top),
            (true, 40, Layer::Top),
            (true, 128, Layer::Top),
            (true, logical_height as u32, Layer::Overlay),
            (false, logical_height as u32, Layer::Overlay),
            (false, 128, Layer::Top),
            (false, 40, Layer::Top),
            (true, 40, Layer::Top),
        ] {
            bar.configure(&mut f, bottom, height, layer);
            let monitor = f.realand().layout.monitor_for_output(&output).unwrap();
            let area = monitor.working_area();
            assert_eq!(area.loc.y, if bottom { 0. } else { 40. });
            assert_eq!(area.size.h, logical_height.round() - 40.);
            let bounds = area.to_physical_precise_round(scale);
            let rail = stage_rail_backgrounds(&mut f, scale);
            assert!(
                rail.len() >= 4,
                "expected complete Stage rail, got {rail:?}"
            );
            for geo in &rail {
                assert!(
                    geo.loc.y >= bounds.loc.y
                        && geo.loc.y + geo.size.h <= bounds.loc.y + bounds.size.h,
                    "Stage rail must fit the work area above the native bar: bottom={bottom}, height={height}, scale={scale}, rail={geo:?}, area={bounds:?}"
                );
            }
            let image = pixels(&mut f, RenderTarget::Output);
            let at = |point: Point<i32, Physical>| {
                let offset = (point.y as usize * 1280 + point.x as usize) * 4;
                &image[offset..offset + 4]
            };
            for geo in &rail {
                // The left inset avoids glyphs and probes the full background,
                // including the count's lower line that the bottom bar hid.
                for y in [geo.loc.y + 2, geo.loc.y + geo.size.h - 3] {
                    let pixel = at((geo.loc.x + 3, y).into());
                    assert!(
                        pixel[2] > pixel[0],
                        "Stage rail pixel was obscured: bottom={bottom}, height={height}, scale={scale}, pixel={pixel:?}"
                    );
                }
            }
            let bar_y = if bottom {
                800 - (20. * scale) as i32
            } else {
                (20. * scale) as i32
            };
            assert_eq!(at((640, bar_y).into()), [255, 0, 0, 255]);
            if let Some(directory) = std::env::var_os("REALAND_STAGE_PNG_DIR") {
                let file = std::fs::File::create(std::path::Path::new(&directory).join(format!(
                    "realand-stage-bar-{scale}-{}-{}-{height}.png",
                    if native { "vulkan" } else { "gles" },
                    if bottom { "bottom" } else { "top" }
                )))
                .unwrap();
                let mut encoder = png::Encoder::new(file, 1280, 800);
                encoder.set_color(png::ColorType::Rgba);
                encoder
                    .write_header()
                    .unwrap()
                    .write_image_data(&image)
                    .unwrap();
            }
        }
    }
}

#[test]
fn stage_rail_respects_top_and_bottom_bar_exclusive_zone_on_gles() {
    test_stage_rail_with_native_bar(false);
}

#[test]
#[ignore = "requires a Vulkan render node; never acquires display master"]
fn stage_rail_respects_top_and_bottom_bar_exclusive_zone_on_vulkan() {
    test_stage_rail_with_native_bar(true);
}
