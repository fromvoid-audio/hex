//! Existing callback-observed stopped Arrangement lease shared by structural
//! edits and in-place bounces. No runtime, source or resource custody lives here.
//!
//! # Deferred fence (#4544)
//!
//! Section edits, section history and in-place bounce history take their lease
//! before any other work, so the worker begins it *before dispatch*
//! ([`Fence::defer`]). An immediate answer dispatches at once. A live callback
//! that has not yet observed the fence parks the original request in
//! [`Fence`] and returns the sole worker to its request loop instead of
//! sleeping on it. Each loop turn ([`Fence::next`], at most every
//! [`FENCE_POLL`]) re-polls the fence and re-dispatches the parked request once
//! it resolves; its handler consumes the outcome through [`acquire_position`]
//! with exactly the result and refusal text of the former synchronous poll.
//!
//! - **Ordering of later mutations.** While a request is parked the worker
//!   serves only [`served_during_lease`] requests, which neither read nor write
//!   state the fenced edits change. The first other request is held in one
//!   slot and the worker stops reading the FIFO, so it and everything behind it
//!   run after the parked request, in submission order. No project mutation
//!   interleaves a pending lease (the sole-worker rule).
//! - **Bound.** A deferral never outlives [`FENCE_DEADLINE`] from its begin; on
//!   expiry the lease is released and the request refuses as before. The audio
//!   callback never wakes the worker: resolution is observed by polling.
//! - **Cancellation and project replacement.** Shutdown, or a newer accepted
//!   document intent (a queued New/Open/audio restart), releases the lease at
//!   the next poll and the parked request refuses as cancelled, so replacement
//!   is never delayed by a stalled callback. The held request runs next,
//!   preserving FIFO order. Worker exit releases a parked lease.
//!
//! In-place bounce adoption fences only after its render, which a deferral
//! would have to repeat, so it keeps the bounded synchronous poll.
use super::*;
use std::time::{Duration, Instant};

/// How long a lease may wait for a live callback's fence before refusing. The
/// callback never wakes the worker (no scheduling on the audio thread), so the
/// worker polls; this bounds both a deferral and the synchronous poll in-place
/// bounce adoption still uses (#3917).
const FENCE_DEADLINE: Duration = Duration::from_millis(100);
pub(super) const FENCE_POLL: Duration = Duration::from_millis(2);
const NOT_ACKNOWLEDGED: &str = "audio output did not acknowledge the arrangement edit; try again";
const CANCELLED: &str = "arrangement edit cancelled";

/// A lease outcome resolved before dispatch, consumed by [`acquire_position`].
pub(super) struct Prefence {
    result: Result<(), SessionError>,
    waited: Duration,
}
impl Prefence {
    /// Time the request spent parked before dispatch, reported as its own
    /// `parked` section phase outside the dispatched worker time.
    pub(super) fn waited(&self) -> Duration {
        self.waited
    }
}

enum State {
    Pending {
        request: Box<QueuedRequest>,
        source: hex_engine::AudioSourceContext,
        begun: Instant,
    },
    Resolved {
        source: hex_engine::AudioSourceContext,
        prefence: Prefence,
    },
}

/// Worker-owned deferral of at most one parked and one held request.
#[derive(Default)]
pub(super) struct Fence {
    state: Option<State>,
    held: Option<QueuedRequest>,
}

/// Requests that may run while a fenced request is parked. They write no
/// project or transport state and read only device state, which the fenced
/// edits never change. A served request is still checked against its own
/// captured revision, which the parked edit has not advanced yet, so it runs
/// against exactly the state its sender observed.
pub(super) fn served_during_lease(request: &Request) -> bool {
    matches!(
        request,
        Request::SaveDevicePreset { .. } | Request::SaveDeviceDefault { .. }
    )
}

impl Fence {
    /// Begin the lease for a request whose handler fences first. Returns the
    /// request to dispatch now (answered, unavailable or already resolved), or
    /// `None` when it was parked for a later loop turn.
    pub(super) fn defer(
        &mut self,
        sink: Option<&mut dyn WorkerAudioSink>,
        queued: QueuedRequest,
        source: hex_engine::AudioSourceContext,
        recover: bool,
    ) -> Option<QueuedRequest> {
        // A resolved request re-dispatches through here once; never re-begin.
        if self.state.is_some() {
            return Some(queued);
        }
        let Some(sink) = sink else {
            return Some(queued);
        };
        // A refused begin keeps the former path: the handler begins again and
        // reports the identical refusal.
        let begun = if recover {
            sink.begin_recoverable_section_edit(source)
        } else {
            sink.begin_section_edit(source, true)
        };
        if begun.is_err() {
            return Some(queued);
        }
        let begun = Instant::now();
        match sink.section_edit_ready() {
            Some(result) => {
                self.resolve(sink, source, result.map(|_| ()), begun);
                Some(queued)
            }
            None => {
                self.state = Some(State::Pending {
                    request: Box::new(queued),
                    source,
                    begun,
                });
                None
            }
        }
    }

