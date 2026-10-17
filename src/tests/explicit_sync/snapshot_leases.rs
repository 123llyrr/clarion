//! Old snapshots must keep their own attachment lease across a same-storage replacement.
use std::collections::HashMap;
use std::rc::Rc;

use calloop::timer::{TimeoutAction, Timer};
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::drm::DrmNode;
use smithay::backend::renderer::element::surface::{
    render_elements_from_surface_tree, WaylandSurfaceRenderElement,
};
use smithay::backend::renderer::element::{Element, Kind, RenderElement};
use smithay::backend::renderer::gles::Uniform;
use smithay::backend::renderer::utils::RendererSurfaceStateUserData;
use smithay::backend::renderer::{Bind, Color32F, ExportMem, Frame, Offscreen, Renderer};
use smithay::reexports::wayland_server::Resource;
use smithay::utils::{Rectangle, Transform};
use smithay::wayland::compositor::with_states;
use smithay::wayland::dmabuf::get_dmabuf;

use super::*;
use crate::render_helpers::shader_element::ShaderRenderElement;
use crate::render_helpers::shaders::{animation_source, mat3_uniform, ProgramType};
use crate::render_helpers::vulkan::hardware_tests::SubmittedGpuWork;
use crate::render_helpers::vulkan::native::{
    owned_surface_dmabuf, NativeRenderer, OwnedDmabufSource,
};
use crate::render_helpers::vulkan::VulkanRenderer;

fn wayland_buffer(
    fixture: &HardwareFixture,
    global: &ZwpLinuxDmabufV1,
    storage: &Dmabuf,
) -> WlBuffer {
    let params = global.create_params(&fixture.client.qh, ());
    let modifier = u64::from(storage.format().modifier);
    for (plane, ((fd, offset), stride)) in storage
        .handles()
        .zip(storage.offsets())
        .zip(storage.strides())
        .enumerate()
    {
        params.add(
            fd.as_fd(),
            plane as u32,
            offset,
            stride,
            (modifier >> 32) as u32,
            modifier as u32,
        );
    }
    let buffer = params.create_immed(
        64,
        64,
        Fourcc::Argb8888 as u32,
        zwp_linux_buffer_params_v1::Flags::empty(),
        &fixture.client.qh,
        BufferReleases::default(),
    );
    params.destroy();
    buffer
}

