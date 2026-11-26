//! Bounded VST3 `IMidiMapping` controller translation, per-note expression/MPE
//! tracking, ordered output event routing, and tuning/MTS capability disclosure.
//!
//! All translation structures use fixed-capacity inline storage so callback-side
//! consumers can translate events without allocating or substituting raw MIDI.

use std::fmt;

/// Matches Steinberg's `ControllerNumbers::kAfterTouch`.
pub const MIDI_CONTROLLER_AFTERTOUCH: u16 = 128;
/// Matches Steinberg's `ControllerNumbers::kPitchBend`.
pub const MIDI_CONTROLLER_PITCH_BEND: u16 = 129;

/// Bounded per-plugin `IMidiMapping` entry capacity.
pub const MAX_MIDI_MAPPINGS: usize = 256;
/// Matches `INPUT_NOTE_COUNT` in the native macOS VST3 worker.
pub const MAX_TRACKED_NOTES: usize = 160;
/// Bounded per-block output event batch capacity.
pub const MAX_OUTPUT_EVENTS: usize = 160;

/// Steinberg VST3 `IMidiMapping` controller address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MidiControllerKind {
    Cc(u8),
    Aftertouch,
    PitchBend,
}

impl MidiControllerKind {
    pub fn from_vst3_controller_number(number: u16) -> Result<Self, MidiTranslationError> {
        match number {
            0..=127 => Ok(Self::Cc(number as u8)),
            MIDI_CONTROLLER_AFTERTOUCH => Ok(Self::Aftertouch),
            MIDI_CONTROLLER_PITCH_BEND => Ok(Self::PitchBend),
            _ => Err(MidiTranslationError::InvalidController),
        }
    }

    pub const fn to_vst3_controller_number(self) -> u16 {
        match self {
            Self::Cc(cc) => cc as u16,
            Self::Aftertouch => MIDI_CONTROLLER_AFTERTOUCH,
            Self::PitchBend => MIDI_CONTROLLER_PITCH_BEND,
        }
    }
}

/// One validated `(channel, controller) -> ParamID` binding from `IMidiMapping`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiControllerMapping {
    pub channel: u8,
    pub controller: MidiControllerKind,
    pub param_id: u32,
}

/// Incoming controller event to translate via `IMidiMapping`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiControllerEvent {
    pub sample_offset: u32,
    pub channel: u8,
    pub controller: MidiControllerKind,
    /// 0..=127 for CC and Aftertouch; 0..=16383 for 14-bit PitchBend (center = 8192).
    pub raw_value: u16,
}

/// Exact parameter change produced by `IMidiMapping` translation (no raw MIDI fallback).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TranslatedParameterChange {
    pub sample_offset: u32,
    pub param_id: u32,
    pub normalized: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TranslationOutcome {
    Mapped(TranslatedParameterChange),
    /// Controller has no `IMidiMapping` binding; never silently substituted with raw MIDI.
    Unmapped,
}

/// Standard Steinberg `NoteExpressionTypeIDs` plus bounded custom expression IDs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NoteExpressionTypeId {
    Volume,
    Pan,
    Tuning,
    Vibrato,
    Expression,
    Brightness,
    Custom(u32),
}

impl NoteExpressionTypeId {
    pub const fn to_vst3_id(self) -> u32 {
        match self {
            Self::Volume => 0,
            Self::Pan => 1,
            Self::Tuning => 2,
            Self::Vibrato => 3,
            Self::Expression => 4,
            Self::Brightness => 5,
            Self::Custom(id) => id,
        }
    }

    pub const fn from_vst3_id(id: u32) -> Self {
        match id {
            0 => Self::Volume,
            1 => Self::Pan,
            2 => Self::Tuning,
            3 => Self::Vibrato,
            4 => Self::Expression,
            5 => Self::Brightness,
            other => Self::Custom(other),
        }
    }
}

/// Per-note expression update bound to an active `note_id` and exact `sample_offset`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoteExpressionEvent {
    pub sample_offset: u32,
    pub note_id: i32,
    pub type_id: NoteExpressionTypeId,
    pub normalized: f64,
}

