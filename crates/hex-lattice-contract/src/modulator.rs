//! Canonical identifiers and bounds for Hex's built-in native modulators.
//!
//! `hex-lattice` defines the runtime registry (`NativeModulatorRegistry`), `hex-devices` registers
//! the 16 built-in codecs, and `hex-storage` references these canonical IDs without adding a
//! runtime dependency on `hex-devices` or `hex-lattice`.

use crate::DeviceTypeId;

/// Maximum number of native modulator types admitted in one immutable registry.
pub const MAX_NATIVE_MODULATOR_TYPES: usize = 64;

pub const ADSR: DeviceTypeId = DeviceTypeId::from_static("hex.modulator.adsr");
pub const AHD_RELEASE: DeviceTypeId = DeviceTypeId::from_static("hex.modulator.ahd-release");
pub const AUDIO_RATE: DeviceTypeId = DeviceTypeId::from_static("hex.modulator.audio-rate");
pub const BUTTON: DeviceTypeId = DeviceTypeId::from_static("hex.modulator.button");
pub const BUTTONS: DeviceTypeId = DeviceTypeId::from_static("hex.modulator.buttons");
pub const ENVELOPE_FOLLOWER: DeviceTypeId =
    DeviceTypeId::from_static("hex.modulator.envelope-follower");
pub const LFO: DeviceTypeId = DeviceTypeId::from_static("hex.modulator.lfo");
pub const MACRO_FOUR: DeviceTypeId = DeviceTypeId::from_static("hex.modulator.macro-four");
pub const MACRO_VALUE: DeviceTypeId = DeviceTypeId::from_static("hex.modulator.macro");
pub const MATH: DeviceTypeId = DeviceTypeId::from_static("hex.modulator.math");
pub const MIX: DeviceTypeId = DeviceTypeId::from_static("hex.modulator.mix");
pub const NOTE_COUNTER: DeviceTypeId = DeviceTypeId::from_static("hex.modulator.note-counter");
pub const NOTE_PRESSURE: DeviceTypeId = DeviceTypeId::from_static("hex.modulator.note-pressure");
pub const RAMP: DeviceTypeId = DeviceTypeId::from_static("hex.modulator.ramp");
pub const RANDOM_HOLD: DeviceTypeId = DeviceTypeId::from_static("hex.modulator.random-hold");
pub const STEPS: DeviceTypeId = DeviceTypeId::from_static("hex.modulator.steps");

/// All 16 built-in modulator type identifiers in canonical registration order.
pub const BUILTIN_MODULATOR_TYPES: [DeviceTypeId; 16] = [
    ADSR,
    AHD_RELEASE,
    AUDIO_RATE,
    BUTTON,
    BUTTONS,
    ENVELOPE_FOLLOWER,
    LFO,
    MACRO_FOUR,
    MACRO_VALUE,
    MATH,
    MIX,
    NOTE_COUNTER,
    NOTE_PRESSURE,
    RAMP,
    RANDOM_HOLD,
    STEPS,
];
