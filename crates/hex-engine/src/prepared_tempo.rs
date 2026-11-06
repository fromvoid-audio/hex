//! Immutable, graph-owned access to the canonical project tempo conversion.
//!
//! Preparation and map construction belong on a control thread. Conversion
//! methods borrow this value, do not clone shared ownership, and delegate all
//! rounding and inverse boundary selection to `hex-project::TempoMap`. A future
//! callback may borrow a map only at bounded event/block boundaries. The graph
//! containing this owned map must be retired away from the callback so its
//! backing point and prefix vectors are never freed there.
use hex_project::{BeatTime, LoopRegion, MAX_TEMPO_POINTS, TempoMap};
mod borrowed;
pub(crate) use borrowed::CanonicalMidiTiming;
mod breaks;
pub(crate) use breaks::{MidiTempoBreaks, TempoBreaks};
mod persistent;

/// Maximum canonical tempo segments admitted into a prepared graph.
pub const MAX_PREPARED_TEMPO_POINTS: usize = 1_024;

/// A checked preparation or conversion failure with no owned error text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreparedTempoError {
    SampleRateMismatch { expected: u32, actual: u32 },
    PointCountOutOfRange { points: usize },
    BeatToFrame(&'static str),
    FrameToBeat(&'static str),
    BeatLowerBound(&'static str),
    InvalidLoop(&'static str),
}

impl std::fmt::Display for PreparedTempoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SampleRateMismatch { expected, actual } => {
                write!(
                    f,
                    "tempo map rate {actual} hz does not match expected {expected} hz"
                )
            }
            Self::PointCountOutOfRange { points } => {
                write!(
                    f,
                    "prepared tempo map has {points} points; expected 1..=1024"
                )
            }
            Self::BeatToFrame(message)
            | Self::FrameToBeat(message)
            | Self::BeatLowerBound(message) => f.write_str(message),
            Self::InvalidLoop(message) => f.write_str(message),
        }
    }
}

/// Small event-boundary timing contract shared by canonical and periodic maps.
/// Beat conversion is for block/event boundaries. The optional local-tempo
/// query also serves native effects at sample offsets through a bounded map lookup.
pub trait MidiTiming {
    /// Optional exact local tempo, excluding any constant beat-origin shift.
    fn tempo_micro_bpm_at_frame(&self, _frame: u64) -> Option<u32> {
        None
    }
    fn sample_rate(&self) -> u32;
    fn max_micro_bpm(&self) -> u64;
    fn beat_to_frame(&self, beat: BeatTime) -> Result<u64, PreparedTempoError>;
    fn frame_to_beat(&self, frame: u64) -> Result<BeatTime, PreparedTempoError>;
    fn beat_lower_bound_for_frame(&self, frame: u64) -> Result<u128, PreparedTempoError>;
}

impl std::error::Error for PreparedTempoError {}

/// Owned canonical tempo data for immutable playback-graph use.
///
/// This type intentionally does not implement `Clone` or `Arc` conversion.
/// Prepare it on the control side, move it into the graph, then call conversion
/// methods through a shared borrow. All queries use the canonical `TempoMap`
/// integer kernel, including nearest-frame ties-later, the latest
/// quantized-boundary inverse rule, and the first-beat lower bound. Lookup is
/// bounded by the 1,024-point map cap and uses the canonical binary search.
#[derive(Debug)]
pub struct PreparedTempoMap {
    canonical: TempoMap,
    max_micro_bpm: u32,
}

impl PreparedTempoMap {
    pub(crate) fn observation_copy(&self) -> Result<Self, crate::Invalid> {
        let mut copy = hex_project::TempoMapCopyCustody::new();
        copy.prepare_from(&self.canonical)
            .map_err(|_| crate::Invalid("could not copy activity timing"))?;
        let mut map = None;
        copy.transfer_into(&mut map)
            .map_err(|_| crate::Invalid("could not retain activity timing"))?;
        Ok(Self {
            canonical: map.ok_or(crate::Invalid("activity timing is unavailable"))?,
            max_micro_bpm: self.max_micro_bpm,
        })
    }
    pub(crate) fn observation_heap_bytes(&self) -> Option<usize> {
        self.canonical
            .owned_allocation_layout()
            .checked_backing_bytes()
    }
    pub(crate) fn observation_copy_quote(&self) -> Option<usize> {
        self.canonical.points().len().checked_mul(
            std::mem::size_of::<hex_project::TempoPoint>() + std::mem::size_of::<u128>(),
        )
    }
    /// Move a validated canonical map into playback-owned prepared state.
    pub fn prepare(
        canonical: TempoMap,
        expected_sample_rate: u32,
    ) -> Result<Self, PreparedTempoError> {
        let max_micro_bpm = Self::validate_canonical(&canonical, expected_sample_rate)?;
        Ok(Self {
            canonical,
            max_micro_bpm,
        })
    }

    // Shared borrowed preflight for the new persistent canonical-copy stage.
    // No consumed owner can be dropped by a validation error on this path.
    pub(crate) fn validate_canonical(
        canonical: &TempoMap,
        expected_sample_rate: u32,
    ) -> Result<u32, PreparedTempoError> {
        let actual_sample_rate = canonical.sample_rate();
        if expected_sample_rate != actual_sample_rate {
            return Err(PreparedTempoError::SampleRateMismatch {
                expected: expected_sample_rate,
                actual: actual_sample_rate,
            });
        }
        let points = canonical.points().len();
        if !(1..=MAX_PREPARED_TEMPO_POINTS).contains(&points) || points > MAX_TEMPO_POINTS {
            return Err(PreparedTempoError::PointCountOutOfRange { points });
        }
        let max_micro_bpm = canonical
            .points()
            .iter()
            .map(|point| point.tempo().micro_bpm())
            .max()
            .ok_or(PreparedTempoError::PointCountOutOfRange { points: 0 })?;
        Ok(max_micro_bpm)
    }

    pub fn sample_rate(&self) -> u32 {
        self.canonical.sample_rate()
    }

    pub fn point_count(&self) -> usize {
        self.canonical.points().len()
    }

    /// Fastest tempo admitted by the prepared map, in integer micro-BPM.
    pub fn max_micro_bpm(&self) -> u32 {
        self.max_micro_bpm
    }

    /// Whether this prepared map represents the exact canonical map value.
    pub fn matches_canonical(&self, canonical: &TempoMap) -> bool {
        self.canonical == *canonical
    }

    pub(crate) fn same_map(&self, other: &Self) -> bool {
        self.canonical.proves_same_clock(&other.canonical)
    }

    /// Convert a Q32.32 quarter-note beat to its canonical nearest frame.
    /// Exact half-frame ties round later, matching `TempoMap::beat_to_frame`.
    pub fn beat_to_frame(&self, beat: BeatTime) -> Result<u64, PreparedTempoError> {
        self.canonical
            .beat_to_frame(beat)
            .map_err(|error| PreparedTempoError::BeatToFrame(error.0))
    }