/// Ordered VST3 plugin output event routed to a Hex event destination.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PluginOutputEvent {
    NoteOn {
        sample_offset: u32,
        channel: u8,
        pitch: u8,
        velocity: f32,
        note_id: i32,
    },
    NoteOff {
        sample_offset: u32,
        channel: u8,
        pitch: u8,
        velocity: f32,
        note_id: i32,
    },
    PolyPressure {
        sample_offset: u32,
        channel: u8,
        pitch: u8,
        pressure: f32,
        note_id: i32,
    },
    NoteExpression(NoteExpressionEvent),
}

impl PluginOutputEvent {
    pub const fn sample_offset(self) -> u32 {
        match self {
            Self::NoteOn { sample_offset, .. }
            | Self::NoteOff { sample_offset, .. }
            | Self::PolyPressure { sample_offset, .. }
            | Self::NoteExpression(NoteExpressionEvent { sample_offset, .. }) => sample_offset,
        }
    }
}

/// Explicit disclosure of a VST3 plugin's tuning/MTS capability so project tuning
/// is never applied twice and unsupported plugins never pretend arbitrary retuning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginTuningCapability {
    /// Plugin advertises `kTuningTypeID` with a symmetric supported range in semitones.
    NoteExpressionTuning { semitone_range: u8 },
    /// Plugin manages its own MTS-ESP or internal microtuning client.
    PluginInternalMts,
    /// Plugin does not expose per-note tuning expression or MTS.
    FixedTwelveTetOnly,
}

/// Resolved host tuning action for a note with a requested microtonal offset in semitones.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TuningDispatchPolicy {
    /// Apply project tuning once via VST3 `kTuningTypeID` normalized expression value.
    ApplyNoteExpression { normalized: f64 },
    /// Leave VST3 note tuning neutral because the plugin applies MTS internally.
    DelegatedToPlugin,
    /// Plugin does not support retuning; disclosed explicitly to host/UI without double-tuning.
    UnsupportedDisclosed,
}

/// VST3 kTuningTypeID spans −120 to +120 semitones with neutral at 0.5.
/// A narrower admitted range limits the displacement without changing its units.
fn normalized_vst3_tuning(semitones: f64, range: u8) -> Result<f64, MidiTranslationError> {
    if range == 0 || !semitones.is_finite() {
        return Err(MidiTranslationError::InvalidValue);
    }
    let limit = f64::from(range.min(120));
    Ok(0.5 + semitones.clamp(-limit, limit) / 240.0)
}

/// Resolve how project tuning interacts with a plugin's declared tuning capability.
pub fn resolve_tuning_dispatch(
    capability: PluginTuningCapability,
    detune_semitones: f64,
) -> Result<TuningDispatchPolicy, MidiTranslationError> {
    if !detune_semitones.is_finite() {
        return Err(MidiTranslationError::InvalidValue);
    }
    match capability {
        PluginTuningCapability::NoteExpressionTuning { semitone_range } => {
            let normalized = normalized_vst3_tuning(detune_semitones, semitone_range)?;
            Ok(TuningDispatchPolicy::ApplyNoteExpression { normalized })
        }
        PluginTuningCapability::PluginInternalMts => Ok(TuningDispatchPolicy::DelegatedToPlugin),
        PluginTuningCapability::FixedTwelveTetOnly => {
            Ok(TuningDispatchPolicy::UnsupportedDisclosed)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiTranslationError {
    InvalidChannel,
    InvalidController,
    InvalidPitch,
    InvalidNoteId,
    InvalidValue,
    DuplicateMapping,
    CapacityExceeded,
    NoteNotActive,
    OutOfOrderOffset,
}

impl fmt::Display for MidiTranslationError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(match self {
            Self::InvalidChannel => "midi channel must be in 0..=15",
            Self::InvalidController => "midi controller number is out of range",
            Self::InvalidPitch => "midi pitch must be in 0..=127",
            Self::InvalidNoteId => "vst3 note id must be non-negative",
            Self::InvalidValue => "midi or note-expression value is out of range",
            Self::DuplicateMapping => "midi controller mapping is duplicated",
            Self::CapacityExceeded => "vst3 midi translation capacity exceeded",
            Self::NoteNotActive => "note expression requires an active note-on",
            Self::OutOfOrderOffset => "output event sample offsets must be non-decreasing",
        })
    }
}

impl std::error::Error for MidiTranslationError {}

/// Fixed-capacity `IMidiMapping` lookup table for zero-allocation translation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MidiControllerMappingTable {
    entries: [Option<MidiControllerMapping>; MAX_MIDI_MAPPINGS],
    len: usize,
}

