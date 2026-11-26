use super::*;

#[test]
fn arrangement_preroll_reconstructs_held_and_released_mutable_feedback() {
    let (prepared, _) = mutable_checkpoint_fixture(1 << 32);
    let untouched = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    let mut caller = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    caller.apply(SessionPlaybackCommand::Play).unwrap();
    let mut caller_prefix = [[0.0; 2]; 37];
    caller.render(&mut caller_prefix).unwrap();
    let caller_frame = caller.performance_frame();

    // A held note exercises both oscillator/voice continuation and feedback.
    // The later target is past the note gate and exercises the ended-gate tail.
    for target in [256_u64, 24_704] {
        let mut uninterrupted = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
        uninterrupted.apply(SessionPlaybackCommand::Play).unwrap();
        let mut remaining = target;
        let partitions = [19_usize, 64, 7, 127, 43];
        let mut partition_index = 0;
        while remaining != 0 {
            let count =
                remaining.min(partitions[partition_index % partitions.len()] as u64) as usize;
            let mut prefix = vec![[0.0; 2]; count];
            uninterrupted.render(&mut prefix).unwrap();
            remaining -= count as u64;
            partition_index += 1;
        }
        let mut expected = [[0.0; 2]; 193];
        uninterrupted.render(&mut expected).unwrap();

        let candidate = caller
            .prepare_arrangement_preroll(target, target, true, &AtomicBool::new(false))
            .unwrap();
        let mut candidate = Playback::from_arrangement_preroll(candidate).unwrap();
        assert!(Arc::ptr_eq(
            candidate.prepared_revision().expect("prepared revision"),
            &prepared
        ));
        assert_eq!(candidate.position(), target);
        assert_eq!(candidate.performance_frame(), target);
        assert!(candidate.playing());
        let mut actual = [[0.0; 2]; 193];
        candidate.render(&mut actual).unwrap();
        assert_eq!(actual, expected, "preroll continuation differs at {target}");
    }

    assert_eq!(caller.performance_frame(), caller_frame);
    assert!(caller.playing());
    assert_eq!(untouched.performance_frame(), 0);
    assert!(!untouched.playing());
}

#[test]
fn interactive_arrangement_preroll_accepts_session_routes_and_handoff() {
    interactive_preroll_session_command_is_available();
    interactive_preroll_warmed_state_inherits_into_compiled_revision();
}

#[test]
fn arrangement_seek_history_query_preserves_analytic_fast_path() {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    let table = Arc::new(hex_devices::SineTable::new());
    let sine = revision(&project(1.0, 0, 8 << 32), Arc::clone(&table));
    assert!(!sine.arrangement_seek_requires_preroll());

    let operator_state = crate::operator_legacy_test::legacy_operator_state();
    let mut snapshot = project(1.0, 0, 8 << 32).snapshot();
    snapshot.tracks[0].instrument.as_mut().unwrap().state = operator_state;
    let operator = revision(
        &Project::from_legacy_snapshot(snapshot).unwrap(),
        Arc::clone(&table),
    );
    assert!(!operator.arrangement_seek_requires_preroll());

    let compare_analytic_seek = |prepared: Arc<PreparedRevision>| {
        let destination = 24_704;
        let mut uninterrupted = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
        uninterrupted.apply(SessionPlaybackCommand::Play).unwrap();
        let _prefix = render_checkpoint_partitioned(
            &mut uninterrupted,
            &[17, 239, 61, 127],
            destination as usize,
        );
        let mut expected = [[0.0; 2]; 193];
        uninterrupted.render(&mut expected).unwrap();

        let mut seeked = SessionPlayback::new(prepared).unwrap();
        seeked
            .apply(SessionPlaybackCommand::Seek(destination))
            .unwrap();
        seeked.apply(SessionPlaybackCommand::Play).unwrap();
        let mut actual = [[0.0; 2]; 193];
        seeked.render(&mut actual).unwrap();
        assert_eq!(actual, expected);
    };
    compare_analytic_seek(Arc::clone(&sine));
    compare_analytic_seek(Arc::clone(&operator));

    let (mutable, _) = mutable_checkpoint_fixture(8 << 32);
    assert!(mutable.arrangement_seek_requires_preroll());

    let filter_state = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            parameter::DC_REMOVE,
            ParameterValue::Bool(true),
        )
        .unwrap();
    let mut track_snapshot = project(1.0, 0, 8 << 32).snapshot();
    track_snapshot.tracks[0].devices.push(DeviceSnapshot {
        modulation: Default::default(),
        parent: None,
        automation: Vec::new(),
        comparison: None,
        id: track_snapshot.next_id,
        bypassed: false,
        state: filter_state.clone(),
    });
    track_snapshot.next_id += 1;
    let stateful_track = revision(
        &Project::from_legacy_snapshot(track_snapshot).unwrap(),
        Arc::clone(&table),
    );
    assert!(stateful_track.arrangement_seek_requires_preroll());

    let mut return_snapshot = project(1.0, 0, 8 << 32).snapshot();
    let return_id = return_snapshot.next_id;
    let return_device_id = return_id + 1;
    return_snapshot.next_id = return_id + 2;
    return_snapshot.tracks[0]
        .sends
        .push(hex_project::SendLevelSnapshot {
            return_track: return_id,
            gain: 0.5,
        });
    return_snapshot
        .return_tracks
        .push(hex_project::ReturnTrackSnapshot {
            id: return_id,
            name: "return".into(),
            gain: 0.75,
            muted: false,
            devices: vec![DeviceSnapshot {
                modulation: Default::default(),
                parent: None,
                automation: Vec::new(),
                comparison: None,
                id: return_device_id,
                bypassed: false,
                state: filter_state,
            }],
        });
    let stateful_return = revision(
        &Project::from_legacy_snapshot(return_snapshot).unwrap(),
        table,
    );
    assert!(stateful_return.arrangement_seek_requires_preroll());
}

