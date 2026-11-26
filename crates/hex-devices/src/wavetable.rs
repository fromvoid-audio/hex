//! Bounded original wavetable source preparation and borrowed-PCM oscillator.
//!
//! This module defines a finite wavetable layout, control-side validation,
//! non-owning source binding, and a single bounded wavetable oscillator on top
//! of `hex_lattice`'s existing host-owned `DeviceResourceView` / `DeviceResources`
//! lease boundary.
//!
//! # Ownership & Safety
//! - Zero PCM ownership, zero `Arc` clones, zero raw pointers, zero `unsafe`.
//! - All wavetable PCM remains owned by the host (`ResidentDeviceAssets` /
//!   `PreparedDeviceResources`) and is borrowed only during callback rendering.
//! - Control-side `prepare_on_control` scans all samples for finiteness and
//!   enforces explicit frame size, table count, channel layout, sample rate,
//!   memory, and per-block work bounds before any callback can read the source.
//! - Callback rendering checks exact slot, content/derivation identity, sample
//!   rate, channel layout, and total frame count before reading a sample; any
//!   missing, changed, or stale source refuses cleanly with silence and never
//!   falls back to another slot or unverified buffer.
//!
//! This module does not register a full Wavetable instrument or UI surface.

use hex_lattice::{
    DeviceResourceIdentity, DeviceResourcePcm, DeviceResourceView, DeviceResources, StereoFrame,
};
use hex_lattice_contract::DeviceAssetSlot;

pub const MIN_FRAME_SIZE: usize = 64;
pub const MAX_FRAME_SIZE: usize = 4_096;
pub const MIN_TABLE_COUNT: usize = 1;
pub const MAX_TABLE_COUNT: usize = 256;
pub const MAX_TOTAL_FRAMES: usize = MAX_TABLE_COUNT * 2_048;
pub const MAX_PCM_BYTES: usize = MAX_TOTAL_FRAMES * std::mem::size_of::<[f32; 2]>();
pub const MIN_SAMPLE_RATE: u32 = 8_000;
pub const MAX_SAMPLE_RATE: u32 = 192_000;
pub const MAX_WORK_FRAMES_PER_BLOCK: usize = 65_536;
const TWO_POW_64: f64 = 18_446_744_073_709_551_616.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WavetableError {
    InvalidLimits,
    InvalidIdentity,
    InvalidSampleRate,
    InvalidFrameSize,
    UnalignedFrameCount,
    TableCountOutOfBounds,
    MemoryLimitExceeded,
    NonFiniteSample,
    InvalidOscillatorSettings,
    WorkLimitExceeded,
    SourceUnavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WavetableChannels {
    Mono,
    Stereo,
}

impl WavetableChannels {
    pub const fn count(self) -> usize {
        match self {
            Self::Mono => 1,
            Self::Stereo => 2,
        }
    }

    pub const fn bytes_per_frame(self) -> usize {
        self.count() * std::mem::size_of::<f32>()
    }

    const fn from_pcm(pcm: DeviceResourcePcm<'_>) -> Self {
        match pcm {
            DeviceResourcePcm::Mono(_) => Self::Mono,
            DeviceResourcePcm::Stereo(_) => Self::Stereo,
        }
    }
}

/// Explicit memory, geometry, and per-block work bounds for one wavetable source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WavetableLimits {
    pub max_frame_size: usize,
    pub max_table_count: usize,
    pub max_total_frames: usize,
    pub max_bytes: usize,
    pub max_work_frames_per_block: usize,
}

impl WavetableLimits {
    pub const DEFAULT: Self = Self {
        max_frame_size: 2_048,
        max_table_count: MAX_TABLE_COUNT,
        max_total_frames: MAX_TOTAL_FRAMES,
        max_bytes: MAX_PCM_BYTES,
        max_work_frames_per_block: 8_192,
    };

    pub const fn validate(self) -> Result<(), WavetableError> {
        if self.max_frame_size < MIN_FRAME_SIZE
            || self.max_frame_size > MAX_FRAME_SIZE
            || !self.max_frame_size.is_power_of_two()
            || self.max_table_count < MIN_TABLE_COUNT
            || self.max_table_count > MAX_TABLE_COUNT
            || self.max_total_frames < MIN_FRAME_SIZE
            || self.max_total_frames > MAX_TOTAL_FRAMES
            || self.max_bytes == 0
            || self.max_bytes > MAX_PCM_BYTES
            || self.max_work_frames_per_block == 0
            || self.max_work_frames_per_block > MAX_WORK_FRAMES_PER_BLOCK
        {
            return Err(WavetableError::InvalidLimits);
        }
        Ok(())
    }
}

