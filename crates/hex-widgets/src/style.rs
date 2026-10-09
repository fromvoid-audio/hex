/// Caller-owned visual tokens. `hex-widgets` never selects product colors.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ControlStyle {
    pub background: u32,
    pub surface: u32,
    pub raised: u32,
    pub rail: u32,
    pub border: u32,
    pub text: u32,
    pub muted: u32,
    pub accent: u32,
    pub hover: u32,
    pub font_size: f32,
}

impl ControlStyle {
    pub fn validate(self) -> Result<Self, &'static str> {
        if self.font_size.is_finite() && (6.0..=32.0).contains(&self.font_size) {
            Ok(self)
        } else {
            Err("control font size must be finite and between 6 and 32 pixels")
        }
    }
}
