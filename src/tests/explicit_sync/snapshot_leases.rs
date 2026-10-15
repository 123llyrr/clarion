//! Old snapshots must keep their own attachment lease across a same-storage replacement.
use super::*;
use calloop::timer::{TimeoutAction, Timer};
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::drm::DrmNode;
use smithay::backend::renderer::element::{
    surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
    Element, Kind, RenderElement,
};
use smithay::backend::renderer::utils::RendererSurfaceStateUserData;
use smithay::backend::renderer::{
    gles::Uniform, Bind, Color32F, ExportMem, Frame, Offscreen, Renderer,
};
use smithay::reexports::wayland_server::Resource;
use smithay::utils::{Rectangle, Transform};
use smithay::wayland::{compositor::with_states, dmabuf::get_dmabuf};

use crate::render_helpers::shader_element::ShaderRenderElement;
use crate::render_helpers::shaders::{animation_source, mat3_uniform, ProgramType};
use crate::render_helpers::vulkan::hardware_tests::SubmittedGpuWork;
use crate::render_helpers::vulkan::native::NativeRenderer;
use crate::render_helpers::vulkan::VulkanRenderer;
use std::{collections::HashMap, rc::Rc};

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
fn replaced_same_storage_snapshots_submit_without_owner_wait_and_release_on_idle() {
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
                .realand
                .root_surface
                .keys()
                .find(|server| server.id().protocol_id() == surface.id().protocol_id())
                .unwrap()
                .clone();
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
             vec2 half_texel = vec2(0.5) / vec2(textureSize(realand_tex, 0));\n\
             vec2 uv = clamp(p.xy, half_texel, vec2(1.0) - half_texel);\n\
             return (texture2D(realand_tex, uv) + texture2D(second, uv)) / 2.0;\n\
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
                mat3_uniform("realand_input_to_geo", glam::Mat3::IDENTITY),
                mat3_uniform("realand_geo_to_tex", glam::Mat3::IDENTITY),
                Uniform::new("realand_geo_size", [128., 64.]),
                Uniform::new("realand_glide_enabled", 0f32),
            ]),
            HashMap::from([
                ("realand_tex".into(), shader_inputs.remove(0)),
                ("second".into(), shader_inputs.remove(0)),
            ]),
            Kind::Unspecified,
        )
        .unwrap();

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
        if export_required {
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
        // Use a legal already-submitted workload, including when exporting a SYNC_FD.
        let mut work = SubmittedGpuWork::new(frame.context());
        let _producer = work.submit();
        let started = Instant::now();
        let sync = frame.finish().unwrap();
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
            .realand
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
