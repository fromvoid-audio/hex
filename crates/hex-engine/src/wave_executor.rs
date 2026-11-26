//! Thread-free topological wave execution contract for independent native DSP chains.
//!
//! `hex-engine` owns graph wave partitioning and the `WaveExecutor` interface without
//! spawning OS threads or depending on audio I/O backends. `hex-audio` supplies the
//! bounded lock-free worker pool implementation (`AudioWorkerPool`), while
//! `SerialWaveExecutor` provides deterministic inline fallback and test verification.
//!
//! All task slots and audio buffers are preallocated once on control by the executor
//! implementation so callback rendering incurs zero heap allocations and never drops
//! processor chains on the audio thread.

use crate::{Frame, compensation::TransportStamp};
use hex_lattice::{NativeInstrumentClock, NativeRuntimeChain};
use hex_lattice_contract::DeviceContractError;
use std::num::NonZeroU64;

/// Maximum stereo frame count processed in a single wave dispatch.
pub const WAVE_MAX_FRAMES: usize = 256;

/// Maximum inline low-delay compensation lookback frames carried by a wave clock snapshot.
pub const WAVE_CLOCK_HISTORY_FRAMES: usize = 256;

/// Maximum distinct high-delay (`> WAVE_CLOCK_HISTORY_FRAMES`) history windows
/// carried per wave. Each window covers up to `WAVE_MAX_FRAMES` consecutive
/// history stamps at arbitrary PDC arrival delays (up to `MAX_COMPENSATION_FRAMES`),
/// allowing VST3 bridges (512+ frames), linear-phase EQs, and lookahead limiters
/// to run in parallel waves without copying the entire compensation ring.
pub const WAVE_CLOCK_HIGH_DELAY_WINDOWS: usize = 64;

/// Maximum independent chain tasks admitted in one topological wave.
pub const MAX_WAVE_TASKS: usize = 128;

/// Maximum per-stage arrival records carried by one wave task.
pub const MAX_WAVE_STAGES: usize = 32;

/// Minimum independent tasks required before fanning out across worker threads.
/// Smaller waves execute inline on the calling thread to avoid wakeup overhead.
pub const PARALLEL_WAVE_MIN_TASKS: usize = 4;

/// Compact per-frame clock sample copied once per wave into a shared read-only
/// `WaveClockWindow`. Preserves exact `OriginWindow` and `ArrangementEffectClock`
/// semantics across worker threads without borrowing graph or delay-ring references.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WaveStamp {
    pub present: bool,
    pub transport_is_running: bool,
    pub authored_clock_epoch: Option<NonZeroU64>,
    pub tempo_micro_bpm: Option<u32>,
    pub performance_frame: u64,
    pub arrangement_frame: u64,
    pub loop_pass: Option<u64>,
    pub performance_beat_raw: Option<u64>,
    pub arrangement_beat_raw: Option<u64>,
    pub meter: Option<(u8, u8)>,
    pub cycle_beats_raw: Option<(u64, u64)>,
}

impl WaveStamp {
    /// Convert a compensated `TransportStamp` into an equivalent `WaveStamp`.
    #[inline]
    pub fn from_transport_stamp(stamp: TransportStamp) -> Self {
        Self {
            present: true,
            transport_is_running: stamp.transport_is_running,
            authored_clock_epoch: stamp.authored_clock_epoch,
            tempo_micro_bpm: stamp.tempo_micro_bpm,
            performance_frame: stamp.performance_frame,
            arrangement_frame: stamp.arrangement_frame,
            loop_pass: Some(stamp.loop_pass),
            performance_beat_raw: Some(stamp.performance_beat_raw),
            arrangement_beat_raw: stamp.arrangement_beat_raw,
            meter: stamp.meter.map(|m| (m.numerator(), m.denominator())),
            cycle_beats_raw: stamp.cycle_beats_raw,
        }
    }
}

