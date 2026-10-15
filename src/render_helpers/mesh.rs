use std::rc::Rc;

use glam::{Mat3, Vec2};
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{
    ffi, link_program, Capability, GlesError, GlesFrame, GlesRenderer, GlesTexture,
};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::backend::renderer::Texture as _;
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Buffer, Logical, Physical, Rectangle, Scale, Size};

pub(super) use super::damage_region::disjoint_damage as disjoint_damage_rects;
use super::renderer::AsGlesFrame as _;
use super::resources::Resources;
use super::shaders::Shaders;
use super::vulkan::native::NativeRenderer;
use super::vulkan::pipeline::MeshGpuVertex;
use super::vulkan::VulkanTexture;
use crate::backend::tty::{TtyFrame, TtyRenderer, TtyRendererError};

pub use realand_core::deformation::MeshVertex;

const EMPTY_VERTEX: MeshVertex = MeshVertex {
    position: [0.; 2],
    uv: [0.; 2],
};

#[derive(Debug, Clone, Copy)]
struct ClipRect {
    min: [f32; 2],
    max: [f32; 2],
}

fn push_vertex(output: &mut Vec<f32>, vertex: MeshVertex) {
    output.extend_from_slice(&[
        vertex.position[0],
        vertex.position[1],
        vertex.uv[0],
        vertex.uv[1],
    ]);
}

fn intersection(from: MeshVertex, to: MeshVertex, axis: usize, boundary: f32) -> MeshVertex {
    let delta = to.position[axis] - from.position[axis];
    let t = if delta.abs() > f32::EPSILON {
        ((boundary - from.position[axis]) / delta).clamp(0., 1.)
    } else {
        0.
    };

    MeshVertex {
        position: [
            from.position[0] + (to.position[0] - from.position[0]) * t,
            from.position[1] + (to.position[1] - from.position[1]) * t,
        ],
        uv: [
            from.uv[0] + (to.uv[0] - from.uv[0]) * t,
            from.uv[1] + (to.uv[1] - from.uv[1]) * t,
        ],
    }
}

fn clip_edge(
    input: &[MeshVertex; 8],
    input_len: usize,
    output: &mut [MeshVertex; 8],
    axis: usize,
    boundary: f32,
    keep_greater: bool,
) -> usize {
    if input_len == 0 {
        return 0;
    }

    let inside = |vertex: MeshVertex| {
        if keep_greater {
            vertex.position[axis] >= boundary
        } else {
            vertex.position[axis] <= boundary
        }
    };

    let mut output_len = 0;
    let mut previous = input[input_len - 1];
    let mut previous_inside = inside(previous);
    for &current in &input[..input_len] {
        let current_inside = inside(current);
        if current_inside != previous_inside {
            debug_assert!(output_len < output.len());
            output[output_len] = intersection(previous, current, axis, boundary);
            output_len += 1;
        }
        if current_inside {
            debug_assert!(output_len < output.len());
            output[output_len] = current;
            output_len += 1;
        }
        previous = current;
        previous_inside = current_inside;
    }

    output_len
}

/// Append one triangle clipped to an element-local damage rectangle.
///
/// RenderElement damage may exclude areas covered by higher layers. Drawing the complete mesh for
/// every non-empty damage set would therefore both waste fill rate and paint through panels or
/// overlays that Smithay deliberately omitted from the current repaint.
fn append_clipped_triangle<T>(
    output: &mut Vec<T>,
    triangle: &[MeshVertex],
    clip: ClipRect,
    push: impl Fn(&mut Vec<T>, MeshVertex),
) {
    debug_assert_eq!(triangle.len(), 3);

    let mut first = [EMPTY_VERTEX; 8];
    first[..3].copy_from_slice(triangle);
    let mut second = [EMPTY_VERTEX; 8];
    let mut len = 3;

    for (axis, boundary, keep_greater) in [
        (0, clip.min[0], true),
        (0, clip.max[0], false),
        (1, clip.min[1], true),
        (1, clip.max[1], false),
    ] {
        len = clip_edge(&first, len, &mut second, axis, boundary, keep_greater);
        if len == 0 {
            return;
        }
        std::mem::swap(&mut first, &mut second);
    }

    for idx in 1..len - 1 {
        push(output, first[0]);
        push(output, first[idx]);
        push(output, first[idx + 1]);
    }
}

