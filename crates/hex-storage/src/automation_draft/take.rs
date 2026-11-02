//! Read-only, control-side content queries over completed shared automation captures.
//!
//! The locked reader owns exact normalized records and the retained source.
//! This owner scans every application and the genuine release using one fixed page,
//! then maps content to its latest covered performance occurrence in constant space.
//! Binary search reads the held event without a resident event or loop-pass index.
//! Same-frame applications resolve by their increasing sequence. Capture metadata
//! are a caller's fenced clock declaration, not proof that arbitrary file data were
//! delivered. Version-one wrapped drafts lack that declaration and remain recovery
//! data; unwrapped intervals can be checked without guessing a saved loop law.
//!
//! Queries use project-rate sample frames. Native publication requires matching
//! project/output rates; mismatched provenance refuses rather than resampling it.
//! Target input/output origins may have different offsets from performance time.
//! No canonical adoption, editable placement, DSP, callback I/O or file reclamation is
//! implemented here. Future playback must prepare bounded pages through its existing
//! resource owner; retaining this reader never grants a callback a disk operation.
use super::{
    AppliedEvent, AutomationContentIdentity, CaptureClock, CompletedDraft, DraftClock, DraftError,
    DraftProvenance, DraftReadLimits, DraftTarget, MAX_APPEND_EVENTS,
};
use hex_project::{CapturedAutomationProjection, LoopRegion, Project, TempoMap};
use std::{ops::Range, path::Path, sync::Arc};

/// An open refusal retains the original draft; busy means its lock is held.
#[derive(Debug)]
pub enum TakeOpenError {
    Busy(DraftError),
    Other(String),
}
impl std::fmt::Display for TakeOpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy(error) => error.fmt(f),
            Self::Other(error) => f.write_str(error),
        }
    }
}
impl std::error::Error for TakeOpenError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Busy(error) => Some(error),
            Self::Other(_) => None,
        }
    }
}

pub const MAX_PREVIEW_QUERIES: usize = 64;

/// A semantically checked immutable source, independent of an open document.
/// The original file survives Drop and every refusal. Canonical source equivalence,
/// capability validation and saved/history/runtime custody still precede adoption.
pub struct CompletedAutomationTake {
    core: TakeCore<CompletedDraft>,
}

trait TakeReader {
    fn captured_tempo_map(&self) -> &TempoMap;
    fn target(&self) -> DraftTarget;
    fn provenance(&self) -> DraftProvenance;
    fn capture_clock(&self) -> Option<CaptureClock>;
    fn end_clock(&self) -> DraftClock;
    fn content_identity(&self) -> AutomationContentIdentity;
    fn event_count(&self) -> u64;
    fn path(&self) -> &Path;
    fn read_events(
        &mut self,
        first: u64,
        output: &mut [Option<AppliedEvent>],
    ) -> Result<usize, DraftError>;
    fn event_with_ordinal_at_or_before(
        &mut self,
        performance: u64,
    ) -> Result<Option<(u64, AppliedEvent)>, DraftError>;
    fn event_at_or_before(&mut self, performance: u64) -> Result<Option<AppliedEvent>, DraftError> {
        self.event_with_ordinal_at_or_before(performance)
            .map(|event| event.map(|(_, event)| event))
    }
}
impl TakeReader for CompletedDraft {
    fn captured_tempo_map(&self) -> &TempoMap {
        self.source().tempo_map()
    }
    fn target(&self) -> DraftTarget {
        self.target()
    }
    fn provenance(&self) -> DraftProvenance {
        self.provenance()
    }
    fn capture_clock(&self) -> Option<CaptureClock> {
        self.capture_clock()
    }
    fn end_clock(&self) -> DraftClock {
        self.end_clock()
    }
    fn content_identity(&self) -> AutomationContentIdentity {
        self.content_identity()
    }
    fn event_count(&self) -> u64 {
        self.event_count()
    }
    fn path(&self) -> &Path {
        self.path()
    }
    fn read_events(
        &mut self,
        first: u64,
        output: &mut [Option<AppliedEvent>],
    ) -> Result<usize, DraftError> {
        self.read_events(first, output)
    }
    fn event_with_ordinal_at_or_before(
        &mut self,
        performance: u64,
    ) -> Result<Option<(u64, AppliedEvent)>, DraftError> {
        self.event_with_ordinal_at_or_before(performance)
    }
}
struct TakeCore<R> {
    source: R,
    mapping: Option<ClockMapping>,
    cursor_owner: Arc<()>,
}
impl CompletedAutomationTake {
    pub fn open_on_control(path: &Path, limits: DraftReadLimits) -> Result<Self, TakeOpenError> {
        let source = CompletedDraft::open(path, limits).map_err(|error| {
            if matches!(&*error.source, crate::StorageError::AutomationDraftBusy(_)) {
                TakeOpenError::Busy(error)
            } else {
                TakeOpenError::Other(error.to_string())
            }
        })?;
        Self::from_completed_on_control(source).map_err(TakeOpenError::Other)
    }

