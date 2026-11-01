//! Existing callback-observed stopped Arrangement lease shared by structural
//! edits and in-place bounces. No runtime, source or resource custody lives here.
use super::*;
use std::time::{Duration, Instant};
/// One callback-observed lease; section edits and in-place bounces (with
/// their history) may allow playing.
pub(super) fn acquire_position(
    sink: &mut dyn WorkerAudioSink,
    context: hex_engine::AudioSourceContext,
    shutdown: &AtomicBool,
    cancel: Option<&hex_export::ExportControl>,
    allow_playing: bool,
) -> Result<ArrangementEditPosition, SessionError> {
    sink.begin_section_edit(context, allow_playing)
        .map_err(SessionError::NativeAudio)?;
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        if shutdown.load(Ordering::Acquire)
            || cancel.is_some_and(|c| c.is_cancelled())
            || Instant::now() >= deadline
        {
            sink.cancel_section_edit();
            return Err(SessionError::Invalid(
                "arrangement edit cancelled or timed out",
            ));
        }
        if let Some(result) = sink.section_edit_ready() {
            return result.map_err(|reason| {
                sink.cancel_section_edit();
                SessionError::NativeAudio(reason)
            });
        }
        // No project mutation can interleave on this sole worker. Device and UI
        // threads remain independent; there is no async runtime or callback wait.
        thread::sleep(Duration::from_millis(2));
    }
}
