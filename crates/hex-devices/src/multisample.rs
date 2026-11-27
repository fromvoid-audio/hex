//! Bounded resident multisample key-zone preparation and playback runtime.
//!
//! Maps MIDI keys `0..=127` across multiple host-leased resident audio sources
//! (`hex_lattice::DeviceResources`) with explicit key ranges, root pitch,
//! detune, deterministic overlap selection (`FirstMatch`, `HighestRootProximity`,
//! `LayerAll`), original-rate/tuning resampling, and fixed inline voice and
//! event limits.
//!
//! # Ownership & Safety
//! - Zero PCM ownership, zero `Arc` clones, zero `unsafe`, zero callback
//!   allocations or drops.
//! - Every zone stores only a non-owning `ZoneSourceKey` verified against the
//!   borrowed `DeviceResources` inventory on each block. Missing, changed, or
//!   stale sources fail preflight cleanly with `SourceUnavailable`, output
//!   silence, and preserve existing voice state.
//! - Does not extend `hex.instrument.sampler` silently or expose a full
//!   multisample instrument, disk streaming, persistence schema, or UI.

use hex_lattice::{
    DeviceResourceIdentity, DeviceResourcePcm, DeviceResourceView, DeviceResources,
    NativeInstrumentEvent, StereoFrame, TuningTable,
};
use hex_lattice_contract::{DeviceAssetSlot, MAX_DEVICE_ASSET_REFERENCES};

pub const MAX_ZONES: usize = 64;
pub const MAX_LAYERED_ZONES_PER_KEY: usize = 8;
pub const MAX_VOICES: usize = 64;
pub const MAX_EVENTS_PER_BLOCK: usize = 259;
pub const MIN_SAMPLE_RATE: u32 = 8_000;
pub const MAX_SAMPLE_RATE: u32 = 192_000;
pub const MAX_TOTAL_SOURCE_FRAMES: usize = 8 * 48_000 * 60;
pub const MAX_TOTAL_SOURCE_BYTES: usize = MAX_TOTAL_SOURCE_FRAMES * std::mem::size_of::<[f32; 2]>();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MultisampleError {
    InvalidLimits,
    InvalidSampleRate,
    EmptyZoneMap,
    TooManyZones,
    InvalidZoneSpec,
    OverlapLayerLimitExceeded,
    MissingSource,
    InvalidSource,
    NonFiniteSample,
    MemoryLimitExceeded,
    InvalidEventBatch,
    EventCapacityExceeded,
    VoiceCapacityExceeded,
    SourceUnavailable,
}

/// Deterministic policy when multiple zones include the same MIDI key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZoneOverlapPolicy {
    /// Lowest zone index covering the key wins.
    FirstMatch,
    /// Zone whose `root_key` is closest in semitones to the played key wins;
    /// ties break to the lowest zone index.
    HighestRootProximity,
    /// Every zone covering the key layers simultaneously (up to
    /// `MAX_LAYERED_ZONES_PER_KEY`). Preflight verifies all layered voices fit
    /// atomically before admitting a note.
    LayerAll,
}

/// Explicit memory, zone, voice, and event ceilings enforced at control
/// preparation and callback preflight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MultisampleLimits {
    pub max_zones: usize,
    pub max_voices: usize,
    pub max_events_per_block: usize,
    pub max_total_frames: usize,
    pub max_total_bytes: usize,
}

impl MultisampleLimits {
    pub const DEFAULT: Self = Self {
        max_zones: MAX_ZONES,
        max_voices: MAX_VOICES,
        max_events_per_block: MAX_EVENTS_PER_BLOCK,
        max_total_frames: MAX_TOTAL_SOURCE_FRAMES,
        max_total_bytes: MAX_TOTAL_SOURCE_BYTES,
    };

