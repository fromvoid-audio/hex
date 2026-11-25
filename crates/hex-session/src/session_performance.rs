//! Control-side admission and atomic Arrangement commit for a captured Session
//! performance (Pulse #3456).
//!
//! # Ownership and contract
//! - Delivery observations (`DeliveredRouteTransition`) originate strictly from
//!   confirmed callback-applied route transitions (`hex-engine` + `hex-audio`
//!   `#3455`), never UI intent or command enqueue timestamps.
//! - `SessionPerformanceBinding` freezes the canonical project history revision,
//!   sample rate, tempo map, active Arrangement loop region, affected track IDs,
//!   initial applied route owners, and referenced Session MIDI/audio source
//!   identities before capture begins.
//! - `SessionPerformanceCapture` validates every delivered batch against the
//!   frozen output/revision/generation/transport-epoch envelope, rejects stale
//!   observations, overflow, cancellation, and non-monotonic timing or seek
//!   discontinuities, and bounds retained transitions to
//!   `MAX_SESSION_PERFORMANCE_EVENTS`.
//! - `Session::commit_session_performance` derives validated `hex-project`
//!   Arrangement edits (`PlaceMidiClipWithNotes`, `PlaceClip`, `DeleteTimeRange`)
//!   preserving original audio `AssetId` custody and MIDI note timing across
//!   Session clip loops, then prepares and commits the complete batch in one
//!   atomic Undo/Redo step via `Editor::execute_many_prepared`. Any validation
//!   or preparation refusal leaves the canonical project and history untouched.
//!
//! # Arrangement overlap, loop, quantization, and cancellation rules
//! - **Overlap**: Session MIDI/audio clip placements overwrite overlapped
//!   Arrangement clips on the same track (`PlaceMidiClipWithNotes` / `PlaceClip`);
//!   `Silence` (`StopTrack` / `StopAllClips`) clears overlapped Arrangement clips
//!   via `DeleteTimeRange`; `Arrangement` (`BackToArrangement`) leaves existing
//!   Arrangement clips untouched.
//! - **Same-block events**: Multiple transitions on one track at the same
//!   performance frame keep FIFO delivery order; zero-duration segments are
//!   superseded without emitting empty clips.
//! - **Arrangement loop**: When an Arrangement loop region is active, each
//!   performance segment is mapped onto its wrapped `[content_start, content_end)`
//!   window (split across loop wraps), so later passes cleanly overwrite earlier
//!   passes on the looped range without producing out-of-range clips.
//! - **Quantization**: Placement timing comes strictly from the delivered
//!   transition frames/beats applied by `SessionPlayback`; no second scheduler
//!   or re-quantization runs here.
//! - **Cancellation / overflow / stale project**: Any cancelled, overflowed,
//!   unfinished, or stale-revision capture refuses atomically with zero mutation.

use crate::{Session, SessionError, prepare_with_waveforms};
use hex_engine::{AppliedSessionRouteTarget, AppliedSessionRouteTransition, SessionMidiOrigin};
use hex_project::{
    AssetId, BeatTime, ClipId, Command, Created, LoopRegion, MidiNoteInsert, MidiSourceIdentity,
    Project, SessionAudioLoop, TempoMap, TimeRange, TrackId,
};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Maximum number of delivered route transitions admitted in one finite
/// Session performance capture.
pub const MAX_SESSION_PERFORMANCE_EVENTS: usize = 4_096;

/// Maximum number of notes projected into one Arrangement MIDI clip segment.
const MAX_PROJECTED_CLIP_NOTES: usize = 16_384;

/// Callback-delivered Session route transition stamped with its owning audio
/// output, installed revision, control generation, and transport epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeliveredRouteTransition {
    pub output_identity: u64,
    pub audio_revision: u64,
    pub control_generation: u64,
    pub transport_epoch: u64,
    pub transition: AppliedSessionRouteTransition,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionPerformanceError {
    InvalidBinding(&'static str),
    StaleObservation,
    Overflow,
    Cancelled,
    NotFinished,
    AlreadyFinished,
    TimingDiscontinuity,
    StaleProject,
    UnsupportedSource(&'static str),
}

impl std::fmt::Display for SessionPerformanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidBinding(msg) => write!(f, "invalid session performance binding: {msg}"),
            Self::StaleObservation => {
                write!(
                    f,
                    "delivered route transition does not match active output/revision"
                )
            }
            Self::Overflow => write!(f, "session performance route observation buffer overflowed"),
            Self::Cancelled => write!(f, "session performance capture was cancelled"),
            Self::NotFinished => write!(f, "session performance capture has not finished"),
            Self::AlreadyFinished => write!(f, "session performance capture is already finished"),
            Self::TimingDiscontinuity => {
                write!(
                    f,
                    "session performance encountered a seek or non-monotonic clock"
                )
            }
            Self::StaleProject => {
                write!(
                    f,
                    "canonical project changed before session performance commit"
                )
            }
            Self::UnsupportedSource(msg) => write!(f, "unsupported session clip source: {msg}"),
        }
    }
}

impl std::error::Error for SessionPerformanceError {}

#[derive(Clone, Debug, PartialEq, Eq)]
enum FrozenSessionSource {
    Midi {
        track: TrackId,
        source_identity: MidiSourceIdentity,
        active: bool,
        playback_offset: BeatTime,
    },
    Audio {
        track: TrackId,
        asset: AssetId,
        source_start: u64,
        source_end: u64,
        loop_range: Option<SessionAudioLoop>,
        active: bool,
    },
}

/// Frozen canonical and audio-delivery scope for one finite Session
/// performance capture.
#[derive(Clone, Debug)]
pub struct SessionPerformanceBinding {
    output_identity: u64,
    audio_revision: u64,
    control_generation: u64,
    transport_epoch: u64,
    history_revision: u64,
    sample_rate: u32,
    tempo: TempoMap,
    loop_region: Option<LoopRegion>,
    tracks: Vec<TrackId>,
    start_frame: u64,
    initial_routes: BTreeMap<TrackId, AppliedSessionRouteTarget>,
    frozen_sources: BTreeMap<ClipId, FrozenSessionSource>,
}

