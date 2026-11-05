//! Immutable source-beat clip volume on the existing global musical clock.
//! Queries keep original frame-ramp endpoints; callback/provider partitions only
//! select offsets. Rounded buckets are coalesced by indexed source/tempo lookup,
//! never by replaying old cycles or iterating every collapsed authored marker.
use crate::prepared_tempo::MidiTempoBreaks;
use crate::{Invalid, SessionMidiOrigin};
use hex_project::{BeatTime, MidiClipVolume, MidiSourcePlayback};

pub(crate) mod arrangement;

pub(crate) const VOLUME_QUERY_FRAMES: usize = crate::MIDI_TEMPLATE_QUERY_FRAMES as usize;
// Conservative pre-collapse source density plus effective distinct-frame tempo
// boundaries. Four authored inventories permit intro + two seam windows; the
// existing 256-frame window and two placement boundaries complete the budget.
pub(crate) const MAX_VOLUME_BOUNDARIES_PER_WINDOW: usize =
    4 * hex_project::MAX_MIDI_VOLUME_POINTS_PER_CLIP + VOLUME_QUERY_FRAMES + 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VolumePlacement {
    Arrangement { start: BeatTime },
    Session { origin: SessionMidiOrigin },
}

/// Endpoints belong to the prepared curve, not the current callback. A held
/// terminal value uses `u64::MAX` as its ramp end and has no next boundary.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct VolumeRamp {
    pub(crate) start_frame: u64,
    pub(crate) end_frame: u64,
    pub(crate) start_gain: f64,
    pub(crate) end_gain: f64,
    pub(crate) next_boundary: Option<u64>,
}
impl VolumeRamp {
    pub(crate) fn gain_at(self, frame: u64) -> Result<f64, Invalid> {
        let offset = frame
            .checked_sub(self.start_frame)
            .ok_or(Invalid("clip volume frame precedes ramp"))?;
        let span = self
            .end_frame
            .checked_sub(self.start_frame)
            .filter(|span| *span > 0)
            .ok_or(Invalid("clip volume ramp has no frame span"))?;
        if offset >= span {
            return Ok(self.end_gain);
        }
        if offset == 0 || self.start_gain == self.end_gain {
            return Ok(self.start_gain);
        }
        Ok(self.start_gain + (self.end_gain - self.start_gain) * (offset as f64 / span as f64))
    }
}

/// A finite owner's outgoing tail freezes the exact left limit. A terminal
/// authored jump's right value is outside the half-open playback interval.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct VolumeEndpoint {
    pub(crate) frame: u64,
    pub(crate) gain: f64,
}