    pub const fn validate(self) -> Result<(), MultisampleError> {
        if self.max_zones == 0
            || self.max_zones > MAX_ZONES
            || self.max_zones > MAX_DEVICE_ASSET_REFERENCES
            || self.max_voices == 0
            || self.max_voices > MAX_VOICES
            || self.max_events_per_block == 0
            || self.max_events_per_block > MAX_EVENTS_PER_BLOCK
            || self.max_total_frames == 0
            || self.max_total_frames > MAX_TOTAL_SOURCE_FRAMES
            || self.max_total_bytes == 0
            || self.max_total_bytes > MAX_TOTAL_SOURCE_BYTES
        {
            return Err(MultisampleError::InvalidLimits);
        }
        Ok(())
    }
}

/// Authored key-zone definition binding a MIDI key span to one host resource slot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeyZoneSpec {
    pub slot: DeviceAssetSlot,
    pub low_key: u8,
    pub high_key: u8,
    pub root_key: u8,
    pub detune_cents: f32,
    pub gain: f32,
    pub release_seconds: f32,
}

impl KeyZoneSpec {
    pub fn validate(self) -> Result<(), MultisampleError> {
        if self.low_key > 127
            || self.high_key > 127
            || self.low_key > self.high_key
            || self.root_key > 127
            || !self.detune_cents.is_finite()
            || !(-100.0..=100.0).contains(&self.detune_cents)
            || !self.gain.is_finite()
            || !(0.0..=2.0).contains(&self.gain)
            || !self.release_seconds.is_finite()
            || !(0.0..=10.0).contains(&self.release_seconds)
        {
            return Err(MultisampleError::InvalidZoneSpec);
        }
        Ok(())
    }