fn push_gpu_vertex(output: &mut Vec<MeshGpuVertex>, vertex: MeshVertex) {
    output.push(MeshGpuVertex {
        position: vertex.position,
        uv: vertex.uv,
    });
}

/// GLES2 program shared by elastic-window and workspace-cube meshes.
#[derive(Debug, Clone, Copy)]
pub struct MeshProgram {
    program: ffi::types::GLuint,
    uniform_matrix: ffi::types::GLint,
    uniform_tex: ffi::types::GLint,
    uniform_tex_scale: ffi::types::GLint,
    uniform_alpha: ffi::types::GLint,
    attrib_position: ffi::types::GLint,
    attrib_uv: ffi::types::GLint,
}

impl MeshProgram {
    pub fn compile(renderer: &mut GlesRenderer) -> Result<Self, GlesError> {
        renderer.with_context(|gl| unsafe {
            let program = link_program(
                gl,
                include_str!("shaders/mesh.vert"),
                include_str!("shaders/mesh.frag"),
            )?;

            Ok(Self {
                program,
                uniform_matrix: gl.GetUniformLocation(program, c"matrix".as_ptr()),
                uniform_tex: gl.GetUniformLocation(program, c"tex".as_ptr()),
                uniform_tex_scale: gl.GetUniformLocation(program, c"tex_scale".as_ptr()),
                uniform_alpha: gl.GetUniformLocation(program, c"alpha".as_ptr()),
                attrib_position: gl.GetAttribLocation(program, c"position".as_ptr()),
                attrib_uv: gl.GetAttribLocation(program, c"uv".as_ptr()),
            })
        })?
    }
}

#[derive(Debug, Clone)]
enum MeshTexture {
    Gles(GlesTexture),
    Vulkan(VulkanTexture),
}

/// A generic texture element whose geometry is supplied as arbitrary triangles.
///
/// The texture remains on the primary GPU. This makes the element usable through
/// both the direct GLES renderer and the multi-GPU TTY renderer.
#[derive(Debug, Clone)]
pub struct MeshRenderElement {
    id: Id,
    commit: CommitCounter,
    texture: MeshTexture,
    area: Rectangle<f64, Logical>,
    vertices: Rc<[MeshVertex]>,
    texture_scale: [f32; 2],
    alpha: f32,
    kind: Kind,
}

impl MeshRenderElement {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: Id,
        commit: CommitCounter,
        texture: GlesTexture,
        used_texture_size: Size<f64, Buffer>,
        area: Rectangle<f64, Logical>,
        vertices: impl Into<Rc<[MeshVertex]>>,
        alpha: f32,
        kind: Kind,
    ) -> Self {
        let texture_size = texture.size().to_f64();
        let texture_scale = [
            (used_texture_size.w / texture_size.w) as f32,
            (used_texture_size.h / texture_size.h) as f32,
        ];

        Self {
            id,
            commit,
            texture: MeshTexture::Gles(texture),
            area,
            vertices: vertices.into(),
            texture_scale,
            alpha,
            kind,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_vulkan(
        id: Id,
        commit: CommitCounter,
        texture: VulkanTexture,
        used_texture_size: Size<f64, Buffer>,
        area: Rectangle<f64, Logical>,
        vertices: impl Into<Rc<[MeshVertex]>>,
        alpha: f32,
        kind: Kind,
    ) -> Self {
        let texture_size: Size<f64, Buffer> =
            Size::from((f64::from(texture.width()), f64::from(texture.height())));
        let texture_scale = [
            (used_texture_size.w / texture_size.w) as f32,
            (used_texture_size.h / texture_size.h) as f32,
        ];

        Self {
            id,
            commit,
            texture: MeshTexture::Vulkan(texture),
            area,
            vertices: vertices.into(),
            texture_scale,
            alpha,
            kind,
        }
    }

    pub fn is_available(renderer: &mut GlesRenderer) -> bool {
        Shaders::get(renderer).mesh.is_some() && Resources::is_available(renderer)
    }
}

impl Element for MeshRenderElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        Rectangle::from_size(Size::from((1., 1.)))
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.area.to_physical_precise_round(scale)
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        if commit == Some(self.commit) {
            DamageSet::default()
        } else {
            DamageSet::from_slice(&[Rectangle::from_size(self.geometry(scale).size)])
        }
    }

    fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        OpaqueRegions::default()
    }

    fn alpha(&self) -> f32 {
        self.alpha
    }

    fn kind(&self) -> Kind {
        self.kind
    }
}

