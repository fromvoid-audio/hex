//! The latest held Arrangement clip timing edit. A gesture sends only a scalar target
//! to the session worker. Prepared PCM and publication stay with the existing
//! resident overlay, and neither a preview nor its cancellation edits history.
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Target {
    Stretch(ArrangementStretchAudition),
    PlayStart(ArrangementPlayStartAudition),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Stretch,
    PlayStart,
}

impl Target {
    fn kind(self) -> Kind {
        match self {
            Self::Stretch(_) => Kind::Stretch,
            Self::PlayStart(_) => Kind::PlayStart,
        }
    }

    pub(super) fn context(self) -> hex_engine::AudioSourceContext {
        match self {
            Self::Stretch(target) => target.context,
            Self::PlayStart(target) => target.context,
        }
    }

    pub(super) fn command(self) -> Command {
        match self {
            Self::Stretch(target) => Command::StretchAudioClipEdge {
                clip: target.clip,
                edge: target.edge,
                frame: target.frame,
            },
            Self::PlayStart(target) => target.edit.command(),
        }
    }

    fn same_gesture(self, other: Self) -> bool {
        if self.context() != other.context() {
            return false;
        }
        match (self, other) {
            (Self::Stretch(a), Self::Stretch(b)) => a.clip == b.clip && a.edge == b.edge,
            (Self::PlayStart(a), Self::PlayStart(b)) => a.edit.clip() == b.edit.clip(),
            _ => false,
        }
    }

    pub(super) fn matches_release(self, request: &Request) -> bool {
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
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
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
        let target = match state.intent {
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
        if let Intent::Held(target) = state.intent
            && target.kind() == kind
        {
            Self::replace(&mut state, Intent::Released(target));
        }
    }

    #[cfg(feature = "source-overview-test-support")]
    pub(super) fn held(&self) -> Option<Target> {
        match self.lock().intent {
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
        let desired = mailbox.lock().intent;
        let held = match desired {
            Intent::Held(target) if target.context() == context && available => Some(target),
            _ => None,
        };
        if held.is_none() {
            self.attempted = None;
        }
        let audition = preview
            .as_ref()
            .and_then(resident_preview::Preview::clip_target);
        let releasing = matches!(desired, Intent::Released(target) if target.context() == context
            && audition.is_some_and(|active| active.same_gesture(target)));
        let restore = audition.is_some() && held.is_none() && !releasing;
        let prepare = held.filter(|target| Some(*target) != self.attempted);
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
        self.next_attempt = Some(Instant::now() + Duration::from_millis(32));
        if restore {
            let _ = resident_preview::restore(session, context, current, preview, sink, admission);
        } else if let Some(target) = prepare
            && let Some(cancel) = mailbox.claim(target)
        {
            self.attempted = Some(target);
            let _ = resident_preview::clip_audition(
                session, target, current, preview, sink, admission, &cancel,
            );
            if cancel.load(Ordering::Acquire) {
                self.attempted = None;
            }
        }
        true
    }
}