#[test]
#[ignore = "requires selected DRM GPU with DMA-BUF and timeline syncobj support"]
fn tile_close_source_survives_precommit_destroy_and_renderer_epoch() {
    use std::sync::atomic::AtomicBool;

    use smithay::wayland::compositor::{add_destruction_hook, add_pre_commit_hook};

    use crate::render_helpers::vulkan::native::recorded_surface_buffer_id;

    for destroy_surface in [false, true] {
        let mut fixture = HardwareFixture::new();
        fixture
            .server
            .state
            .backend
            .with_renderer(|renderer| {
                let AnyRenderer::Native(renderer) = renderer else {
                    unreachable!()
                };
                renderer
                    .prepare_output_format_for_test(Fourcc::Argb8888)
                    .unwrap();
                renderer
                    .prepare_output_format_for_test(Fourcc::Abgr8888)
                    .unwrap();
            })
            .unwrap();
        let syncobj: WpLinuxDrmSyncobjManagerV1 = fixture.bind();
        let dmabuf: ZwpLinuxDmabufV1 = fixture.bind();
        fixture.roundtrip_without_render();
        let acquire = Timeline::new(&fixture.node.device);
        acquire.signal(1);
        let release = Timeline::new(&fixture.node.device);
        let acquire_proxy =
            syncobj.import_timeline(acquire.export().as_fd(), &fixture.client.qh, ());
        let release_proxy =
            syncobj.import_timeline(release.export().as_fd(), &fixture.client.qh, ());
        let buffer = fixture.make_dmabuf(&dmabuf);
        let surface = fixture.client.create_window().surface.clone();
        let sync_surface = syncobj.get_surface(&surface, &fixture.client.qh, ());
        surface.commit();
        // Initial configure is emitted by the compositor's normal refresh.
        // The producer is not attached yet; later lifecycle steps deliberately
        // dispatch without rendering the headless output's synchronous fixture.
        fixture.roundtrip();
        fixture.roundtrip();
        fixture.client.window(&surface).ack_last();
        sync_surface.set_acquire_point(&acquire_proxy, 0, 1);
        sync_surface.set_release_point(&release_proxy, 0, 2);
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, 64, 64);
        surface.commit();
        fixture.roundtrip_without_render();
        fixture.roundtrip_without_render();
        assert_eq!(fixture.server.state.clarion.layout.windows().count(), 1);
        fixture
            .server
            .state
            .clarion
            .clock
            .set_complete_instantly(true);
        fixture.server.state.clarion.advance_animations();
        fixture
            .server
            .state
            .clarion
            .clock
            .set_complete_instantly(false);
        let server_surface = fixture
            .server
            .state
            .clarion
            .root_surface
            .keys()
            .find(|server| server.id().protocol_id() == surface.id().protocol_id())
            .unwrap()
            .clone();
        let expected = owned_surface_dmabuf(&server_surface)
            .unwrap()
            .dmabuf()
            .clone();
        let (window, output) = fixture
            .server
            .state
            .clarion
            .layout
            .find_window_and_output(&server_surface)
            .map(|(mapped, output)| (mapped.window.clone(), output.cloned()))
            .unwrap();
        let resources = fixture
            .server
            .state
            .clarion
            .layout
            .active_workspace()
            .unwrap()
            .tiles()
            .next()
            .unwrap()
            .scene_registration()
            .1;

        // Exercise the real Tile preparation and renderer-replacement entry.
        // A context epoch discards GPU snapshots but not this exact client lease.
        fixture
            .server
            .state
            .store_unmap_snapshot(&window, output.as_ref());
        assert_eq!(
            resources
                .native_close_source
                .borrow()
                .as_ref()
                .map(|s| s.dmabuf()),
            Some(&expected)
        );
        fixture.server.state.clarion.on_native_renderer_replaced();
        assert!(resources.native_unmap.borrow().is_none());
        assert_eq!(
            resources
                .native_close_source
                .borrow()
                .as_ref()
                .map(|s| s.dmabuf()),
            Some(&expected)
        );
        assert_eq!(release.value(), 0);

        // The mapped pre-commit hook clears stale close preparation on a normal
        // commit. It must not keep an old source for a later, different close.
        surface.damage_buffer(0, 0, 64, 64);
        surface.commit();
        fixture.roundtrip_without_render();
        fixture.roundtrip_without_render();
        assert!(resources.native_close_source.borrow().is_none());
        let observed = Arc::new(AtomicBool::new(false));
        let observe = {
            let observed = observed.clone();
            move |state: &mut State, server: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface| {
                let tile = state.clarion.layout.active_workspace().unwrap().tiles().next().unwrap();
                let resources = tile.scene_registration().1;
                assert_eq!(resources.native_close_source.borrow().as_ref().map(|s| s.dmabuf()), Some(&expected));
                if destroy_surface {
                    // This hook executes after State::destroyed and before the
                    // surface user data is reclaimed. The handler must have
                    // captured the source and cleared the surface-local slot.
                    assert!(recorded_surface_buffer_id(server).is_none());
                } else {
                    // Added after the mapped pre-commit hook: its Tile source is
                    // already captured, while the old renderer Buffer is current.
                    assert!(recorded_surface_buffer_id(server).is_some());
                }
                observed.store(true, Ordering::Relaxed);
            }
        };
        if destroy_surface {
            add_destruction_hook::<State, _>(&server_surface, observe);
            surface.destroy();
        } else {
            add_pre_commit_hook::<State, _>(&server_surface, move |state, _, server| {
                observe(state, server)
            });
            surface.attach(None, 0, 0);
            surface.commit();
        }
        fixture.roundtrip_without_render();
        fixture.roundtrip_without_render();
        assert!(
            observed.load(Ordering::Relaxed),
            "real close hook was not dispatched"
        );
        assert!(recorded_surface_buffer_id(&server_surface).is_none());
        if destroy_surface {
            // Deliberately destroy the protocol role after wl_surface, matching
            // the non-guaranteed destruction order on unexpected client exit.
            let window = fixture.client.window(&surface);
            window.xdg_toplevel.destroy();
            window.xdg_surface.destroy();
            fixture.roundtrip_without_render();
            fixture.roundtrip_without_render();
        }
        assert_eq!(fixture.server.state.clarion.layout.windows().count(), 0);
        assert!(resources.native_close_source.borrow().is_none());
        // Discard the completed client's animations through the production epoch
        // reset. The held test lease then contains no original client Buffer.
        fixture.server.state.clarion.on_native_renderer_replaced();
        let deadline = Instant::now() + Duration::from_secs(5);
        while release.value() != 2 {
            assert!(
                Instant::now() < deadline,
                "original attachment did not retire after close"
            );
            fixture.roundtrip_without_render();
        }
        fixture
            .server
            .state
            .clarion
            .display_handle
            .flush_clients()
            .unwrap();
        fixture.client.dispatch();
        assert_eq!(buffer.data::<BufferReleases>().unwrap().count(), 1);
        assert!(fixture.client.connection.protocol_error().is_none());
    }
}

