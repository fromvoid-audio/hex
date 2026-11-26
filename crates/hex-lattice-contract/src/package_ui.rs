//! Declarative, bounded, versioned UI panel schema and read-only presentation
//! snapshot contracts for installable Hex-native device packages (`#2994`).
//!
//! # Purpose
//! Describe native GPUI control layouts (scalar dials/sliders/inline values,
//! toggles, selectors, compact sections, conditional visibility, and read-only
//! meter/status readouts) purely as validated data with independent `ui_version`
//! and explicit capability negotiation.
//!
//! # Ownership & Safety Contract
//! - Packages supply declarative schema data only — never executable code,
//!   JavaScript, foreign UI handles, or per-device type-ID switches.
//! - Unknown `ui_version` values, unknown capability bits, unknown widget kinds,
//!   or parameter-kind mismatches are refused explicitly so the host can fall
//!   back to functional generic descriptor controls (never decorative stubs).
//! - Runtime presentation snapshots ([`PackagePresentationSnapshot`]) carry
//!   read-only bounded telemetry (meters, voice count, status text) strictly
//!   separate from authored [`crate::NativeDeviceState`] and Undo history.

use crate::{ParameterDescriptor, ParameterId, ParameterKind, ParameterValue};
use std::fmt;

/// Current declarative package UI schema version.
pub const PACKAGE_UI_VERSION_V1: u32 = 1;
/// Hard byte cap for serialized declarative package UI payloads.
pub const MAX_PACKAGE_UI_JSON_BYTES: usize = 16_384;
/// Maximum layout sections in one package panel.
pub const MAX_PACKAGE_UI_SECTIONS: usize = 8;
/// Maximum total items (controls + meters) across all sections in one panel.
pub const MAX_PACKAGE_UI_ITEMS: usize = 64;
/// Maximum read-only meter widgets in one panel.
pub const MAX_PACKAGE_UI_METERS: usize = 8;
/// Maximum UTF-8 bytes in a section heading or control label.
pub const MAX_PACKAGE_UI_LABEL_BYTES: usize = 64;
/// Maximum UTF-8 bytes in a control tooltip.
pub const MAX_PACKAGE_UI_TOOLTIP_BYTES: usize = 160;
/// Maximum UTF-8 bytes in a runtime presentation status badge.
pub const MAX_PACKAGE_UI_STATUS_BYTES: usize = 64;
/// Minimum compact device panel body width in logical pixels.
pub const MIN_PACKAGE_UI_FRAME_WIDTH: f32 = 136.0;
/// Maximum compact device panel body width in logical pixels.
pub const MAX_PACKAGE_UI_FRAME_WIDTH: f32 = 640.0;

/// UI capability flags negotiated independently of DSP ABI capabilities.
pub const UI_CAP_SCALAR: u64 = 1 << 0;
pub const UI_CAP_TOGGLE: u64 = 1 << 1;
pub const UI_CAP_SELECTOR: u64 = 1 << 2;
pub const UI_CAP_METER: u64 = 1 << 3;
pub const UI_CAP_SECTIONS: u64 = 1 << 4;
pub const UI_CAP_CONDITIONAL_VISIBILITY: u64 = 1 << 5;
/// Bitmask of all UI capabilities supported by v1 generic native hosts.
pub const UI_CAP_KNOWN_MASK_V1: u64 = UI_CAP_SCALAR
    | UI_CAP_TOGGLE
    | UI_CAP_SELECTOR
    | UI_CAP_METER
    | UI_CAP_SECTIONS
    | UI_CAP_CONDITIONAL_VISIBILITY;

/// Explicit refusal reasons for malformed or unsupported package UI schemas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackageUiError {
    UnsupportedUiVersion,
    UnsupportedCapability,
    UnknownWidgetKind,
    InvalidBounds,
    InvalidText,
    EmptySections,
    TooManySections,
    TooManyItems,
    TooManyMeters,
    UnknownParameter,
    IncompatibleWidgetKind,
    MissingCapability,
    DuplicateParameterBinding,
    InvalidVisibilityRule,
    InvalidSnapshot,
}

