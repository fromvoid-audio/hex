/// Semantic availability is separate from transient edit admission. Busy controls
/// retain their available appearance while refusing input and accessibility edits.
/// Numeric transitions to busy cancel an unsubmitted local draft, as disabling did;
/// callers must keep a control available while it owns an active gesture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ControlAvailability {
    #[default]
    Available,
    Busy,
    Unavailable,
}

impl ControlAvailability {
    pub fn for_edit(available: bool, busy: bool) -> Self {
        if !available {
            Self::Unavailable
        } else if busy {
            Self::Busy
        } else {
            Self::Available
        }
    }
    pub(crate) fn interactive(self) -> bool {
        self == Self::Available
    }
    pub(crate) fn painted_enabled(self) -> bool {
        self != Self::Unavailable
    }
}

/// Shared boolean-control corner radius, in logical pixels.
pub const TOGGLE_CORNER_RADIUS: f32 = 2.0;

/// Shared native device outline radius, in logical pixels.
/// All device frames inherit this token.
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
    /// Caller-resolved foreground for labels/icons painted directly on accent.
    pub accent_text: u32,
    /// Opaque numeric rail fill resolved by the owner to keep `text` readable
    /// over both filled and empty portions of a numeric field.
    pub numeric_fill: u32,
    /// Caller-resolved foreground on the hover surface.
    pub hover_text: u32,
    /// Neutral selected-control corner color, supplied by the application.
    pub focus_marker: u32,
    /// Full unfilled knob rail color, independent of the parameter value fill.
    pub dial_rail: u32,
    pub hover: u32,
    /// Opaque text-selection highlight behind `text` in editable fields.
    pub selection: u32,
    /// Outline for rejected/invalid typed values.
    pub invalid: u32,
    pub font_size: f32,
    /// Presentation only; source drafts, parser inputs and clipboard stay unchanged.
    pub text_case: crate::TextCase,
}

impl ControlStyle {
    /// Resolve header selection without changing the neutral frame/control palette.
    /// The owner retains selection, focus, bypass and drag/drop state separately.
    pub fn with_device_selection(self, selected: bool) -> DeviceFrameStyle {
        let frame = DeviceFrameStyle::from(self);
        if selected {
            frame.with_header_colors(self.accent, self.background)
        } else {
            frame
        }
    }

    pub fn validate(self) -> Result<Self, &'static str> {
        if self.font_size.is_finite() && (6.0..=32.0).contains(&self.font_size) {
            Ok(self)
        } else {
            Err("control font size must be finite and between 6 and 32 pixels")
        }
    }
}

/// Neutral shell colors and independently resolved header paint. A selected
/// header must never recolor the surround, outer border or caller-owned body.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeviceFrameStyle {
    pub(crate) controls: ControlStyle,
    pub(crate) header_background: u32,
    pub(crate) header_text: u32,
}

impl From<ControlStyle> for DeviceFrameStyle {
    fn from(controls: ControlStyle) -> Self {
        Self {
            header_background: controls.surface,
            header_text: controls.text,
            controls,
        }
    }
}

impl DeviceFrameStyle {
    /// Inject semantic header foreground/background colors, independently of the
    /// shell. The product can supply a contrast-checked selected foreground.
    pub fn with_header_colors(mut self, background: u32, text: u32) -> Self {
        self.header_background = background;
        self.header_text = text;
        self
    }
}

/// Caller-resolved text against both idle and hover backgrounds. No palette or
/// contrast policy lives in the widget library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionStyle {
    pub background: u32,
    pub text: u32,
    pub hover_background: u32,
    /// Override the foreground on hover when a contrast change is required.
    /// None preserves the caller's semantic foreground (for example active loop).
    pub hover_text: Option<u32>,
}

impl ActionStyle {
    /// Preserve a state indicator on hover instead of replacing its active colors.
    pub fn with_state_colors(mut self, background: u32, text: u32) -> Self {
        self.background = background;
        self.text = text;
        self.hover_background = background;
        self.hover_text = Some(text);
        self
    }
}