/// Validated geometry of a contiguous multi-frame single-cycle wavetable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WavetableLayout {
    frame_size: usize,
    table_count: usize,
    channels: WavetableChannels,
    sample_rate: u32,
    total_frames: usize,
    pcm_bytes: usize,
}

impl WavetableLayout {
    pub const fn frame_size(self) -> usize {
        self.frame_size
    }

    pub const fn table_count(self) -> usize {
        self.table_count
    }

    pub const fn channels(self) -> WavetableChannels {
        self.channels
    }

    pub const fn sample_rate(self) -> u32 {
        self.sample_rate
    }

    pub const fn total_frames(self) -> usize {
        self.total_frames
    }

    pub const fn pcm_bytes(self) -> usize {
        self.pcm_bytes
    }
}

/// Non-owning identity binding to one exact host-leased wavetable resource.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WavetableSourceKey {
    slot: DeviceAssetSlot,
    identity: DeviceResourceIdentity,
    layout: WavetableLayout,
}

impl WavetableSourceKey {
    pub const fn slot(self) -> DeviceAssetSlot {
        self.slot
    }

    pub const fn identity(self) -> DeviceResourceIdentity {
        self.identity
    }

    pub const fn layout(self) -> WavetableLayout {
        self.layout
    }

    pub fn matches(self, view: DeviceResourceView<'_>) -> bool {
        self.slot == view.slot
            && self.identity == view.identity
            && self.layout.sample_rate == view.sample_rate
            && self.layout.total_frames == view.pcm.frames()
            && self.layout.channels == WavetableChannels::from_pcm(view.pcm)
    }
}

/// Control-validated wavetable preparation handle. Holds only immutable scalar
/// layout, identity, and work limits; never owns or clones PCM memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreparedWavetableSource {
    key: WavetableSourceKey,
    limits: WavetableLimits,
}