impl fmt::Display for PackageUiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UnsupportedUiVersion => "unsupported package ui schema version",
            Self::UnsupportedCapability => "package ui requires an unsupported host capability",
            Self::UnknownWidgetKind => "package ui declares an unknown widget kind",
            Self::InvalidBounds => "package ui geometry bounds are invalid",
            Self::InvalidText => "package ui text is empty, oversized, or contains control characters",
            Self::EmptySections => "package ui declares no sections",
            Self::TooManySections => "package ui exceeds maximum section count",
            Self::TooManyItems => "package ui exceeds maximum item count",
            Self::TooManyMeters => "package ui exceeds maximum meter count",
            Self::UnknownParameter => "package ui references an unknown parameter id",
            Self::IncompatibleWidgetKind => "package ui widget kind does not match parameter kind",
            Self::MissingCapability => "package ui uses a widget without declaring its capability",
            Self::DuplicateParameterBinding => {
                "package ui binds the same parameter multiple times without mutually exclusive visibility"
            }
            Self::InvalidVisibilityRule => "package ui visibility rule is invalid",
            Self::InvalidSnapshot => "package presentation snapshot is invalid or out of bounds",
        })
    }
}

impl std::error::Error for PackageUiError {}

/// Native GPUI widget primitives addressable by a declarative package schema.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackageUiWidgetKind {
    Dial,
    MicroDial,
    Slider,
    FilledSlider,
    InlineValue,
    ToggleButton,
    ToggleSwitch,
    SelectorMenu,
    SelectorSegments { columns: u8 },
    ChoiceDial,
}

impl PackageUiWidgetKind {
    /// Capability bit required to render this widget kind.
    pub const fn required_capability(self) -> u64 {
        match self {
            Self::Dial
            | Self::MicroDial
            | Self::Slider
            | Self::FilledSlider
            | Self::InlineValue => UI_CAP_SCALAR,
            Self::ToggleButton | Self::ToggleSwitch => UI_CAP_TOGGLE,
            Self::SelectorMenu | Self::SelectorSegments { .. } | Self::ChoiceDial => {
                UI_CAP_SELECTOR
            }
        }
    }

    /// Whether this widget kind matches the parameter's canonical value kind.
    pub const fn matches_kind(self, kind: ParameterKind) -> bool {
        matches!(
            (self, kind),
            (
                Self::Dial
                    | Self::MicroDial
                    | Self::Slider
                    | Self::FilledSlider
                    | Self::InlineValue,
                ParameterKind::Float { .. },
            ) | (Self::ToggleButton | Self::ToggleSwitch, ParameterKind::Bool)
                | (
                    Self::SelectorMenu | Self::SelectorSegments { .. } | Self::ChoiceDial,
                    ParameterKind::Choice { .. },
                )
        )
    }
}

/// Optional host-owned numeric display/parser codec for scalar controls.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PackageNumericFormat {
    #[default]
    Default,
    Percent,
    AmplitudeDb,
    Hertz,
    Balance,
    MidSide,
    Seconds,
    Ratio,
    Number,
}

/// Declarative visibility predicate keyed to another canonical parameter value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PackageVisibilityRule {
    pub parameter_id: ParameterId,
    pub expected: ParameterValue,
}

/// One declarative parameter control entry inside a [`PackageUiSection`].
#[derive(Clone, Debug, PartialEq)]
pub struct PackageUiControl {
    pub parameter_id: ParameterId,
    pub widget: PackageUiWidgetKind,
    pub label: Option<String>,
    pub tooltip: Option<String>,
    pub display_format: PackageNumericFormat,
    pub show_when: Option<PackageVisibilityRule>,
}

/// One read-only meter indicator bound to a [`PackageMeterSample::meter_id`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageUiMeter {
    pub meter_id: u32,
    pub label: String,
    pub stereo: bool,
}

/// One item inside a [`PackageUiSection`].
#[derive(Clone, Debug, PartialEq)]
pub enum PackageUiItem {
    Control(PackageUiControl),
    Meter(PackageUiMeter),
}

