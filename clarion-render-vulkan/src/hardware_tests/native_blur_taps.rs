//! Actual Native blur-pass and output-draw tap/flip coverage, no display master.
use super::*;
use crate::render_helpers::mapping_to_vec;
use smithay::backend::renderer::{Color32F, Frame as _, ImportMem, Texture as _};
mod oracle;
#[test]
#[ignore = "requires selected DRM render node and Vulkan; no GUI or display master"]
fn native_blur_vertex_footprints_match_independent_pixel_measure_and_y_orientation() {
    let (vk, _reference, _allocator) = setup();
    let mut renderer = native::NativeRenderer::new(vk).unwrap();
    renderer
        .prepare_output_format_for_test(Fourcc::Abgr8888)
        .unwrap();
    for (input, output) in [([13, 9], [7, 5]), ([7, 5], [17, 11]), ([1, 7], [1, 3])] {
        let bytes = oracle::image(input);
        for flip in [false, true] {
            let source = renderer
                .import_memory(
                    &bytes,
                    Fourcc::Abgr8888,
                    (input[0] as i32, input[1] as i32).into(),
                    flip,
                )
                .unwrap();
            for up in [false, true] {
                for offset in [0f32, 0.75, 2.25] {
                    let expected = oracle::convolve(&bytes, input, output, offset, up, flip);
                    for final_draw in [false, true] {
                        let mut target = renderer
                            .create_buffer(
                                Fourcc::Abgr8888,
                                (output[0] as i32, output[1] as i32).into(),
                            )
                            .unwrap();
                        let size = target
                            .size()
                            .to_logical(1, Transform::Normal)
                            .to_physical(1);
                        if final_draw {
                            let mut binding = renderer.bind(&mut target).unwrap();
                            let mut frame = renderer
                                .render(&mut binding, size, Transform::Normal)
                                .unwrap();
                            frame
                                .clear(Color32F::TRANSPARENT, &[Rectangle::from_size(size)])
                                .unwrap();
                            let footprint = if up { input } else { output };
                            frame
                                .draw_blur(
                                    &source,
                                    footprint.map(|side| 0.5 * offset / side as f32),
                                    up,
                                )
                                .unwrap();
                            frame.finish().unwrap().wait().unwrap();
                        } else {
                            let mut anchor = renderer
                                .create_buffer(Fourcc::Abgr8888, (1, 1).into())
                                .unwrap();
                            let mut binding = renderer.bind(&mut anchor).unwrap();
                            let mut frame = renderer
                                .render(&mut binding, (1, 1).into(), Transform::Normal)
                                .unwrap();
                            frame
                                .clear(
                                    Color32F::TRANSPARENT,
                                    &[Rectangle::from_size((1, 1).into())],
                                )
                                .unwrap();
                            frame.blur_pass(&source, &target, offset, up).unwrap();
                            frame.finish().unwrap().wait().unwrap();
                        }
                        let binding = renderer.bind(&mut target).unwrap();
                        let mapping = renderer
                            .copy_framebuffer(
                                &binding,
                                Rectangle::from_size(binding.size()),
                                Fourcc::Abgr8888,
                            )
                            .unwrap();
                        let actual = mapping_to_vec(&mut renderer, &mapping).unwrap();
                        assert_eq!(actual.len(), expected.len());
                        for (index, (&actual, &expected)) in
                            actual.iter().zip(&expected).enumerate()
                        {
                            assert!(actual.abs_diff(expected)<=1,"input={input:?} output={output:?} up={up} offset={offset} flip={flip} final={final_draw} pixel={} channel={}: {actual} vs {expected}",index/4,index%4);
                        }
                    }
                }
            }
        }
    }
}