    /// Convert a frame to the canonical nearest Q32.32 beat.
    ///
    /// If multiple tempo boundaries quantize to this frame, the latest boundary
    /// wins, matching `TempoMap::frame_to_beat`.
    pub fn frame_to_beat(&self, frame: u64) -> Result<BeatTime, PreparedTempoError> {
        self.canonical
            .frame_to_beat(frame)
            .map_err(|error| PreparedTempoError::FrameToBeat(error.0))
    }

    /// Find the first raw Q32.32 beat whose canonical forward conversion is at
    /// least `frame`. Returns `u64::MAX + 1` when no representable beat reaches
    /// the requested frame. This is distinct from the latest-boundary inverse.
    pub fn beat_lower_bound_for_frame(&self, frame: u64) -> Result<u128, PreparedTempoError> {
        self.canonical
            .beat_lower_bound_for_frame(frame)
            .map_err(|error| PreparedTempoError::BeatLowerBound(error.0))
    }
}

impl MidiTiming for PreparedTempoMap {
    fn tempo_micro_bpm_at_frame(&self, frame: u64) -> Option<u32> {
        SessionBaseTimingView::Canonical(self)
            .micro_bpm_at_frame(frame)
            .ok()
    }
    fn sample_rate(&self) -> u32 {
        self.sample_rate()
    }
    fn max_micro_bpm(&self) -> u64 {
        u64::from(self.max_micro_bpm())
    }
    fn beat_to_frame(&self, beat: BeatTime) -> Result<u64, PreparedTempoError> {
        self.beat_to_frame(beat)
    }
    fn frame_to_beat(&self, frame: u64) -> Result<BeatTime, PreparedTempoError> {
        self.frame_to_beat(frame)
    }
    fn beat_lower_bound_for_frame(&self, frame: u64) -> Result<u128, PreparedTempoError> {
        self.beat_lower_bound_for_frame(frame)
    }
}

/// Control-prepared cycle metadata for Session time over an enabled
/// Arrangement loop. It owns only scalar boundaries; the canonical map remains
/// owned by the immutable prepared revision and is borrowed through `view`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PreparedPeriodicSessionTempo {
    start_frame: u64,
    end_frame: u64,
    start_beat: BeatTime,
    end_beat: BeatTime,
    frame_period: u64,
    beat_period: u64,
    admitted_max_micro_bpm: u64,
}

impl PreparedPeriodicSessionTempo {
    pub(crate) fn prepare(
        map: &PreparedTempoMap,
        loop_region: LoopRegion,
    ) -> Result<Self, PreparedTempoError> {
        if !loop_region.enabled() || loop_region.end() <= loop_region.start() {
            return Err(PreparedTempoError::InvalidLoop(
                "session tempo loop must have positive frame length",
            ));
        }
        let start_frame = loop_region.start();
        let end_frame = loop_region.end();
        let start_beat = map.frame_to_beat(start_frame)?;
        let end_beat = map.frame_to_beat(end_frame)?;
        let frame_period =
            end_frame
                .checked_sub(start_frame)
                .ok_or(PreparedTempoError::InvalidLoop(
                    "session tempo loop frame range is invalid",
                ))?;
        let beat_period = end_beat
            .raw()
            .checked_sub(start_beat.raw())
            .filter(|period| *period > 0)
            .ok_or(PreparedTempoError::InvalidLoop(
                "session tempo loop beat period must be positive",
            ))?;
        if map.beat_to_frame(start_beat)? != start_frame
            || map.beat_to_frame(end_beat)? != end_frame
        {
            return Err(PreparedTempoError::InvalidLoop(
                "session tempo loop endpoints do not round-trip",
            ));
        }
        let factor = u128::from(map.sample_rate()) * 60_000_000;
        let numerator =
            u128::from(beat_period)
                .checked_mul(factor)
                .ok_or(PreparedTempoError::InvalidLoop(
                    "session tempo loop rate overflow",
                ))?;
        let denominator = u128::from(frame_period) << 32;
        let rate = numerator / denominator + u128::from(numerator % denominator != 0);
        // The same published source can also play a canonical first pass from
        // a cursor beyond the loop. Admit both reachable clocks on control;
        // this upper bound does not change the periodic mapping itself.
        let admitted_max_micro_bpm = u64::try_from(rate.max(u128::from(map.max_micro_bpm())))
            .map_err(|_| PreparedTempoError::InvalidLoop("session tempo loop rate overflow"))?;
        Ok(Self {
            start_frame,
            end_frame,
            start_beat,
            end_beat,
            frame_period,
            beat_period,
            admitted_max_micro_bpm,
        })
    }

    pub(crate) fn view<'a>(self, canonical: &'a PreparedTempoMap) -> PeriodicSessionTempoView<'a> {
        PeriodicSessionTempoView {
            canonical,
            cycle: self,
        }
    }

    pub(crate) const fn beat_range_raw(self) -> (u64, u64) {
        (self.start_beat.raw(), self.end_beat.raw())
    }

    #[cfg(test)]
    pub fn start_beat(self) -> BeatTime {
        self.start_beat
    }
    #[cfg(test)]
    pub fn end_beat(self) -> BeatTime {
        self.end_beat
    }
    #[cfg(test)]
    pub fn frame_period(self) -> u64 {
        self.frame_period
    }
    #[cfg(test)]
    pub fn beat_period(self) -> u64 {
        self.beat_period
    }
}

/// Borrowed conversion view: no self-reference, allocation, or Arc operation.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PeriodicSessionTempoView<'a> {
    canonical: &'a PreparedTempoMap,
    cycle: PreparedPeriodicSessionTempo,
}

/// Beat continuity at a prepared-revision cut. The effective Session clock is
/// the new base clock translated so both revisions identify the cut as the
/// same musical position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionClockAnchor {
    /// Performance frame at which this prepared-revision translation was established.
    pub cut_frame: u64,
    /// Global performance beat preserved from the preceding clock.
    pub old_beat: BeatTime,
    /// Same frame expressed in the current canonical/periodic base clock.
    pub new_beat: BeatTime,
}

impl SessionClockAnchor {
    /// Decode the existing scalar clock translation at a telemetry boundary.
    /// Every u64 beat value is a representable Q32.32 coordinate.
    pub const fn from_raw(cut_frame: u64, old_beat_raw: u64, new_beat_raw: u64) -> Self {
        Self {
            cut_frame,
            old_beat: BeatTime::from_raw(old_beat_raw),
            new_beat: BeatTime::from_raw(new_beat_raw),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct AnchoredSessionTempoView<'a> {
    base: SessionBaseTimingView<'a>,
    anchor: SessionClockAnchor,
}

impl AnchoredSessionTempoView<'_> {
    pub(crate) fn new<'a>(
        base: SessionBaseTimingView<'a>,
        anchor: SessionClockAnchor,
    ) -> AnchoredSessionTempoView<'a> {
        AnchoredSessionTempoView { base, anchor }
    }

    /// Translate a base-clock raw beat into the anchored performance clock.
    /// The signed result may precede beat zero; callers decide whether that is
    /// an error (a point conversion) or saturates (a lower bound).
    fn shift_raw(&self, raw: u128) -> Result<i128, PreparedTempoError> {
        let shift = i128::from(self.anchor.old_beat.raw()) - i128::from(self.anchor.new_beat.raw());
        let raw = i128::try_from(raw)
            .map_err(|_| PreparedTempoError::FrameToBeat("anchored beat conversion overflow"))?;
        raw.checked_add(shift)
            .ok_or(PreparedTempoError::FrameToBeat(
                "anchored beat conversion overflow",
            ))
    }
}