impl RenderElement<GlesRenderer> for MeshRenderElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dest: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
        _cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        if damage.is_empty() || self.vertices.is_empty() {
            return Ok(());
        }
        let MeshTexture::Gles(texture) = &self.texture else {
            return Ok(());
        };

        let frame = frame.as_gles_frame();
        let supports_instancing = frame.capabilities().contains(&Capability::Instancing);
        let Some(program) = Shaders::get_from_frame(frame).mesh else {
            return Ok(());
        };
        let Some(resources) = Resources::get(frame) else {
            return Ok(());
        };
        let mut resources = resources.borrow_mut();
        resources.vertices.clear();
        if dest.size.w <= 0 || dest.size.h <= 0 {
            return Ok(());
        }

        let source_min = [src.loc.x as f32, src.loc.y as f32];
        let source_size = [src.size.w as f32, src.size.h as f32];
        if source_size[0] <= 0. || source_size[1] <= 0. {
            return Ok(());
        }
        let source_max = [
            source_min[0] + source_size[0],
            source_min[1] + source_size[1],
        ];
        let source_clip = ClipRect {
            min: source_min,
            max: source_max,
        };
        let source_is_full = source_min == [0., 0.] && source_max == [1., 1.];

        let damage_covers_dest = damage.iter().any(|rect| {
            rect.loc.x <= 0
                && rect.loc.y <= 0
                && rect.loc.x.saturating_add(rect.size.w) >= dest.size.w
                && rect.loc.y.saturating_add(rect.size.h) >= dest.size.h
        });
        if damage_covers_dest && source_is_full {
            resources.vertices.reserve(self.vertices.len() * 4);
            for &vertex in self.vertices.iter() {
                push_vertex(&mut resources.vertices, vertex);
            }
        } else {
            let width = dest.size.w as f32;
            let height = dest.size.h as f32;
            let append_damage = |rect: Rectangle<i32, Physical>, output: &mut Vec<f32>| {
                let dest_min_x = rect.loc.x.clamp(0, dest.size.w) as f32 / width;
                let dest_min_y = rect.loc.y.clamp(0, dest.size.h) as f32 / height;
                let dest_max_x =
                    rect.loc.x.saturating_add(rect.size.w).clamp(0, dest.size.w) as f32 / width;
                let dest_max_y =
                    rect.loc.y.saturating_add(rect.size.h).clamp(0, dest.size.h) as f32 / height;
                if dest_min_x >= dest_max_x || dest_min_y >= dest_max_y {
                    return;
                }

                // Damage is relative to the cropped destination. Convert it back to the original
                // unit-square element coordinates before clipping triangles.
                let clip = ClipRect {
                    min: [
                        source_min[0] + dest_min_x * source_size[0],
                        source_min[1] + dest_min_y * source_size[1],
                    ],
                    max: [
                        source_min[0] + dest_max_x * source_size[0],
                        source_min[1] + dest_max_y * source_size[1],
                    ],
                };
                for triangle in self.vertices.as_chunks::<3>().0 {
                    append_clipped_triangle(output, triangle, clip, push_vertex);
                }
            };

            if damage_covers_dest {
                for triangle in self.vertices.as_chunks::<3>().0 {
                    append_clipped_triangle(
                        &mut resources.vertices,
                        triangle,
                        source_clip,
                        push_vertex,
                    );
                }
            } else {
                for rect in disjoint_damage_rects(damage, dest.size) {
                    append_damage(rect, &mut resources.vertices);
                }
            }
        }

        if resources.vertices.is_empty() {
            return Ok(());
        }

        let vertex_count = (resources.vertices.len() / 4) as i32;

        for vertex in resources.vertices.as_chunks_mut::<4>().0 {
            vertex[0] = (vertex[0] - source_min[0]) / source_size[0] * dest.size.w as f32;
            vertex[1] = (vertex[1] - source_min[1]) / source_size[1] * dest.size.h as f32;
        }

        let matrix = Mat3::from_cols_array(frame.projection())
            * Mat3::from_translation(Vec2::new(dest.loc.x as f32, dest.loc.y as f32));

        frame.with_profiled_context(
            smithay::gpu_span_location!("draw texture mesh"),
            |gl| -> Result<(), GlesError> {
                unsafe {
                    gl.ActiveTexture(ffi::TEXTURE0);
                    gl.BindTexture(ffi::TEXTURE_2D, texture.tex_id());
                    gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MIN_FILTER, ffi::LINEAR as i32);
                    gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MAG_FILTER, ffi::LINEAR as i32);
                    gl.TexParameteri(
                        ffi::TEXTURE_2D,
                        ffi::TEXTURE_WRAP_S,
                        ffi::CLAMP_TO_EDGE as i32,
                    );
                    gl.TexParameteri(
                        ffi::TEXTURE_2D,
                        ffi::TEXTURE_WRAP_T,
                        ffi::CLAMP_TO_EDGE as i32,
                    );

                    gl.UseProgram(program.program);
                    gl.Uniform1i(program.uniform_tex, 0);
                    gl.Uniform2f(
                        program.uniform_tex_scale,
                        self.texture_scale[0],
                        self.texture_scale[1],
                    );
                    gl.Uniform1f(program.uniform_alpha, self.alpha);
                    gl.UniformMatrix3fv(
                        program.uniform_matrix,
                        1,
                        ffi::FALSE,
                        matrix.as_ref().as_ptr(),
                    );

                    gl.BindBuffer(ffi::ARRAY_BUFFER, resources.vbos[1]);
                    gl.BufferData(
                        ffi::ARRAY_BUFFER,
                        (resources.vertices.len() * std::mem::size_of::<f32>()) as isize,
                        resources.vertices.as_ptr().cast(),
                        ffi::STREAM_DRAW,
                    );

                    let stride = (4 * std::mem::size_of::<f32>()) as i32;
                    gl.EnableVertexAttribArray(program.attrib_position as u32);
                    if supports_instancing {
                        // Smithay's regular texture path leaves its per-instance attribute at a
                        // divisor of one. Shader attribute locations are program-local, so that
                        // same slot can be our UV input here. Reset both inputs or every vertex
                        // samples one texel and the whole mesh becomes a solid color.
                        gl.VertexAttribDivisor(program.attrib_position as u32, 0);
                        gl.VertexAttribDivisor(program.attrib_uv as u32, 0);
                    }
                    gl.VertexAttribPointer(
                        program.attrib_position as u32,
                        2,
                        ffi::FLOAT,
                        ffi::FALSE,
                        stride,
                        std::ptr::null(),
                    );
                    gl.EnableVertexAttribArray(program.attrib_uv as u32);
                    gl.VertexAttribPointer(
                        program.attrib_uv as u32,
                        2,
                        ffi::FLOAT,
                        ffi::FALSE,
                        stride,
                        (2 * std::mem::size_of::<f32>()) as *const _,
                    );

                    gl.DrawArrays(ffi::TRIANGLES, 0, vertex_count);

                    gl.DisableVertexAttribArray(program.attrib_position as u32);
                    gl.DisableVertexAttribArray(program.attrib_uv as u32);
                    gl.BindBuffer(ffi::ARRAY_BUFFER, 0);
                    gl.BindTexture(ffi::TEXTURE_2D, 0);
                }
                Ok(())
            },
        )??;

        Ok(())
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

