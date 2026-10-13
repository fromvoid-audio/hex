/// Shared native device outline radius, in logical pixels.
/// Standard and compact frames both inherit this token.
pub const DEVICE_CORNER_RADIUS: f32 = 4.0;

/// Shared numeric readout corner radius, in logical pixels.
pub const CONTROL_CORNER_RADIUS: f32 = 3.0;

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
    /// Neutral selected-control corner color, supplied by the application.
    pub focus_marker: u32,
    /// Full unfilled knob rail color, independent of the parameter value fill.
    pub dial_rail: u32,
    pub hover: u32,
    pub font_size: f32,
}

impl ControlStyle {
    /// Highlight a selected device's shared frame border and title bar.
    /// Apply this to frame rendering only, keeping parameter control colors stable.
    pub fn with_device_selection(mut self, selected: bool) -> Self {
        if selected {
            self.border = self.accent;
            self.raised = self.accent;
            self.text = self.background;
        }
        self
    }

    pub fn validate(self) -> Result<Self, &'static str> {
        if self.font_size.is_finite() && (6.0..=32.0).contains(&self.font_size) {
            Ok(self)
        } else {
            Err("control font size must be finite and between 6 and 32 pixels")
        }
    }
}
