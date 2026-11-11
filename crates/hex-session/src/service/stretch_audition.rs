//! The latest held Arrangement stretch. A gesture sends only a scalar target
//! to the session worker. Prepared PCM and publication stay with the existing
//! resident overlay, and neither a preview nor its cancellation edits history.
use super::*;
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Intent {
    Idle,
    Held(ArrangementStretchAudition),
    Released(ArrangementStretchAudition),
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

    pub(super) fn set(&self, target: Option<ArrangementStretchAudition>) {
        let mut state = self.lock();
        Self::replace(&mut state, target.map_or(Intent::Idle, Intent::Held));
    }

    pub(super) fn release(&self) {
        let mut state = self.lock();
        if let Intent::Held(target) = state.intent {
            Self::replace(&mut state, Intent::Released(target));
        }
    }

    #[cfg(feature = "source-overview-test-support")]
    pub(super) fn held(&self) -> Option<ArrangementStretchAudition> {
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

    fn claim(&self, target: ArrangementStretchAudition) -> Option<Arc<AtomicBool>> {
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
    attempted: Option<ArrangementStretchAudition>,
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
            Intent::Held(target) if target.context == context && available => Some(target),
            _ => None,
        };
        if held.is_none() {
            self.attempted = None;
        }
        let stretch = preview
            .as_ref()
            .and_then(resident_preview::Preview::stretch_target);
        let releasing = matches!(desired, Intent::Released(target) if target.context == context
            && stretch.is_some_and(|active| active.clip == target.clip && active.edge == target.edge));
        let restore = stretch.is_some() && held.is_none() && !releasing;
        let prepare = held.filter(|target| Some(*target) != self.attempted);
        if !restore && prepare.is_none() {
            return false;
        }
        if preview.is_some() && stretch.is_none()
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
            let _ = resident_preview::stretch(
                session, target, current, preview, sink, admission, &cancel,
            );
            if cancel.load(Ordering::Acquire) {
                self.attempted = None;
            }
        }
        true
    }
}