/// Contiguous slice of compensation stamp history for high-latency stages
/// (`delay > WAVE_CLOCK_HISTORY_FRAMES`).
#[derive(Clone, Debug)]
pub struct WaveDelayWindow {
    pub start_delay: u32,
    pub len: usize,
    pub stamps: [WaveStamp; WAVE_MAX_FRAMES],
}

impl Default for WaveDelayWindow {
    fn default() -> Self {
        Self {
            start_delay: 0,
            len: 0,
            stamps: [WaveStamp::default(); WAVE_MAX_FRAMES],
        }
    }
}

/// Shared read-only clock snapshot for all tasks in one topological wave.
///
/// Allocated once on control inside the `WaveExecutor` implementation and updated
/// in place before each wave dispatch without heap allocation.
#[derive(Clone, Debug)]
pub struct WaveClockWindow {
    pub history_len: usize,
    pub incoming_len: usize,
    pub high_delay_len: usize,
    pub history: [WaveStamp; WAVE_CLOCK_HISTORY_FRAMES],
    pub incoming: [WaveStamp; WAVE_MAX_FRAMES],
    high_delays: Box<[WaveDelayWindow]>,
}

impl Default for WaveClockWindow {
    fn default() -> Self {
        let high_delays = std::iter::repeat_with(WaveDelayWindow::default)
            .take(WAVE_CLOCK_HIGH_DELAY_WINDOWS)
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            history_len: 0,
            incoming_len: 0,
            high_delay_len: 0,
            history: [WaveStamp::default(); WAVE_CLOCK_HISTORY_FRAMES],
            incoming: [WaveStamp::default(); WAVE_MAX_FRAMES],
            high_delays,
        }
    }
}

impl WaveClockWindow {
    /// Allocate a zeroed clock window on the control thread.
    pub fn boxed_default() -> Box<Self> {
        Box::new(Self::default())
    }

    /// Populate from a compensated delay ring and upcoming transport stamp window
    /// for stages with arrival delays `<= WAVE_CLOCK_HISTORY_FRAMES`.
    #[allow(dead_code)]
    pub(crate) fn fill_from_compensated(
        &mut self,
        delays: &crate::compensation::CompensationRuntime,
        stamps: &[Option<TransportStamp>],
        max_delay: usize,
    ) {
        self.fill_from_compensated_delays(delays, stamps, max_delay, &mut []);
    }

    /// Populate from a compensated delay ring for both low-delay stages
    /// (`<= WAVE_CLOCK_HISTORY_FRAMES`) and arbitrary high-delay stages
    /// (up to `MAX_COMPENSATION_FRAMES`) without copying unused intermediate history.
    pub(crate) fn fill_from_compensated_delays(
        &mut self,
        delays: &crate::compensation::CompensationRuntime,
        stamps: &[Option<TransportStamp>],
        base_max_delay: usize,
        high_delays: &mut [u32],
    ) {
        let incoming_len = stamps.len().min(WAVE_MAX_FRAMES);
        self.incoming_len = incoming_len;
        for (slot, stamp) in self.incoming[..incoming_len].iter_mut().zip(stamps) {
            *slot = stamp
                .map(WaveStamp::from_transport_stamp)
                .unwrap_or_default();
        }
        let history_len = base_max_delay.min(WAVE_CLOCK_HISTORY_FRAMES);
        self.history_len = history_len;
        for (delay, slot) in self.history[..history_len].iter_mut().enumerate() {
            *slot = delays
                .stamp_at_delay_ref(delay as u32)
                .copied()
                .map(WaveStamp::from_transport_stamp)
                .unwrap_or_default();
        }
        high_delays.sort_unstable();
        let mut window_count = 0usize;
        let mut covered_end = history_len as u32;
        let incoming_u32 = incoming_len as u32;
        for &delay in high_delays.iter() {
            if delay <= covered_end || window_count >= self.high_delays.len() {
                continue;
            }
            let start = delay.saturating_sub(incoming_u32).max(covered_end);
            let len = (delay - start) as usize;
            debug_assert!(len <= WAVE_MAX_FRAMES);
            let len = len.min(WAVE_MAX_FRAMES);
            let window = &mut self.high_delays[window_count];
            window.start_delay = start;
            window.len = len;
            for (i, slot) in window.stamps[..len].iter_mut().enumerate() {
                *slot = delays
                    .stamp_at_delay_ref(start + i as u32)
                    .copied()
                    .map(WaveStamp::from_transport_stamp)
                    .unwrap_or_default();
            }
            covered_end = start + len as u32;
            window_count += 1;
        }
        self.high_delay_len = window_count;
    }