#[derive(Clone, Debug)]
pub(crate) struct PreparedMidiClipVolume {
    volume: MidiClipVolume,
    source: MidiSourcePlayback,
    source_offset: u64,
    reversed: bool,
    constant: Option<f64>,
    clip_gain: f64,
    sample_rate: u32,
    max_micro_bpm: u64,
    source_boundaries: usize,
    metadata_bytes: usize,
}
/// Constructor-side metadata witness. Its retained curve reference prevents
/// pointer reuse from making a different preparation appear identical. This is
/// a conservative control-metadata owner, not a DATA allocation declaration.
#[derive(Debug, Clone)]
pub(crate) struct VolumePreparationIdentity(PreparedMidiClipVolume);
impl VolumePreparationIdentity {
    pub(crate) fn matches(&self, program: &PreparedMidiClipVolume) -> bool {
        let old = &self.0;
        old.volume.same_revision(&program.volume)
            && old.source == program.source
            && old.source_offset == program.source_offset
            && old.reversed == program.reversed
            && old.constant == program.constant
            && old.clip_gain == program.clip_gain
            && old.sample_rate == program.sample_rate
            && old.max_micro_bpm == program.max_micro_bpm
            && old.source_boundaries == program.source_boundaries
            && old.metadata_bytes == program.metadata_bytes
    }
    pub(crate) fn retained_control_metadata_bytes(&self) -> usize {
        self.0.metadata_bytes
    }
}
#[derive(Clone, Copy)]
struct Domain {
    origin: SessionMidiOrigin,
    floor_global: u64,
    end_elapsed: Option<u64>,
    end_global: Option<u64>,
}
#[derive(Clone, Copy)]
struct Neighbors {
    previous: u64,
    next: Option<u64>,
    constant: bool,
}
impl PreparedMidiClipVolume {
    /// One source-playback law shared by ordinary and sealed CURRENT Console
    /// compilation. This owns only immutable curve metadata, never note/PCM data.
    pub(crate) fn prepare_session<T: MidiTempoBreaks + ?Sized>(
        clip: &hex_project::SessionMidiClip,
        timing: &T,
    ) -> Result<Option<Self>, Invalid> {
        if clip.volume().is_unity() {
            return Ok(None);
        }
        let data = clip.source();
        let horizon = u64::MAX
            .checked_sub(clip.playback_offset().raw())
            .filter(|n| *n > 0)
            .ok_or(Invalid("clip volume source window exhausted"))?;
        let playback = data
            .playback_for_duration(BeatTime::from_raw(horizon))
            .and_then(|p| p.with_playback_offset(clip.playback_offset()))
            .map_err(|e| Invalid(e.0))?;
        Self::prepare(clip.volume(), playback, timing).map(Some)
    }
    pub(crate) fn preparation_identity(&self) -> VolumePreparationIdentity {
        VolumePreparationIdentity(self.clone())
    }
    /// Control-side density admission is independent of MIDI note density. The
    /// source bound deliberately overestimates intro/loop seam overlap; it may
    /// refuse a very short/dense loop even when many raw knots round together.
    /// Runtime work does not traverse that bound: each query performs indexed
    /// source lookup and adjacent effective-tempo lookup with positive progress.
    pub(crate) fn prepare<T: MidiTempoBreaks + ?Sized>(
        volume: &MidiClipVolume,
        source: MidiSourcePlayback,
        timing: &T,
    ) -> Result<Self, Invalid> {
        volume
            .validate_source_length(source.source_length())
            .map_err(|e| Invalid(e.0))?;
        if timing.sample_rate() == 0 || timing.max_micro_bpm() == 0 {
            return Err(Invalid("invalid clip volume timing admission"));
        }
        let clip_gain = f64::from(volume.clip_gain().linear());
        let constant = if let Some(first) = volume.points().first() {
            volume
                .points()
                .iter()
                .all(|p| p.incoming == first.incoming && p.value == first.incoming)
                .then_some(first.incoming.linear())
        } else {
            Some(1.0)
        }
        .map(|gain| gain * clip_gain);
        let source_boundaries = if constant.is_some() {
            2
        } else {
            // Same two-frame rounding/prefix margin as MIDI edge admission.
            let numerator = (u128::from(crate::MIDI_TEMPLATE_QUERY_FRAMES) + 2)
                .checked_mul(1_u128 << 32)
                .and_then(|n| n.checked_mul(u128::from(timing.max_micro_bpm())))
                .ok_or(Invalid("clip volume density arithmetic overflow"))?;
            let denominator = 60_000_000_u128 * u128::from(timing.sample_rate());
            let width = numerator / denominator + u128::from(numerator % denominator != 0);
            let mut count = volume.points().len() as u128 + 2;
            if let Some((start, end)) = source.loop_range() {
                let period = end.raw() - start.raw();
                let repeated = volume
                    .points()
                    .iter()
                    .filter(|p| p.beat >= start && p.beat < end)
                    .count() as u128
                    + 1;
                count = count
                    .checked_add(
                        (width / u128::from(period) + 2)
                            .checked_mul(repeated)
                            .ok_or(Invalid("clip volume density arithmetic overflow"))?,
                    )
                    .ok_or(Invalid("clip volume density arithmetic overflow"))?;
            }
            usize::try_from(count).map_err(|_| Invalid("clip volume density exceeds capacity"))?
        };
        let tempo = if constant.is_some() {
            0
        } else {
            timing
                .max_tempo_breaks_per_window(VOLUME_QUERY_FRAMES as u64)
                .map_err(|_| Invalid("clip volume tempo density is unavailable"))?
        };
        if source_boundaries
            .checked_add(tempo)
            .is_none_or(|n| n > MAX_VOLUME_BOUNDARIES_PER_WINDOW)
        {
            return Err(Invalid("clip volume density exceeds capacity"));
        }
        let source_offset = source.playback_offset().raw();
        let reversed = source.playback_reversed();
        let source = source
            .with_content_window(
                hex_project::ContentWindow::new(0, source.arrangement_duration().raw(), false)
                    .map_err(|e| Invalid(e.0))?,
            )
            .map_err(|e| Invalid(e.0))?;
        Ok(Self {
            volume: volume.clone(),
            source,
            source_offset,
            reversed,
            constant,
            clip_gain,
            sample_rate: timing.sample_rate(),
            max_micro_bpm: timing.max_micro_bpm(),
            source_boundaries,
            metadata_bytes: volume
                .retained_control_metadata_bytes()
                .ok_or(Invalid("clip volume metadata size overflow"))?,
        })
    }
    /// Curve identity belongs to this program; source/note occurrence identity
    /// deliberately remains independent so a gain-only edit can continue voices.
    #[cfg(test)]
    pub(crate) fn source_revision_matches(&self, volume: &MidiClipVolume) -> bool {
        self.volume.same_revision(volume)
    }

