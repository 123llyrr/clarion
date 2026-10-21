//! Exact FP16 image replay is a texel permutation, including reflected turns.
use super::*;

#[test]
#[ignore = "requires selected physical DRM render node and native Vulkan"]
fn exact_fp16_replay_keeps_half_values_at_all_orthogonal_pixel_centers() {
    let (vk, _gles, _allocator) = super::super::hardware_tests::setup();
    let mut renderer = NativeRenderer::new(vk).unwrap();
    renderer
        .prepare_output_format_for_test(Fourcc::Abgr16161616f)
        .unwrap();
    let size: Size<i32, Buffer> = (640, 400).into();
    let mut bytes = Vec::with_capacity((size.w * size.h * 8) as usize);
    let mut expected = Vec::with_capacity((size.w * size.h * 4) as usize);
    // Positive binary16 values throughout [0,1], including every mantissa and
    // values near UNORM thresholds. Decode independently in f64 for the oracle.
    let decode = |bits: u16| {
        let exponent = (bits >> 10) & 31;
        let fraction = f64::from(bits & 1023);
        if exponent == 0 {
            fraction * 2.0_f64.powi(-24)
        } else {
            (1. + fraction / 1024.) * 2.0_f64.powi(i32::from(exponent) - 15)
        }
    };
    for index in 0..(size.w * size.h) as u32 {
        for channel in 0..3 {
            let bits = ((index.wrapping_mul(37) + channel * 1337) % 0x3c01) as u16;
            bytes.extend_from_slice(&bits.to_le_bytes());
            expected.push((decode(bits) * 255.).round() as u8);
        }
        bytes.extend_from_slice(&0x3c00_u16.to_le_bytes());
        expected.push(255);
    }
    let source = renderer
        .import_memory(&bytes, Fourcc::Abgr16161616f, size, false)
        .unwrap();
    let mapping = renderer
        .copy_texture(&source, Rectangle::from_size(size), Fourcc::Abgr8888)
        .unwrap();
    assert!(
        renderer.map_texture(&mapping).unwrap() == expected,
        "direct half-float decode must agree with scalar oracle"
    );
    for transform in [
        Transform::Normal,
        Transform::_90,
        Transform::_180,
        Transform::_270,
        Transform::Flipped,
        Transform::Flipped90,
        Transform::Flipped180,
        Transform::Flipped270,
    ] {
        let output: Size<i32, Physical> = transform.transform_size(Size::from((size.w, size.h)));
        let mut image: VulkanTexture = renderer
            .create_buffer(Fourcc::Abgr16161616f, (output.w, output.h).into())
            .unwrap();
        {
            let mut target = renderer.bind(&mut image).unwrap();
            let mut frame = renderer
                .render(&mut target, output, Transform::Normal)
                .unwrap();
            let destination = Rectangle::from_size(output);
            frame.clear(Color32F::TRANSPARENT, &[destination]).unwrap();
            frame
                .render_texture_from_to(
                    &source,
                    Rectangle::from_size(size.to_f64()),
                    destination,
                    &[destination],
                    &[],
                    transform,
                    1.,
                )
                .unwrap();
            frame.finish().unwrap().wait().unwrap();
        }
        let mapping = renderer
            .copy_texture(
                &image,
                Rectangle::from_size((output.w, output.h).into()),
                Fourcc::Abgr8888,
            )
            .unwrap();
        let pixels = renderer.map_texture(&mapping).unwrap();
        let mut mismatches = 0;
        let mut first = None;
        for y in 0..size.h {
            for x in 0..size.w {
                // Integer source->destination permutation, independent of the
                // renderer's floating unit-square matrices and normalized UVs.
                let (dx, dy) = match transform {
                    Transform::Normal => (x, y),
                    Transform::_90 => (size.h - 1 - y, x),
                    Transform::_180 => (size.w - 1 - x, size.h - 1 - y),
                    Transform::_270 => (y, size.w - 1 - x),
                    Transform::Flipped => (size.w - 1 - x, y),
                    Transform::Flipped90 => (y, x),
                    Transform::Flipped180 => (x, size.h - 1 - y),
                    Transform::Flipped270 => (size.h - 1 - y, size.w - 1 - x),
                };
                let from = ((y * size.w + x) * 4) as usize;
                let to = ((dy * output.w + dx) * 4) as usize;
                if pixels[to..to + 4] != expected[from..from + 4] {
                    mismatches += 1;
                    first.get_or_insert((x, y, dx, dy));
                }
            }
        }
        assert_eq!(
            mismatches, 0,
            "{transform:?} FP16 texel permutation first mismatch {first:?}"
        );
    }
}