/// Compact vertical/grid section inside a [`PackageUiSchema`].
#[derive(Clone, Debug, PartialEq)]
pub struct PackageUiSection {
    pub heading: String,
    pub columns: u8,
    pub items: Vec<PackageUiItem>,
}

/// Bounded declarative panel layout for an installed native device package.
#[derive(Clone, Debug, PartialEq)]
pub struct PackageUiSchema {
    pub ui_version: u32,
    pub required_capabilities: u64,
    pub frame_width: f32,
    pub sections: Vec<PackageUiSection>,
}

impl PackageUiSchema {
    /// Validate schema version, capability bits, geometry bounds, text bounds,
    /// parameter bindings, widget-kind compatibility, and visibility rules
    /// against the device's canonical [`ParameterDescriptor`] list.
    pub fn validate(&self, descriptors: &[ParameterDescriptor]) -> Result<(), PackageUiError> {
        if self.ui_version != PACKAGE_UI_VERSION_V1 {
            return Err(PackageUiError::UnsupportedUiVersion);
        }
        if (self.required_capabilities & !UI_CAP_KNOWN_MASK_V1) != 0 {
            return Err(PackageUiError::UnsupportedCapability);
        }
        if !self.frame_width.is_finite()
            || self.frame_width < MIN_PACKAGE_UI_FRAME_WIDTH
            || self.frame_width > MAX_PACKAGE_UI_FRAME_WIDTH
        {
            return Err(PackageUiError::InvalidBounds);
        }
        if self.sections.is_empty() {
            return Err(PackageUiError::EmptySections);
        }
        if self.sections.len() > MAX_PACKAGE_UI_SECTIONS {
            return Err(PackageUiError::TooManySections);
        }
        if self.sections.len() > 1 && (self.required_capabilities & UI_CAP_SECTIONS) == 0 {
            return Err(PackageUiError::MissingCapability);
        }

        let mut total_items = 0usize;
        let mut total_meters = 0usize;
        let mut seen_bindings: Vec<(ParameterId, Option<PackageVisibilityRule>)> = Vec::new();
        let mut seen_meters: Vec<u32> = Vec::new();

        for section in &self.sections {
            if !valid_ui_text(&section.heading, MAX_PACKAGE_UI_LABEL_BYTES) {
                return Err(PackageUiError::InvalidText);
            }
            if !(1..=4).contains(&section.columns) || section.items.is_empty() {
                return Err(PackageUiError::InvalidBounds);
            }
            for item in &section.items {
                total_items += 1;
                if total_items > MAX_PACKAGE_UI_ITEMS {
                    return Err(PackageUiError::TooManyItems);
                }
                match item {
                    PackageUiItem::Control(control) => {
                        let req_cap = control.widget.required_capability();
                        if (self.required_capabilities & req_cap) == 0 {
                            return Err(PackageUiError::MissingCapability);
                        }
                        if let PackageUiWidgetKind::SelectorSegments { columns } = control.widget
                            && !(1..=8).contains(&columns)
                        {
                            return Err(PackageUiError::InvalidBounds);
                        }
                        if let Some(label) = control.label.as_deref()
                            && !valid_ui_text(label, MAX_PACKAGE_UI_LABEL_BYTES)
                        {
                            return Err(PackageUiError::InvalidText);
                        }
                        if let Some(tooltip) = control.tooltip.as_deref()
                            && !valid_ui_text(tooltip, MAX_PACKAGE_UI_TOOLTIP_BYTES)
                        {
                            return Err(PackageUiError::InvalidText);
                        }
                        let descriptor = descriptors
                            .iter()
                            .find(|candidate| candidate.id == control.parameter_id)
                            .ok_or(PackageUiError::UnknownParameter)?;
                        if !control.widget.matches_kind(descriptor.kind) {
                            return Err(PackageUiError::IncompatibleWidgetKind);
                        }
                        if let Some(rule) = control.show_when {
                            if (self.required_capabilities & UI_CAP_CONDITIONAL_VISIBILITY) == 0 {
                                return Err(PackageUiError::MissingCapability);
                            }
                            let cond_desc = descriptors
                                .iter()
                                .find(|candidate| candidate.id == rule.parameter_id)
                                .ok_or(PackageUiError::InvalidVisibilityRule)?;
                            if cond_desc.validate(rule.expected).is_err() {
                                return Err(PackageUiError::InvalidVisibilityRule);
                            }
                        }
                        for &(prev_id, prev_rule) in &seen_bindings {
                            if prev_id == control.parameter_id {
                                let mutually_exclusive = match (prev_rule, control.show_when) {
                                    (Some(a), Some(b)) => {
                                        a.parameter_id == b.parameter_id && a.expected != b.expected
                                    }
                                    _ => false,
                                };
                                if !mutually_exclusive {
                                    return Err(PackageUiError::DuplicateParameterBinding);
                                }
                            }
                        }
                        seen_bindings.push((control.parameter_id, control.show_when));
                    }
                    PackageUiItem::Meter(meter) => {
                        if (self.required_capabilities & UI_CAP_METER) == 0 {
                            return Err(PackageUiError::MissingCapability);
                        }
                        if meter.meter_id == 0 || seen_meters.contains(&meter.meter_id) {
                            return Err(PackageUiError::InvalidBounds);
                        }
                        if !valid_ui_text(&meter.label, MAX_PACKAGE_UI_LABEL_BYTES) {
                            return Err(PackageUiError::InvalidText);
                        }
                        total_meters += 1;
                        if total_meters > MAX_PACKAGE_UI_METERS {
                            return Err(PackageUiError::TooManyMeters);
                        }
                        seen_meters.push(meter.meter_id);
                    }
                }
            }
        }
        Ok(())
    }
}