    /// Populate from any `NativeInstrumentClock` (such as `ArrangementEffectClock`)
    /// for `frames` samples with zero history lookback.
    pub(crate) fn fill_from_clock(&mut self, clock: &dyn NativeInstrumentClock, frames: usize) {
        self.history_len = 0;
        self.high_delay_len = 0;
        let incoming_len = frames.min(WAVE_MAX_FRAMES);
        self.incoming_len = incoming_len;
        let running = clock.transport_is_running().unwrap_or(false);
        for (offset, slot) in self.incoming[..incoming_len].iter_mut().enumerate() {
            let arrangement_frame = clock.arrangement_frame_at_offset(offset);
            let performance_frame = clock.performance_frame_at_offset(offset);
            *slot = WaveStamp {
                present: arrangement_frame.is_some()
                    || performance_frame.is_some()
                    || clock.beat_at_offset(offset).is_some()
                    || clock.tempo_micro_bpm_at_offset(offset).is_some(),
                transport_is_running: running,
                authored_clock_epoch: clock.authored_clock_epoch_at_offset(offset),
                tempo_micro_bpm: clock.tempo_micro_bpm_at_offset(offset),
                performance_frame: performance_frame.unwrap_or(0),
                arrangement_frame: arrangement_frame.unwrap_or(0),
                loop_pass: clock.loop_pass_at_offset(offset),
                performance_beat_raw: clock.beat_at_offset(offset),
                arrangement_beat_raw: clock.arrangement_beat_at_offset(offset),
                meter: clock.arrangement_meter_at_offset(offset),
                cycle_beats_raw: clock.arrangement_cycle_at_offset(offset),
            };
        }
    }

    #[inline]
    fn stamp_ref(&self, delay: u32, offset: usize) -> Option<&WaveStamp> {
        let delay_usize = delay as usize;
        let stamp = if let Some(upcoming) = offset.checked_sub(delay_usize) {
            self.incoming
                .get(upcoming)
                .filter(|_| upcoming < self.incoming_len)?
        } else {
            let hist_index = delay_usize - offset - 1;
            if hist_index < self.history_len {
                self.history.get(hist_index)?
            } else {
                let hist_u32 = u32::try_from(hist_index).ok()?;
                let windows = &self.high_delays[..self.high_delay_len];
                let pos = windows
                    .binary_search_by(|w| {
                        let end = w.start_delay.saturating_add(w.len as u32);
                        if hist_u32 < w.start_delay {
                            std::cmp::Ordering::Greater
                        } else if hist_u32 >= end {
                            std::cmp::Ordering::Less
                        } else {
                            std::cmp::Ordering::Equal
                        }
                    })
                    .ok()?;
                let window = &windows[pos];
                window
                    .stamps
                    .get((hist_u32 - window.start_delay) as usize)?
            }
        };
        stamp.present.then_some(stamp)
    }
}

/// Borrowed view of a `WaveClockWindow` at one stage's arrival delay.
pub struct WaveStageClock<'a> {
    window: &'a WaveClockWindow,
    delay: u32,
}