impl<'render> RenderElement<TtyRenderer<'render>> for MeshRenderElement {
    fn draw(
        &self,
        frame: &mut TtyFrame<'render, '_, '_>,
        src: Rectangle<f64, Buffer>,
        dest: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), TtyRendererError<'render>> {
        RenderElement::<GlesRenderer>::draw(
            self,
            frame.as_gles_frame(),
            src,
            dest,
            damage,
            opaque_regions,
            cache,
        )?;
        Ok(())
    }

    fn underlying_storage(
        &self,
        _renderer: &mut TtyRenderer<'render>,
    ) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

impl smithay::backend::renderer::element::RenderElement<NativeRenderer> for MeshRenderElement {
    fn draw(
        &self,
        frame: &mut crate::render_helpers::vulkan::native::NativeFrame<'_>,
        src: Rectangle<f64, Buffer>,
        dest: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
        _cache: Option<&UserDataMap>,
    ) -> Result<(), crate::render_helpers::vulkan::native::NativeError> {
        if damage.is_empty() || self.vertices.is_empty() || dest.size.w <= 0 || dest.size.h <= 0 {
            return Ok(());
        }
        let MeshTexture::Vulkan(texture) = &self.texture else {
            return Ok(());
        };

        let source_min = [src.loc.x as f32, src.loc.y as f32];
        let source_size = [src.size.w as f32, src.size.h as f32];
        if source_size[0] <= 0. || source_size[1] <= 0. {
            return Ok(());
        }
        let source_max = [
            source_min[0] + source_size[0],
            source_min[1] + source_size[1],
        ];
        let source_clip = ClipRect {
            min: source_min,
            max: source_max,
        };
        let source_is_full = source_min == [0., 0.] && source_max == [1., 1.];

        let damage_covers_dest = damage.iter().any(|rect| {
            rect.loc.x <= 0
                && rect.loc.y <= 0
                && rect.loc.x.saturating_add(rect.size.w) >= dest.size.w
                && rect.loc.y.saturating_add(rect.size.h) >= dest.size.h
        });

        let mut vertices = Vec::new();
        if damage_covers_dest && source_is_full {
            vertices.reserve(self.vertices.len());
            for &vertex in self.vertices.iter() {
                push_gpu_vertex(&mut vertices, vertex);
            }
        } else {
            let width = dest.size.w as f32;
            let height = dest.size.h as f32;
            let append_damage = |rect: Rectangle<i32, Physical>,
                                 output: &mut Vec<MeshGpuVertex>| {
                let dest_min_x = rect.loc.x.clamp(0, dest.size.w) as f32 / width;
                let dest_min_y = rect.loc.y.clamp(0, dest.size.h) as f32 / height;
                let dest_max_x =
                    rect.loc.x.saturating_add(rect.size.w).clamp(0, dest.size.w) as f32 / width;
                let dest_max_y =
                    rect.loc.y.saturating_add(rect.size.h).clamp(0, dest.size.h) as f32 / height;
                if dest_min_x >= dest_max_x || dest_min_y >= dest_max_y {
                    return;
                }
                let clip = ClipRect {
                    min: [
                        source_min[0] + dest_min_x * source_size[0],
                        source_min[1] + dest_min_y * source_size[1],
                    ],
                    max: [
                        source_min[0] + dest_max_x * source_size[0],
                        source_min[1] + dest_max_y * source_size[1],
                    ],
                };
                for triangle in self.vertices.as_chunks::<3>().0 {
                    append_clipped_triangle(output, triangle, clip, push_gpu_vertex);
                }
            };

            if damage_covers_dest {
                for triangle in self.vertices.as_chunks::<3>().0 {
                    append_clipped_triangle(&mut vertices, triangle, source_clip, push_gpu_vertex);
                }
            } else {
                for rect in disjoint_damage_rects(damage, dest.size) {
                    append_damage(rect, &mut vertices);
                }
            }
        }

        if vertices.is_empty() {
            return Ok(());
        }

        for vertex in &mut vertices {
            vertex.position[0] = (vertex.position[0] - source_min[0]) / source_size[0];
            vertex.position[1] = (vertex.position[1] - source_min[1]) / source_size[1];
        }

        frame.draw_mesh(
            texture,
            dest,
            damage,
            vertices,
            self.texture_scale,
            self.alpha,
        )
    }

