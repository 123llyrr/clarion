use std::rc::Rc;

use glam::{Mat3, Vec2};
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{
    ffi, link_program, Capability, GlesError, GlesFrame, GlesRenderer,
};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Buffer, Logical, Physical, Rectangle, Scale};

use super::mesh::disjoint_damage_rects;
use super::renderer::AsGlesFrame as _;
use super::resources::Resources;
use super::shaders::Shaders;
use crate::backend::tty::{TtyFrame, TtyRenderer, TtyRendererError};

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ParticleVertex {
    /// Position relative to the element's bounding rectangle.
    pub position: [f32; 2],
    /// Position relative to the particle width. Rectangle y spans +/- aspect/2.
    pub local: [f32; 2],
    /// Premultiplied RGBA color.
    pub color: [f32; 4],
    /// One for a circle, zero for a rounded rectangle.
    pub shape: f32,
    /// One physical pixel expressed in particle-width units.
    pub inv_width: f32,
    /// Particle height divided by width.
    pub aspect: f32,
}

#[derive(Debug, Clone, Copy)]
struct ClipRect {
    min: [f32; 2],
    max: [f32; 2],
}

fn interpolate(from: ParticleVertex, to: ParticleVertex, t: f32) -> ParticleVertex {
    let lerp = |a: f32, b: f32| a + (b - a) * t;
    ParticleVertex {
        position: [
            lerp(from.position[0], to.position[0]),
            lerp(from.position[1], to.position[1]),
        ],
        local: [
            lerp(from.local[0], to.local[0]),
            lerp(from.local[1], to.local[1]),
        ],
        color: [
            lerp(from.color[0], to.color[0]),
            lerp(from.color[1], to.color[1]),
            lerp(from.color[2], to.color[2]),
            lerp(from.color[3], to.color[3]),
        ],
        shape: lerp(from.shape, to.shape),
        inv_width: lerp(from.inv_width, to.inv_width),
        aspect: lerp(from.aspect, to.aspect),
    }
}

fn intersection(
    from: ParticleVertex,
    to: ParticleVertex,
    axis: usize,
    boundary: f32,
) -> ParticleVertex {
    let delta = to.position[axis] - from.position[axis];
    let t = if delta.abs() > f32::EPSILON {
        ((boundary - from.position[axis]) / delta).clamp(0., 1.)
    } else {
        0.
    };
    interpolate(from, to, t)
}

fn clip_edge(
    input: &[ParticleVertex; 8],
    input_len: usize,
    output: &mut [ParticleVertex; 8],
    axis: usize,
    boundary: f32,
    keep_greater: bool,
) -> usize {
    if input_len == 0 {
        return 0;
    }

    let inside = |vertex: ParticleVertex| {
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
            output[output_len] = intersection(previous, current, axis, boundary);
            output_len += 1;
        }
        if current_inside {
            output[output_len] = current;
            output_len += 1;
        }
        previous = current;
        previous_inside = current_inside;
    }
    output_len
}

fn push_vertex(output: &mut Vec<f32>, vertex: ParticleVertex) {
    output.extend_from_slice(&[
        vertex.position[0],
        vertex.position[1],
        vertex.local[0],
        vertex.local[1],
        vertex.color[0],
        vertex.color[1],
        vertex.color[2],
        vertex.color[3],
        vertex.shape,
        vertex.inv_width,
        vertex.aspect,
    ]);
}

fn append_clipped_triangle(output: &mut Vec<f32>, triangle: &[ParticleVertex], clip: ClipRect) {
    debug_assert_eq!(triangle.len(), 3);
    // Fully contained triangles need no clipping. Preserve their attributes
    // directly without four polygon clipping passes.
    if triangle.iter().all(|vertex| {
        (0..2).all(|axis| {
            vertex.position[axis] >= clip.min[axis] && vertex.position[axis] <= clip.max[axis]
        })
    }) {
        for &vertex in triangle {
            push_vertex(output, vertex);
        }
        return;
    }
    let mut first = [ParticleVertex::default(); 8];
    first[..3].copy_from_slice(triangle);
    let mut second = [ParticleVertex::default(); 8];
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
        push_vertex(output, first[0]);
        push_vertex(output, first[idx]);
        push_vertex(output, first[idx + 1]);
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ParticleProgram {
    program: ffi::types::GLuint,
    uniform_matrix: ffi::types::GLint,
    attrib_position: ffi::types::GLint,
    attrib_local: ffi::types::GLint,
    attrib_color: ffi::types::GLint,
    attrib_shape: ffi::types::GLint,
    attrib_inv_width: ffi::types::GLint,
    attrib_aspect: ffi::types::GLint,
}

impl ParticleProgram {
    pub fn compile(renderer: &mut GlesRenderer) -> Result<Self, GlesError> {
        renderer.with_context(|gl| unsafe {
            let program = link_program(
                gl,
                include_str!("shaders/particles.vert"),
                include_str!("shaders/particles.frag"),
            )?;
            Ok(Self {
                program,
                uniform_matrix: gl.GetUniformLocation(program, c"matrix".as_ptr()),
                attrib_position: gl.GetAttribLocation(program, c"position".as_ptr()),
                attrib_local: gl.GetAttribLocation(program, c"local".as_ptr()),
                attrib_color: gl.GetAttribLocation(program, c"color".as_ptr()),
                attrib_shape: gl.GetAttribLocation(program, c"shape".as_ptr()),
                attrib_inv_width: gl.GetAttribLocation(program, c"inv_width".as_ptr()),
                attrib_aspect: gl.GetAttribLocation(program, c"aspect".as_ptr()),
            })
        })?
    }
}

#[derive(Debug, Clone)]
pub struct ParticleRenderElement {
    id: Id,
    commit: CommitCounter,
    area: Rectangle<f64, Logical>,
    vertices: Rc<[ParticleVertex]>,
    kind: Kind,
}

impl ParticleRenderElement {
    pub fn new(
        id: Id,
        commit: CommitCounter,
        area: Rectangle<f64, Logical>,
        vertices: Vec<ParticleVertex>,
        kind: Kind,
    ) -> Self {
        Self {
            id,
            commit,
            area,
            vertices: vertices.into(),
            kind,
        }
    }

    pub fn is_available(renderer: &mut GlesRenderer) -> bool {
        Shaders::get(renderer).particles.is_some() && Resources::is_available(renderer)
    }
}

impl Element for ParticleRenderElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        Rectangle::from_size((1., 1.).into())
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
        1.
    }

    fn kind(&self) -> Kind {
        self.kind
    }
}

