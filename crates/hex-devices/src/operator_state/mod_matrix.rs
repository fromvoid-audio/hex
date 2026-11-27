//! Version 26 Vector modulation source/target contract and fixed-slot migration.
//!
//! Exposes Vector's internal modulation sources (audio-rate oscillators a–d,
//! per-voice pitch and filter envelopes, expressions including per-note random
//! with Operator's exact splitmix64 distribution, and slot "lfo 1") alongside
//! explicit per-voice and audio-rate target eligibility rules.
//!
//! Migration from v1..v25 records every legacy fixed assignment as typed routes
//! and retires `interpolation` (`603`) and `antialias` (`604`) by forcing both
//! on for higher audio quality. When either flag was off in a pre-v26 state,
//! `quality_upgraded_from_legacy` is preserved so save flows surface an explicit
//! compatibility warning rather than altering saved projects silently.
use super::*;

pub const MAX_ROUTES: usize = 48;
/// Default FM modulation depth in turns used by catalog algorithms and new
/// `osc -> pitch` FM routes (`0.25` turns = `+25%` normalized mod-mode depth).
pub const DEFAULT_FM_DEPTH_TURNS: f32 = 0.25;
/// Maximum normalized mod-mode FM depth in turns (`±100%` maps linearly to `±1.0` turn,
/// well inside `fm4::Routing`'s `[-8.0, 8.0]` turn domain).
pub const MAX_MOD_MODE_FM_DEPTH_TURNS: f32 = 1.0;

/// Non-pitch audio-rate targets currently executed at audio rate by Vector's
/// `fm4` voice kernel. `OscPitch(_)` is executed at audio rate by `fm4::Routing`;
/// `OscLevel(_)` and `FilterFrequency` remain declared in the v26 route contract
/// for future audio-rate DSP wiring and are NOT outlined in FM mod mode today.
pub const SUPPORTED_NON_PITCH_AUDIO_RATE_TARGETS: &[VectorModTarget] = &[];

/// Map a signed normalized mod-mode depth (`-1.0..=1.0`) to FM phase-modulation
/// depth in turns using the linear law `turns = depth_norm * MAX_MOD_MODE_FM_DEPTH_TURNS`.
/// Thus `+0.25` (`+25%`) produces the exact `0.25`-turn catalog algorithm coefficient,
/// `0.0` removes the edge, and negative depths invert modulator phase.
pub fn fm_depth_turns_from_mod_depth(depth_norm: f32) -> Result<f32, VectorModError> {
    if !depth_norm.is_finite() || !(-1.0..=1.0).contains(&depth_norm) {
        return Err(VectorModError::NonFiniteOrOutOfRangeDepth);
    }
    Ok(depth_norm * MAX_MOD_MODE_FM_DEPTH_TURNS)
}

/// Inverse of [`fm_depth_turns_from_mod_depth`], clamped to `-1.0..=1.0` for UI display.
pub fn mod_depth_from_fm_depth_turns(turns: f32) -> f32 {
    if !turns.is_finite() {
        return 0.0;
    }
    (turns / MAX_MOD_MODE_FM_DEPTH_TURNS).clamp(-1.0, 1.0)
}

/// Summary of a Vector device's canonical FM `osc -> pitch` routing graph.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OperatorFmSummary {
    /// Directed FM modulation depths in turns (`matrix[source][target]`).
    pub matrix: [[f32; OPERATORS]; OPERATORS],
    /// True for each oscillator that routes audio to the output bus (modulates nothing in DAG mode).
    pub carriers: [bool; OPERATORS],
    /// Matching catalog algorithm (`Some(1..=11)`) or `None` when the graph is `"custom"`.
    pub algorithm_choice: Option<u32>,
}

impl OperatorFmSummary {
    /// True when `operator` drives at least one other oscillator's pitch.
    pub fn is_modulator(&self, operator: usize) -> bool {
        operator < OPERATORS
            && (0..OPERATORS).any(|dst| dst != operator && self.matrix[operator][dst] != 0.0)
    }

    /// Row role label: `"fm"` when modulating another oscillator, `"out"` when a carrier.
    pub fn row_role_label(&self, operator: usize) -> &'static str {
        if self.is_modulator(operator) {
            "fm"
        } else {
            "out"
        }
    }

    /// Collect all nonzero directed edges `(source, target)` in deterministic order.
    pub fn edges(&self) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for src in 0..OPERATORS {
            for dst in 0..OPERATORS {
                if src != dst && self.matrix[src][dst] != 0.0 {
                    out.push((src, dst));
                }
            }
        }
        out
    }
}

/// Header (`flags: u32`, `route_count: u32`) plus `MAX_ROUTES * 8` bytes.
pub(super) const HEADER_BYTES: usize = 8;
pub(super) const ROUTE_BYTES: usize = 8;
pub(super) const BYTES: usize = HEADER_BYTES + MAX_ROUTES * ROUTE_BYTES;

const FLAG_QUALITY_UPGRADED: u32 = 1 << 0;
const FLAG_MIGRATED_FIXED_SLOTS: u32 = 1 << 1;

/// User-facing lowercase warning surfaced when saving over a pre-v26 project
/// whose Vector interpolation or antialiasing switch was disabled.
pub const QUALITY_UPGRADE_COMPATIBILITY_WARNING: &str = "saving makes this file incompatible with older hex readers and enables vector interpolation and antialiasing (higher quality); use save as to preserve it, or explicitly confirm overwrite";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum VectorModSource {
    OscA = 0,
    OscB = 1,
    OscC = 2,
    OscD = 3,
    PitchEnv = 4,
    FilterEnv = 5,
    Velocity = 6,
    Key = 7,
    Aftertouch = 8,
    PitchBend = 9,
    ModWheel = 10,
    /// Per-note random source using Operator's exact splitmix64 uniform `[-1, 1]`
    /// distribution (`global::Settings::gains`), ensuring lossless `pan<rnd` parity.
    NoteRandom = 11,
    /// Migrated Operator LFO ("lfo 1" slot modulator).
    SlotLfo1 = 12,
}

impl VectorModSource {
    pub const fn osc(index: usize) -> Option<Self> {
        match index {
            0 => Some(Self::OscA),
            1 => Some(Self::OscB),
            2 => Some(Self::OscC),
            3 => Some(Self::OscD),
            _ => None,
        }
    }

    pub const fn osc_index(self) -> Option<u8> {
        match self {
            Self::OscA => Some(0),
            Self::OscB => Some(1),
            Self::OscC => Some(2),
            Self::OscD => Some(3),
            _ => None,
        }
    }

    pub const fn is_audio_rate(self) -> bool {
        self.osc_index().is_some()
    }

