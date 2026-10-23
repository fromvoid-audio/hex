//! Revision-bound structural intent and the existing canonical resident transition.
//! The short, bounded native handshake runs only on the control worker. It never
//! waits for UI delivery, and priority Stop remains on the native command path.
use super::*;
use std::time::{Duration, Instant};

pub(super) fn is_command(command: &Command) -> bool {
    matches!(
        command,
        Command::AddArrangementSection { .. }
            | Command::SetArrangementSectionRange { .. }
            | Command::RenameArrangementSection { .. }
            | Command::SetArrangementSectionColor { .. }
            | Command::RemoveArrangementSection(_)
            | Command::ReorderSectionBefore { .. }
            | Command::ReorderSectionAfter { .. }
            | Command::ResizeArrangementSection { .. }
            | Command::ResizeArrangementSectionEdge { .. }
            | Command::ResizeArrangementSectionBoundary { .. }
            | Command::DeleteArrangementSection(_)
            | Command::DuplicateArrangementSection(_)
            | Command::MergeArrangementSections(_)
    )
}
fn stopped_frame(
    sink: &mut dyn WorkerAudioSink,
    context: hex_engine::AudioSourceContext,
    shutdown: &AtomicBool,
) -> Result<u64, SessionError> {
    sink.begin_section_edit(context)
        .map_err(SessionError::NativeAudio)?;
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        if shutdown.load(Ordering::Acquire) || Instant::now() >= deadline {
            sink.cancel_section_edit();
            return Err(SessionError::Invalid(
                "stopped section edit cancelled or timed out",
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
#[allow(clippy::too_many_arguments)]
pub(super) fn command(
    session: &mut Session,
    command: Command,
    previous: Option<&PreparedRevision>,
    context: hex_engine::AudioSourceContext,
    events: &mut WorkerEvents,
    resident: &mut Option<resident_audio::Snapshot>,
    admission: sampler_export::Admission<'_>,
) -> Result<(Created, Arc<PreparedRevision>), SessionError> {
    let structural = command.requires_stopped_section_edit();
    let no_op = match &command {
        Command::ReorderSectionBefore { section, before } => session
            .project()
            .preview_section_reorder(*section, *before)?
            .is_noop(),
        Command::ReorderSectionAfter { section, after } => session
            .project()
            .preview_section_reorder_after(*section, *after)?
            .is_noop(),
        _ => false,
    };
    if no_op {
        return session.command_prepared_revision(command, previous);
    }
    if !structural {
        return resident_audio::execute_command(
            session, command, previous, context, events, resident, admission,
        );
    }
    if previous.is_some_and(|prepared| prepared.gain_recording_track().is_some()) {
        return Err(SessionError::Invalid(
            "finish or cancel gain recording before editing song sections",
        ));
    }
    if session.audio_mode() != DocumentAudioMode::Resident {
        return Err(SessionError::Invalid(
            "song section edit requires resident audio",
        ));
    }
    if !events.resident_publication_enabled() {
        if events.native_audio_requested() {
            return Err(SessionError::Invalid(
                "song section edit requires the canonical native publication owner",
            ));
        }
        return session.command_prepared_revision(command, previous);
    }
    let shutdown = Arc::clone(&session.shutdown);
    let sink = events
        .resident_audio()
        .ok_or(SessionError::Invalid("native worker unavailable"))?;
    let source_context = hex_engine::AudioSourceContext {
        editor_revision: context.editor_revision.saturating_sub(1),
        ..context
    };
    let frame = stopped_frame(sink, source_context, &shutdown)?;
    let result = session.stopped_section_snapshot(
        Some(command),
        false,
        frame,
        context,
        sink,
        admission,
        resident.as_ref(),
    );
    match result {
        Ok(Some((created, snapshot))) => {
            let prepared = Arc::clone(&snapshot.prepared);
            *resident = Some(snapshot);
            Ok((created, prepared))
        }
        Ok(None) => {
            sink.cancel_section_edit();
            Err(SessionError::Invalid("section edit produced no candidate"))
        }
        Err(error) => {
            sink.cancel_section_edit();
            Err(error)
        }
    }
}
#[allow(clippy::too_many_arguments)]
pub(super) fn history(
    session: &mut Session,
    redo: bool,
    context: hex_engine::AudioSourceContext,
    events: &mut WorkerEvents,
    resident: &mut Option<resident_audio::Snapshot>,
    admission: sampler_export::Admission<'_>,
) -> Result<Option<Arc<PreparedRevision>>, SessionError> {
    let shutdown = Arc::clone(&session.shutdown);
    let sink = events
        .resident_audio()
        .ok_or(SessionError::Invalid("native worker unavailable"))?;
    let source_context = hex_engine::AudioSourceContext {
        editor_revision: context.editor_revision.saturating_sub(1),
        ..context
    };
    let frame = stopped_frame(sink, source_context, &shutdown)?;
    match session.stopped_section_snapshot(
        None,
        redo,
        frame,
        context,
        sink,
        admission,
        resident.as_ref(),
    ) {
        Ok(Some((_, snapshot))) => {
            let prepared = Arc::clone(&snapshot.prepared);
            *resident = Some(snapshot);
            Ok(Some(prepared))
        }
        Ok(None) => {
            sink.cancel_section_edit();
            Ok(None)
        }
        Err(error) => {
            sink.cancel_section_edit();
            Err(error)
        }
    }
}