impl SessionPerformanceBinding {
    /// Freeze the current canonical project revision, affected track IDs,
    /// initial route owners, and referenced Session clip sources.
    #[allow(clippy::too_many_arguments)]
    pub fn capture(
        session: &Session,
        output_identity: u64,
        audio_revision: u64,
        control_generation: u64,
        transport_epoch: u64,
        tracks: &[TrackId],
        start_frame: u64,
        initial_routes: &[(TrackId, AppliedSessionRouteTarget)],
    ) -> Result<Self, SessionPerformanceError> {
        if output_identity == 0 || audio_revision == 0 || transport_epoch == 0 {
            return Err(SessionPerformanceError::InvalidBinding(
                "output identity, audio revision, and transport epoch must be nonzero",
            ));
        }
        if tracks.is_empty() {
            return Err(SessionPerformanceError::InvalidBinding(
                "at least one track is required",
            ));
        }
        let project = session.project();
        let loop_region = project.loop_region().filter(|region| region.enabled());
        if let Some(region) = loop_region
            && start_frame >= region.end()
        {
            return Err(SessionPerformanceError::InvalidBinding(
                "start frame is past enabled loop end",
            ));
        }

        let mut canonical_tracks = Vec::with_capacity(tracks.len());
        for &track in tracks {
            if project.track(track).is_none() {
                return Err(SessionPerformanceError::InvalidBinding(
                    "affected track no longer exists",
                ));
            }
            if canonical_tracks.contains(&track) {
                return Err(SessionPerformanceError::InvalidBinding(
                    "duplicate track id in binding",
                ));
            }
            canonical_tracks.push(track);
        }

        let mut initial = BTreeMap::new();
        for &track in &canonical_tracks {
            initial.insert(track, AppliedSessionRouteTarget::Arrangement);
        }
        for &(track, owner) in initial_routes {
            if let Some(slot) = initial.get_mut(&track) {
                *slot = owner;
            }
        }

        let mut frozen_sources = BTreeMap::new();
        for cell in project.session_cells() {
            if !canonical_tracks.contains(&cell.track()) {
                continue;
            }
            match cell.content() {
                hex_project::SessionCellContent::Stop => {}
                hex_project::SessionCellContent::Midi(clip) => {
                    frozen_sources.insert(
                        clip.id(),
                        FrozenSessionSource::Midi {
                            track: cell.track(),
                            source_identity: clip.source_identity(),
                            active: clip.active(),
                            playback_offset: clip.playback_offset(),
                        },
                    );
                }
                hex_project::SessionCellContent::Audio(clip) => {
                    if project.asset(clip.asset()).is_none() {
                        return Err(SessionPerformanceError::UnsupportedSource(
                            "session audio clip asset is missing",
                        ));
                    }
                    if project.asset_offline(clip.asset()) {
                        return Err(SessionPerformanceError::UnsupportedSource(
                            "session audio clip asset is offline",
                        ));
                    }
                    frozen_sources.insert(
                        clip.id(),
                        FrozenSessionSource::Audio {
                            track: cell.track(),
                            asset: clip.asset(),
                            source_start: clip.source_start(),
                            source_end: clip.source_end(),
                            loop_range: clip.loop_range(),
                            active: clip.active(),
                        },
                    );
                }
            }
        }

        Ok(Self {
            output_identity,
            audio_revision,
            control_generation,
            transport_epoch,
            history_revision: session.history_revision(),
            sample_rate: project.sample_rate(),
            tempo: project.tempo_map().clone(),
            loop_region,
            tracks: canonical_tracks,
            start_frame,
            initial_routes: initial,
            frozen_sources,
        })
    }

    pub fn history_revision(&self) -> u64 {
        self.history_revision
    }

    pub fn start_frame(&self) -> u64 {
        self.start_frame
    }

    pub fn tracks(&self) -> &[TrackId] {
        &self.tracks
    }

    fn validate_against(&self, session: &Session) -> Result<(), SessionPerformanceError> {
        if session.history_revision() != self.history_revision {
            return Err(SessionPerformanceError::StaleProject);
        }
        let project = session.project();
        if project.sample_rate() != self.sample_rate
            || project.tempo_map() != &self.tempo
            || project.loop_region().filter(|r| r.enabled()) != self.loop_region
        {
            return Err(SessionPerformanceError::StaleProject);
        }
        for &track in &self.tracks {
            if project.track(track).is_none() {
                return Err(SessionPerformanceError::StaleProject);
            }
        }
        for (&clip_id, frozen) in &self.frozen_sources {
            match frozen {
                FrozenSessionSource::Midi {
                    track,
                    source_identity,
                    active,
                    playback_offset,
                } => {
                    let Some(clip) = project.session_midi_clip(clip_id) else {
                        return Err(SessionPerformanceError::StaleProject);
                    };
                    let owner_matches = project
                        .session_cells()
                        .iter()
                        .any(|c| c.track() == *track && matches!(c.content(), hex_project::SessionCellContent::Midi(m) if m.id() == clip_id));
                    if !owner_matches
                        || clip.source_identity() != *source_identity
                        || clip.active() != *active
                        || clip.playback_offset() != *playback_offset
                    {
                        return Err(SessionPerformanceError::StaleProject);
                    }
                }
                FrozenSessionSource::Audio {
                    track,
                    asset,
                    source_start,
                    source_end,
                    loop_range,
                    active,
                } => {
                    let Some(clip) = project.session_audio_clip(clip_id) else {
                        return Err(SessionPerformanceError::StaleProject);
                    };
                    let owner_matches = project
                        .session_cells()
                        .iter()
                        .any(|c| c.track() == *track && matches!(c.content(), hex_project::SessionCellContent::Audio(a) if a.id() == clip_id));
                    let asset_online =
                        project.asset(*asset).is_some() && !project.asset_offline(*asset);
                    if !owner_matches
                        || !asset_online
                        || clip.asset() != *asset
                        || clip.source_start() != *source_start
                        || clip.source_end() != *source_end
                        || clip.loop_range() != *loop_range
                        || clip.active() != *active
                    {
                        return Err(SessionPerformanceError::StaleProject);
                    }
                }
            }
        }
        Ok(())
    }
}