    /// Retained Arc payload/header bytes; the embedding owner's inline layout
    /// accounts for `Self` separately. No resource or PCM ownership is retained.
    pub(crate) fn retained_control_metadata_bytes(&self) -> usize {
        self.metadata_bytes
    }

    /// Evaluate once on the owning old clock before retirement/handoff. The
    /// returned scalar can outlive that clock; a replacement clock must not
    /// reinterpret a historical endpoint. Unbounded Session loops have no end.
    pub(crate) fn finite_endpoint<T: MidiTempoBreaks + ?Sized>(
        &self,
        timing: &T,
        placement: VolumePlacement,
    ) -> Result<Option<VolumeEndpoint>, Invalid> {
        if timing.sample_rate() != self.sample_rate || timing.max_micro_bpm() > self.max_micro_bpm {
            return Err(Invalid("clip volume timing exceeds admission"));
        }
        let Some(domain) = self.domain(placement)? else {
            return Ok(None);
        };
        let Some(end) = domain.end_global else {
            return Ok(None);
        };
        if timing
            .rebase_boundary()
            .map_err(|_| Invalid("clip volume clock rebase is unavailable"))?
            .is_some_and(|cut| end < cut.beat.raw())
        {
            return Err(Invalid("clip volume endpoint precedes clock rebase"));
        }
        Ok(Some(VolumeEndpoint {
            frame: mapped_frame(timing, end)?,
            gain: self.value_at(domain, end, true)?,
        }))
    }

    /// Stateless history can recover an immutable source-clock left limit
    /// without fabricating a frame on a retired clock. Stateful voices instead
    /// retain their already captured endpoint scalar in the occurrence ledger.
    pub(crate) fn historical_end_gain<T: MidiTempoBreaks + ?Sized>(
        &self,
        timing: &T,
        placement: VolumePlacement,
    ) -> Result<Option<f64>, Invalid> {
        let Some(domain) = self.domain(placement)? else {
            return Ok(None);
        };
        let Some(end) = domain.end_global else {
            return Ok(None);
        };
        let cut = timing
            .rebase_boundary()
            .map_err(|_| Invalid("clip volume clock rebase is unavailable"))?;
        if cut.is_some_and(|cut| end < cut.beat.raw()) {
            self.value_at(domain, end, true).map(Some)
        } else {
            Ok(None)
        }
    }

