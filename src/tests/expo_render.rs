use std::time::Duration;

use realand_config::{BlockOutFrom, Config, LayerRule, Workspace, WorkspaceName};
use smithay::output::Scale as OutputScale;
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::Layer, zwlr_layer_surface_v1::Anchor,
};
use smithay::utils::{Logical, Point};

use super::client::LayerConfigureProps;
use super::stage_render::pixels;
use super::Fixture;
use crate::render_helpers::RenderTarget;

fn test_expo_layers(native: bool) {
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
            f.realand_state()
                .backend
                .with_renderer(|renderer| match renderer {
                    crate::render_helpers::renderer::AnyRenderer::Native(renderer) => eprintln!(
                        "HARDWARE: compositor Expo native {}",
                        renderer.vulkan().info.device_name
                    ),
                    crate::render_helpers::renderer::AnyRenderer::Gles(_) => {
                        panic!("Expo native regression must not fall back to GLES")
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
        let client = f.add_client();
        for (layer, namespace, size, color) in [
            (
                Layer::Background,
                "expo-wallpaper",
                (0, 0),
                [0, u32::MAX / 5 * 4, u32::MAX / 5, u32::MAX],
            ),
            (
                Layer::Bottom,
                "expo-bottom",
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
        f.realand().layout.toggle_expo();
        assert_eq!(pixels(&mut f, RenderTarget::Output), desktop);
        f.realand_complete_animations();

        let monitor = f.realand().layout.monitor_for_output(&output).unwrap();
        let card_points: Vec<_> = monitor
            .workspaces_with_render_geo()
            .map(|(_, card)| {
                let zoom = card.size.w / monitor.view_size().w;
                let to_pixel = |local: Point<f64, Logical>| {
                    (card.loc + local.upscale(zoom)).to_physical_precise_round::<f64, i32>(scale)
                };
                // Probe both sides of the Bottom layer edge; an incorrect global
                // overview zoom leaves the interior correct but moves this edge.
                (to_pixel((60., 40.).into()), to_pixel((126., 40.).into()))
            })
            .collect();
        let output_pixels = pixels(&mut f, RenderTarget::Output);
        let capture = pixels(&mut f, RenderTarget::ScreenCapture);
        let at = |image: &[u8], point: Point<i32, smithay::utils::Physical>| {
            let offset = (point.y as usize * 1280 + point.x as usize) * 4;
            image[offset..offset + 4].to_vec()
        };
        for (bottom, outside_bottom) in card_points {
            let inside = at(&output_pixels, bottom);
            assert!(inside[0] > 200 && inside[1] < 50 && inside[2] < 50);
            let outside = at(&output_pixels, outside_bottom);
            assert!(outside[0] < 10 && outside[1] > 180 && outside[2] < 80,
                "Expo layers must share their card zoom: scale={scale}, native={native}, pixel={outside:?}");
            assert!(at(&capture, bottom)[..3].iter().all(|c| *c < 10));
            assert_eq!(at(&capture, outside_bottom), outside);
        }
        f.realand().layout.toggle_expo();
        assert_eq!(pixels(&mut f, RenderTarget::Output), output_pixels);
        let now = f.realand().clock.now_unadjusted();
        f.realand()
            .clock
            .set_unadjusted(now + Duration::from_millis(80));
        f.realand().advance_animations();
        let closing = pixels(&mut f, RenderTarget::Output);
        f.realand().layout.toggle_expo();
        assert_eq!(pixels(&mut f, RenderTarget::Output), closing,
            "Expo reversal must retain wallpaper and Bottom layers at scale={scale}, native={native}");
        f.realand().layout.toggle_expo();
        f.realand_complete_animations();
        assert_eq!(pixels(&mut f, RenderTarget::Output), desktop);
    }
}

#[test]
fn expo_layers_share_card_zoom_and_remain_continuous_on_gles() {
    test_expo_layers(false);
}

#[test]
#[ignore = "requires a Vulkan render node; never acquires display master"]
fn expo_layers_share_card_zoom_and_remain_continuous_on_vulkan() {
    test_expo_layers(true);
}