    pub fn from_completed_on_control(mut source: CompletedDraft) -> Result<Self, String> {
        let invalid = |reason| {
            format!(
                "automation take at {}: {reason}",
                crate::text::verbatim(source.path().display())
            )
        };
        if source.provenance().sample_rate != source.source().sample_rate() {
            return Err(invalid("capture and source sample rates differ"));
        }
        let mut page = [None; MAX_APPEND_EVENTS];
        let count = source
            .read_events(0, &mut page)
            .map_err(|error| error.to_string())?;
        let Some(first) = page.first().copied().flatten() else {
            return Ok(Self {
                core: TakeCore {
                    source,
                    mapping: None,
                    cursor_owner: Arc::new(()),
                },
            });
        };
        let end = source.end_clock();
        let region = match source.capture_clock() {
            Some(clock) => {
                if clock.sample_rate != source.source().sample_rate() {
                    return Err(format!(
                        "automation take at {}: captured clock rate differs from source",
                        crate::text::verbatim(source.path().display())
                    ));
                }
                clock.loop_region
            }
            None if first.loop_pass == end.loop_pass => None,
            None => {
                return Err(format!(
                    "automation take at {}: wrapped draft has no captured loop context; retain it for recovery",
                    crate::text::verbatim(source.path().display())
                ));
            }
        };
        let mapping = ClockMapping::new(first, end, region).map_err(|reason| {
            format!(
                "automation take at {}: {reason}",
                crate::text::verbatim(source.path().display())
            )
        })?;
        let mut ordinal = 0;
        let mut count = count;
        loop {
            for event in page[..count].iter().flatten() {
                let clock = mapping
                    .clock_at(event.performance_frame)
                    .map_err(|reason| {
                        format!(
                            "automation take at {}: {reason}",
                            crate::text::verbatim(source.path().display())
                        )
                    })?;
                if clock != event_clock(*event) {
                    return Err(format!(
                        "automation take at {}: application clock is discontinuous",
                        crate::text::verbatim(source.path().display())
                    ));
                }
            }
            ordinal += count as u64;
            if ordinal == source.event_count() {
                break;
            }
            count = source
                .read_events(ordinal, &mut page)
                .map_err(|error| error.to_string())?;
            if count == 0 {
                return Err("automation take lost its validated event prefix".into());
            }
        }
        Ok(Self {
            core: TakeCore {
                source,
                mapping: Some(mapping),
                cursor_owner: Arc::new(()),
            },
        })
    }

    pub fn source(&self) -> &Project {
        self.core.source.source()
    }
}