    pub const fn is_per_voice(self) -> bool {
        !matches!(self, Self::Aftertouch | Self::PitchBend | Self::ModWheel)
    }

    fn from_tag(tag: u8) -> Result<Self, DeviceContractError> {
        match tag {
            0 => Ok(Self::OscA),
            1 => Ok(Self::OscB),
            2 => Ok(Self::OscC),
            3 => Ok(Self::OscD),
            4 => Ok(Self::PitchEnv),
            5 => Ok(Self::FilterEnv),
            6 => Ok(Self::Velocity),
            7 => Ok(Self::Key),
            8 => Ok(Self::Aftertouch),
            9 => Ok(Self::PitchBend),
            10 => Ok(Self::ModWheel),
            11 => Ok(Self::NoteRandom),
            12 => Ok(Self::SlotLfo1),
            _ => Err(DeviceContractError::MalformedState),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VectorModTarget {
    OscPitch(u8),
    OscLevel(u8),
    OscVelocityLevel(u8),
    OscVelocityPitch(u8),
    OscEnvelopeTime(u8),
    FilterFrequency,
    FilterResonance,
    FilterMorph,
    LfoRate,
    LfoAmount,
    Volume,
    Pan,
    GlobalPitch,
    Time,
}

impl VectorModTarget {
    pub const fn supports_audio_rate(self) -> bool {
        matches!(
            self,
            Self::OscPitch(_) | Self::OscLevel(_) | Self::FilterFrequency
        )
    }

    /// True only for targets currently executed at audio rate by the live `fm4` kernel.
    pub const fn supports_live_audio_rate_fm_execution(self) -> bool {
        matches!(self, Self::OscPitch(_))
    }

    pub const fn supports_per_voice(self) -> bool {
        !matches!(self, Self::LfoRate | Self::LfoAmount)
    }

    fn validate_index(self) -> Result<(), VectorModError> {
        match self {
            Self::OscPitch(i)
            | Self::OscLevel(i)
            | Self::OscVelocityLevel(i)
            | Self::OscVelocityPitch(i)
            | Self::OscEnvelopeTime(i)
                if i >= OPERATORS as u8 =>
            {
                Err(VectorModError::InvalidOperatorIndex)
            }
            _ => Ok(()),
        }
    }

    fn encode(self) -> (u8, u8) {
        match self {
            Self::OscPitch(i) => (0, i),
            Self::OscLevel(i) => (1, i),
            Self::OscVelocityLevel(i) => (2, i),
            Self::OscVelocityPitch(i) => (3, i),
            Self::OscEnvelopeTime(i) => (4, i),
            Self::FilterFrequency => (5, 0),
            Self::FilterResonance => (6, 0),
            Self::FilterMorph => (7, 0),
            Self::LfoRate => (8, 0),
            Self::LfoAmount => (9, 0),
            Self::Volume => (10, 0),
            Self::Pan => (11, 0),
            Self::GlobalPitch => (12, 0),
            Self::Time => (13, 0),
        }
    }

    fn decode(tag: u8, index: u8) -> Result<Self, DeviceContractError> {
        let target = match tag {
            0 => Self::OscPitch(index),
            1 => Self::OscLevel(index),
            2 => Self::OscVelocityLevel(index),
            3 => Self::OscVelocityPitch(index),
            4 => Self::OscEnvelopeTime(index),
            5 if index == 0 => Self::FilterFrequency,
            6 if index == 0 => Self::FilterResonance,
            7 if index == 0 => Self::FilterMorph,
            8 if index == 0 => Self::LfoRate,
            9 if index == 0 => Self::LfoAmount,
            10 if index == 0 => Self::Volume,
            11 if index == 0 => Self::Pan,
            12 if index == 0 => Self::GlobalPitch,
            13 if index == 0 => Self::Time,
            _ => return Err(DeviceContractError::MalformedState),
        };
        target
            .validate_index()
            .map_err(|_| DeviceContractError::MalformedState)?;
        Ok(target)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VectorModError {
    SelfModulationUseFeedback,
    AudioRateCycleDetected,
    IneligibleAudioRateTarget,
    IneligiblePerVoiceTarget,
    IneligibleSourceTarget,
    InvalidOperatorIndex,
    NonFiniteOrOutOfRangeDepth,
    TooManyRoutes,
}

impl VectorModError {
    pub const fn reason(self) -> &'static str {
        match self {
            Self::SelfModulationUseFeedback => {
                "oscillator self-modulation must use the feedback parameter"
            }
            Self::AudioRateCycleDetected => {
                "audio-rate oscillator modulation routes must stay acyclic"
            }
            Self::IneligibleAudioRateTarget => {
                "target parameter does not support audio-rate oscillator modulation"
            }
            Self::IneligiblePerVoiceTarget => {
                "target parameter does not support per-voice modulation from this source"
            }
            Self::IneligibleSourceTarget => "source cannot modulate this target parameter",
            Self::InvalidOperatorIndex => "operator index must be in 0..4",
            Self::NonFiniteOrOutOfRangeDepth => "modulation depth must be finite and in -100..=100",
            Self::TooManyRoutes => "vector modulation route capacity reached",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VectorModRoute {
    pub source: VectorModSource,
    pub target: VectorModTarget,
    pub depth: f32,
}

const EMPTY_ROUTE: VectorModRoute = VectorModRoute {
    source: VectorModSource::Velocity,
    target: VectorModTarget::Volume,
    depth: 0.0,
};

/// Explicit eligibility check for a Vector modulation route.
pub fn check_route_eligibility(
    source: VectorModSource,
    target: VectorModTarget,
    depth: f32,
    existing: &[VectorModRoute],
) -> Result<(), VectorModError> {
    target.validate_index()?;
    if !depth.is_finite() || !(-100.0..=100.0).contains(&depth) {
        return Err(VectorModError::NonFiniteOrOutOfRangeDepth);
    }
    if let Some(src) = source.osc_index() {
        if !target.supports_audio_rate() {
            return Err(VectorModError::IneligibleAudioRateTarget);
        }
        if let VectorModTarget::OscPitch(dst) = target {
            if src == dst {
                return Err(VectorModError::SelfModulationUseFeedback);
            }
            if would_create_fm_cycle(src as usize, dst as usize, existing) {
                return Err(VectorModError::AudioRateCycleDetected);
            }
        }
        return Ok(());
    }
    if matches!(
        target,
        VectorModTarget::OscVelocityLevel(_) | VectorModTarget::OscVelocityPitch(_)
    ) && source != VectorModSource::Velocity
    {
        return Err(VectorModError::IneligibleSourceTarget);
    }
    if source == VectorModSource::SlotLfo1
        && matches!(
            target,
            VectorModTarget::LfoRate | VectorModTarget::LfoAmount
        )
    {
        return Err(VectorModError::IneligibleSourceTarget);
    }
    if matches!(
        source,
        VectorModSource::NoteRandom | VectorModSource::FilterEnv
    ) && !target.supports_per_voice()
    {
        return Err(VectorModError::IneligiblePerVoiceTarget);
    }
    Ok(())
}

fn would_create_fm_cycle(src: usize, dst: usize, existing: &[VectorModRoute]) -> bool {
    let mut adj = [[false; OPERATORS]; OPERATORS];
    for route in existing {
        if let (Some(a), VectorModTarget::OscPitch(b)) = (route.source.osc_index(), route.target)
            && (a as usize) < OPERATORS
            && (b as usize) < OPERATORS
            && route.depth != 0.0
        {
            adj[a as usize][b as usize] = true;
        }
    }
    adj[src][dst] = true;
    // Reachability from `dst` back to `src` across 4 nodes.
    let mut visited = [false; OPERATORS];
    let mut stack = [0usize; OPERATORS];
    let mut sp = 1usize;
    stack[0] = dst;
    visited[dst] = true;
    while sp > 0 {
        sp -= 1;
        let cur = stack[sp];
        for (next, &edge) in adj[cur].iter().enumerate() {
            if edge {
                if next == src {
                    return true;
                }
                if !visited[next] {
                    visited[next] = true;
                    stack[sp] = next;
                    sp += 1;
                }
            }
        }
    }
    false
}

/// Fixed-size stack-resident representation of Vector's migrated and authored
/// modulation routes plus quality-flag retirement metadata.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModMatrix {
    pub quality_upgraded_from_legacy: bool,
    pub migrated_fixed_slots: bool,
    len: u8,
    routes: [VectorModRoute; MAX_ROUTES],
}

impl Default for ModMatrix {
    fn default() -> Self {
        Self {
            quality_upgraded_from_legacy: false,
            migrated_fixed_slots: false,
            len: 0,
            routes: [EMPTY_ROUTE; MAX_ROUTES],
        }
    }
}

impl ModMatrix {
    pub fn routes(&self) -> &[VectorModRoute] {
        &self.routes[..usize::from(self.len)]
    }

    pub fn push_route(&mut self, route: VectorModRoute) -> Result<(), VectorModError> {
        check_route_eligibility(route.source, route.target, route.depth, self.routes())?;
        let index = usize::from(self.len);
        if index >= MAX_ROUTES {
            return Err(VectorModError::TooManyRoutes);
        }
        self.routes[index] = route;
        self.len += 1;
        Ok(())
    }

    /// Deterministically derive the complete migrated route list from Vector's
    /// canonical settings (algorithm/custom matrix, LFO destinations, expression
    /// controller slots, `level<vel`/`osc<vel`, `pan<key`/`pan<rnd`, pitch/filter env).
    pub(super) fn migrate_from_settings(
        settings: &OperatorSettings,
        quality_upgraded_from_legacy: bool,
    ) -> Self {
        let mut matrix = Self {
            quality_upgraded_from_legacy,
            migrated_fixed_slots: true,
            len: 0,
            routes: [EMPTY_ROUTE; MAX_ROUTES],
        };
        // 1. Algorithm choice / custom FM routing matrix -> Osc -> OscPitch routes.
        for src in 0..OPERATORS {
            for dst in 0..OPERATORS {
                if src == dst {
                    continue;
                }
                let depth = settings.fields[12 + src * OPERATORS + dst];
                if depth != 0.0
                    && let Some(source) = VectorModSource::osc(src)
                {
                    let _ = matrix.push_route(VectorModRoute {
                        source,
                        target: VectorModTarget::OscPitch(dst as u8),
                        depth,
                    });
                }
            }
        }
        // 2. Operator LFO -> Slot "lfo 1" pitch (a–d) and filter frequency routes.
        let lfo_amount = (settings.lfo.amount() * 100.0) as f32;
        if settings.lfo.active() && lfo_amount != 0.0 {
            for op in 0..OPERATORS {
                if settings.lfo.selected(op) {
                    let _ = matrix.push_route(VectorModRoute {
                        source: VectorModSource::SlotLfo1,
                        target: VectorModTarget::OscPitch(op as u8),
                        depth: lfo_amount,
                    });
                }
            }
            if settings.lfo_ext.dest_filter() {
                let _ = matrix.push_route(VectorModRoute {
                    source: VectorModSource::SlotLfo1,
                    target: VectorModTarget::FilterFrequency,
                    depth: lfo_amount,
                });
            }
        }
        // 3. In-device pitch envelope & filter envelope routes.
        if settings.pitch_env.enabled() && settings.pitch_env.amount() != 0.0 {
            let depth = settings.pitch_env.amount();
            for op in 0..OPERATORS {
                if settings.pitch_env.dest_operator(op) {
                    let _ = matrix.push_route(VectorModRoute {
                        source: VectorModSource::PitchEnv,
                        target: VectorModTarget::OscPitch(op as u8),
                        depth,
                    });
                }
            }
            if settings.pitch_env.dest_lfo() {
                let _ = matrix.push_route(VectorModRoute {
                    source: VectorModSource::PitchEnv,
                    target: VectorModTarget::LfoRate,
                    depth,
                });
            }
        }
        if settings.filter_env.amount() != 0.0 {
            let _ = matrix.push_route(VectorModRoute {
                source: VectorModSource::FilterEnv,
                target: VectorModTarget::FilterFrequency,
                depth: settings.filter_env.amount(),
            });
        }
        // 4. Controller destination slots (velocity, key, aftertouch, pitch bend, mod wheel)
        //    including fixed key -> time and pitch bend -> pitch assignments.
        let expr_sources = [
            VectorModSource::Velocity,
            VectorModSource::Key,
            VectorModSource::Aftertouch,
            VectorModSource::PitchBend,
            VectorModSource::ModWheel,
        ];
        for (row, source) in expr_sources.into_iter().enumerate() {
            for slot in 0..2 {
                let idx = row * 4 + slot * 2;
                let amount = f32::from_bits(settings.modulation.0[idx + 1]);
                if amount == 0.0 {
                    continue;
                }
                let dest_code = match (row, slot) {
                    (1, 1) => 13,
                    (3, 1) => 12,
                    _ => settings.modulation.0[idx],
                };
                if let Some(target) = controller_destination_target(dest_code) {
                    let _ = matrix.push_route(VectorModRoute {
                        source,
                        target,
                        depth: amount,
                    });
                }
            }
        }
        // 5. level<vel (74..=77) and osc<vel (358 + op*8) -> velocity mappings.
        for op in 0..OPERATORS {
            let vel_level = settings.amplitude_velocity[op];
            if vel_level != 0.0 {
                let _ = matrix.push_route(VectorModRoute {
                    source: VectorModSource::Velocity,
                    target: VectorModTarget::OscVelocityLevel(op as u8),
                    depth: vel_level * 100.0,
                });
            }
            let osc_vel = f32::from_bits(settings.companion.words[op][1]);
            if osc_vel != 0.0 {
                let _ = matrix.push_route(VectorModRoute {
                    source: VectorModSource::Velocity,
                    target: VectorModTarget::OscVelocityPitch(op as u8),
                    depth: osc_vel * 100.0,
                });
            }
        }
        // 6. pan<key -> Key -> Pan, pan<rnd -> NoteRandom -> Pan.
        if settings.global.key != 0.0 {
            let _ = matrix.push_route(VectorModRoute {
                source: VectorModSource::Key,
                target: VectorModTarget::Pan,
                depth: settings.global.key,
            });
        }
        if settings.global.random != 0.0 {
            let _ = matrix.push_route(VectorModRoute {
                source: VectorModSource::NoteRandom,
                target: VectorModTarget::Pan,
                depth: settings.global.random,
            });
        }
        matrix
    }

    pub(super) fn append(&self, out: &mut Vec<u8>) {
        let mut flags = 0_u32;
        if self.quality_upgraded_from_legacy {
            flags |= FLAG_QUALITY_UPGRADED;
        }
        if self.migrated_fixed_slots {
            flags |= FLAG_MIGRATED_FIXED_SLOTS;
        }
        out.extend_from_slice(&flags.to_le_bytes());
        out.extend_from_slice(&u32::from(self.len).to_le_bytes());
        for route in self.routes() {
            let (target_tag, target_idx) = route.target.encode();
            out.push(route.source as u8);
            out.push(target_tag);
            out.push(target_idx);
            out.push(0);
            out.extend_from_slice(&route.depth.to_le_bytes());
        }
        let padding = (MAX_ROUTES - usize::from(self.len)) * ROUTE_BYTES;
        out.extend(std::iter::repeat_n(0_u8, padding));
    }

    pub(super) fn decode(bytes: &[u8]) -> Result<Self, DeviceContractError> {
        if bytes.len() != BYTES {
            return Err(DeviceContractError::MalformedState);
        }
        let flags = u32::from_le_bytes(
            bytes[0..4]
                .try_into()
                .map_err(|_| DeviceContractError::MalformedState)?,
        );
        if (flags & !(FLAG_QUALITY_UPGRADED | FLAG_MIGRATED_FIXED_SLOTS)) != 0 {
            return Err(DeviceContractError::MalformedState);
        }
        let count = u32::from_le_bytes(
            bytes[4..8]
                .try_into()
                .map_err(|_| DeviceContractError::MalformedState)?,
        ) as usize;
        if count > MAX_ROUTES {
            return Err(DeviceContractError::MalformedState);
        }
        let used_end = HEADER_BYTES + count * ROUTE_BYTES;
        if bytes[used_end..].iter().any(|&b| b != 0) {
            return Err(DeviceContractError::MalformedState);
        }
        let mut matrix = Self {
            quality_upgraded_from_legacy: (flags & FLAG_QUALITY_UPGRADED) != 0,
            migrated_fixed_slots: (flags & FLAG_MIGRATED_FIXED_SLOTS) != 0,
            len: 0,
            routes: [EMPTY_ROUTE; MAX_ROUTES],
        };
        for chunk in bytes[HEADER_BYTES..used_end].chunks_exact(ROUTE_BYTES) {
            if chunk[3] != 0 {
                return Err(DeviceContractError::MalformedState);
            }
            let source = VectorModSource::from_tag(chunk[0])?;
            let target = VectorModTarget::decode(chunk[1], chunk[2])?;
            let depth = f32::from_le_bytes(
                chunk[4..8]
                    .try_into()
                    .map_err(|_| DeviceContractError::MalformedState)?,
            );
            matrix
                .push_route(VectorModRoute {
                    source,
                    target,
                    depth,
                })
                .map_err(|_| DeviceContractError::MalformedState)?;
        }
        Ok(matrix)
    }
}

fn controller_destination_target(code: u32) -> Option<VectorModTarget> {
    match code {
        1 => Some(VectorModTarget::OscLevel(0)),
        2 => Some(VectorModTarget::OscLevel(1)),
        3 => Some(VectorModTarget::OscLevel(2)),
        4 => Some(VectorModTarget::OscLevel(3)),
        5 => Some(VectorModTarget::FilterFrequency),
        6 => Some(VectorModTarget::FilterResonance),
        7 => Some(VectorModTarget::FilterMorph),
        8 => Some(VectorModTarget::LfoRate),
        9 => Some(VectorModTarget::LfoAmount),
        10 => Some(VectorModTarget::Volume),
        11 => Some(VectorModTarget::Pan),
        12 => Some(VectorModTarget::GlobalPitch),
        13 => Some(VectorModTarget::Time),
        _ => None,
    }
}

/// Translation of Operator's LFO settings into the shared `hex.modulator.lfo`
/// library slot format, recording any extended Operator LFO features whose
/// per-voice evaluation remains owned by Vector's runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LfoSlotMigration {
    pub slot_state: NativeDeviceState,
    pub exact_slot_parity: bool,
    pub notes: &'static str,
}

pub fn migrate_lfo_to_shared_slot(
    settings: &OperatorSettings,
) -> Result<LfoSlotMigration, DeviceContractError> {
    use crate::modulation::lfo as shared_lfo;
    let waveform_choice = match settings.lfo.read(parameter::LFO_WAVEFORM) {
        Some(ParameterValue::Choice(choice)) => choice,
        _ => 0,
    };
    let (waveform, waveform_exact) = match waveform_choice {
        0 => (shared_lfo::Waveform::Sine, true),
        1 => (shared_lfo::Waveform::Triangle, true),
        2 => (shared_lfo::Waveform::RisingRamp, true),
        3 => (shared_lfo::Waveform::Square, true),
        _ => (shared_lfo::Waveform::Sine, false),
    };
    let rate = if settings.lfo.sync() || settings.lfo_ext.is_sync() {
        shared_lfo::Rate::BeatsPerCycle(settings.lfo.beats())
    } else {
        let hz = settings.lfo.rate()
            * if settings.lfo_ext.is_high() {
                10.0
            } else {
                1.0
            };
        shared_lfo::Rate::Hertz(hz.max(0.01))
    };
    let slot_state = shared_lfo::Settings {
        rate,
        phase: 0.0,
        waveform,
    }
    .encode()?;
    let has_per_voice_extras = settings.lfo.retrigger()
        || settings.lfo_ext.rate_key() != 0.0
        || settings.lfo_ext.amount_vel() != 0.0
        || settings.lfo_ext.env_enabled();
    let (exact_slot_parity, notes) = if !waveform_exact {
        (
            false,
            "extended operator lfo waveform (sample-and-hold, saw down or noise) remains evaluated per voice in vector",
        )
    } else if has_per_voice_extras {
        (
            false,
            "per-voice lfo retrigger, rate<key, amount<vel or lfo envelope remains evaluated per voice in vector",
        )
    } else {
        (true, "exact shared lfo slot parity")
    };
    Ok(LfoSlotMigration {
        slot_state,
        exact_slot_parity,
        notes,
    })
}

/// Inspect the canonical FM `osc -> pitch` routing summary of a Vector state.
pub fn operator_fm_summary(
    state: &NativeDeviceState,
) -> Result<OperatorFmSummary, DeviceContractError> {
    let settings = OperatorSettings::from_state(state)?;
    let matrix = std::array::from_fn(|src| {
        std::array::from_fn(|dst| settings.fields[12 + src * OPERATORS + dst])
    });
    let carriers = std::array::from_fn(|op| settings.fields[28 + op] > 0.0);
    let choice = algorithms::choice(&settings.fields[12..32]);
    Ok(OperatorFmSummary {
        matrix,
        carriers,
        algorithm_choice: (choice > 0).then_some(choice),
    })
}

/// Check whether adding/activating an FM edge `source -> target` is valid (acyclic, non-self).
pub fn operator_can_map_fm_edge(
    state: &NativeDeviceState,
    source: usize,
    target: usize,
) -> Result<(), VectorModError> {
    let settings =
        OperatorSettings::from_state(state).map_err(|_| VectorModError::InvalidOperatorIndex)?;
    if source >= OPERATORS || target >= OPERATORS {
        return Err(VectorModError::InvalidOperatorIndex);
    }
    if source == target {
        return Err(VectorModError::SelfModulationUseFeedback);
    }
    let Some(src) = VectorModSource::osc(source) else {
        return Err(VectorModError::InvalidOperatorIndex);
    };
    // Removing an already-active edge is always valid; otherwise check cycle freedom.
    if settings.fields[12 + source * OPERATORS + target] != 0.0 {
        return Ok(());
    }
    check_route_eligibility(
        src,
        VectorModTarget::OscPitch(target as u8),
        DEFAULT_FM_DEPTH_TURNS,
        settings.mod_matrix.routes(),
    )
}

fn apply_fm_edge_turns(
    state: &NativeDeviceState,
    source: usize,
    target: usize,
    turns: f32,
) -> Result<NativeDeviceState, VectorModError> {
    let mut settings =
        OperatorSettings::from_state(state).map_err(|_| VectorModError::InvalidOperatorIndex)?;
    if source >= OPERATORS || target >= OPERATORS {
        return Err(VectorModError::InvalidOperatorIndex);
    }
    if source == target {
        return Err(VectorModError::SelfModulationUseFeedback);
    }
    if !turns.is_finite()
        || !(-MAX_MOD_MODE_FM_DEPTH_TURNS..=MAX_MOD_MODE_FM_DEPTH_TURNS).contains(&turns)
    {
        return Err(VectorModError::NonFiniteOrOutOfRangeDepth);
    }
    let existing = settings.fields[12 + source * OPERATORS + target];
    if turns != 0.0 && existing == 0.0 {
        let Some(src) = VectorModSource::osc(source) else {
            return Err(VectorModError::InvalidOperatorIndex);
        };
        check_route_eligibility(
            src,
            VectorModTarget::OscPitch(target as u8),
            turns,
            settings.mod_matrix.routes(),
        )?;
    }
    settings.global.active = true;
    settings.fields[12 + source * OPERATORS + target] = turns;
    // Derive carrier membership strictly from outgoing FM edges:
    // an oscillator that modulates nothing is a carrier (heard).
    for op in 0..OPERATORS {
        let modulates_any = (0..OPERATORS)
            .any(|dst| dst != op && settings.fields[12 + op * OPERATORS + dst] != 0.0);
        settings.fields[28 + op] = if modulates_any { 0.0 } else { 1.0 };
    }
    // Validate the resulting DAG through fm4::Routing before serializing.
    settings
        .kernel_parts()
        .map_err(|_| VectorModError::AudioRateCycleDetected)?;
    settings
        .to_state()
        .map_err(|_| VectorModError::AudioRateCycleDetected)
}

/// Toggle an FM `source -> target` mapping between off (`0.0`) and the catalog
/// `0.25`-turn depth in one canonical validated state transition.
pub fn operator_toggle_fm_edge(
    state: &NativeDeviceState,
    source: usize,
    target: usize,
) -> Result<NativeDeviceState, VectorModError> {
    let settings =
        OperatorSettings::from_state(state).map_err(|_| VectorModError::InvalidOperatorIndex)?;
    if source >= OPERATORS || target >= OPERATORS {
        return Err(VectorModError::InvalidOperatorIndex);
    }
    if source == target {
        return Err(VectorModError::SelfModulationUseFeedback);
    }
    let next_turns = if settings.fields[12 + source * OPERATORS + target] != 0.0 {
        0.0
    } else {
        DEFAULT_FM_DEPTH_TURNS
    };
    apply_fm_edge_turns(state, source, target, next_turns)
}

/// Set an FM `source -> target` mapping depth from a normalized mod-mode depth
/// (`-1.0..=1.0`, where `0.0` removes the edge and `+0.25` = `0.25` turns).
pub fn operator_set_fm_edge_depth(
    state: &NativeDeviceState,
    source: usize,
    target: usize,
    depth_norm: f32,
) -> Result<NativeDeviceState, VectorModError> {
    let turns = fm_depth_turns_from_mod_depth(depth_norm)?;
    apply_fm_edge_turns(state, source, target, turns)
}

/// True when `state` is a Vector (`hex.instrument.operator`) state that either
/// had `interpolation` or `antialias` disabled prior to v26, or was migrated
/// from such a state and carries the retired-quality-upgrade record.
pub fn operator_state_requires_quality_upgrade_warning(state: &NativeDeviceState) -> bool {
    if state.type_id() != OPERATOR_TYPE_ID {
        return false;
    }
    let Ok(settings) = OperatorSettings::from_state(state) else {
        return false;
    };
    if state.state_version() < MOD_MATRIX_STATE_VERSION {
        !settings.global.interpolation || !settings.global.antialias
    } else {
        settings.mod_matrix.quality_upgraded_from_legacy
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice::{
        DevicePreparationContext, NativeInstrumentEvent, NativeInstrumentRuntimeLimits,
        NativeInstrumentRuntimePair,
    };

    fn render_operator(
        state: &NativeDeviceState,
        events: &[NativeInstrumentEvent],
    ) -> Vec<[f32; 2]> {
        let tuning = hex_lattice::TuningTable::default();
        let context = DevicePreparationContext {
            sample_rate: 48_000,
            tuning: &tuning,
            resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
        };
        let prepared = OPERATOR_METADATA
            .prepare_instrument(state, &context)
            .unwrap();
        let factory = prepared
            .prepare_mutable_runtime()
            .unwrap()
            .expect("global operator runtime");
        let limits = NativeInstrumentRuntimeLimits {
            max_pair_bytes: 2 * 1024 * 1024,
            max_events_per_block: 259,
            max_active_occurrences: 64,
            max_tail_frames: 48_000 * 600,
        };
        let mut pair = NativeInstrumentRuntimePair::new(factory.as_ref(), limits).unwrap();
        let mut out = vec![[0.0_f32; 2]; 256];
        pair.begin_block();
        pair.process_block(events, &mut out, &hex_lattice::EMPTY_DEVICE_RESOURCES)
            .unwrap();
        pair.commit_block();
        out
    }

    fn note_events() -> Vec<NativeInstrumentEvent> {
        vec![
            NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 1,
                pitch: 64,
                velocity: 108,
            },
            NativeInstrumentEvent::NoteOn {
                frame_offset: 32,
                occurrence_id: 2,
                pitch: 72,
                velocity: 92,
            },
            NativeInstrumentEvent::Expression {
                frame_offset: 48,
                expression: hex_lattice::NativeInstrumentExpression::ChannelPressure(80),
            },
            NativeInstrumentEvent::Expression {
                frame_offset: 64,
                expression: hex_lattice::NativeInstrumentExpression::PitchBend(4096),
            },
            NativeInstrumentEvent::Expression {
                frame_offset: 80,
                expression: hex_lattice::NativeInstrumentExpression::ModWheel(96),
            },
        ]
    }

    fn v25_from(settings: &OperatorSettings) -> NativeDeviceState {
        NativeDeviceState::new(
            OPERATOR_TYPE_ID,
            LFO_EXT_STATE_VERSION,
            tlv_codec::encode_v25(settings).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn v25_migration_preserves_exact_pcm_across_all_algorithms_custom_dag_and_fixed_slots() {
        let events = note_events();
        // 1. Every catalog algorithm (1..=11) renders identically before and after v25 -> v26 migration.
        for choice in 1..=11_u32 {
            let mut state = OPERATOR_METADATA.default_state().unwrap();
            for (id, value) in [
                (parameter::ALGORITHM, ParameterValue::Choice(choice)),
                (parameter::oscillator_level(1), ParameterValue::Float(0.8)),
                (parameter::oscillator_level(2), ParameterValue::Float(0.65)),
                (parameter::oscillator_level(3), ParameterValue::Float(0.5)),
                (parameter::oscillator_level(4), ParameterValue::Float(0.4)),
            ] {
                state = OPERATOR_METADATA
                    .patch_parameter(&state, id, value)
                    .unwrap();
            }
            let settings = OperatorSettings::from_state(&state).unwrap();
            let v25 = v25_from(&settings);
            let migrated = OPERATOR_METADATA.migrate_state(&v25).unwrap();
            assert_eq!(migrated.state_version(), OPERATOR_STATE_VERSION);
            assert!(!operator_state_requires_quality_upgrade_warning(&migrated));
            let decoded = OperatorSettings::from_state(&migrated).unwrap();
            assert!(decoded.mod_matrix.migrated_fixed_slots);
            let fm_routes: Vec<_> = decoded
                .mod_matrix
                .routes()
                .iter()
                .filter(|r| r.source.is_audio_rate())
                .collect();
            assert_eq!(
                fm_routes.len(),
                algorithms::CATALOG[choice as usize - 1].edges.len()
            );
            assert_eq!(
                render_operator(&v25, &events),
                render_operator(&migrated, &events),
                "algorithm {choice} PCM must be bit-identical after v25 -> v26 migration"
            );
        }

        // 2. Custom DAG routing + LFO + all expression slots + level<vel/osc<vel + pan<key/pan<rnd.
        let mut rich = OperatorSettings::defaults();
        rich.fields[12..32].fill(0.0);
        rich.fields[25] = 0.35; // d -> b (12 + 3*4 + 1)
        rich.fields[16] = 0.25; // b -> a (12 + 1*4 + 0)
        rich.fields[20] = 0.15; // c -> a (12 + 2*4 + 0)
        rich.fields[28] = 1.0; // a carrier
        let mut state = rich.to_state().unwrap();
        for (id, value) in [
            (parameter::oscillator_level(1), ParameterValue::Float(0.8)),
            (parameter::oscillator_level(2), ParameterValue::Float(0.6)),
            (parameter::oscillator_level(3), ParameterValue::Float(0.5)),
            (parameter::oscillator_level(4), ParameterValue::Float(0.4)),
            (parameter::LFO_ENABLED, ParameterValue::Bool(true)),
            (parameter::LFO_RATE, ParameterValue::Float(5.5)),
            (parameter::LFO_AMOUNT, ParameterValue::Float(24.0)),
            (parameter::LFO_DEST_FILTER, ParameterValue::Bool(true)),
            (ParameterId::from_static(20_001), ParameterValue::Bool(true)),
            (parameter::amplitude_velocity(1), ParameterValue::Float(0.5)),
            (ParameterId::from_static(358), ParameterValue::Float(0.25)), // op1 osc<vel
            (parameter::PAN_KEY, ParameterValue::Float(40.0)),
            (parameter::PAN_RANDOM, ParameterValue::Float(30.0)),
            (ParameterId::from_static(620), ParameterValue::Choice(5)), // vel -> filter freq
            (ParameterId::from_static(621), ParameterValue::Float(35.0)),
            (ParameterId::from_static(624), ParameterValue::Choice(11)), // key -> pan
            (ParameterId::from_static(625), ParameterValue::Float(-20.0)),
            (ParameterId::from_static(627), ParameterValue::Float(15.0)), // key -> time
            (ParameterId::from_static(628), ParameterValue::Choice(1)), // aftertouch -> osc a level
            (ParameterId::from_static(629), ParameterValue::Float(25.0)),
            (ParameterId::from_static(635), ParameterValue::Float(7.0)), // pitch bend -> pitch
            (ParameterId::from_static(636), ParameterValue::Choice(8)),  // mod wheel -> lfo rate
            (ParameterId::from_static(637), ParameterValue::Float(50.0)),
        ] {
            state = OPERATOR_METADATA
                .patch_parameter(&state, id, value)
                .unwrap();
        }
        let settings = OperatorSettings::from_state(&state).unwrap();
        let v25 = v25_from(&settings);
        let migrated = OPERATOR_METADATA.migrate_state(&v25).unwrap();
        let decoded = OperatorSettings::from_state(&migrated).unwrap();
        let routes = decoded.mod_matrix.routes();
        assert!(routes.iter().any(|r| {
            r.source == VectorModSource::OscD && r.target == VectorModTarget::OscPitch(1)
        }));
        assert!(routes.iter().any(|r| {
            r.source == VectorModSource::SlotLfo1 && r.target == VectorModTarget::FilterFrequency
        }));
        assert!(routes.iter().any(|r| {
            r.source == VectorModSource::Velocity
                && r.target == VectorModTarget::OscVelocityLevel(0)
        }));
        assert!(routes.iter().any(|r| {
            r.source == VectorModSource::Velocity
                && r.target == VectorModTarget::OscVelocityPitch(0)
        }));
        assert!(
            routes
                .iter()
                .any(|r| { r.source == VectorModSource::Key && r.target == VectorModTarget::Pan })
        );
        assert!(routes.iter().any(|r| {
            r.source == VectorModSource::NoteRandom && r.target == VectorModTarget::Pan
        }));
        assert!(
            routes
                .iter()
                .any(|r| { r.source == VectorModSource::Key && r.target == VectorModTarget::Time })
        );
        assert!(routes.iter().any(|r| {
            r.source == VectorModSource::PitchBend && r.target == VectorModTarget::GlobalPitch
        }));
        assert_eq!(
            render_operator(&v25, &events),
            render_operator(&migrated, &events),
            "rich custom DAG + fixed-slot fixture must render bit-identically after migration"
        );
    }

    #[test]
    fn retired_interpolation_and_antialias_force_on_with_explicit_warning() {
        let mut settings = OperatorSettings::defaults();
        settings.global.interpolation = false;
        settings.global.antialias = false;
        let v25 = v25_from(&settings);
        assert!(operator_state_requires_quality_upgrade_warning(&v25));

        let migrated = OPERATOR_METADATA.migrate_state(&v25).unwrap();
        assert_eq!(migrated.state_version(), OPERATOR_STATE_VERSION);
        assert!(operator_state_requires_quality_upgrade_warning(&migrated));
        let decoded = OperatorSettings::from_state(&migrated).unwrap();
        assert!(decoded.global.interpolation);
        assert!(decoded.global.antialias);
        assert!(decoded.mod_matrix.quality_upgraded_from_legacy);

        // Round-trip preserves the quality-upgrade record cleanly.
        let reopened = OperatorSettings::from_state(&decoded.to_state().unwrap()).unwrap();
        assert_eq!(reopened, decoded);
        // Tag 22 is rejected on pre-v26 states.
        assert!(tlv_codec::decode(migrated.payload(), LFO_EXT_STATE_VERSION).is_err());
    }

    #[test]
    fn route_eligibility_declares_audio_rate_per_voice_and_cycle_refusals() {
        let existing = [
            VectorModRoute {
                source: VectorModSource::OscD,
                target: VectorModTarget::OscPitch(2),
                depth: 0.25,
            },
            VectorModRoute {
                source: VectorModSource::OscC,
                target: VectorModTarget::OscPitch(1),
                depth: 0.25,
            },
        ];
        // Valid acyclic FM & audio-rate targets.
        assert!(
            check_route_eligibility(
                VectorModSource::OscB,
                VectorModTarget::OscPitch(0),
                0.25,
                &existing
            )
            .is_ok()
        );
        assert!(
            check_route_eligibility(
                VectorModSource::OscA,
                VectorModTarget::FilterFrequency,
                20.0,
                &existing
            )
            .is_ok()
        );
        // Self-FM refused -> must use feedback.
        assert_eq!(
            check_route_eligibility(
                VectorModSource::OscA,
                VectorModTarget::OscPitch(0),
                0.25,
                &existing
            ),
            Err(VectorModError::SelfModulationUseFeedback)
        );
        // Cycle B -> D refused (D -> C -> B -> D).
        assert_eq!(
            check_route_eligibility(
                VectorModSource::OscB,
                VectorModTarget::OscPitch(3),
                0.25,
                &existing
            ),
            Err(VectorModError::AudioRateCycleDetected)
        );
        // Non-audio-rate target for oscillator source refused.
        assert_eq!(
            check_route_eligibility(VectorModSource::OscA, VectorModTarget::Time, 10.0, &[]),
            Err(VectorModError::IneligibleAudioRateTarget)
        );
        // Per-voice-only source targeting device-level LFO rate refused.
        assert_eq!(
            check_route_eligibility(
                VectorModSource::NoteRandom,
                VectorModTarget::LfoRate,
                10.0,
                &[]
            ),
            Err(VectorModError::IneligiblePerVoiceTarget)
        );
        // Self-modulating SlotLfo1 -> LfoRate refused.
        assert_eq!(
            check_route_eligibility(
                VectorModSource::SlotLfo1,
                VectorModTarget::LfoRate,
                10.0,
                &[]
            ),
            Err(VectorModError::IneligibleSourceTarget)
        );
        // Non-finite / out-of-range depth refused.
        assert_eq!(
            check_route_eligibility(
                VectorModSource::Velocity,
                VectorModTarget::Pan,
                f32::NAN,
                &[]
            ),
            Err(VectorModError::NonFiniteOrOutOfRangeDepth)
        );
        assert_eq!(
            check_route_eligibility(VectorModSource::Velocity, VectorModTarget::Pan, 101.0, &[]),
            Err(VectorModError::NonFiniteOrOutOfRangeDepth)
        );
    }

    #[test]
    fn operator_lfo_translates_to_valid_shared_lfo_slot_with_explicit_parity_notes() {
        let mut state = OPERATOR_METADATA.default_state().unwrap();
        state = OPERATOR_METADATA
            .patch_parameter(&state, parameter::LFO_ENABLED, ParameterValue::Bool(true))
            .unwrap();
        state = OPERATOR_METADATA
            .patch_parameter(&state, parameter::LFO_WAVEFORM, ParameterValue::Choice(1))
            .unwrap();
        state = OPERATOR_METADATA
            .patch_parameter(&state, parameter::LFO_RATE, ParameterValue::Float(4.0))
            .unwrap();
        state = OPERATOR_METADATA
            .patch_parameter(
                &state,
                parameter::LFO_RETRIGGER,
                ParameterValue::Bool(false),
            )
            .unwrap();
        let settings = OperatorSettings::from_state(&state).unwrap();
        let migrated = migrate_lfo_to_shared_slot(&settings).unwrap();
        assert!(migrated.exact_slot_parity);
        let registry = crate::native_modulator_registry().unwrap();
        registry
            .definition(migrated.slot_state.type_id())
            .expect("registered shared lfo modulator")
            .validate_state(&migrated.slot_state)
            .unwrap();

        // Extended waveform (saw down = 5) still produces a valid shared slot and flags parity note.
        let extended = OPERATOR_METADATA
            .patch_parameter(&state, parameter::LFO_WAVEFORM, ParameterValue::Choice(5))
            .unwrap();
        let ext_settings = OperatorSettings::from_state(&extended).unwrap();
        let ext_migrated = migrate_lfo_to_shared_slot(&ext_settings).unwrap();
        assert!(!ext_migrated.exact_slot_parity);
        registry
            .definition(ext_migrated.slot_state.type_id())
            .unwrap()
            .validate_state(&ext_migrated.slot_state)
            .unwrap();
    }

    #[test]
    fn fm_as_modulation_depth_law_cycle_refusal_carrier_derivation_and_custom_dag_render() {
        // 1. Documented linear depth law: +25% -> 0.25 turns (catalog coefficient), +/-100% -> +/-1.0 turn.
        assert_eq!(
            fm_depth_turns_from_mod_depth(0.25),
            Ok(DEFAULT_FM_DEPTH_TURNS)
        );
        assert_eq!(fm_depth_turns_from_mod_depth(-0.5), Ok(-0.5));
        assert_eq!(mod_depth_from_fm_depth_turns(0.25), 0.25);
        assert!(fm_depth_turns_from_mod_depth(f32::NAN).is_err());
        assert!(fm_depth_turns_from_mod_depth(1.5).is_err());
        assert!(SUPPORTED_NON_PITCH_AUDIO_RATE_TARGETS.is_empty());
        assert!(VectorModTarget::OscPitch(0).supports_live_audio_rate_fm_execution());
        assert!(!VectorModTarget::OscLevel(0).supports_live_audio_rate_fm_execution());
        assert!(!VectorModTarget::FilterFrequency.supports_live_audio_rate_fm_execution());

        // 2. Default state is Algorithm 1 (3->2->1->0, only 0 is carrier).
        let mut state = OPERATOR_METADATA.default_state().unwrap();
        for op in 1..=4 {
            state = OPERATOR_METADATA
                .patch_parameter(
                    &state,
                    parameter::oscillator_level(op),
                    ParameterValue::Float(0.6),
                )
                .unwrap();
        }
        let summary = operator_fm_summary(&state).unwrap();
        assert_eq!(summary.algorithm_choice, Some(1));
        assert_eq!(summary.carriers, [true, false, false, false]);
        assert_eq!(summary.row_role_label(0), "out");
        assert_eq!(summary.row_role_label(1), "fm");

        // 3. Cycle refusal & self-modulation refusal.
        assert_eq!(
            operator_can_map_fm_edge(&state, 0, 0),
            Err(VectorModError::SelfModulationUseFeedback)
        );
        assert_eq!(
            operator_toggle_fm_edge(&state, 0, 0),
            Err(VectorModError::SelfModulationUseFeedback)
        );
        // 0 -> 3 would close 3 -> 2 -> 1 -> 0 -> 3 cycle.
        assert_eq!(
            operator_can_map_fm_edge(&state, 0, 3),
            Err(VectorModError::AudioRateCycleDetected)
        );
        assert_eq!(
            operator_toggle_fm_edge(&state, 0, 3),
            Err(VectorModError::AudioRateCycleDetected)
        );
        // 1 -> 2 would close 2 -> 1 -> 2 cycle.
        assert_eq!(
            operator_can_map_fm_edge(&state, 1, 2),
            Err(VectorModError::AudioRateCycleDetected)
        );

        // 4. Toggle existing edge 2 -> 1 off => creates Algorithm 8 (3->2, 1->0, carriers [0, 2])!
        let algo8 = operator_toggle_fm_edge(&state, 2, 1).unwrap();
        let algo8_summary = operator_fm_summary(&algo8).unwrap();
        assert_eq!(algo8_summary.algorithm_choice, Some(8));
        assert_eq!(algo8_summary.carriers, [true, false, true, false]);
        assert_eq!(algo8_summary.row_role_label(2), "out");

        // 5. Add non-catalog custom edge 0 -> 2 (still acyclic: 3->2, 1->0->2; carrier only 2) => "custom".
        let custom = operator_toggle_fm_edge(&algo8, 0, 2).unwrap();
        let custom_summary = operator_fm_summary(&custom).unwrap();
        assert_eq!(custom_summary.algorithm_choice, None);
        assert_eq!(custom_summary.carriers, [false, false, true, false]);
        assert_eq!(custom_summary.matrix[0][2], DEFAULT_FM_DEPTH_TURNS);
        // Now 2 -> 1 would close 1 -> 0 -> 2 -> 1 cycle and must be refused!
        assert_eq!(
            operator_can_map_fm_edge(&custom, 2, 1),
            Err(VectorModError::AudioRateCycleDetected)
        );

        // 6. Setting custom FM depth (+60% => 0.60 turns) alters rendered PCM and survives save/reopen.
        let deeper = operator_set_fm_edge_depth(&custom, 0, 2, 0.6).unwrap();
        let deeper_summary = operator_fm_summary(&deeper).unwrap();
        assert!((deeper_summary.matrix[0][2] - 0.6).abs() < 1e-6);
        let events = note_events();
        let pcm_custom = render_operator(&custom, &events);
        let pcm_deeper = render_operator(&deeper, &events);
        assert_ne!(
            pcm_custom, pcm_deeper,
            "FM depth change must audibly change rendered PCM"
        );
        // Save/reopen roundtrip preserves custom DAG & depth identically.
        let reopened = OperatorSettings::from_state(&deeper)
            .unwrap()
            .to_state()
            .unwrap();
        assert_eq!(operator_fm_summary(&reopened).unwrap(), deeper_summary);
        assert_eq!(render_operator(&reopened, &events), pcm_deeper);

        // 7. Selecting a catalog algorithm preset replaces all custom osc -> pitch mappings cleanly.
        let reset = OPERATOR_METADATA
            .patch_parameter(&deeper, parameter::ALGORITHM, ParameterValue::Choice(11))
            .unwrap();
        let reset_summary = operator_fm_summary(&reset).unwrap();
        assert_eq!(reset_summary.algorithm_choice, Some(11));
        assert!(reset_summary.edges().is_empty());
        assert_eq!(reset_summary.carriers, [true; 4]);
    }
}
