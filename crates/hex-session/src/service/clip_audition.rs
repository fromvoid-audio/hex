//! The latest held Arrangement clip edit. Scalar targets and bounded shared
//! group selections stay on control/worker threads. Prepared PCM and publication
//! stay with the resident overlay; preview and cancellation never edit history.
use super::*;
use hex_project::ClipId;
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArrangementStretchAudition {
    pub context: hex_engine::AudioSourceContext,
    pub clip: hex_project::ClipId,
    pub edge: hex_project::TrimEdge,
    pub frame: u64,
}

/// The exact clip play-start edit being auditioned against an authored revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArrangementPlayStartAudition {
    pub context: hex_engine::AudioSourceContext,
    pub edit: PlayStartEdit,
}

/// A resident-playback clip move or copy. Callers freeze the full selection
/// and reuse its allocation as the pointer target changes. Project validation
/// owns destination, duplicate-ID and clip compatibility checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArrangementMoveAudition {
    pub context: hex_engine::AudioSourceContext,
    /// Gesture identity and monotonically increasing target sequence, supplied
    /// by the control-side caller; neither is an audio application receipt.
    pub gesture: u64,
    pub sequence: u64,
    pub clips: Arc<[ClipId]>,
    pub anchor: ClipId,
    pub destination: ArrangementMoveDestination,
    pub position: hex_project::ClipGroupPosition,
    pub mode: ArrangementMoveMode,
}

/// Destination data is captured once on the control side, before preparation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArrangementMoveDestination {
    Existing(hex_project::TrackId),
    NewTrack(NewTrackMoveDestination),
}

/// Validated bounded inputs for the existing canonical new-track commands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewTrackMoveDestination {
    name: Arc<str>,
    before_track: Option<hex_project::TrackId>,
    gain_bits: u32,
}

impl NewTrackMoveDestination {
    /// Captures a valid track name, insertion anchor, and finite initial gain.
    /// Returns `None` when the name or gain is outside the project contract.
    pub fn new(
        name: Arc<str>,
        before_track: Option<hex_project::TrackId>,
        gain: f32,
    ) -> Option<Self> {
        if name.trim().is_empty()
            || name.len() > hex_project::MAX_NAME_BYTES
            || name.chars().any(char::is_control)
            || !gain.is_finite()
            || !(0.0..=2.0).contains(&gain)
        {
            return None;
        }
        Some(Self {
            name,
            before_track,
            gain_bits: gain.to_bits(),
        })
    }
}

/// Scalar destination sufficient to correlate callback-applied placement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipMovePreviewDestination {
    Existing(hex_project::TrackId),
    NewTrack {
        before_track: Option<hex_project::TrackId>,
    },
}
impl ClipMovePreviewDestination {
    /// Returns the existing track, if the applied destination is not new.
    pub fn existing_track(self) -> Option<hex_project::TrackId> {
        match self {
            Self::Existing(track) => Some(track),
            Self::NewTrack { .. } => None,
        }
    }
}

/// Selects the same authored placement semantics used by the eventual drag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArrangementMoveMode {
    /// Move the existing identities.
    Move,
    /// Create independent copies.
    Copy,
    /// Create linked aliases.
    AliasCopy,
}

/// Scalar correlation for one submitted clip-move preview. The actual
/// callback audio revision and output identity remain separate witnesses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClipMovePreviewIdentity {
    pub context: hex_engine::AudioSourceContext,
    pub gesture: u64,
    pub sequence: u64,
    /// The anchor placement represented by this publication, in project units.
    pub destination: ClipMovePreviewDestination,
    pub position: hex_project::ClipGroupPosition,
    pub mode: ArrangementMoveMode,
}

/// Bounded worker-to-host metadata; acceptance is not callback application.
pub enum VerifiedClipMoveUpdate {
    Preview(Arc<VerifiedClipMoveSource>),
    Released {
        canonical: Arc<CanonicalAudioSourceSnapshot>,
        identity: ClipMovePreviewIdentity,
    },
    Cancelled {
        canonical: Arc<CanonicalAudioSourceSnapshot>,
        identity: ClipMovePreviewIdentity,
    },
}