impl<R: TakeReader> TakeCore<R> {
    fn captured_tempo_map(&self) -> &TempoMap {
        self.source.captured_tempo_map()
    }
    pub fn capture_clock(&self) -> Option<CaptureClock> {
        self.source.capture_clock()
    }
    pub fn end_clock(&self) -> DraftClock {
        self.source.end_clock()
    }
    pub fn content_identity(&self) -> AutomationContentIdentity {
        self.source.content_identity()
    }
    pub fn target(&self) -> DraftTarget {
        self.source.target()
    }
    pub fn provenance(&self) -> DraftProvenance {
        self.source.provenance()
    }
    pub fn path(&self) -> &Path {
        self.source.path()
    }
    pub fn event_count(&self) -> u64 {
        self.source.event_count()
    }

    /// Exact source-frame coverage, compressed to at most two disjoint ranges
    /// regardless of loop count. The original authored base owns every gap.
    pub fn captured_coverage(&self) -> [Option<Range<u64>>; 2] {
        self.mapping.map_or([None, None], ClockMapping::coverage)
    }

    /// Forward-projected half-open coverage. Collapsed ranges contribute no
    /// destination samples; conversion failure never becomes outside coverage.
    pub fn projected_coverage(
        &self,
        destination: &TempoMap,
    ) -> Result<[Option<Range<u64>>; 2], String> {
        let projection = CapturedAutomationProjection::new(self.captured_tempo_map(), destination);
        let mut output = [None, None];
        for (range, slot) in self.captured_coverage().into_iter().zip(&mut output) {
            if let Some(range) = range {
                let placed = projection
                    .project_range(range)
                    .map_err(|error| error.to_string())?;
                if !placed.is_empty() {
                    *slot = Some(placed);
                }
            }
        }
        Ok(output)
    }

    /// Preview through the current musical clock without rewriting captured
    /// events or cloning tempo maps. Forward predicates resolve rounded event
    /// collisions; the returned events retain their original application stamps.
    /// Future callback consumers still require worker-prepared scalar pages.
    pub fn read_projected_preview_on_control(
        &mut self,
        destination: &TempoMap,
        frames: &[u64],
        output: &mut [Option<AppliedEvent>],
    ) -> Result<(), String> {
        output.fill(None);
        if frames.len() != output.len() || frames.len() > MAX_PREVIEW_QUERIES {
            return Err("automation preview needs matching buffers of at most 64 positions".into());
        }
        self.projected_coverage(destination)?;
        let coverage = self.captured_coverage();
        let result = (|| {
            for (&frame, slot) in frames.iter().zip(output.iter_mut()) {
                let captured = {
                    let projection =
                        CapturedAutomationProjection::new(self.captured_tempo_map(), destination);
                    let mut captured = None;
                    for range in coverage.iter().flatten() {
                        if let Some(source_frame) = projection
                            .source_frame_at(frame, range.clone())
                            .map_err(|error| error.to_string())?
                        {
                            captured = Some(source_frame);
                            break;
                        }
                    }
                    captured
                };
                if let Some(captured) = captured {
                    *slot = self.value_at_on_control(captured)?;
                }
            }
            Ok(())
        })();
        if result.is_err() {
            output.fill(None);
        }
        result
    }

    /// None is outside half-open touch coverage, where the unchanged base owns
    /// the value. An invalid/unavailable source is an error, never a base value.
    pub fn value_at_on_control(
        &mut self,
        content_frame: u64,
    ) -> Result<Option<AppliedEvent>, String> {
        let Some(performance) = self.mapping.and_then(|map| map.latest_at(content_frame)) else {
            return Ok(None);
        };
        let event = self
            .source
            .event_at_or_before(performance)
            .map_err(|error| error.to_string())?
            .ok_or("automation take has no held event inside its coverage")?;
        Ok(Some(event))
    }

