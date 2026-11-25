//! Sequential control-side scheduling for bounded Beats transient-anchored WSOLA warp.
//!
//! Supports `Tail::Forward`, `Tail::Alternating`, `Tail::Off`, `decay: 0..=100`,
//! `division_frames`, explicit `attacks`, unity copy/decay, and automatic streaming
//! transient detection over arbitrary streamed `TimeMap` extents up to
//! `MAX_STREAMED_OUTPUT_FRAMES` within `MAX_SCRATCH_BYTES = 32 MiB`.
mod bed;
mod onsets;

use self::{
    bed::{BedContinuation, BedSpan, BedState, Boundary, DrainedHistory, init_bed, step_bed},
    onsets::{MAX_CACHED_LOOP_REGION, OnsetScanner, OnsetScannerCheckpoint},
};
use crate::{
    Error, Frame, GranularOverlap, GranularSource, MAX_POINTS, MAX_SCRATCH_BYTES,
    MAX_STREAMED_OUTPUT_FRAMES, MAX_WORK_UNITS, Mode, Q32, Resampler, Tail, TimeMap, V2_MAX_TAPS,
    Work, finite, granular_overlap::OverlapCheckpoint, granular_sequential::next_renderer_id,
    resample::Bandlimited,
};

/// Maximum source-frame lookahead when searching for the next physical attack on a
/// long non-looping file before inserting a bounded virtual split. Any segment within
/// this horizon matches resident `beats::render` bit-for-bit.
const MAX_SEGMENT_HORIZON_FRAMES: u64 = 262_144;
const COPY_CHUNK_FRAMES: u64 = 1_024;
const FIRST_PASS_CHUNK_FRAMES: u64 = 512;

#[derive(Clone, Copy, Debug)]
struct Anchor {
    source: f64,
    output: u64,
    before: u64,
    after: u64,
    attack: bool,
}

fn smooth_cubic(x: f64) -> f64 {
    x * x * (3.0 - 2.0 * x)
}

fn smooth_quintic(x: f64) -> f64 {
    x * x * x * (10.0 + x * (-15.0 + 6.0 * x))
}

struct Baseline {
    fitted: [f64; 2],
    endpoint: [f64; 2],
}

fn restart_baseline<S: GranularSource>(
    sample_rate: u32,
    bank: &Bandlimited,
    start: i128,
    end: i128,
    pitch: f64,
    source: &mut S,
) -> Result<Baseline, S::Error> {
    if end <= start {
        return Ok(Baseline {
            fitted: [0.0; 2],
            endpoint: [0.0; 2],
        });
    }
    let available = (((end - start) as f64 / pitch).floor() as usize).max(1);
    let count = ((sample_rate / 2_000).max(2) as usize).min(available);
    let center = (count - 1) as f64 * 0.5;
    let mut sum = [0.0; 2];
    let mut square = [0.0; 2];
    let mut slope = [0.0; 2];
    let mut denominator = 0.0;
    let mut minimum = [f64::INFINITY; 2];
    let mut maximum = [f64::NEG_INFINITY; 2];
    let mut endpoint = [0.0; 2];
    for i in 0..count {
        let t = i as f64 - center;
        let sample = bank.try_sample_segment(
            end as f64 - (count - i) as f64 * pitch,
            start,
            end - 1,
            |at| source.frame(at),
        )?;
        endpoint = sample;
        denominator += t * t;
        for c in 0..2 {
            sum[c] += sample[c];
            square[c] += sample[c] * sample[c];
            slope[c] += t * sample[c];
            minimum[c] = minimum[c].min(sample[c]);
            maximum[c] = maximum[c].max(sample[c]);
        }
    }
    let variance: f64 = (0..2)
        .map(|c| (square[c] - sum[c] * sum[c] / count as f64).max(0.0))
        .sum();
    let fitted_energy = if denominator > 0.0 {
        slope.iter().map(|s| s * s / denominator).sum()
    } else {
        0.0
    };
    let predictable = variance - fitted_energy <= variance * 0.05 + 1e-16;
    let fitted = std::array::from_fn(|c| {
        let mean = sum[c] / count as f64;
        if predictable && denominator > 0.0 {
            (mean + slope[c] / denominator * center).clamp(minimum[c], maximum[c])
        } else {
            mean
        }
    });
    Ok(Baseline {
        fitted,
        endpoint: if predictable {
            endpoint
        } else {
            sum.map(|s| s / count as f64)
        },
    })
}

#[derive(Clone, Copy, Debug)]
struct PhysicalStreamState {
    next_cycle: i128,
    next_onset_local: u64,
    emitted_first: bool,
    emitted_last: bool,
    last_emitted_source: i128,
}

#[derive(Clone, Copy, Debug)]
struct ChosenStreamState {
    next_cycle: i128,
    next_item: usize,
    emitted_first: bool,
    emitted_last: bool,
    last_output: u64,
    last_source: f64,
}

#[derive(Clone, Copy, Debug)]
enum PhysicalPhase {
    NeedFirst,
    NeedNextAttack {
        prev_idx: usize,
        curr_idx: usize,
        output_cursor: u64,
        source_cursor: f64,
    },
    Bed {
        prev_idx: usize,
        curr_idx: usize,
        next_idx: usize,
        bed: BedState,
    },
    AttackGuard {
        prev_idx: usize,
        curr_idx: usize,
        next_idx: usize,
        frame: u64,
        end: u64,
    },
    FinalBed {
        bed: BedState,
    },
    Done,
}

#[derive(Clone, Copy, Debug)]
enum StopPhase {
    NeedSegment {
        seg_index: usize,
        previous_gap: bool,
        prev_seg_source: f64,
    },
    Sounding {
        seg_index: usize,
        a: Boundary,
        b: Boundary,
        duration: u64,
        sounding: u64,
        gap: bool,
        onset: u64,
        baseline: [f64; 2],
        join: u64,
        release: u64,
        age: u64,
        native_prev: usize,
        native_next: usize,
        join_prev: usize,
        join_next: usize,
    },
    Silence {
        seg_index: usize,
        a: Boundary,
        b: Boundary,
        at: u64,
        gap: bool,
    },
    Done,
}

#[derive(Clone, Copy, Debug)]
enum ChosenPhase {
    NeedGroup {
        group_index: usize,
        previous_cut_attack: Option<i128>,
        previous_aligned: bool,
        previous_source_cut: Option<f64>,
        previous_gap: bool,
        bounds_prev: usize,
        bounds_next: usize,
        future_idx: usize,
        last_attack: Option<usize>,
    },
    FirstPass {
        group_index: usize,
        a: Boundary,
        b: Boundary,
        duration: u64,
        sounding: u64,
        native_end: u64,
        sustain: bool,
        align_end: bool,
        overlap_len: u64,
        blend_start: u64,
        join: u64,
        correction: [f64; 2],
        release: u64,
        safe_start: f64,
        safe_end: f64,
        age: u64,
        excluded_before: Option<f64>,
        bounds_prev: usize,
        bounds_next: usize,
        future_idx: usize,
        last_attack: Option<usize>,
    },
    SustainBed {
        group_index: usize,
        b: Boundary,
        align_end: bool,
        previous_cut_attack: Option<i128>,
        previous_source_cut: Option<f64>,
        bounds_prev: usize,
        bounds_next: usize,
        future_idx: usize,
        last_attack: Option<usize>,
        bed: BedState,
    },
    Silence {
        group_index: usize,
        b: Boundary,
        at: u64,
        previous_cut_attack: Option<i128>,
        previous_aligned: bool,
        previous_source_cut: Option<f64>,
        previous_gap: bool,
        bounds_prev: usize,
        bounds_next: usize,
        future_idx: usize,
        last_attack: Option<usize>,
    },
    Done,
}

#[derive(Clone, Copy, Debug)]
enum EnginePhase {
    UnityCopy { cursor: u64 },
    UnityDecay { cursor: u64, seg_index: usize },
    Physical(PhysicalPhase),
    Stop(StopPhase),
    Chosen(ChosenPhase),
}

/// Instance-bound snapshot of Beats sequential WSOLA, onset scanner, and overlap state.
pub struct BeatsCheckpoint {
    owner: u64,
    budget: usize,
    overlap: OverlapCheckpoint,
    history: DrainedHistory,
    scanner: Option<OnsetScannerCheckpoint>,
    physical_anchors: Vec<Anchor>,
    chosen_boundaries: Vec<Boundary>,
    env_boundaries: Vec<Boundary>,
    physical_stream: PhysicalStreamState,
    chosen_stream: ChosenStreamState,
    phase: EnginePhase,
    ready_end: u64,
    pristine: bool,
}

impl BeatsCheckpoint {
    /// Output cursor captured with this checkpoint.
    pub fn position(&self) -> u64 {
        self.overlap.position()
    }

    /// Retained heap allocations, excluding this inline checkpoint value.
    pub fn backing_bytes(&self) -> Option<usize> {
        let mut total = self
            .overlap
            .backing_bytes()?
            .checked_add(self.history.backing_bytes()?)?;
        if let Some(scanner) = &self.scanner {
            total = total.checked_add(scanner.backing_bytes()?)?;
        }
        total = total.checked_add(
            self.physical_anchors
                .capacity()
                .checked_mul(std::mem::size_of::<Anchor>())?,
        )?;
        total = total.checked_add(
            self.chosen_boundaries
                .capacity()
                .checked_mul(std::mem::size_of::<Boundary>())?,
        )?;
        total = total.checked_add(
            self.env_boundaries
                .capacity()
                .checked_mul(std::mem::size_of::<Boundary>())?,
        )?;
        Some(total)
    }
}

/// Parameters for a sequential Beats transient-anchored WSOLA render.
pub struct SequentialBeatsConfig {
    pub map: TimeMap,
    pub sample_rate: u32,
    pub mode: Mode,
    pub pitch_cents: i16,
    pub resampler: Resampler,
    pub region_length: u64,
    pub looping: bool,
    pub attacks: Vec<u64>,
}