#[test]
#[ignore = "requires selected DRM GPU with DMA-BUF and timeline syncobj support"]
fn replaced_same_storage_snapshots_submit_without_owner_wait_and_release_on_idle() {
    same_storage_snapshot_release(false);
}

#[test]
#[ignore = "requires selected DRM GPU with DMA-BUF and timeline syncobj support"]
fn owned_close_fallback_copies_detached_generations_without_owner_wait() {
    same_storage_snapshot_release(true);
}

fn same_storage_snapshot_release(owned_close: bool) {
    for export_required in [true, false] {
        let mut fixture = HardwareFixture::new();
        let syncobj: WpLinuxDrmSyncobjManagerV1 = fixture.bind();
        let dmabuf: ZwpLinuxDmabufV1 = fixture.bind();
        fixture.roundtrip();
        let acquire = Timeline::new(&fixture.node.device);
        acquire.signal(1);
        let acquire_proxy =
            syncobj.import_timeline(acquire.export().as_fd(), &fixture.client.qh, ());
        let releases = [
            Timeline::new(&fixture.node.device),
            Timeline::new(&fixture.node.device),
        ];
        let release_proxies = releases.each_ref().map(|timeline| {
            syncobj.import_timeline(timeline.export().as_fd(), &fixture.client.qh, ())
        });
        let modifiers = fixture
            .server
            .state
            .backend
            .with_renderer(|renderer| match renderer {
                AnyRenderer::Native(renderer) => renderer
                    .dmabuf_formats()
                    .iter()
                    .filter(|format| format.code == Fourcc::Argb8888)
                    .map(|format| format.modifier)
                    .collect::<Vec<_>>(),
                _ => unreachable!(),
            })
            .unwrap();
        let mut storage = fixture
            .node
            .allocator
            .create_buffer(64, 64, Fourcc::Argb8888, &modifiers)
            .unwrap()
            .export()
            .unwrap();
        // Separate protocol resources name the very same producer allocation.
        // A lease stored in an image/cache instead of a handle can cross these generations.
        let buffers = [
            wayland_buffer(&fixture, &dmabuf, &storage),
            wayland_buffer(&fixture, &dmabuf, &storage),
        ];
        assert_ne!(buffers[0].id(), buffers[1].id());
        let mut vulkan = VulkanRenderer::new();
        vulkan
            .init_from_drm_node(Some(&DrmNode::from_file(&fixture.node.device).unwrap()))
            .unwrap();
        eprintln!(
            "HARDWARE: {} (same-storage snapshot lease)",
            vulkan.info.device_name
        );
        let mut renderer = NativeRenderer::new(vulkan).unwrap();
        renderer.set_completion_export_required(export_required);
        renderer
            .prepare_output_format_for_test(Fourcc::Argb8888)
            .unwrap();
        renderer
            .prepare_output_format_for_test(Fourcc::Abgr8888)
            .unwrap();
        // Fill the independent producer before attaching it. This diagnostic synchronization
        // and source preparation are outside the old-snapshot submission under test.
        {
            let mut framebuffer = renderer.bind(&mut storage).unwrap();
            let mut frame = renderer
                .render(&mut framebuffer, (64, 64).into(), Transform::Normal)
                .unwrap();
            frame
                .clear(
                    Color32F::new(0.25, 0.5, 0.75, 1.),
                    &[Rectangle::from_size((64, 64).into())],
                )
                .unwrap();
            frame.finish().unwrap().wait().unwrap();
        }
        renderer.retire_completed().unwrap();
        let surface = fixture
            .client
            .state
            .compositor
            .as_ref()
            .unwrap()
            .create_surface(&fixture.client.qh, ());
        let sync_surface = syncobj.get_surface(&surface, &fixture.client.qh, ());
        let mut snapshots: Vec<WaylandSurfaceRenderElement<NativeRenderer>> = Vec::new();
        let mut shader_inputs = Vec::new();
        let mut raw_cache_handles = Vec::new();
        let mut owned_sources: Vec<OwnedDmabufSource> = Vec::new();
        for index in 0..2 {
            surface.attach(Some(&buffers[index]), 0, 0);
            sync_surface.set_acquire_point(&acquire_proxy, 0, 1);
            sync_surface.set_release_point(&release_proxies[index], 0, 2);
            surface.damage_buffer(0, 0, 64, 64);
            surface.commit();
            fixture.roundtrip_without_render();
            fixture.roundtrip_without_render();
            let server_surface = fixture
                .server
                .state
                .clarion
                .root_surface
                .keys()
                .find(|server| server.id().protocol_id() == surface.id().protocol_id())
                .unwrap()
                .clone();
            let source = owned_surface_dmabuf(&server_surface)
                .expect("committed DMA-BUF must retain Smithay's original attachment");
            if let Some(previous) = owned_sources.first() {
                let owner = with_states(&server_surface, |states| {
                    states
                        .data_map
                        .get::<RendererSurfaceStateUserData>()
                        .unwrap()
                        .lock()
                        .unwrap()
                        .buffer()
                        .unwrap()
                        .clone()
                });
                // Identical storage via a second protocol resource is still a
                // distinct attachment generation; never attach its release lease
                // to the old Dmabuf object just because the FDs name the same file.
                assert!(OwnedDmabufSource::new(previous.dmabuf().clone(), owner).is_err());
            }
            owned_sources.push(source);
            snapshots.extend(render_elements_from_surface_tree(
                &mut renderer,
                &server_surface,
                (index as i32 * 64, 0),
                1.,
                1.,
                Kind::Unspecified,
            ));
            // An exact cache hit through raw ImportDma must not inherit this
            // Wayland attachment lease. Keep that handle alive past idle release.
            let (raw_source, shader_input) = with_states(&server_surface, |states| {
                let data = states
                    .data_map
                    .get::<RendererSurfaceStateUserData>()
                    .unwrap()
                    .lock()
                    .unwrap();
                (
                    get_dmabuf(data.buffer().unwrap()).unwrap().clone(),
                    data.texture(renderer.context_id()).unwrap().clone(),
                )
            });
            shader_inputs.push(shader_input);
            raw_cache_handles.push(renderer.import_dmabuf(&raw_source, None).unwrap());
            assert_eq!(snapshots.len(), index + 1);
            assert_eq!(
                releases[0].value(),
                0,
                "replacement released the retained old snapshot"
            );
        }
        surface.attach(None, 0, 0);
        surface.commit();
        fixture.roundtrip_without_render();
        fixture.roundtrip_without_render();
        for release in &releases {
            assert_eq!(
                release.value(),
                0,
                "detachment released a retained snapshot"
            );
        }

        // The 64-pixel inputs are stretched over 128 output pixels. The
        // animation sampler intentionally filters against a transparent border,
        // so full unit UVs would dim the first/last columns. Keep both sampled
        // attachments inside their texel centers for this uniform-color oracle.
        let source = animation_source(
            ProgramType::Close,
            Some(
                "uniform sampler2D second;\n\
             vec4 close_color(vec3 p, vec3 s) {\n\
             vec2 half_texel = vec2(0.5) / vec2(textureSize(clarion_tex, 0));\n\
             vec2 uv = clamp(p.xy, half_texel, vec2(1.0) - half_texel);\n\
             return (texture2D(clarion_tex, uv) + texture2D(second, uv)) / 2.0;\n\
             }",
            ),
        );
        renderer
            .wait_for_animation_program_for_test(ProgramType::Close, &source)
            .unwrap();
        let shader = ShaderRenderElement::new_native(
            &mut renderer,
            ProgramType::Close,
            &source,
            (128., 64.).into(),
            1.,
            1.,
            Rc::new([
                mat3_uniform("clarion_input_to_geo", glam::Mat3::IDENTITY),
                mat3_uniform("clarion_geo_to_tex", glam::Mat3::IDENTITY),
                Uniform::new("clarion_geo_size", [128., 64.]),
                Uniform::new("clarion_glide_enabled", 0f32),
            ]),
            HashMap::from([
                ("clarion_tex".into(), shader_inputs.remove(0)),
                ("second".into(), shader_inputs.remove(0)),
            ]),
            Kind::Unspecified,
        )
        .unwrap();

        if owned_close {
            let builtin = animation_source(
                ProgramType::Close,
                crate::render_helpers::shaders::resolve_close_shader(
                    None,
                    clarion_config::WindowCloseEffect::Fade,
                ),
            );
            renderer
                .wait_for_animation_program_for_test(ProgramType::Close, &builtin)
                .unwrap();
        }
        // Queue a legal long GPU workload before the one-shot owned copies.
        // Their completion must hold the attachment after all source handles drop.
        let mut work = SubmittedGpuWork::new(renderer.vulkan().ctx.as_ref().unwrap().clone());
        let _producer = work.submit();
        let waits = crate::render_helpers::vulkan::owner_fence_wait_calls_for_test();
        let scene_resources = crate::scene::resources::SceneResources::new();
        let mut closings = Vec::new();
        let mut close_elements = Vec::new();
        if owned_close {
            use crate::animation::Animation;
            use crate::render_helpers::{RenderCtx, RenderTarget};
            use crate::scene::closing_window::ClosingWindow;
            use crate::scene::window_resources::NativeCloseSource;
            use crate::utils::transaction::Transaction;

            for (index, source) in owned_sources.drain(..).enumerate() {
                let clock = crate::animation::Clock::with_time(Duration::ZERO);
                let transaction = Transaction::new();
                let closing = ClosingWindow::new_native(
                    &mut renderer,
                    1.0.into(),
                    (64., 64.).into(),
                    (index as f64 * 64., 0.).into(),
                    transaction.blocker(),
                    Animation::new(
                        clock,
                        0.,
                        1.,
                        0.,
                        clarion_config::Animations::default().window_close.anim,
                    ),
                    Default::default(),
                    clarion_config::WindowCloseEffect::Fade,
                    None,
                    None,
                    Some(NativeCloseSource::Owned(source)),
                    scene_resources.allocator().closing,
                )
                .unwrap();
                // A failed full snapshot gives no trusted capture image. The
                // output may animate the copy; screen/window capture stays hidden.
                for target in [RenderTarget::Screencast, RenderTarget::ScreenCapture] {
                    assert!(closing
                        .render(
                            RenderCtx {
                                renderer: &mut renderer,
                                target,
                                xray: None
                            },
                            Rectangle::from_size((128., 64.).into()),
                            1.0.into()
                        )
                        .is_empty());
                }
                close_elements.extend(closing.render(
                    RenderCtx {
                        renderer: &mut renderer,
                        target: RenderTarget::Output,
                        xray: None,
                    },
                    Rectangle::from_size((128., 64.).into()),
                    1.0.into(),
                ));
                closings.push(closing);
            }
        }
        drop(owned_sources);
        let mut completion_loop = EventLoop::<bool>::try_new().unwrap();
        renderer
            .install_completion_wakeup(&completion_loop.handle())
            .unwrap();
        let mut target = renderer
            .create_buffer(Fourcc::Abgr8888, (128, 64).into())
            .unwrap();
        let mut framebuffer = renderer.bind(&mut target).unwrap();
        let mut frame = renderer
            .render(&mut framebuffer, (128, 64).into(), Transform::Normal)
            .unwrap();
        frame
            .clear(Color32F::BLACK, &[Rectangle::from_size((128, 64).into())])
            .unwrap();
        if owned_close {
            for element in &close_elements {
                let geometry = element.geometry(1.0.into());
                <crate::scene::closing_window::ClosingWindowRenderElement as RenderElement<
                    NativeRenderer,
                >>::draw(
                    element,
                    &mut frame,
                    element.src(),
                    geometry,
                    &[Rectangle::from_size(geometry.size)],
                    &[],
                    None,
                )
                .unwrap();
            }
        } else if export_required {
            for snapshot in &snapshots {
                let geometry = snapshot.geometry(1.0.into());
                snapshot
                    .draw(
                        &mut frame,
                        snapshot.src(),
                        geometry,
                        &[Rectangle::from_size(geometry.size)],
                        &[],
                        None,
                    )
                    .unwrap();
            }
        } else {
            // ShaderDraw's two sampled wrappers must retain both attachment
            // leases as well; this branch has no Quad draw to hide an omission.
            let geometry = shader.geometry(1.0.into());
            <ShaderRenderElement as RenderElement<NativeRenderer>>::draw(
                &shader,
                &mut frame,
                shader.src(),
                geometry,
                &[Rectangle::from_size(geometry.size)],
                &[],
                None,
            )
            .unwrap();
        }
        let started = Instant::now();
        let sync = frame.finish().unwrap();
        assert_eq!(
            crate::render_helpers::vulkan::owner_fence_wait_calls_for_test(),
            waits,
            "owned source/copy submission performed a compositor-thread fence wait"
        );
        eprintln!(
            "OLD_SNAPSHOT_OWNER_WORK export_required={export_required} finish_wall_ms={}",
            started.elapsed().as_secs_f64() * 1000.
        );
        assert!(
            !sync.is_reached(),
            "old snapshot finish waited for GPU completion"
        );
        drop(framebuffer);
        drop(snapshots);
        drop(shader);
        for (release, buffer) in releases.iter().zip(&buffers) {
            assert_eq!(
                release.value(),
                0,
                "submitted snapshot released while sampled by GPU"
            );
            assert_eq!(buffer.data::<BufferReleases>().unwrap().count(), 0);
        }
        completion_loop
            .handle()
            .insert_source(
                Timer::from_duration(Duration::from_secs(10)),
                |_, _, timed_out| {
                    *timed_out = true;
                    TimeoutAction::Drop
                },
            )
            .unwrap();
        let mut timed_out = false;
        // No renderer prune, second frame, or client destruction can release these owners.
        completion_loop.dispatch(None, &mut timed_out).unwrap();
        assert!(
            !timed_out,
            "old snapshot idle GPU completion failed to wake"
        );
        assert!(sync.is_reached());
        fixture
            .server
            .state
            .clarion
            .display_handle
            .flush_clients()
            .unwrap();
        fixture.client.dispatch();
        for (release, buffer) in releases.iter().zip(&buffers) {
            assert_eq!(
                release.value(),
                2,
                "an exact attachment lease did not retire on idle"
            );
            assert_eq!(buffer.data::<BufferReleases>().unwrap().count(), 1);
        }
        assert_eq!(
            raw_cache_handles.len(),
            2,
            "raw image/cache handles remain alive without keeping attachment leases"
        );
        assert!(fixture.client.connection.protocol_error().is_none());
        let mapping = renderer
            .copy_texture(
                &target,
                Rectangle::from_size((128, 64).into()),
                Fourcc::Abgr8888,
            )
            .unwrap();
        let pixels = renderer.map_texture(&mapping).unwrap();
        let first_mismatch = pixels
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .find(|(_, pixel)| {
                pixel
                    .iter()
                    .zip([64u8, 128, 191, 255])
                    .any(|(actual, expected)| actual.abs_diff(expected) > 1)
            })
            .map(|(index, pixel)| (index % 128, index / 128, pixel));
        assert!(
            pixels.as_chunks::<4>().0.iter().all(|pixel| pixel
                .iter()
                .zip([64u8, 128, 191, 255])
                .all(|(actual, expected)| actual.abs_diff(expected) <= 1)),
            "old and new same-storage snapshots lost their pixels: \
             export_required={export_required}, first_mismatch_xy_rgba={first_mismatch:?}"
        );
    }
}