/// One bounded peak-meter reading emitted by runtime telemetry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PackageMeterSample {
    pub meter_id: u32,
    pub left_peak: f32,
    pub right_peak: f32,
}

/// Read-only runtime presentation telemetry, strictly separated from authored
/// [`crate::NativeDeviceState`] so meter/voice/status updates never dirty
/// projects or Undo history.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PackagePresentationSnapshot {
    pub meters: Vec<PackageMeterSample>,
    pub active_voices: Option<u16>,
    pub status: Option<String>,
}

impl PackagePresentationSnapshot {
    pub fn validate(&self) -> Result<(), PackageUiError> {
        if self.meters.len() > MAX_PACKAGE_UI_METERS {
            return Err(PackageUiError::InvalidSnapshot);
        }
        for sample in &self.meters {
            if sample.meter_id == 0
                || !sample.left_peak.is_finite()
                || !sample.right_peak.is_finite()
                || sample.left_peak < 0.0
                || sample.right_peak < 0.0
            {
                return Err(PackageUiError::InvalidSnapshot);
            }
        }
        if let Some(status) = self.status.as_deref()
            && !valid_ui_text(status, MAX_PACKAGE_UI_STATUS_BYTES)
        {
            return Err(PackageUiError::InvalidSnapshot);
        }
        Ok(())
    }
}