/// Bounded control-side Beats scheduler and overlap owner.
///
/// Each `advance` performs one bounded onset-scan chunk, WSOLA grain, attack guard,
/// or copy chunk. Drain every completed frame before advancing again. Do not run on
/// an audio callback.
pub struct SequentialBeats {
    config: SequentialBeatsConfig,
    owner: u64,
    bank: Option<Bandlimited>,
    overlap: GranularOverlap,
    history: DrainedHistory,
    scanner: Option<OnsetScanner>,
    total: u64,
    pitch: f64,
    division_frames: Option<u64>,
    tail: Tail,
    decay: u8,
    source_only: bool,
    first_source_q32: i128,
    end_source_q32: i128,
    last_cycle: i128,
    physical_anchors: Vec<Anchor>,
    chosen_boundaries: Vec<Boundary>,
    env_boundaries: Vec<Boundary>,
    physical_stream: PhysicalStreamState,
    chosen_stream: ChosenStreamState,
    phase: EnginePhase,
    ready_end: u64,
    max_source_frames: usize,
    pristine: bool,
    poisoned: bool,
}

impl SequentialBeats {
    /// Validate configuration and allocate only bounded filter, scanner, and overlap state.
    pub fn new(
        config: SequentialBeatsConfig,
        cancel: &mut dyn FnMut() -> bool,
    ) -> Result<Self, Error> {
        let Mode::Beats {
            division_frames,
            tail,
            decay,
        } = config.mode
        else {
            return Err(Error::Invalid("invalid sequential beats mode"));
        };
        if decay > 100 || division_frames == Some(0) {
            return Err(Error::Invalid("invalid transient division or decay"));
        }
        if !(8_000..=192_000).contains(&config.sample_rate)
            || config.region_length == 0
            || config.region_length > MAX_STREAMED_OUTPUT_FRAMES
        {
            return Err(Error::Invalid("invalid sequential beats source"));
        }
        if config.attacks.len() > MAX_POINTS
            || config.attacks.capacity() > MAX_POINTS
            || (!config.attacks.is_empty() && config.attacks.first() != Some(&0))
            || config
                .attacks
                .iter()
                .any(|&frame| frame >= config.region_length)
            || config.attacks.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(Error::Invalid(
                "explicit beat attacks must be sorted within the source region and begin at zero",
            ));
        }
        config
            .map
            .validate_stretch(config.mode, config.pitch_cents)?;
        let Some((first_seg, rest_segs)) = config.map.segments().split_first() else {
            return Err(Error::Invalid("empty beats map"));
        };
        let last_seg = rest_segs.last().unwrap_or(first_seg);
        let total = last_seg.output_end;
        if total == 0 || total > MAX_STREAMED_OUTPUT_FRAMES {
            return Err(Error::Invalid("invalid sequential beats extent"));
        }
        let pitch = 2.0_f64.powf(f64::from(config.pitch_cents) / 1_200.0);
        let unity = pitch == 1.0
            && config.map.segments().iter().all(|s| {
                s.source_end_q32 - s.source_start_q32
                    == i128::from(s.output_end - s.output_start) * Q32
            })
            && first_seg.source_start_q32.rem_euclid(Q32) == 0;
        let source_only = !config.attacks.is_empty() || division_frames.is_some();
        let owner = next_renderer_id()?;
        let max_grain = (config.sample_rate as usize / 20).max(4);
        let ring = if unity && decay == 100 {
            COPY_CHUNK_FRAMES as usize + 4
        } else {
            max_grain * 2 + 4
        };
        let overlap = GranularOverlap::new(total, ring)?;
        let history = DrainedHistory::new(config.sample_rate)?;
        let bank = if unity {
            None
        } else {
            let mut work = Work {
                cancel,
                used: 0,
                limit: MAX_WORK_UNITS,
                scratch: 0,
                base_scratch: 0,
                resampler: config.resampler,
            };
            work.check(0)?;
            Some(Bandlimited::new(pitch, &mut work)?)
        };
        let needs_scanner = !(unity && (decay == 100 || source_only));
        let scanner = if needs_scanner {
            Some(OnsetScanner::new(
                config.sample_rate,
                config.region_length,
                config.looping,
            )?)
        } else {
            None
        };
        let first_source_q32 = first_seg.source_start_q32;
        let end_source_q32 = last_seg.source_end_q32;
        let region = i128::from(config.region_length);
        let raw_first_cycle = first_source_q32.div_euclid(Q32).div_euclid(region);
        let first_cycle = if config.looping {
            raw_first_cycle
        } else {
            raw_first_cycle.max(0)
        };
        let last_cycle = end_source_q32.div_euclid(Q32).div_euclid(region);
        let phase = if unity && decay == 100 {
            EnginePhase::UnityCopy { cursor: 0 }
        } else if unity {
            EnginePhase::UnityDecay {
                cursor: 0,
                seg_index: 0,
            }
        } else if source_only {
            EnginePhase::Chosen(ChosenPhase::NeedGroup {
                group_index: 0,
                previous_cut_attack: None,
                previous_aligned: false,
                previous_source_cut: None,
                previous_gap: false,
                bounds_prev: 0,
                bounds_next: 1,
                future_idx: 0,
                last_attack: None,
            })
        } else if tail == Tail::Off {
            EnginePhase::Stop(StopPhase::NeedSegment {
                seg_index: 0,
                previous_gap: false,
                prev_seg_source: first_source_q32 as f64 / Q32 as f64,
            })
        } else {
            EnginePhase::Physical(PhysicalPhase::NeedFirst)
        };
        let max_source_frames = (((config.sample_rate as f64 / 25.0).max(4.0) * pitch)
            .ceil()
            .max(4096.0) as usize)
            + V2_MAX_TAPS
            + 8;
        let result = Self {
            config,
            owner,
            bank,
            overlap,
            history,
            scanner,
            total,
            pitch,
            division_frames,
            tail,
            decay,
            source_only,
            first_source_q32,
            end_source_q32,
            last_cycle,
            physical_anchors: Vec::new(),
            chosen_boundaries: Vec::new(),
            env_boundaries: Vec::new(),
            physical_stream: PhysicalStreamState {
                next_cycle: first_cycle,
                next_onset_local: 0,
                emitted_first: false,
                emitted_last: false,
                last_emitted_source: first_source_q32.div_euclid(Q32),
            },
            chosen_stream: ChosenStreamState {
                next_cycle: first_cycle,
                next_item: 0,
                emitted_first: false,
                emitted_last: false,
                last_output: 0,
                last_source: first_source_q32 as f64 / Q32 as f64,
            },
            phase,
            ready_end: 0,
            max_source_frames,
            pristine: true,
            poisoned: false,
        };
        let aggregate = result
            .backing_bytes()
            .and_then(|bytes| {
                max_source_frames
                    .checked_mul(std::mem::size_of::<[f64; 2]>())
                    .and_then(|scratch| bytes.checked_add(scratch))
            })
            .ok_or(Error::ScratchLimit)?;
        if aggregate > MAX_SCRATCH_BYTES {
            return Err(Error::ScratchLimit);
        }
        Ok(result)
    }

    pub fn is_pristine(&self) -> bool {
        !self.poisoned && self.pristine && self.position() == 0
    }

    pub fn total_frames(&self) -> u64 {
        self.total
    }

    pub fn sample_rate(&self) -> u32 {
        self.config.sample_rate
    }

    pub fn max_source_frames(&self) -> usize {
        self.max_source_frames
    }

    pub fn available(&self) -> usize {
        (self.ready_end - self.overlap.position()) as usize
    }

    pub fn position(&self) -> u64 {
        self.overlap.position()
    }

    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    pub fn is_complete(&self) -> bool {
        !self.poisoned && self.position() == self.total
    }

    /// Retained heap allocations, excluding this inline object.
    pub fn backing_bytes(&self) -> Option<usize> {
        let mut total = self
            .config
            .map
            .backing_bytes()?
            .checked_add(
                self.config
                    .attacks
                    .capacity()
                    .checked_mul(std::mem::size_of::<u64>())?,
            )?
            .checked_add(self.overlap.backing_bytes()?)?
            .checked_add(self.history.backing_bytes()?)?;
        if let Some(bank) = &self.bank {
            total = total.checked_add(bank.backing_bytes()?)?;
        }
        if let Some(scanner) = &self.scanner {
            total = total.checked_add(scanner.backing_bytes()?)?;
        }
        total = total.checked_add(
            self.physical_anchors
                .capacity()
                .checked_mul(std::mem::size_of::<Anchor>())?,
        )?;
        total = total.checked_add(
            self.chosen_boundaries
                .capacity()
                .checked_mul(std::mem::size_of::<Boundary>())?,
        )?;
        total = total.checked_add(
            self.env_boundaries
                .capacity()
                .checked_mul(std::mem::size_of::<Boundary>())?,
        )?;
        Some(total)
    }

    pub fn checkpoint(&self, budget: usize) -> Result<BeatsCheckpoint, Error> {
        if self.poisoned {
            return Err(Error::Invalid("beats renderer is poisoned"));
        }
        let available = MAX_SCRATCH_BYTES
            .checked_sub(self.backing_bytes().ok_or(Error::ScratchLimit)?)
            .and_then(|n| {
                self.max_source_frames
                    .checked_mul(std::mem::size_of::<[f64; 2]>())
                    .and_then(|scratch| n.checked_sub(scratch))
            })
            .ok_or(Error::ScratchLimit)?;
        let effective = budget.min(available);
        let overlap = self.overlap.checkpoint(effective)?;
        let rem = effective
            .checked_sub(overlap.backing_bytes().ok_or(Error::ScratchLimit)?)
            .ok_or(Error::ScratchLimit)?;
        let history = self.history.checkpoint(rem)?;
        let mut rem = rem
            .checked_sub(history.backing_bytes().ok_or(Error::ScratchLimit)?)
            .ok_or(Error::ScratchLimit)?;
        let scanner = if let Some(sc) = &self.scanner {
            let cp = sc.checkpoint(rem)?;
            rem = rem
                .checked_sub(cp.backing_bytes().ok_or(Error::ScratchLimit)?)
                .ok_or(Error::ScratchLimit)?;
            Some(cp)
        } else {
            None
        };
        let vec_bytes = self
            .physical_anchors
            .len()
            .checked_mul(std::mem::size_of::<Anchor>())
            .and_then(|v| {
                v.checked_add(
                    self.chosen_boundaries
                        .len()
                        .checked_mul(std::mem::size_of::<Boundary>())?,
                )
            })
            .and_then(|v| {
                v.checked_add(
                    self.env_boundaries
                        .len()
                        .checked_mul(std::mem::size_of::<Boundary>())?,
                )
            })
            .ok_or(Error::ScratchLimit)?;
        if vec_bytes > rem {
            return Err(Error::ScratchLimit);
        }
        let mut physical_anchors = Vec::new();
        physical_anchors
            .try_reserve_exact(self.physical_anchors.len())
            .map_err(|_| Error::Allocation)?;
        physical_anchors.extend_from_slice(&self.physical_anchors);
        let mut chosen_boundaries = Vec::new();
        chosen_boundaries
            .try_reserve_exact(self.chosen_boundaries.len())
            .map_err(|_| Error::Allocation)?;
        chosen_boundaries.extend_from_slice(&self.chosen_boundaries);
        let mut env_boundaries = Vec::new();
        env_boundaries
            .try_reserve_exact(self.env_boundaries.len())
            .map_err(|_| Error::Allocation)?;
        env_boundaries.extend_from_slice(&self.env_boundaries);
        let checkpoint = BeatsCheckpoint {
            owner: self.owner,
            budget: effective,
            overlap,
            history,
            scanner,
            physical_anchors,
            chosen_boundaries,
            env_boundaries,
            physical_stream: self.physical_stream,
            chosen_stream: self.chosen_stream,
            phase: self.phase,
            ready_end: self.ready_end,
            pristine: self.pristine,
        };
        if checkpoint
            .backing_bytes()
            .is_none_or(|bytes| bytes > effective)
        {
            return Err(Error::ScratchLimit);
        }
        Ok(checkpoint)
    }

    pub fn owns_checkpoint(&self, checkpoint: &BeatsCheckpoint) -> bool {
        checkpoint.owner == self.owner
    }

    /// Refresh an existing checkpoint in place within its originally admitted
    /// slot byte budget. Leaves `checkpoint` unchanged on error.
    pub fn refresh_checkpoint(&self, checkpoint: &mut BeatsCheckpoint) -> Result<(), Error> {
        self.refresh_checkpoint_with_budget(checkpoint, checkpoint.budget)
    }

    /// Refresh an existing checkpoint in place within `budget` bytes (capped by
    /// the renderer's remaining scratch allowance). Leaves `checkpoint`
    /// completely unchanged if the budget is exceeded or allocation fails.
    pub fn refresh_checkpoint_with_budget(
        &self,
        checkpoint: &mut BeatsCheckpoint,
        budget: usize,
    ) -> Result<(), Error> {
        if self.poisoned {
            return Err(Error::Invalid("beats renderer is poisoned"));
        }
        if !self.owns_checkpoint(checkpoint) {
            return Err(Error::Invalid("foreign beats checkpoint"));
        }
        let available = MAX_SCRATCH_BYTES
            .checked_sub(self.backing_bytes().ok_or(Error::ScratchLimit)?)
            .and_then(|n| {
                self.max_source_frames
                    .checked_mul(std::mem::size_of::<[f64; 2]>())
                    .and_then(|scratch| n.checked_sub(scratch))
            })
            .ok_or(Error::ScratchLimit)?;
        let effective = budget.min(available);
        let fixed_bytes = checkpoint
            .overlap
            .backing_bytes()
            .and_then(|a| a.checked_add(checkpoint.history.backing_bytes()?))
            .ok_or(Error::ScratchLimit)?;
        let mut rem = effective
            .checked_sub(fixed_bytes)
            .ok_or(Error::ScratchLimit)?;
        let scanner_onsets = match (&self.scanner, &checkpoint.scanner) {
            (Some(sc), Some(cp)) => {
                let (replacement, bytes) = sc.prepare_refresh_onsets(cp, rem)?;
                rem = rem.checked_sub(bytes).ok_or(Error::ScratchLimit)?;
                Some(replacement)
            }
            (None, None) => None,
            _ => return Err(Error::Invalid("incompatible beats checkpoint")),
        };
        let (anchors_rep, anchors_bytes) = prepare_vec_within(
            &self.physical_anchors,
            checkpoint.physical_anchors.capacity(),
            rem,
        )?;
        rem = rem.checked_sub(anchors_bytes).ok_or(Error::ScratchLimit)?;
        let (chosen_rep, chosen_bytes) = prepare_vec_within(
            &self.chosen_boundaries,
            checkpoint.chosen_boundaries.capacity(),
            rem,
        )?;
        rem = rem.checked_sub(chosen_bytes).ok_or(Error::ScratchLimit)?;
        let (env_rep, env_bytes) = prepare_vec_within(
            &self.env_boundaries,
            checkpoint.env_boundaries.capacity(),
            rem,
        )?;
        let _ = rem.checked_sub(env_bytes).ok_or(Error::ScratchLimit)?;

        // All budgets and fallible reservations succeeded; validate fixed-shape
        // copies before committing dynamic buffers and metadata.
        self.overlap.refresh_checkpoint(&mut checkpoint.overlap)?;
        self.history.refresh_checkpoint(&mut checkpoint.history)?;
        if let (Some(sc), Some(cp), Some(onsets_rep)) =
            (&self.scanner, &mut checkpoint.scanner, scanner_onsets)
        {
            sc.commit_refresh(cp, onsets_rep);
        }
        commit_vec_replacement(
            &self.physical_anchors,
            &mut checkpoint.physical_anchors,
            anchors_rep,
        );
        commit_vec_replacement(
            &self.chosen_boundaries,
            &mut checkpoint.chosen_boundaries,
            chosen_rep,
        );
        commit_vec_replacement(
            &self.env_boundaries,
            &mut checkpoint.env_boundaries,
            env_rep,
        );
        checkpoint.budget = effective;
        checkpoint.physical_stream = self.physical_stream;
        checkpoint.chosen_stream = self.chosen_stream;
        checkpoint.phase = self.phase;
        checkpoint.ready_end = self.ready_end;
        checkpoint.pristine = self.pristine;
        debug_assert!(
            checkpoint
                .backing_bytes()
                .is_some_and(|bytes| bytes <= effective)
        );
        Ok(())
    }

    pub fn restore(&mut self, checkpoint: &BeatsCheckpoint) -> Result<(), Error> {
        if checkpoint.owner != self.owner {
            return Err(Error::Invalid("foreign beats checkpoint"));
        }
        self.overlap.restore(&checkpoint.overlap)?;
        self.history.restore(&checkpoint.history)?;
        match (&mut self.scanner, &checkpoint.scanner) {
            (Some(sc), Some(cp)) => sc.restore(cp)?,
            (None, None) => {}
            _ => return Err(Error::Invalid("incompatible beats checkpoint")),
        }
        copy_vec(&checkpoint.physical_anchors, &mut self.physical_anchors)?;
        copy_vec(&checkpoint.chosen_boundaries, &mut self.chosen_boundaries)?;
        copy_vec(&checkpoint.env_boundaries, &mut self.env_boundaries)?;
        self.physical_stream = checkpoint.physical_stream;
        self.chosen_stream = checkpoint.chosen_stream;
        self.phase = checkpoint.phase;
        self.ready_end = checkpoint.ready_end;
        self.pristine = checkpoint.pristine;
        self.poisoned = false;
        Ok(())
    }

    /// Perform one bounded Beats synthesis or onset-scan step.
    pub fn advance<S: GranularSource>(
        &mut self,
        source: &mut S,
        scratch: &mut [[f64; 2]],
        cancel: &mut dyn FnMut() -> bool,
    ) -> Result<bool, S::Error> {
        if self.poisoned {
            return Err(Error::Invalid("beats renderer is poisoned").into());
        }
        if self.available() != 0 {
            return Err(Error::Invalid("beats output must be drained first").into());
        }
        if self.is_complete() {
            return Ok(false);
        }
        if scratch.len() < self.max_source_frames
            || self
                .backing_bytes()
                .and_then(|bytes| {
                    scratch
                        .len()
                        .checked_mul(std::mem::size_of::<[f64; 2]>())
                        .and_then(|n| bytes.checked_add(n))
                })
                .is_none_or(|bytes| bytes > MAX_SCRATCH_BYTES)
        {
            return Err(Error::ScratchLimit.into());
        }
        if cancel() {
            return Err(Error::Cancelled.into());
        }
        self.pristine = false;
        let step_result = match self.phase {
            EnginePhase::UnityCopy { cursor } => self.step_unity_copy(cursor, source, cancel),
            EnginePhase::UnityDecay { cursor, seg_index } => {
                self.step_unity_decay(cursor, seg_index, source, scratch, cancel)
            }
            EnginePhase::Physical(p) => self.step_physical(p, source, scratch, cancel),
            EnginePhase::Stop(p) => self.step_stop(p, source, scratch, cancel),
            EnginePhase::Chosen(p) => self.step_chosen(p, source, scratch, cancel),
        };
        if let Err(err) = step_result {
            self.poisoned = true;
            return Err(err);
        }
        Ok(true)
    }

    /// Drain at most `available()` frames, recording pre-decay history and applying
    /// logical segment decay envelopes when `decay < 100`.
    pub fn drain(&mut self, output: &mut [Frame]) -> Result<(), Error> {
        if self.poisoned {
            return Err(Error::Invalid("beats renderer is poisoned"));
        }
        if output.len() > self.available() {
            return Err(Error::Invalid("beats drain exceeds completed frames"));
        }
        let start_pos = self.overlap.position();
        if let Err(error) = self.overlap.drain(output) {
            self.poisoned = true;
            return Err(error);
        }
        for (i, frame) in output.iter_mut().enumerate() {
            self.history.push(*frame);
            if self.decay < 100 {
                let pos = start_pos + i as u64;
                if let Some((seg_start, seg_end)) = self.envelope_segment(pos) {
                    let age = pos - seg_start;
                    let gain = (-12.0 * f64::from(100 - self.decay) / 100.0 * age as f64
                        / (seg_end - seg_start) as f64)
                        .exp();
                    match finite(frame.map(|x| f64::from(x) * gain)) {
                        Ok(scaled) => *frame = scaled,
                        Err(error) => {
                            self.poisoned = true;
                            return Err(error);
                        }
                    }
                }
            }
        }
        if self.decay < 100 {
            let cursor = self.overlap.position();
            while self.env_boundaries.len() > 2 && self.env_boundaries[1].output <= cursor {
                self.env_boundaries.remove(0);
            }
        }
        Ok(())
    }

    fn envelope_segment(&self, pos: u64) -> Option<(u64, u64)> {
        if self.env_boundaries.len() < 2 {
            return None;
        }
        let mut lo = 0;
        let mut hi = self.env_boundaries.len() - 1;
        while lo + 1 < hi {
            let mid = (lo + hi) / 2;
            if self.env_boundaries[mid].output <= pos {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let a = self.env_boundaries[lo].output;
        let b = self.env_boundaries[lo + 1].output;
        (b > a && pos >= a && pos < b).then_some((a, b))
    }

    fn record_env_boundary(&mut self, boundary: Boundary) -> Result<(), Error> {
        if self.decay == 100 {
            return Ok(());
        }
        if let Some(last) = self.env_boundaries.last()
            && last.output >= boundary.output
        {
            return Ok(());
        }
        self.env_boundaries
            .try_reserve(1)
            .map_err(|_| Error::Allocation)?;
        self.env_boundaries.push(boundary);
        Ok(())
    }

    fn push_physical_anchor(
        anchors: &mut Vec<Anchor>,
        source: f64,
        output: u64,
        attack: bool,
        source_only: bool,
    ) -> Result<(), Error> {
        if let Some(last) = anchors.last_mut() {
            if source_only && (last.source - source).abs() < 1e-9 {
                last.attack |= attack;
                return Ok(());
            }
            if !source_only && last.output == output {
                if (last.source - source).abs() > 1e-9 && attack && last.attack {
                    return Err(Error::Invalid(
                        "distinct attacks project onto the same output sample",
                    ));
                }
                if attack && !last.attack {
                    last.source = source;
                    last.attack = true;
                }
                return Ok(());
            }
        }
        anchors.try_reserve(1).map_err(|_| Error::Allocation)?;
        anchors.push(Anchor {
            source,
            output,
            before: 0,
            after: 0,
            attack,
        });
        Ok(())
    }

    /// Advance the physical onset scanner and emit physical anchors up to `target_source`
    /// (unwrapped source frames). Returns `Ok(true)` once all physical anchors up to
    /// `target_source` (or clip end) are present in `self.physical_anchors`.
    fn ensure_physical_through<S: GranularSource>(
        &mut self,
        target_source: i128,
        source: &mut S,
        scratch: &mut [[f64; 2]],
        cancel: &mut dyn FnMut() -> bool,
    ) -> Result<bool, S::Error> {
        if !self.physical_stream.emitted_first {
            Self::push_physical_anchor(
                &mut self.physical_anchors,
                self.first_source_q32 as f64 / Q32 as f64,
                0,
                false,
                self.source_only,
            )
            .map_err(S::Error::from)?;
            if !self.source_only {
                self.record_env_boundary(Boundary {
                    source: self.first_source_q32 as f64 / Q32 as f64,
                    output: 0,
                })
                .map_err(S::Error::from)?;
            }
            self.physical_stream.emitted_first = true;
        }
        if self.physical_stream.emitted_last {
            return Ok(true);
        }
        let region = i128::from(self.config.region_length);
        let max_src = self.end_source_q32.div_euclid(Q32);
        let goal_src = target_source.min(max_src);

        while self.physical_stream.next_cycle <= self.last_cycle {
            let cycle = self.physical_stream.next_cycle;
            if cycle > 0 && !self.config.looping {
                break;
            }
            let cycle_start = cycle * region;
            if cycle_start > goal_src {
                return Ok(true);
            }
            let need_local = if goal_src >= cycle_start + region {
                self.config.region_length
            } else {
                ((goal_src - cycle_start).max(0) as u64 + 1).min(self.config.region_length)
            };
            let Some(scanner) = self.scanner.as_mut() else {
                return Err(Error::Invalid("beats onset scanner missing").into());
            };
            if !scanner.step_toward(source, scratch, need_local, cancel)? {
                return Ok(false);
            }
            let boundary_attack = scanner.boundary_attack();
            let finalized = scanner.finalized_through();
            let start_idx = scanner
                .onsets()
                .partition_point(|&at| at < self.physical_stream.next_onset_local);
            let mut emitted_local = self.physical_stream.next_onset_local;
            for &at in &scanner.onsets()[start_idx..] {
                if at > finalized && finalized < self.config.region_length {
                    break;
                }
                if at >= need_local && need_local < self.config.region_length {
                    break;
                }
                let src = cycle * region + i128::from(at);
                if let Some(output) = self.config.map.output_for_source(src * Q32)
                    && output < self.total
                {
                    let attack = at != 0 || (output != 0 && boundary_attack);
                    Self::push_physical_anchor(
                        &mut self.physical_anchors,
                        src as f64,
                        output,
                        attack,
                        self.source_only,
                    )
                    .map_err(S::Error::from)?;
                    if !self.source_only && self.decay < 100 {
                        let boundary = Boundary {
                            source: src as f64,
                            output,
                        };
                        if self
                            .env_boundaries
                            .last()
                            .is_none_or(|last| last.output < boundary.output)
                        {
                            self.env_boundaries
                                .try_reserve(1)
                                .map_err(|_| S::Error::from(Error::Allocation))?;
                            self.env_boundaries.push(boundary);
                        }
                    }
                    self.physical_stream.last_emitted_source = src;
                }
                emitted_local = at.saturating_add(1);
            }
            let at_clip_end = cycle == self.last_cycle && goal_src >= max_src;
            let advance_local = if need_local >= self.config.region_length {
                finalized
            } else {
                need_local.min(finalized)
            };
            self.physical_stream.next_onset_local = emitted_local.max(advance_local);
            if (need_local >= self.config.region_length || at_clip_end) && finalized >= need_local {
                self.physical_stream.next_cycle += 1;
                self.physical_stream.next_onset_local = 0;
            } else {
                return Ok(true);
            }
        }
        Self::push_physical_anchor(
            &mut self.physical_anchors,
            self.end_source_q32 as f64 / Q32 as f64,
            self.total,
            false,
            self.source_only,
        )
        .map_err(S::Error::from)?;
        if !self.source_only {
            self.record_env_boundary(Boundary {
                source: self.end_source_q32 as f64 / Q32 as f64,
                output: self.total,
            })
            .map_err(S::Error::from)?;
        }
        self.physical_stream.emitted_last = true;
        Ok(true)
    }

    /// Advance the physical stream until we have at least one attack anchor strictly
    /// after `after_idx`, or we hit the end anchor, or we reach `MAX_SEGMENT_HORIZON_FRAMES`
    /// past `physical_anchors[after_idx]` on a long non-looping region.
    fn ensure_next_physical_attack<S: GranularSource>(
        &mut self,
        after_idx: usize,
        source: &mut S,
        scratch: &mut [[f64; 2]],
        cancel: &mut dyn FnMut() -> bool,
    ) -> Result<Option<usize>, S::Error> {
        let base_src = self.physical_anchors[after_idx].source.floor() as i128;
        let is_short_or_cached = self.config.region_length <= MAX_CACHED_LOOP_REGION;
        let horizon_src = if is_short_or_cached {
            // For normal / cached-loop regions, allow reaching the next onset within
            // one full cycle (or clip end) so behavior matches resident bit-for-bit.
            (base_src + i128::from(self.config.region_length) + 1)
                .min(self.end_source_q32.div_euclid(Q32))
        } else {
            (base_src + i128::from(MAX_SEGMENT_HORIZON_FRAMES))
                .min(self.end_source_q32.div_euclid(Q32))
        };
        loop {
            if let Some(pos) = (after_idx + 1..self.physical_anchors.len())
                .find(|&j| self.physical_anchors[j].attack)
            {
                return Ok(Some(pos));
            }
            if self.physical_stream.emitted_last {
                return Ok(Some(self.physical_anchors.len() - 1));
            }
            let current_frontier = if self.physical_stream.next_cycle > self.last_cycle {
                self.end_source_q32.div_euclid(Q32)
            } else {
                self.physical_stream.next_cycle * i128::from(self.config.region_length)
                    + i128::from(self.physical_stream.next_onset_local)
            };
            if current_frontier >= horizon_src {
                // On a long quiet non-looping span (>262,144 frames with zero onsets),
                // emit a bounded virtual non-attack split so streaming stays O(1) lookahead.
                if !is_short_or_cached
                    && horizon_src < self.end_source_q32.div_euclid(Q32)
                    && let Some(out) = self.config.map.output_for_source(horizon_src * Q32)
                    && out > self.physical_anchors[after_idx].output
                    && out < self.total
                {
                    Self::push_physical_anchor(
                        &mut self.physical_anchors,
                        horizon_src as f64,
                        out,
                        false,
                        false,
                    )
                    .map_err(S::Error::from)?;
                    self.record_env_boundary(Boundary {
                        source: horizon_src as f64,
                        output: out,
                    })
                    .map_err(S::Error::from)?;
                    return Ok(Some(self.physical_anchors.len() - 1));
                }
                // Advance slightly further if cached loop region spans multiple quiet cycles.
                let next_target =
                    (current_frontier + 16_384).min(self.end_source_q32.div_euclid(Q32));
                if !self.ensure_physical_through(next_target, source, scratch, cancel)? {
                    return Ok(None);
                }
                continue;
            }
            let step_target = (current_frontier + 16_384).min(horizon_src);
            if !self.ensure_physical_through(step_target, source, scratch, cancel)? {
                return Ok(None);
            }
        }
    }

    /// Generate the next logical boundary in `self.chosen_boundaries` (for `division_frames`
    /// or explicit `attacks`). Pure metadata math, O(1) work per boundary.
    fn ensure_chosen_boundary(&mut self, needed_index: usize) -> Result<(), Error> {
        if !self.chosen_stream.emitted_first {
            let first = Boundary {
                source: self.first_source_q32 as f64 / Q32 as f64,
                output: 0,
            };
            self.chosen_boundaries
                .try_reserve(1)
                .map_err(|_| Error::Allocation)?;
            self.chosen_boundaries.push(first);
            self.record_env_boundary(first)?;
            self.chosen_stream.emitted_first = true;
            self.chosen_stream.last_output = 0;
            self.chosen_stream.last_source = first.source;
        }
        let region = i128::from(self.config.region_length);
        let step_division = if self.config.attacks.is_empty() {
            Some(
                self.division_frames
                    .ok_or(Error::Invalid("preserve division missing"))?,
            )
        } else {
            None
        };
        let count_per_cycle = if let Some(division) = step_division {
            usize::try_from((self.config.region_length - 1) / division + 1)
                .map_err(|_| Error::Invalid("beat divisions exceed bounded source plan"))?
        } else {
            self.config.attacks.len()
        };
        while self.chosen_boundaries.len() <= needed_index && !self.chosen_stream.emitted_last {
            if self.chosen_stream.next_cycle > self.last_cycle
                || (self.chosen_stream.next_cycle > 0 && !self.config.looping)
            {
                let end_b = Boundary {
                    source: self.end_source_q32 as f64 / Q32 as f64,
                    output: self.total,
                };
                self.chosen_boundaries
                    .try_reserve(1)
                    .map_err(|_| Error::Allocation)?;
                self.chosen_boundaries.push(end_b);
                self.record_env_boundary(end_b)?;
                self.chosen_stream.emitted_last = true;
                break;
            }
            let cycle = self.chosen_stream.next_cycle;
            let i = self.chosen_stream.next_item;
            if i >= count_per_cycle {
                self.chosen_stream.next_cycle += 1;
                self.chosen_stream.next_item = 0;
                continue;
            }
            self.chosen_stream.next_item += 1;
            let at = if let Some(division) = step_division {
                i as u64 * division
            } else {
                self.config.attacks[i]
            };
            let src = cycle * region + i128::from(at);
            if let Some(output) = self.config.map.output_for_source(src * Q32)
                && output < self.total
                && output >= self.chosen_stream.last_output
                && (src as f64) > self.chosen_stream.last_source
            {
                let b = Boundary {
                    source: src as f64,
                    output,
                };
                self.chosen_boundaries
                    .try_reserve(1)
                    .map_err(|_| Error::Allocation)?;
                self.chosen_boundaries.push(b);
                self.record_env_boundary(b)?;
                self.chosen_stream.last_output = output;
                self.chosen_stream.last_source = b.source;
            }
        }
        Ok(())
    }

    fn compute_anchor_guards(&mut self, prev: usize, curr: usize, next: usize) {
        let rate = f64::from(self.config.sample_rate);
        let pitch = self.pitch;
        let before = if curr == 0 {
            0
        } else {
            ((rate / 1000.0 / pitch).floor() as u64)
                .min((self.physical_anchors[curr].output - self.physical_anchors[prev].output) / 3)
                .min(
                    ((self.physical_anchors[curr].source - self.physical_anchors[prev].source)
                        / pitch
                        / 3.0)
                        .floor()
                        .max(0.0) as u64,
                )
        };
        let after = ((rate / 125.0 / pitch).floor() as u64)
            .max(1)
            .min(
                (self.physical_anchors[next].output - self.physical_anchors[curr].output)
                    .div_ceil(3),
            )
            .min(
                ((self.physical_anchors[next].source - self.physical_anchors[curr].source)
                    / pitch
                    / 3.0)
                    .ceil()
                    .max(1.0) as u64,
            );
        self.physical_anchors[curr].before = before;
        self.physical_anchors[curr].after = after;
    }

    fn prune_physical_anchors(&mut self, keep_from: usize) -> usize {
        if keep_from <= 16 {
            return 0;
        }
        let drop_count = keep_from - 1;
        let floor_src = self.physical_anchors[drop_count].source.floor().max(0.0) as u64;
        self.physical_anchors.drain(..drop_count);
        if let Some(scanner) = self.scanner.as_mut() {
            scanner.prune_before(floor_src);
        }
        drop_count
    }

    fn step_unity_copy<S: GranularSource>(
        &mut self,
        cursor: u64,
        source: &mut S,
        cancel: &mut dyn FnMut() -> bool,
    ) -> Result<(), S::Error> {
        let end = (cursor + COPY_CHUNK_FRAMES).min(self.total);
        for frame in cursor..end {
            if (frame - cursor).is_multiple_of(512) && cancel() {
                return Err(Error::Cancelled.into());
            }
            let pos = self
                .config
                .map
                .position_q32(i128::from(frame))
                .map_err(S::Error::from)?
                / Q32;
            let sample = source.frame(pos)?;
            self.overlap
                .add(i128::from(frame), sample, 1.0)
                .map_err(S::Error::from)?;
        }
        self.ready_end = end;
        self.phase = EnginePhase::UnityCopy { cursor: end };
        Ok(())
    }

    fn step_unity_decay<S: GranularSource>(
        &mut self,
        cursor: u64,
        mut seg_index: usize,
        source: &mut S,
        scratch: &mut [[f64; 2]],
        cancel: &mut dyn FnMut() -> bool,
    ) -> Result<(), S::Error> {
        // Ensure we have logical boundaries covering `cursor..end`.
        let chunk_end = (cursor + COPY_CHUNK_FRAMES).min(self.total);
        if self.source_only {
            loop {
                self.ensure_chosen_boundary(seg_index + 1)
                    .map_err(S::Error::from)?;
                if self.chosen_boundaries[seg_index + 1].output >= chunk_end
                    || self.chosen_stream.emitted_last
                {
                    break;
                }
                seg_index += 1;
            }
        } else {
            let target_src = self
                .config
                .map
                .position_q32(i128::from(chunk_end))
                .map_err(S::Error::from)?
                .div_euclid(Q32)
                + 1;
            if !self.ensure_physical_through(target_src, source, scratch, cancel)? {
                return Ok(());
            }
        }
        for frame in cursor..chunk_end {
            if (frame - cursor).is_multiple_of(512) && cancel() {
                return Err(Error::Cancelled.into());
            }
            let pos = self
                .config
                .map
                .position_q32(i128::from(frame))
                .map_err(S::Error::from)?
                / Q32;
            let sample = source.frame(pos)?;
            self.overlap
                .add(i128::from(frame), sample, 1.0)
                .map_err(S::Error::from)?;
        }
        self.ready_end = chunk_end;
        self.phase = EnginePhase::UnityDecay {
            cursor: chunk_end,
            seg_index,
        };
        Ok(())
    }

    fn step_physical<S: GranularSource>(
        &mut self,
        phase: PhysicalPhase,
        source: &mut S,
        scratch: &mut [[f64; 2]],
        cancel: &mut dyn FnMut() -> bool,
    ) -> Result<(), S::Error> {
        match phase {
            PhysicalPhase::NeedFirst => {
                // Ensure first anchor (index 0) is present.
                let first_src = self.first_source_q32.div_euclid(Q32);
                if !self.ensure_physical_through(first_src, source, scratch, cancel)? {
                    return Ok(());
                }
                let Some(next_idx) =
                    self.ensure_next_physical_attack(0, source, scratch, cancel)?
                else {
                    return Ok(());
                };
                self.compute_anchor_guards(0, 0, next_idx);
                let anchor0 = self.physical_anchors[0];
                let end = (anchor0.output + anchor0.after).min(self.total);
                self.phase = EnginePhase::Physical(PhysicalPhase::AttackGuard {
                    prev_idx: 0,
                    curr_idx: 0,
                    next_idx,
                    frame: 0,
                    end,
                });
                Ok(())
            }
            PhysicalPhase::NeedNextAttack {
                mut prev_idx,
                mut curr_idx,
                output_cursor,
                source_cursor,
            } => {
                let dropped = self.prune_physical_anchors(prev_idx);
                prev_idx -= dropped;
                curr_idx -= dropped;
                let curr = self.physical_anchors[curr_idx];
                if curr.output >= self.total {
                    // No more attack anchors; run final bed to `total`.
                    let span = BedSpan {
                        start: output_cursor,
                        end: self.total,
                        source_start: source_cursor,
                        source_end: curr.source,
                        locked_end: false,
                    };
                    if let Some(bed) = init_bed(
                        span,
                        None,
                        self.config.sample_rate,
                        self.pitch,
                        &self.config.map,
                        &self.history,
                    )
                    .map_err(S::Error::from)?
                    {
                        self.phase = EnginePhase::Physical(PhysicalPhase::FinalBed { bed });
                    } else {
                        self.ready_end = self.total;
                        self.phase = EnginePhase::Physical(PhysicalPhase::Done);
                    }
                    return Ok(());
                }
                if !curr.attack {
                    // Virtual horizon split on a long non-looping silent stretch.
                    let span = BedSpan {
                        start: output_cursor,
                        end: curr.output,
                        source_start: source_cursor,
                        source_end: curr.source,
                        locked_end: false,
                    };
                    let Some(next_idx) =
                        self.ensure_next_physical_attack(curr_idx, source, scratch, cancel)?
                    else {
                        self.phase = EnginePhase::Physical(PhysicalPhase::NeedNextAttack {
                            prev_idx,
                            curr_idx,
                            output_cursor,
                            source_cursor,
                        });
                        return Ok(());
                    };
                    if let Some(bed) = init_bed(
                        span,
                        None,
                        self.config.sample_rate,
                        self.pitch,
                        &self.config.map,
                        &self.history,
                    )
                    .map_err(S::Error::from)?
                    {
                        self.phase = EnginePhase::Physical(PhysicalPhase::Bed {
                            prev_idx,
                            curr_idx,
                            next_idx,
                            bed,
                        });
                    } else {
                        self.phase = EnginePhase::Physical(PhysicalPhase::NeedNextAttack {
                            prev_idx: curr_idx,
                            curr_idx: next_idx,
                            output_cursor: curr.output,
                            source_cursor: curr.source,
                        });
                    }
                    return Ok(());
                }
                let Some(next_idx) =
                    self.ensure_next_physical_attack(curr_idx, source, scratch, cancel)?
                else {
                    self.phase = EnginePhase::Physical(PhysicalPhase::NeedNextAttack {
                        prev_idx,
                        curr_idx,
                        output_cursor,
                        source_cursor,
                    });
                    return Ok(());
                };
                self.compute_anchor_guards(prev_idx, curr_idx, next_idx);
                let curr = self.physical_anchors[curr_idx];
                let start = curr.output.saturating_sub(curr.before);
                let end = (curr.output + curr.after).min(self.total);
                let span = BedSpan {
                    start: output_cursor,
                    end: start,
                    source_start: source_cursor,
                    source_end: curr.source - curr.before as f64 * self.pitch,
                    locked_end: curr.attack,
                };
                if let Some(bed) = init_bed(
                    span,
                    None,
                    self.config.sample_rate,
                    self.pitch,
                    &self.config.map,
                    &self.history,
                )
                .map_err(S::Error::from)?
                {
                    self.phase = EnginePhase::Physical(PhysicalPhase::Bed {
                        prev_idx,
                        curr_idx,
                        next_idx,
                        bed,
                    });
                } else {
                    self.phase = EnginePhase::Physical(PhysicalPhase::AttackGuard {
                        prev_idx,
                        curr_idx,
                        next_idx,
                        frame: start,
                        end,
                    });
                }
                Ok(())
            }
            PhysicalPhase::Bed {
                prev_idx,
                curr_idx,
                next_idx,
                mut bed,
            } => {
                let boundaries: Vec<Boundary> = self.physical_anchors[prev_idx..=curr_idx]
                    .iter()
                    .map(|a| Boundary {
                        source: a.source,
                        output: a.output,
                    })
                    .collect();
                let Some(bank) = self.bank.as_ref() else {
                    return Err(Error::Invalid("beats filter bank missing").into());
                };
                let (ready, finished) = step_bed(
                    &mut bed,
                    self.config.sample_rate,
                    self.config.mode,
                    self.pitch,
                    &self.config.map,
                    bank,
                    &boundaries,
                    &mut self.overlap,
                    &self.history,
                    source,
                    scratch,
                    cancel,
                )?;
                self.ready_end = ready;
                if finished {
                    let curr = self.physical_anchors[curr_idx];
                    if curr.attack {
                        let start = curr.output.saturating_sub(curr.before);
                        let end = (curr.output + curr.after).min(self.total);
                        self.phase = EnginePhase::Physical(PhysicalPhase::AttackGuard {
                            prev_idx,
                            curr_idx,
                            next_idx,
                            frame: start,
                            end,
                        });
                    } else {
                        self.phase = EnginePhase::Physical(PhysicalPhase::NeedNextAttack {
                            prev_idx: curr_idx,
                            curr_idx: next_idx,
                            output_cursor: curr.output,
                            source_cursor: curr.source,
                        });
                    }
                } else {
                    self.phase = EnginePhase::Physical(PhysicalPhase::Bed {
                        prev_idx,
                        curr_idx,
                        next_idx,
                        bed,
                    });
                }
                Ok(())
            }
            PhysicalPhase::AttackGuard {
                prev_idx,
                curr_idx,
                next_idx,
                mut frame,
                end,
            } => {
                let anchor = self.physical_anchors[curr_idx];
                let prev_src = self.physical_anchors[prev_idx].source;
                let next_src = self.physical_anchors[next_idx].source;
                let Some(bank) = self.bank.as_ref() else {
                    return Err(Error::Invalid("beats filter bank missing").into());
                };
                let chunk_end = (frame + FIRST_PASS_CHUNK_FRAMES).min(end);
                while frame < chunk_end {
                    if frame.is_multiple_of(256) && cancel() {
                        return Err(Error::Cancelled.into());
                    }
                    let position =
                        anchor.source + (frame as f64 - anchor.output as f64) * self.pitch;
                    let (lower, upper) = if frame < anchor.output {
                        (prev_src.floor() as i128, anchor.source.ceil() as i128 - 1)
                    } else {
                        (anchor.source.floor() as i128, next_src.ceil() as i128 - 1)
                    };
                    let sample =
                        bank.try_sample_segment(position, lower, upper.max(lower), |at| {
                            source.frame(at)
                        })?;
                    self.overlap
                        .add(i128::from(frame), sample, 1.0)
                        .map_err(S::Error::from)?;
                    frame += 1;
                }
                self.ready_end = frame;
                if frame >= end {
                    let next_source = anchor.source + anchor.after as f64 * self.pitch;
                    self.phase = EnginePhase::Physical(PhysicalPhase::NeedNextAttack {
                        prev_idx: curr_idx,
                        curr_idx: next_idx,
                        output_cursor: end,
                        source_cursor: next_source,
                    });
                } else {
                    self.phase = EnginePhase::Physical(PhysicalPhase::AttackGuard {
                        prev_idx,
                        curr_idx,
                        next_idx,
                        frame,
                        end,
                    });
                }
                Ok(())
            }
            PhysicalPhase::FinalBed { mut bed } => {
                let boundaries: Vec<Boundary> = self
                    .physical_anchors
                    .iter()
                    .map(|a| Boundary {
                        source: a.source,
                        output: a.output,
                    })
                    .collect();
                let Some(bank) = self.bank.as_ref() else {
                    return Err(Error::Invalid("beats filter bank missing").into());
                };
                let (ready, finished) = step_bed(
                    &mut bed,
                    self.config.sample_rate,
                    self.config.mode,
                    self.pitch,
                    &self.config.map,
                    bank,
                    &boundaries,
                    &mut self.overlap,
                    &self.history,
                    source,
                    scratch,
                    cancel,
                )?;
                self.ready_end = ready;
                self.phase = if finished {
                    EnginePhase::Physical(PhysicalPhase::Done)
                } else {
                    EnginePhase::Physical(PhysicalPhase::FinalBed { bed })
                };
                Ok(())
            }
            PhysicalPhase::Done => Ok(()),
        }
    }

    fn step_stop<S: GranularSource>(
        &mut self,
        phase: StopPhase,
        source: &mut S,
        scratch: &mut [[f64; 2]],
        cancel: &mut dyn FnMut() -> bool,
    ) -> Result<(), S::Error> {
        match phase {
            StopPhase::NeedSegment {
                mut seg_index,
                previous_gap,
                prev_seg_source,
            } => {
                // Make sure physical_anchors has seg_index, seg_index + 1, and enough
                // lookahead to know whether seg_index + 2 exists and the next attack after b.
                if !self.physical_stream.emitted_first {
                    let first_src = self.first_source_q32.div_euclid(Q32);
                    if !self.ensure_physical_through(first_src, source, scratch, cancel)? {
                        return Ok(());
                    }
                }
                let dropped = self.prune_physical_anchors(seg_index);
                seg_index -= dropped;
                while self.physical_anchors.len() <= seg_index + 2
                    && !self.physical_stream.emitted_last
                {
                    let frontier = if self.physical_stream.next_cycle > self.last_cycle {
                        self.end_source_q32.div_euclid(Q32)
                    } else {
                        self.physical_stream.next_cycle * i128::from(self.config.region_length)
                            + i128::from(self.physical_stream.next_onset_local)
                    };
                    let target = (frontier + 16_384).min(self.end_source_q32.div_euclid(Q32));
                    if !self.ensure_physical_through(target, source, scratch, cancel)? {
                        self.phase = EnginePhase::Stop(StopPhase::NeedSegment {
                            seg_index,
                            previous_gap,
                            prev_seg_source,
                        });
                        return Ok(());
                    }
                }
                if seg_index + 1 >= self.physical_anchors.len() {
                    self.ready_end = self.total;
                    self.phase = EnginePhase::Stop(StopPhase::Done);
                    return Ok(());
                }
                let a = Boundary {
                    source: self.physical_anchors[seg_index].source,
                    output: self.physical_anchors[seg_index].output,
                };
                let b = Boundary {
                    source: self.physical_anchors[seg_index + 1].source,
                    output: self.physical_anchors[seg_index + 1].output,
                };
                // Also ensure we have the next physical attack at or after `b` for bounds.
                let Some(next_attack_after_b) =
                    self.ensure_next_physical_attack(seg_index + 1, source, scratch, cancel)?
                else {
                    self.phase = EnginePhase::Stop(StopPhase::NeedSegment {
                        seg_index,
                        previous_gap,
                        prev_seg_source,
                    });
                    return Ok(());
                };
                let _ = next_attack_after_b;
                let has_more_segments = seg_index + 2 < self.physical_anchors.len()
                    || !self.physical_stream.emitted_last;
                let duration = b.output - a.output;
                let native_duration = (b.source - a.source) / self.pitch;
                let sounding = duration.min(native_duration.ceil() as u64);
                let changed = (duration as f64 - native_duration).abs() > 1e-6;
                let gap = sounding < duration;
                let onset = if previous_gap {
                    u64::from(self.config.sample_rate / 500).min(sounding)
                } else {
                    0
                };
                let Some(bank) = self.bank.as_ref() else {
                    return Err(Error::Invalid("beats filter bank missing").into());
                };
                let baseline = if onset >= 2 {
                    restart_baseline(
                        self.config.sample_rate,
                        bank,
                        prev_seg_source.floor() as i128,
                        a.source.ceil() as i128,
                        self.pitch,
                        source,
                    )?
                    .fitted
                } else {
                    [0.0; 2]
                };
                let join = if changed && self.decay == 100 && has_more_segments && !gap {
                    u64::from(self.config.sample_rate / 500).min(duration / 4)
                } else {
                    0
                };
                let release = if changed && join == 0 && (gap || !has_more_segments) {
                    u64::from(self.config.sample_rate / 1000).min(sounding / 4)
                } else {
                    0
                };
                // Initialize Bounds matching `stop::Bounds::new` advanced to `a.source`.
                let mut native_prev = 0;
                let last = self.physical_anchors.len() - 1;
                let mut native_next = (1..self.physical_anchors.len())
                    .find(|&i| self.physical_anchors[i].attack)
                    .unwrap_or(last);
                while native_next < last && a.source >= self.physical_anchors[native_next].source {
                    native_prev = native_next;
                    native_next = (native_next + 1..self.physical_anchors.len())
                        .find(|&i| self.physical_anchors[i].attack)
                        .unwrap_or(last);
                }
                let (join_prev, join_next) = (native_prev, native_next);
                self.phase = EnginePhase::Stop(StopPhase::Sounding {
                    seg_index,
                    a,
                    b,
                    duration,
                    sounding,
                    gap,
                    onset,
                    baseline,
                    join,
                    release,
                    age: 0,
                    native_prev,
                    native_next,
                    join_prev,
                    join_next,
                });
                Ok(())
            }
            StopPhase::Sounding {
                seg_index,
                a,
                b,
                duration,
                sounding,
                gap,
                onset,
                baseline,
                join,
                release,
                mut age,
                mut native_prev,
                mut native_next,
                mut join_prev,
                mut join_next,
            } => {
                let Some(bank) = self.bank.as_ref() else {
                    return Err(Error::Invalid("beats filter bank missing").into());
                };
                let last = self.physical_anchors.len() - 1;
                let chunk_end = (age + FIRST_PASS_CHUNK_FRAMES).min(sounding);
                while age < chunk_end {
                    if age.is_multiple_of(256) && cancel() {
                        return Err(Error::Cancelled.into());
                    }
                    let position = a.source + age as f64 * self.pitch;
                    if position < b.source {
                        while native_next < last
                            && position >= self.physical_anchors[native_next].source
                        {
                            native_prev = native_next;
                            native_next = (native_next + 1..self.physical_anchors.len())
                                .find(|&i| self.physical_anchors[i].attack)
                                .unwrap_or(last);
                        }
                        let lower = self.physical_anchors[native_prev].source.floor() as i128;
                        let upper = self.physical_anchors[native_next].source.ceil() as i128 - 1;
                        let mut sample =
                            bank.try_sample_segment(position, lower, upper.max(lower), |at| {
                                source.frame(at)
                            })?;
                        let restart = if onset >= 2 && age < onset {
                            1.0 - smooth_quintic(age as f64 / (onset - 1) as f64)
                        } else {
                            0.0
                        };
                        let remaining = sounding - age - 1;
                        let safety = if release > 0 && remaining < release {
                            let x = remaining as f64 / release as f64;
                            smooth_cubic(x)
                        } else {
                            1.0
                        };
                        for c in 0..2 {
                            sample[c] = (sample[c] - baseline[c] * restart) * safety;
                        }
                        let until_next = duration - age;
                        if join > 0 && until_next <= join {
                            let jpos = b.source - until_next as f64 * self.pitch;
                            while join_next < last
                                && jpos >= self.physical_anchors[join_next].source
                            {
                                join_prev = join_next;
                                join_next = (join_next + 1..self.physical_anchors.len())
                                    .find(|&i| self.physical_anchors[i].attack)
                                    .unwrap_or(last);
                            }
                            let jlower = self.physical_anchors[join_prev].source.floor() as i128;
                            let jupper = self.physical_anchors[join_next].source.ceil() as i128 - 1;
                            let incoming =
                                bank.try_sample_segment(jpos, jlower, jupper.max(jlower), |at| {
                                    source.frame(at)
                                })?;
                            let weight =
                                smooth_quintic((join - until_next + 1) as f64 / join as f64);
                            sample = std::array::from_fn(|c| {
                                sample[c] * (1.0 - weight) + incoming[c] * weight
                            });
                        }
                        self.overlap
                            .add(i128::from(a.output + age), sample, 1.0)
                            .map_err(S::Error::from)?;
                    } else {
                        self.overlap
                            .add(i128::from(a.output + age), [0.0; 2], 1.0)
                            .map_err(S::Error::from)?;
                    }
                    age += 1;
                }
                self.ready_end = a.output + age;
                if age >= sounding {
                    if sounding < duration {
                        self.phase = EnginePhase::Stop(StopPhase::Silence {
                            seg_index,
                            a,
                            b,
                            at: a.output + sounding,
                            gap,
                        });
                    } else {
                        self.phase = EnginePhase::Stop(StopPhase::NeedSegment {
                            seg_index: seg_index + 1,
                            previous_gap: gap,
                            prev_seg_source: a.source,
                        });
                    }
                } else {
                    self.phase = EnginePhase::Stop(StopPhase::Sounding {
                        seg_index,
                        a,
                        b,
                        duration,
                        sounding,
                        gap,
                        onset,
                        baseline,
                        join,
                        release,
                        age,
                        native_prev,
                        native_next,
                        join_prev,
                        join_next,
                    });
                }
                Ok(())
            }
            StopPhase::Silence {
                seg_index,
                a,
                b,
                mut at,
                gap,
            } => {
                let chunk_end = (at + COPY_CHUNK_FRAMES).min(b.output);
                while at < chunk_end {
                    self.overlap
                        .add(i128::from(at), [0.0; 2], 1.0)
                        .map_err(S::Error::from)?;
                    at += 1;
                }
                self.ready_end = at;
                if at >= b.output {
                    self.phase = EnginePhase::Stop(StopPhase::NeedSegment {
                        seg_index: seg_index + 1,
                        previous_gap: gap,
                        prev_seg_source: a.source,
                    });
                } else {
                    self.phase = EnginePhase::Stop(StopPhase::Silence {
                        seg_index,
                        a,
                        b,
                        at,
                        gap,
                    });
                }
                Ok(())
            }
            StopPhase::Done => Ok(()),
        }
    }

    fn step_chosen<S: GranularSource>(
        &mut self,
        phase: ChosenPhase,
        source: &mut S,
        scratch: &mut [[f64; 2]],
        cancel: &mut dyn FnMut() -> bool,
    ) -> Result<(), S::Error> {
        match phase {
            ChosenPhase::NeedGroup {
                mut group_index,
                previous_cut_attack,
                previous_aligned,
                previous_source_cut,
                previous_gap,
                bounds_prev,
                mut bounds_next,
                mut future_idx,
                mut last_attack,
            } => {
                // Prune old chosen boundaries strictly before group_index.
                if group_index > 8 {
                    let drop = group_index - 1;
                    self.chosen_boundaries.drain(..drop);
                    group_index -= drop;
                }
                self.ensure_chosen_boundary(group_index + 2)
                    .map_err(S::Error::from)?;
                if group_index + 1 >= self.chosen_boundaries.len() {
                    self.ready_end = self.total;
                    self.phase = EnginePhase::Chosen(ChosenPhase::Done);
                    return Ok(());
                }
                let a = self.chosen_boundaries[group_index];
                let b = self.chosen_boundaries[group_index + 1];
                let has_more_groups = group_index + 2 < self.chosen_boundaries.len()
                    || !self.chosen_stream.emitted_last;
                // Ensure physical anchors exist up through `b.source` AND the next physical
                // attack after `b.source` (within horizon).
                let target_src = b.source.ceil() as i128 + 1;
                if !self.ensure_physical_through(target_src, source, scratch, cancel)? {
                    self.phase = EnginePhase::Chosen(ChosenPhase::NeedGroup {
                        group_index,
                        previous_cut_attack,
                        previous_aligned,
                        previous_source_cut,
                        previous_gap,
                        bounds_prev,
                        bounds_next,
                        future_idx,
                        last_attack,
                    });
                    return Ok(());
                }
                let after_b_idx = self
                    .physical_anchors
                    .iter()
                    .position(|x| x.attack && x.source >= b.source);
                if after_b_idx.is_none() && !self.physical_stream.emitted_last {
                    let tail_idx = self.physical_anchors.len().saturating_sub(1);
                    if self
                        .ensure_next_physical_attack(tail_idx, source, scratch, cancel)?
                        .is_none()
                    {
                        self.phase = EnginePhase::Chosen(ChosenPhase::NeedGroup {
                            group_index,
                            previous_cut_attack,
                            previous_aligned,
                            previous_source_cut,
                            previous_gap,
                            bounds_prev,
                            bounds_next,
                            future_idx,
                            last_attack,
                        });
                        return Ok(());
                    }
                }
                let physical_end = self.end_source_q32 as f64 / Q32 as f64;
                let first_physical_src = self.first_source_q32 as f64 / Q32 as f64;
                let guard = f64::from(self.config.sample_rate) / 125.0;
                let duration = b.output - a.output;
                let sounding = (((b.source - a.source) / self.pitch).ceil() as u64).min(duration);
                let native_end = a.output + sounding;

                while future_idx < self.physical_anchors.len() {
                    if !self.physical_anchors[future_idx].attack {
                        future_idx += 1;
                        continue;
                    }
                    if self.physical_anchors[future_idx].source >= b.source {
                        break;
                    }
                    last_attack = Some(future_idx);
                    future_idx += 1;
                }
                let next_attack = self
                    .physical_anchors
                    .get(future_idx)
                    .filter(|x| x.attack)
                    .map(|x| x.source);
                let safe_start = if let Some(index) = last_attack {
                    let onset = self.physical_anchors[index].source;
                    let next = next_attack.unwrap_or(physical_end);
                    onset + guard.min((next - onset).max(0.0) / 3.0)
                } else {
                    first_physical_src
                };
                let safe_end = next_attack.map_or(physical_end, |onset| {
                    onset
                        - (f64::from(self.config.sample_rate) / 1000.0)
                            .min((onset - safe_start).max(0.0) / 3.0)
                });
                let sustain = self.tail != Tail::Off
                    && native_end < b.output
                    && b.source - safe_start >= 4.0
                    && safe_end - safe_start >= 4.0;
                let align_end = has_more_groups && b.source <= safe_end;
                let overlap_len = if sustain {
                    u64::from(self.config.sample_rate / 500)
                        .max(1)
                        .min(sounding / 4)
                } else {
                    0
                };
                let blend_start = native_end - overlap_len;
                let is_after_first = a.output > 0 || group_index > 0;
                let join = if is_after_first {
                    u64::from(self.config.sample_rate / if previous_gap { 500 } else { 200 })
                        .min(sounding)
                } else {
                    0
                };
                let Some(bank) = self.bank.as_ref() else {
                    return Err(Error::Invalid("beats filter bank missing").into());
                };
                let correction = if join >= 2 && !previous_aligned {
                    let prior = first_physical_src.floor() as i128;
                    let prior_onset = self
                        .physical_anchors
                        .iter()
                        .rev()
                        .find(|x| x.attack && x.source < a.source)
                        .filter(|x| previous_source_cut.is_some_and(|cut| x.source >= cut));
                    let baseline_end = prior_onset.map_or(a.source, |x| x.source);
                    let baseline = restart_baseline(
                        self.config.sample_rate,
                        bank,
                        prior,
                        baseline_end.ceil() as i128,
                        self.pitch,
                        source,
                    )?
                    .endpoint;
                    let outgoing = if let Some(onset) = previous_cut_attack {
                        restart_baseline(
                            self.config.sample_rate,
                            bank,
                            prior,
                            onset,
                            self.pitch,
                            source,
                        )?
                        .endpoint
                    } else {
                        self.history.output_background(self.config.sample_rate)
                    };
                    std::array::from_fn(|c| baseline[c] - outgoing[c])
                } else {
                    [0.0; 2]
                };
                let release = if self.tail == Tail::Off && native_end < b.output {
                    u64::from(self.config.sample_rate / 1000).min(sounding / 4)
                } else {
                    0
                };
                // Keep bounds_next in valid range.
                bounds_next = bounds_next.min(self.physical_anchors.len().saturating_sub(1));
                self.phase = EnginePhase::Chosen(ChosenPhase::FirstPass {
                    group_index,
                    a,
                    b,
                    duration,
                    sounding,
                    native_end,
                    sustain,
                    align_end,
                    overlap_len,
                    blend_start,
                    join,
                    correction,
                    release,
                    safe_start,
                    safe_end,
                    age: 0,
                    excluded_before: previous_source_cut,
                    bounds_prev,
                    bounds_next,
                    future_idx,
                    last_attack,
                });
                Ok(())
            }
            ChosenPhase::FirstPass {
                group_index,
                a,
                b,
                duration,
                sounding,
                native_end,
                sustain,
                align_end,
                overlap_len,
                blend_start,
                join,
                correction,
                release,
                safe_start,
                safe_end,
                mut age,
                excluded_before,
                mut bounds_prev,
                mut bounds_next,
                future_idx,
                last_attack,
            } => {
                let Some(bank) = self.bank.as_ref() else {
                    return Err(Error::Invalid("beats filter bank missing").into());
                };
                let guard = f64::from(self.config.sample_rate) / 125.0;
                let chunk_end = (age + FIRST_PASS_CHUNK_FRAMES).min(sounding);
                let last_phys = self.physical_anchors.len().saturating_sub(1);
                while age < chunk_end {
                    if age.is_multiple_of(256) && cancel() {
                        return Err(Error::Cancelled.into());
                    }
                    let at = a.output + age;
                    let position = a.source + age as f64 * self.pitch;
                    // Advance SourceBounds matching `groups::SourceBounds::advance`.
                    while bounds_next < last_phys
                        && (self.physical_anchors[bounds_next].source <= position
                            || self.physical_anchors[bounds_next].source < b.source
                                && ((self.physical_anchors[bounds_next].source - a.source)
                                    / self.pitch)
                                    .floor()
                                    <= age as f64)
                    {
                        if self.physical_anchors[bounds_next].attack {
                            bounds_prev = bounds_next;
                        }
                        bounds_next += 1;
                    }
                    while bounds_next < last_phys && !self.physical_anchors[bounds_next].attack {
                        bounds_next += 1;
                    }
                    let onset = self.physical_anchors[bounds_prev].source;
                    let mut lower = onset.floor() as i128;
                    if excluded_before.is_some_and(|cut| onset >= cut && onset < a.source) {
                        lower = lower.max(a.source.ceil() as i128);
                    }
                    let upper = self.physical_anchors[bounds_next].source.ceil() as i128 - 1;
                    let mut sample =
                        bank.try_sample_segment(position, lower, upper.max(lower), |p| {
                            source.frame(p)
                        })?;
                    let correction_gain = if join >= 2 && age < join {
                        1.0 - smooth_cubic(age as f64 / (join - 1) as f64)
                    } else {
                        0.0
                    };
                    let remaining = sounding - age - 1;
                    let prev_anchor = self.physical_anchors[bounds_prev];
                    let protected = prev_anchor.attack && position - prev_anchor.source < guard;
                    let gain = if release > 0 && remaining < release && !protected {
                        smooth_cubic(remaining as f64 / release as f64)
                    } else {
                        1.0
                    };
                    for c in 0..2 {
                        sample[c] = (sample[c] - correction[c] * correction_gain) * gain;
                    }
                    let weight = if overlap_len > 0 && at >= blend_start {
                        1.0 - smooth_cubic((at - blend_start) as f64 / overlap_len as f64)
                    } else {
                        1.0
                    };
                    self.overlap
                        .add(i128::from(at), sample, weight)
                        .map_err(S::Error::from)?;
                    self.ready_end = (at + 1).saturating_sub(overlap_len).max(a.output);
                    age += 1;
                }
                if age < sounding {
                    self.phase = EnginePhase::Chosen(ChosenPhase::FirstPass {
                        group_index,
                        a,
                        b,
                        duration,
                        sounding,
                        native_end,
                        sustain,
                        align_end,
                        overlap_len,
                        blend_start,
                        join,
                        correction,
                        release,
                        safe_start,
                        safe_end,
                        age,
                        excluded_before,
                        bounds_prev,
                        bounds_next,
                        future_idx,
                        last_attack,
                    });
                    return Ok(());
                }
                // First pass finished!
                let prev_anchor = self.physical_anchors[bounds_prev];
                let previous_cut_attack = if duration > 0
                    && native_end == b.output
                    && sounding > 0
                    && prev_anchor.attack
                    && (a.source + (sounding - 1) as f64 * self.pitch) - prev_anchor.source < guard
                {
                    Some(prev_anchor.source.ceil() as i128)
                } else {
                    None
                };
                let cut = a.source + duration as f64 * self.pitch;
                let previous_source_cut = if duration == 0 {
                    Some(excluded_before.map_or(cut, |prev| prev.min(cut)))
                } else {
                    (cut < b.source).then_some(cut)
                };
                if sustain {
                    self.ready_end = blend_start;
                    let span = BedSpan {
                        start: blend_start,
                        end: b.output,
                        source_start: safe_start,
                        source_end: safe_end,
                        locked_end: align_end,
                    };
                    let continuation = Some(BedContinuation {
                        source: a.source + (blend_start - a.output) as f64 * self.pitch,
                        target: b.source,
                        native_end,
                        fade: overlap_len as usize,
                    });
                    if let Some(bed) = init_bed(
                        span,
                        continuation,
                        self.config.sample_rate,
                        self.pitch,
                        &self.config.map,
                        &self.history,
                    )
                    .map_err(S::Error::from)?
                    {
                        self.phase = EnginePhase::Chosen(ChosenPhase::SustainBed {
                            group_index,
                            b,
                            align_end,
                            previous_cut_attack,
                            previous_source_cut,
                            bounds_prev,
                            bounds_next,
                            future_idx,
                            last_attack,
                            bed,
                        });
                    } else {
                        self.ready_end = b.output;
                        self.phase = EnginePhase::Chosen(ChosenPhase::NeedGroup {
                            group_index: group_index + 1,
                            previous_cut_attack,
                            previous_aligned: align_end,
                            previous_source_cut,
                            previous_gap: false,
                            bounds_prev,
                            bounds_next,
                            future_idx,
                            last_attack,
                        });
                    }
                } else {
                    let previous_aligned = native_end == b.output
                        && (a.source + duration as f64 * self.pitch - b.source).abs() < 1e-9;
                    let previous_gap = duration > 0 && native_end < b.output;
                    self.ready_end = native_end;
                    if native_end < b.output {
                        self.phase = EnginePhase::Chosen(ChosenPhase::Silence {
                            group_index,
                            b,
                            at: native_end,
                            previous_cut_attack,
                            previous_aligned,
                            previous_source_cut,
                            previous_gap,
                            bounds_prev,
                            bounds_next,
                            future_idx,
                            last_attack,
                        });
                    } else {
                        self.phase = EnginePhase::Chosen(ChosenPhase::NeedGroup {
                            group_index: group_index + 1,
                            previous_cut_attack,
                            previous_aligned,
                            previous_source_cut,
                            previous_gap,
                            bounds_prev,
                            bounds_next,
                            future_idx,
                            last_attack,
                        });
                    }
                }
                Ok(())
            }
            ChosenPhase::SustainBed {
                group_index,
                b,
                align_end,
                previous_cut_attack,
                previous_source_cut,
                bounds_prev,
                bounds_next,
                future_idx,
                last_attack,
                mut bed,
            } => {
                let Some(bank) = self.bank.as_ref() else {
                    return Err(Error::Invalid("beats filter bank missing").into());
                };
                let (ready, finished) = step_bed(
                    &mut bed,
                    self.config.sample_rate,
                    self.config.mode,
                    self.pitch,
                    &self.config.map,
                    bank,
                    &self.chosen_boundaries,
                    &mut self.overlap,
                    &self.history,
                    source,
                    scratch,
                    cancel,
                )?;
                self.ready_end = ready;
                if finished {
                    self.phase = EnginePhase::Chosen(ChosenPhase::NeedGroup {
                        group_index: group_index + 1,
                        previous_cut_attack,
                        previous_aligned: align_end,
                        previous_source_cut,
                        previous_gap: false,
                        bounds_prev,
                        bounds_next,
                        future_idx,
                        last_attack,
                    });
                } else {
                    self.phase = EnginePhase::Chosen(ChosenPhase::SustainBed {
                        group_index,
                        b,
                        align_end,
                        previous_cut_attack,
                        previous_source_cut,
                        bounds_prev,
                        bounds_next,
                        future_idx,
                        last_attack,
                        bed,
                    });
                }
                Ok(())
            }
            ChosenPhase::Silence {
                group_index,
                b,
                mut at,
                previous_cut_attack,
                previous_aligned,
                previous_source_cut,
                previous_gap,
                bounds_prev,
                bounds_next,
                future_idx,
                last_attack,
            } => {
                let chunk_end = (at + COPY_CHUNK_FRAMES).min(b.output);
                while at < chunk_end {
                    self.overlap
                        .add(i128::from(at), [0.0; 2], 1.0)
                        .map_err(S::Error::from)?;
                    at += 1;
                }
                self.ready_end = at;
                if at >= b.output {
                    self.phase = EnginePhase::Chosen(ChosenPhase::NeedGroup {
                        group_index: group_index + 1,
                        previous_cut_attack,
                        previous_aligned,
                        previous_source_cut,
                        previous_gap,
                        bounds_prev,
                        bounds_next,
                        future_idx,
                        last_attack,
                    });
                } else {
                    self.phase = EnginePhase::Chosen(ChosenPhase::Silence {
                        group_index,
                        b,
                        at,
                        previous_cut_attack,
                        previous_aligned,
                        previous_source_cut,
                        previous_gap,
                        bounds_prev,
                        bounds_next,
                        future_idx,
                        last_attack,
                    });
                }
                Ok(())
            }
            ChosenPhase::Done => Ok(()),
        }
    }
}

pub(super) fn prepare_vec_within<T: Copy>(
    src: &[T],
    existing_capacity: usize,
    remaining_bytes: usize,
) -> Result<(Option<Vec<T>>, usize), Error> {
    let elem = std::mem::size_of::<T>();
    let existing_bytes = existing_capacity
        .checked_mul(elem)
        .ok_or(Error::ScratchLimit)?;
    if existing_capacity >= src.len() && existing_bytes <= remaining_bytes {
        return Ok((None, existing_bytes));
    }
    let min_bytes = src.len().checked_mul(elem).ok_or(Error::ScratchLimit)?;
    if min_bytes > remaining_bytes {
        return Err(Error::ScratchLimit);
    }
    let mut fresh = Vec::new();
    fresh
        .try_reserve_exact(src.len())
        .map_err(|_| Error::Allocation)?;
    let actual_bytes = fresh
        .capacity()
        .checked_mul(elem)
        .ok_or(Error::ScratchLimit)?;
    if actual_bytes > remaining_bytes {
        return Err(Error::ScratchLimit);
    }
    fresh.extend_from_slice(src);
    Ok((Some(fresh), actual_bytes))
}

fn commit_vec_replacement<T: Copy>(src: &[T], dst: &mut Vec<T>, replacement: Option<Vec<T>>) {
    match replacement {
        Some(fresh) => *dst = fresh,
        None => {
            dst.clear();
            dst.extend_from_slice(src);
        }
    }
}

pub(super) fn copy_vec<T: Copy>(src: &[T], dst: &mut Vec<T>) -> Result<(), Error> {
    dst.clear();
    if dst.capacity() < src.len() {
        dst.try_reserve_exact(src.len())
            .map_err(|_| Error::Allocation)?;
    }
    dst.extend_from_slice(src);
    Ok(())
}
