//! Vertex definition for hardware-accelerated particle effects.

#[repr(C)]
#[derive(Copy, Clone, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
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