impl NativeInstrumentClock for WaveStageClock<'_> {
    fn arrangement_meter_at_offset(&self, offset: usize) -> Option<(u8, u8)> {
        self.window.stamp_ref(self.delay, offset)?.meter
    }
    fn arrangement_cycle_at_offset(&self, offset: usize) -> Option<(u64, u64)> {
        self.window.stamp_ref(self.delay, offset)?.cycle_beats_raw
    }
    fn beat_at_offset(&self, offset: usize) -> Option<u64> {
        self.window
            .stamp_ref(self.delay, offset)?
            .performance_beat_raw
    }
    fn tempo_micro_bpm_at_offset(&self, offset: usize) -> Option<u32> {
        self.window.stamp_ref(self.delay, offset)?.tempo_micro_bpm
    }
    fn arrangement_beat_at_offset(&self, offset: usize) -> Option<u64> {
        self.window
            .stamp_ref(self.delay, offset)?
            .arrangement_beat_raw
    }
    fn arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
        self.window
            .stamp_ref(self.delay, offset)
            .map(|stamp| stamp.arrangement_frame)
    }
    fn authored_clock_epoch_at_offset(&self, offset: usize) -> Option<NonZeroU64> {
        self.window
            .stamp_ref(self.delay, offset)?
            .authored_clock_epoch
    }
    fn authored_arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
        self.window
            .stamp_ref(self.delay, offset)
            .filter(|stamp| stamp.transport_is_running)
            .map(|stamp| stamp.arrangement_frame)
    }
    fn loop_pass_at_offset(&self, offset: usize) -> Option<u64> {
        self.window.stamp_ref(self.delay, offset)?.loop_pass
    }
    fn performance_frame_at_offset(&self, offset: usize) -> Option<u64> {
        self.window
            .stamp_ref(self.delay, offset)
            .filter(|stamp| stamp.transport_is_running)
            .map(|stamp| stamp.performance_frame)
    }
    fn transport_is_running(&self) -> Option<bool> {
        self.window
            .stamp_ref(self.delay, 0)
            .map(|stamp| stamp.transport_is_running)
    }
    fn contiguous_running_frames(&self, first_offset: usize, len: usize) -> Option<(u64, u64)> {
        if len == 0 || !self.window.stamp_ref(self.delay, 0)?.transport_is_running {
            return None;
        }
        let first = self.window.stamp_ref(self.delay, first_offset)?;
        if !first.transport_is_running {
            return None;
        }
        let arr0 = first.arrangement_frame;
        let perf0 = first.performance_frame;
        arr0.checked_add((len - 1) as u64)?;
        perf0.checked_add((len - 1) as u64)?;
        let end = first_offset.checked_add(len)?;
        for offset in (first_offset + 1)..end {
            let stamp = self.window.stamp_ref(self.delay, offset)?;
            let delta = (offset - first_offset) as u64;
            if !stamp.transport_is_running
                || stamp.arrangement_frame != arr0 + delta
                || stamp.performance_frame != perf0 + delta
            {
                return None;
            }
        }
        Some((arr0, perf0))
    }
    fn constant_tempo_micro_bpm(&self, first_offset: usize, len: usize) -> Option<u32> {
        if len == 0 {
            return None;
        }
        let tempo = self
            .window
            .stamp_ref(self.delay, first_offset)?
            .tempo_micro_bpm?;
        let end = first_offset.checked_add(len)?;
        for offset in (first_offset + 1)..end {
            if self.window.stamp_ref(self.delay, offset)?.tempo_micro_bpm != Some(tempo) {
                return None;
            }
        }
        Some(tempo)
    }
}

/// Per-stage arrival delays for compensated wave tasks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WaveStageArrival {
    pub input_delay: u32,
    pub output_delay: u32,
}

