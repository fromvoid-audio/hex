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
/// Maximum interactive curve/band nodes bound to one reusable curve plot (`#3000`).
pub const MAX_PACKAGE_UI_CURVE_NODES: usize = 16;
/// Maximum harmonic bar bindings in one reusable harmonic editor (`#3001`).
pub const MAX_PACKAGE_UI_HARMONICS: usize = 16;
/// Maximum control-computed frequency-response points in a presentation snapshot.
pub const MAX_PACKAGE_RESPONSE_BINS: usize = 128;
/// Maximum FFT spectrum bins in a read-only presentation snapshot.
pub const MAX_PACKAGE_SPECTRUM_BINS: usize = 128;
/// Maximum waveform peak overview bins in a read-only presentation snapshot (`#3003`).
pub const MAX_PACKAGE_WAVEFORM_PEAKS: usize = 128;
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
/// Reusable bounded curve/node/plot capability (`#3000`).
pub const UI_CAP_CURVE_PLOT: u64 = 1 << 6;
/// Reusable bounded read-only spectrum overlay capability (`#3000`).
pub const UI_CAP_SPECTRUM_OVERLAY: u64 = 1 << 7;
/// Reusable bounded ADSR envelope plot capability (`#3001`).
pub const UI_CAP_ENVELOPE_PLOT: u64 = 1 << 8;
/// Reusable bounded harmonic bar-graph editor capability (`#3001`).
pub const UI_CAP_HARMONIC_EDITOR: u64 = 1 << 9;
/// Reusable bounded waveform and source-region editor capability (`#3003`).
pub const UI_CAP_WAVEFORM_REGION: u64 = 1 << 10;
/// Bitmask of all UI capabilities supported by v1 generic native hosts.
pub const UI_CAP_KNOWN_MASK_V1: u64 = UI_CAP_SCALAR
    | UI_CAP_TOGGLE
    | UI_CAP_SELECTOR
    | UI_CAP_METER
    | UI_CAP_SECTIONS
    | UI_CAP_CONDITIONAL_VISIBILITY
    | UI_CAP_CURVE_PLOT
    | UI_CAP_SPECTRUM_OVERLAY
    | UI_CAP_ENVELOPE_PLOT
    | UI_CAP_HARMONIC_EDITOR
    | UI_CAP_WAVEFORM_REGION;

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

/// Stable band/node parameter binding inside a reusable [`PackageUiCurvePlot`].
/// `node_id` identifies the dynamic band identity so removed bands can never
/// retarget another parameter when stale UI gestures arrive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PackageUiCurveNodeBinding {
    pub node_id: u32,
    pub freq_parameter: ParameterId,
    pub gain_parameter: ParameterId,
    pub q_parameter: Option<ParameterId>,
    pub enable_parameter: Option<ParameterId>,
}

/// Reusable bounded curve/node/plot/spectrum primitive (`#3000`) consumed
/// generically by EQ/EQ8/filter packages without per-type-ID GPUI switches.
#[derive(Clone, Debug, PartialEq)]
pub struct PackageUiCurvePlot {
    pub plot_id: u32,
    pub label: String,
    pub min_freq_hz: f32,
    pub max_freq_hz: f32,
    pub min_db: f32,
    pub max_db: f32,
    pub spectrum_overlay: bool,
    pub nodes: Vec<PackageUiCurveNodeBinding>,
}

/// Reusable bounded ADSR envelope plot bound to four scalar parameters (`#3001`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageUiEnvelopePlot {
    pub plot_id: u32,
    pub label: String,
    pub attack_parameter: ParameterId,
    pub decay_parameter: ParameterId,
    pub sustain_parameter: ParameterId,
    pub release_parameter: ParameterId,
}

/// Reusable bounded harmonic bar-graph editor bound to ordered scalar partial
/// parameters (`#3001`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageUiHarmonicBarGraph {
    pub graph_id: u32,
    pub label: String,
    pub harmonic_parameters: Vec<ParameterId>,
}