impl Default for MidiControllerMappingTable {
    fn default() -> Self {
        Self::new()
    }
}

impl MidiControllerMappingTable {
    pub const fn new() -> Self {
        Self {
            entries: [None; MAX_MIDI_MAPPINGS],
            len: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn insert(&mut self, mapping: MidiControllerMapping) -> Result<(), MidiTranslationError> {
        if mapping.channel > 15 {
            return Err(MidiTranslationError::InvalidChannel);
        }
        if let MidiControllerKind::Cc(cc) = mapping.controller
            && cc > 127
        {
            return Err(MidiTranslationError::InvalidController);
        }
        if self.entries[..self.len].iter().flatten().any(|existing| {
            existing.channel == mapping.channel && existing.controller == mapping.controller
        }) {
            return Err(MidiTranslationError::DuplicateMapping);
        }
        if self.len >= MAX_MIDI_MAPPINGS {
            return Err(MidiTranslationError::CapacityExceeded);
        }
        self.entries[self.len] = Some(mapping);
        self.len += 1;
        Ok(())
    }

    /// Translate one MIDI CC, channel pressure, or 14-bit pitch-bend event to
    /// its mapped VST3 `ParamID` and normalized value at the exact `sample_offset`.
    pub fn translate(
        &self,
        event: MidiControllerEvent,
    ) -> Result<TranslationOutcome, MidiTranslationError> {
        if event.channel > 15 {
            return Err(MidiTranslationError::InvalidChannel);
        }
        let normalized = match event.controller {
            MidiControllerKind::Cc(cc) => {
                if cc > 127 {
                    return Err(MidiTranslationError::InvalidController);
                }
                if event.raw_value > 127 {
                    return Err(MidiTranslationError::InvalidValue);
                }
                f64::from(event.raw_value) / 127.0
            }
            MidiControllerKind::Aftertouch => {
                if event.raw_value > 127 {
                    return Err(MidiTranslationError::InvalidValue);
                }
                f64::from(event.raw_value) / 127.0
            }
            MidiControllerKind::PitchBend => {
                if event.raw_value > 16_383 {
                    return Err(MidiTranslationError::InvalidValue);
                }
                f64::from(event.raw_value) / 16_383.0
            }
        };
        let Some(mapping) = self.entries[..self.len]
            .iter()
            .flatten()
            .find(|entry| entry.channel == event.channel && entry.controller == event.controller)
        else {
            return Ok(TranslationOutcome::Unmapped);
        };
        Ok(TranslationOutcome::Mapped(TranslatedParameterChange {
            sample_offset: event.sample_offset,
            param_id: mapping.param_id,
            normalized,
        }))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ActiveNote {
    channel: u8,
    pitch: u8,
    note_id: i32,
}

/// Fixed-capacity active-note ledger enforcing that per-note expression and MPE
/// updates follow their Note-On and retain exact `note_id` across blocks until Note-Off.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteExpressionTracker {
    active: [Option<ActiveNote>; MAX_TRACKED_NOTES],
    len: usize,
}

impl Default for NoteExpressionTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl NoteExpressionTracker {
    pub const fn new() -> Self {
        Self {
            active: [None; MAX_TRACKED_NOTES],
            len: 0,
        }
    }

    pub const fn active_count(&self) -> usize {
        self.len
    }

    pub fn note_on(
        &mut self,
        channel: u8,
        pitch: u8,
        note_id: i32,
    ) -> Result<(), MidiTranslationError> {
        if channel > 15 {
            return Err(MidiTranslationError::InvalidChannel);
        }
        if pitch > 127 {
            return Err(MidiTranslationError::InvalidPitch);
        }
        if note_id < 0 {
            return Err(MidiTranslationError::InvalidNoteId);
        }
        if let Some(slot) = self.active[..self.len]
            .iter_mut()
            .flatten()
            .find(|entry| entry.note_id == note_id)
        {
            slot.channel = channel;
            slot.pitch = pitch;
            return Ok(());
        }
        if self.len >= MAX_TRACKED_NOTES {
            return Err(MidiTranslationError::CapacityExceeded);
        }
        self.active[self.len] = Some(ActiveNote {
            channel,
            pitch,
            note_id,
        });
        self.len += 1;
        Ok(())
    }

    pub fn note_off(&mut self, note_id: i32) -> Result<bool, MidiTranslationError> {
        if note_id < 0 {
            return Err(MidiTranslationError::InvalidNoteId);
        }
        let Some(pos) = self.active[..self.len]
            .iter()
            .position(|slot| slot.is_some_and(|entry| entry.note_id == note_id))
        else {
            return Ok(false);
        };
        for index in pos..self.len.saturating_sub(1) {
            self.active[index] = self.active[index + 1];
        }
        self.len -= 1;
        self.active[self.len] = None;
        Ok(true)
    }

    /// Validate a direct per-note expression event against an active `note_id`.
    pub fn admit_expression(
        &self,
        event: NoteExpressionEvent,
    ) -> Result<NoteExpressionEvent, MidiTranslationError> {
        if event.note_id < 0 {
            return Err(MidiTranslationError::InvalidNoteId);
        }
        if !event.normalized.is_finite() || !(0.0..=1.0).contains(&event.normalized) {
            return Err(MidiTranslationError::InvalidValue);
        }
        let found = self.active[..self.len]
            .iter()
            .flatten()
            .any(|entry| entry.note_id == event.note_id);
        if !found {
            return Err(MidiTranslationError::NoteNotActive);
        }
        Ok(event)
    }

    /// Translate an MPE member-channel pitch-bend, pressure, or CC74 (brightness/slide)
    /// message into a VST3 `NoteExpressionEvent` bound to the active note on `channel`.
    /// For pitch bend, pass the member bend range captured for that note.
    pub fn translate_mpe_channel_event(
        &self,
        event: MidiControllerEvent,
        member_bend_range_semitones: u8,
    ) -> Result<NoteExpressionEvent, MidiTranslationError> {
        if event.channel > 15 {
            return Err(MidiTranslationError::InvalidChannel);
        }
        let active = self.active[..self.len]
            .iter()
            .flatten()
            .rev()
            .find(|entry| entry.channel == event.channel)
            .ok_or(MidiTranslationError::NoteNotActive)?;
        let (type_id, normalized) = match event.controller {
            MidiControllerKind::PitchBend => {
                if event.raw_value > 16_383 {
                    return Err(MidiTranslationError::InvalidValue);
                }
                let bend = i32::from(event.raw_value) - 8192;
                let full_scale = if bend < 0 { 8192.0 } else { 8191.0 };
                let semitones =
                    f64::from(bend) * f64::from(member_bend_range_semitones) / full_scale;
                (
                    NoteExpressionTypeId::Tuning,
                    normalized_vst3_tuning(semitones, member_bend_range_semitones)?,
                )
            }
            MidiControllerKind::Aftertouch => {
                if event.raw_value > 127 {
                    return Err(MidiTranslationError::InvalidValue);
                }
                (
                    NoteExpressionTypeId::Expression,
                    f64::from(event.raw_value) / 127.0,
                )
            }
            MidiControllerKind::Cc(74) => {
                if event.raw_value > 127 {
                    return Err(MidiTranslationError::InvalidValue);
                }
                (
                    NoteExpressionTypeId::Brightness,
                    f64::from(event.raw_value) / 127.0,
                )
            }
            MidiControllerKind::Cc(_) => return Err(MidiTranslationError::InvalidController),
        };
        Ok(NoteExpressionEvent {
            sample_offset: event.sample_offset,
            note_id: active.note_id,
            type_id,
            normalized,
        })
    }

    pub fn clear(&mut self) {
        self.active[..self.len].fill(None);
        self.len = 0;
    }
}

/// Fixed-capacity, sample-offset-ordered batch of VST3 plugin output events.
#[derive(Clone, Debug, PartialEq)]
pub struct OutputEventBatch {
    events: [Option<PluginOutputEvent>; MAX_OUTPUT_EVENTS],
    len: usize,
    last_offset: u32,
}

impl Default for OutputEventBatch {
    fn default() -> Self {
        Self::new()
    }
}

impl OutputEventBatch {
    pub const fn new() -> Self {
        Self {
            events: [None; MAX_OUTPUT_EVENTS],
            len: 0,
            last_offset: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn push(&mut self, event: PluginOutputEvent) -> Result<(), MidiTranslationError> {
        let offset = event.sample_offset();
        if self.len > 0 && offset < self.last_offset {
            return Err(MidiTranslationError::OutOfOrderOffset);
        }
        match event {
            PluginOutputEvent::NoteOn {
                channel,
                pitch,
                velocity,
                ..
            }
            | PluginOutputEvent::NoteOff {
                channel,
                pitch,
                velocity,
                ..
            } => {
                if channel > 15 {
                    return Err(MidiTranslationError::InvalidChannel);
                }
                if pitch > 127 {
                    return Err(MidiTranslationError::InvalidPitch);
                }
                if !velocity.is_finite() || !(0.0..=1.0).contains(&velocity) {
                    return Err(MidiTranslationError::InvalidValue);
                }
            }
            PluginOutputEvent::PolyPressure {
                channel,
                pitch,
                pressure,
                ..
            } => {
                if channel > 15 {
                    return Err(MidiTranslationError::InvalidChannel);
                }
                if pitch > 127 {
                    return Err(MidiTranslationError::InvalidPitch);
                }
                if !pressure.is_finite() || !(0.0..=1.0).contains(&pressure) {
                    return Err(MidiTranslationError::InvalidValue);
                }
            }
            PluginOutputEvent::NoteExpression(expr) => {
                if expr.note_id < 0 {
                    return Err(MidiTranslationError::InvalidNoteId);
                }
                if !expr.normalized.is_finite() || !(0.0..=1.0).contains(&expr.normalized) {
                    return Err(MidiTranslationError::InvalidValue);
                }
            }
        }
        if self.len >= MAX_OUTPUT_EVENTS {
            return Err(MidiTranslationError::CapacityExceeded);
        }
        self.events[self.len] = Some(event);
        self.len += 1;
        self.last_offset = offset;
        Ok(())
    }

    pub fn iter(&self) -> impl Iterator<Item = PluginOutputEvent> + '_ {
        self.events[..self.len].iter().flatten().copied()
    }

    pub fn clear(&mut self) {
        self.events[..self.len].fill(None);
        self.len = 0;
        self.last_offset = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midi_mapping_translates_cc_aftertouch_and_pitch_bend_without_raw_midi_fallback() {
        let mut table = MidiControllerMappingTable::new();
        table
            .insert(MidiControllerMapping {
                channel: 0,
                controller: MidiControllerKind::Cc(1),
                param_id: 1001,
            })
            .unwrap();
        table
            .insert(MidiControllerMapping {
                channel: 0,
                controller: MidiControllerKind::Aftertouch,
                param_id: 1002,
            })
            .unwrap();
        table
            .insert(MidiControllerMapping {
                channel: 0,
                controller: MidiControllerKind::PitchBend,
                param_id: 1003,
            })
            .unwrap();
        // Duplicate refuses cleanly.
        assert_eq!(
            table.insert(MidiControllerMapping {
                channel: 0,
                controller: MidiControllerKind::Cc(1),
                param_id: 2000,
            }),
            Err(MidiTranslationError::DuplicateMapping)
        );

        let cc = table
            .translate(MidiControllerEvent {
                sample_offset: 17,
                channel: 0,
                controller: MidiControllerKind::Cc(1),
                raw_value: 127,
            })
            .unwrap();
        assert_eq!(
            cc,
            TranslationOutcome::Mapped(TranslatedParameterChange {
                sample_offset: 17,
                param_id: 1001,
                normalized: 1.0,
            })
        );

        let at = table
            .translate(MidiControllerEvent {
                sample_offset: 23,
                channel: 0,
                controller: MidiControllerKind::Aftertouch,
                raw_value: 64,
            })
            .unwrap();
        assert_eq!(
            at,
            TranslationOutcome::Mapped(TranslatedParameterChange {
                sample_offset: 23,
                param_id: 1002,
                normalized: 64.0 / 127.0,
            })
        );

        let pb = table
            .translate(MidiControllerEvent {
                sample_offset: 31,
                channel: 0,
                controller: MidiControllerKind::PitchBend,
                raw_value: 16_383,
            })
            .unwrap();
        assert_eq!(
            pb,
            TranslationOutcome::Mapped(TranslatedParameterChange {
                sample_offset: 31,
                param_id: 1003,
                normalized: 1.0,
            })
        );

        // Unmapped controller is explicitly Unmapped, never substituted as raw MIDI.
        assert_eq!(
            table
                .translate(MidiControllerEvent {
                    sample_offset: 40,
                    channel: 0,
                    controller: MidiControllerKind::Cc(74),
                    raw_value: 64,
                })
                .unwrap(),
            TranslationOutcome::Unmapped
        );
    }

    #[test]
    fn mpe_tuning_recovers_the_member_bend_in_semitones() {
        let mut tracker = NoteExpressionTracker::new();
        tracker.note_on(2, 60, 42).unwrap();
        for range in [2_u8, 48] {
            for (raw_value, expected) in [
                (0, -f64::from(range)),
                (4096, -0.5 * f64::from(range)),
                (8192, 0.0),
                (16383, f64::from(range)),
            ] {
                let event = MidiControllerEvent {
                    sample_offset: 19,
                    channel: 2,
                    controller: MidiControllerKind::PitchBend,
                    raw_value,
                };
                let expression = tracker.translate_mpe_channel_event(event, range).unwrap();
                // Steinberg ivstnoteexpression.h defines the inverse for kTuningTypeID.
                let semitones = 240.0 * (expression.normalized - 0.5);
                assert!(
                    (semitones - expected).abs() < 1e-12,
                    "{range} {raw_value}: {semitones}"
                );
                assert_eq!((expression.note_id, expression.sample_offset), (42, 19));
            }
        }
    }

    #[test]
    fn project_tuning_clips_to_supported_semitones_and_preserves_sdk_limits() {
        for (range, requested, expected) in
            [(2, -12.0, -2.0), (48, 90.0, 48.0), (127, 121.0, 120.0)]
        {
            let capability = PluginTuningCapability::NoteExpressionTuning {
                semitone_range: range,
            };
            let TuningDispatchPolicy::ApplyNoteExpression { normalized } =
                resolve_tuning_dispatch(capability, requested).unwrap()
            else {
                panic!("expected tuning")
            };
            assert!((240.0 * (normalized - 0.5) - expected).abs() < 1e-12);
        }
        for (range, requested) in [(0, 1.0), (2, f64::NAN)] {
            assert_eq!(
                resolve_tuning_dispatch(
                    PluginTuningCapability::NoteExpressionTuning {
                        semitone_range: range
                    },
                    requested
                ),
                Err(MidiTranslationError::InvalidValue)
            );
        }
    }

    #[test]
    fn note_expression_and_mpe_follow_note_on_across_blocks_and_refuse_orphans() {
        let mut tracker = NoteExpressionTracker::new();
        // Expression before Note-On is refused.
        assert_eq!(
            tracker.admit_expression(NoteExpressionEvent {
                sample_offset: 4,
                note_id: 42,
                type_id: NoteExpressionTypeId::Brightness,
                normalized: 0.75,
            }),
            Err(MidiTranslationError::NoteNotActive)
        );

        // Block 1: Note-On on MPE member channel 2 with note_id 42.
        tracker.note_on(2, 60, 42).unwrap();
        // Block 2: MPE pitch bend and CC74 retain exact note_id 42 across blocks.
        let bend = tracker
            .translate_mpe_channel_event(
                MidiControllerEvent {
                    sample_offset: 12,
                    channel: 2,
                    controller: MidiControllerKind::PitchBend,
                    raw_value: 12_288,
                },
                48,
            )
            .unwrap();
        assert_eq!(bend.note_id, 42);
        assert_eq!(bend.sample_offset, 12);
        assert_eq!(bend.type_id, NoteExpressionTypeId::Tuning);

        let slide = tracker
            .translate_mpe_channel_event(
                MidiControllerEvent {
                    sample_offset: 19,
                    channel: 2,
                    controller: MidiControllerKind::Cc(74),
                    raw_value: 127,
                },
                48,
            )
            .unwrap();
        assert_eq!(slide.note_id, 42);
        assert_eq!(slide.type_id, NoteExpressionTypeId::Brightness);
        assert_eq!(slide.normalized, 1.0);

        // Direct expression on note_id 42 is admitted.
        assert!(
            tracker
                .admit_expression(NoteExpressionEvent {
                    sample_offset: 25,
                    note_id: 42,
                    type_id: NoteExpressionTypeId::Pan,
                    normalized: 0.25,
                })
                .is_ok()
        );

        // Block 3: Note-Off releases note_id 42; further expression is refused.
        assert!(tracker.note_off(42).unwrap());
        assert_eq!(tracker.active_count(), 0);
        assert_eq!(
            tracker.admit_expression(NoteExpressionEvent {
                sample_offset: 2,
                note_id: 42,
                type_id: NoteExpressionTypeId::Brightness,
                normalized: 0.5,
            }),
            Err(MidiTranslationError::NoteNotActive)
        );
    }

    #[test]
    fn output_event_batch_enforces_ordering_and_tuning_policy_prevents_double_retuning() {
        let mut batch = OutputEventBatch::new();
        let on = PluginOutputEvent::NoteOn {
            sample_offset: 5,
            channel: 0,
            pitch: 64,
            velocity: 0.8,
            note_id: 9,
        };
        let expr = PluginOutputEvent::NoteExpression(NoteExpressionEvent {
            sample_offset: 10,
            note_id: 9,
            type_id: NoteExpressionTypeId::Tuning,
            normalized: 0.6,
        });
        let off = PluginOutputEvent::NoteOff {
            sample_offset: 20,
            channel: 0,
            pitch: 64,
            velocity: 0.0,
            note_id: 9,
        };
        batch.push(on).unwrap();
        batch.push(expr).unwrap();
        batch.push(off).unwrap();
        assert_eq!(batch.iter().collect::<Vec<_>>(), vec![on, expr, off]);

        // Out-of-order offset refuses cleanly.
        assert_eq!(batch.push(on), Err(MidiTranslationError::OutOfOrderOffset));

        // Tuning/MTS interaction: apply once via NoteExpression, delegate for MTS, disclose unsupported.
        assert_eq!(
            resolve_tuning_dispatch(
                PluginTuningCapability::NoteExpressionTuning { semitone_range: 2 },
                1.0
            )
            .unwrap(),
            TuningDispatchPolicy::ApplyNoteExpression {
                normalized: 0.5 + 1.0 / 240.0
            }
        );
        assert_eq!(
            resolve_tuning_dispatch(PluginTuningCapability::PluginInternalMts, 1.0).unwrap(),
            TuningDispatchPolicy::DelegatedToPlugin
        );
        assert_eq!(
            resolve_tuning_dispatch(PluginTuningCapability::FixedTwelveTetOnly, 1.0).unwrap(),
            TuningDispatchPolicy::UnsupportedDisclosed
        );
    }
}
