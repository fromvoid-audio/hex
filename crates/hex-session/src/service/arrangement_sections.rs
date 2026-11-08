//! Revision-bound structural intent and the existing canonical resident transition.
//! The short, bounded native handshake runs only on the control worker. It never
//! waits for UI delivery, and priority Stop remains on the native command path.
use super::diagnostics::SectionPhases;
use super::*;
use std::time::{Duration, Instant};

pub(super) fn is_command(command: &Command) -> bool {
    matches!(
        command,
        Command::AddArrangementSection { .. }
            | Command::SetArrangementSectionRange { .. }
            | Command::MoveArrangementSectionMarker { .. }
            | Command::ResizeArrangementSectionMarker { .. }
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
            | Command::DuplicateArrangementSectionAt { .. }
            | Command::AppendArrangementSectionCopy { .. }
            | Command::MoveArrangementSectionBeyondEnd { .. }
            | Command::SplitArrangementSection { .. }
            | Command::MergeArrangementSections(_)
    )
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
    let structural = command.requires_section_publication();
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
        if events.resident_publication_enabled()
            && session.audio_mode() == DocumentAudioMode::Resident
            && let Some(current) = resident.as_ref()
        {
            return session.section_noop_revision(command, current);
        }
        return session.command_prepared_revision(command, previous);
    }
    if !structural {
        // Marker-only edits take no callback lease; fence stays zero.
        let mut phases = SectionPhases::default();
        if session.audio_mode() == DocumentAudioMode::Resident
            && let Some(sink) = events.resident_audio()
            && let Some(current) = resident.as_mut()
        {
            let result = session
                .section_marker_transition(
                    Some(command),
                    false,
                    context,
                    sink,
                    admission,
                    current,
                    &mut phases,
                )
                .and_then(|result| {
                    result.ok_or(SessionError::Invalid("section edit produced no candidate"))
                });
            events.record_section_phases(phases);
            return result;
        }
        let result = resident_audio::execute_command_timed(
            session,
            command,
            previous,
            context,
            events,
            resident,
            admission,
            Some(&mut phases),
        );
        events.record_section_phases(phases);
        return result;
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
    // Every section edit may run while playing: the callback's live handoff
    // restarts audio at the cursor when history cannot carry over (#3784).
    fenced_transition(
        session,
        Some(command),
        false,
        context,
        events,
        resident,
        admission,
    )?
    .ok_or(SessionError::Invalid("section edit produced no candidate"))
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
    Ok(
        fenced_transition(session, None, redo, context, events, resident, admission)?
            .map(|(_, prepared)| prepared),
    )
}
/// Undo/Redo of a marker-only section step: republish without preparation.
pub(super) fn marker_history(
    session: &mut Session,
    redo: bool,
    context: hex_engine::AudioSourceContext,
    events: &mut WorkerEvents,
    resident: &mut Option<resident_audio::Snapshot>,
    admission: sampler_export::Admission<'_>,
) -> Result<Option<Arc<PreparedRevision>>, SessionError> {
    let sink = events
        .resident_audio()
        .ok_or(SessionError::Invalid("native worker unavailable"))?;
    let current = resident
        .as_mut()
        .ok_or(SessionError::Invalid("resident audio unavailable"))?;
    let mut phases = SectionPhases::default();
    let result = session.section_marker_transition(
        None,
        redo,
        context,
        sink,
        admission,
        current,
        &mut phases,
    );
    events.record_section_phases(phases);
    Ok(result?.map(|(_, prepared)| prepared))
}
/// One callback-fenced resident transition for an edit or its history step.
/// The lease is cancelled unless a candidate commits; worker phases are
/// recorded for the request's diagnostic outcome, including failures.
#[allow(clippy::too_many_arguments)]
fn fenced_transition(
    session: &mut Session,
    command: Option<Command>,
    redo: bool,
    context: hex_engine::AudioSourceContext,
    events: &mut WorkerEvents,
    resident: &mut Option<resident_audio::Snapshot>,
    admission: sampler_export::Admission<'_>,
) -> Result<Option<(Created, Arc<PreparedRevision>)>, SessionError> {
    let shutdown = Arc::clone(&session.shutdown);
    let source_context = hex_engine::AudioSourceContext {
        editor_revision: context.editor_revision.saturating_sub(1),
        ..context
    };
    // A lease the worker resolved before dispatch, including its parked time.
    let prefence = events.take_stopped_fence(source_context);
    let parked = prefence
        .as_ref()
        .map_or(Duration::ZERO, super::stopped_arrangement::Prefence::waited);
    let sink = events
        .resident_audio()
        .ok_or(SessionError::Invalid("native worker unavailable"))?;
    let mut phases = SectionPhases::default();
    let fence = Instant::now();
    let position = super::stopped_arrangement::acquire_position(
        sink,
        source_context,
        &shutdown,
        None,
        true,
        prefence,
    );
    phases.fence = fence.elapsed();
    phases.parked = parked;
    // A refused or timed-out lease is already cancelled by `acquire_position`.
    let committed = position.and_then(|position| {
        match session.section_snapshot(
            command,
            redo,
            position,
            context,
            sink,
            admission,
            resident.as_ref(),
            &mut phases,
        ) {
            Ok(Some((created, snapshot))) => {
                let prepared = Arc::clone(&snapshot.prepared);
                *resident = Some(snapshot);
                Ok(Some((created, prepared)))
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
    });
    events.record_section_phases(phases);
    committed
}

#[cfg(test)]
mod split_tests {
    use super::*;
    fn next(service: &Service) -> Event {
        // Blocks on the event channel; bounded only against hangs.
        match service.events.recv_timeout(crate::test_wait::HANG_BOUND) {
            Ok(event) => event,
            Err(error) => panic!("next: worker event absent: {error}"),
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

#[cfg(test)]
mod append_tests {
    use super::*;

    fn next(service: &Service) -> Event {
        service
            .events
            .recv_timeout(crate::test_wait::HANG_BOUND)
            .unwrap()
    }

    #[test]
    fn stamped_absolute_placements_use_structural_preparation_and_one_history_edge() {
        for moving in [false, true] {
            let service = Service::new(48_000, 8, 0).unwrap();
            let initial = next(&service);
            service
                .submit(Request::EditArrangementSection {
                    project_generation: 0,
                    revision: initial.state.revision,
                    command: Command::AddArrangementSection {
                        name: "source".into(),
                        color: hex_project::ElementColor::Blue,
                        start: hex_project::BeatTime::ZERO,
                        end: hex_project::BeatTime::from_quarters(4).unwrap(),
                    },
                })
                .unwrap();
            let added = next(&service);
            let Some(Created::ArrangementSection(section)) = added.created else {
                panic!("source");
            };
            let before = added.state.project.snapshot();
            let start = hex_project::BeatTime::from_quarters(8).unwrap();
            let edit = if moving {
                Command::MoveArrangementSectionBeyondEnd {
                    section,
                    start,
                    gap_name: "gap".into(),
                    gap_color: hex_project::ElementColor::Rose,
                }
            } else {
                Command::AppendArrangementSectionCopy {
                    section,
                    start,
                    gap_name: "gap".into(),
                    gap_color: hex_project::ElementColor::Rose,
                }
            };
            assert!(is_command(&edit));
            assert!(edit.requires_section_publication());
            service
                .submit(Request::EditArrangementSection {
                    project_generation: 0,
                    revision: added.state.revision,
                    command: edit,
                })
                .unwrap();
            let copied = next(&service);
            assert!(copied.error.is_none(), "{:?}", copied.error);
            assert_eq!(
                copied.state.project.arrangement_sections().len(),
                if moving { 2 } else { 3 }
            );
            if moving {
                assert_eq!(copied.created, Some(Created::None));
                assert_eq!(
                    copied
                        .state
                        .project
                        .arrangement_section(section)
                        .unwrap()
                        .start(),
                    start
                );
            }
            let after = copied.state.project.snapshot();
            service.submit(Request::Undo).unwrap();
            let mut restored = next(&service).state.project.snapshot();
            assert!(restored.next_id >= after.next_id);
            restored.next_id = before.next_id;
            assert_eq!(restored, before);
            service.submit(Request::Redo).unwrap();
            assert_eq!(next(&service).state.project.snapshot(), after);
        }
    }
}
