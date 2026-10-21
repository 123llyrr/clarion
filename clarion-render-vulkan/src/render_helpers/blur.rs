//! Blur options for compositor effects.

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct BlurOptions {
    pub passes: u8,
    pub offset: f64,
}

impl BlurOptions {
    pub fn new(passes: u8, offset: f64) -> Self {
        Self { passes, offset }
    }
}