    fn resolve(
        &mut self,
        sink: &mut dyn WorkerAudioSink,
        source: hex_engine::AudioSourceContext,
        result: Result<(), String>,
        begun: Instant,
    ) {
        let result = result.map_err(|reason| {
            sink.cancel_section_edit();
            SessionError::NativeAudio(reason)
        });
        self.state = Some(State::Resolved {
            source,
            prefence: Prefence {
                result,
                waited: begun.elapsed(),
            },
        });
    }

    fn refuse(
        &mut self,
        sink: Option<&mut dyn WorkerAudioSink>,
        source: hex_engine::AudioSourceContext,
        reason: &'static str,
        begun: Instant,
    ) {
        if let Some(sink) = sink {
            sink.cancel_section_edit();
        }
        self.state = Some(State::Resolved {
            source,
            prefence: Prefence {
                result: Err(SessionError::Invalid(reason)),
                waited: begun.elapsed(),
            },
        });
    }

    /// Whether a parked or held request must run before the FIFO is read.
    pub(super) fn active(&self) -> bool {
        matches!(self.state, Some(State::Pending { .. })) || self.held.is_some()
    }

    /// One request-loop turn while a request is parked or held. `Some` is the
    /// next request to run; `None` means poll again. `fifo` is read at most
    /// once, and never while a request is held.
    pub(super) fn next(
        &mut self,
        mut sink: Option<&mut dyn WorkerAudioSink>,
        shutdown: bool,
        intent_changed: impl Fn(&QueuedRequest) -> bool,
        fifo: impl FnOnce() -> Option<QueuedRequest>,
    ) -> Option<QueuedRequest> {
        let Some(State::Pending { request, begun, .. }) = &self.state else {
            return self.held.take();
        };
        let resolution = if shutdown || intent_changed(request) {
            Err(CANCELLED)
        } else if let Some(result) = sink.as_deref_mut().and_then(|s| s.section_edit_ready()) {
            Ok(result.map(|_| ()))
        } else if begun.elapsed() >= FENCE_DEADLINE {
            Err(NOT_ACKNOWLEDGED)
        } else {
            if self.held.is_none() {
                // Serve unrelated work; hold the first other request.
                let queued = fifo()?;
                if served_during_lease(&queued.request) {
                    return Some(queued);
                }
                self.held = Some(queued);
            }
            return None;
        };
        let Some(State::Pending {
            request,
            source,
            begun,
        }) = self.state.take()
        else {
            return None;
        };
        match (resolution, sink) {
            (Ok(result), Some(sink)) => self.resolve(sink, source, result, begun),
            (Ok(_), None) => self.refuse(None, source, NOT_ACKNOWLEDGED, begun),
            (Err(reason), sink) => self.refuse(sink, source, reason, begun),
        }
        Some(*request)
    }

    /// The outcome resolved for `source`, consumed by its fenced handler.
    pub(super) fn take(&mut self, source: hex_engine::AudioSourceContext) -> Option<Prefence> {
        match self.state.take() {
            Some(State::Resolved {
                source: resolved,
                prefence,
            }) if resolved == source => Some(prefence),
            other => {
                self.state = other;
                None
            }
        }
    }

    /// After each request: release a resolved lease its handler did not
    /// consume (it refused before fencing). A parked request stays parked.
    pub(super) fn finish_request(&mut self, sink: Option<&mut dyn WorkerAudioSink>) {
        if let Some(State::Resolved { prefence, .. }) = &self.state {
            if prefence.result.is_ok()
                && let Some(sink) = sink
            {
                sink.cancel_section_edit();
            }
            self.state = None;
        }
    }

    /// Worker exit: release a parked lease; its request is not answered.
    pub(super) fn close(&mut self, sink: Option<&mut dyn WorkerAudioSink>) {
        if matches!(self.state, Some(State::Pending { .. }))
            && let Some(sink) = sink
        {
            sink.cancel_section_edit();
        }
        self.state = None;
        self.held = None;
    }
}

