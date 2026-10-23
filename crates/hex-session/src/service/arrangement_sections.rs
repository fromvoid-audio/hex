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
            | Command::SplitArrangementSection { .. }
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

#[cfg(test)]
mod split_tests {
    use super::*;
    fn next(service: &Service) -> Event {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(event) = service.try_event() {
                return event;
            }
            assert!(Instant::now() < deadline, "section split worker completion");
            thread::sleep(Duration::from_millis(2));
        }
    }
    #[test]
    fn stamped_marker_split_refuses_stale_requests_and_preserves_atomic_history() {
        let service = Service::new(48_000, 8, 0).unwrap();
        let initial = next(&service);
        service
            .submit(Request::EditArrangementSection {
                project_generation: 0,
                revision: initial.state.revision,
                command: Command::AddArrangementSection {
                    name: "Original Name".into(),
                    color: hex_project::ElementColor::Rose,
                    start: hex_project::BeatTime::ZERO,
                    end: hex_project::BeatTime::from_quarters(4).unwrap(),
                },
            })
            .unwrap();
        let added = next(&service);
        assert!(added.error.is_none());
        let Some(Created::ArrangementSection(section)) = added.created else {
            panic!("section")
        };
        let before = added.state.project.snapshot();
        let command = Command::SplitArrangementSection {
            section,
            at: hex_project::BeatTime::from_quarters(2).unwrap(),
        };
        for (generation, revision) in [(1, added.state.revision), (0, initial.state.revision)] {
            service
                .submit(Request::EditArrangementSection {
                    project_generation: generation,
                    revision,
                    command: command.clone(),
                })
                .unwrap();
            let refused = next(&service);
            assert!(refused.error.is_some());
            assert_eq!(refused.state.project.snapshot(), before);
            assert_eq!(refused.state.revision, added.state.revision);
        }
        service
            .submit(Request::EditArrangementSection {
                project_generation: 0,
                revision: added.state.revision,
                command,
            })
            .unwrap();
        let split = next(&service);
        assert!(split.error.is_none(), "{:?}", split.error);
        let Some(Created::ArrangementSection(right)) = split.created else {
            panic!("split")
        };
        assert_ne!(right, section);
        let after = split.state.project.snapshot();
        service.submit(Request::Undo).unwrap();
        let undone = next(&service);
        assert!(undone.error.is_none());
        let mut snapshot = undone.state.project.snapshot();
        snapshot.next_id = before.next_id;
        assert_eq!(snapshot, before);
        service.submit(Request::Redo).unwrap();
        let redone = next(&service);
        assert!(redone.error.is_none());
        assert_eq!(redone.state.project.snapshot(), after);
    }
}