/// Preallocated wave task slot owning a stereo audio scratch buffer and a temporarily
/// moved `NativeRuntimeChain`. Moves safely across threads without heap allocation.
pub struct WaveTaskSlot {
    pub owner_index: usize,
    pub chain: NativeRuntimeChain,
    pub buffer: Box<[Frame; WAVE_MAX_FRAMES]>,
    pub frames: usize,
    pub frame_offset: usize,
    pub stage_start: usize,
    pub stage_end: usize,
    pub compensated: bool,
    pub stage_arrivals: [WaveStageArrival; MAX_WAVE_STAGES],
    pub error: Option<DeviceContractError>,
}

impl WaveTaskSlot {
    /// Allocate an idle task slot on the control thread.
    pub fn new_idle() -> Self {
        Self {
            owner_index: 0,
            chain: NativeRuntimeChain::empty(),
            buffer: Box::new([[0.0; 2]; WAVE_MAX_FRAMES]),
            frames: 0,
            frame_offset: 0,
            stage_start: 0,
            stage_end: 0,
            compensated: false,
            stage_arrivals: [WaveStageArrival::default(); MAX_WAVE_STAGES],
            error: None,
        }
    }

    /// Populate this preallocated slot with a task without allocating.
    #[allow(clippy::too_many_arguments)]
    pub fn load(
        &mut self,
        owner_index: usize,
        chain: NativeRuntimeChain,
        audio: &[Frame],
        frame_offset: usize,
        stage_start: usize,
        stage_end: usize,
        compensated: bool,
        stage_arrivals: &[WaveStageArrival],
    ) {
        self.owner_index = owner_index;
        self.chain = chain;
        self.frames = audio.len();
        self.buffer[..audio.len()].copy_from_slice(audio);
        self.frame_offset = frame_offset;
        self.stage_start = stage_start;
        self.stage_end = stage_end;
        self.compensated = compensated;
        if compensated {
            let count = stage_arrivals.len().min(MAX_WAVE_STAGES);
            self.stage_arrivals[..count].copy_from_slice(&stage_arrivals[..count]);
        }
        self.error = None;
    }

    /// Execute this task's native processor stages against the shared wave clock.
    pub fn run(&mut self, clock: &WaveClockWindow) {
        if self.stage_start >= self.stage_end || self.frames == 0 {
            return;
        }
        let frames = &mut self.buffer[..self.frames];
        let mut ignore_observer = |_, _, _, _, _, _: &[Frame]| {};
        if !self.compensated {
            let stage_clock = WaveStageClock {
                window: clock,
                delay: 0,
            };
            let result = self.chain.process_range_tapped_with_context(
                self.stage_start..self.stage_end,
                frames,
                hex_lattice::StageProcessContext::new(
                    self.frame_offset,
                    Some(&stage_clock),
                    Some(&stage_clock),
                )
                .with_foreign(&mut hex_lattice::RefuseForeign),
                &mut ignore_observer,
            );
            if let Err(err) = result {
                self.error = Some(err);
            }
            return;
        }
        for (offset, stage) in (self.stage_start..self.stage_end).enumerate() {
            let arrival = self.stage_arrivals[offset];
            let input_clock = WaveStageClock {
                window: clock,
                delay: arrival.input_delay,
            };
            let output_clock = WaveStageClock {
                window: clock,
                delay: arrival.output_delay,
            };
            let result = self.chain.process_range_tapped_with_context(
                stage..stage + 1,
                frames,
                hex_lattice::StageProcessContext::new(
                    self.frame_offset,
                    Some(&input_clock),
                    Some(&output_clock),
                )
                .with_foreign(&mut hex_lattice::RefuseForeign),
                &mut ignore_observer,
            );
            if let Err(err) = result {
                self.error = Some(err);
                return;
            }
        }
    }

