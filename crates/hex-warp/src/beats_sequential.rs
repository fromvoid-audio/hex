//! Sequential control-side scheduling for bounded Beats transient-anchored WSOLA warp.
//!
//! Supports `Tail::Forward`, `Tail::Alternating`, `Tail::Off`, `decay: 0..=100`,
//! `division_frames`, explicit `attacks`, unity copy/decay, and automatic streaming
//! transient detection over arbitrary streamed `TimeMap` extents up to
//! `MAX_STREAMED_OUTPUT_FRAMES` within `MAX_SCRATCH_BYTES = 32 MiB`.
mod bed;
mod onsets;

mod chosen;
mod physical;
mod stop;
mod unity;
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
            EnginePhase::UnityCopy { cursor } => {
                self.step_unity_copy(cursor, source, scratch, cancel)
            }
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
            let drop_count = self
                .env_boundaries
                .iter()
                .skip(1)
                .take(self.env_boundaries.len().saturating_sub(2))
                .take_while(|b| b.output <= cursor)
                .count();
            if drop_count > 0 {
                self.env_boundaries.drain(..drop_count);
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