impl PreparedWavetableSource {
    /// Validate geometry, identity, sample rate, memory bounds, and complete
    /// finite PCM on the control thread before any audio callback can read it.
    pub fn prepare_on_control(
        view: DeviceResourceView<'_>,
        frame_size: usize,
        limits: WavetableLimits,
    ) -> Result<Self, WavetableError> {
        limits.validate()?;
        if view.identity.derivation_version == 0 {
            return Err(WavetableError::InvalidIdentity);
        }
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&view.sample_rate) {
            return Err(WavetableError::InvalidSampleRate);
        }
        if frame_size < MIN_FRAME_SIZE
            || frame_size > limits.max_frame_size
            || !frame_size.is_power_of_two()
        {
            return Err(WavetableError::InvalidFrameSize);
        }
        let total_frames = view.pcm.frames();
        if total_frames == 0 || !total_frames.is_multiple_of(frame_size) {
            return Err(WavetableError::UnalignedFrameCount);
        }
        let table_count = total_frames / frame_size;
        if !(MIN_TABLE_COUNT..=limits.max_table_count).contains(&table_count) {
            return Err(WavetableError::TableCountOutOfBounds);
        }
        if total_frames > limits.max_total_frames {
            return Err(WavetableError::MemoryLimitExceeded);
        }
        let channels = WavetableChannels::from_pcm(view.pcm);
        let pcm_bytes = total_frames
            .checked_mul(channels.bytes_per_frame())
            .filter(|&bytes| bytes <= limits.max_bytes)
            .ok_or(WavetableError::MemoryLimitExceeded)?;
        let finite = match view.pcm {
            DeviceResourcePcm::Mono(pcm) => pcm.iter().all(|sample| sample.is_finite()),
            DeviceResourcePcm::Stereo(pcm) => pcm.iter().flatten().all(|sample| sample.is_finite()),
        };
        if !finite {
            return Err(WavetableError::NonFiniteSample);
        }
        let layout = WavetableLayout {
            frame_size,
            table_count,
            channels,
            sample_rate: view.sample_rate,
            total_frames,
            pcm_bytes,
        };
        Ok(Self {
            key: WavetableSourceKey {
                slot: view.slot,
                identity: view.identity,
                layout,
            },
            limits,
        })
    }

    pub const fn key(self) -> WavetableSourceKey {
        self.key
    }

    pub const fn layout(self) -> WavetableLayout {
        self.key.layout
    }

    pub const fn limits(self) -> WavetableLimits {
        self.limits
    }

    /// Construct a bounded oscillator borrowing this source's host lease.
    pub fn oscillator(
        self,
        settings: WavetableOscillatorSettings,
    ) -> Result<WavetableOscillator, WavetableError> {
        WavetableOscillator::new(self, settings)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WavetableInterpolation {
    Linear,
    CubicHermite,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WavetableOscillatorSettings {
    pub output_sample_rate: u32,
    pub frequency_hz: f64,
    pub table_position: f64,
    pub initial_phase: f64,
    pub gain: f64,
    pub interpolation: WavetableInterpolation,
}

impl WavetableOscillatorSettings {
    pub fn validate(self) -> Result<(), WavetableError> {
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&self.output_sample_rate) {
            return Err(WavetableError::InvalidSampleRate);
        }
        let nyquist = 0.5 * f64::from(self.output_sample_rate);
        if !self.frequency_hz.is_finite()
            || self.frequency_hz <= 0.0
            || self.frequency_hz >= nyquist
            || !self.table_position.is_finite()
            || !(0.0..=1.0).contains(&self.table_position)
            || !self.initial_phase.is_finite()
            || !(0.0..1.0).contains(&self.initial_phase)
            || !self.gain.is_finite()
            || !(0.0..=2.0).contains(&self.gain)
        {
            return Err(WavetableError::InvalidOscillatorSettings);
        }
        Ok(())
    }
}

/// Bounded single-voice wavetable oscillator operating strictly on borrowed
/// host `DeviceResources`. Zero heap storage, zero allocations during render.
#[derive(Clone, Copy, Debug)]
pub struct WavetableOscillator {
    source: PreparedWavetableSource,
    output_sample_rate: u32,
    initial_phase: u64,
    phase: u64,
    phase_step: u64,
    table_position: f64,
    gain: f64,
    interpolation: WavetableInterpolation,
}

impl WavetableOscillator {
    pub fn new(
        source: PreparedWavetableSource,
        settings: WavetableOscillatorSettings,
    ) -> Result<Self, WavetableError> {
        settings.validate()?;
        let initial_phase = phase_from_turns(settings.initial_phase);
        let phase_step = step_from_frequency(settings.frequency_hz, settings.output_sample_rate)?;
        Ok(Self {
            source,
            output_sample_rate: settings.output_sample_rate,
            initial_phase,
            phase: initial_phase,
            phase_step,
            table_position: settings.table_position,
            gain: settings.gain,
            interpolation: settings.interpolation,
        })
    }

    pub const fn source(self) -> PreparedWavetableSource {
        self.source
    }

    pub const fn phase(self) -> u64 {
        self.phase
    }

    pub fn reset(&mut self) {
        self.phase = self.initial_phase;
    }

    pub fn set_frequency_hz(&mut self, frequency_hz: f64) -> Result<(), WavetableError> {
        let step = step_from_frequency(frequency_hz, self.output_sample_rate)?;
        self.phase_step = step;
        Ok(())
    }

    pub fn set_table_position(&mut self, position: f64) -> Result<(), WavetableError> {
        if !position.is_finite() || !(0.0..=1.0).contains(&position) {
            return Err(WavetableError::InvalidOscillatorSettings);
        }
        self.table_position = position;
        Ok(())
    }

    pub fn set_gain(&mut self, gain: f64) -> Result<(), WavetableError> {
        if !gain.is_finite() || !(0.0..=2.0).contains(&gain) {
            return Err(WavetableError::InvalidOscillatorSettings);
        }
        self.gain = gain;
        Ok(())
    }

    /// Render `out.len()` frames strictly from the borrowed host lease.
    ///
    /// - Refuses read-only (`WorkLimitExceeded`) if `out.len()` exceeds the
    ///   prepared work bound.
    /// - Refuses (`SourceUnavailable`) and writes exact silence without
    ///   advancing phase if the host lease is missing, changed, or stale.
    pub fn render_block(
        &mut self,
        resources: &dyn DeviceResources,
        out: &mut [StereoFrame],
    ) -> Result<(), WavetableError> {
        if out.len() > self.source.limits.max_work_frames_per_block {
            return Err(WavetableError::WorkLimitExceeded);
        }
        let Some(view) = resources
            .slot(self.source.key.slot)
            .filter(|view| self.source.key.matches(*view))
        else {
            out.fill([0.0; 2]);
            return Err(WavetableError::SourceUnavailable);
        };
        let layout = self.source.key.layout;
        let frame_size = layout.frame_size;
        let mask = frame_size - 1;
        let (table_a, table_b, morph) = table_indices(layout.table_count, self.table_position);
        let base_a = table_a * frame_size;
        let base_b = table_b * frame_size;
        let gain = self.gain;

        for slot in out.iter_mut() {
            let phase_turns = (self.phase as f64) / TWO_POW_64;
            let scaled = phase_turns * (frame_size as f64);
            let idx = (scaled.floor() as usize) & mask;
            let frac = scaled - scaled.floor();
            let sample_a = sample_cycle(view.pcm, base_a, mask, idx, frac, self.interpolation);
            let sample = if table_a == table_b || morph == 0.0 {
                sample_a
            } else {
                let sample_b = sample_cycle(view.pcm, base_b, mask, idx, frac, self.interpolation);
                [
                    sample_a[0] + (sample_b[0] - sample_a[0]) * morph,
                    sample_a[1] + (sample_b[1] - sample_a[1]) * morph,
                ]
            };
            let left = sample[0] * gain;
            let right = sample[1] * gain;
            *slot = [
                if left.is_finite() { left as f32 } else { 0.0 },
                if right.is_finite() { right as f32 } else { 0.0 },
            ];
            self.phase = self.phase.wrapping_add(self.phase_step);
        }
        Ok(())
    }
}

fn phase_from_turns(turns: f64) -> u64 {
    ((turns.clamp(0.0, 1.0 - f64::EPSILON)) * TWO_POW_64).round() as u64
}

fn step_from_frequency(frequency_hz: f64, output_sample_rate: u32) -> Result<u64, WavetableError> {
    let nyquist = 0.5 * f64::from(output_sample_rate);
    if !frequency_hz.is_finite() || frequency_hz <= 0.0 || frequency_hz >= nyquist {
        return Err(WavetableError::InvalidOscillatorSettings);
    }
    let step = ((frequency_hz / f64::from(output_sample_rate)) * TWO_POW_64)
        .round()
        .clamp(1.0, (1_u64 << 63) as f64 - 1.0) as u64;
    Ok(step)
}

#[inline]
fn table_indices(table_count: usize, position: f64) -> (usize, usize, f64) {
    if table_count <= 1 {
        return (0, 0, 0.0);
    }
    let last = table_count - 1;
    let scaled = (position.clamp(0.0, 1.0)) * (last as f64);
    let idx = (scaled.floor() as usize).min(last);
    if idx == last {
        (last, last, 0.0)
    } else {
        (idx, idx + 1, scaled - (idx as f64))
    }
}

#[inline]
fn frame_at(pcm: DeviceResourcePcm<'_>, index: usize) -> [f64; 2] {
    match pcm.frame(index) {
        Some([l, r]) => [f64::from(l), f64::from(r)],
        None => [0.0; 2],
    }
}

#[inline]
fn catmull_rom(y0: f64, y1: f64, y2: f64, y3: f64, t: f64) -> f64 {
    let c0 = y1;
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * t + c2) * t + c1) * t + c0
}

#[inline]
fn sample_cycle(
    pcm: DeviceResourcePcm<'_>,
    base: usize,
    mask: usize,
    idx: usize,
    frac: f64,
    mode: WavetableInterpolation,
) -> [f64; 2] {
    match mode {
        WavetableInterpolation::Linear => {
            let a = frame_at(pcm, base + idx);
            let b = frame_at(pcm, base + ((idx + 1) & mask));
            [a[0] + (b[0] - a[0]) * frac, a[1] + (b[1] - a[1]) * frac]
        }
        WavetableInterpolation::CubicHermite => {
            let p0 = frame_at(pcm, base + (idx.wrapping_sub(1) & mask));
            let p1 = frame_at(pcm, base + idx);
            let p2 = frame_at(pcm, base + ((idx + 1) & mask));
            let p3 = frame_at(pcm, base + ((idx + 2) & mask));
            [
                catmull_rom(p0[0], p1[0], p2[0], p3[0], frac),
                catmull_rom(p0[1], p1[1], p2[1], p3[1], frac),
            ]
        }
    }
}

#[cfg(test)]
mod tests;