fn valid_ui_text(text: &str, max_bytes: usize) -> bool {
    !text.is_empty() && text.len() <= max_bytes && !text.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ParameterPresentation;

    fn sample_descriptors() -> Vec<ParameterDescriptor> {
        vec![
            ParameterDescriptor {
                id: ParameterId::from_static(1),
                label: "gain",
                kind: ParameterKind::Float {
                    min: 0.0,
                    max: 2.0,
                    unit: None,
                },
                default: ParameterValue::Float(1.0),
                presentation: ParameterPresentation::Linear,
            },
            ParameterDescriptor {
                id: ParameterId::from_static(2),
                label: "mute",
                kind: ParameterKind::Bool,
                default: ParameterValue::Bool(false),
                presentation: ParameterPresentation::Linear,
            },
            ParameterDescriptor {
                id: ParameterId::from_static(3),
                label: "channel",
                kind: ParameterKind::Choice {
                    labels: &["stereo", "left", "right"],
                },
                default: ParameterValue::Choice(0),
                presentation: ParameterPresentation::Linear,
            },
        ]
    }

    fn valid_schema() -> PackageUiSchema {
        PackageUiSchema {
            ui_version: PACKAGE_UI_VERSION_V1,
            required_capabilities: UI_CAP_SCALAR
                | UI_CAP_TOGGLE
                | UI_CAP_SELECTOR
                | UI_CAP_METER
                | UI_CAP_SECTIONS,
            frame_width: 240.0,
            sections: vec![
                PackageUiSection {
                    heading: "input".into(),
                    columns: 1,
                    items: vec![
                        PackageUiItem::Control(PackageUiControl {
                            parameter_id: ParameterId::from_static(3),
                            widget: PackageUiWidgetKind::SelectorMenu,
                            label: None,
                            tooltip: Some("channel mode".into()),
                            display_format: PackageNumericFormat::Default,
                            show_when: None,
                        }),
                        PackageUiItem::Control(PackageUiControl {
                            parameter_id: ParameterId::from_static(2),
                            widget: PackageUiWidgetKind::ToggleButton,
                            label: Some("mute".into()),
                            tooltip: None,
                            display_format: PackageNumericFormat::Default,
                            show_when: None,
                        }),
                    ],
                },
                PackageUiSection {
                    heading: "output".into(),
                    columns: 1,
                    items: vec![
                        PackageUiItem::Control(PackageUiControl {
                            parameter_id: ParameterId::from_static(1),
                            widget: PackageUiWidgetKind::Dial,
                            label: Some("gain".into()),
                            tooltip: None,
                            display_format: PackageNumericFormat::AmplitudeDb,
                            show_when: None,
                        }),
                        PackageUiItem::Meter(PackageUiMeter {
                            meter_id: 1,
                            label: "out".into(),
                            stereo: true,
                        }),
                    ],
                },
            ],
        }
    }

    #[test]
    fn schema_validation_accepts_valid_layout_and_rejects_unsupported_or_mismatched_definitions() {
        let descriptors = sample_descriptors();
        let schema = valid_schema();
        assert_eq!(schema.validate(&descriptors), Ok(()));

        let mut future = schema.clone();
        future.ui_version = PACKAGE_UI_VERSION_V1 + 1;
        assert_eq!(
            future.validate(&descriptors),
            Err(PackageUiError::UnsupportedUiVersion)
        );

        let mut unknown_cap = schema.clone();
        unknown_cap.required_capabilities |= 1 << 60;
        assert_eq!(
            unknown_cap.validate(&descriptors),
            Err(PackageUiError::UnsupportedCapability)
        );

        let mut missing_cap = schema.clone();
        missing_cap.required_capabilities &= !UI_CAP_TOGGLE;
        assert_eq!(
            missing_cap.validate(&descriptors),
            Err(PackageUiError::MissingCapability)
        );

        let mut wrong_widget = schema.clone();
        if let PackageUiItem::Control(control) = &mut wrong_widget.sections[1].items[0] {
            control.widget = PackageUiWidgetKind::ToggleButton;
        }
        assert_eq!(
            wrong_widget.validate(&descriptors),
            Err(PackageUiError::IncompatibleWidgetKind)
        );

        let mut duplicate = schema.clone();
        duplicate.sections[1]
            .items
            .push(PackageUiItem::Control(PackageUiControl {
                parameter_id: ParameterId::from_static(1),
                widget: PackageUiWidgetKind::InlineValue,
                label: None,
                tooltip: None,
                display_format: PackageNumericFormat::Default,
                show_when: None,
            }));
        assert_eq!(
            duplicate.validate(&descriptors),
            Err(PackageUiError::DuplicateParameterBinding)
        );

        let snap = PackagePresentationSnapshot {
            meters: vec![PackageMeterSample {
                meter_id: 1,
                left_peak: 0.4,
                right_peak: 0.35,
            }],
            active_voices: Some(3),
            status: Some("ready".into()),
        };
        assert_eq!(snap.validate(), Ok(()));

        let bad_snap = PackagePresentationSnapshot {
            meters: vec![PackageMeterSample {
                meter_id: 1,
                left_peak: f32::NAN,
                right_peak: 0.0,
            }],
            active_voices: None,
            status: None,
        };
        assert_eq!(bad_snap.validate(), Err(PackageUiError::InvalidSnapshot));
    }
}