/// Bounded accumulator for delivery-confirmed Session route transitions over
/// one finite recording interval.
#[derive(Clone, Debug)]
pub struct SessionPerformanceCapture {
    binding: SessionPerformanceBinding,
    events: Vec<AppliedSessionRouteTransition>,
    last_performance_frame: Option<u64>,
    last_content_frame: Option<u64>,
    last_loop_pass: Option<u64>,
    overflowed: bool,
    cancelled: bool,
    finished_end_frame: Option<u64>,
}

impl SessionPerformanceCapture {
    pub fn new(binding: SessionPerformanceBinding) -> Self {
        Self {
            binding,
            events: Vec::new(),
            last_performance_frame: None,
            last_content_frame: None,
            last_loop_pass: None,
            overflowed: false,
            cancelled: false,
            finished_end_frame: None,
        }
    }

    pub fn binding(&self) -> &SessionPerformanceBinding {
        &self.binding
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled
    }

    pub fn is_overflowed(&self) -> bool {
        self.overflowed
    }

    /// Record a batch of callback-delivered route transitions. Surfaces
    /// overflow, stale observations, and timing discontinuities explicitly.
    pub fn record_batch(
        &mut self,
        observations: &[DeliveredRouteTransition],
        batch_overflowed: bool,
    ) -> Result<(), SessionPerformanceError> {
        if self.cancelled {
            return Err(SessionPerformanceError::Cancelled);
        }
        if self.finished_end_frame.is_some() {
            return Err(SessionPerformanceError::AlreadyFinished);
        }
        if batch_overflowed {
            self.overflowed = true;
            return Err(SessionPerformanceError::Overflow);
        }
        for &obs in observations {
            if obs.output_identity != self.binding.output_identity
                || obs.audio_revision != self.binding.audio_revision
                || obs.control_generation != self.binding.control_generation
                || obs.transport_epoch != self.binding.transport_epoch
            {
                return Err(SessionPerformanceError::StaleObservation);
            }
            let event = obs.transition;
            if event.performance_frame < self.binding.start_frame {
                return Err(SessionPerformanceError::TimingDiscontinuity);
            }
            if self
                .last_performance_frame
                .is_some_and(|prev| event.performance_frame < prev)
            {
                return Err(SessionPerformanceError::TimingDiscontinuity);
            }
            if let Some(region) = self.binding.loop_region {
                if self
                    .last_loop_pass
                    .is_some_and(|prev| event.loop_pass < prev)
                {
                    return Err(SessionPerformanceError::TimingDiscontinuity);
                }
                if event.loop_pass > 0
                    && (event.content_frame < region.start() || event.content_frame >= region.end())
                {
                    return Err(SessionPerformanceError::TimingDiscontinuity);
                }
            } else {
                // Linear mode: content frame must advance in lockstep with performance frame
                // (no mid-recording seek jumps).
                if let (Some(prev_perf), Some(prev_content)) =
                    (self.last_performance_frame, self.last_content_frame)
                {
                    let delta = event.performance_frame - prev_perf;
                    if prev_content.checked_add(delta) != Some(event.content_frame) {
                        return Err(SessionPerformanceError::TimingDiscontinuity);
                    }
                } else if event.content_frame < self.binding.start_frame {
                    return Err(SessionPerformanceError::TimingDiscontinuity);
                }
            }
            self.last_performance_frame = Some(event.performance_frame);
            self.last_content_frame = Some(event.content_frame);
            self.last_loop_pass = Some(event.loop_pass);

            if !self.binding.tracks.contains(&event.track) {
                continue;
            }
            if self.events.len() >= MAX_SESSION_PERFORMANCE_EVENTS {
                self.overflowed = true;
                return Err(SessionPerformanceError::Overflow);
            }
            self.events.push(event);
        }
        Ok(())
    }

    /// Cancel capture so a subsequent commit refuses without mutating the project.
    pub fn cancel(&mut self) {
        self.cancelled = true;
    }

    /// Freeze the finite recording interval `[start_frame, end_frame)`.
    pub fn finish(&mut self, end_frame: u64) -> Result<(), SessionPerformanceError> {
        if self.cancelled {
            return Err(SessionPerformanceError::Cancelled);
        }
        if self.overflowed {
            return Err(SessionPerformanceError::Overflow);
        }
        if self.finished_end_frame.is_some() {
            return Err(SessionPerformanceError::AlreadyFinished);
        }
        if end_frame <= self.binding.start_frame {
            return Err(SessionPerformanceError::TimingDiscontinuity);
        }
        if self
            .last_performance_frame
            .is_some_and(|last| end_frame < last)
        {
            return Err(SessionPerformanceError::TimingDiscontinuity);
        }
        self.finished_end_frame = Some(end_frame);
        Ok(())
    }
}

/// Result of committing a captured Session performance into Arrangement.
#[derive(Debug)]
pub struct SessionPerformanceCommit {
    pub commands_applied: usize,
    pub placed_clips: Vec<ClipId>,
    pub prepared: Option<Arc<hex_engine::PreparedRevision>>,
}

#[derive(Clone, Copy, Debug)]
struct TrackSpan {
    perf_start: u64,
    perf_end: u64,
    owner: AppliedSessionRouteTarget,
}

fn build_track_spans(
    start_frame: u64,
    end_frame: u64,
    initial: AppliedSessionRouteTarget,
    events: &[AppliedSessionRouteTransition],
) -> Vec<TrackSpan> {
    let mut spans = Vec::new();
    let mut current_start = start_frame;
    let mut current_owner = initial;
    for &event in events {
        let at = event.performance_frame.clamp(start_frame, end_frame);
        if at > current_start {
            spans.push(TrackSpan {
                perf_start: current_start,
                perf_end: at,
                owner: current_owner,
            });
            current_start = at;
        }
        // Same-frame transitions supersede zero-duration intervals in FIFO order.
        current_owner = event.target;
    }
    if end_frame > current_start {
        spans.push(TrackSpan {
            perf_start: current_start,
            perf_end: end_frame,
            owner: current_owner,
        });
    }
    spans
}