    /// Reclaim the moved chain and copy processed frames into `audio_out`.
    pub fn take_result(
        &mut self,
        audio_out: &mut [Frame],
    ) -> (usize, NativeRuntimeChain, Option<DeviceContractError>) {
        let count = self.frames.min(audio_out.len());
        audio_out[..count].copy_from_slice(&self.buffer[..count]);
        let chain = std::mem::replace(&mut self.chain, NativeRuntimeChain::empty());
        let error = self.error.take();
        self.frames = 0;
        self.stage_start = 0;
        self.stage_end = 0;
        (self.owner_index, chain, error)
    }
}

/// Bounded lock-free wave execution contract implemented by `hex-audio::AudioWorkerPool`
/// (and `SerialWaveExecutor` for deterministic single-threaded execution).
pub trait WaveExecutor {
    /// Number of background worker threads available (`0` executes inline).
    fn worker_count(&self) -> usize;

    /// Borrow the preallocated mutable clock snapshot before staging tasks.
    /// Returns `None` if the executor is unavailable (caller runs serially).
    fn prepare_clock(&mut self) -> Option<&mut WaveClockWindow>;

    /// Stage one independent native chain task into a preallocated slot.
    /// Returns `Err(chain)` without dropping `chain` if capacity or bounds are exceeded.
    #[allow(clippy::too_many_arguments)]
    fn push_task(
        &mut self,
        owner_index: usize,
        chain: NativeRuntimeChain,
        audio: &[Frame],
        frame_offset: usize,
        stage_start: usize,
        stage_end: usize,
        compensated: bool,
        stage_arrivals: &[WaveStageArrival],
    ) -> Result<usize, NativeRuntimeChain>;

    /// Execute all staged tasks (fanning out across workers when eligible) and wait
    /// for every task to complete before returning.
    fn execute_staged(&mut self);

    /// Reclaim the `slot`-th completed task's chain, processed audio, and optional error.
    fn reclaim_task(
        &mut self,
        slot: usize,
        audio_out: &mut [Frame],
    ) -> (usize, NativeRuntimeChain, Option<DeviceContractError>);

    /// Reset staged counters after all slots have been reclaimed.
    fn finish_wave(&mut self);
}

/// Deterministic single-threaded `WaveExecutor` used for testing and serial fallback.
pub struct SerialWaveExecutor {
    clock: Box<WaveClockWindow>,
    slots: Vec<WaveTaskSlot>,
    staged: usize,
}

impl SerialWaveExecutor {
    /// Allocate a serial wave executor on control with `capacity` (`<= MAX_WAVE_TASKS`) slots.
    pub fn new(capacity: usize) -> Self {
        let cap = capacity.min(MAX_WAVE_TASKS);
        let mut slots = Vec::with_capacity(cap);
        for _ in 0..cap {
            slots.push(WaveTaskSlot::new_idle());
        }
        Self {
            clock: WaveClockWindow::boxed_default(),
            slots,
            staged: 0,
        }
    }
}

impl Default for SerialWaveExecutor {
    fn default() -> Self {
        Self::new(MAX_WAVE_TASKS)
    }
}

impl WaveExecutor for SerialWaveExecutor {
    fn worker_count(&self) -> usize {
        0
    }

    fn prepare_clock(&mut self) -> Option<&mut WaveClockWindow> {
        if self.staged != 0 {
            return None;
        }
        Some(&mut self.clock)
    }

    fn push_task(
        &mut self,
        owner_index: usize,
        chain: NativeRuntimeChain,
        audio: &[Frame],
        frame_offset: usize,
        stage_start: usize,
        stage_end: usize,
        compensated: bool,
        stage_arrivals: &[WaveStageArrival],
    ) -> Result<usize, NativeRuntimeChain> {
        if self.staged >= self.slots.len()
            || audio.len() > WAVE_MAX_FRAMES
            || stage_end.saturating_sub(stage_start) > MAX_WAVE_STAGES
            || (compensated && stage_arrivals.len() < stage_end.saturating_sub(stage_start))
        {
            return Err(chain);
        }
        let slot = self.staged;
        self.slots[slot].load(
            owner_index,
            chain,
            audio,
            frame_offset,
            stage_start,
            stage_end,
            compensated,
            stage_arrivals,
        );
        self.staged += 1;
        Ok(slot)
    }