/// Reusable bounded waveform and source-region editor primitive (`#3003`)
/// consumed generically by sampler/granular packages without per-type-ID GPUI
/// switches or private state decoding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageUiWaveformRegion {
    pub region_id: u32,
    pub asset_slot: u32,
    pub label: String,
    pub start_parameter: ParameterId,
    pub end_parameter: ParameterId,
    pub loop_start_parameter: Option<ParameterId>,
    pub loop_end_parameter: Option<ParameterId>,
    pub loop_enable_parameter: Option<ParameterId>,
}

/// One item inside a [`PackageUiSection`].
#[derive(Clone, Debug, PartialEq)]
pub enum PackageUiItem {
    Control(PackageUiControl),
    Meter(PackageUiMeter),
    CurvePlot(PackageUiCurvePlot),
    EnvelopePlot(PackageUiEnvelopePlot),
    HarmonicBarGraph(PackageUiHarmonicBarGraph),
    WaveformRegion(PackageUiWaveformRegion),
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
                    PackageUiItem::CurvePlot(plot) => {
                        if (self.required_capabilities & UI_CAP_CURVE_PLOT) == 0 {
                            return Err(PackageUiError::MissingCapability);
                        }
                        if plot.spectrum_overlay
                            && (self.required_capabilities & UI_CAP_SPECTRUM_OVERLAY) == 0
                        {
                            return Err(PackageUiError::MissingCapability);
                        }
                        if plot.plot_id == 0
                            || !plot.min_freq_hz.is_finite()
                            || !plot.max_freq_hz.is_finite()
                            || plot.min_freq_hz < 10.0
                            || plot.min_freq_hz >= plot.max_freq_hz
                            || plot.max_freq_hz > 48_000.0
                            || !plot.min_db.is_finite()
                            || !plot.max_db.is_finite()
                            || plot.min_db < -48.0
                            || plot.min_db >= plot.max_db
                            || plot.max_db > 48.0
                            || plot.nodes.len() > MAX_PACKAGE_UI_CURVE_NODES
                        {
                            return Err(PackageUiError::InvalidBounds);
                        }
                        if !valid_ui_text(&plot.label, MAX_PACKAGE_UI_LABEL_BYTES) {
                            return Err(PackageUiError::InvalidText);
                        }
                        let mut seen_nodes = Vec::with_capacity(plot.nodes.len());
                        for node in &plot.nodes {
                            if node.node_id == 0 || seen_nodes.contains(&node.node_id) {
                                return Err(PackageUiError::InvalidBounds);
                            }
                            seen_nodes.push(node.node_id);
                            for required_float in [node.freq_parameter, node.gain_parameter] {
                                let desc = descriptors
                                    .iter()
                                    .find(|candidate| candidate.id == required_float)
                                    .ok_or(PackageUiError::UnknownParameter)?;
                                if !matches!(desc.kind, ParameterKind::Float { .. }) {
                                    return Err(PackageUiError::IncompatibleWidgetKind);
                                }
                            }
                            if let Some(q_id) = node.q_parameter {
                                let desc = descriptors
                                    .iter()
                                    .find(|candidate| candidate.id == q_id)
                                    .ok_or(PackageUiError::UnknownParameter)?;
                                if !matches!(desc.kind, ParameterKind::Float { .. }) {
                                    return Err(PackageUiError::IncompatibleWidgetKind);
                                }
                            }
                            if let Some(en_id) = node.enable_parameter {
                                let desc = descriptors
                                    .iter()
                                    .find(|candidate| candidate.id == en_id)
                                    .ok_or(PackageUiError::UnknownParameter)?;
                                if !matches!(
                                    desc.kind,
                                    ParameterKind::Bool | ParameterKind::Float { .. }
                                ) {
                                    return Err(PackageUiError::IncompatibleWidgetKind);
                                }
                            }
                        }
                    }
                    PackageUiItem::EnvelopePlot(env) => {
                        if (self.required_capabilities & UI_CAP_ENVELOPE_PLOT) == 0 {
                            return Err(PackageUiError::MissingCapability);
                        }
                        if env.plot_id == 0 {
                            return Err(PackageUiError::InvalidBounds);
                        }
                        if !valid_ui_text(&env.label, MAX_PACKAGE_UI_LABEL_BYTES) {
                            return Err(PackageUiError::InvalidText);
                        }
                        for param_id in [
                            env.attack_parameter,
                            env.decay_parameter,
                            env.sustain_parameter,
                            env.release_parameter,
                        ] {
                            let desc = descriptors
                                .iter()
                                .find(|candidate| candidate.id == param_id)
                                .ok_or(PackageUiError::UnknownParameter)?;
                            if !matches!(desc.kind, ParameterKind::Float { .. }) {
                                return Err(PackageUiError::IncompatibleWidgetKind);
                            }
                        }
                    }
                    PackageUiItem::HarmonicBarGraph(harm) => {
                        if (self.required_capabilities & UI_CAP_HARMONIC_EDITOR) == 0 {
                            return Err(PackageUiError::MissingCapability);
                        }
                        if harm.graph_id == 0
                            || harm.harmonic_parameters.is_empty()
                            || harm.harmonic_parameters.len() > MAX_PACKAGE_UI_HARMONICS
                        {
                            return Err(PackageUiError::InvalidBounds);
                        }
                        if !valid_ui_text(&harm.label, MAX_PACKAGE_UI_LABEL_BYTES) {
                            return Err(PackageUiError::InvalidText);
                        }
                        for &param_id in &harm.harmonic_parameters {
                            let desc = descriptors
                                .iter()
                                .find(|candidate| candidate.id == param_id)
                                .ok_or(PackageUiError::UnknownParameter)?;
                            if !matches!(desc.kind, ParameterKind::Float { .. }) {
                                return Err(PackageUiError::IncompatibleWidgetKind);
                            }
                        }
                    }
                    PackageUiItem::WaveformRegion(region) => {
                        if (self.required_capabilities & UI_CAP_WAVEFORM_REGION) == 0 {
                            return Err(PackageUiError::MissingCapability);
                        }
                        if region.region_id == 0
                            || region.asset_slot == 0
                            || (region.asset_slot as usize) > crate::MAX_DEVICE_ASSET_REFERENCES
                        {
                            return Err(PackageUiError::InvalidBounds);
                        }
                        if !valid_ui_text(&region.label, MAX_PACKAGE_UI_LABEL_BYTES) {
                            return Err(PackageUiError::InvalidText);
                        }
                        for param_id in [region.start_parameter, region.end_parameter] {
                            let desc = descriptors
                                .iter()
                                .find(|candidate| candidate.id == param_id)
                                .ok_or(PackageUiError::UnknownParameter)?;
                            if !matches!(desc.kind, ParameterKind::Float { .. }) {
                                return Err(PackageUiError::IncompatibleWidgetKind);
                            }
                        }
                        for param_id in [region.loop_start_parameter, region.loop_end_parameter]
                            .into_iter()
                            .flatten()
                        {
                            let desc = descriptors
                                .iter()
                                .find(|candidate| candidate.id == param_id)
                                .ok_or(PackageUiError::UnknownParameter)?;
                            if !matches!(desc.kind, ParameterKind::Float { .. }) {
                                return Err(PackageUiError::IncompatibleWidgetKind);
                            }
                        }
                        if let Some(loop_en) = region.loop_enable_parameter {
                            let desc = descriptors
                                .iter()
                                .find(|candidate| candidate.id == loop_en)
                                .ok_or(PackageUiError::UnknownParameter)?;
                            if !matches!(
                                desc.kind,
                                ParameterKind::Bool | ParameterKind::Float { .. }
                            ) {
                                return Err(PackageUiError::IncompatibleWidgetKind);
                            }
                        }
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
    /// Bounded control-computed frequency-response curve in dB (`#3000`).
    pub response_db: Vec<f32>,
    /// Bounded off-callback FFT spectrum snapshot in dB (`#3000`).
    pub spectrum_db: Vec<f32>,
    /// Bounded control-computed normalized peak overview (`0.0..=1.0`, `#3003`).
    pub waveform_peaks: Vec<f32>,
    /// Preserved user sample display name/path stem (`#3003`), never mutating state.
    pub sample_name: Option<String>,
}

impl PackagePresentationSnapshot {
    pub fn validate(&self) -> Result<(), PackageUiError> {
        if self.meters.len() > MAX_PACKAGE_UI_METERS
            || self.response_db.len() > MAX_PACKAGE_RESPONSE_BINS
            || self.spectrum_db.len() > MAX_PACKAGE_SPECTRUM_BINS
            || self.waveform_peaks.len() > MAX_PACKAGE_WAVEFORM_PEAKS
        {
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
        if self.response_db.iter().any(|v| !v.is_finite())
            || self.spectrum_db.iter().any(|v| !v.is_finite())
            || self
                .waveform_peaks
                .iter()
                .any(|v| !v.is_finite() || *v < 0.0 || *v > 1.0)
        {
            return Err(PackageUiError::InvalidSnapshot);
        }
        if let Some(status) = self.status.as_deref()
            && !valid_ui_text(status, MAX_PACKAGE_UI_STATUS_BYTES)
        {
            return Err(PackageUiError::InvalidSnapshot);
        }
        if let Some(name) = self.sample_name.as_deref()
            && !valid_ui_text(name, MAX_PACKAGE_UI_STATUS_BYTES)
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
            response_db: vec![0.0, 1.5, -2.0],
            spectrum_db: vec![-60.0, -32.0, -18.0],
            waveform_peaks: vec![0.1, 0.8, 0.45],
            sample_name: Some("kick_01.wav".into()),
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
            response_db: vec![],
            spectrum_db: vec![],
            waveform_peaks: vec![],
            sample_name: None,
        };
        assert_eq!(bad_snap.validate(), Err(PackageUiError::InvalidSnapshot));

        // WaveformRegion requires explicit UI_CAP_WAVEFORM_REGION and valid asset_slot.
        let mut wf_schema = PackageUiSchema {
            ui_version: PACKAGE_UI_VERSION_V1,
            required_capabilities: UI_CAP_WAVEFORM_REGION,
            frame_width: 320.0,
            sections: vec![PackageUiSection {
                heading: "sample".into(),
                columns: 1,
                items: vec![PackageUiItem::WaveformRegion(PackageUiWaveformRegion {
                    region_id: 1,
                    asset_slot: 1,
                    label: "waveform".into(),
                    start_parameter: ParameterId::from_static(1),
                    end_parameter: ParameterId::from_static(1),
                    loop_start_parameter: None,
                    loop_end_parameter: None,
                    loop_enable_parameter: Some(ParameterId::from_static(2)),
                })],
            }],
        };
        assert_eq!(wf_schema.validate(&descriptors), Ok(()));
        wf_schema.required_capabilities &= !UI_CAP_WAVEFORM_REGION;
        assert_eq!(
            wf_schema.validate(&descriptors),
            Err(PackageUiError::MissingCapability)
        );

        // CurvePlot requires explicit UI_CAP_CURVE_PLOT and UI_CAP_SPECTRUM_OVERLAY when enabled.
        let mut curve_schema = PackageUiSchema {
            ui_version: PACKAGE_UI_VERSION_V1,
            required_capabilities: UI_CAP_CURVE_PLOT | UI_CAP_SPECTRUM_OVERLAY,
            frame_width: 320.0,
            sections: vec![PackageUiSection {
                heading: "response".into(),
                columns: 1,
                items: vec![PackageUiItem::CurvePlot(PackageUiCurvePlot {
                    plot_id: 1,
                    label: "eq curve".into(),
                    min_freq_hz: 20.0,
                    max_freq_hz: 20_000.0,
                    min_db: -24.0,
                    max_db: 24.0,
                    spectrum_overlay: true,
                    nodes: vec![PackageUiCurveNodeBinding {
                        node_id: 1,
                        freq_parameter: ParameterId::from_static(1),
                        gain_parameter: ParameterId::from_static(1),
                        q_parameter: None,
                        enable_parameter: Some(ParameterId::from_static(2)),
                    }],
                })],
            }],
        };
        assert_eq!(curve_schema.validate(&descriptors), Ok(()));
        curve_schema.required_capabilities &= !UI_CAP_SPECTRUM_OVERLAY;
        assert_eq!(
            curve_schema.validate(&descriptors),
            Err(PackageUiError::MissingCapability)
        );
    }
}
