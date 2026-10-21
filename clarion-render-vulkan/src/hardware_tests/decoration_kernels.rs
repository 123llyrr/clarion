//! Real pixel checks against standard transfer values and an independent
//! numerical convolution, rather than only comparing two GPU backends.
use clarion_config::{
    Color, CornerRadius, GradientColorSpace, GradientInterpolation, HueInterpolation,
};

use super::*;
use crate::render_helpers::border::BorderRenderElement;
use crate::render_helpers::shadow::ShadowRenderElement;
use crate::render_helpers::{render_to_vec, resources, shaders};

fn cdf(x: f64) -> f64 {
    if x <= -6.0 {
        return 0.0;
    }
    if x >= 6.0 {
        return 1.0;
    }
    // Integrate the Taylor expansion of exp(-t*t/2) in f64. This oracle
    // shares neither the shader's CDF approximation nor its quadrature.
    let mut term = x;
    let mut integral = x;
    for n in 1..100 {
        term *= -x * x / (2.0 * n as f64);
        integral += term / (2 * n + 1) as f64;
    }
    0.5 + integral / (2.0 * std::f64::consts::PI).sqrt()
}

fn integrated_shadow(x: f64, y: f64, width: f64, height: f64, radii: [f64; 4], sigma: f64) -> f64 {
    let lower = (y - 6.0 * sigma).max(0.0);
    let upper = (y + 6.0 * sigma).min(height);
    if lower >= upper {
        return 0.0;
    }
    let step = (upper - lower) / 1024.0;
    let mut mass = 0.0;
    for i in 0..1024 {
        let row = lower + (i as f64 + 0.5) * step;
        let mut left: f64 = 0.0;
        let mut right = width;
        for (radius, edge, is_right) in [
            (radii[0], row, false),
            (radii[1], row, true),
            (radii[2], height - row, true),
            (radii[3], height - row, false),
        ] {
            if edge < radius {
                let inset = radius - (radius * radius - (radius - edge).powi(2)).max(0.0).sqrt();
                if is_right {
                    right = right.min(width - inset);
                } else {
                    left = left.max(inset);
                }
            }
        }
        let horizontal = cdf((right - x) / sigma) - cdf((left - x) / sigma);
        let dy = (row - y) / sigma;
        mass += horizontal.max(0.0) * (-0.5 * dy * dy).exp();
    }
    mass * step / ((2.0 * std::f64::consts::PI).sqrt() * sigma)
}

#[test]
fn numerical_normal_oracle_matches_known_probabilities() {
    for (x, expected) in [
        (0.0, 0.5),
        (1.0, 0.8413447460685429),
        (2.0, 0.9772498680518208),
        (4.0, 0.9999683287581669),
    ] {
        assert!((cdf(x) - expected).abs() < 1e-10);
        assert!((cdf(x) + cdf(-x) - 1.0).abs() < 1e-10);
    }
}

#[test]
#[ignore = "requires selected DRM node, Vulkan and GLES"]
fn gradient_pixels_follow_srgb_transfer_and_alpha_weighted_endpoints() {
    let (vk, mut gles, _) = setup();
    let mut native = native::NativeRenderer::new(vk).unwrap();
    native
        .prepare_output_format_for_test(Fourcc::Abgr8888)
        .unwrap();
    resources::init(&mut gles);
    shaders::init(&mut gles);
    let size = (160, 32);
    for space in [
        GradientColorSpace::SrgbLinear,
        GradientColorSpace::Oklab,
        GradientColorSpace::Oklch,
    ] {
        for invisible in [false, true] {
            let a = if invisible {
                Color::new_unpremul(1.0, 0.0, 0.0, 0.0)
            } else {
                Color::new_unpremul(0.0, 0.0, 0.0, 1.0)
            };
            let b = if invisible {
                Color::new_unpremul(0.0, 0.0, 1.0, 0.8)
            } else {
                Color::new_unpremul(1.0, 1.0, 1.0, 1.0)
            };
            let element = BorderRenderElement::new(
                (160.0, 32.0).into(),
                Rectangle::from_size((160.0, 32.0).into()),
                GradientInterpolation {
                    color_space: space,
                    hue_interpolation: HueInterpolation::Shorter,
                },
                a,
                b,
                0.0,
                Rectangle::from_size((160.0, 32.0).into()),
                0.0,
                CornerRadius::default(),
                1.0,
                1.0,
            );
            let native_pixels = render_to_vec(
                &mut native,
                size.into(),
                1.0.into(),
                Transform::Normal,
                Fourcc::Abgr8888,
                [&element].into_iter(),
            )
            .unwrap();
            let gles_pixels = render_to_vec(
                &mut gles,
                size.into(),
                1.0.into(),
                Transform::Normal,
                Fourcc::Abgr8888,
                [&element].into_iter(),
            )
            .unwrap();
            for x in [0usize, 15, 63, 79, 127, 159] {
                let t = (x as f64 + 0.5) / 160.0;
                let expected = if invisible {
                    [
                        0,
                        0,
                        (0.8 * t * 255.0).round() as u8,
                        (0.8 * t * 255.0).round() as u8,
                    ]
                } else {
                    // For a black/white Oklab interpolation L=t, so linear RGB=t^3.
                    let linear = if matches!(space, GradientColorSpace::SrgbLinear) {
                        t
                    } else {
                        t * t * t
                    };
                    let encoded = if linear <= 0.0031308 {
                        12.92 * linear
                    } else {
                        1.055 * linear.powf(1.0 / 2.4) - 0.055
                    };
                    let c = (encoded * 255.0).round() as u8;
                    [c, c, c, 255]
                };
                let offset = (16 * size.0 as usize + x) * 4;
                for pixels in [&native_pixels, &gles_pixels] {
                    let actual = &pixels[offset..offset + 4];
                    assert!(
                        actual.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 2),
                        "{space:?} transparent={invisible} x={x}: {actual:?} != {expected:?}"
                    );
                }
            }
        }
    }
}