impl ArrangementMoveAudition {
    pub fn identity(&self) -> ClipMovePreviewIdentity {
        ClipMovePreviewIdentity {
            context: self.context,
            gesture: self.gesture,
            sequence: self.sequence,
            destination: match &self.destination {
                ArrangementMoveDestination::Existing(track) => {
                    ClipMovePreviewDestination::Existing(*track)
                }
                ArrangementMoveDestination::NewTrack(track) => {
                    ClipMovePreviewDestination::NewTrack {
                        before_track: track.before_track,
                    }
                }
            },
            position: self.position,
            mode: self.mode,
        }
    }
    pub(super) fn valid(&self) -> bool {
        !self.clips.is_empty()
            && self.clips.len() <= hex_project::MAX_CLIPS
            && self.clips.contains(&self.anchor)
            && (!matches!(self.destination, ArrangementMoveDestination::NewTrack(_))
                || self.clips.len() == 1)
    }
}

/// Clip-local marker edits admitted to the Arrangement audition lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayStartEdit {
    Audio {
        clip: ClipId,
        position: u64,
    },
    MidiRegion {
        clip: ClipId,
        source_start: hex_project::BeatTime,
    },
    MidiSource {
        clip: ClipId,
        source_length: hex_project::BeatTime,
        source_start: hex_project::BeatTime,
        loop_start: hex_project::BeatTime,
        loop_end: hex_project::BeatTime,
        looping: bool,
        length: hex_project::BeatTime,
    },
}

impl PlayStartEdit {
    /// Reuse the MIDI ruler's choice of canonical edit for its preview.
    pub fn from_midi_command(command: &Command) -> Option<Self> {
        match *command {
            Command::ResizeMidiClipView {
                clip,
                operation: hex_project::MidiViewResizeOperation::PlayStart { source_start },
            } => Some(Self::MidiRegion { clip, source_start }),
            Command::SetMidiClipSource {
                clip,
                source_length,
                source_start,
                loop_start,
                loop_end,
                looping,
                length,
            } => Some(Self::MidiSource {
                clip,
                source_length,
                source_start,
                loop_start,
                loop_end,
                looping,
                length,
            }),
            _ => None,
        }
    }

    fn from_command(command: &Command) -> Option<Self> {
        match *command {
            Command::ResizeAudioClipView {
                clip,
                operation: hex_project::AudioViewResizeOperation::PlayStart { position },
            } => Some(Self::Audio { clip, position }),
            _ => Self::from_midi_command(command),
        }
    }

    fn clip(self) -> ClipId {
        match self {
            Self::Audio { clip, .. }
            | Self::MidiRegion { clip, .. }
            | Self::MidiSource { clip, .. } => clip,
        }
    }