impl MidiTiming for AnchoredSessionTempoView<'_> {
    fn tempo_micro_bpm_at_frame(&self, frame: u64) -> Option<u32> {
        self.base.micro_bpm_at_frame(frame).ok()
    }
    fn sample_rate(&self) -> u32 {
        self.base.sample_rate()
    }
    fn max_micro_bpm(&self) -> u64 {
        self.base.max_micro_bpm()
    }
    fn beat_to_frame(&self, beat: BeatTime) -> Result<u64, PreparedTempoError> {
        let raw = i128::from(self.anchor.new_beat.raw()) + i128::from(beat.raw())
            - i128::from(self.anchor.old_beat.raw());
        let raw = u64::try_from(raw).map_err(|_| {
            PreparedTempoError::BeatToFrame("anchored beat exceeds the beat domain")
        })?;
        self.base.beat_to_frame(BeatTime::from_raw(raw))
    }
    fn frame_to_beat(&self, frame: u64) -> Result<BeatTime, PreparedTempoError> {
        let raw = self.base.frame_to_beat(frame)?.raw();
        // A frame whose anchored beat precedes zero has no beat; refuse it,
        // as `beat_to_frame` refuses the inverse, instead of reporting beat 0.
        let raw = self.shift_raw(u128::from(raw))?;
        if raw < 0 {
            return Err(PreparedTempoError::FrameToBeat(
                "anchored frame precedes the beat domain",
            ));
        }
        let raw = u64::try_from(raw).map_err(|_| {
            PreparedTempoError::FrameToBeat("anchored beat exceeds the beat domain")
        })?;
        Ok(BeatTime::from_raw(raw))
    }
    fn beat_lower_bound_for_frame(&self, frame: u64) -> Result<u128, PreparedTempoError> {
        // The anchored clock is the base clock translated, so it stays
        // monotonic. A negative translated bound means every beat in the
        // domain already reaches `frame`; beat zero is then the exact first
        // such beat, not an approximation.
        let raw = self.shift_raw(self.base.beat_lower_bound_for_frame(frame)?)?;
        Ok(u128::try_from(raw)
            .unwrap_or(0)
            .min(TempoMap::BEAT_RANGE_SENTINEL))
    }
}

impl PeriodicSessionTempoView<'_> {
    fn lift_raw(&self, cycles: u64, local_raw: u64) -> Result<BeatTime, PreparedTempoError> {
        let local_delta = local_raw.checked_sub(self.cycle.start_beat.raw()).ok_or(
            PreparedTempoError::FrameToBeat("periodic local beat precedes its loop"),
        )?;
        let offset = u128::from(cycles)
            .checked_mul(u128::from(self.cycle.beat_period))
            .and_then(|value| value.checked_add(u128::from(local_delta)))
            .ok_or(PreparedTempoError::FrameToBeat(
                "periodic session beat overflow",
            ))?;
        let raw = u128::from(self.cycle.start_beat.raw())
            .checked_add(offset)
            .ok_or(PreparedTempoError::FrameToBeat(
                "periodic session beat overflow",
            ))?;
        let raw = u64::try_from(raw).map_err(|_| {
            PreparedTempoError::FrameToBeat("periodic session beat exceeds the beat domain")
        })?;
        Ok(BeatTime::from_raw(raw))
    }

    fn periodic_frame(&self, beat: BeatTime) -> Result<u64, PreparedTempoError> {
        if beat < self.cycle.start_beat {
            return self.canonical.beat_to_frame(beat);
        }
        let delta = beat.raw().checked_sub(self.cycle.start_beat.raw()).ok_or(
            PreparedTempoError::BeatToFrame("periodic session beat precedes its loop"),
        )?;
        let cycles = delta / self.cycle.beat_period;
        let local_raw = self.cycle.start_beat.raw() + delta % self.cycle.beat_period;
        let local_frame = self
            .canonical
            .beat_to_frame(BeatTime::from_raw(local_raw))?;
        let local_offset = local_frame.checked_sub(self.cycle.start_frame).ok_or(
            PreparedTempoError::BeatToFrame("periodic session beat maps before its loop"),
        )?;
        let offset = cycles
            .checked_mul(self.cycle.frame_period)
            .and_then(|value| value.checked_add(local_offset))
            .ok_or(PreparedTempoError::BeatToFrame(
                "periodic session frame overflow",
            ))?;
        self.cycle
            .start_frame
            .checked_add(offset)
            .ok_or(PreparedTempoError::BeatToFrame(
                "periodic session frame overflow",
            ))
    }

    fn scoped_local_inverse(&self, frame: u64) -> Result<BeatTime, PreparedTempoError> {
        let candidate = self.canonical.frame_to_beat(frame);
        if let Ok(beat) = candidate
            && beat >= self.cycle.start_beat
            && beat < self.cycle.end_beat
        {
            return Ok(beat);
        }
        let lower = self.canonical.beat_lower_bound_for_frame(frame)?;
        let upper_frame = frame.checked_add(1).ok_or(PreparedTempoError::FrameToBeat(
            "periodic local frame overflow",
        ))?;
        let upper = self.canonical.beat_lower_bound_for_frame(upper_frame)?;
        let raw = lower.max(u128::from(self.cycle.start_beat.raw()));
        let raw = raw
            .min(upper.saturating_sub(1))
            .min(u128::from(self.cycle.end_beat.raw() - 1));
        let raw = u64::try_from(raw).map_err(|_| {
            PreparedTempoError::FrameToBeat("periodic local beat exceeds the beat domain")
        })?;
        let beat = BeatTime::from_raw(raw);
        if self.canonical.beat_to_frame(beat)? != frame {
            return Err(PreparedTempoError::FrameToBeat(
                "frame has no local beat in the periodic tempo segment",
            ));
        }
        Ok(beat)
    }
}