#[test]
#[ignore = "requires selected DRM node, Vulkan and GLES"]
fn shadow_pixels_follow_independent_four_corner_convolution() {
    let (vk, mut gles, _) = setup();
    let mut native = native::NativeRenderer::new(vk).unwrap();
    native
        .prepare_output_format_for_test(Fourcc::Abgr8888)
        .unwrap();
    resources::init(&mut gles);
    shaders::init(&mut gles);
    for sigma in [0.2, 2.0, 8.0] {
        let radii = [2.0, 14.0, 5.0, 9.0];
        let element = ShadowRenderElement::new(
            (64.0, 40.0).into(),
            Rectangle::new((8.0, 6.0).into(), (46.0, 29.0).into()),
            Color::new_unpremul(1.0, 1.0, 1.0, 1.0),
            sigma,
            CornerRadius {
                top_left: 2.0,
                top_right: 14.0,
                bottom_right: 5.0,
                bottom_left: 9.0,
            },
            1.0,
            Rectangle::default(),
            CornerRadius::default(),
            1.0,
        );
        let native_pixels = render_to_vec(
            &mut native,
            (64, 40).into(),
            1.0.into(),
            Transform::Normal,
            Fourcc::Abgr8888,
            [&element].into_iter(),
        )
        .unwrap();
        let gles_pixels = render_to_vec(
            &mut gles,
            (64, 40).into(),
            1.0.into(),
            Transform::Normal,
            Fourcc::Abgr8888,
            [&element].into_iter(),
        )
        .unwrap();
        for y in (0..40).step_by(3) {
            for x in (0..64).step_by(3) {
                let expected = (integrated_shadow(
                    x as f64 + 0.5 - 8.0,
                    y as f64 + 0.5 - 6.0,
                    46.0,
                    29.0,
                    radii,
                    f64::from(sigma),
                ) * 255.0)
                    .round() as u8;
                let offset = (y * 64 + x) * 4;
                for pixels in [&native_pixels, &gles_pixels] {
                    let actual = pixels[offset + 3];
                    assert!(
                        actual.abs_diff(expected) <= 2,
                        "sigma={sigma} pixel=({x},{y}): {actual} != oracle {expected}"
                    );
                }
            }
        }
    }
}

#[test]
#[ignore = "requires selected DRM node, Vulkan and GLES"]
fn high_power_squircles_have_finite_full_coverage_inside_corners() {
    let (vk, mut gles, _) = setup();
    let mut native = native::NativeRenderer::new(vk).unwrap();
    native
        .prepare_output_format_for_test(Fourcc::Abgr8888)
        .unwrap();
    resources::init(&mut gles);
    shaders::init(&mut gles);
    let white = Color::new_unpremul(1.0, 1.0, 1.0, 1.0);
    let element = BorderRenderElement::new(
        (128.0, 128.0).into(),
        Rectangle::from_size((128.0, 128.0).into()),
        GradientInterpolation::default(),
        white,
        white,
        0.0,
        Rectangle::from_size((128.0, 128.0).into()),
        0.0,
        CornerRadius {
            top_left: 64.0,
            top_right: 64.0,
            bottom_right: 64.0,
            bottom_left: 64.0,
        },
        1.0,
        1.0,
    )
    .with_rounding_power(64.0);
    let native_pixels = render_to_vec(
        &mut native,
        (128, 128).into(),
        1.0.into(),
        Transform::Normal,
        Fourcc::Abgr8888,
        [&element].into_iter(),
    )
    .unwrap();
    let gles_pixels = render_to_vec(
        &mut gles,
        (128, 128).into(),
        1.0.into(),
        Transform::Normal,
        Fourcc::Abgr8888,
        [&element].into_iter(),
    )
    .unwrap();
    for y in [4, 32, 64, 96, 123] {
        for x in [4, 32, 64, 96, 123] {
            // In all these cells, (|x-c|/r)^64+(|y-c|/r)^64 < 1
            // by a margin greater than one complete antialiasing pixel.
            for pixels in [&native_pixels, &gles_pixels] {
                assert_eq!(&pixels[(y * 128 + x) * 4..(y * 128 + x) * 4 + 4], &[255; 4]);
            }
        }
    }
}