/// Split one performance interval `[perf_start, perf_end)` into linear
/// Arrangement content-frame segments `(arr_start, arr_end, seg_perf_start)`.
fn project_to_arrangement_segments(
    perf_origin: u64,
    perf_start: u64,
    perf_end: u64,
    loop_region: Option<LoopRegion>,
) -> Result<Vec<(u64, u64, u64)>, SessionPerformanceError> {
    if perf_end <= perf_start {
        return Ok(Vec::new());
    }
    let Some(region) = loop_region else {
        return Ok(vec![(perf_start, perf_end, perf_start)]);
    };
    let loop_start = region.start();
    let loop_end = region.end();
    let loop_len = loop_end
        .checked_sub(loop_start)
        .filter(|len| *len > 0)
        .ok_or(SessionPerformanceError::TimingDiscontinuity)?;

    let mut segments = Vec::new();
    let mut cursor = perf_start;
    let mut content = if perf_origin < loop_end {
        let elapsed = perf_start
            .checked_sub(perf_origin)
            .ok_or(SessionPerformanceError::TimingDiscontinuity)?;
        let raw = perf_origin
            .checked_add(elapsed)
            .ok_or(SessionPerformanceError::TimingDiscontinuity)?;
        if raw < loop_end {
            raw
        } else {
            loop_start + ((raw - loop_end) % loop_len)
        }
    } else {
        perf_start
    };
    while cursor < perf_end {
        let remaining = perf_end - cursor;
        let to_wrap = if content < loop_end {
            loop_end - content
        } else {
            remaining
        };
        let step = remaining.min(to_wrap);
        segments.push((content, content + step, cursor));
        cursor += step;
        content += step;
        if content == loop_end && cursor < perf_end {
            content = loop_start;
        }
    }
    Ok(segments)
}

/// Project notes from a Session MIDI clip onto a clip-local Arrangement span
/// `[0, clip_length)` starting at `elapsed_start` beats after clip launch.
fn project_session_midi_notes(
    clip: &hex_project::SessionMidiClip,
    elapsed_start: BeatTime,
    clip_length: BeatTime,
) -> Result<Vec<MidiNoteInsert>, SessionPerformanceError> {
    if !clip.active() || clip_length.raw() == 0 {
        return Ok(Vec::new());
    }
    let source = clip.source();
    let source_start = source.source_start().raw();
    let effective_end = source.effective_end().raw();
    let looping = source.looping();
    let loop_start = source.loop_start().raw();
    let loop_end = source.loop_end().raw();
    let base_offset = clip
        .playback_offset()
        .raw()
        .checked_add(elapsed_start.raw())
        .ok_or(SessionPerformanceError::TimingDiscontinuity)?;
    let span_end = base_offset
        .checked_add(clip_length.raw())
        .ok_or(SessionPerformanceError::TimingDiscontinuity)?;

    let mut projected = Vec::new();
    if !looping {
        let window_start = source_start
            .checked_add(base_offset)
            .ok_or(SessionPerformanceError::TimingDiscontinuity)?;
        let window_end = source_start
            .checked_add(span_end)
            .ok_or(SessionPerformanceError::TimingDiscontinuity)?
            .min(effective_end);
        for note in source.notes() {
            if !note.active() {
                continue;
            }
            let n_start = note.start().raw();
            if n_start < window_start || n_start >= window_end {
                continue;
            }
            let n_end = n_start
                .checked_add(note.duration().raw())
                .ok_or(SessionPerformanceError::TimingDiscontinuity)?
                .min(window_end);
            if n_end <= n_start {
                continue;
            }
            if projected.len() >= MAX_PROJECTED_CLIP_NOTES {
                return Err(SessionPerformanceError::Overflow);
            }
            projected.push(MidiNoteInsert {
                active: true,
                start: BeatTime::from_raw(n_start - window_start),
                duration: BeatTime::from_raw(n_end - n_start),
                pitch: note.pitch(),
                channel: note.channel(),
                velocity: note.velocity(),
            });
        }
        return Ok(projected);
    }

    let loop_len = loop_end
        .checked_sub(loop_start)
        .filter(|len| *len > 0)
        .ok_or(SessionPerformanceError::UnsupportedSource(
            "session midi clip loop has zero length",
        ))?;
    let first_cycle_len = loop_end
        .checked_sub(source_start)
        .filter(|len| *len > 0)
        .ok_or(SessionPerformanceError::UnsupportedSource(
            "session midi clip start is past loop end",
        ))?;

    // Walk elapsed beats [base_offset, span_end) cycle by cycle.
    let mut elapsed = base_offset;
    while elapsed < span_end {
        let (cycle_source_start, cycle_source_end) = if elapsed < first_cycle_len {
            let s = source_start + elapsed;
            let step = (span_end - elapsed).min(loop_end - s);
            (s, s + step)
        } else {
            let into_loop = (elapsed - first_cycle_len) % loop_len;
            let s = loop_start + into_loop;
            let step = (span_end - elapsed).min(loop_end - s);
            (s, s + step)
        };
        let seg_len = cycle_source_end - cycle_source_start;
        let dest_base = elapsed - base_offset;
        for note in source.notes() {
            if !note.active() {
                continue;
            }
            let n_start = note.start().raw();
            if n_start < cycle_source_start || n_start >= cycle_source_end {
                continue;
            }
            let n_end = n_start
                .checked_add(note.duration().raw())
                .ok_or(SessionPerformanceError::TimingDiscontinuity)?
                .min(loop_end);
            let local_start = dest_base + (n_start - cycle_source_start);
            let local_end = (dest_base + (n_end - cycle_source_start)).min(clip_length.raw());
            if local_end <= local_start {
                continue;
            }
            if projected.len() >= MAX_PROJECTED_CLIP_NOTES {
                return Err(SessionPerformanceError::Overflow);
            }
            projected.push(MidiNoteInsert {
                active: true,
                start: BeatTime::from_raw(local_start),
                duration: BeatTime::from_raw(local_end - local_start),
                pitch: note.pitch(),
                channel: note.channel(),
                velocity: note.velocity(),
            });
        }
        elapsed += seg_len;
    }
    Ok(projected)
}