    /// Query an actual global frame with a checked caller window of at most256
    /// frames. Errors return no partial output and never mutate playback state.
    /// The full window domain is checked before returning any ramp; a caller
    /// stops at `next_boundary`, preserving these original endpoints and gains.
    pub(crate) fn segment_at<T: MidiTempoBreaks + ?Sized>(
        &self,
        timing: &T,
        placement: VolumePlacement,
        frame: u64,
        frames: usize,
    ) -> Result<Option<VolumeRamp>, Invalid> {
        if frames == 0 || frames > VOLUME_QUERY_FRAMES {
            return Err(Invalid("clip volume query exceeds frame capacity"));
        }
        if timing.sample_rate() != self.sample_rate || timing.max_micro_bpm() > self.max_micro_bpm {
            return Err(Invalid("clip volume timing exceeds admission"));
        }
        let tempo = if self.constant.is_some() {
            0
        } else {
            timing
                .max_tempo_breaks_per_window(VOLUME_QUERY_FRAMES as u64)
                .map_err(|_| Invalid("clip volume tempo density is unavailable"))?
        };
        if self
            .source_boundaries
            .checked_add(tempo)
            .is_none_or(|n| n > MAX_VOLUME_BOUNDARIES_PER_WINDOW)
        {
            return Err(Invalid("clip volume timing exceeds density admission"));
        }
        let rebase = timing
            .rebase_boundary()
            .map_err(|_| Invalid("clip volume clock rebase is unavailable"))?;
        if rebase.is_some_and(|cut| frame < cut.frame) {
            return Err(Invalid("clip volume query precedes clock rebase"));
        }
        let last_frame = frame
            .checked_add(frames as u64 - 1)
            .ok_or(Invalid("clip volume query frame overflow"))?;
        let last_beat = latest_rounded_beat(timing, last_frame)?;
        let Some(mut domain) = self.domain(placement)? else {
            return Ok(None);
        };
        if let Some(cut) = rebase {
            domain.floor_global = domain.floor_global.max(cut.beat.raw());
        }
        // Historical origins/ends may be outside the replacement clock's
        // forward domain. Compare in global beats before mapping only the
        // surviving stable floor. Source phase still uses the original origin.
        if domain
            .end_global
            .is_some_and(|end| end <= domain.floor_global)
        {
            return Ok(None);
        }
        let active_last = domain
            .end_global
            .map_or(last_beat, |end| end.min(last_beat));
        if active_last >= domain.origin.performance_anchor.raw()
            && domain
                .origin
                .elapsed_at(BeatTime::from_raw(active_last))
                .is_none()
        {
            return Err(Invalid("clip volume window exceeds elapsed beat domain"));
        }
        let start_frame = mapped_frame(timing, domain.floor_global)?;
        if frame < start_frame {
            return Ok(None);
        }
        if let Some(end) = domain.end_global
            && frame >= mapped_frame(timing, end)?
        {
            return Ok(None);
        }
        let latest = latest_rounded_beat(timing, frame)?;
        let neighbors = self.neighbors(timing, domain, latest)?;
        let begin_frame = mapped_frame(timing, neighbors.previous)?;
        if begin_frame > frame {
            return Err(Invalid("clip volume boundary follows query"));
        }
        let start_gain = self.value_at(domain, neighbors.previous, false)?;
        let Some(next) = neighbors.next else {
            if !neighbors.constant {
                return Err(Invalid("clip volume ramp exceeds beat domain"));
            }
            return Ok(Some(VolumeRamp {
                start_frame: begin_frame,
                end_frame: u64::MAX,
                start_gain,
                end_gain: start_gain,
                next_boundary: None,
            }));
        };
        let next_frame = mapped_frame(timing, next)?;
        if next_frame <= frame {
            return Err(Invalid("clip volume boundary makes no frame progress"));
        }
        // Last rounded boundary wins at the next frame too. Merely taking the
        // first future authored knot changes the preceding frame-ramp endpoint.
        let next_latest = latest_rounded_beat(timing, next_frame)?;
        let end = if let Some(end) = domain.end_global.filter(|end| *end <= next_latest) {
            end
        } else {
            self.neighbors(timing, domain, next_latest)?.previous
        };
        if mapped_frame(timing, end)? != next_frame {
            return Err(Invalid("clip volume rounded boundary is inconsistent"));
        }
        let end_gain = self.value_at(domain, end, true)?;
        Ok(Some(VolumeRamp {
            start_frame: begin_frame,
            end_frame: next_frame,
            start_gain,
            end_gain,
            next_boundary: Some(next_frame),
        }))
    }
    fn domain(&self, placement: VolumePlacement) -> Result<Option<Domain>, Invalid> {
        let (origin, finite) = match placement {
            VolumePlacement::Arrangement { start } => (
                SessionMidiOrigin::launch(start),
                Some(self.source.arrangement_duration().raw()),
            ),
            VolumePlacement::Session { origin } => (origin, None),
        };
        let mut floor_elapsed = origin.elapsed_at_anchor.raw();
        let source_end = if self.reversed {
            let upper = self
                .source_offset
                .checked_add(self.source.arrangement_duration().raw())
                .ok_or(Invalid("clip volume reverse window overflow"))?;
            if self.source.loop_range().is_none() {
                let end = self.source.initial_end().raw() - self.source.source_start().raw();
                if self.source_offset >= end {
                    return Ok(None);
                }
                floor_elapsed = floor_elapsed.max(upper.saturating_sub(end));
            }
            Some(self.source.arrangement_duration().raw())
        } else if self.source.loop_range().is_none() {
            let end = self.source.initial_end().raw() - self.source.source_start().raw();
            let Some(end) = end.checked_sub(self.source_offset) else {
                return Ok(None);
            };
            Some(end)
        } else {
            None
        };
        let end_elapsed = match (finite, source_end) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        // A one-shot cue at/after its source end is silent, not a wrapped
        // source or an invented negative projection before the cue floor.
        if end_elapsed.is_some_and(|end| end <= origin.elapsed_at_anchor.raw()) {
            return Ok(None);
        }
        let end_global = end_elapsed.map(|end| project(origin, end)).transpose()?;
        Ok(Some(Domain {
            origin,
            floor_global: project(origin, floor_elapsed)?,
            end_elapsed,
            end_global,
        }))
    }
    fn neighbors<T: MidiTempoBreaks + ?Sized>(
        &self,
        timing: &T,
        domain: Domain,
        global: u64,
    ) -> Result<Neighbors, Invalid> {
        let elapsed = domain
            .origin
            .elapsed_at(BeatTime::from_raw(global))
            .ok_or(Invalid("clip volume elapsed beat exceeds domain"))?
            .raw();
        if domain.end_elapsed.is_some_and(|end| elapsed >= end) {
            return Err(Invalid("clip volume query follows source end"));
        }
        let (previous, next, constant) = if self.constant.is_some() {
            (
                domain.origin.elapsed_at_anchor.raw(),
                domain.end_elapsed,
                true,
            )
        } else {
            self.source_neighbors(domain, elapsed)?
        };
        let mut previous = project(domain.origin, previous)?.max(domain.floor_global);
        let mut next = next.map(|raw| project(domain.origin, raw)).transpose()?;
        // Raw local equality is insufficient: several future authored knots
        // may collapse to one frame and change this ramp's incoming endpoint.
        // Preserve tempo knots for every globally varying prepared curve.
        if self.constant.is_none() {
            let breaks = timing
                .tempo_breaks_around(BeatTime::from_raw(global))
                .map_err(|_| Invalid("clip volume tempo boundaries are unavailable"))?;
            if let Some(marker) = breaks.previous {
                previous = previous.max(marker.raw());
            }
            if let Some(marker) = breaks.next {
                next = Some(next.map_or(marker.raw(), |raw| raw.min(marker.raw())));
            }
        }
        Ok(Neighbors {
            previous,
            next,
            constant,
        })
    }
    fn raw_elapsed(&self, elapsed: u64) -> Result<u64, Invalid> {
        let local = if self.reversed {
            self.source
                .arrangement_duration()
                .raw()
                .checked_sub(elapsed)
                .ok_or(Invalid("clip volume reverse query exceeds window"))?
        } else {
            elapsed
        };
        self.source_offset
            .checked_add(local)
            .ok_or(Invalid("clip volume source window overflow"))
    }
    fn source_neighbors(
        &self,
        domain: Domain,
        elapsed: u64,
    ) -> Result<(u64, Option<u64>, bool), Invalid> {
        let raw = self.raw_elapsed(elapsed)?;
        // Select the left source interval for a reversed right-continuous
        // destination query. Endpoints themselves remain continuous boundaries;
        // no one-tick shift is introduced into the gain/ramp coordinates.
        let probe = if self.reversed {
            raw.checked_sub(1)
                .ok_or(Invalid("clip volume reverse source exhausted"))?
        } else {
            raw
        };
        let phase = self
            .source
            .source_phase_at_elapsed(BeatTime::from_raw(probe))
            .ok_or(Invalid("clip volume source phase is unavailable"))?;
        let (cycle_elapsed, cycle_source, cycle_end) =
            if let Some((start, end)) = self.source.loop_range() {
                let first = end.raw() - self.source.source_start().raw();
                if probe < first {
                    (0, self.source.source_start().raw(), end.raw())
                } else {
                    let period = end.raw() - start.raw();
                    (
                        first + ((probe - first) / period) * period,
                        start.raw(),
                        end.raw(),
                    )
                }
            } else {
                (
                    0,
                    self.source.source_start().raw(),
                    self.source.initial_end().raw(),
                )
            };
        let points = self.volume.points();
        let index = points.partition_point(|point| point.beat <= phase);
        let lower = index
            .checked_sub(1)
            .and_then(|i| points.get(i))
            .filter(|p| p.beat.raw() >= cycle_source)
            .map_or(cycle_elapsed, |p| {
                cycle_elapsed + (p.beat.raw() - cycle_source)
            });
        let next_phase = points
            .get(index)
            .filter(|p| p.beat.raw() < cycle_end)
            .map_or(cycle_end, |p| p.beat.raw());
        let upper = cycle_elapsed.checked_add(next_phase - cycle_source);
        let (previous, next) = if self.reversed {
            let end = self
                .source_offset
                .checked_add(self.source.arrangement_duration().raw())
                .ok_or(Invalid("clip volume reverse window overflow"))?;
            let previous = upper.map_or(0, |upper| end.saturating_sub(upper));
            let next = end.checked_sub(lower.max(self.source_offset));
            (previous, next)
        } else {
            (
                lower.saturating_sub(self.source_offset),
                upper.and_then(|upper| upper.checked_sub(self.source_offset)),
            )
        };
        let previous = previous.max(domain.origin.elapsed_at_anchor.raw());
        let next = match (next, domain.end_elapsed) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let left = self.value_at_elapsed(previous, false)?;
        let right = if let Some(next) = next {
            self.value_at_elapsed(next, true)?
        } else {
            self.volume
                .gain_before(BeatTime::from_raw(next_phase))
                .linear()
        };
        Ok((previous, next, left == right))
    }
    fn value_at(&self, domain: Domain, global: u64, before: bool) -> Result<f64, Invalid> {
        let elapsed = domain
            .origin
            .elapsed_at(BeatTime::from_raw(global))
            .ok_or(Invalid("clip volume elapsed beat exceeds domain"))?;
        self.value_at_elapsed(elapsed.raw(), before)
    }
    fn value_at_elapsed(&self, elapsed: u64, before: bool) -> Result<f64, Invalid> {
        if let Some(value) = self.constant {
            return Ok(value);
        }
        let raw = self.raw_elapsed(elapsed)?;
        let before = before != self.reversed;
        let phase = if before {
            if let Some((start, end)) = self.source.loop_range() {
                let first = end.raw() - self.source.source_start().raw();
                if raw >= first && (raw - first).is_multiple_of(end.raw() - start.raw()) {
                    Some(end)
                } else {
                    self.source.source_phase_at_elapsed(BeatTime::from_raw(raw))
                }
            } else if raw == self.source.initial_end().raw() - self.source.source_start().raw() {
                Some(self.source.initial_end())
            } else {
                self.source.source_phase_at_elapsed(BeatTime::from_raw(raw))
            }
        } else {
            self.source.source_phase_at_elapsed(BeatTime::from_raw(raw))
        };
        let phase = phase.ok_or(Invalid("clip volume source phase is unavailable"))?;
        Ok(if before {
            self.volume.gain_before(phase)
        } else {
            self.volume.gain_at(phase)
        }
        .linear()
            * self.clip_gain)
    }
}
fn project(origin: SessionMidiOrigin, elapsed: u64) -> Result<u64, Invalid> {
    origin
        .project(u128::from(elapsed))
        .map_err(|_| Invalid("clip volume projection exceeds beat domain"))?
        .ok_or(Invalid("clip volume boundary precedes cue floor"))
}
fn mapped_frame<T: MidiTempoBreaks + ?Sized>(timing: &T, raw: u64) -> Result<u64, Invalid> {
    timing
        .beat_to_frame(BeatTime::from_raw(raw))
        .map_err(|_| Invalid("clip volume frame mapping is unavailable"))
}
fn latest_rounded_beat<T: MidiTempoBreaks + ?Sized>(
    timing: &T,
    frame: u64,
) -> Result<u64, Invalid> {
    let next = frame
        .checked_add(1)
        .ok_or(Invalid("clip volume frame domain exhausted"))?;
    let lower = timing
        .beat_lower_bound_for_frame(frame)
        .map_err(|_| Invalid("clip volume beat bucket is unavailable"))?;
    if lower > u128::from(u64::MAX) {
        return Err(Invalid("clip volume frame exceeds beat domain"));
    }
    let upper = timing
        .beat_lower_bound_for_frame(next)
        .map_err(|_| Invalid("clip volume beat bucket is unavailable"))?;
    if upper <= lower {
        return Err(Invalid(
            "clip volume frame has no representable beat bucket",
        ));
    }
    upper
        .checked_sub(1)
        .and_then(|raw| u64::try_from(raw).ok())
        .ok_or(Invalid(
            "clip volume frame has no representable beat bucket",
        ))
}

#[cfg(test)]
mod tests;