impl MidiTiming for PeriodicSessionTempoView<'_> {
    fn tempo_micro_bpm_at_frame(&self, frame: u64) -> Option<u32> {
        SessionBaseTimingView::Periodic(*self)
            .micro_bpm_at_frame(frame)
            .ok()
    }
    fn sample_rate(&self) -> u32 {
        self.canonical.sample_rate()
    }
    fn max_micro_bpm(&self) -> u64 {
        self.cycle.admitted_max_micro_bpm
    }
    fn beat_to_frame(&self, beat: BeatTime) -> Result<u64, PreparedTempoError> {
        self.periodic_frame(beat)
    }

    fn frame_to_beat(&self, frame: u64) -> Result<BeatTime, PreparedTempoError> {
        if frame < self.cycle.start_frame {
            return self.canonical.frame_to_beat(frame);
        }
        let elapsed = frame - self.cycle.start_frame;
        let cycles = elapsed / self.cycle.frame_period;
        let local_offset = elapsed % self.cycle.frame_period;
        let beat = if local_offset == 0 {
            self.cycle.start_beat
        } else {
            let local_frame = self.cycle.start_frame.checked_add(local_offset).ok_or(
                PreparedTempoError::FrameToBeat("periodic local frame overflow"),
            )?;
            self.scoped_local_inverse(local_frame)?
        };
        self.lift_raw(cycles, beat.raw())
    }

    fn beat_lower_bound_for_frame(&self, frame: u64) -> Result<u128, PreparedTempoError> {
        if frame <= self.cycle.start_frame {
            return self.canonical.beat_lower_bound_for_frame(frame);
        }
        let elapsed = frame - self.cycle.start_frame;
        let cycles = elapsed / self.cycle.frame_period;
        let local_offset = elapsed % self.cycle.frame_period;
        if local_offset == 0 {
            // Beats at the end of the preceding cycle may already round to
            // this seam. The canonical lower bound at E captures that tail.
            if cycles > 0 {
                let tail = self
                    .canonical
                    .beat_lower_bound_for_frame(self.cycle.end_frame)?;
                let tail = tail.max(u128::from(self.cycle.start_beat.raw()));
                if tail < u128::from(self.cycle.end_beat.raw()) {
                    let raw = u128::from(cycles - 1)
                        .checked_mul(u128::from(self.cycle.beat_period))
                        .and_then(|value| value.checked_add(tail))
                        .ok_or(PreparedTempoError::BeatLowerBound(
                            "periodic seam lower bound overflow",
                        ))?;
                    return Ok(raw.min(TempoMap::BEAT_RANGE_SENTINEL));
                }
            }
            let raw = u128::from(self.cycle.start_beat.raw())
                .checked_add(
                    u128::from(cycles)
                        .checked_mul(u128::from(self.cycle.beat_period))
                        .ok_or(PreparedTempoError::BeatLowerBound(
                            "periodic lower bound overflow",
                        ))?,
                )
                .ok_or(PreparedTempoError::BeatLowerBound(
                    "periodic lower bound overflow",
                ))?;
            return Ok(raw.min(TempoMap::BEAT_RANGE_SENTINEL));
        }
        let local_frame = self.cycle.start_frame.checked_add(local_offset).ok_or(
            PreparedTempoError::BeatLowerBound("periodic local frame overflow"),
        )?;
        let local = self
            .canonical
            .beat_lower_bound_for_frame(local_frame)?
            .max(u128::from(self.cycle.start_beat.raw()));
        if local >= u128::from(self.cycle.end_beat.raw()) {
            return Err(PreparedTempoError::BeatLowerBound(
                "periodic frame is not reached inside the loop cycle",
            ));
        }
        let raw = u128::from(self.cycle.start_beat.raw())
            .checked_add(
                u128::from(cycles)
                    .checked_mul(u128::from(self.cycle.beat_period))
                    .and_then(|value| {
                        value.checked_add(local - u128::from(self.cycle.start_beat.raw()))
                    })
                    .ok_or(PreparedTempoError::BeatLowerBound(
                        "periodic lower bound overflow",
                    ))?,
            )
            .ok_or(PreparedTempoError::BeatLowerBound(
                "periodic lower bound overflow",
            ))?;
        Ok(raw.min(TempoMap::BEAT_RANGE_SENTINEL))
    }
}