    fn execute_staged(&mut self) {
        let clock = &*self.clock;
        for slot in &mut self.slots[..self.staged] {
            slot.run(clock);
        }
    }

    fn reclaim_task(
        &mut self,
        slot: usize,
        audio_out: &mut [Frame],
    ) -> (usize, NativeRuntimeChain, Option<DeviceContractError>) {
        self.slots[slot].take_result(audio_out)
    }

    fn finish_wave(&mut self) {
        self.staged = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serial_wave_executor_preserves_chain_ownership_and_enforces_bounds() {
        let mut exec = SerialWaveExecutor::new(2);
        assert_eq!(exec.worker_count(), 0);
        assert!(exec.prepare_clock().is_some());

        let audio = [[0.25_f32, -0.5_f32]; 16];
        let s0 = exec
            .push_task(0, NativeRuntimeChain::empty(), &audio, 0, 0, 0, false, &[])
            .ok()
            .unwrap();
        let s1 = exec
            .push_task(1, NativeRuntimeChain::empty(), &audio, 0, 0, 0, false, &[])
            .ok()
            .unwrap();
        // Capacity bound returns the chain safely without dropping it.
        assert!(
            exec.push_task(2, NativeRuntimeChain::empty(), &audio, 0, 0, 0, false, &[])
                .is_err()
        );
        exec.execute_staged();

        let mut out = [[0.0_f32; 2]; 16];
        let (owner0, _chain0, err0) = exec.reclaim_task(s0, &mut out);
        assert_eq!(owner0, 0);
        assert!(err0.is_none());
        assert_eq!(out, audio);

        let (owner1, _chain1, err1) = exec.reclaim_task(s1, &mut out);
        assert_eq!(owner1, 1);
        assert!(err1.is_none());
        exec.finish_wave();
        assert!(exec.prepare_clock().is_some());
    }

    #[test]
    fn wave_stage_clock_matches_transport_stamp_history_and_incoming() {
        let mut window = WaveClockWindow {
            history_len: 2,
            incoming_len: 2,
            ..Default::default()
        };
        // delay=0 is newest history sample (frame 99), delay=1 is frame 98.
        window.history[0] = WaveStamp {
            present: true,
            transport_is_running: true,
            tempo_micro_bpm: Some(120_000_000),
            performance_frame: 99,
            arrangement_frame: 99,
            loop_pass: Some(0),
            performance_beat_raw: Some(1000),
            arrangement_beat_raw: Some(1000),
            meter: Some((4, 4)),
            cycle_beats_raw: None,
            authored_clock_epoch: None,
        };
        window.history[1] = WaveStamp {
            present: true,
            transport_is_running: true,
            tempo_micro_bpm: Some(120_000_000),
            performance_frame: 98,
            arrangement_frame: 98,
            loop_pass: Some(0),
            performance_beat_raw: Some(900),
            arrangement_beat_raw: Some(900),
            meter: Some((4, 4)),
            cycle_beats_raw: None,
            authored_clock_epoch: None,
        };
        window.incoming[0] = WaveStamp {
            present: true,
            transport_is_running: true,
            tempo_micro_bpm: Some(120_000_000),
            performance_frame: 100,
            arrangement_frame: 100,
            loop_pass: Some(0),
            performance_beat_raw: Some(1100),
            arrangement_beat_raw: Some(1100),
            meter: Some((4, 4)),
            cycle_beats_raw: None,
            authored_clock_epoch: None,
        };
        window.incoming[1] = WaveStamp {
            present: true,
            transport_is_running: true,
            tempo_micro_bpm: Some(120_000_000),
            performance_frame: 101,
            arrangement_frame: 101,
            loop_pass: Some(0),
            performance_beat_raw: Some(1200),
            arrangement_beat_raw: Some(1200),
            meter: Some((4, 4)),
            cycle_beats_raw: None,
            authored_clock_epoch: None,
        };
        let clock = WaveStageClock {
            window: &window,
            delay: 2,
        };
        assert_eq!(clock.arrangement_frame_at_offset(0), Some(98));
        assert_eq!(clock.arrangement_frame_at_offset(1), Some(99));
        assert_eq!(clock.arrangement_frame_at_offset(2), Some(100));
        assert_eq!(clock.arrangement_frame_at_offset(3), Some(101));
        assert_eq!(clock.contiguous_running_frames(0, 4), Some((98, 98)));
        assert_eq!(clock.constant_tempo_micro_bpm(0, 4), Some(120_000_000));
        assert_eq!(clock.arrangement_meter_at_offset(0), Some((4, 4)));
    }

    #[test]
    fn wave_clock_window_serves_high_pdc_delays_above_256_frames() {
        use crate::compensation::{
            CompensationLimits, CompensationRuntime, LatencyEdge, LatencyNode, LatencyPlan,
        };

        let plan = LatencyPlan::build(
            &[
                LatencyNode {
                    key: 0,
                    latency_frames: Some(4096),
                },
                LatencyNode {
                    key: 1,
                    latency_frames: Some(0),
                },
            ],
            &[LatencyEdge {
                key: 0,
                from: 0,
                to: 1,
            }],
            CompensationLimits::default(),
        )
        .unwrap();
        let mut runtime = CompensationRuntime::new(plan, CompensationLimits::default()).unwrap();
        runtime.begin_transaction().unwrap();
        for frame in 0..4096u64 {
            runtime
                .push_transport_stamp(TransportStamp {
                    authored_clock_epoch: None,
                    tempo_micro_bpm: Some(120_000_000),
                    performance_frame: frame,
                    transport_is_running: true,
                    arrangement_frame: frame,
                    loop_pass: 0,
                    performance_beat_raw: frame,
                    arrangement_beat_raw: Some(frame),
                    meter: None,
                    cycle_beats_raw: None,
                })
                .unwrap();
        }
        let incoming: Vec<Option<TransportStamp>> = (4096..4160u64)
            .map(|frame| {
                Some(TransportStamp {
                    authored_clock_epoch: None,
                    tempo_micro_bpm: Some(120_000_000),
                    performance_frame: frame,
                    transport_is_running: true,
                    arrangement_frame: frame,
                    loop_pass: 0,
                    performance_beat_raw: frame,
                    arrangement_beat_raw: Some(frame),
                    meter: None,
                    cycle_beats_raw: None,
                })
            })
            .collect();

        let mut window = WaveClockWindow::default();
        // Mix overlapping and disjoint high PDC arrival delays (>= 512 frames).
        let mut high = [4096, 512, 600, 2048, 512];
        window.fill_from_compensated_delays(&runtime, &incoming, 64, &mut high);
        assert!(window.high_delay_len >= 3);

        for delay in [64u32, 512, 600, 2048, 4096] {
            let clock = WaveStageClock {
                window: &window,
                delay,
            };
            let origin = crate::compensation::OriginWindow {
                history: &runtime,
                incoming: &incoming,
                delay,
            };
            for offset in 0..incoming.len() {
                assert_eq!(
                    clock.arrangement_frame_at_offset(offset),
                    origin.arrangement_frame_at_offset(offset),
                    "delay={delay} offset={offset}"
                );
                assert_eq!(
                    clock.performance_frame_at_offset(offset),
                    origin.performance_frame_at_offset(offset),
                    "delay={delay} offset={offset}"
                );
            }
            assert_eq!(
                clock.contiguous_running_frames(0, incoming.len()),
                origin.contiguous_running_frames(0, incoming.len()),
                "delay={delay}"
            );
        }
    }
}