/// One callback-observed lease; section edits and in-place bounces (with
/// their history) may allow playing. A `prefence` resolved by [`Fence`]
/// answers without waiting; otherwise the lease begins here and a live but
/// silent callback is polled for at most [`FENCE_DEADLINE`].
pub(super) fn acquire_position(
    sink: &mut dyn WorkerAudioSink,
    context: hex_engine::AudioSourceContext,
    shutdown: &AtomicBool,
    cancel: Option<&hex_export::ExportControl>,
    allow_playing: bool,
    prefence: Option<Prefence>,
) -> Result<ArrangementEditPosition, SessionError> {
    match prefence {
        Some(Prefence {
            result: Err(error), ..
        }) => Err(error),
        Some(Prefence { result: Ok(()), .. }) => match sink.section_edit_ready() {
            Some(result) => result.map_err(|reason| {
                sink.cancel_section_edit();
                SessionError::NativeAudio(reason)
            }),
            None => {
                sink.cancel_section_edit();
                Err(SessionError::Invalid(NOT_ACKNOWLEDGED))
            }
        },
        None => acquire_position_within(
            sink,
            context,
            shutdown,
            cancel,
            allow_playing,
            FENCE_DEADLINE,
        ),
    }
}

fn acquire_position_within(
    sink: &mut dyn WorkerAudioSink,
    context: hex_engine::AudioSourceContext,
    shutdown: &AtomicBool,
    cancel: Option<&hex_export::ExportControl>,
    allow_playing: bool,
    deadline: Duration,
) -> Result<ArrangementEditPosition, SessionError> {
    sink.begin_section_edit(context, allow_playing)
        .map_err(SessionError::NativeAudio)?;
    let deadline = Instant::now() + deadline;
    loop {
        if let Some(result) = sink.section_edit_ready() {
            return result.map_err(|reason| {
                sink.cancel_section_edit();
                SessionError::NativeAudio(reason)
            });
        }
        if shutdown.load(Ordering::Acquire) || cancel.is_some_and(|c| c.is_cancelled()) {
            sink.cancel_section_edit();
            return Err(SessionError::Invalid(CANCELLED));
        }
        if Instant::now() >= deadline {
            sink.cancel_section_edit();
            return Err(SessionError::Invalid(NOT_ACKNOWLEDGED));
        }
        // No project mutation can interleave on this sole worker. Device and UI
        // threads remain independent; there is no async runtime or callback wait.
        thread::sleep(FENCE_POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A live output whose callback never acknowledges the fence.
    #[derive(Default)]
    struct SilentCallback {
        begun: bool,
        cancelled: bool,
    }
    impl WorkerAudioSink for SilentCallback {
        fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
            Ok(48_000)
        }
        fn canonical_changed(&mut self, _: Arc<CanonicalAudioSourceSnapshot>) {}
        fn poll(&mut self) {}
        fn close(&mut self) {}
        fn begin_section_edit(
            &mut self,
            _: hex_engine::AudioSourceContext,
            _: bool,
        ) -> Result<(), String> {
            self.begun = true;
            Ok(())
        }
        fn section_edit_ready(&mut self) -> Option<Result<ArrangementEditPosition, String>> {
            None
        }
        fn cancel_section_edit(&mut self) {
            self.cancelled = true;
        }
    }

    const CONTEXT: hex_engine::AudioSourceContext = hex_engine::AudioSourceContext {
        project_generation: 0,
        editor_revision: 0,
    };

    #[test]
    fn silent_callback_refuses_within_the_fence_deadline_and_releases_the_lease() {
        let mut sink = SilentCallback::default();
        let started = Instant::now();
        let error = acquire_position(
            &mut sink,
            CONTEXT,
            &AtomicBool::new(false),
            None,
            true,
            None,
        )
        .expect_err("no acknowledgement");
        let elapsed = started.elapsed();
        assert!(
            matches!(error, SessionError::Invalid(message) if message.contains("did not acknowledge"))
        );
        assert!(elapsed >= FENCE_DEADLINE);
        // Bounded far below the former 1 s worker freeze, with scheduling slack.
        assert!(elapsed < Duration::from_millis(500), "{elapsed:?}");
        assert!(sink.begun && sink.cancelled);
    }

    #[test]
    fn shutdown_refuses_without_waiting_for_the_deadline() {
        let mut sink = SilentCallback::default();
        let started = Instant::now();
        let error = acquire_position(&mut sink, CONTEXT, &AtomicBool::new(true), None, true, None)
            .expect_err("shutdown");
        assert!(matches!(
            error,
            SessionError::Invalid("arrangement edit cancelled")
        ));
        assert!(started.elapsed() < FENCE_DEADLINE);
        assert!(sink.cancelled);
    }

    fn queued(request: Request, intent: u64) -> QueuedRequest {
        QueuedRequest::continuation(request, &std::sync::atomic::AtomicU64::new(intent))
    }

    #[test]
    fn project_replacement_releases_a_parked_lease_without_waiting() {
        let mut sink = SilentCallback::default();
        let mut fence = Fence::default();
        assert!(
            fence
                .defer(Some(&mut sink), queued(Request::Undo, 1), CONTEXT, false)
                .is_none(),
            "a silent callback parks the request"
        );
        assert!(fence.active() && sink.begun && !sink.cancelled);
        // A later mutation is held; the FIFO is not read again while it waits.
        assert!(
            fence
                .next(
                    Some(&mut sink),
                    false,
                    |_| false,
                    || Some(queued(Request::Redo, 1))
                )
                .is_none()
        );
        let started = Instant::now();
        // A queued New/Open advanced the accepted document intent.
        let parked = fence
            .next(
                Some(&mut sink),
                false,
                |parked| parked.document_intent != 2,
                || panic!("held request blocks the fifo"),
            )
            .expect("parked request re-dispatches");
        assert!(matches!(parked.request, Request::Undo));
        assert!(started.elapsed() < FENCE_DEADLINE);
        assert!(sink.cancelled, "the lease is released");
        let error = acquire_position(
            &mut sink,
            CONTEXT,
            &AtomicBool::new(false),
            None,
            true,
            fence.take(CONTEXT),
        )
        .expect_err("cancelled");
        assert!(matches!(error, SessionError::Invalid(CANCELLED)));
        fence.finish_request(Some(&mut sink));
        // The held request runs next, before any newer FIFO request.
        let held = fence
            .next(Some(&mut sink), false, |_| false, || panic!("fifo"))
            .expect("held request");
        assert!(matches!(held.request, Request::Redo));
        assert!(!fence.active());
    }

    #[test]
    fn unconsumed_acknowledged_lease_is_released_after_its_request() {
        let mut sink = Immediate::default();
        let mut fence = Fence::default();
        let request = fence
            .defer(Some(&mut sink), queued(Request::Undo, 0), CONTEXT, false)
            .expect("an immediate answer dispatches at once");
        assert!(matches!(request.request, Request::Undo));
        assert!(
            fence
                .take(hex_engine::AudioSourceContext {
                    editor_revision: 9,
                    ..CONTEXT
                })
                .is_none(),
            "another source never consumes the outcome"
        );
        fence.finish_request(Some(&mut sink));
        assert!(
            sink.0.cancelled,
            "a handler that refused early never leaks the lease"
        );
        assert!(fence.take(CONTEXT).is_none());
    }

    #[derive(Default)]
    struct Immediate(SilentCallback);
    impl WorkerAudioSink for Immediate {
        fn initialize(&mut self, s: &AtomicBool) -> Result<u32, String> {
            self.0.initialize(s)
        }
        fn canonical_changed(&mut self, s: Arc<CanonicalAudioSourceSnapshot>) {
            self.0.canonical_changed(s);
        }
        fn poll(&mut self) {}
        fn close(&mut self) {}
        fn begin_section_edit(
            &mut self,
            _: hex_engine::AudioSourceContext,
            _: bool,
        ) -> Result<(), String> {
            Ok(())
        }
        fn section_edit_ready(&mut self) -> Option<Result<ArrangementEditPosition, String>> {
            Some(ArrangementEditPosition::from_frames(7, false, None))
        }
        fn cancel_section_edit(&mut self) {
            self.0.cancelled = true;
        }
    }

    #[test]
    fn immediate_answer_completes_without_polling() {
        let mut sink = Immediate::default();
        let started = Instant::now();
        let position = acquire_position_within(
            &mut sink,
            CONTEXT,
            &AtomicBool::new(false),
            None,
            false,
            Duration::ZERO,
        )
        .expect("immediate position");
        assert_eq!((position.frame, position.playing), (7, false));
        assert!(started.elapsed() < Duration::from_millis(10));
    }
}