    /// Bounded preview consumer of the same held-value query. Caller-provided
    /// positions need not be sorted; no partial preview escapes a failed read.
    pub fn read_preview_on_control(
        &mut self,
        content_frames: &[u64],
        output: &mut [Option<AppliedEvent>],
    ) -> Result<(), String> {
        output.fill(None);
        if content_frames.len() != output.len() || content_frames.len() > MAX_PREVIEW_QUERIES {
            return Err("automation preview needs matching buffers of at most 64 positions".into());
        }
        for (&frame, slot) in content_frames.iter().zip(output.iter_mut()) {
            match self.value_at_on_control(frame) {
                Ok(event) => *slot = event,
                Err(error) => {
                    output.fill(None);
                    return Err(error);
                }
            }
        }
        Ok(())
    }
}

fn event_clock(event: AppliedEvent) -> DraftClock {
    DraftClock {
        performance_frame: event.performance_frame,
        content_frame: event.content_frame,
        loop_pass: event.loop_pass,
    }
}

#[derive(Clone, Copy)]
struct ClockMapping {
    first: DraftClock,
    end: DraftClock,
    region: Option<LoopRegion>,
}
impl ClockMapping {
    fn coverage(self) -> [Option<Range<u64>>; 2] {
        let duration = self.end.performance_frame - self.first.performance_frame;
        if duration == 0 {
            return [None, None];
        }
        let Some(region) = self.region else {
            return [Some(self.first.content_frame..self.end.content_frame), None];
        };
        let initial = region.end() - self.first.content_frame;
        let first = self.first.content_frame..self.first.content_frame + duration.min(initial);
        if duration <= initial {
            return [Some(first), None];
        }
        let repeated = region.start()
            ..region.start() + (duration - initial).min(region.end() - region.start());
        if first.start <= repeated.end && repeated.start <= first.end {
            [
                Some(first.start.min(repeated.start)..first.end.max(repeated.end)),
                None,
            ]
        } else {
            [Some(repeated), Some(first)]
        }
    }

    fn new(
        first: AppliedEvent,
        end: DraftClock,
        region: Option<LoopRegion>,
    ) -> Result<Self, String> {
        let first = event_clock(first);
        if region.is_some_and(|region| !region.enabled()) {
            return Err("captured loop must be enabled".into());
        }
        let region =
            region.filter(|region| first.loop_pass > 0 || first.content_frame < region.end());
        if let Some(region) = region {
            if !region.enabled()
                || first.content_frame >= region.end()
                || (first.loop_pass > 0 && first.content_frame < region.start())
            {
                return Err("captured loop origin is invalid".into());
            }
        }
        let mapping = Self { first, end, region };
        if mapping.clock_at(end.performance_frame)? != end {
            return Err("release clock is discontinuous".into());
        }
        Ok(mapping)
    }

    fn clock_at(self, performance_frame: u64) -> Result<DraftClock, String> {
        if performance_frame < self.first.performance_frame
            || performance_frame > self.end.performance_frame
        {
            return Err("application lies outside its completed take".into());
        }
        let delta = performance_frame - self.first.performance_frame;
        let mut content = u128::from(self.first.content_frame) + u128::from(delta);
        let mut pass = u128::from(self.first.loop_pass);
        if let Some(region) = self.region {
            let initial = region.end() - self.first.content_frame;
            if delta >= initial {
                let remaining = delta - initial;
                let width = region.end() - region.start();
                content = u128::from(region.start() + remaining % width);
                pass += 1 + u128::from(remaining / width);
            }
        }
        Ok(DraftClock {
            performance_frame,
            content_frame: u64::try_from(content).map_err(|_| "capture content clock overflow")?,
            loop_pass: u64::try_from(pass).map_err(|_| "capture loop pass overflow")?,
        })
    }

    /// The periodic candidate is strictly before release. Earlier initial
    /// coverage remains available when the final pass does not reach a frame.
    fn latest_at(self, content: u64) -> Option<u64> {
        let first_performance = u128::from(self.first.performance_frame);
        let end = u128::from(self.end.performance_frame);
        let initial = if content >= self.first.content_frame
            && self.region.is_none_or(|region| content < region.end())
        {
            let performance = first_performance + u128::from(content - self.first.content_frame);
            (performance < end).then_some(performance)
        } else {
            None
        };
        let periodic = self.region.and_then(|region| {
            if !(region.start()..region.end()).contains(&content) {
                return None;
            }
            let first = first_performance
                + u128::from(region.end() - self.first.content_frame)
                + u128::from(content - region.start());
            if first >= end {
                return None;
            }
            let width = u128::from(region.end() - region.start());
            Some(first + ((end - 1 - first) / width) * width)
        });
        periodic
            .or(initial)
            .and_then(|frame| u64::try_from(frame).ok())
    }
}

