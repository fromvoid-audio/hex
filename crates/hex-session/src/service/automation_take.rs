//! Read-only, control-side content queries over completed shared automation captures.
//!
//! The locked storage reader owns exact normalized records and the retained source.
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
//! No canonical adoption, beat placement, DSP, callback I/O or file reclamation is
//! implemented here. Future playback must prepare bounded pages through its existing
//! resource owner; retaining this reader never grants a callback a disk operation.
use hex_project::{LoopRegion, Project};
use hex_storage::automation_draft::{
    AppliedEvent, CompletedDraft, DraftClock, DraftProvenance, DraftReadLimits, DraftTarget,
    MAX_APPEND_EVENTS,
};
use std::path::Path;

pub const MAX_PREVIEW_QUERIES: usize = 64;

/// A semantically checked immutable source, independent of an open document.
/// The original file survives Drop and every refusal. Canonical source equivalence,
/// capability validation and saved/history/runtime custody still precede adoption.
pub struct CompletedAutomationTake {
    source: CompletedDraft,
    mapping: Option<ClockMapping>,
}
impl CompletedAutomationTake {
    pub fn open_on_control(path: &Path, limits: DraftReadLimits) -> Result<Self, String> {
        Self::from_completed_on_control(
            CompletedDraft::open(path, limits).map_err(|error| error.to_string())?,
        )
    }

    pub fn from_completed_on_control(mut source: CompletedDraft) -> Result<Self, String> {
        let invalid = |reason| format!("automation take at {}: {reason}", source.path().display());
        if source.provenance().sample_rate != source.source().sample_rate() {
            return Err(invalid("capture and source sample rates differ"));
        }
        let mut page = [None; MAX_APPEND_EVENTS];
        let count = source
            .read_events(0, &mut page)
            .map_err(|error| error.to_string())?;
        let Some(first) = page.first().copied().flatten() else {
            return Ok(Self {
                source,
                mapping: None,
            });
        };
        let end = source.end_clock();
        let region = match source.capture_clock() {
            Some(clock) => {
                if clock.sample_rate != source.source().sample_rate() {
                    return Err(format!(
                        "automation take at {}: captured clock rate differs from source",
                        source.path().display()
                    ));
                }
                clock.loop_region
            }
            None if first.loop_pass == end.loop_pass => None,
            None => {
                return Err(format!(
                    "automation take at {}: wrapped draft has no captured loop context; retain it for recovery",
                    source.path().display()
                ));
            }
        };
        let mapping = ClockMapping::new(first, end, region).map_err(|reason| {
            format!("automation take at {}: {reason}", source.path().display())
        })?;
        let mut ordinal = 0;
        let mut count = count;
        loop {
            for event in page[..count].iter().flatten() {
                let clock = mapping
                    .clock_at(event.performance_frame)
                    .map_err(|reason| {
                        format!("automation take at {}: {reason}", source.path().display())
                    })?;
                if clock != event_clock(*event) {
                    return Err(format!(
                        "automation take at {}: application clock is discontinuous",
                        source.path().display()
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
            source,
            mapping: Some(mapping),
        })
    }

    pub fn source(&self) -> &Project {
        self.source.source()
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

#[cfg(test)]
mod tests;