    pub const fn contains_key(self, key: u8) -> bool {
        key >= self.low_key && key <= self.high_key
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ZoneSourceKey {
    slot: DeviceAssetSlot,
    identity: DeviceResourceIdentity,
    sample_rate: u32,
    frames: usize,
    is_stereo: bool,
}

impl ZoneSourceKey {
    pub const fn slot(self) -> DeviceAssetSlot {
        self.slot
    }

    pub const fn identity(self) -> DeviceResourceIdentity {
        self.identity
    }

    pub const fn sample_rate(self) -> u32 {
        self.sample_rate
    }

    pub const fn frames(self) -> usize {
        self.frames
    }

    pub fn matches(self, view: DeviceResourceView<'_>) -> bool {
        let is_stereo = matches!(view.pcm, DeviceResourcePcm::Stereo(_));
        self.slot == view.slot
            && self.identity == view.identity
            && self.sample_rate == view.sample_rate
            && self.frames == view.pcm.frames()
            && self.is_stereo == is_stereo
    }
}

#[derive(Clone, Copy, Debug)]
struct PreparedZone {
    spec: KeyZoneSpec,
    source: ZoneSourceKey,
    release_frames: u64,
    key_steps: [f64; 128],
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct KeySelection {
    len: u8,
    zones: [u8; MAX_LAYERED_ZONES_PER_KEY],
}

impl KeySelection {
    fn slice(&self) -> &[u8] {
        &self.zones[..usize::from(self.len)]
    }
}

/// Control-prepared immutable multisample key-zone map. Owns no PCM buffers.
#[derive(Clone, Debug)]
pub struct PreparedMultisampleMap {
    output_sample_rate: u32,
    overlap_policy: ZoneOverlapPolicy,
    limits: MultisampleLimits,
    zones: Box<[PreparedZone]>,
    key_map: [KeySelection; 128],
    unique_source_frames: usize,
    unique_source_bytes: usize,
}

impl PreparedMultisampleMap {
    /// Validate every zone, bind its exact host resource identity, scan unique
    /// resident PCM for finiteness, enforce memory/zone limits, and precompute
    /// O(1) key-zone selection and rate/tuning pitch steps on the control side.
    pub fn prepare_on_control(
        zones: &[KeyZoneSpec],
        resources: &dyn DeviceResources,
        output_sample_rate: u32,
        tuning: &TuningTable,
        overlap_policy: ZoneOverlapPolicy,
        limits: MultisampleLimits,
    ) -> Result<Self, MultisampleError> {
        limits.validate()?;
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&output_sample_rate) {
            return Err(MultisampleError::InvalidSampleRate);
        }
        if zones.is_empty() {
            return Err(MultisampleError::EmptyZoneMap);
        }
        if zones.len() > limits.max_zones {
            return Err(MultisampleError::TooManyZones);
        }

        let mut prepared = Vec::new();
        prepared
            .try_reserve_exact(zones.len())
            .map_err(|_| MultisampleError::MemoryLimitExceeded)?;
        let mut accounted: Vec<ZoneSourceKey> = Vec::new();
        accounted
            .try_reserve_exact(zones.len())
            .map_err(|_| MultisampleError::MemoryLimitExceeded)?;
        let mut unique_source_frames = 0_usize;
        let mut unique_source_bytes = 0_usize;

        for &spec in zones {
            spec.validate()?;
            let view = resources
                .slot(spec.slot)
                .ok_or(MultisampleError::MissingSource)?;
            if view.identity.derivation_version == 0
                || !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&view.sample_rate)
                || view.pcm.frames() == 0
            {
                return Err(MultisampleError::InvalidSource);
            }
            let is_stereo = matches!(view.pcm, DeviceResourcePcm::Stereo(_));
            let source = ZoneSourceKey {
                slot: view.slot,
                identity: view.identity,
                sample_rate: view.sample_rate,
                frames: view.pcm.frames(),
                is_stereo,
            };
            if !accounted.contains(&source) {
                let bytes_per_frame = if is_stereo {
                    std::mem::size_of::<[f32; 2]>()
                } else {
                    std::mem::size_of::<f32>()
                };
                let bytes = source
                    .frames
                    .checked_mul(bytes_per_frame)
                    .ok_or(MultisampleError::MemoryLimitExceeded)?;
                unique_source_frames = unique_source_frames
                    .checked_add(source.frames)
                    .filter(|&total| total <= limits.max_total_frames)
                    .ok_or(MultisampleError::MemoryLimitExceeded)?;
                unique_source_bytes = unique_source_bytes
                    .checked_add(bytes)
                    .filter(|&total| total <= limits.max_total_bytes)
                    .ok_or(MultisampleError::MemoryLimitExceeded)?;
                let finite = match view.pcm {
                    DeviceResourcePcm::Mono(pcm) => pcm.iter().all(|s| s.is_finite()),
                    DeviceResourcePcm::Stereo(pcm) => pcm.iter().flatten().all(|s| s.is_finite()),
                };
                if !finite {
                    return Err(MultisampleError::NonFiniteSample);
                }
                accounted.push(source);
            }

            let root_hz = tuning.frequencies()[usize::from(spec.root_key)];
            if !root_hz.is_finite() || root_hz <= 0.0 {
                return Err(MultisampleError::InvalidZoneSpec);
            }
            let detune_ratio = 2.0_f64.powf(f64::from(spec.detune_cents) / 1200.0);
            let rate_ratio = f64::from(source.sample_rate) / f64::from(output_sample_rate);
            let mut key_steps = [0.0_f64; 128];
            for key in spec.low_key..=spec.high_key {
                let key_hz = tuning.frequencies()[usize::from(key)];
                if !key_hz.is_finite() || key_hz <= 0.0 {
                    return Err(MultisampleError::InvalidZoneSpec);
                }
                let step = (key_hz / root_hz) * detune_ratio * rate_ratio;
                if !step.is_finite() || step <= 0.0 {
                    return Err(MultisampleError::InvalidZoneSpec);
                }
                key_steps[usize::from(key)] = step;
            }
            let release_frames =
                (f64::from(spec.release_seconds) * f64::from(output_sample_rate)).round() as u64;
            prepared.push(PreparedZone {
                spec,
                source,
                release_frames,
                key_steps,
            });
        }

        let mut key_map = [KeySelection::default(); 128];
        for (key, selection) in key_map.iter_mut().enumerate() {
            let key_u8 = key as u8;
            match overlap_policy {
                ZoneOverlapPolicy::FirstMatch => {
                    if let Some(index) = prepared
                        .iter()
                        .position(|zone| zone.spec.contains_key(key_u8))
                    {
                        selection.zones[0] = index as u8;
                        selection.len = 1;
                    }
                }
                ZoneOverlapPolicy::HighestRootProximity => {
                    let mut best: Option<(usize, u8)> = None;
                    for (index, zone) in prepared.iter().enumerate() {
                        if zone.spec.contains_key(key_u8) {
                            let dist = key_u8.abs_diff(zone.spec.root_key);
                            if best.is_none_or(|(_, best_dist)| dist < best_dist) {
                                best = Some((index, dist));
                            }
                        }
                    }
                    if let Some((index, _)) = best {
                        selection.zones[0] = index as u8;
                        selection.len = 1;
                    }
                }
                ZoneOverlapPolicy::LayerAll => {
                    let mut count = 0_usize;
                    for (index, zone) in prepared.iter().enumerate() {
                        if zone.spec.contains_key(key_u8) {
                            if count >= MAX_LAYERED_ZONES_PER_KEY {
                                return Err(MultisampleError::OverlapLayerLimitExceeded);
                            }
                            selection.zones[count] = index as u8;
                            count += 1;
                        }
                    }
                    selection.len = count as u8;
                }
            }
        }

        Ok(Self {
            output_sample_rate,
            overlap_policy,
            limits,
            zones: prepared.into_boxed_slice(),
            key_map,
            unique_source_frames,
            unique_source_bytes,
        })
    }

    pub const fn output_sample_rate(&self) -> u32 {
        self.output_sample_rate
    }

    pub const fn overlap_policy(&self) -> ZoneOverlapPolicy {
        self.overlap_policy
    }

    pub const fn limits(&self) -> MultisampleLimits {
        self.limits
    }

    pub fn zone_count(&self) -> usize {
        self.zones.len()
    }

    pub const fn unique_source_frames(&self) -> usize {
        self.unique_source_frames
    }

    pub const fn unique_source_bytes(&self) -> usize {
        self.unique_source_bytes
    }

    /// Deterministic zone indices selected for MIDI key `0..=127` (empty when unmapped).
    pub fn selected_zones_for_key(&self, key: u8) -> &[u8] {
        self.key_map
            .get(usize::from(key))
            .map_or(&[], KeySelection::slice)
    }

    /// Pitch- and rate-adjusted source frame increment for `zone_index` at `key`.
    pub fn step_for_zone_and_key(&self, zone_index: usize, key: u8) -> Option<f64> {
        let zone = self.zones.get(zone_index)?;
        if !zone.spec.contains_key(key) {
            return None;
        }
        Some(zone.key_steps[usize::from(key)])
    }

    pub fn source_key(&self, zone_index: usize) -> Option<ZoneSourceKey> {
        self.zones.get(zone_index).map(|zone| zone.source)
    }

    pub fn create_runtime(&self) -> MultisampleRuntime {
        MultisampleRuntime {
            voices: [Voice::default(); MAX_VOICES],
            max_voices: self.limits.max_voices,
            max_events: self.limits.max_events_per_block,
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Voice {
    occurrence_id: u64,
    zone_index: u8,
    velocity: u8,
    position: f64,
    step: f64,
    gain: f64,
    age: u64,
    gate_age: Option<u64>,
    release_frames: u64,
    source_frames: usize,
}

impl Voice {
    fn advance_metadata(&mut self, frames: u64) {
        if self.occurrence_id == 0 {
            return;
        }
        self.age = self.age.saturating_add(frames);
        self.position += self.step * (frames as f64);
        let release_done = self
            .gate_age
            .is_some_and(|gate| self.age.saturating_sub(gate) >= self.release_frames);
        let exhausted = self.position >= (self.source_frames as f64);
        if release_done || exhausted {
            *self = Self::default();
        }
    }
}

/// Fixed-capacity inline multi-zone playback runtime. Zero heap allocations.
pub struct MultisampleRuntime {
    voices: [Voice; MAX_VOICES],
    max_voices: usize,
    max_events: usize,
}

impl MultisampleRuntime {
    pub fn reset(&mut self) {
        self.voices = [Voice::default(); MAX_VOICES];
    }

    pub fn is_quiescent(&self) -> bool {
        self.voices[..self.max_voices]
            .iter()
            .all(|voice| voice.occurrence_id == 0)
    }

    pub fn active_voice_count(&self) -> usize {
        self.voices[..self.max_voices]
            .iter()
            .filter(|voice| voice.occurrence_id != 0)
            .count()
    }

    /// Read-only preflight verifying event ordering, bounds, voice capacity,
    /// and host resource availability before mutating any voice state.
    pub fn preflight_block(
        &self,
        map: &PreparedMultisampleMap,
        events: &[NativeInstrumentEvent],
        frame_count: usize,
        resources: &dyn DeviceResources,
    ) -> Result<(), MultisampleError> {
        if events.len() > self.max_events {
            return Err(MultisampleError::EventCapacityExceeded);
        }
        let mut voices = self.voices;
        let max_voices = self.max_voices;
        let mut prev_offset = 0_usize;
        for &event in events {
            let offset = event.frame_offset();
            if offset < prev_offset || offset >= frame_count {
                return Err(MultisampleError::InvalidEventBatch);
            }
            let delta = (offset - prev_offset) as u64;
            if delta > 0 {
                for voice in &mut voices[..max_voices] {
                    voice.advance_metadata(delta);
                }
            }
            Self::apply_event(map, &mut voices[..max_voices], event, resources)?;
            prev_offset = offset;
        }
        // Verify every active voice's bound host resource is still valid and unchanged.
        for voice in &voices[..max_voices] {
            if voice.occurrence_id != 0 {
                let zone = map
                    .zones
                    .get(usize::from(voice.zone_index))
                    .ok_or(MultisampleError::SourceUnavailable)?;
                if resources
                    .slot(zone.source.slot)
                    .is_none_or(|view| !zone.source.matches(view))
                {
                    return Err(MultisampleError::SourceUnavailable);
                }
            }
        }
        Ok(())
    }

    /// Process one block atomically against the borrowed `DeviceResources`
    /// lease. On any preflight failure, `out` is silenced and voice state is
    /// completely preserved.
    pub fn process_block(
        &mut self,
        map: &PreparedMultisampleMap,
        events: &[NativeInstrumentEvent],
        out: &mut [StereoFrame],
        resources: &dyn DeviceResources,
    ) -> Result<(), MultisampleError> {
        if let Err(error) = self.preflight_block(map, events, out.len(), resources) {
            out.fill([0.0; 2]);
            return Err(error);
        }
        let max_voices = self.max_voices;
        let mut cursor = 0_usize;
        for (frame_index, slot) in out.iter_mut().enumerate() {
            while let Some(&event) = events.get(cursor)
                && event.frame_offset() == frame_index
            {
                let _ = Self::apply_event(map, &mut self.voices[..max_voices], event, resources);
                cursor += 1;
            }
            let mut sum = [0.0_f64; 2];
            for voice in &mut self.voices[..max_voices] {
                if voice.occurrence_id == 0 {
                    continue;
                }
                let zone = &map.zones[usize::from(voice.zone_index)];
                if let Some(view) = resources
                    .slot(zone.source.slot)
                    .filter(|view| zone.source.matches(*view))
                {
                    let env = match voice.gate_age {
                        None => 1.0,
                        Some(gate) => {
                            let rel = voice.age.saturating_sub(gate);
                            if voice.release_frames == 0 || rel >= voice.release_frames {
                                0.0
                            } else {
                                1.0 - (rel as f64 / voice.release_frames as f64)
                            }
                        }
                    };
                    if env > 0.0 && voice.position < (zone.source.frames as f64) {
                        let sample = interpolate_linear(view.pcm, voice.position);
                        let scale = env * voice.gain * (f64::from(voice.velocity) / 127.0);
                        sum[0] += sample[0] * scale;
                        sum[1] += sample[1] * scale;
                    }
                }
                voice.advance_metadata(1);
            }
            *slot = [
                if sum[0].is_finite() {
                    sum[0] as f32
                } else {
                    0.0
                },
                if sum[1].is_finite() {
                    sum[1] as f32
                } else {
                    0.0
                },
            ];
        }
        Ok(())
    }

    fn apply_event(
        map: &PreparedMultisampleMap,
        voices: &mut [Voice],
        event: NativeInstrumentEvent,
        resources: &dyn DeviceResources,
    ) -> Result<(), MultisampleError> {
        match event {
            NativeInstrumentEvent::NoteOn {
                occurrence_id,
                pitch,
                velocity,
                ..
            } => {
                if occurrence_id == 0
                    || pitch > 127
                    || !(1..=127).contains(&velocity)
                    || voices
                        .iter()
                        .any(|voice| voice.occurrence_id == occurrence_id)
                {
                    return Err(MultisampleError::InvalidEventBatch);
                }
                let selected = map.selected_zones_for_key(pitch);
                if selected.is_empty() {
                    return Ok(());
                }
                // Check every selected zone's resource and capacity before allocating any voice slot.
                for &zone_idx in selected {
                    let zone = map
                        .zones
                        .get(usize::from(zone_idx))
                        .ok_or(MultisampleError::SourceUnavailable)?;
                    if resources
                        .slot(zone.source.slot)
                        .is_none_or(|view| !zone.source.matches(view))
                    {
                        return Err(MultisampleError::SourceUnavailable);
                    }
                }
                let free = voices
                    .iter()
                    .filter(|voice| voice.occurrence_id == 0)
                    .count();
                if free < selected.len() {
                    return Err(MultisampleError::VoiceCapacityExceeded);
                }
                for &zone_idx in selected {
                    let zone = &map.zones[usize::from(zone_idx)];
                    if let Some(slot) = voices.iter_mut().find(|v| v.occurrence_id == 0) {
                        *slot = Voice {
                            occurrence_id,
                            zone_index: zone_idx,
                            velocity,
                            position: 0.0,
                            step: zone.key_steps[usize::from(pitch)],
                            gain: f64::from(zone.spec.gain),
                            age: 0,
                            gate_age: None,
                            release_frames: zone.release_frames,
                            source_frames: zone.source.frames,
                        };
                    }
                }
            }
            NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                for voice in voices
                    .iter_mut()
                    .filter(|voice| voice.occurrence_id == occurrence_id)
                {
                    if voice.gate_age.is_none() {
                        voice.gate_age = Some(voice.age);
                    }
                    voice.advance_metadata(0);
                }
            }
            _ => {}
        }
        Ok(())
    }
}

#[inline]
fn interpolate_linear(pcm: DeviceResourcePcm<'_>, position: f64) -> [f64; 2] {
    let idx = position.floor() as usize;
    let frac = position - position.floor();
    let a = pcm.frame(idx).unwrap_or([0.0; 2]);
    let b = pcm.frame(idx + 1).unwrap_or([0.0; 2]);
    [
        f64::from(a[0]) + (f64::from(b[0]) - f64::from(a[0])) * frac,
        f64::from(a[1]) + (f64::from(b[1]) - f64::from(a[1])) * frac,
    ]
}

pub mod instrument;
pub use instrument::{
    GAIN, MULTISAMPLE_DEFINITION, MULTISAMPLE_STATE_VERSION, MULTISAMPLE_TYPE_ID,
    MultisampleDefinition, MultisampleState, OVERLAP_POLICY, PreparedMultisampleInstrumentRuntime,
    VOICE_LIMIT,
};

#[cfg(test)]
mod tests;