#[test]
#[ignore = "requires selected DRM render node and native Vulkan; no display master"]
fn fp16_source_over_preserves_opaque_coverage_and_fractional_alpha() {
    let (vk, _gles, _allocator) = super::super::hardware_tests::setup();
    let mut renderer = NativeRenderer::new(vk).unwrap();
    renderer
        .prepare_output_format_for_test(Fourcc::Abgr16161616f)
        .unwrap();
    let size: Size<i32, Buffer> = (256, 4).into();
    let mut bytes = Vec::with_capacity((size.w * size.h * 4) as usize);
    for _ in 0..size.h {
        for alpha in 0..256 {
            bytes.extend_from_slice(&[alpha as u8, 0, 0, alpha as u8]);
        }
    }
    let source = renderer
        .import_memory(&bytes, Fourcc::Abgr8888, size, false)
        .unwrap();
    let mut image: VulkanTexture = renderer.create_buffer(Fourcc::Abgr16161616f, size).unwrap();
    let backgrounds = [1., 0., 0.5, 2047. / 2048.];
    let output: Size<i32, Physical> = (size.w, size.h).into();
    let full = Rectangle::from_size(output);
    {
        let mut target = renderer.bind(&mut image).unwrap();
        let mut frame = renderer
            .render(&mut target, output, Transform::Normal)
            .unwrap();
        for (row, alpha) in backgrounds.into_iter().enumerate() {
            frame
                .clear(
                    Color32F::new(0., alpha / 4., 0., alpha),
                    &[Rectangle::new((0, row as i32).into(), (256, 1).into())],
                )
                .unwrap();
        }
        // Three independent source-over passes make alpha drift visible while
        // retaining a f64 oracle with a bounded FP16 accumulation error.
        for _ in 0..3 {
            frame
                .render_texture_from_to(
                    &source,
                    Rectangle::from_size(size.to_f64()),
                    full,
                    &[full],
                    &[],
                    Transform::Normal,
                    1.,
                )
                .unwrap();
        }
        frame.finish().unwrap().wait().unwrap();
    }
    let mapping = renderer
        .copy_texture(&image, Rectangle::from_size(size), Fourcc::Abgr16161616f)
        .unwrap();
    let pixels = renderer.map_texture(&mapping).unwrap();
    let half = |bits: u16| {
        let exponent = (bits >> 10) & 31;
        let mantissa = f64::from(bits & 1023);
        if exponent == 0 {
            mantissa * 2.0_f64.powi(-24)
        } else {
            (1. + mantissa / 1024.) * 2.0_f64.powi(i32::from(exponent) - 15)
        }
    };
    for (pixel, actual) in pixels.as_chunks::<8>().0.iter().enumerate() {
        let row = pixel / 256;
        let source = (pixel % 256) as f64 / 255.;
        let background = f64::from(backgrounds[row]);
        let mut expected = [0., background / 4., 0., background];
        for _ in 0..3 {
            expected = std::array::from_fn(|channel| {
                let value = if channel == 0 || channel == 3 {
                    source
                } else {
                    0.
                };
                value + expected[channel] * (1. - source)
            });
        }
        for (channel, expected) in expected.into_iter().enumerate() {
            let bits = u16::from_le_bytes([actual[channel * 2], actual[channel * 2 + 1]]);
            if row == 0 && channel == 3 {
                assert_eq!(
                    bits,
                    0x3c00,
                    "opaque destination lost coverage at source alpha {}",
                    pixel % 256
                );
            }
            // At most three FP16 stores plus fixed-function factor rounding.
            // This bound applies only to fractional arithmetic, never the
            // exact opaque invariant above or the Stage whole-frame assertion.
            assert!(
                (half(bits) - expected).abs() <= 0.0015,
                "row={row} source={} channel={channel}: {} vs {}",
                pixel % 256,
                half(bits),
                expected
            );
        }
        if row == 1 && pixel % 256 == 0 {
            assert_eq!(
                u16::from_le_bytes([actual[6], actual[7]]),
                0,
                "transparent input must stay transparent"
            );
        }
    }
}