impl RenderElement<GlesRenderer> for ParticleRenderElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        _src: Rectangle<f64, Buffer>,
        dest: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
        _cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        if damage.is_empty() || self.vertices.is_empty() || dest.size.w <= 0 || dest.size.h <= 0 {
            return Ok(());
        }

        let frame = frame.as_gles_frame();
        let supports_instancing = frame.capabilities().contains(&Capability::Instancing);
        let Some(program) = Shaders::get_from_frame(frame).particles else {
            return Ok(());
        };
        let Some(resources) = Resources::get(frame) else {
            return Ok(());
        };
        let mut resources = resources.borrow_mut();
        resources.vertices.clear();

        let width = dest.size.w as f32;
        let height = dest.size.h as f32;
        let damage_covers_dest = damage.iter().any(|rect| {
            rect.loc.x <= 0
                && rect.loc.y <= 0
                && rect.loc.x.saturating_add(rect.size.w) >= dest.size.w
                && rect.loc.y.saturating_add(rect.size.h) >= dest.size.h
        });
        if damage_covers_dest {
            resources.vertices.reserve(self.vertices.len() * 11);
            for &vertex in self.vertices.iter() {
                push_vertex(&mut resources.vertices, vertex);
            }
        } else {
            for rect in disjoint_damage_rects(damage, dest.size) {
                let min_x = rect.loc.x.clamp(0, dest.size.w) as f32 / width;
                let min_y = rect.loc.y.clamp(0, dest.size.h) as f32 / height;
                let max_x =
                    rect.loc.x.saturating_add(rect.size.w).clamp(0, dest.size.w) as f32 / width;
                let max_y =
                    rect.loc.y.saturating_add(rect.size.h).clamp(0, dest.size.h) as f32 / height;
                if min_x >= max_x || min_y >= max_y {
                    continue;
                }
                let clip = ClipRect {
                    min: [min_x, min_y],
                    max: [max_x, max_y],
                };
                for triangle in self.vertices.as_chunks::<3>().0 {
                    append_clipped_triangle(&mut resources.vertices, triangle, clip);
                }
            }
        }
        if resources.vertices.is_empty() {
            return Ok(());
        }

        for vertex in resources.vertices.as_chunks_mut::<11>().0 {
            vertex[0] *= width;
            vertex[1] *= height;
        }
        let vertex_count = (resources.vertices.len() / 11) as i32;
        let matrix = Mat3::from_cols_array(frame.projection())
            * Mat3::from_translation(Vec2::new(dest.loc.x as f32, dest.loc.y as f32));

        frame.with_profiled_context(
            smithay::gpu_span_location!("draw denial particles"),
            |gl| -> Result<(), GlesError> {
                unsafe {
                    gl.UseProgram(program.program);
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

                    let stride = (11 * std::mem::size_of::<f32>()) as i32;
                    for attrib in [
                        program.attrib_position,
                        program.attrib_local,
                        program.attrib_color,
                        program.attrib_shape,
                        program.attrib_inv_width,
                        program.attrib_aspect,
                    ] {
                        gl.EnableVertexAttribArray(attrib as u32);
                        if supports_instancing {
                            // Attribute divisors are shared GL context state, not shader state.
                            // Clear divisors left behind by Smithay's instanced quad renderer.
                            gl.VertexAttribDivisor(attrib as u32, 0);
                        }
                    }
                    gl.VertexAttribPointer(
                        program.attrib_position as u32,
                        2,
                        ffi::FLOAT,
                        ffi::FALSE,
                        stride,
                        std::ptr::null(),
                    );
                    gl.VertexAttribPointer(
                        program.attrib_local as u32,
                        2,
                        ffi::FLOAT,
                        ffi::FALSE,
                        stride,
                        (2 * std::mem::size_of::<f32>()) as *const _,
                    );
                    gl.VertexAttribPointer(
                        program.attrib_color as u32,
                        4,
                        ffi::FLOAT,
                        ffi::FALSE,
                        stride,
                        (4 * std::mem::size_of::<f32>()) as *const _,
                    );
                    gl.VertexAttribPointer(
                        program.attrib_shape as u32,
                        1,
                        ffi::FLOAT,
                        ffi::FALSE,
                        stride,
                        (8 * std::mem::size_of::<f32>()) as *const _,
                    );
                    gl.VertexAttribPointer(
                        program.attrib_inv_width as u32,
                        1,
                        ffi::FLOAT,
                        ffi::FALSE,
                        stride,
                        (9 * std::mem::size_of::<f32>()) as *const _,
                    );
                    gl.VertexAttribPointer(
                        program.attrib_aspect as u32,
                        1,
                        ffi::FLOAT,
                        ffi::FALSE,
                        stride,
                        (10 * std::mem::size_of::<f32>()) as *const _,
                    );
                    gl.DrawArrays(ffi::TRIANGLES, 0, vertex_count);
                    for attrib in [
                        program.attrib_position,
                        program.attrib_local,
                        program.attrib_color,
                        program.attrib_shape,
                        program.attrib_inv_width,
                        program.attrib_aspect,
                    ] {
                        gl.DisableVertexAttribArray(attrib as u32);
                    }
                    gl.BindBuffer(ffi::ARRAY_BUFFER, 0);
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

impl<'render> RenderElement<TtyRenderer<'render>> for ParticleRenderElement {
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

impl
    smithay::backend::renderer::element::RenderElement<
        crate::render_helpers::vulkan::native::NativeRenderer,
    > for ParticleRenderElement
{
    fn draw(
        &self,
        frame: &mut crate::render_helpers::vulkan::native::NativeFrame<'_>,
        _src: Rectangle<f64, Buffer>,
        dest: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
        _cache: Option<&UserDataMap>,
    ) -> Result<(), crate::render_helpers::vulkan::native::NativeError> {
        if damage.is_empty() || self.vertices.is_empty() || dest.size.w <= 0 || dest.size.h <= 0 {
            return Ok(());
        }
        frame.draw_particles(dest, damage, self.vertices.clone())
    }

    fn underlying_storage(
        &self,
        _renderer: &mut crate::render_helpers::vulkan::native::NativeRenderer,
    ) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn damage_is_local_at_nonzero_and_negative_positions() {
        for position in [(160.25, 120.75), (-80.5, -30.25)] {
            for scale in [1., 1.25, 2.] {
                let element = ParticleRenderElement::new(
                    Id::new(),
                    CommitCounter::default(),
                    Rectangle::new(position.into(), (100.5, 75.25).into()),
                    Vec::new(),
                    Kind::Unspecified,
                );
                let scale = Scale::from(scale);
                let expected = Rectangle::from_size(element.geometry(scale).size);
                assert_eq!(&*element.damage_since(scale, None), &[expected]);
                assert!(element
                    .damage_since(scale, Some(element.current_commit()))
                    .is_empty());
                let mut previous = element.current_commit();
                previous.increment();
                assert_eq!(&*element.damage_since(scale, Some(previous)), &[expected]);
            }
        }
    }

    #[test]
    fn contained_triangle_preserves_all_attributes_and_existing_output() {
        let triangle = [[0.25, 0.25], [0.75, 0.25], [0.5, 0.75]].map(|position| ParticleVertex {
            position,
            local: [0.2, 0.8],
            color: [0.1, 0.2, 0.3, 0.4],
            shape: 1.,
            inv_width: 0.125,
            aspect: 2.,
        });
        let mut expected = vec![42.];
        for vertex in triangle {
            push_vertex(&mut expected, vertex);
        }
        let mut output = vec![42.];
        append_clipped_triangle(
            &mut output,
            &triangle,
            ClipRect {
                min: [0.25, 0.25],
                max: [0.75, 0.75],
            },
        );
        assert_eq!(output, expected);
    }

    #[test]
    fn clipping_keeps_particle_vertices_inside_damage() {
        let vertex = |x, y| ParticleVertex {
            position: [x, y],
            ..ParticleVertex::default()
        };
        let triangle = [vertex(0., 0.), vertex(0., 1.), vertex(1., 1.)];
        let mut output = Vec::new();
        append_clipped_triangle(
            &mut output,
            &triangle,
            ClipRect {
                min: [0.25, 0.25],
                max: [0.75, 0.75],
            },
        );
        assert!(!output.is_empty());
        assert_eq!(output.len() % 33, 0);
        for vertex in output.as_chunks::<11>().0 {
            assert!((0.25..=0.75).contains(&vertex[0]));
            assert!((0.25..=0.75).contains(&vertex[1]));
        }
    }
}