    fn underlying_storage(&self, _renderer: &mut NativeRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_damage_is_split_before_blended_mesh_draw() {
        let damage: [Rectangle<i32, Physical>; 3] = [
            Rectangle::new((0, 0).into(), (70, 70).into()),
            Rectangle::new((30, 30).into(), (70, 70).into()),
            Rectangle::new((-10, 80).into(), (20, 20).into()),
        ];
        let disjoint = disjoint_damage_rects(&damage, (100, 100).into());

        for (index, rect) in disjoint.iter().enumerate() {
            assert!(Rectangle::from_size((100, 100).into()).contains_rect(*rect));
            for other in &disjoint[index + 1..] {
                assert!(rect.intersection(*other).is_none());
            }
        }

        // The third rectangle adds 10 by 20 pixels outside the first one.
        let area: i32 = disjoint.iter().map(|rect| rect.size.w * rect.size.h).sum();
        assert_eq!(area, 70 * 70 + 70 * 70 - 40 * 40 + 10 * 20);
    }

    #[test]
    fn clipped_mesh_stays_inside_damage_and_interpolates_uv() {
        let triangle = [
            MeshVertex {
                position: [0., 0.],
                uv: [0., 0.],
            },
            MeshVertex {
                position: [0., 1.],
                uv: [0., 1.],
            },
            MeshVertex {
                position: [1., 1.],
                uv: [1., 1.],
            },
        ];
        let mut output = Vec::new();
        append_clipped_triangle(
            &mut output,
            &triangle,
            ClipRect {
                min: [0.25, 0.25],
                max: [0.75, 0.75],
            },
            push_vertex,
        );

        assert!(!output.is_empty());
        assert_eq!(output.len() % 12, 0);
        for vertex in output.as_chunks::<4>().0 {
            assert!((0.25..=0.75).contains(&vertex[0]));
            assert!((0.25..=0.75).contains(&vertex[1]));
            assert_eq!(vertex[0], vertex[2]);
            assert_eq!(vertex[1], vertex[3]);
        }
    }

    #[test]
    fn source_crop_keeps_texture_coordinates_instead_of_squashing_the_mesh() {
        let triangles = [
            MeshVertex {
                position: [0., 0.],
                uv: [0., 0.],
            },
            MeshVertex {
                position: [0., 1.],
                uv: [0., 1.],
            },
            MeshVertex {
                position: [1., 1.],
                uv: [1., 1.],
            },
            MeshVertex {
                position: [0., 0.],
                uv: [0., 0.],
            },
            MeshVertex {
                position: [1., 1.],
                uv: [1., 1.],
            },
            MeshVertex {
                position: [1., 0.],
                uv: [1., 0.],
            },
        ];
        let source = ClipRect {
            min: [0.5, 0.],
            max: [1., 1.],
        };
        let mut output = Vec::new();
        for triangle in triangles.as_chunks::<3>().0 {
            append_clipped_triangle(&mut output, triangle, source, push_vertex);
        }

        assert!(!output.is_empty());
        for vertex in output.as_chunks_mut::<4>().0 {
            vertex[0] = (vertex[0] - source.min[0]) / (source.max[0] - source.min[0]);
            vertex[1] = (vertex[1] - source.min[1]) / (source.max[1] - source.min[1]);
            assert!((0.0..=1.0).contains(&vertex[0]));
            assert!((0.0..=1.0).contains(&vertex[1]));
            assert_eq!(vertex[2], 0.5 + vertex[0] * 0.5);
            assert_eq!(vertex[3], vertex[1]);
        }
    }
}