mod prepared;
pub use prepared::{
    AutomationSharedData, PreparedAutomationTake, PreparedAutomationTakeDataLayout,
    automation_projection_fingerprint,
};

mod spans;
pub use spans::{
    MAX_SCALAR_RANGE_FRAMES, MAX_SPANS_PER_STEP, ProjectedAutomationSpan, ProjectedSpanCursor,
    SpanReadProgress, SpanReadState,
};

// Public wrappers delegate every query to the same private interpreter. The
// lean wrapper's type contains no Project/FileToken alternative or optional slot.
macro_rules! take_queries {
    ($type:ty) => {
        impl $type {
            pub fn captured_tempo_map(&self) -> &TempoMap {
                self.core.captured_tempo_map()
            }
            pub fn target(&self) -> DraftTarget {
                self.core.target()
            }
            pub fn provenance(&self) -> DraftProvenance {
                self.core.provenance()
            }
            pub fn capture_clock(&self) -> Option<CaptureClock> {
                self.core.capture_clock()
            }
            pub fn end_clock(&self) -> DraftClock {
                self.core.end_clock()
            }
            pub fn content_identity(&self) -> AutomationContentIdentity {
                self.core.content_identity()
            }
            pub fn path(&self) -> &Path {
                self.core.path()
            }
            pub fn event_count(&self) -> u64 {
                self.core.event_count()
            }
            pub fn captured_coverage(&self) -> [Option<Range<u64>>; 2] {
                self.core.captured_coverage()
            }
            pub fn projected_coverage(
                &self,
                destination: &TempoMap,
            ) -> Result<[Option<Range<u64>>; 2], String> {
                self.core.projected_coverage(destination)
            }
            pub fn value_at_on_control(
                &mut self,
                frame: u64,
            ) -> Result<Option<AppliedEvent>, String> {
                self.core.value_at_on_control(frame)
            }
            pub fn read_preview_on_control(
                &mut self,
                frames: &[u64],
                output: &mut [Option<AppliedEvent>],
            ) -> Result<(), String> {
                self.core.read_preview_on_control(frames, output)
            }
            pub fn read_projected_preview_on_control(
                &mut self,
                destination: &TempoMap,
                frames: &[u64],
                output: &mut [Option<AppliedEvent>],
            ) -> Result<(), String> {
                self.core
                    .read_projected_preview_on_control(destination, frames, output)
            }
            pub fn begin_projected_spans_on_control(
                &mut self,
                destination: Arc<TempoMap>,
                range: Range<u64>,
            ) -> Result<ProjectedSpanCursor, String> {
                self.core
                    .begin_projected_spans_on_control(destination, range)
            }
            pub fn read_projected_spans_on_control(
                &mut self,
                destination: &Arc<TempoMap>,
                cursor: &mut ProjectedSpanCursor,
                input_budget: usize,
                output: &mut [Option<ProjectedAutomationSpan>],
            ) -> Result<SpanReadProgress, String> {
                self.core
                    .read_projected_spans_on_control(destination, cursor, input_budget, output)
            }
            pub fn read_projected_values_on_control(
                &mut self,
                destination: Arc<TempoMap>,
                start: u64,
                output: &mut [Option<hex_project::AutomationValue>],
            ) -> Result<(), String> {
                self.core
                    .read_projected_values_on_control(destination, start, output)
            }
        }
    };
}
take_queries!(CompletedAutomationTake);
take_queries!(PreparedAutomationTake);