/// Borrowed timing choice used by the Session kernel. It keeps conversion
/// dispatch at block/event boundaries and never owns the canonical map.
#[derive(Clone, Copy, Debug)]
pub(crate) enum SessionBaseTimingView<'a> {
    Canonical(&'a PreparedTempoMap),
    Periodic(PeriodicSessionTempoView<'a>),
}

impl MidiTiming for SessionBaseTimingView<'_> {
    fn tempo_micro_bpm_at_frame(&self, frame: u64) -> Option<u32> {
        self.micro_bpm_at_frame(frame).ok()
    }
    fn sample_rate(&self) -> u32 {
        match self {
            Self::Canonical(map) => map.sample_rate(),
            Self::Periodic(map) => map.sample_rate(),
        }
    }
    fn max_micro_bpm(&self) -> u64 {
        match self {
            Self::Canonical(map) => u64::from(map.max_micro_bpm()),
            Self::Periodic(map) => map.max_micro_bpm(),
        }
    }
    fn beat_to_frame(&self, beat: BeatTime) -> Result<u64, PreparedTempoError> {
        match self {
            Self::Canonical(map) => map.beat_to_frame(beat),
            Self::Periodic(map) => map.beat_to_frame(beat),
        }
    }
    fn frame_to_beat(&self, frame: u64) -> Result<BeatTime, PreparedTempoError> {
        match self {
            Self::Canonical(map) => map.frame_to_beat(frame),
            Self::Periodic(map) => map.frame_to_beat(frame),
        }
    }
    fn beat_lower_bound_for_frame(&self, frame: u64) -> Result<u128, PreparedTempoError> {
        match self {
            Self::Canonical(map) => map.beat_lower_bound_for_frame(frame),
            Self::Periodic(map) => map.beat_lower_bound_for_frame(frame),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum SessionTimingView<'a> {
    Canonical(&'a PreparedTempoMap),
    Periodic(PeriodicSessionTempoView<'a>),
    Anchored(AnchoredSessionTempoView<'a>),
}

impl SessionBaseTimingView<'_> {
    fn micro_bpm_at_frame(self, frame: u64) -> Result<u32, PreparedTempoError> {
        let (map, frame) = match self {
            Self::Canonical(map) => (map, frame),
            Self::Periodic(view) => {
                let frame = if frame < view.cycle.start_frame {
                    frame
                } else {
                    view.cycle.start_frame
                        + (frame - view.cycle.start_frame) % view.cycle.frame_period
                };
                (view.canonical, frame)
            }
        };
        Ok(map
            .canonical
            .tempo_at(map.frame_to_beat(frame)?)
            .micro_bpm())
    }
}

impl<'a> SessionTimingView<'a> {
    /// Effective local tempo, excluding the anchor's constant beat offset.
    pub(crate) fn micro_bpm_at_frame(self, frame: u64) -> Result<u32, PreparedTempoError> {
        match self {
            Self::Canonical(map) => SessionBaseTimingView::Canonical(map).micro_bpm_at_frame(frame),
            Self::Periodic(view) => SessionBaseTimingView::Periodic(view).micro_bpm_at_frame(frame),
            Self::Anchored(view) => view.base.micro_bpm_at_frame(frame),
        }
    }

    pub(crate) fn from_base(base: SessionBaseTimingView<'a>) -> Self {
        match base {
            SessionBaseTimingView::Canonical(map) => Self::Canonical(map),
            SessionBaseTimingView::Periodic(map) => Self::Periodic(map),
        }
    }
}

impl MidiTiming for SessionTimingView<'_> {
    fn tempo_micro_bpm_at_frame(&self, frame: u64) -> Option<u32> {
        self.micro_bpm_at_frame(frame).ok()
    }
    fn sample_rate(&self) -> u32 {
        match self {
            Self::Canonical(map) => map.sample_rate(),
            Self::Periodic(map) => map.sample_rate(),
            Self::Anchored(map) => map.sample_rate(),
        }
    }
    fn max_micro_bpm(&self) -> u64 {
        match self {
            Self::Canonical(map) => u64::from(map.max_micro_bpm()),
            Self::Periodic(map) => map.max_micro_bpm(),
            Self::Anchored(map) => map.max_micro_bpm(),
        }
    }
    fn beat_to_frame(&self, beat: BeatTime) -> Result<u64, PreparedTempoError> {
        match self {
            Self::Canonical(map) => map.beat_to_frame(beat),
            Self::Periodic(map) => map.beat_to_frame(beat),
            Self::Anchored(map) => map.beat_to_frame(beat),
        }
    }
    fn frame_to_beat(&self, frame: u64) -> Result<BeatTime, PreparedTempoError> {
        match self {
            Self::Canonical(map) => map.frame_to_beat(frame),
            Self::Periodic(map) => map.frame_to_beat(frame),
            Self::Anchored(map) => map.frame_to_beat(frame),
        }
    }
    fn beat_lower_bound_for_frame(&self, frame: u64) -> Result<u128, PreparedTempoError> {
        match self {
            Self::Canonical(map) => map.beat_lower_bound_for_frame(frame),
            Self::Periodic(map) => map.beat_lower_bound_for_frame(frame),
            Self::Anchored(map) => map.beat_lower_bound_for_frame(frame),
        }
    }
}

impl PreparedTempoMap {
    pub(crate) fn visit_audio_storage<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), crate::AudioStorageError> {
        let Self {
            canonical,
            max_micro_bpm: _,
        } = self;
        ledger.map(canonical)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_PREPARED_TEMPO_POINTS, MidiTiming, PreparedPeriodicSessionTempo, PreparedTempoError,
        PreparedTempoMap,
    };
    use hex_project::{BeatTime, LoopRegion, Tempo, TempoMap, TempoPoint};

    fn beat(raw: u64) -> BeatTime {
        BeatTime::from_raw(raw)
    }

    fn tempo(micro_bpm: u32) -> Tempo {
        Tempo::new(micro_bpm).expect("test tempo is in the canonical range")
    }

    fn map(sample_rate: u32) -> TempoMap {
        TempoMap::new(
            sample_rate,
            vec![
                TempoPoint::new(BeatTime::ZERO, tempo(120_000_000)),
                TempoPoint::new(beat(BeatTime::ONE.raw() * 3), tempo(90_000_000)),
                TempoPoint::new(beat(BeatTime::ONE.raw() * 9), tempo(137_000_000)),
            ],
        )
        .expect("test map is valid")
    }

    #[test]
    fn constant_tempo_matches_independent_frame_arithmetic() {
        let canonical = TempoMap::constant(48_000, tempo(120_000_000)).unwrap();
        let prepared = PreparedTempoMap::prepare(canonical, 48_000).unwrap();
        for quarters in [0, 1, 2, 7, 32] {
            let position = BeatTime::from_quarters(quarters).unwrap();
            assert_eq!(prepared.beat_to_frame(position).unwrap(), quarters * 24_000);
            assert_eq!(prepared.frame_to_beat(quarters * 24_000).unwrap(), position);
        }
    }

    #[test]
    fn quantized_shared_tempo_boundaries_choose_the_latest_boundary() {
        let canonical = TempoMap::new(
            8_000,
            vec![
                TempoPoint::new(beat(0), tempo(120_000_000)),
                TempoPoint::new(beat(1), tempo(60_000_000)),
                TempoPoint::new(beat(2), tempo(240_000_000)),
            ],
        )
        .unwrap();
        let prepared = PreparedTempoMap::prepare(canonical.clone(), 8_000).unwrap();
        assert_eq!(canonical.beat_to_frame(beat(1)).unwrap(), 0);
        assert_eq!(canonical.beat_to_frame(beat(2)).unwrap(), 0);
        assert_eq!(prepared.frame_to_beat(0).unwrap(), beat(2));
        assert_eq!(
            prepared.frame_to_beat(0).unwrap(),
            canonical.frame_to_beat(0).unwrap()
        );
    }

    #[test]
    fn exact_half_frame_ties_round_to_the_later_frame() {
        // At 8 kHz and 937.5 bpm this segment is exactly 512 frames per beat.
        // Raw beat 2^22 is 1/1024 beat, or exactly half a frame.
        let canonical = TempoMap::constant(8_000, tempo(937_500_000)).unwrap();
        let prepared = PreparedTempoMap::prepare(canonical.clone(), 8_000).unwrap();
        let half_frame = beat(1 << 22);
        assert_eq!(canonical.beat_to_frame(half_frame).unwrap(), 1);
        assert_eq!(prepared.beat_to_frame(half_frame).unwrap(), 1);
    }

    #[test]
    fn lower_bound_delegates_canonical_minimality_and_sentinel() {
        let canonical = TempoMap::new(
            8_000,
            vec![
                TempoPoint::new(beat(0), tempo(120_000_000)),
                TempoPoint::new(beat(1), tempo(60_000_000)),
                TempoPoint::new(beat(2), tempo(240_000_000)),
            ],
        )
        .unwrap();
        let prepared = PreparedTempoMap::prepare(canonical.clone(), 8_000).unwrap();
        for frame in [0, 1, 2, 100, u64::MAX] {
            let actual = prepared.beat_lower_bound_for_frame(frame).unwrap();
            assert_eq!(actual, canonical.beat_lower_bound_for_frame(frame).unwrap());
            if actual == TempoMap::BEAT_RANGE_SENTINEL {
                assert!(canonical.beat_to_frame(beat(u64::MAX)).unwrap() < frame);
            } else {
                assert!(canonical.beat_to_frame(beat(actual as u64)).unwrap() >= frame);
                if actual > 0 {
                    assert!(canonical.beat_to_frame(beat(actual as u64 - 1)).unwrap() < frame);
                }
            }
        }
        assert_eq!(prepared.beat_lower_bound_for_frame(0).unwrap(), 0);
    }

    #[test]
    fn rate_refusal_and_extremal_conversion_errors_are_typed() {
        assert!(matches!(
            PreparedTempoMap::prepare(
                TempoMap::constant(48_000, tempo(120_000_000)).unwrap(),
                44_100
            ),
            Err(PreparedTempoError::SampleRateMismatch {
                expected: 44_100,
                actual: 48_000,
            })
        ));
        let prepared = PreparedTempoMap::prepare(
            TempoMap::constant(8_000, tempo(1_000_000_000)).unwrap(),
            8_000,
        )
        .unwrap();
        assert!(matches!(
            prepared.frame_to_beat(u64::MAX),
            Err(PreparedTempoError::FrameToBeat(_))
        ));
    }

    #[test]
    fn prepared_conversions_match_canonical_maps_at_supported_rates() {
        for sample_rate in [8_000, 44_100, 48_000, 96_000, 192_000] {
            let canonical = map(sample_rate);
            let prepared = PreparedTempoMap::prepare(canonical.clone(), sample_rate).unwrap();
            assert_eq!(prepared.sample_rate(), sample_rate);
            for raw in [
                0,
                1,
                BeatTime::ONE.raw() / 3,
                BeatTime::ONE.raw(),
                BeatTime::ONE.raw() * 3,
                BeatTime::ONE.raw() * 7 + 91,
                BeatTime::ONE.raw() * 9,
                BeatTime::ONE.raw() * 20,
            ] {
                let position = beat(raw);
                assert_eq!(
                    prepared.beat_to_frame(position),
                    canonical
                        .beat_to_frame(position)
                        .map_err(|error| { PreparedTempoError::BeatToFrame(error.0) })
                );
            }
            for frame in [0, 1, 17, 399, 4_000, 24_000, 1_000_003, 5_000_000] {
                assert_eq!(
                    prepared.frame_to_beat(frame),
                    canonical
                        .frame_to_beat(frame)
                        .map_err(|error| { PreparedTempoError::FrameToBeat(error.0) })
                );
            }
        }
    }

    #[test]
    fn varied_query_order_matches_block_and_event_replay() {
        let canonical = map(48_000);
        let prepared = PreparedTempoMap::prepare(canonical.clone(), 48_000).unwrap();
        let events: Vec<_> = (0..512_u64)
            .map(|index| beat(BeatTime::ONE.raw() * index / 19 + (index % 7)))
            .collect();
        let event_answers: Vec<_> = events
            .iter()
            .map(|position| prepared.beat_to_frame(*position).unwrap())
            .collect();
        let mut reversed: Vec<_> = events.iter().copied().enumerate().collect();
        reversed.reverse();
        for (index, position) in reversed {
            assert_eq!(
                prepared.beat_to_frame(position).unwrap(),
                event_answers[index]
            );
        }

        let block_frames: Vec<_> = (0..512_u64).map(|index| index * 257).collect();
        let block_answers: Vec<_> = block_frames
            .iter()
            .map(|frame| prepared.frame_to_beat(*frame).unwrap())
            .collect();
        for index in (0..block_frames.len()).rev() {
            assert_eq!(
                prepared.frame_to_beat(block_frames[index]).unwrap(),
                block_answers[index]
            );
        }
        assert_eq!(
            event_answers,
            events
                .iter()
                .map(|position| canonical.beat_to_frame(*position).unwrap())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn maximum_point_count_is_accepted() {
        let points: Vec<_> = (0..MAX_PREPARED_TEMPO_POINTS)
            .map(|index| {
                TempoPoint::new(
                    beat(BeatTime::ONE.raw() * index as u64),
                    tempo(120_000_000 + (index % 17) as u32 * 1_000_000),
                )
            })
            .collect();
        let canonical = TempoMap::new(48_000, points).unwrap();
        let prepared = PreparedTempoMap::prepare(canonical, 48_000).unwrap();
        assert_eq!(prepared.point_count(), MAX_PREPARED_TEMPO_POINTS);
        for index in 0..MAX_PREPARED_TEMPO_POINTS as u64 {
            let position = beat(BeatTime::ONE.raw() * index);
            assert!(prepared.beat_to_frame(position).is_ok());
        }
    }

    fn lower_bound_oracle<T: super::MidiTiming>(map: &T, frame: u64) -> u128 {
        let sentinel = u64::MAX as u128 + 1;
        if map.beat_to_frame(beat(u64::MAX)).unwrap() < frame {
            return sentinel;
        }
        let mut low = 0_u128;
        let mut high = sentinel;
        while low < high {
            let middle = low + (high - low) / 2;
            if map.beat_to_frame(beat(middle as u64)).unwrap() >= frame {
                high = middle;
            } else {
                low = middle + 1;
            }
        }
        low
    }

    fn periodic_clock_fixture() -> (PreparedTempoMap, LoopRegion, PreparedPeriodicSessionTempo) {
        let canonical = TempoMap::new(
            48_000,
            vec![
                TempoPoint::new(beat(0), tempo(120_000_000)),
                TempoPoint::new(beat(BeatTime::ONE.raw()), tempo(90_000_000)),
                TempoPoint::new(beat(BeatTime::ONE.raw() * 3), tempo(150_000_000)),
                // This post-loop marker must never affect periodic cycles.
                TempoPoint::new(beat(BeatTime::ONE.raw() * 8), tempo(600_000_000)),
            ],
        )
        .unwrap();
        let prepared = PreparedTempoMap::prepare(canonical, 48_000).unwrap();
        let loop_region = LoopRegion::new(1_237, 91_111, true).unwrap();
        let cycle = PreparedPeriodicSessionTempo::prepare(&prepared, loop_region).unwrap();
        (prepared, loop_region, cycle)
    }

    #[test]
    fn periodic_clock_boundaries_match_canonical_tempo() {
        let (prepared, _loop_region, cycle) = periodic_clock_fixture();
        let view = cycle.view(&prepared);
        // Admission covers a later absolute cursor too; the coordinate checks
        // below still prove that the 600 BPM marker never enters loop cycles.
        assert_eq!(view.max_micro_bpm(), 600_000_000);

        assert_eq!(view.beat_to_frame(cycle.start_beat()).unwrap(), 1_237);
        assert_eq!(view.beat_to_frame(cycle.end_beat()).unwrap(), 91_111);
        assert!(cycle.beat_period() > 0);
        assert!(cycle.frame_period() > 0);

        for raw in [
            0,
            cycle.start_beat().raw().saturating_sub(1),
            cycle.start_beat().raw(),
            cycle.start_beat().raw() + cycle.beat_period() / 3,
            cycle.end_beat().raw().saturating_sub(1),
            cycle.end_beat().raw(),
            cycle.end_beat().raw() + cycle.beat_period() * 4,
        ] {
            let position = beat(raw);
            let actual = view.beat_to_frame(position).unwrap();
            if position < cycle.start_beat() {
                assert_eq!(actual, prepared.beat_to_frame(position).unwrap());
            } else {
                let delta = position.raw() - cycle.start_beat().raw();
                let iterations = delta / cycle.beat_period();
                let local_raw = cycle.start_beat().raw() + delta % cycle.beat_period();
                let local = prepared.beat_to_frame(beat(local_raw)).unwrap();
                let expected = cycle.start_frame
                    + iterations * cycle.frame_period()
                    + (local - cycle.start_frame);
                assert_eq!(actual, expected);
            }
        }
    }

    // Partition the original exhaustive range without gaps or overlap. Each
    // frame retains the independent full-range binary-search oracle.
    fn check_periodic_clock_frame_partition(partition: u64) {
        const PARTITIONS: u64 = 64;
        let (prepared, loop_region, cycle) = periodic_clock_fixture();
        let view = cycle.view(&prepared);
        let start = loop_region.start().saturating_sub(2);
        let end_exclusive = loop_region.start() + cycle.frame_period() * 9 + 3;
        let count = end_exclusive - start;
        let partition_start = start + count * partition / PARTITIONS;
        let partition_end = start + count * (partition + 1) / PARTITIONS;
        for frame in partition_start..partition_end {
            let actual = view.beat_lower_bound_for_frame(frame).unwrap();
            let expected = lower_bound_oracle(&view, frame);
            assert_eq!(actual, expected, "lower bound at frame {frame}");
            if actual != TempoMap::BEAT_RANGE_SENTINEL {
                assert!(view.beat_to_frame(beat(actual as u64)).unwrap() >= frame);
                if actual > 0 {
                    assert!(view.beat_to_frame(beat(actual as u64 - 1)).unwrap() < frame);
                }
            }
            if frame < u64::MAX {
                let beat = view.frame_to_beat(frame).unwrap();
                assert_eq!(view.beat_to_frame(beat).unwrap(), frame);
                if frame > loop_region.start() && frame < loop_region.end() + cycle.frame_period() {
                    assert!(beat >= cycle.start_beat());
                    let second_cycle_end =
                        u128::from(cycle.start_beat().raw()) + 2 * u128::from(cycle.beat_period());
                    assert!(u128::from(beat.raw()) < second_cycle_end);
                    if frame < loop_region.end() {
                        assert!(beat < cycle.end_beat());
                    }
                }
            }
        }
    }

    macro_rules! periodic_clock_partition {
        ($name:ident, $partition:expr) => {
            #[test]
            fn $name() {
                check_periodic_clock_frame_partition($partition);
            }
        };
    }

    periodic_clock_partition!(periodic_clock_frames_00, 0);
    periodic_clock_partition!(periodic_clock_frames_01, 1);
    periodic_clock_partition!(periodic_clock_frames_02, 2);
    periodic_clock_partition!(periodic_clock_frames_03, 3);
    periodic_clock_partition!(periodic_clock_frames_04, 4);
    periodic_clock_partition!(periodic_clock_frames_05, 5);
    periodic_clock_partition!(periodic_clock_frames_06, 6);
    periodic_clock_partition!(periodic_clock_frames_07, 7);
    periodic_clock_partition!(periodic_clock_frames_08, 8);
    periodic_clock_partition!(periodic_clock_frames_09, 9);
    periodic_clock_partition!(periodic_clock_frames_10, 10);
    periodic_clock_partition!(periodic_clock_frames_11, 11);
    periodic_clock_partition!(periodic_clock_frames_12, 12);
    periodic_clock_partition!(periodic_clock_frames_13, 13);
    periodic_clock_partition!(periodic_clock_frames_14, 14);
    periodic_clock_partition!(periodic_clock_frames_15, 15);
    periodic_clock_partition!(periodic_clock_frames_16, 16);
    periodic_clock_partition!(periodic_clock_frames_17, 17);
    periodic_clock_partition!(periodic_clock_frames_18, 18);
    periodic_clock_partition!(periodic_clock_frames_19, 19);
    periodic_clock_partition!(periodic_clock_frames_20, 20);
    periodic_clock_partition!(periodic_clock_frames_21, 21);
    periodic_clock_partition!(periodic_clock_frames_22, 22);
    periodic_clock_partition!(periodic_clock_frames_23, 23);
    periodic_clock_partition!(periodic_clock_frames_24, 24);
    periodic_clock_partition!(periodic_clock_frames_25, 25);
    periodic_clock_partition!(periodic_clock_frames_26, 26);
    periodic_clock_partition!(periodic_clock_frames_27, 27);
    periodic_clock_partition!(periodic_clock_frames_28, 28);
    periodic_clock_partition!(periodic_clock_frames_29, 29);
    periodic_clock_partition!(periodic_clock_frames_30, 30);
    periodic_clock_partition!(periodic_clock_frames_31, 31);
    periodic_clock_partition!(periodic_clock_frames_32, 32);
    periodic_clock_partition!(periodic_clock_frames_33, 33);
    periodic_clock_partition!(periodic_clock_frames_34, 34);
    periodic_clock_partition!(periodic_clock_frames_35, 35);
    periodic_clock_partition!(periodic_clock_frames_36, 36);
    periodic_clock_partition!(periodic_clock_frames_37, 37);
    periodic_clock_partition!(periodic_clock_frames_38, 38);
    periodic_clock_partition!(periodic_clock_frames_39, 39);
    periodic_clock_partition!(periodic_clock_frames_40, 40);
    periodic_clock_partition!(periodic_clock_frames_41, 41);
    periodic_clock_partition!(periodic_clock_frames_42, 42);
    periodic_clock_partition!(periodic_clock_frames_43, 43);
    periodic_clock_partition!(periodic_clock_frames_44, 44);
    periodic_clock_partition!(periodic_clock_frames_45, 45);
    periodic_clock_partition!(periodic_clock_frames_46, 46);
    periodic_clock_partition!(periodic_clock_frames_47, 47);
    periodic_clock_partition!(periodic_clock_frames_48, 48);
    periodic_clock_partition!(periodic_clock_frames_49, 49);
    periodic_clock_partition!(periodic_clock_frames_50, 50);
    periodic_clock_partition!(periodic_clock_frames_51, 51);
    periodic_clock_partition!(periodic_clock_frames_52, 52);
    periodic_clock_partition!(periodic_clock_frames_53, 53);
    periodic_clock_partition!(periodic_clock_frames_54, 54);
    periodic_clock_partition!(periodic_clock_frames_55, 55);
    periodic_clock_partition!(periodic_clock_frames_56, 56);
    periodic_clock_partition!(periodic_clock_frames_57, 57);
    periodic_clock_partition!(periodic_clock_frames_58, 58);
    periodic_clock_partition!(periodic_clock_frames_59, 59);
    periodic_clock_partition!(periodic_clock_frames_60, 60);
    periodic_clock_partition!(periodic_clock_frames_61, 61);
    periodic_clock_partition!(periodic_clock_frames_62, 62);
    periodic_clock_partition!(periodic_clock_frames_63, 63);

    #[test]
    fn periodic_lower_bound_seam_uses_previous_cycle_tail() {
        let canonical = TempoMap::constant(8_000, tempo(937_500_000)).unwrap();
        let prepared = PreparedTempoMap::prepare(canonical, 8_000).unwrap();
        let cycle = PreparedPeriodicSessionTempo::prepare(
            &prepared,
            LoopRegion::new(10, 11, true).unwrap(),
        )
        .unwrap();
        let view = cycle.view(&prepared);
        for frame in 10..=40 {
            assert_eq!(
                view.beat_lower_bound_for_frame(frame).unwrap(),
                lower_bound_oracle(&view, frame),
                "frame {frame}"
            );
        }
    }

    #[test]
    fn quantized_marker_at_loop_start_uses_canonical_latest_boundary() {
        let canonical = TempoMap::new(
            8_000,
            vec![
                TempoPoint::new(beat(0), tempo(937_500_000)),
                TempoPoint::new(beat(1), tempo(60_000_000)),
                TempoPoint::new(beat(2), tempo(240_000_000)),
            ],
        )
        .unwrap();
        assert_eq!(canonical.beat_to_frame(beat(1)).unwrap(), 0);
        assert_eq!(canonical.beat_to_frame(beat(2)).unwrap(), 0);
        let prepared = PreparedTempoMap::prepare(canonical.clone(), 8_000).unwrap();
        let cycle =
            PreparedPeriodicSessionTempo::prepare(&prepared, LoopRegion::new(0, 1, true).unwrap())
                .unwrap();
        assert_eq!(cycle.start_beat(), canonical.frame_to_beat(0).unwrap());
        assert_eq!(cycle.end_beat(), canonical.frame_to_beat(1).unwrap());
        let view = cycle.view(&prepared);
        for frame in 1..=64 {
            let lower = view.beat_lower_bound_for_frame(frame).unwrap();
            assert_eq!(lower, lower_bound_oracle(&view, frame));
            assert!(view.beat_to_frame(beat(lower as u64)).unwrap() >= frame);
            assert!(view.beat_to_frame(beat(lower as u64 - 1)).unwrap() < frame);
        }
    }

    #[test]
    fn quantized_marker_at_loop_end_uses_canonical_latest_boundary() {
        let first_marker = (1_u64 << 23) + 1;
        let second_marker = first_marker + 1;
        let canonical = TempoMap::new(
            8_000,
            vec![
                TempoPoint::new(beat(0), tempo(937_500_000)),
                TempoPoint::new(beat(first_marker), tempo(60_000_000)),
                TempoPoint::new(beat(second_marker), tempo(240_000_000)),
            ],
        )
        .unwrap();
        assert_eq!(canonical.beat_to_frame(beat(first_marker)).unwrap(), 1);
        assert_eq!(canonical.beat_to_frame(beat(second_marker)).unwrap(), 1);
        assert_eq!(canonical.frame_to_beat(1).unwrap(), beat(second_marker));
        let prepared = PreparedTempoMap::prepare(canonical.clone(), 8_000).unwrap();
        let cycle =
            PreparedPeriodicSessionTempo::prepare(&prepared, LoopRegion::new(0, 1, true).unwrap())
                .unwrap();
        assert_eq!(cycle.end_beat(), beat(second_marker));
        let view = cycle.view(&prepared);
        for frame in 1..=64 {
            let lower = view.beat_lower_bound_for_frame(frame).unwrap();
            assert_eq!(lower, lower_bound_oracle(&view, frame));
            assert!(view.beat_to_frame(beat(lower as u64)).unwrap() >= frame);
            assert!(view.beat_to_frame(beat(lower as u64 - 1)).unwrap() < frame);
        }
    }

    #[test]
    fn periodic_clock_handles_large_cycle_counts_and_frame_domain_exhaustion() {
        let canonical = TempoMap::constant(48_000, tempo(120_000_000)).unwrap();
        let prepared = PreparedTempoMap::prepare(canonical, 48_000).unwrap();
        let cycle = PreparedPeriodicSessionTempo::prepare(
            &prepared,
            LoopRegion::new(0, 48_000, true).unwrap(),
        )
        .unwrap();
        let view = cycle.view(&prepared);
        assert!(view.frame_to_beat(u64::MAX).is_err());
        assert_eq!(
            view.beat_lower_bound_for_frame(u64::MAX).unwrap(),
            TempoMap::BEAT_RANGE_SENTINEL
        );
        assert!(
            view.beat_to_frame(beat(u64::MAX))
                .expect("representable raw beat conversion")
                < u64::MAX
        );
    }

    #[test]
    fn slowest_supported_periodic_clock_stays_inside_frame_domain() {
        // Even the highest supported rate at the lowest supported tempo maps
        // the entire Q32 beat domain well below u64::MAX frames. Thus a
        // forward-frame overflow is unreachable for a valid canonical map.
        let canonical = TempoMap::constant(192_000, tempo(1_000_000)).unwrap();
        let prepared = PreparedTempoMap::prepare(canonical, 192_000).unwrap();
        let one_beat_frame = prepared.beat_to_frame(BeatTime::ONE).unwrap();
        let cycle = PreparedPeriodicSessionTempo::prepare(
            &prepared,
            LoopRegion::new(0, one_beat_frame, true).unwrap(),
        )
        .unwrap();
        let view = cycle.view(&prepared);
        let max_beat_frame = view.beat_to_frame(beat(u64::MAX)).unwrap();
        assert!(max_beat_frame < 50_000_000_000_000_000);
        assert!(max_beat_frame < u64::MAX);
    }

    #[test]
    fn anchored_clock_preserves_cut_beat_and_translates_future_conversions() {
        let old = PreparedTempoMap::prepare(
            TempoMap::constant(48_000, tempo(120_000_000)).unwrap(),
            48_000,
        )
        .unwrap();
        let new = PreparedTempoMap::prepare(
            TempoMap::constant(48_000, tempo(90_000_000)).unwrap(),
            48_000,
        )
        .unwrap();
        let cut = 24_000;
        let old_beat = old.frame_to_beat(cut).unwrap();
        let new_beat = new.frame_to_beat(cut).unwrap();
        let base = super::SessionBaseTimingView::Canonical(&new);
        let anchored = super::AnchoredSessionTempoView::new(
            base,
            super::SessionClockAnchor {
                cut_frame: cut,
                old_beat,
                new_beat,
            },
        );
        assert_eq!(anchored.frame_to_beat(cut).unwrap(), old_beat);
        assert_eq!(anchored.beat_to_frame(old_beat).unwrap(), cut);
        for frame in [cut, cut + 1, cut + 32_000, cut + 96_000] {
            let effective = anchored.frame_to_beat(frame).unwrap();
            let base_beat = new.frame_to_beat(frame).unwrap();
            let expected_raw = i128::from(old_beat.raw()) + i128::from(base_beat.raw())
                - i128::from(new_beat.raw());
            assert_eq!(i128::from(effective.raw()), expected_raw);
            assert_eq!(anchored.beat_to_frame(effective).unwrap(), frame);
        }
    }

    #[test]
    fn anchored_clock_rejects_frames_before_its_beat_domain() {
        let map = PreparedTempoMap::prepare(
            TempoMap::constant(48_000, tempo(120_000_000)).unwrap(),
            48_000,
        )
        .unwrap();
        let anchored = super::AnchoredSessionTempoView::new(
            super::SessionBaseTimingView::Canonical(&map),
            super::SessionClockAnchor {
                cut_frame: 24_000,
                old_beat: BeatTime::ZERO,
                new_beat: BeatTime::ONE,
            },
        );
        // Frames before the cut precede anchored beat zero: both directions refuse.
        assert!(anchored.frame_to_beat(0).is_err());
        assert!(anchored.frame_to_beat(23_999).is_err());
        assert_eq!(anchored.frame_to_beat(24_000).unwrap(), BeatTime::ZERO);
        assert_eq!(anchored.beat_to_frame(BeatTime::ZERO).unwrap(), 24_000);
        // The lower bound is the first domain beat reaching the frame. Beat
        // zero reaches every frame up to the cut, so it is exact there.
        assert_eq!(anchored.beat_lower_bound_for_frame(0).unwrap(), 0);
        assert_eq!(anchored.beat_lower_bound_for_frame(24_000).unwrap(), 0);
        let after_cut = anchored.beat_lower_bound_for_frame(24_001).unwrap();
        assert!(after_cut > 0);
        let after_cut = BeatTime::from_raw(u64::try_from(after_cut).unwrap());
        assert!(anchored.beat_to_frame(after_cut).unwrap() >= 24_001);
    }
}