    fn command(self) -> Command {
        match self {
            Self::Audio { clip, position } => Command::ResizeAudioClipView {
                clip,
                operation: hex_project::AudioViewResizeOperation::PlayStart { position },
            },
            Self::MidiRegion { clip, source_start } => Command::ResizeMidiClipView {
                clip,
                operation: hex_project::MidiViewResizeOperation::PlayStart { source_start },
            },
            Self::MidiSource {
                clip,
                source_length,
                source_start,
                loop_start,
                loop_end,
                looping,
                length,
            } => Command::SetMidiClipSource {
                clip,
                source_length,
                source_start,
                loop_start,
                loop_end,
                looping,
                length,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Target {
    Stretch(ArrangementStretchAudition),
    PlayStart(ArrangementPlayStartAudition),
    Move(ArrangementMoveAudition),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Stretch,
    PlayStart,
    Move,
}

impl Target {
    fn kind(&self) -> Kind {
        match self {
            Self::Stretch(_) => Kind::Stretch,
            Self::PlayStart(_) => Kind::PlayStart,
            Self::Move(_) => Kind::Move,
        }
    }

    pub(super) fn context(&self) -> hex_engine::AudioSourceContext {
        match self {
            Self::Stretch(target) => target.context,
            Self::PlayStart(target) => target.context,
            Self::Move(target) => target.context,
        }
    }

    pub(super) fn move_identity(&self) -> Option<ClipMovePreviewIdentity> {
        match self {
            Self::Move(target) => Some(target.identity()),
            _ => None,
        }
    }

    pub(super) fn command(&self) -> Command {
        match self {
            Self::Stretch(target) => Command::StretchAudioClipEdge {
                clip: target.clip,
                edge: target.edge,
                frame: target.frame,
            },
            Self::PlayStart(target) => target.edit.command(),
            Self::Move(target) => match &target.destination {
                ArrangementMoveDestination::Existing(track) => {
                    match (target.clips.len(), target.mode, target.position) {
                        (_, ArrangementMoveMode::AliasCopy, position) => {
                            Command::DuplicateAliasClipsAt {
                                clips: target.clips.to_vec(),
                                anchor: target.anchor,
                                track: *track,
                                position,
                            }
                        }
                        (
                            1,
                            ArrangementMoveMode::Move,
                            hex_project::ClipGroupPosition::Audio(start),
                        ) => Command::MoveClipWithOverwrite {
                            clip: target.anchor,
                            track: *track,
                            start,
                        },
                        (
                            1,
                            ArrangementMoveMode::Move,
                            hex_project::ClipGroupPosition::Midi(start),
                        ) => Command::MoveMidiClipWithOverwrite {
                            clip: target.anchor,
                            track: *track,
                            start,
                        },
                        (
                            1,
                            ArrangementMoveMode::Copy,
                            hex_project::ClipGroupPosition::Audio(start),
                        ) => Command::CopyClipTo {
                            clip: target.anchor,
                            track: *track,
                            start,
                        },
                        (
                            1,
                            ArrangementMoveMode::Copy,
                            hex_project::ClipGroupPosition::Midi(start),
                        ) => Command::CopyMidiClipTo {
                            clip: target.anchor,
                            track: *track,
                            start,
                        },
                        (_, mode, position) => Command::MoveClipsWithOverwrite {
                            clips: target.clips.to_vec(),
                            anchor: target.anchor,
                            track: *track,
                            position,
                            copy: mode == ArrangementMoveMode::Copy,
                        },
                    }
                }
                ArrangementMoveDestination::NewTrack(destination) => {
                    let name = destination.name.to_string();
                    let before_track = destination.before_track;
                    let initial_gain = Some(f32::from_bits(destination.gain_bits));
                    let clip = target.anchor;
                    match (target.mode, target.position) {
                        (ArrangementMoveMode::AliasCopy, position) => {
                            Command::DuplicateAliasClipToNewTrack {
                                clip,
                                name,
                                before_track,
                                position,
                                initial_gain,
                            }
                        }
                        (
                            ArrangementMoveMode::Move,
                            hex_project::ClipGroupPosition::Audio(start),
                        ) => Command::MoveClipToNewTrack {
                            clip,
                            name,
                            before_track,
                            start,
                            initial_gain,
                        },
                        (
                            ArrangementMoveMode::Move,
                            hex_project::ClipGroupPosition::Midi(start),
                        ) => Command::MoveMidiClipToNewTrack {
                            clip,
                            name,
                            before_track,
                            start,
                            initial_gain,
                        },
                        (
                            ArrangementMoveMode::Copy,
                            hex_project::ClipGroupPosition::Audio(start),
                        ) => Command::CopyClipToNewTrack {
                            clip,
                            name,
                            before_track,
                            start,
                            initial_gain,
                        },
                        (
                            ArrangementMoveMode::Copy,
                            hex_project::ClipGroupPosition::Midi(start),
                        ) => Command::CopyMidiClipToNewTrack {
                            clip,
                            name,
                            before_track,
                            start,
                            initial_gain,
                        },
                    }
                }
            },
        }
    }

    fn same_gesture(&self, other: &Self) -> bool {
        if self.context() != other.context() {
            return false;
        }
        match (self, other) {
            (Self::Stretch(a), Self::Stretch(b)) => a.clip == b.clip && a.edge == b.edge,
            (Self::PlayStart(a), Self::PlayStart(b)) => a.edit.clip() == b.edit.clip(),
            (Self::Move(a), Self::Move(b)) => {
                a.gesture == b.gesture && a.anchor == b.anchor && a.clips == b.clips
            }
            _ => false,
        }
    }

    pub(super) fn matches_release(&self, request: &Request) -> bool {
        if let (
            Self::Move(target),
            Request::MoveExistingClips {
                project_generation,
                revision,
                command,
            },
        ) = (self, request)
        {
            let new_track_command = matches!(
                command,
                Command::MoveClipToNewTrack { .. }
                    | Command::MoveMidiClipToNewTrack { .. }
                    | Command::CopyClipToNewTrack { .. }
                    | Command::CopyMidiClipToNewTrack { .. }
                    | Command::DuplicateAliasClipToNewTrack { .. }
            );
            return *project_generation == target.context.project_generation
                && *revision == target.context.editor_revision
                && new_track_command
                    == matches!(target.destination, ArrangementMoveDestination::NewTrack(_))
                && match command {
                    Command::MoveClipsWithOverwrite { clips, anchor, .. }
                    | Command::DuplicateAliasClipsAt { clips, anchor, .. } => {
                        *anchor == target.anchor && clips.as_slice() == target.clips.as_ref()
                    }
                    Command::MoveClipWithOverwrite { clip, .. }
                    | Command::MoveMidiClipWithOverwrite { clip, .. }
                    | Command::CopyClipTo { clip, .. }
                    | Command::CopyMidiClipTo { clip, .. }
                    | Command::MoveClipToNewTrack { clip, .. }
                    | Command::MoveMidiClipToNewTrack { clip, .. }
                    | Command::CopyClipToNewTrack { clip, .. }
                    | Command::CopyMidiClipToNewTrack { clip, .. }
                    | Command::DuplicateAliasClipToNewTrack { clip, .. } => {
                        target.clips.len() == 1 && *clip == target.anchor
                    }
                    _ => false,
                };
        }
        let Request::Edit(command) = request else {
            return false;
        };
        match self {
            Self::Stretch(_) => matches!(
                command,
                Command::StretchAudioClipEdge { .. } | Command::TrimClipEdge { .. }
            ),
            Self::PlayStart(target) => PlayStartEdit::from_command(command)
                .is_some_and(|edit| edit.clip() == target.edit.clip()),
            Self::Move(_) => false,
        }
    }

    pub(super) fn retained_bytes(&self) -> usize {
        match self {
            Self::Move(target) => target
                .clips
                .len()
                .saturating_mul(std::mem::size_of::<ClipId>())
                .saturating_add(2 * std::mem::size_of::<usize>())
                .saturating_add(match &target.destination {
                    ArrangementMoveDestination::NewTrack(destination) => destination
                        .name
                        .len()
                        .saturating_add(2 * std::mem::size_of::<usize>()),
                    ArrangementMoveDestination::Existing(_) => 0,
                }),
            _ => 0,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
enum Intent {
    Idle,
    Held(Target),
    Released(Target),
}

struct State {
    intent: Intent,
    preparing: Option<Arc<AtomicBool>>,
}

pub(super) struct Mailbox(Mutex<State>);

impl Default for Mailbox {
    fn default() -> Self {
        Self(Mutex::new(State {
            intent: Intent::Idle,
            preparing: None,
        }))
    }
}

impl Mailbox {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(super) fn set(&self, target: Target) {
        Self::replace(&mut self.lock(), Intent::Held(target));
    }

    pub(super) fn clear(&self, kind: Option<Kind>, held_only: bool) {
        let mut state = self.lock();
        let target = match &state.intent {
            Intent::Held(target) => target,
            Intent::Released(target) if !held_only => target,
            _ => return,
        };
        if kind.is_none_or(|kind| target.kind() == kind) {
            Self::replace(&mut state, Intent::Idle);
        }
    }

    pub(super) fn release(&self, kind: Kind) {
        let mut state = self.lock();
        if let Intent::Held(target) = &state.intent
            && target.kind() == kind
        {
            let target = target.clone();
            Self::replace(&mut state, Intent::Released(target));
        }
    }

    #[cfg(feature = "source-overview-test-support")]
    pub(super) fn held(&self) -> Option<Target> {
        match self.lock().intent.clone() {
            Intent::Held(target) => Some(target),
            _ => None,
        }
    }

    fn replace(state: &mut State, intent: Intent) {
        if state.intent != intent {
            if let Some(job) = state.preparing.take() {
                job.store(true, Ordering::Release);
            }
            state.intent = intent;
        }
    }

    fn claim(&self, target: Target) -> Option<Arc<AtomicBool>> {
        let mut state = self.lock();
        if state.intent != Intent::Held(target) {
            return None;
        }
        let job = Arc::new(AtomicBool::new(false));
        state.preparing = Some(Arc::clone(&job));
        Some(job)
    }
}

// Successful move preparation is capped at one attempt per 16 ms (at most
// 63 starts per second). Refusal, restoration and other clip auditions retain
// the original slower interval to avoid repeated expensive failures.
const MOVE_PREVIEW_INTERVAL: Duration = Duration::from_millis(16);
const RETRY_INTERVAL: Duration = Duration::from_millis(32);

fn attempt_interval(kind: Kind, failed: bool, cancelled: bool) -> Duration {
    if kind == Kind::Move && (!failed || cancelled) {
        MOVE_PREVIEW_INTERVAL
    } else {
        RETRY_INTERVAL
    }
}

/// One worker-owned delivery state. The host owns graph construction and may
/// refuse any update; this state never treats delivery as audible playback.
#[derive(Default)]
pub(super) struct VerifiedLane {
    attempted: Option<ArrangementMoveAudition>,
    active: Option<(Arc<CanonicalAudioSourceSnapshot>, ClipMovePreviewIdentity)>,
    released: bool,
    next_attempt: Option<Instant>,
}

impl VerifiedLane {
    pub(super) fn drive(
        &mut self,
        mailbox: &Mailbox,
        state: &ViewState,
        project: &Project,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        available: bool,
    ) -> bool {
        let intent = mailbox.lock().intent.clone();
        let canonical = state.canonical_audio_source.as_ref().filter(|source| {
            available
                && source.audio_mode() == DocumentAudioMode::VerifiedStreaming
                && state.project.as_ref() == project
                && state.audio_authoring_revision(context) == Some(state.revision)
                && context.editor_revision == state.revision
        });
        let held = match (&intent, canonical) {
            (Intent::Held(Target::Move(target)), Some(source)) if target.context == context => {
                Some((target, source))
            }
            _ => None,
        };
        let release = match (&intent, canonical, self.active.as_ref()) {
            (
                Intent::Released(Target::Move(target)),
                Some(source),
                Some((active_source, active)),
            ) if Arc::ptr_eq(source, active_source)
                && target.context == active.context
                && target.gesture == active.gesture =>
            {
                Some((target.identity(), source))
            }
            _ => None,
        };
        if let Some((identity, source)) = release {
            self.attempted = None;
            if !self.released {
                self.released = true;
                sink.verified_clip_move_update(VerifiedClipMoveUpdate::Released {
                    canonical: Arc::clone(source),
                    identity,
                });
            }
            return false;
        }
        let Some((target, source)) = held else {
            self.attempted = None;
            self.released = false;
            if let Some((canonical, identity)) = self.active.take() {
                sink.verified_clip_move_update(VerifiedClipMoveUpdate::Cancelled {
                    canonical,
                    identity,
                });
            }
            return false;
        };
        self.released = false;
        if self.attempted.as_ref() == Some(target) {
            return false;
        }
        if self.next_attempt.is_some_and(|next| next > Instant::now()) {
            return true;
        }
        self.attempted = Some(target.clone());
        let Some(cancel) = mailbox.claim(Target::Move(target.clone())) else {
            self.attempted = None;
            return true;
        };
        let started = Instant::now();
        let candidate = VerifiedClipMoveSource::capture_current(
            Arc::clone(source),
            target.clone(),
            project,
            state,
        );
        let mut accepted = false;
        let mut deliverable = false;
        if let Ok(candidate) = candidate
            && candidate.candidate_reservation_quote().is_ok()
            && !cancel.load(Ordering::Acquire)
        {
            deliverable = true;
            let identity = candidate.identity();
            accepted = sink
                .verified_clip_move_update(VerifiedClipMoveUpdate::Preview(Arc::new(candidate)));
            if accepted {
                self.active = Some((Arc::clone(source), identity));
            }
        }
        let cancelled = cancel.load(Ordering::Acquire);
        self.next_attempt = Some(started + attempt_interval(Kind::Move, !accepted, cancelled));
        if !accepted && (deliverable || cancelled) {
            // Retry a refused or cancelled attempt if its target is still held.
            self.attempted = None;
            return true;
        }
        false
    }
}

#[derive(Default)]
pub(super) struct Lane {
    attempted: Option<Target>,
    next_attempt: Option<Instant>,
}

impl Lane {
    /// Runs only after the canonical FIFO and ready continuations were checked.
    /// Publication never holds the mailbox lock. Rejected preparations leave the
    /// last accepted graph running; restore retries at bounded cadence.
    #[allow(clippy::too_many_arguments)] // Existing resident publication and source custody.
    pub(super) fn drive(
        &mut self,
        mailbox: &Mailbox,
        session: &Session,
        context: hex_engine::AudioSourceContext,
        current: &mut Option<resident_audio::Snapshot>,
        preview: &mut Option<resident_preview::Preview>,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        available: bool,
    ) -> bool {
        let desired = mailbox.lock().intent.clone();
        let held = match &desired {
            Intent::Held(target) if target.context() == context && available => {
                Some(target.clone())
            }
            _ => None,
        };
        if held.is_none() {
            self.attempted = None;
        }
        let audition = preview
            .as_ref()
            .and_then(resident_preview::Preview::clip_target);
        let releasing = matches!(&desired, Intent::Released(target) if target.context() == context
            && audition.as_ref().is_some_and(|active| active.same_gesture(target)));
        let restore = audition.is_some() && held.is_none() && !releasing;
        let prepare = held.filter(|target| Some(target) != self.attempted.as_ref());
        if !restore && prepare.is_none() {
            return false;
        }
        if preview.is_some() && audition.is_none()
            || session.audio_mode() != DocumentAudioMode::Resident
            || !sink.resident_publication_enabled()
        {
            return false;
        }
        if self.next_attempt.is_some_and(|time| time > Instant::now()) {
            return true;
        }
        use ResidentPublicationReadiness::{Applied, Idle, Refused};
        if !matches!(
            sink.resident_publication_readiness(),
            Idle {
                publication_in_flight: false,
                ..
            } | Applied {
                publication_in_flight: false,
                ..
            } | Refused {
                publication_in_flight: false,
                ..
            }
        ) {
            return true;
        }
        if restore {
            self.next_attempt = Some(Instant::now() + RETRY_INTERVAL);
            let _ = resident_preview::restore(session, context, current, preview, sink, admission);
        } else if let Some(target) = prepare
            && let Some(cancel) = mailbox.claim(target.clone())
        {
            let started = Instant::now();
            let kind = target.kind();
            self.attempted = Some(target.clone());
            let result = resident_preview::clip_audition(
                session, target, current, preview, sink, admission, &cancel,
            );
            let cancelled = cancel.load(Ordering::Acquire);
            self.next_attempt = Some(started + attempt_interval(kind, result.is_err(), cancelled));
            if cancelled {
                self.attempted = None;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{BeatTime, ClipGroupPosition, Created, Editor, Project};

    fn target() -> ArrangementMoveAudition {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: "source".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let mut clips = Vec::new();
        for start in [BeatTime::ZERO, BeatTime::from_quarters(2).unwrap()] {
            let Created::Clip(clip) = editor
                .execute(Command::PlaceMidiClip {
                    track,
                    start,
                    length: BeatTime::ONE,
                })
                .unwrap()
            else {
                panic!("clip")
            };
            clips.push(clip);
        }
        ArrangementMoveAudition {
            context: hex_engine::AudioSourceContext {
                project_generation: 3,
                editor_revision: 7,
            },
            gesture: 1,
            sequence: 1,
            anchor: clips[0],
            clips: clips.into(),
            destination: ArrangementMoveDestination::Existing(track),
            position: ClipGroupPosition::Midi(BeatTime::from_quarters(4).unwrap()),
            mode: ArrangementMoveMode::Move,
        }
    }

    #[test]
    fn move_cadence_bounds_work_and_keeps_failure_backoff() {
        let movement = Target::Move(target());
        let stretch = Target::Stretch(ArrangementStretchAudition {
            context: movement.context(),
            clip: target().anchor,
            edge: hex_project::TrimEdge::Right,
            frame: 1,
        });
        assert_eq!(
            attempt_interval(movement.kind(), false, false),
            Duration::from_millis(16)
        );
        assert_eq!(
            attempt_interval(movement.kind(), true, true),
            Duration::from_millis(16)
        );
        assert_eq!(
            attempt_interval(movement.kind(), true, false),
            Duration::from_millis(32)
        );
        assert_eq!(
            attempt_interval(stretch.kind(), false, false),
            Duration::from_millis(32)
        );
        // Count possible starts over a half-open one-second interval.
        assert_eq!(
            (1000_u64 - 1) / attempt_interval(movement.kind(), false, false).as_millis() as u64 + 1,
            63
        );
    }

    #[test]
    fn new_track_targets_match_canonical_commands_and_reject_invalid_inputs() {
        let mut single = target();
        single.clips = vec![single.anchor].into();
        let before = match single.destination {
            ArrangementMoveDestination::Existing(track) => Some(track),
            _ => panic!("track"),
        };
        for gain in [f32::NAN, f32::INFINITY, -0.1, 2.1] {
            assert!(NewTrackMoveDestination::new(Arc::from("track 2"), before, gain).is_none());
        }
        for name in [
            String::new(),
            "bad\nname".into(),
            "x".repeat(hex_project::MAX_NAME_BYTES + 1),
        ] {
            assert!(NewTrackMoveDestination::new(Arc::from(name), before, 1.25).is_none());
        }
        let destination = NewTrackMoveDestination::new(Arc::from("track 2"), before, 1.25).unwrap();
        single.destination = ArrangementMoveDestination::NewTrack(destination);
        let mut at_end = single.clone();
        at_end.destination = ArrangementMoveDestination::NewTrack(
            NewTrackMoveDestination::new(Arc::from("track 2"), None, 1.25).unwrap(),
        );
        assert_eq!(
            at_end.identity().destination,
            ClipMovePreviewDestination::NewTrack { before_track: None }
        );
        let mut invalid_group = single.clone();
        invalid_group.clips = vec![single.anchor, single.anchor].into();
        assert!(!invalid_group.valid());
        assert!(single.valid());
        assert!(Target::Move(single.clone()).retained_bytes() >= "track 2".len());
        for position in [
            ClipGroupPosition::Audio(123),
            ClipGroupPosition::Midi(BeatTime::ONE),
        ] {
            for mode in [
                ArrangementMoveMode::Move,
                ArrangementMoveMode::Copy,
                ArrangementMoveMode::AliasCopy,
            ] {
                single.position = position;
                single.mode = mode;
                let target = Target::Move(single.clone());
                assert_eq!(
                    single.identity().destination,
                    ClipMovePreviewDestination::NewTrack {
                        before_track: before
                    }
                );
                let command = target.command();
                let request = Request::MoveExistingClips {
                    project_generation: single.context.project_generation,
                    revision: single.context.editor_revision,
                    command: command.clone(),
                };
                assert!(target.matches_release(&request));
                let (
                    actual_mode,
                    actual_clip,
                    actual_name,
                    actual_before,
                    actual_position,
                    actual_gain,
                ) = match command {
                    Command::MoveClipToNewTrack {
                        clip,
                        name,
                        before_track,
                        start,
                        initial_gain,
                    } => (
                        ArrangementMoveMode::Move,
                        clip,
                        name,
                        before_track,
                        ClipGroupPosition::Audio(start),
                        initial_gain,
                    ),
                    Command::CopyClipToNewTrack {
                        clip,
                        name,
                        before_track,
                        start,
                        initial_gain,
                    } => (
                        ArrangementMoveMode::Copy,
                        clip,
                        name,
                        before_track,
                        ClipGroupPosition::Audio(start),
                        initial_gain,
                    ),
                    Command::MoveMidiClipToNewTrack {
                        clip,
                        name,
                        before_track,
                        start,
                        initial_gain,
                    } => (
                        ArrangementMoveMode::Move,
                        clip,
                        name,
                        before_track,
                        ClipGroupPosition::Midi(start),
                        initial_gain,
                    ),
                    Command::CopyMidiClipToNewTrack {
                        clip,
                        name,
                        before_track,
                        start,
                        initial_gain,
                    } => (
                        ArrangementMoveMode::Copy,
                        clip,
                        name,
                        before_track,
                        ClipGroupPosition::Midi(start),
                        initial_gain,
                    ),
                    Command::DuplicateAliasClipToNewTrack {
                        clip,
                        name,
                        before_track,
                        position,
                        initial_gain,
                    } => (
                        ArrangementMoveMode::AliasCopy,
                        clip,
                        name,
                        before_track,
                        position,
                        initial_gain,
                    ),
                    other => panic!("unexpected command: {other:?}"),
                };
                assert_eq!(
                    (
                        actual_mode,
                        actual_clip,
                        actual_name.as_str(),
                        actual_before,
                        actual_position,
                        actual_gain
                    ),
                    (mode, single.anchor, "track 2", before, position, Some(1.25))
                );
                let mut existing = single.clone();
                existing.destination = ArrangementMoveDestination::Existing(before.unwrap());
                assert!(!Target::Move(existing).matches_release(&request));
            }
        }
    }

    #[test]
    fn group_custody_cancels_old_work_and_matches_only_its_source() {
        let first = target();
        let identity = first.identity();
        assert_eq!(
            identity.destination,
            ClipMovePreviewDestination::Existing(match first.destination {
                ArrangementMoveDestination::Existing(track) => track,
                _ => panic!("existing"),
            })
        );
        assert_eq!(identity.position, first.position);
        assert_eq!(identity.mode, first.mode);
        let mailbox = Mailbox::default();
        let a = Target::Move(first.clone());
        mailbox.set(a.clone());
        let canceled = mailbox.claim(a.clone()).unwrap();
        let mut moved = first.clone();
        moved.position = ClipGroupPosition::Midi(BeatTime::from_quarters(6).unwrap());
        let b = Target::Move(moved.clone());
        mailbox.set(b.clone());
        assert!(canceled.load(Ordering::Acquire));
        assert!(mailbox.claim(a.clone()).is_none());
        let Intent::Held(Target::Move(retained)) = mailbox.lock().intent.clone() else {
            panic!("held group")
        };
        assert!(Arc::ptr_eq(&first.clips, &retained.clips));
        assert!(a.same_gesture(&b));
        let mut other_gesture = moved.clone();
        other_gesture.gesture += 1;
        assert!(!a.same_gesture(&Target::Move(other_gesture)));
        let mut other = moved.clone();
        other.context.editor_revision += 1;
        assert!(!a.same_gesture(&Target::Move(other)));
        mailbox.release(Kind::Move);
        assert!(
            matches!(&mailbox.lock().intent, Intent::Released(target) if target.same_gesture(&b))
        );
        let request = Request::MoveExistingClips {
            project_generation: moved.context.project_generation,
            revision: moved.context.editor_revision,
            command: b.command(),
        };
        assert!(a.matches_release(&request));
        let Request::MoveExistingClips {
            project_generation,
            revision,
            command,
        } = request
        else {
            panic!("move request")
        };
        assert!(!a.matches_release(&Request::MoveExistingClips {
            project_generation,
            revision: revision + 1,
            command: command.clone(),
        }));
        assert!(!a.matches_release(&Request::MoveExistingClips {
            project_generation: project_generation + 1,
            revision,
            command,
        }));
        let mut different_group = moved;
        different_group.clips = vec![different_group.anchor].into();
        assert!(!a.matches_release(&Request::MoveExistingClips {
            project_generation,
            revision,
            command: Target::Move(different_group).command(),
        }));
        mailbox.clear(Some(Kind::Move), false);
        assert!(matches!(mailbox.lock().intent, Intent::Idle));
    }

    #[test]
    fn retained_group_bytes_cover_the_complete_selection() {
        let group = target();
        assert!(group.valid());
        let mut invalid = group.clone();
        invalid.clips = Arc::from([]);
        assert!(!invalid.valid());
        invalid.clips = vec![group.clips[1]].into();
        assert!(!invalid.valid());
        invalid.clips = vec![group.anchor; hex_project::MAX_CLIPS + 1].into();
        assert!(!invalid.valid());
        let target = Target::Move(group.clone());
        assert!(target.retained_bytes() >= group.clips.len() * std::mem::size_of::<ClipId>());
        assert!(
            matches!(target.command(), Command::MoveClipsWithOverwrite { clips, anchor, .. } if clips.as_slice() == group.clips.as_ref() && anchor == group.anchor)
        );
    }

    #[test]
    fn targets_match_single_group_and_alias_commit_commands() {
        let mut group = target();
        for mode in [
            ArrangementMoveMode::Move,
            ArrangementMoveMode::Copy,
            ArrangementMoveMode::AliasCopy,
        ] {
            group.mode = mode;
            let target = Target::Move(group.clone());
            let command = target.command();
            assert!(matches!(
                (&mode, &command),
                (
                    ArrangementMoveMode::Move,
                    Command::MoveClipsWithOverwrite { copy: false, .. }
                ) | (
                    ArrangementMoveMode::Copy,
                    Command::MoveClipsWithOverwrite { copy: true, .. }
                ) | (
                    ArrangementMoveMode::AliasCopy,
                    Command::DuplicateAliasClipsAt { .. }
                )
            ));
            assert!(target.matches_release(&Request::MoveExistingClips {
                project_generation: group.context.project_generation,
                revision: group.context.editor_revision,
                command,
            }));
        }
        group.clips = vec![group.anchor].into();
        for position in [
            ClipGroupPosition::Audio(100),
            ClipGroupPosition::Midi(BeatTime::ONE),
        ] {
            for mode in [
                ArrangementMoveMode::Move,
                ArrangementMoveMode::Copy,
                ArrangementMoveMode::AliasCopy,
            ] {
                group.position = position;
                group.mode = mode;
                let target = Target::Move(group.clone());
                let command = target.command();
                assert!(matches!(
                    (position, mode, &command),
                    (
                        ClipGroupPosition::Audio(_),
                        ArrangementMoveMode::Move,
                        Command::MoveClipWithOverwrite { .. }
                    ) | (
                        ClipGroupPosition::Audio(_),
                        ArrangementMoveMode::Copy,
                        Command::CopyClipTo { .. }
                    ) | (
                        ClipGroupPosition::Midi(_),
                        ArrangementMoveMode::Move,
                        Command::MoveMidiClipWithOverwrite { .. }
                    ) | (
                        ClipGroupPosition::Midi(_),
                        ArrangementMoveMode::Copy,
                        Command::CopyMidiClipTo { .. }
                    ) | (
                        _,
                        ArrangementMoveMode::AliasCopy,
                        Command::DuplicateAliasClipsAt { .. }
                    )
                ));
                assert!(target.matches_release(&Request::MoveExistingClips {
                    project_generation: group.context.project_generation,
                    revision: group.context.editor_revision,
                    command,
                }));
            }
        }
    }
}

#[cfg(test)]
#[path = "clip_audition/verified_lane_tests.rs"]
mod verified_lane_tests;