fn derive_commands(
    project: &Project,
    binding: &SessionPerformanceBinding,
    events: &[AppliedSessionRouteTransition],
    end_frame: u64,
) -> Result<Vec<Command>, SessionPerformanceError> {
    let mut commands = Vec::new();
    for &track in &binding.tracks {
        let initial = binding
            .initial_routes
            .get(&track)
            .copied()
            .unwrap_or(AppliedSessionRouteTarget::Arrangement);
        let track_events: Vec<_> = events
            .iter()
            .copied()
            .filter(|e| e.track == track)
            .collect();
        let spans = build_track_spans(binding.start_frame, end_frame, initial, &track_events);

        for span in spans {
            let segments = project_to_arrangement_segments(
                binding.start_frame,
                span.perf_start,
                span.perf_end,
                binding.loop_region,
            )?;
            for (arr_start, arr_end, seg_perf_start) in segments {
                let duration = arr_end - arr_start;
                if duration == 0 {
                    continue;
                }
                match span.owner {
                    AppliedSessionRouteTarget::Arrangement => {
                        // Existing Arrangement clips remain untouched.
                    }
                    AppliedSessionRouteTarget::Silence => {
                        let range = TimeRange::new(vec![track], arr_start, arr_end)
                            .map_err(|_| SessionPerformanceError::TimingDiscontinuity)?;
                        commands.push(Command::DeleteTimeRange { range });
                    }
                    AppliedSessionRouteTarget::SessionMidi { clip, origin } => {
                        let session_clip = project.session_midi_clip(clip).ok_or(
                            SessionPerformanceError::UnsupportedSource(
                                "session midi clip no longer exists",
                            ),
                        )?;
                        let start_beat = binding
                            .tempo
                            .frame_to_beat(arr_start)
                            .map_err(|_| SessionPerformanceError::TimingDiscontinuity)?;
                        let end_beat = binding
                            .tempo
                            .frame_to_beat(arr_end)
                            .map_err(|_| SessionPerformanceError::TimingDiscontinuity)?;
                        let length_raw = end_beat
                            .raw()
                            .checked_sub(start_beat.raw())
                            .filter(|len| *len > 0)
                            .ok_or(SessionPerformanceError::TimingDiscontinuity)?;
                        let length = BeatTime::from_raw(length_raw);
                        let perf_beat = binding
                            .tempo
                            .frame_to_beat(seg_perf_start)
                            .map_err(|_| SessionPerformanceError::TimingDiscontinuity)?;
                        let elapsed_start = session_midi_elapsed(origin, perf_beat);
                        let notes =
                            project_session_midi_notes(session_clip, elapsed_start, length)?;
                        commands.push(Command::PlaceMidiClipWithNotes {
                            track,
                            start: start_beat,
                            length,
                            notes,
                        });
                    }
                    AppliedSessionRouteTarget::SessionAudio { clip, launch_frame } => {
                        let session_clip = project.session_audio_clip(clip).ok_or(
                            SessionPerformanceError::UnsupportedSource(
                                "session audio clip no longer exists",
                            ),
                        )?;
                        if !session_clip.active() {
                            let range = TimeRange::new(vec![track], arr_start, arr_end)
                                .map_err(|_| SessionPerformanceError::TimingDiscontinuity)?;
                            commands.push(Command::DeleteTimeRange { range });
                            continue;
                        }
                        let source_start = session_clip.source_start();
                        let source_end = session_clip.source_end();
                        let source_length = source_end
                            .checked_sub(source_start)
                            .filter(|len| *len > 0)
                            .ok_or(SessionPerformanceError::UnsupportedSource(
                                "session audio clip has zero source length",
                            ))?;
                        let elapsed = seg_perf_start.saturating_sub(launch_frame);
                        if let Some(loop_range) = session_clip.loop_range() {
                            let loop_len = loop_range.end().saturating_sub(loop_range.start());
                            if loop_len == 0 {
                                return Err(SessionPerformanceError::UnsupportedSource(
                                    "session audio loop has zero length",
                                ));
                            }
                            let first_len = loop_range.end().saturating_sub(source_start);
                            if elapsed < first_len && loop_range.start() != source_start {
                                let intro_rem = first_len - elapsed;
                                let intro_dur = duration.min(intro_rem);
                                commands.push(Command::PlaceClip {
                                    track,
                                    asset: session_clip.asset(),
                                    start: arr_start,
                                    duration: intro_dur,
                                    source_offset: source_start + elapsed,
                                    source_length,
                                    looping: false,
                                });
                                if intro_dur < duration {
                                    commands.push(Command::PlaceClip {
                                        track,
                                        asset: session_clip.asset(),
                                        start: arr_start + intro_dur,
                                        duration: duration - intro_dur,
                                        source_offset: loop_range.start(),
                                        source_length: loop_len,
                                        looping: true,
                                    });
                                }
                            } else {
                                let offset = if elapsed < first_len {
                                    source_start + elapsed
                                } else {
                                    loop_range.start() + ((elapsed - first_len) % loop_len)
                                };
                                commands.push(Command::PlaceClip {
                                    track,
                                    asset: session_clip.asset(),
                                    start: arr_start,
                                    duration,
                                    source_offset: offset,
                                    source_length: loop_len,
                                    looping: true,
                                });
                            }
                        } else {
                            let remaining =
                                source_end.saturating_sub(source_start.saturating_add(elapsed));
                            if remaining == 0 {
                                let range = TimeRange::new(vec![track], arr_start, arr_end)
                                    .map_err(|_| SessionPerformanceError::TimingDiscontinuity)?;
                                commands.push(Command::DeleteTimeRange { range });
                            } else {
                                let audible = duration.min(remaining);
                                commands.push(Command::PlaceClip {
                                    track,
                                    asset: session_clip.asset(),
                                    start: arr_start,
                                    duration: audible,
                                    source_offset: source_start + elapsed,
                                    source_length,
                                    looping: false,
                                });
                                if audible < duration {
                                    let range =
                                        TimeRange::new(vec![track], arr_start + audible, arr_end)
                                            .map_err(|_| {
                                            SessionPerformanceError::TimingDiscontinuity
                                        })?;
                                    commands.push(Command::DeleteTimeRange { range });
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(commands)
}

fn session_midi_elapsed(origin: SessionMidiOrigin, performance_beat: BeatTime) -> BeatTime {
    let delta = performance_beat
        .raw()
        .saturating_sub(origin.performance_anchor.raw());
    BeatTime::from_raw(origin.elapsed_at_anchor.raw().saturating_add(delta))
}

impl Session {
    /// Atomically commit a finished, delivery-confirmed Session performance
    /// capture into Arrangement in one Undo/Redo step with prepare-before-commit.
    ///
    /// Refuses stale/replaced projects, overflowed/cancelled captures, timing
    /// discontinuities, and unsupported/offline sources without mutating the
    /// canonical project or history.
    pub fn commit_session_performance(
        &mut self,
        capture: SessionPerformanceCapture,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<SessionPerformanceCommit, SessionError> {
        if capture.cancelled {
            return Err(SessionError::Invalid(
                "session performance capture was cancelled",
            ));
        }
        if capture.overflowed {
            return Err(SessionError::Invalid(
                "session performance capture overflowed",
            ));
        }
        let Some(end_frame) = capture.finished_end_frame else {
            return Err(SessionError::Invalid(
                "session performance capture has not finished",
            ));
        };
        capture
            .binding
            .validate_against(self)
            .map_err(map_perf_error)?;

        let commands =
            derive_commands(self.project(), &capture.binding, &capture.events, end_frame)
                .map_err(map_perf_error)?;
        if commands.is_empty() {
            return Ok(SessionPerformanceCommit {
                commands_applied: 0,
                placed_clips: Vec::new(),
                prepared: None,
            });
        }
        let commands_applied = commands.len();
        let media = self.media_bank();
        let sine_table = self.sine_table();
        let (created, prepared) =
            self.editor
                .execute_many_prepared(commands, |candidate, _results| {
                    prepare_with_waveforms(
                        candidate,
                        &media,
                        Arc::clone(&sine_table),
                        previous.map(|rev| rev.session().as_ref()),
                        &self.audio_transforms,
                        &self.audio_clip_waveforms,
                    )
                    .map(Arc::new)
                    .map_err(SessionError::from)
                })?;
        let placed_clips = created
            .into_iter()
            .filter_map(|item| match item {
                Created::Clip(id) => Some(id),
                _ => None,
            })
            .collect();
        Ok(SessionPerformanceCommit {
            commands_applied,
            placed_clips,
            prepared: Some(prepared),
        })
    }
}

fn map_perf_error(error: SessionPerformanceError) -> SessionError {
    match error {
        SessionPerformanceError::InvalidBinding(msg)
        | SessionPerformanceError::UnsupportedSource(msg) => SessionError::Invalid(msg),
        SessionPerformanceError::StaleObservation => {
            SessionError::Invalid("delivered route transition does not match active output")
        }
        SessionPerformanceError::Overflow => {
            SessionError::Invalid("session performance observation buffer overflowed")
        }
        SessionPerformanceError::Cancelled => {
            SessionError::Invalid("session performance capture was cancelled")
        }
        SessionPerformanceError::NotFinished => {
            SessionError::Invalid("session performance capture has not finished")
        }
        SessionPerformanceError::AlreadyFinished => {
            SessionError::Invalid("session performance capture is already finished")
        }
        SessionPerformanceError::TimingDiscontinuity => {
            SessionError::Invalid("session performance encountered a clock discontinuity")
        }
        SessionPerformanceError::StaleProject => {
            SessionError::Invalid("canonical project changed before session performance commit")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PreparedImport;
    use hex_engine::{
        LaunchQuantization, PreparedRevision, SessionPlayback, SessionPlaybackCommand,
    };
    use hex_project::{Command, Created};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    const RATE: u32 = 8_000;

    struct TempWavDir(PathBuf);

    impl TempWavDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let dir =
                std::env::temp_dir().join(format!("hex-session-perf-{}-{id}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn prepared_wav(&self, name: &str, frames: u32, sample: i16) -> PreparedImport {
            let path = self.0.join(name);
            let data_bytes = frames * 4;
            let mut bytes = Vec::with_capacity(44 + data_bytes as usize);
            bytes.extend_from_slice(b"RIFF");
            bytes.extend_from_slice(&(36 + data_bytes).to_le_bytes());
            bytes.extend_from_slice(b"WAVEfmt ");
            bytes.extend_from_slice(&16u32.to_le_bytes());
            bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
            bytes.extend_from_slice(&2u16.to_le_bytes()); // stereo
            bytes.extend_from_slice(&RATE.to_le_bytes());
            bytes.extend_from_slice(&(RATE * 4).to_le_bytes());
            bytes.extend_from_slice(&4u16.to_le_bytes());
            bytes.extend_from_slice(&16u16.to_le_bytes());
            bytes.extend_from_slice(b"data");
            bytes.extend_from_slice(&data_bytes.to_le_bytes());
            for _ in 0..frames {
                bytes.extend_from_slice(&sample.to_le_bytes());
                bytes.extend_from_slice(&sample.to_le_bytes());
            }
            std::fs::write(&path, bytes).unwrap();
            let decoded = hex_media::decode_wav_file(
                &path,
                hex_media::Limits {
                    max_file_bytes: 8 * 1024 * 1024,
                    max_frames: 1_000_000,
                },
            )
            .unwrap();
            PreparedImport::new(decoded).unwrap()
        }
    }

    impl Drop for TempWavDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn setup_two_track_session() -> (
        TempWavDir,
        Session,
        Arc<PreparedRevision>,
        TrackId,
        TrackId,
        AssetId,
        hex_project::SceneId,
        ClipId,
        ClipId,
    ) {
        let dir = TempWavDir::new();
        let mut session = Session::new(RATE, 32, 8_000_000).unwrap();
        let sine_state = hex_project::default_instrument_state().unwrap();
        let Created::Track(midi_track) = session
            .editor
            .execute(Command::InsertTrackWithDevice {
                name: "lead".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: sine_state,
            })
            .unwrap()
        else {
            panic!("midi track")
        };
        let Created::Track(audio_track) = session
            .editor
            .execute(Command::AddTrack {
                name: "drums".into(),
            })
            .unwrap()
        else {
            panic!("audio track")
        };

        // Pre-existing Arrangement MIDI clip on Track 1 spanning beats 0..4 (note at beat 3..4).
        let Created::Clip(arr_midi) = session
            .editor
            .execute(Command::PlaceMidiClip {
                track: midi_track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        else {
            panic!("arr midi")
        };
        session
            .editor
            .execute(Command::AddMidiNote {
                clip: arr_midi,
                start: BeatTime::from_quarters(3).unwrap(),
                duration: BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap();

        // Import resident audio into Track 2 Arrangement at frames 0..16000 (~0.1)
        // and create a distinct Session audio clip (~0.4, 4000 frames) on Scene 1.
        let arr_prepared = dir.prepared_wav("arr.wav", 16_000, 3_277);
        let (Created::Imported { .. }, rev_after_arr) = session
            .import_prepared_audio_clip_revision(arr_prepared, audio_track, 0, None)
            .unwrap()
        else {
            panic!("arr import")
        };

        let Created::Scene(scene_1) = session
            .editor
            .execute(Command::AddScene {
                name: "main".into(),
            })
            .unwrap()
        else {
            panic!("scene 1")
        };
        let Created::SessionClip(sess_midi) = session
            .editor
            .execute(Command::CreateSessionMidiClip {
                track: midi_track,
                scene: scene_1,
                name: "riff".into(),
                source_length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("sess midi")
        };
        session
            .editor
            .execute(Command::AddMidiNote {
                clip: sess_midi,
                start: BeatTime::ZERO,
                duration: BeatTime::from_raw(1 << 31),
                pitch: 72,
                channel: 0,
                velocity: 110,
            })
            .unwrap();

        let sess_prepared = dir.prepared_wav("sess.wav", 4_000, 13_107);
        let (Created::SessionClip(sess_audio), initial_rev) = session
            .import_session_audio_clip_revision(
                sess_prepared,
                audio_track,
                scene_1,
                Some(rev_after_arr.as_ref()),
            )
            .unwrap()
        else {
            panic!("sess audio import")
        };
        let sess_asset = session
            .project()
            .session_audio_clip(sess_audio)
            .unwrap()
            .asset();

        (
            dir,
            session,
            initial_rev,
            midi_track,
            audio_track,
            sess_asset,
            scene_1,
            sess_midi,
            sess_audio,
        )
    }

    #[test]
    fn two_track_session_performance_commits_with_original_sources_one_undo_save_reopen_and_matching_pcm()
     {
        let (
            _dir,
            mut session,
            initial_rev,
            midi_track,
            audio_track,
            sess_asset,
            scene_1,
            _sess_midi,
            _sess_audio,
        ) = setup_two_track_session();
        let before_commit = session.project().snapshot();

        // Prepare live SessionPlayback from the Session's actual canonical project & media.
        let mut live = SessionPlayback::new(Arc::clone(&initial_rev)).unwrap();
        let binding = SessionPerformanceBinding::capture(
            &session,
            1,
            10,
            1,
            1,
            &[midi_track, audio_track],
            0,
            &[
                (midi_track, AppliedSessionRouteTarget::Arrangement),
                (audio_track, AppliedSessionRouteTarget::Arrangement),
            ],
        )
        .unwrap();
        let mut capture = SessionPerformanceCapture::new(binding);

        let wrap = |events: Vec<AppliedSessionRouteTransition>| -> Vec<DeliveredRouteTransition> {
            events
                .into_iter()
                .map(|transition| DeliveredRouteTransition {
                    output_identity: 1,
                    audio_revision: 10,
                    control_generation: 1,
                    transport_epoch: 1,
                    transition,
                })
                .collect()
        };

        // Run a real 4-beat (16_000-frame) performance:
        // - Beat 0..1 (0..4000): both tracks play Arrangement (0.1 on audio track, silent on midi track)
        // - Beat 1 (frame 4000): quantized LaunchScene(scene_1) starts Session MIDI on track 1 and Session Audio (0.4) on track 2
        // - Beat 2 (frame 8000): quantized StopTrack(audio_track) silences track 2 (cutting over pre-existing Arrangement 0.1!)
        // - Beat 3 (frame 12000): quantized BackToArrangement returns track 1 to Arrangement (note at beat 3..4)
        live.apply(SessionPlaybackCommand::Play).unwrap();
        let mut live_pcm = vec![[0.0; 2]; 16_000];
        live.render(&mut live_pcm[0..500]).unwrap();
        live.apply(SessionPlaybackCommand::LaunchScene {
            scene: scene_1,
            quantization: LaunchQuantization::Quarter,
        })
        .unwrap();
        live.render(&mut live_pcm[500..4500]).unwrap();
        let mut drained = Vec::new();
        let ov = live.drain_applied_route_transitions(|e| drained.push(e));
        capture.record_batch(&wrap(drained), ov).unwrap();

        live.apply(SessionPlaybackCommand::StopTrack {
            track: audio_track,
            quantization: LaunchQuantization::Quarter,
        })
        .unwrap();
        live.render(&mut live_pcm[4500..8500]).unwrap();
        let mut drained = Vec::new();
        let ov = live.drain_applied_route_transitions(|e| drained.push(e));
        capture.record_batch(&wrap(drained), ov).unwrap();

        live.apply(SessionPlaybackCommand::BackToArrangement {
            quantization: LaunchQuantization::Quarter,
        })
        .unwrap();
        live.render(&mut live_pcm[8500..16_000]).unwrap();
        let mut drained = Vec::new();
        let ov = live.drain_applied_route_transitions(|e| drained.push(e));
        capture.record_batch(&wrap(drained), ov).unwrap();

        capture.finish(16_000).unwrap();

        let committed = session
            .commit_session_performance(capture, Some(initial_rev.as_ref()))
            .unwrap();
        assert!(committed.commands_applied >= 3);
        let prepared = committed.prepared.expect("prepared revision");

        // Verify Track 2 preserved original Session AssetId (no duplicate assets created!)
        assert_eq!(session.project().assets().count(), 2);
        let track_2 = session.project().track(audio_track).unwrap();
        let session_placed = track_2
            .clips()
            .iter()
            .find(|c| c.start() == 4_000)
            .expect("session audio clip placed at 4000");
        assert_eq!(session_placed.asset(), sess_asset);
        assert_eq!(session_placed.duration(), 4_000);
        // Track 2 frames 8000..12000 were silenced by StopTrack (no clip covering 8000..12000).
        assert!(
            !track_2
                .clips()
                .iter()
                .any(|c| c.start() < 12_000 && c.start() + c.duration() > 8_000)
        );
        // Track 2 frames 12000..16000 restored Arrangement (0.1) via BackToArrangement.
        assert!(
            track_2
                .clips()
                .iter()
                .any(|c| c.start() <= 12_000 && c.start() + c.duration() >= 16_000)
        );

        // Render committed Arrangement-only playback and verify audible PCM!
        let mut arr_playback = SessionPlayback::new(prepared).unwrap();
        arr_playback.apply(SessionPlaybackCommand::Play).unwrap();
        let mut arr_pcm = vec![[0.0; 2]; 16_000];
        arr_playback.render(&mut arr_pcm).unwrap();

        // Beat 0 (0..4000): audio 0.1
        assert!((arr_pcm[100][0] - 0.1).abs() < 1e-4);
        // Beat 1 (4000..8000): session MIDI (pitch 72) + session audio (0.4)
        assert!(arr_pcm[4_500][0].abs() > 0.3);
        // Beat 2 (8000..12000): session MIDI still playing on beat 2..3 (looped!), audio stopped -> no 0.1 or 0.4 DC offset at quiet tail
        assert!((arr_pcm[11_500][0]).abs() < 1e-4);
        // Beat 3 (12000..16000): BackToArrangement -> pre-existing Arrangement MIDI + Arrangement audio 0.1
        assert!(arr_pcm[12_500][0].abs() > 0.05);

        // Strict save/reopen round trip via hex-storage!
        let encoded = hex_storage::encode(session.project()).unwrap();
        let reopened = hex_storage::decode(&encoded).unwrap();
        assert_eq!(&reopened, session.project());

        // Single Undo restores exact pre-commit project (preserving global next_id high-water mark);
        // single Redo restores committed state!
        let committed_snapshot = session.project().snapshot();
        assert!(session.undo());
        let mut expected_before = before_commit;
        expected_before.next_id = committed_snapshot.next_id;
        assert_eq!(session.project().snapshot(), expected_before);
        assert!(session.redo());
        assert_eq!(session.project().snapshot(), committed_snapshot);
    }

    #[test]
    fn stale_project_overflow_cancel_and_discontinuity_refuse_without_mutating() {
        let (
            _dir,
            mut session,
            _initial_rev,
            midi_track,
            audio_track,
            _asset,
            scene_1,
            sess_midi,
            _sess_audio,
        ) = setup_two_track_session();
        let snapshot_before = session.project().clone();
        let binding = SessionPerformanceBinding::capture(
            &session,
            7,
            2,
            1,
            1,
            &[midi_track, audio_track],
            0,
            &[],
        )
        .unwrap();

        // 1. Stale observation refused.
        let mut stale_obs = SessionPerformanceCapture::new(binding.clone());
        let bad_obs = DeliveredRouteTransition {
            output_identity: 999,
            audio_revision: 2,
            control_generation: 1,
            transport_epoch: 1,
            transition: AppliedSessionRouteTransition {
                track: midi_track,
                scene: Some(scene_1),
                previous: AppliedSessionRouteTarget::Arrangement,
                target: AppliedSessionRouteTarget::SessionMidi {
                    clip: sess_midi,
                    origin: SessionMidiOrigin::launch(BeatTime::ZERO),
                },
                performance_frame: 0,
                content_frame: 0,
                loop_pass: 0,
                beat: BeatTime::ZERO,
            },
        };
        assert_eq!(
            stale_obs.record_batch(&[bad_obs], false),
            Err(SessionPerformanceError::StaleObservation)
        );

        // 2. Overflowed capture refuses commit with zero mutation.
        let mut overflowed = SessionPerformanceCapture::new(binding.clone());
        assert_eq!(
            overflowed.record_batch(&[], true),
            Err(SessionPerformanceError::Overflow)
        );
        assert!(
            session
                .commit_session_performance(overflowed, None)
                .is_err()
        );
        assert_eq!(session.project(), &snapshot_before);

        // 3. Cancelled capture refuses commit with zero mutation.
        let mut cancelled = SessionPerformanceCapture::new(binding.clone());
        cancelled.cancel();
        assert!(session.commit_session_performance(cancelled, None).is_err());
        assert_eq!(session.project(), &snapshot_before);

        // 4. Seek discontinuity in linear mode refused.
        let mut seek_jump = SessionPerformanceCapture::new(binding.clone());
        let ev1 = DeliveredRouteTransition {
            output_identity: 7,
            audio_revision: 2,
            control_generation: 1,
            transport_epoch: 1,
            transition: AppliedSessionRouteTransition {
                track: midi_track,
                scene: None,
                previous: AppliedSessionRouteTarget::Arrangement,
                target: AppliedSessionRouteTarget::Silence,
                performance_frame: 100,
                content_frame: 100,
                loop_pass: 0,
                beat: BeatTime::ZERO,
            },
        };
        let ev2 = DeliveredRouteTransition {
            transition: AppliedSessionRouteTransition {
                performance_frame: 200,
                content_frame: 50, // backward seek!
                ..ev1.transition
            },
            ..ev1
        };
        seek_jump.record_batch(&[ev1], false).unwrap();
        assert_eq!(
            seek_jump.record_batch(&[ev2], false),
            Err(SessionPerformanceError::TimingDiscontinuity)
        );

        // 5. Same-block events at equal performance frame succeed, but stale project refuses commit!
        let mut same_block = SessionPerformanceCapture::new(binding);
        let sb1 = DeliveredRouteTransition {
            transition: AppliedSessionRouteTransition {
                performance_frame: 4000,
                content_frame: 4000,
                beat: BeatTime::ONE,
                target: AppliedSessionRouteTarget::Silence,
                ..ev1.transition
            },
            ..ev1
        };
        let sb2 = DeliveredRouteTransition {
            transition: AppliedSessionRouteTransition {
                performance_frame: 4000,
                content_frame: 4000,
                beat: BeatTime::ONE,
                previous: AppliedSessionRouteTarget::Silence,
                target: AppliedSessionRouteTarget::SessionMidi {
                    clip: sess_midi,
                    origin: SessionMidiOrigin::launch(BeatTime::ONE),
                },
                ..ev1.transition
            },
            ..ev1
        };
        same_block.record_batch(&[sb1, sb2], false).unwrap();
        same_block.finish(8000).unwrap();

        // Mutate the canonical project after capture -> history_revision changed -> commit refuses!
        session
            .editor
            .execute(Command::RenameTrack {
                track: midi_track,
                name: "renamed".into(),
            })
            .unwrap();
        let after_rename = session.project().clone();
        assert!(
            session
                .commit_session_performance(same_block, None)
                .is_err()
        );
        assert_eq!(session.project(), &after_rename);
    }
}
