//! Existing callback-observed stopped Arrangement lease shared by structural
//! edits and in-place bounces. No runtime, source or resource custody lives here.
use super::*;
use std::time::{Duration, Instant};

/// How long the sole worker polls for a live callback's fence before refusing.
/// The callback never wakes the worker (no scheduling on the audio thread), so
/// this bounded poll is the only wait; it also bounds how long a stalled
/// callback can hold every other session command (#3917).
const FENCE_DEADLINE: Duration = Duration::from_millis(100);
const FENCE_POLL: Duration = Duration::from_millis(2);

/// One callback-observed lease; section edits and in-place bounces (with
/// their history) may allow playing. A sink without a live callback answers
/// `section_edit_ready` immediately; only a live but silent callback waits.
pub(super) fn acquire_position(
    sink: &mut dyn WorkerAudioSink,
    context: hex_engine::AudioSourceContext,
    shutdown: &AtomicBool,
    cancel: Option<&hex_export::ExportControl>,
    allow_playing: bool,
) -> Result<ArrangementEditPosition, SessionError> {
    acquire_position_within(
        sink,
        context,
        shutdown,
        cancel,
        allow_playing,
        FENCE_DEADLINE,
    )
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
            return Err(SessionError::Invalid("arrangement edit cancelled"));
        }
        if Instant::now() >= deadline {
            sink.cancel_section_edit();
            return Err(SessionError::Invalid(
                "audio output did not acknowledge the arrangement edit; try again",
            ));
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
        let error = acquire_position(&mut sink, CONTEXT, &AtomicBool::new(false), None, true)
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
        let error = acquire_position(&mut sink, CONTEXT, &AtomicBool::new(true), None, true)
            .expect_err("shutdown");
        assert!(matches!(
            error,
            SessionError::Invalid("arrangement edit cancelled")
        ));
        assert!(started.elapsed() < FENCE_DEADLINE);
        assert!(sink.cancelled);
    }

    #[test]
    fn immediate_answer_completes_without_polling() {
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
        }
        let mut sink = Immediate(SilentCallback::default());
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