fn check_shader_damage_pixels(gles: &mut GlesRenderer) {
    use smithay::backend::renderer::element::RenderElement;
    let instanced = gles
        .capabilities()
        .contains(&smithay::backend::renderer::gles::Capability::Instancing);
    resources::init(gles);
    shaders::init(gles);
    let color = Color::new_unpremul(0.0, 1.0, 0.0, 0.5);
    let element = BorderRenderElement::new(
        (24.0, 8.0).into(),
        Rectangle::from_size((24.0, 8.0).into()),
        GradientInterpolation::default(),
        color,
        color,
        0.0,
        Rectangle::from_size((24.0, 8.0).into()),
        0.0,
        CornerRadius::default(),
        1.0,
        1.0,
    );
    let mut damage: Vec<_> = (0..3)
        .flat_map(|y| (0..11).map(move |x| Rectangle::new((2 * x, 2 * y).into(), (1, 1).into())))
        .collect();
    damage.extend([
        damage[0],
        damage[17],
        Rectangle::new((-5, -5).into(), (1, 1).into()),
    ]);
    let mut target = gles
        .create_buffer(Fourcc::Abgr8888, (24, 8).into())
        .unwrap();
    let mut framebuffer = gles.bind(&mut target).unwrap();
    let mut frame = gles
        .render(&mut framebuffer, (24, 8).into(), Transform::Normal)
        .unwrap();
    let full = Rectangle::from_size((24, 8).into());
    frame.clear(Color32F::TRANSPARENT, &[full]).unwrap();
    <BorderRenderElement as RenderElement<GlesRenderer>>::draw(
        &element,
        &mut frame,
        Rectangle::from_size((1.0, 1.0).into()),
        full,
        &damage,
        &[],
        None,
    )
    .unwrap();
    frame.finish().unwrap().wait().unwrap();
    drop(framebuffer);
    let map = gles
        .copy_texture(
            &target,
            Rectangle::from_size((24, 8).into()),
            Fourcc::Abgr8888,
        )
        .unwrap();
    let pixels = gles.map_texture(&map).unwrap();
    for y in 0..8 {
        for x in 0..24 {
            let expected = if x % 2 == 0 && x <= 20 && y % 2 == 0 && y <= 4 {
                [0, 128, 0, 128]
            } else {
                [0; 4]
            };
            let actual = &pixels[(y * 24 + x) * 4..(y * 24 + x) * 4 + 4];
            assert!(
                actual.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 1),
                "instanced={instanced} at ({x},{y}): {actual:?} != {expected:?}"
            );
        }
    }
}

#[test]
#[ignore = "requires selected DRM node and GLES"]
fn shader_damage_is_disjoint_and_all_fallback_batches_reach_pixels() {
    let (_vk, reference, _) = setup();
    for instanced in [false, true] {
        let context = EGLContext::new(reference.egl_context().display()).unwrap();
        let mut gles = renderer_with_instancing(context, instanced);
        check_shader_damage_pixels(&mut gles);
    }
}

fn renderer_with_instancing(context: EGLContext, instanced: bool) -> GlesRenderer {
    use smithay::backend::renderer::gles::Capability;
    let capabilities = unsafe { GlesRenderer::supported_capabilities(&context) }
        .unwrap()
        .into_iter()
        .filter(|item| instanced || *item != Capability::Instancing)
        .collect::<Vec<_>>();
    let renderer = unsafe { GlesRenderer::with_capabilities(context, capabilities) }.unwrap();
    assert_eq!(
        renderer.capabilities().contains(&Capability::Instancing),
        instanced
    );
    renderer
}

#[test]
#[ignore = "requires selected DRM node and GLES"]
fn shared_contexts_can_alternate_instanced_and_fallback_geometry_in_both_orders() {
    let (_vk, reference, _) = setup();
    for first_instanced in [false, true] {
        let context = EGLContext::new(reference.egl_context().display()).unwrap();
        let mut first = renderer_with_instancing(context, first_instanced);
        check_shader_damage_pixels(&mut first);
        let context =
            EGLContext::new_shared(first.egl_context().display(), first.egl_context()).unwrap();
        let mut shared = renderer_with_instancing(context, !first_instanced);
        check_shader_damage_pixels(&mut shared);
        check_shader_damage_pixels(&mut first);
    }
}