#[test]
fn operator_supported_state_edit_retargets_live_history_and_joins_later_notes() {
    use hex_devices::operator_state::{OPERATOR_METADATA, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    fn edited_state(
        feedback: f32,
        level: f32,
        carrier: f32,
    ) -> hex_lattice_contract::NativeDeviceState {
        let state = crate::operator_legacy_test::legacy_operator_state();
        let state = OPERATOR_METADATA
            .patch_parameter(
                &state,
                parameter::feedback(1),
                ParameterValue::Float(feedback),
            )
            .unwrap();
        let state = OPERATOR_METADATA
            .patch_parameter(
                &state,
                parameter::oscillator_level(1),
                ParameterValue::Float(level),
            )
            .unwrap();
        OPERATOR_METADATA
            .patch_parameter(
                &state,
                parameter::carrier(1),
                ParameterValue::Float(carrier),
            )
            .unwrap()
    }

    fn run(
        partition: &[usize],
        session_route: bool,
        apply_intermediate: bool,
    ) -> Vec<crate::Frame> {
        let table = Arc::new(hex_devices::SineTable::new());
        let mut snapshot = project(1.0, 0, 8 << 32).snapshot();
        snapshot.tracks[0].instrument.as_mut().unwrap().state = edited_state(0.15, 0.45, 0.3);
        let arrangement = match &mut snapshot.tracks[0].content {
            TrackContentSnapshot::Instrument(clips) => clips,
            TrackContentSnapshot::Hybrid { midi_clips, .. } => midi_clips,
            _ => panic!("fixture has MIDI arrangement content"),
        };
        arrangement[0].notes.push(MidiNoteSnapshot {
            active: true,
            id: 7,
            start_raw: 1 << 24,
            duration_raw: 1 << 32,
            pitch: 67,
            channel: 0,
            velocity: 88,
            expression: Default::default(),
            expression_points: Vec::new(),
        });
        let Some(session_cell) = snapshot.session.cells.first_mut() else {
            panic!("fixture has Session MIDI content");
        };
        let SessionCellContentSnapshot::Midi(session_clip) = &mut session_cell.content else {
            panic!("fixture has Session MIDI content");
        };
        session_clip.source.notes.push(MidiNoteSnapshot {
            active: true,
            id: snapshot.next_id,
            start_raw: 1 << 24,
            duration_raw: 1 << 32,
            pitch: 67,
            channel: 0,
            velocity: 88,
            expression: Default::default(),
            expression_points: Vec::new(),
        });
        snapshot.next_id += 1;
        let source_project = Project::from_legacy_snapshot(snapshot).unwrap();
        let old_revision = Arc::new(
            PreparedRevision::prepare(&source_project, &BTreeMap::new(), Arc::clone(&table), None)
                .unwrap(),
        );
        let track_id = old_revision.arrangement().tracks()[0]
            .canonical_track_id()
            .unwrap();
        assert!(
            old_revision
                .session()
                .mutable_instrument_factory(track_id)
                .is_some()
        );

        let instrument_id = source_project.tracks()[0].instrument().unwrap().id();
        let mut intermediate_editor = Editor::new(source_project.clone(), 4).unwrap();
        intermediate_editor
            .execute(Command::ReplaceDeviceState {
                device: instrument_id,
                state: edited_state(0.48, 0.7, 0.6),
            })
            .unwrap();
        let intermediate_project = intermediate_editor.project().clone();
        let intermediate_revision = Arc::new(
            PreparedRevision::prepare(
                &intermediate_project,
                &BTreeMap::new(),
                Arc::clone(&table),
                Some(old_revision.session()),
            )
            .unwrap(),
        );
        assert!(
            intermediate_revision
                .session()
                .mutable_history_retarget(track_id)
                .is_some()
        );

        let mut final_editor = Editor::new(intermediate_project, 4).unwrap();
        final_editor
            .execute(Command::ReplaceDeviceState {
                device: instrument_id,
                state: edited_state(0.75, 0.9, 0.8),
            })
            .unwrap();
        // Deliberately prepare C from canonical B. The audio owner may still
        // have A, so C's target proof must be applicable to the actual runtime
        // rather than being chained to the most recently prepared revision.
        let candidate_revision = Arc::new(
            PreparedRevision::prepare(
                final_editor.project(),
                &BTreeMap::new(),
                Arc::clone(&table),
                Some(intermediate_revision.session()),
            )
            .unwrap(),
        );
        assert!(
            candidate_revision
                .session()
                .mutable_history_retarget(track_id)
                .is_some()
        );
        assert_eq!(
            candidate_revision
                .session()
                .mutable_history_retarget_storage_bytes(),
            std::mem::size_of::<u64>(),
            "the inventory separately accounts the exact candidate proof"
        );

        let mut previous = SessionPlayback::new(old_revision).unwrap();
        if session_route {
            let clip = previous.revision.session().midi_clips()[0].id;
            previous
                .apply(SessionPlaybackCommand::LaunchClip {
                    clip,
                    quantization: LaunchQuantization::Immediate,
                })
                .unwrap();
        }
        previous.apply(SessionPlaybackCommand::Play).unwrap();
        let mut prefix = [[0.0; 2]; 64];
        previous.render(&mut prefix).unwrap();
        assert!(prefix.iter().any(|frame| frame[0].abs() > 1.0e-6));

        let mut successor = if apply_intermediate {
            let mut intermediate = SessionPlayback::new(intermediate_revision).unwrap();
            intermediate.try_inherit_state_from(&mut previous).unwrap();
            let mut ramp_prefix = [[0.0; 2]; 7];
            intermediate.render(&mut ramp_prefix).unwrap();
            SessionPlayback::new(candidate_revision)
                .and_then(|mut final_candidate| {
                    final_candidate.try_inherit_state_from(&mut intermediate)?;
                    Ok(final_candidate)
                })
                .unwrap()
        } else {
            let mut final_candidate = SessionPlayback::new(candidate_revision).unwrap();
            final_candidate
                .try_inherit_state_from(&mut previous)
                .unwrap();
            final_candidate
        };
        let mut suffix = Vec::new();
        let mut index = 0;
        while suffix.len() < 384 {
            let count = partition[index % partition.len()].min(384 - suffix.len());
            let mut block = vec![[0.0; 2]; count];
            successor.render(&mut block).unwrap();
            suffix.extend(block);
            index += 1;
        }
        assert!(suffix.iter().any(|frame| frame[0].abs() > 1.0e-6));
        suffix
    }

    for session_route in [false, true] {
        for apply_intermediate in [false, true] {
            let single_frame = run(&[1], session_route, apply_intermediate);
            assert_eq!(
                run(&[37, 128, 19, 64], session_route, apply_intermediate),
                single_frame
            );
        }
    }
}

#[test]
fn unsupported_operator_state_edit_waits_for_committed_quiescence() {
    use hex_devices::operator_state::{OPERATOR_METADATA, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    fn source_project(note_duration: u64, arrangement_note: bool) -> Project {
        let mut state = crate::operator_legacy_test::legacy_operator_state();
        state = OPERATOR_METADATA
            .patch_parameter(&state, parameter::feedback(1), ParameterValue::Float(0.25))
            .unwrap();
        let mut snapshot = project(1.0, 0, 8 << 32).snapshot();
        let instrument = snapshot.tracks[0].instrument.as_mut().unwrap();
        instrument.state = state;
        let clips = match &mut snapshot.tracks[0].content {
            TrackContentSnapshot::Instrument(clips) => clips,
            TrackContentSnapshot::Hybrid { midi_clips, .. } => midi_clips,
            _ => panic!("fixture has MIDI arrangement content"),
        };
        if arrangement_note {
            clips[0].notes.push(MidiNoteSnapshot {
                active: true,
                id: snapshot.next_id,
                start_raw: 0,
                duration_raw: note_duration,
                pitch: 61,
                channel: 0,
                velocity: 100,
                expression: Default::default(),
                expression_points: Vec::new(),
            });
            snapshot.next_id += 1;
        }
        let Some(session_cell) = snapshot.session.cells.first_mut() else {
            panic!("fixture has Session content");
        };
        let SessionCellContentSnapshot::Midi(session_clip) = &mut session_cell.content else {
            panic!("fixture has Session MIDI content");
        };
        session_clip.source.notes.push(MidiNoteSnapshot {
            active: true,
            id: snapshot.next_id,
            start_raw: 0,
            duration_raw: note_duration,
            pitch: 61,
            channel: 0,
            velocity: 100,
            expression: Default::default(),
            expression_points: Vec::new(),
        });
        snapshot.next_id += 1;
        Project::from_legacy_snapshot(snapshot).unwrap()
    }

    fn unsupported_candidate(
        source: &Project,
        instrument_id: hex_project::DeviceId,
        old_revision: &PreparedRevision,
        table: Arc<hex_devices::SineTable>,
    ) -> Arc<PreparedRevision> {
        let mut state = source.tracks()[0].instrument().unwrap().state().clone();
        state = OPERATOR_METADATA
            .patch_parameter(
                &state,
                parameter::oscillator_ratio(1),
                ParameterValue::Float(2.0),
            )
            .unwrap();
        let mut editor = Editor::new(source.clone(), 4).unwrap();
        editor
            .execute(Command::ReplaceDeviceState {
                device: instrument_id,
                state,
            })
            .unwrap();
        let candidate = Arc::new(
            PreparedRevision::prepare(
                editor.project(),
                &BTreeMap::new(),
                table,
                Some(old_revision.session()),
            )
            .unwrap(),
        );
        let track = source.tracks()[0].id();
        assert!(
            !candidate
                .session()
                .mutable_history_retarget_requires_quiescence(track)
        );
        assert!(
            candidate
                .session()
                .mutable_history_retarget(track)
                .is_some()
        );
        candidate
    }

    let table = Arc::new(hex_devices::SineTable::new());

    // A held voice prevents a same-source unsupported law change from being
    // detached into an outgoing tail.
    let source = source_project(1 << 32, false);
    let instrument_id = source.tracks()[0].instrument().unwrap().id();
    let old_revision = Arc::new(
        PreparedRevision::prepare(&source, &BTreeMap::new(), Arc::clone(&table), None).unwrap(),
    );
    let candidate =
        unsupported_candidate(&source, instrument_id, &old_revision, Arc::clone(&table));
    let mut previous = SessionPlayback::new(old_revision).unwrap();
    let session_clip = previous.revision.session().midi_clips()[0].id;
    previous
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: session_clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut held_prefix = [[0.0; 2]; 128];
    previous.render(&mut held_prefix).unwrap();
    assert!(previous.routes.iter().enumerate().any(|(index, route)| {
        matches!(route.owner, RouteOwner::Session { .. })
            && previous
                .mutable_instruments
                .get(index)
                .and_then(Option::as_ref)
                .is_some_and(|runtime| !runtime.committed_is_quiescent())
    }));
    let mut successor = SessionPlayback::new(candidate).unwrap();
    assert_eq!(
        successor.try_inherit_state_from(&mut previous),
        Err(SessionPlaybackError::SourceHistoryUnavailable)
    );
    assert!(previous.playing());

    // A released voice still owns feedback/envelope tail state and receives
    // the same refusal until that committed tail expires.
    let source = source_project(1 << 30, true);
    let instrument_id = source.tracks()[0].instrument().unwrap().id();
    let old_revision = Arc::new(
        PreparedRevision::prepare(&source, &BTreeMap::new(), Arc::clone(&table), None).unwrap(),
    );
    let candidate =
        unsupported_candidate(&source, instrument_id, &old_revision, Arc::clone(&table));
    let mut previous = SessionPlayback::new(old_revision).unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut released_prefix = vec![[0.0; 2]; 7_000];
    previous.render(&mut released_prefix).unwrap();
    assert!(
        previous.mutable_instruments[0]
            .as_ref()
            .is_some_and(|runtime| !runtime.committed_is_quiescent())
    );
    let mut successor = SessionPlayback::new(candidate).unwrap();
    assert_eq!(
        successor.try_inherit_state_from(&mut previous),
        Err(SessionPlaybackError::SourceHistoryUnavailable)
    );

    // The same edit is admissible after the prior runtime has no active ledger
    // entries or feedback tail; the new law starts from a fresh runtime.
    let source = source_project(1 << 32, true);
    let instrument_id = source.tracks()[0].instrument().unwrap().id();
    let old_revision = Arc::new(
        PreparedRevision::prepare(&source, &BTreeMap::new(), Arc::clone(&table), None).unwrap(),
    );
    let candidate = unsupported_candidate(&source, instrument_id, &old_revision, table);
    let mut previous = SessionPlayback::new(old_revision).unwrap();
    let mut successor = SessionPlayback::new(candidate).unwrap();
    successor.try_inherit_state_from(&mut previous).unwrap();
}

#[test]
fn skipped_unsupported_operator_edit_uses_actual_source_law() {
    use hex_devices::operator_state::{OPERATOR_METADATA, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    let table = Arc::new(hex_devices::SineTable::new());
    let source = {
        let mut state = crate::operator_legacy_test::legacy_operator_state();
        state = OPERATOR_METADATA
            .patch_parameter(&state, parameter::feedback(1), ParameterValue::Float(0.25))
            .unwrap();
        let mut snapshot = project(1.0, 0, 8 << 32).snapshot();
        snapshot.tracks[0].instrument.as_mut().unwrap().state = state;
        let clips = match &mut snapshot.tracks[0].content {
            TrackContentSnapshot::Instrument(clips) => clips,
            TrackContentSnapshot::Hybrid { midi_clips, .. } => midi_clips,
            _ => panic!("fixture has MIDI arrangement content"),
        };
        clips[0].notes.push(MidiNoteSnapshot {
            active: true,
            id: snapshot.next_id,
            start_raw: 0,
            duration_raw: 1 << 32,
            pitch: 61,
            channel: 0,
            velocity: 100,
            expression: Default::default(),
            expression_points: Vec::new(),
        });
        snapshot.next_id += 1;
        let SessionCellContentSnapshot::Midi(session_clip) = &mut snapshot.session.cells[0].content
        else {
            panic!("fixture has Session MIDI content");
        };
        session_clip.source.notes.push(MidiNoteSnapshot {
            active: true,
            id: snapshot.next_id,
            start_raw: 0,
            duration_raw: 1 << 32,
            pitch: 61,
            channel: 0,
            velocity: 100,
            expression: Default::default(),
            expression_points: Vec::new(),
        });
        snapshot.next_id += 1;
        Project::from_legacy_snapshot(snapshot).unwrap()
    };
    let instrument_id = source.tracks()[0].instrument().unwrap().id();
    let old_revision = Arc::new(
        PreparedRevision::prepare(&source, &BTreeMap::new(), Arc::clone(&table), None).unwrap(),
    );

    let edit = |base: &Project, state: hex_lattice_contract::NativeDeviceState| {
        let mut editor = Editor::new(base.clone(), 4).unwrap();
        editor
            .execute(Command::ReplaceDeviceState {
                device: instrument_id,
                state,
            })
            .unwrap();
        editor.project().clone()
    };
    let unsupported_state = OPERATOR_METADATA
        .patch_parameter(
            &source.tracks()[0].instrument().unwrap().state().clone(),
            parameter::oscillator_ratio(1),
            ParameterValue::Float(2.0),
        )
        .unwrap();
    let skipped_project = edit(&source, unsupported_state);
    let skipped_revision = Arc::new(
        PreparedRevision::prepare(
            &skipped_project,
            &BTreeMap::new(),
            Arc::clone(&table),
            Some(old_revision.session()),
        )
        .unwrap(),
    );
    let compatible_state = OPERATOR_METADATA
        .patch_parameter(
            &source.tracks()[0].instrument().unwrap().state().clone(),
            parameter::feedback(1),
            ParameterValue::Float(0.75),
        )
        .unwrap();
    let final_project = edit(&skipped_project, compatible_state);
    let final_revision = Arc::new(
        PreparedRevision::prepare(
            &final_project,
            &BTreeMap::new(),
            Arc::clone(&table),
            Some(skipped_revision.session()),
        )
        .unwrap(),
    );

    // B changes an immutable Operator law but is never published. C is
    // prepared from B, while the audio owner is still on A; target-bound proof
    // and actual-source validation should allow the compatible A -> C edit.
    let mut actual_a = SessionPlayback::new(old_revision.clone()).unwrap();
    let session_clip = actual_a.revision.session().midi_clips()[0].id;
    actual_a
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: session_clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    actual_a.apply(SessionPlaybackCommand::Play).unwrap();
    let mut active = [[0.0; 2]; 128];
    actual_a.render(&mut active).unwrap();
    assert!(
        actual_a.mutable_instruments[0]
            .as_ref()
            .is_some_and(|runtime| !runtime.committed_is_quiescent())
    );
    let mut direct_c = SessionPlayback::new(Arc::clone(&final_revision)).unwrap();
    direct_c.try_inherit_state_from(&mut actual_a).unwrap();

    // The same C target must refuse if B really was applied: its immutable
    // waveform law no longer matches the actual runtime while notes are live.
    let mut quiescent_a = SessionPlayback::new(old_revision).unwrap();
    let mut applied_b = SessionPlayback::new(skipped_revision).unwrap();
    applied_b.try_inherit_state_from(&mut quiescent_a).unwrap();
    let session_clip = applied_b.revision.session().midi_clips()[0].id;
    applied_b
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: session_clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    applied_b.apply(SessionPlaybackCommand::Play).unwrap();
    let mut active = [[0.0; 2]; 128];
    applied_b.render(&mut active).unwrap();
    assert!(
        applied_b.mutable_instruments[0]
            .as_ref()
            .is_some_and(|runtime| !runtime.committed_is_quiescent())
    );
    let mut from_applied_b = SessionPlayback::new(final_revision).unwrap();
    assert_eq!(
        from_applied_b.try_inherit_state_from(&mut applied_b),
        Err(SessionPlaybackError::SourceHistoryUnavailable)
    );
    assert!(from_applied_b.candidate_admission_open);
}

#[test]
fn section_loop_preroll_starts_canonical_loop_with_empty_device_history() {
    thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let region = LoopRegion::new(24_000, 48_000, true).unwrap();
            let source = mutable_feedback_with_stateful_track_and_return_edited(|snapshot| {
                snapshot.loop_region = Some(region)
            });
            // Section launches use the interactive Play-from law at the loop
            // start: no prefix replay, empty tails and chased held notes.
            let mut expected = SessionPlayback::new(Arc::clone(&source)).unwrap();
            expected.play_from(24_000).unwrap();
            let mut history = SessionPlayback::new(Arc::clone(&expected.revision)).unwrap();
            history.apply(SessionPlaybackCommand::Play).unwrap();
            render_checkpoint_partitioned(&mut history, &[257], 24_000);
            let mut actual = SessionPlayback::prepare_section_arrangement_preroll(
                Arc::clone(&source),
                None,
                region,
                24_000,
                &AtomicBool::new(false),
            )
            .unwrap();
            assert_eq!(actual.loop_region(), Some(region));
            assert!(Arc::ptr_eq(&actual.revision, &source));
            assert_eq!(
                (actual.arrangement_frame(), actual.performance_frame()),
                (24_000, 24_000)
            );
            let mut reference = vec![[0.0; 2]; 48_129];
            let mut rendered = vec![[0.0; 2]; reference.len()];
            for chunk in reference.chunks_mut(257) {
                expected.render(chunk).unwrap();
            }
            for chunk in rendered.chunks_mut(127) {
                actual.render(chunk).unwrap();
            }
            assert_eq!(
                rendered, reference,
                "first pass, loop gates and empty tails"
            );
            let mut replayed = vec![[0.0; 2]; reference.len()];
            for chunk in replayed.chunks_mut(257) {
                history.render(chunk).unwrap();
            }
            assert_ne!(rendered, replayed, "prefix history is not reconstructed");
            assert_eq!(actual.arrangement_frame(), expected.arrangement_frame());
            assert_eq!(actual.performance_frame(), expected.performance_frame());
            let playback =
                crate::playback::Playback::from_interactive_arrangement_preroll(actual).unwrap();
            assert_eq!(playback.section_loop(), None);
            assert_eq!(playback.automation_capture_loop(), Some(region));
            assert_eq!(
                SessionPlayback::prepare_section_arrangement_preroll(
                    source,
                    None,
                    region,
                    24_000,
                    &AtomicBool::new(true),
                )
                .unwrap_err(),
                ArrangementPrerollError::Cancelled
            );
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn section_loop_readmits_rounded_timing_without_rebuilding_note_resources() {
    thread::Builder::new().stack_size(8 * 1024 * 1024).spawn(|| {
        let source = project_at_tempo(0.7, 0, 1 << 32, Tempo::new(123_000_000).unwrap());
        let region = LoopRegion::new(0, 23_415, true).unwrap();
        let prepared = revision(&source, Arc::new(hex_devices::SineTable::new()));
        let periodic = PreparedPeriodicSessionTempo::prepare(prepared.session().timing(), region).unwrap();
        assert_eq!(periodic.view(prepared.session().timing()).max_micro_bpm(), 123_000_001);
        let quote = SessionPlayback::section_runtime_allowance(&prepared, region).unwrap();
        let mut actual = SessionPlayback::prepare_section_arrangement_preroll(
            Arc::clone(&prepared), None, region, 0, &AtomicBool::new(false)).unwrap();
        assert!(actual.midi_admission.is_some(), "one micro-bpm rounding needs a new proof");
        assert!(actual.accounted_runtime_storage_bytes().unwrap() <= quote);
        let original = &prepared.session().midi_clips()[0];
        let effective = crate::session_inventory::EffectiveSessionMidi::new(prepared.session(), actual.midi_admission.as_ref()).midi_clip(original.id).unwrap();
        assert!(Arc::ptr_eq(original.instrument_template().note_bank(), effective.instrument_template().note_bank()), "note resources stay shared");
        assert_eq!(actual.session_work_envelope(), prepared.session().work());
        let mut snapshot = source.snapshot();
        snapshot.loop_region = Some(region);
        let oracle_source = Project::from_legacy_snapshot(snapshot).unwrap();
        let mut expected = SessionPlayback::new(revision(&oracle_source, Arc::new(hex_devices::SineTable::new()))).unwrap();
        for runtime in [&mut actual, &mut expected] {
            runtime.apply(SessionPlaybackCommand::LaunchClip { clip: original.id, quantization: LaunchQuantization::Immediate }).unwrap();
            runtime.apply(SessionPlaybackCommand::Play).unwrap();
        }
        let mut rendered = vec![[0.0; 2]; 48_013];
        let mut oracle = rendered.clone();
        for chunk in rendered.chunks_mut(127) { actual.render(chunk).unwrap(); }
        for chunk in oracle.chunks_mut(257) { expected.render(chunk).unwrap(); }
        assert_eq!(rendered, oracle, "effective source, interval index and release timing match a freshly prepared canonical loop");
        assert!(prepared.arrangement().loop_region().is_none());
        let playback = Playback::from_interactive_arrangement_preroll(actual).unwrap();
        let observation = playback.prepare_observation_on_control().unwrap();
        assert!(observation.retained_bytes_on_control().unwrap() <= playback.observation_storage_quote_on_control().unwrap());
        // Re-admission still refuses a truly dense periodic source. One note
        // per quarter is valid at123bpm but exceeds256 edges/256frames here.
        let mut snapshot = source.snapshot();
        let SessionCellContentSnapshot::Midi(clip) = &mut snapshot.session.cells[0].content else { panic!("midi fixture") };
        clip.source.source_length_raw = 1 << 32;
        clip.source.loop_end_raw = 1 << 32;
        clip.source.looping = true;
        let dense_source = Project::from_legacy_snapshot(snapshot).unwrap();
        let dense = revision(&dense_source, Arc::new(hex_devices::SineTable::new()));
        assert!(matches!(dense.session().midi_clips()[0].source_template().reprepare_for_timing_on_control(2_000_000_000_000), Err(crate::MidiTemplateError::TooManyEdges | crate::MidiTemplateError::TooManyOnsets)));
    }).unwrap().join().unwrap();
}

#[test]
fn arrangement_preroll_held_mutable_source_and_return_send_follow_seek_law() {
    let prepared = mutable_feedback_with_stateful_track_and_return();
    assert!(prepared.arrangement_seek_requires_preroll());
    let caller = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    assert_eq!(caller.device_chains[0].stateful_count(), 1);
    assert_eq!(caller.return_chains[0].stateful_count(), 1);
    drop(caller);
    run_preroll_continuation_on_worker_stack(prepared, 512);
}

#[test]
fn ordinary_preroll_refuses_stopped_pcm_and_preserves_held_instrument_history() {
    let prepared = mutable_feedback_with_stateful_track_and_return();
    let track = prepared.arrangement().tracks()[0]
        .canonical_track_id()
        .unwrap();
    let caller = SessionPlayback::new(prepared).unwrap();
    let prepare = || {
        caller
            .prepare_arrangement_preroll(512, 512, false, &AtomicBool::new(false))
            .unwrap()
    };
    let mut candidate = prepare();
    let mut reference = prepare();
    let mut output = [[1.0; 2]; 64];
    assert_eq!(
        candidate.render_paused_pcm(
            track,
            &[[0.5; 2]; 64],
            &mut output,
            &mut hex_lattice::RefuseForeign
        ),
        Err(SessionPlaybackError::InvalidCommand)
    );
    assert_eq!(output, [[0.0; 2]; 64]);
    candidate.render(&mut output).unwrap();
    assert_eq!(output, [[0.0; 2]; 64]);
    assert_eq!(candidate.performance_frame(), 512);
    let mut expected = output;
    for (runtime, frames) in [
        (&mut candidate, &mut output),
        (&mut reference, &mut expected),
    ] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        runtime.render(frames).unwrap();
    }
    assert!(expected.iter().any(|frame| *frame != [0.0; 2]));
    assert_eq!(output, expected);
}

#[test]
fn interactive_arrangement_preroll_released_mutable_source_and_return_start_empty() {
    let prepared = mutable_feedback_with_stateful_track_and_return();
    assert!(prepared.arrangement_seek_requires_preroll());
    run_preroll_continuation_on_worker_stack(prepared, 24_704);
}

#[test]
fn interactive_arrangement_preroll_loop_destinations_follow_play_from_law() {
    thread::Builder::new()
        .name("hex-engine-loop-preroll-test".into())
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let tempo = |snapshot: &mut Snapshot| {
                snapshot.tempo_points.push(TempoPoint::new(
                    BeatTime::from_ratio(1, 100).unwrap(),
                    Tempo::new(90_000_000).unwrap(),
                ));
                // This later authored transition is outside the loop. It must
                // not leak into the periodic performance clock after the seek.
                snapshot.tempo_points.push(TempoPoint::new(
                    BeatTime::from_ratio(1, 10).unwrap(),
                    Tempo::new(180_000_000).unwrap(),
                ));
            };
            let prepared = mutable_feedback_with_stateful_track_and_return_edited(|snapshot| {
                tempo(snapshot);
                snapshot.loop_region = Some(LoopRegion::new(128, 1024, true).unwrap());
            });
            for (requested, destination) in [
                (0, 0),
                (64, 64),
                (128, 128),
                (512, 512),
                (1024, 1024),
                (1408, 1408),
                (91_008, 91_008),
            ] {
                for playing in [false, true] {
                    assert_loop_preroll_continuation(&prepared, requested, destination, playing);
                }
            }
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn interactive_arrangement_preroll_accepts_loops_and_checks_cancellation() {
    let mut looped_snapshot = project(0.5, 0, 1 << 32).snapshot();
    looped_snapshot.loop_region = Some(LoopRegion::new(0, 1024, true).unwrap());
    let looped_project = Project::from_legacy_snapshot(looped_snapshot).unwrap();
    let looped = revision(&looped_project, Arc::new(hex_devices::SineTable::new()));
    const SEEK_STACK_BYTES: usize = 8 * 1024 * 1024;
    thread::Builder::new()
        .name("hex-engine-interactive-refusal-test".into())
        .stack_size(SEEK_STACK_BYTES)
        .spawn(move || {
            let mut candidate =
                SessionPlayback::prepare_interactive_arrangement_preroll_from_revision(
                    Arc::clone(&looped),
                    128,
                    128,
                    true,
                    &AtomicBool::new(false),
                )
                .expect("enabled loop permits interactive seek");
            assert!(Arc::ptr_eq(&candidate.revision, &looped));
            assert_eq!(candidate.arrangement_frame(), 128);
            let mut actual = [[0.0; 2]; 1200];
            candidate.render(&mut actual).unwrap();
            assert_eq!(candidate.arrangement_frame(), 304);
            assert!(actual.iter().any(|frame| *frame != [0.0; 2]));
            assert_eq!(
                SessionPlayback::prepare_interactive_arrangement_preroll_from_revision(
                    Arc::clone(&looped),
                    128,
                    128,
                    false,
                    &AtomicBool::new(true),
                )
                .unwrap_err(),
                ArrangementPrerollError::Cancelled
            );
            // No prefix replays, so a post-loop destination needs no replay
            // work budget and keeps its absolute cursor.
            let post_loop = SessionPlayback::prepare_interactive_arrangement_preroll_from_revision(
                looped,
                1408,
                0,
                true,
                &AtomicBool::new(false),
            )
            .expect("interactive seek renders no prefix");
            assert_eq!(post_loop.arrangement_frame(), 1408);
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn interactive_and_section_destinations_exceed_the_linear_replay_ceiling() {
    // 120 bpm at 48 kHz: beat 1398 starts at frame 33_552_000, so the
    // destination lands inside the clip's first held note.
    let destination = MAX_ARRANGEMENT_PREROLL_FRAMES * 4 + 37;
    let mut snapshot = project(0.5, 0, 1 << 32).snapshot();
    let (TrackContentSnapshot::Instrument(clips)
    | TrackContentSnapshot::Hybrid {
        midi_clips: clips, ..
    }) = &mut snapshot.tracks[0].content
    else {
        panic!("fixture track holds midi clips");
    };
    clips[0].start_raw = 1398 << 32;
    let prepared = revision(
        &Project::from_legacy_snapshot(snapshot).unwrap(),
        Arc::new(hex_devices::SineTable::new()),
    );
    // Linear export still replays the prefix and keeps its work ceiling.
    assert_eq!(
        SessionPlayback::prepare_arrangement_preroll_from_revision(
            Arc::clone(&prepared),
            destination,
            MAX_ARRANGEMENT_PREROLL_FRAMES,
            true,
            &AtomicBool::new(false),
        )
        .unwrap_err(),
        ArrangementPrerollError::WorkBudgetExceeded
    );
    thread::Builder::new()
        .name("hex-engine-long-seek-test".into())
        .stack_size(8 * 1024 * 1024)
        .spawn(move || {
            let mut expected = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
            expected.play_from(destination).unwrap();
            let mut expected_audio = [[0.0; 2]; 129];
            expected.render(&mut expected_audio).unwrap();
            assert!(expected_audio.iter().any(|frame| *frame != [0.0; 2]));
            for start_playing in [true, false] {
                let mut candidate =
                    SessionPlayback::prepare_interactive_arrangement_preroll_from_revision(
                        Arc::clone(&prepared),
                        destination,
                        destination,
                        start_playing,
                        &AtomicBool::new(false),
                    )
                    .expect("interactive seek beyond the replay ceiling");
                assert_eq!(candidate.arrangement_frame(), destination);
                assert_eq!(candidate.playing(), start_playing);
                if !start_playing {
                    candidate.apply(SessionPlaybackCommand::Play).unwrap();
                }
                let mut actual = [[0.0; 2]; 129];
                candidate.render(&mut actual).unwrap();
                assert_eq!(
                    actual, expected_audio,
                    "held note is chased at {destination}"
                );
            }
            let region = LoopRegion::new(destination, destination + 4096, true).unwrap();
            let section = SessionPlayback::prepare_section_arrangement_preroll(
                Arc::clone(&prepared),
                None,
                region,
                destination,
                &AtomicBool::new(false),
            )
            .expect("section launch beyond the replay ceiling");
            assert_eq!(section.arrangement_frame(), destination);
            assert!(section.playing());
            // Play-from arithmetic still bounds the destination.
            assert_eq!(
                SessionPlayback::prepare_interactive_arrangement_preroll_from_revision(
                    prepared,
                    u64::MAX,
                    u64::MAX,
                    true,
                    &AtomicBool::new(false),
                )
                .unwrap_err(),
                ArrangementPrerollError::Playback(SessionPlaybackError::ArithmeticOverflow)
            );
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn arrangement_preroll_checks_work_cancellation_loop_and_pause_contracts() {
    let prepared = revision(
        &project(0.5, 0, 1 << 32),
        Arc::new(hex_devices::SineTable::new()),
    );
    let caller = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    let cancelled = AtomicBool::new(true);
    assert_eq!(
        caller
            .prepare_arrangement_preroll(256, 256, false, &cancelled)
            .unwrap_err(),
        ArrangementPrerollError::Cancelled
    );
    assert_eq!(
        caller
            .prepare_arrangement_preroll(257, 256, false, &AtomicBool::new(false))
            .unwrap_err(),
        ArrangementPrerollError::WorkBudgetExceeded
    );
    assert_eq!(
        caller
            .prepare_arrangement_preroll(
                u64::MAX,
                MAX_ARRANGEMENT_PREROLL_FRAMES,
                false,
                &AtomicBool::new(false),
            )
            .unwrap_err(),
        ArrangementPrerollError::WorkBudgetExceeded
    );

    let paused = caller
        .prepare_arrangement_preroll(256, 256, false, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(paused.performance_frame(), 256);
    assert_eq!(paused.arrangement_frame(), 256);
    assert!(!paused.playing());

    let polls = AtomicUsize::new(0);
    let error = caller
        .prepare_arrangement_preroll_checked(
            512,
            512,
            true,
            MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
            || polls.fetch_add(1, Ordering::Relaxed) >= 3,
        )
        .unwrap_err();
    assert_eq!(error, ArrangementPrerollError::Cancelled);
    assert_eq!(caller.performance_frame(), 0);
    assert!(!caller.playing());

    let final_block_polls = AtomicUsize::new(0);
    let error = caller
        .prepare_arrangement_preroll_checked(
            256,
            256,
            true,
            MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
            || final_block_polls.fetch_add(1, Ordering::Relaxed) >= 3,
        )
        .unwrap_err();
    assert_eq!(error, ArrangementPrerollError::Cancelled);
    assert_eq!(final_block_polls.load(Ordering::Relaxed), 4);
    assert_eq!(caller.performance_frame(), 0);
    assert!(!caller.playing());
}

#[test]
fn arrangement_preroll_accepts_zero_frame_and_refuses_transport_loop() {
    let prepared = revision(
        &project(0.5, 0, 1 << 32),
        Arc::new(hex_devices::SineTable::new()),
    );
    let caller = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    let zero = caller
        .prepare_arrangement_preroll(0, 0, false, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(zero.performance_frame(), 0);
    assert_eq!(zero.arrangement_frame(), 0);
    assert!(!zero.playing());
    let zero_playing = caller
        .prepare_arrangement_preroll(0, 0, true, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(zero_playing.performance_frame(), 0);
    assert_eq!(zero_playing.arrangement_frame(), 0);
    assert!(zero_playing.playing());

    let mut looped_snapshot = project(0.5, 0, 1 << 32).snapshot();
    looped_snapshot.loop_region = Some(LoopRegion::new(0, 1024, true).unwrap());
    let looped = Project::from_legacy_snapshot(looped_snapshot).unwrap();
    let looped = revision(&looped, Arc::new(hex_devices::SineTable::new()));
    let looped_caller = SessionPlayback::new(Arc::clone(&looped)).unwrap();
    assert_eq!(
        looped_caller
            .prepare_arrangement_preroll(128, 128, false, &AtomicBool::new(false))
            .unwrap_err(),
        ArrangementPrerollError::UnsupportedTransportLoop
    );
}

#[test]
fn arrangement_preroll_empty_project_can_advance_beyond_content_end() {
    let empty_project = Project::new(48_000).unwrap();
    let empty_revision = revision(&empty_project, Arc::new(hex_devices::SineTable::new()));
    let empty_caller = SessionPlayback::new(Arc::clone(&empty_revision)).unwrap();
    let mut candidate = empty_caller
        .prepare_arrangement_preroll(12_345, 12_345, true, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(candidate.performance_frame(), 12_345);
    assert_eq!(candidate.arrangement_frame(), 12_345);
    assert!(candidate.playing());
    let mut silence = [[1.0, -1.0]; 16];
    candidate.render(&mut silence).unwrap();
    assert!(silence.iter().all(|frame| *frame == [0.0, 0.0]));
    assert_eq!(empty_caller.performance_frame(), 0);
    assert!(!empty_caller.playing());
}

#[test]
fn arrangement_preroll_refuses_storage_preflight_before_runtime_factory() {
    let (prepared, _) = mutable_checkpoint_fixture(8 << 32);
    let caller = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    let counter = Arc::new(AtomicUsize::new(0));
    MUTABLE_RUNTIME_CREATE_COUNTER.with(|slot| *slot.borrow_mut() = Some(Arc::clone(&counter)));
    MUTABLE_RUNTIME_DECLARED_BYTES_OVERRIDE.with(|bytes| {
        bytes.set(Some(hex_lattice::MAX_PROCESSOR_RUNTIME_BYTES));
    });
    let preflight = super::preroll::arrangement_runtime_storage_preflight_bytes(&prepared).unwrap();
    assert!(preflight > 1);
    let result =
        caller.prepare_arrangement_preroll_checked(256, 256, true, preflight - 1, || false);
    assert_eq!(
        result.unwrap_err(),
        ArrangementPrerollError::StorageBudgetExceeded
    );
    assert_eq!(counter.load(Ordering::Relaxed), 0);
    assert_eq!(caller.performance_frame(), 0);
    assert!(!caller.playing());
    MUTABLE_RUNTIME_DECLARED_BYTES_OVERRIDE.with(|bytes| bytes.set(None));
    MUTABLE_RUNTIME_CREATE_COUNTER.with(|slot| *slot.borrow_mut() = None);
}

#[test]
fn linear_audio_only_export_omits_unused_banks_preserves_pcm_and_refuses_console() {
    let source: Vec<Frame> = (0..32)
        .map(|i| [i as f32 / 64.0, -(i as f32) / 128.0])
        .collect();
    let audio = crate::Audio::new(48_000, source.clone()).unwrap();
    let mut snapshot = audio_project(0, 32, 4).snapshot();
    snapshot.next_id = 8;
    snapshot.tracks[0].content = TrackContentSnapshot::Audio(vec![ClipSnapshot {
        active: true,
        name: "arrangement".into(),
        flavor: hex_project::ClipFlavor::default(),
        edit: None,
        gain: hex_project::ClipGain::UNITY,
        id: 7,
        asset: 4,
        start: 0,
        duration: 32,
        source_offset: 0,
        source_length: 32,
        looping: false,
        source_phase: 0,
        fade: ClipFade::neutral(32).unwrap(),
    }]);
    let project = Project::from_snapshot(snapshot).unwrap();
    // Unlinked authoring retains its MIDI metadata wrapper, with no compiled
    // note spans or generator. It must not spend interactive release storage.
    let mut editor = Editor::new(project, 8).unwrap();
    let Created::Track(unbound) = editor
        .execute(Command::AddMidiTrack {
            name: "unbound".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track: unbound,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let project = editor.project();
    let media = audio_media(project, &audio, &audio);
    let revision = Arc::new(
        PreparedRevision::prepare(project, &media, Arc::new(crate::SineTable::new()), None)
            .unwrap(),
    );
    let interactive_quote =
        super::preroll::arrangement_runtime_storage_preflight_bytes(&revision).unwrap();
    assert_eq!(
        SessionPlayback::interactive_runtime_allowance(&revision).unwrap(),
        interactive_quote
    );
    let quote = SessionPlayback::linear_export_runtime_allowance(&revision).unwrap();
    let omitted = 2
        * SESSION_RELEASE_CAPACITY
        * (std::mem::size_of::<ReleaseEntry>() + std::mem::size_of::<MidiOccurrenceSuppression>());
    assert_eq!(interactive_quote - quote, omitted);
    let mut interactive = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    assert_eq!(
        interactive.release_storage_capacity(),
        SESSION_RELEASE_CAPACITY
    );
    let mut export = SessionPlayback::new_linear_arrangement_export(revision).unwrap();
    assert_eq!(export.release_storage_capacity(), 0);
    assert_eq!(export.releases.capacity(), 0);
    assert_eq!(export.release_scratch.capacity(), 0);
    assert_eq!(export.suppressions.capacity(), 0);
    assert_eq!(export.suppression_scratch.capacity(), 0);
    assert_eq!(
        export.accounted_runtime_storage_bytes().unwrap() + super::preroll::PREROLL_SCRATCH_BYTES,
        quote
    );
    assert!(export.try_inherit_state_from(&mut interactive).is_err());
    assert!(
        export
            .apply(SessionPlaybackCommand::LaunchClip {
                clip: export.revision.session().audio_clips()[0].id,
                quantization: LaunchQuantization::Immediate,
            })
            .is_err()
    );
    export.apply(SessionPlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 40];
    export.render(&mut output).unwrap();
    assert_eq!(output[..32], source);
    assert_eq!(output[32..], [[0.0; 2]; 8]);

    let prepare = |budget| {
        SessionPlayback::prepare_arrangement_preroll_from_revision_checked(
            Arc::clone(&interactive.revision),
            4,
            4,
            true,
            budget,
            || false,
        )
    };
    assert_eq!(
        prepare(quote - 1).unwrap_err(),
        ArrangementPrerollError::StorageBudgetExceeded
    );
    let mut replay = prepare(quote).unwrap();
    let mut next = [[0.0; 2]; 4];
    replay.render(&mut next).unwrap();
    assert_eq!(next, source[4..8]);

    let (midi, _) = mutable_checkpoint_fixture(8 << 32);
    let midi_quote = super::preroll::arrangement_runtime_storage_preflight_bytes(&midi).unwrap();
    assert_eq!(
        SessionPlayback::linear_export_runtime_allowance(&midi).unwrap(),
        midi_quote
    );
    let midi_export = SessionPlayback::new_linear_arrangement_export(midi).unwrap();
    assert_eq!(
        midi_export.release_storage_capacity(),
        SESSION_RELEASE_CAPACITY
    );
}

#[test]
fn linear_receiverless_bound_source_uses_audio_only_quote_and_renders_silence() {
    let mut editor = Editor::new(project(1.0, 0, 1 << 32), 8).unwrap();
    let receiver = editor.project().tracks()[0].id();
    let source = editor
        .project()
        .track(receiver)
        .unwrap()
        .source_id()
        .unwrap();
    let Created::Track(sender) = editor
        .execute(Command::AddMidiTrack {
            name: "sender".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::BindTrackSource {
            track: sender,
            source: Some(source),
        })
        .unwrap();
    editor
        .execute(Command::SetNativeSourceReceiver {
            source,
            track: None,
        })
        .unwrap();
    assert_eq!(editor.project().source_contributors(source).count(), 2);
    assert!(
        !editor.project().track(receiver).unwrap().midi_clips()[0]
            .notes()
            .is_empty()
    );
    let prepared = revision(editor.project(), Arc::new(crate::SineTable::new()));
    assert!(
        prepared
            .arrangement()
            .tracks()
            .iter()
            .all(|track| !track.receives_source_audio())
    );
    let quote = SessionPlayback::linear_export_runtime_allowance(&prepared).unwrap();
    let mut export = SessionPlayback::new_linear_arrangement_export(prepared).unwrap();
    assert_eq!(export.release_storage_capacity(), 0);
    assert!(
        export.accounted_runtime_storage_bytes().unwrap() + super::preroll::PREROLL_SCRATCH_BYTES
            <= quote
    );
    export.apply(SessionPlaybackCommand::Play).unwrap();
    let mut output = [[1.0; 2]; 64];
    export.render(&mut output).unwrap();
    assert_eq!(output, [[0.0; 2]; 64]);

    editor
        .execute(Command::SetNativeSourceReceiver {
            source,
            track: Some(receiver),
        })
        .unwrap();
    let prepared = revision(editor.project(), Arc::new(crate::SineTable::new()));
    let full = super::preroll::arrangement_runtime_storage_preflight_bytes(&prepared).unwrap();
    assert_eq!(
        SessionPlayback::linear_export_runtime_allowance(&prepared).unwrap(),
        full
    );
    assert_eq!(
        SessionPlayback::new_linear_arrangement_export(prepared)
            .unwrap()
            .release_storage_capacity(),
        SESSION_RELEASE_CAPACITY
    );
}

#[test]
fn resident_batch_storage_is_included_in_preroll_admission() {
    let fixture = crate::signal_graph::tests::fixture(&[&[7], &[]], 32);
    let revision = crate::signal_graph::tests::prepare(&fixture).unwrap();
    let preflight = super::preroll::arrangement_runtime_storage_preflight_bytes(&revision).unwrap();
    let runtime = SessionPlayback::new(revision).unwrap();
    assert_eq!(runtime.resident_source_scratch.len(), 2);
    let actual =
        runtime.accounted_runtime_storage_bytes().unwrap() + super::preroll::PREROLL_SCRATCH_BYTES;
    assert!(
        actual <= preflight,
        "actual={actual}, preflight={preflight}"
    );
}

#[test]
fn arrangement_preroll_storage_preflight_covers_constructed_runtime() {
    let (prepared, _) = mutable_checkpoint_fixture(8 << 32);
    let preflight = super::preroll::arrangement_runtime_storage_preflight_bytes(&prepared).unwrap();
    let caller = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    let candidate = caller
        .prepare_arrangement_preroll(1_024, 1_024, false, &AtomicBool::new(false))
        .unwrap();
    let actual = candidate.accounted_runtime_storage_bytes().unwrap();
    let actual_with_scratch = actual + super::preroll::PREROLL_SCRATCH_BYTES;
    assert!(
        actual_with_scratch <= preflight,
        "actual_with_scratch={actual_with_scratch}, preflight={preflight}"
    );
    assert!(preflight <= MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES);
    eprintln!("preroll runtime storage: preflight={preflight} actual={actual_with_scratch}");
}

#[test]
fn arrangement_preroll_returns_no_candidate_when_source_render_fails() {
    let table = Arc::new(hex_devices::SineTable::new());
    let source_project = session_audio_midi_arrangement_mix_project();
    let asset = source_project
        .assets()
        .find(|asset| asset.id().value() == 4)
        .unwrap();
    let streamed = StreamedSourceMetadata::new(
        StreamSourceKey::new(82, asset.id()),
        asset.source_sample_rate(),
        source_project.sample_rate(),
        asset.source_frames(),
        asset.playback_frames(),
        asset.conversion_version(),
        [0x82; 32],
    )
    .unwrap();
    let prepared_audio = source_project
        .assets()
        .map(|asset| {
            let source = if asset.id().value() == 4 {
                PreparedAudioSource::Streamed(Arc::new(streamed.clone()))
            } else {
                PreparedAudioSource::Resident(
                    crate::Audio::new(48_000, vec![[0.25, -0.25]; 32]).unwrap(),
                )
            };
            (asset.id(), source)
        })
        .collect::<BTreeMap<_, _>>();
    let prepared = Arc::new(
        PreparedRevision::prepare_with_prepared_audio_sources(
            &source_project,
            &prepared_audio,
            82,
            table,
            None,
        )
        .unwrap(),
    );
    let caller = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    let error = caller
        .prepare_arrangement_preroll(256, 256, true, &AtomicBool::new(false))
        .unwrap_err();
    assert_eq!(
        error,
        ArrangementPrerollError::Playback(SessionPlaybackError::MediaPagesUnavailable)
    );
    assert_eq!(caller.performance_frame(), 0);
    assert!(!caller.playing());
}

#[test]
#[ignore = "release-only timing evidence; run with --release and --ignored"]
fn arrangement_preroll_profiles_100_stateful_tracks() {
    let prepared = mutable_hundred_track_preroll_revision();
    let caller = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    let target = 16_384;
    let storage = super::preroll::arrangement_runtime_storage_preflight_bytes(&prepared).unwrap();
    assert_eq!(caller.route_count(), 100);
    let mut elapsed = Vec::new();
    for _ in 0..3 {
        let start = std::time::Instant::now();
        let candidate = caller
            .prepare_arrangement_preroll(target, target, false, &AtomicBool::new(false))
            .unwrap();
        elapsed.push(start.elapsed());
        assert_eq!(candidate.performance_frame(), target);
        assert!(!candidate.playing());
    }
    eprintln!(
        "100-track mutable preroll: sample_rate=48000 target_frames={target} blocks={} storage_upper_bound={storage} elapsed={elapsed:?}",
        target / 256
    );
}

#[test]
#[ignore = "release-only timing evidence; run with --release and --ignored"]
fn interactive_arrangement_preroll_profiles_100_stateful_tracks() {
    let prepared = mutable_hundred_track_preroll_revision();
    let target = 48_000;
    let storage = super::preroll::arrangement_runtime_storage_preflight_bytes(&prepared).unwrap();
    let timings = std::thread::Builder::new()
        .name("interactive-preroll-profile".into())
        .stack_size(8 * 1024 * 1024)
        .spawn(move || {
            let mut elapsed = Vec::with_capacity(3);
            for _ in 0..3 {
                let start = std::time::Instant::now();
                let candidate =
                    SessionPlayback::prepare_interactive_arrangement_preroll_from_revision(
                        Arc::clone(&prepared),
                        target,
                        target,
                        false,
                        &AtomicBool::new(false),
                    )
                    .unwrap();
                elapsed.push(start.elapsed());
                assert_eq!(candidate.performance_frame(), target);
                assert!(!candidate.playing());
            }
            elapsed
        })
        .unwrap()
        .join()
        .unwrap();
    eprintln!(
        "100-track interactive mutable preroll: sample_rate=48000 target_frames={target} blocks={} storage_upper_bound={storage} session_playback_bytes={} elapsed={timings:?}; excludes native callback/publication and is not a readiness deadline",
        target.div_ceil(256),
        std::mem::size_of::<SessionPlayback>()
    );
}
