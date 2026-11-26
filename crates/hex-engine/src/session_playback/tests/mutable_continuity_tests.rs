use super::*;

#[test]
fn ordinary_fresh_mutable_install_suppresses_precut_gates_with_atomic_capacity_refusal() {
    use hex_lattice::NativeInstrumentDefinition;
    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.fresh-install"),
            levels: [0.2, 0.4],
            release_frames: 8,
        },
    };
    let definitions: [&dyn NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let enabled = with_test_instrument(
        &with_second_arrangement_note(&project(1.0, 0, 8 << 32)),
        definition.metadata.id,
        0,
    );
    let track = enabled.tracks()[0].id();
    let device = enabled.tracks()[0].instrument().unwrap().id();
    let mut editor = Editor::new(enabled.clone(), 8).unwrap();
    editor
        .execute(Command::SetDeviceBypass {
            device,
            bypassed: true,
        })
        .unwrap();
    let disabled = editor.project().clone();
    for session in [false, true] {
        let mut old = ordinary_mutable_playback(&disabled, &registry);
        old.apply(SessionPlaybackCommand::Play).unwrap();
        if session {
            old.apply(SessionPlaybackCommand::LaunchClip {
                clip: old.revision.session().midi_clips()[0].id,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        }
        old.render(&mut [[0.0; 2]; 64]).unwrap();
        let old_owner = old.owner(track);
        let mut candidate = ordinary_mutable_playback(&enabled, &registry);
        candidate.release_capacity = 0;
        assert_eq!(
            candidate.try_inherit_state_from(&mut old),
            Err(SessionPlaybackError::ReleaseBankFull)
        );
        assert_eq!(old.performance_frame(), 64);
        assert_eq!(old.owner(track), old_owner);
        assert!(old.suppressions.is_empty());
        assert_eq!(candidate.performance_frame(), 0);
        assert!(candidate.suppressions.is_empty());
        candidate.release_capacity = SESSION_RELEASE_CAPACITY;
        candidate.try_inherit_state_from(&mut old).unwrap();
        assert_eq!(candidate.suppressions.len(), 1);
        let mut silent = [[1.0; 2]; 64];
        candidate.render(&mut silent).unwrap();
        assert_eq!(silent, [[0.0; 2]; 64]);
        // The one-shot exception is no longer present on an unchanged edit, but its exact
        // suppression remains until the original occurrence ends.
        let mut successor = ordinary_mutable_playback(&enabled, &registry);
        successor.try_inherit_state_from(&mut candidate).unwrap();
        successor.render(&mut silent).unwrap();
        assert_eq!(silent, [[0.0; 2]; 64]);
        if !session {
            let mut until_next = vec![[0.0; 2]; 24_000 - 192];
            successor.render(&mut until_next).unwrap();
            assert!(until_next.iter().all(|frame| *frame == [0.0; 2]));
            successor.render(&mut silent[..1]).unwrap();
            assert!(silent[0][0] > 0.0, "exact future onset is dispatched");
        }
    }
    // Complete old evidence also distinguishes a genuinely new canonical track from an
    // unobserved predecessor. Its pre-cut gate is suppressed without altering old sources.
    let mut new_track_editor = Editor::new(disabled.clone(), 8).unwrap();
    let Created::Track(new_track) = new_track_editor
        .execute(Command::AddMidiTrack { name: "new".into() })
        .unwrap()
    else {
        panic!("track")
    };
    new_track_editor
        .execute(Command::InsertInstrument {
            track: new_track,
            state: hex_lattice_contract::NativeDeviceState::new(definition.metadata.id, 1, vec![0])
                .unwrap(),
        })
        .unwrap();
    let Created::Clip(new_clip) = new_track_editor
        .execute(Command::PlaceMidiClip {
            track: new_track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    new_track_editor
        .execute(Command::AddMidiNote {
            clip: new_clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 64,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let mut old = ordinary_mutable_playback(&disabled, &registry);
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 64]).unwrap();
    let mut new_track_candidate = ordinary_mutable_playback(new_track_editor.project(), &registry);
    new_track_candidate
        .try_inherit_state_from(&mut old)
        .unwrap();
    let mut silent = [[1.0; 2]; 64];
    new_track_candidate.render(&mut silent).unwrap();
    assert_eq!(silent, [[0.0; 2]; 64]);

    // A changed active MIDI signature is not an installation proof, even alongside a real
    // authored device discontinuity. Missing legacy evidence also cannot authorize suppression.
    let mut old = ordinary_mutable_playback(&disabled, &registry);
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 64]).unwrap();
    let mut changed = Editor::new(enabled.clone(), 8).unwrap();
    changed
        .execute(Command::TrimMidiClipEdge {
            clip: enabled.tracks()[0].midi_clips()[0].id(),
            edge: hex_project::TrimEdge::Right,
            beat: BeatTime::from_raw(2 << 32),
        })
        .unwrap();
    let mut missing_source_proof = ordinary_mutable_playback(changed.project(), &registry);
    assert_eq!(
        missing_source_proof.try_inherit_state_from(&mut old),
        Err(SessionPlaybackError::MutableInstrumentChase)
    );
    assert_eq!(old.performance_frame(), 64);
    let mut legacy = SessionPlayback::new(revision_with_instrument_registry(
        &disabled,
        Arc::new(hex_devices::SineTable::new()),
        &registry,
    ))
    .unwrap();
    legacy.apply(SessionPlaybackCommand::Play).unwrap();
    legacy.render(&mut [[0.0; 2]; 64]).unwrap();
    assert_eq!(
        ordinary_mutable_playback(&enabled, &registry).try_inherit_state_from(&mut legacy),
        Err(SessionPlaybackError::MutableInstrumentChase)
    );
}

#[test]
fn shared_context_table_is_common_to_arrangement_session_and_reuses_exact_configuration() {
    let id = hex_lattice_contract::DeviceTypeId::from_static("test.instrument.shared.context");
    let definition = ContinuityInstrumentDefinition {
        id,
        levels: [0.25, 0.5],
        release_frames: 4,
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let base = with_test_instrument(&project(1.0, 0, 8 << 32), id, 0);
    let original = revision_with_instrument_registry(&base, Arc::clone(&table), &registry);
    let arrangement_contexts = original.arrangement().shared_contexts();
    let session_contexts = original.session().shared_contexts();
    assert!(Arc::ptr_eq(arrangement_contexts, session_contexts));
    assert!(arrangement_contexts.storage_bytes().unwrap() > 0);
    assert_eq!(arrangement_contexts.payload_bytes(), 16);
    let device = base.tracks()[0].instrument().unwrap().id();
    let index = arrangement_contexts.index_for_device(device).unwrap();
    let original_id = arrangement_contexts.owner(index).unwrap().id();
    assert_eq!(arrangement_contexts.index_for_id(original_id), Some(index));

    let compatible = Arc::new(
        PreparedRevision::prepare_with_instrument_registry(
            &base,
            &BTreeMap::new(),
            Arc::clone(&table),
            Some(original.session()),
            &registry,
        )
        .unwrap(),
    );
    assert_eq!(
        compatible
            .arrangement()
            .shared_contexts()
            .owner_for_device(device)
            .unwrap()
            .id(),
        original_id
    );
    assert!(Arc::ptr_eq(
        compatible.arrangement().shared_contexts(),
        compatible.session().shared_contexts()
    ));

    // Context identity belongs to the retained source. Moving its receiver
    // while keeping the immutable configuration reuses the same context.
    let mut moved_snapshot = base.snapshot();
    moved_snapshot.tracks[0].id = moved_snapshot.next_id;
    moved_snapshot.next_id += 1;
    moved_snapshot.session = Default::default();
    let moved = Project::from_legacy_snapshot(moved_snapshot).unwrap();
    let moved = PreparedRevision::prepare_with_instrument_registry(
        &moved,
        &BTreeMap::new(),
        Arc::clone(&table),
        Some(original.session()),
        &registry,
    )
    .unwrap();
    assert_eq!(
        moved
            .arrangement()
            .shared_contexts()
            .owner_for_device(device)
            .unwrap()
            .id(),
        original_id
    );

    let changed = with_test_instrument(&base, id, 1);
    let changed = PreparedRevision::prepare_with_instrument_registry(
        &changed,
        &BTreeMap::new(),
        table,
        Some(original.session()),
        &registry,
    )
    .unwrap();
    assert_ne!(
        changed
            .arrangement()
            .shared_contexts()
            .owner_for_device(device)
            .unwrap()
            .id(),
        original_id
    );
}

#[test]
fn shared_context_table_admits_the_full_track_bound_without_duplicate_owners() {
    let id = hex_lattice_contract::DeviceTypeId::from_static("test.instrument.shared.context");
    let definition = ContinuityInstrumentDefinition {
        id,
        levels: [0.25, 0.5],
        release_frames: 4,
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 128).unwrap();
    for index in 0..hex_project::MAX_TRACKS {
        let track = match editor
            .execute(Command::InsertTrackWithDevice {
                name: format!("context {index}"),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().expect("fixture instrument state"),
            })
            .unwrap()
        {
            Created::Track(track) => track,
            _ => panic!("MIDI track command returned an unexpected value"),
        };
        let device = editor
            .project()
            .track(track)
            .and_then(|track| track.instrument())
            .unwrap()
            .id();
        editor
            .execute(Command::ReplaceDeviceState {
                device,
                state: hex_lattice_contract::NativeDeviceState::new(id, 1, vec![0]).unwrap(),
            })
            .unwrap();
    }
    let prepared = revision_with_instrument_registry(editor.project(), table, &registry);
    let contexts = prepared.arrangement().shared_contexts();
    assert_eq!(contexts.context_count(), hex_project::MAX_TRACKS);
    assert_eq!(contexts.payload_bytes(), 16 * hex_project::MAX_TRACKS);
    assert_eq!(
        contexts.storage_bytes().unwrap(),
        std::mem::size_of_val(contexts.as_ref())
            + hex_project::MAX_TRACKS * std::mem::size_of::<(TrackId, hex_project::DeviceId)>()
            + contexts.context_count()
                * (std::mem::size_of::<Arc<crate::shared_analytic::SharedContextOwner>>()
                    + std::mem::size_of::<(crate::shared_analytic::SharedContextId, u16,)>())
    );
}

#[test]
fn shared_context_preparation_refuses_payload_above_per_context_limit() {
    let definition = OversizedSharedContextDefinition;
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let source = with_test_instrument(
        &project(0.5, 0, 8 << 32),
        hex_lattice_contract::DeviceTypeId::from_static("test.instrument.oversized-context"),
        0,
    );
    let result = PreparedRevision::prepare_with_instrument_registry(
        &source,
        &BTreeMap::new(),
        Arc::new(hex_devices::SineTable::new()),
        None,
        &registry,
    );
    assert_eq!(
        result.unwrap_err().0,
        "shared analytic context exceeds its byte limit"
    );
}

#[test]
pub(super) fn registered_mutable_instrument_adds_feedback_audio_in_arrangement_and_session() {
    let id = hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback");
    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id,
            levels: [0.2, 0.4],
            release_frames: 8,
        },
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let base = with_test_instrument(&project(1.0, 0, 8 << 32), id, 0);

    for session_owner in [false, true] {
        let source = if session_owner {
            session_only_project(&base)
        } else {
            base.clone()
        };
        let revision = revision_with_instrument_registry(&source, Arc::clone(&table), &registry);
        let mut whole = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let mut partitioned = SessionPlayback::new(revision).unwrap();
        if session_owner {
            let clip = whole.revision.session().midi_clips()[0].id;
            for playback in [&mut whole, &mut partitioned] {
                playback
                    .apply(SessionPlaybackCommand::LaunchClip {
                        clip,
                        quantization: LaunchQuantization::Immediate,
                    })
                    .unwrap();
                playback.apply(SessionPlaybackCommand::Play).unwrap();
            }
        } else {
            whole.apply(SessionPlaybackCommand::Play).unwrap();
            partitioned.apply(SessionPlaybackCommand::Play).unwrap();
        }
        let mut expected = [[0.0_f32; 2]; 16];
        let mut feedback = 0.0_f32;
        for frame in &mut expected {
            let direct = 0.2_f32;
            feedback = feedback * 0.5 + direct * 0.25;
            frame[0] = direct + feedback;
            frame[1] = direct + feedback;
        }
        let mut actual = [[0.0; 2]; 16];
        whole.render(&mut actual).unwrap();
        assert_eq!(
            actual, expected,
            "mutable runtime must add its independent feedback law once"
        );
        let mut first = [[0.0; 2]; 5];
        let mut rest = [[0.0; 2]; 11];
        partitioned.render(&mut first).unwrap();
        partitioned.render(&mut rest).unwrap();
        assert_eq!(first.as_slice(), &expected[..5]);
        assert_eq!(rest.as_slice(), &expected[5..]);
    }
}

#[test]
pub(super) fn prepared_revision_budget_counts_return_effects_with_mutable_instruments_transactionally()
 {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    let id = hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback");
    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id,
            levels: [0.2, 0.4],
            release_frames: 8,
        },
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let source = with_test_instrument(&project(1.0, 0, 8 << 32), id, 0);
    let filter_state = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            parameter::DC_REMOVE,
            ParameterValue::Bool(true),
        )
        .unwrap();
    let mut snapshot = source.snapshot();
    let mut next_id = snapshot.next_id;
    let mut return_tracks = Vec::new();
    for return_index in 0..hex_project::MAX_RETURN_TRACKS {
        let devices_in_chain = if return_index + 1 == hex_project::MAX_RETURN_TRACKS {
            15
        } else {
            16
        };
        let mut devices = Vec::new();
        for _ in 0..devices_in_chain {
            devices.push(DeviceSnapshot {
                modulation: Default::default(),
                parent: None,
                automation: Vec::new(),
                comparison: None,
                id: next_id,
                bypassed: false,
                state: filter_state.clone(),
            });
            next_id += 1;
        }
        return_tracks.push(hex_project::ReturnTrackSnapshot {
            id: next_id,
            name: format!("return {return_index}"),
            gain: 1.0,
            muted: false,
            devices,
        });
        next_id += 1;
    }
    snapshot.next_id = next_id;
    snapshot.return_tracks = return_tracks;
    let base = Project::from_legacy_snapshot(snapshot).unwrap();
    let return_track = base.return_tracks().last().unwrap().id();
    let mut editor = Editor::new(base, 16).unwrap();
    editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    assert!(editor.undo());
    assert!(editor.can_redo());
    // Exactly 255 stateful Returns plus one mutable instrument are admitted.
    // The candidate below adds the 256th Return effect and exceeds the joint cap.
    let predecessor = Arc::new(
        PreparedRevision::prepare_with_instrument_registry(
            editor.project(),
            &BTreeMap::new(),
            Arc::clone(&table),
            None,
            &registry,
        )
        .unwrap(),
    );
    let mut predecessor_playback = SessionPlayback::new(Arc::clone(&predecessor)).unwrap();
    let mut oracle_playback = SessionPlayback::new(Arc::clone(&predecessor)).unwrap();
    predecessor_playback
        .apply(SessionPlaybackCommand::Play)
        .unwrap();
    oracle_playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 16];
    predecessor_playback.render(&mut prefix).unwrap();
    oracle_playback.render(&mut prefix).unwrap();

    let before = editor.project().snapshot();
    let can_undo = editor.can_undo();
    let can_redo = editor.can_redo();
    let rejected = editor.execute_prepared(
        Command::InsertReturnDevice {
            return_track,
            before: None,
            state: filter_state,
        },
        |candidate| {
            PreparedRevision::prepare_with_instrument_registry(
                candidate,
                &BTreeMap::new(),
                Arc::clone(&table),
                Some(predecessor.session()),
                &registry,
            )
            .map_err(|_| hex_project::Invalid("prepared graph exceeded runtime budget"))
        },
    );
    assert_eq!(
        rejected.unwrap_err().0,
        "prepared graph exceeded runtime budget"
    );
    assert_eq!(editor.project().snapshot(), before);
    assert_eq!(editor.can_undo(), can_undo);
    assert_eq!(editor.can_redo(), can_redo);

    let mut after_rejection = [[0.0; 2]; 16];
    let mut oracle_after_rejection = [[0.0; 2]; 16];
    predecessor_playback.render(&mut after_rejection).unwrap();
    oracle_playback.render(&mut oracle_after_rejection).unwrap();
    assert_eq!(after_rejection, oracle_after_rejection);
}

#[test]
pub(super) fn unchanged_mutable_arrangement_and_session_owners_keep_history_across_editor_revisions()
 {
    use hex_project::{Command, Editor};

    let id = hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback");
    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id,
            levels: [0.2, 0.4],
            release_frames: 8,
        },
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());

    for (session_owner, note_duration, recent_release) in [
        (false, 8 << 32, false),
        (true, 8 << 32, false),
        (false, (1 << 32) / 6_000, true),
        (true, (1 << 32) / 6_000, true),
    ] {
        let base = with_test_instrument(&project(1.0, 0, note_duration), id, 0);
        let source = if session_owner {
            session_only_project(&base)
        } else {
            base
        };
        let initial_revision =
            revision_with_instrument_registry(&source, Arc::clone(&table), &registry);
        let mut playback = SessionPlayback::new(Arc::clone(&initial_revision)).unwrap();
        let mut uninterrupted = SessionPlayback::new(initial_revision).unwrap();
        if session_owner {
            let clip = playback.revision.session().midi_clips()[0].id;
            for owner in [&mut playback, &mut uninterrupted] {
                owner
                    .apply(SessionPlaybackCommand::LaunchClip {
                        clip,
                        quantization: LaunchQuantization::Immediate,
                    })
                    .unwrap();
            }
        }
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        uninterrupted.apply(SessionPlaybackCommand::Play).unwrap();

        let mut preroll_frames = 0;
        loop {
            let mut actual = [[0.0; 2]; 1];
            let mut expected = [[0.0; 2]; 1];
            playback.render(&mut actual).unwrap();
            uninterrupted.render(&mut expected).unwrap();
            assert_eq!(actual, expected);
            preroll_frames += 1;
            let runtime = playback.mutable_instruments[0].as_ref().unwrap();
            if recent_release {
                if runtime.committed_feedback_tail_is_active() {
                    break;
                }
            } else if preroll_frames == 16 {
                assert!(!runtime.committed_ledger_is_empty());
                break;
            }
            assert!(
                preroll_frames < 64,
                "fixture did not reach its test boundary"
            );
        }

        let mut editor = Editor::new(source, 8).unwrap();
        for name in ["first unrelated edit", "second unrelated edit"] {
            let track = editor.project().tracks()[0].id();
            let (_, candidate_revision) = editor
                .execute_prepared(
                    Command::RenameTrack {
                        track,
                        name: name.to_owned(),
                    },
                    |candidate| {
                        Ok::<_, hex_project::Invalid>(revision_with_instrument_registry(
                            candidate,
                            Arc::clone(&table),
                            &registry,
                        ))
                    },
                )
                .unwrap();
            let mut candidate = SessionPlayback::new(candidate_revision).unwrap();
            candidate.try_inherit_state_from(&mut playback).unwrap();
            assert!(
                playback
                    .tail_mutable_instruments
                    .iter()
                    .all(Option::is_none)
            );
            assert_eq!(
                candidate.mutable_instruments[0]
                    .as_ref()
                    .unwrap()
                    .next_occurrence_id(),
                uninterrupted.mutable_instruments[0]
                    .as_ref()
                    .unwrap()
                    .next_occurrence_id()
            );
            let mut actual = [[0.0; 2]; 24];
            let mut expected = [[0.0; 2]; 24];
            candidate.render(&mut actual).unwrap();
            uninterrupted.render(&mut expected).unwrap();
            assert_eq!(actual, expected);
            playback = candidate;
        }
    }
}

#[test]
fn unchanged_mutable_session_route_carries_across_another_route_source_edit() {
    use hex_lattice::NativeInstrumentDefinition;

    let id = hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback");
    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id,
            levels: [0.2, 0.4],
            release_frames: 8,
        },
    };
    let definitions: [&dyn NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let first_track = match editor
        .execute(Command::InsertTrackWithDevice {
            name: "short source".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap()
    {
        Created::Track(track) => track,
        _ => panic!("expected first MIDI track"),
    };
    let second_track = match editor
        .execute(Command::InsertTrackWithDevice {
            name: "held source".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap()
    {
        Created::Track(track) => track,
        _ => panic!("expected second MIDI track"),
    };
    let scene = match editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    {
        Created::Scene(scene) => scene,
        _ => panic!("expected Session scene"),
    };
    let mut first_note = None;
    for (track, pitch, duration) in [
        (first_track, 60, (1_u64 << 32) / 6_000),
        (second_track, 64, 4_u64 << 32),
    ] {
        let clip = match editor
            .execute(Command::CreateSessionMidiClip {
                track,
                scene,
                name: "source".into(),
                source_length: BeatTime::from_quarters(8).unwrap(),
            })
            .unwrap()
        {
            Created::SessionClip(clip) => clip,
            _ => panic!("expected Session MIDI clip"),
        };
        let note = match editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_raw(duration),
                pitch,
                channel: 0,
                velocity: 100,
            })
            .unwrap()
        {
            Created::Note(note) => note,
            _ => panic!("expected Session MIDI note"),
        };
        if track == first_track {
            first_note = Some(note);
        }
        let device = editor
            .project()
            .track(track)
            .and_then(|track| track.instrument())
            .expect("MIDI track has an instrument")
            .id();
        editor
            .execute(Command::ReplaceDeviceState {
                device,
                state: registry.default_state(id).unwrap(),
            })
            .unwrap();
    }
    let first_note = first_note.expect("first Session note was created");
    let original_revision =
        revision_with_instrument_registry(editor.project(), Arc::clone(&table), &registry);
    let mut previous = SessionPlayback::new(Arc::clone(&original_revision)).unwrap();
    let mut uninterrupted = SessionPlayback::new(original_revision).unwrap();
    for playback in [&mut previous, &mut uninterrupted] {
        playback
            .apply(SessionPlaybackCommand::LaunchScene {
                scene,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
    }

    // The short first source is finished and quiescent; the second Session
    // route still owns a held mutable occurrence at the source-edit boundary.
    let mut prefix = [[0.0; 2]; 64];
    let mut reference_prefix = [[0.0; 2]; 64];
    previous.render(&mut prefix).unwrap();
    uninterrupted.render(&mut reference_prefix).unwrap();
    assert_eq!(prefix, reference_prefix);
    assert!(
        previous.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .committed_is_quiescent()
    );
    assert!(
        !previous.mutable_instruments[1]
            .as_ref()
            .unwrap()
            .committed_is_quiescent()
    );

    editor
        .execute(Command::UpdateMidiNote {
            note: first_note,
            start: BeatTime::ZERO,
            duration: BeatTime::from_raw((1_u64 << 32) / 6_000),
            pitch: 61,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let mut candidate = SessionPlayback::new(revision_with_instrument_registry(
        editor.project(),
        table,
        &registry,
    ))
    .unwrap();
    candidate.try_inherit_state_from(&mut previous).unwrap();
    assert!(candidate.historical_floor.is_some());
    let second_index = candidate
        .routes
        .iter()
        .position(|route| route.id == second_track)
        .unwrap();
    assert!(
        candidate.mutable_instruments[second_index]
            .as_ref()
            .is_some_and(|runtime| !runtime.committed_is_quiescent())
    );
    assert!(
        candidate
            .tail_mutable_instruments
            .iter()
            .all(Option::is_none)
    );
    assert_eq!(
        candidate.mutable_instruments[second_index]
            .as_ref()
            .unwrap()
            .next_occurrence_id(),
        uninterrupted.mutable_instruments[1]
            .as_ref()
            .unwrap()
            .next_occurrence_id(),
        "global source-floor handoff carries the unchanged route's ledger"
    );
    let mut actual = [[0.0; 2]; 32];
    let mut expected = [[0.0; 2]; 32];
    candidate.render(&mut actual).unwrap();
    uninterrupted.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn mutable_arrangement_continuity_uses_track_ids_after_an_earlier_track_is_removed() {
    let id = hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback");
    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id,
            levels: [0.2, 0.4],
            release_frames: 8,
        },
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let first_track = match editor
        .execute(Command::InsertTrackWithDevice {
            name: "removed mutable track".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap()
    {
        Created::Track(track) => track,
        _ => panic!("expected first MIDI track"),
    };
    let retained_track = match editor
        .execute(Command::InsertTrackWithDevice {
            name: "retained mutable track".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap()
    {
        Created::Track(track) => track,
        _ => panic!("expected second MIDI track"),
    };
    for track in [first_track, retained_track] {
        let instrument = editor
            .project()
            .track(track)
            .and_then(|track| track.instrument())
            .expect("new MIDI track has an instrument")
            .id();
        editor
            .execute(Command::ReplaceDeviceState {
                device: instrument,
                state: hex_lattice_contract::NativeDeviceState::new(id, 1, vec![0]).unwrap(),
            })
            .unwrap();
        let clip = match editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(8).unwrap(),
            })
            .unwrap()
        {
            Created::Clip(clip) => clip,
            _ => panic!("expected Arrangement MIDI clip"),
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_quarters(4).unwrap(),
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
    }
    let original_revision =
        revision_with_instrument_registry(editor.project(), Arc::clone(&table), &registry);
    let mut playback = SessionPlayback::new(Arc::clone(&original_revision)).unwrap();
    let mut oracle = SessionPlayback::new(original_revision).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    oracle.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 32];
    playback.render(&mut prefix).unwrap();
    oracle.render(&mut prefix).unwrap();

    let (_, candidate_revision) = editor
        .execute_prepared(Command::RemoveTrack(first_track), |candidate| {
            Ok::<_, hex_project::Invalid>(revision_with_instrument_registry(
                candidate,
                Arc::clone(&table),
                &registry,
            ))
        })
        .unwrap();
    let mut retained_oracle = SessionPlayback::new(revision_with_instrument_registry(
        editor.project(),
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    retained_oracle.apply(SessionPlaybackCommand::Play).unwrap();
    let mut oracle_prefix = [[0.0; 2]; 32];
    retained_oracle.render(&mut oracle_prefix).unwrap();
    let mut candidate = SessionPlayback::new(candidate_revision).unwrap();
    candidate.try_inherit_state_from(&mut playback).unwrap();
    assert_eq!(candidate.routes[0].id, retained_track);
    assert!(
        candidate
            .tail_mutable_instruments
            .iter()
            .any(Option::is_some)
    );

    let mut actual = [[0.0; 2]; 32];
    let mut expected = [[0.0; 2]; 32];
    candidate.render(&mut actual).unwrap();
    retained_oracle.render(&mut expected).unwrap();
    assert!(
        actual[0][0] > expected[0][0],
        "removed route keeps its old-law tail"
    );
    // The removed route's declared release plus feedback tail is 16 frames;
    // after that, exact PCM must match the retained route's uninterrupted law.
    assert_eq!(&actual[16..], &expected[16..]);
}

#[test]
fn mutable_arrangement_source_change_does_not_take_the_exact_continuity_path() {
    use hex_project::{Command, Editor};

    let id = hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback");
    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id,
            levels: [0.2, 0.4],
            release_frames: 8,
        },
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let source = with_test_instrument(&project(1.0, 0, 8 << 32), id, 0);
    let mut editor = Editor::new(source, 8).unwrap();
    let mut playback = SessionPlayback::new(revision_with_instrument_registry(
        editor.project(),
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    let note = editor.project().tracks()[0].midi_clips()[0].notes()[0].id();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 8];
    playback.render(&mut prefix).unwrap();
    let frame_before_handoff = playback.performance_frame();

    let (_, candidate_revision) = editor
        .execute_prepared(
            Command::UpdateMidiNote {
                note,
                start: BeatTime::ZERO,
                duration: BeatTime::from_raw(8 << 32),
                pitch: 61,
                channel: 0,
                velocity: 100,
            },
            |candidate| {
                Ok::<_, hex_project::Invalid>(revision_with_instrument_registry(
                    candidate,
                    Arc::clone(&table),
                    &registry,
                ))
            },
        )
        .unwrap();
    let mut candidate = SessionPlayback::new(candidate_revision).unwrap();
    assert_eq!(
        candidate.try_inherit_state_from(&mut playback),
        Err(SessionPlaybackError::MutableInstrumentChase)
    );
    assert_eq!(playback.performance_frame(), frame_before_handoff);
    assert!(
        playback
            .tail_mutable_instruments
            .iter()
            .all(Option::is_none)
    );
    assert!(
        playback.mutable_instruments[0]
            .as_ref()
            .is_some_and(|runtime| !runtime.committed_is_quiescent())
    );
}

/// Field report (Pulse #3770): editing a note that is sounding under the
/// cursor refused publication with `MutableInstrumentChase`, so audio stayed on
/// the old revision. The strict planner still refuses; the live publication
/// path restarts audio at the cursor instead (playing or paused), chasing the
/// edited note, and a following edit applies too.
#[test]
fn live_handoff_restarts_an_unproven_chase_while_playing_or_paused() {
    use hex_project::{Command, Editor};

    let id = hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback");
    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id,
            levels: [0.2, 0.4],
            release_frames: 8,
        },
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let revision = |project: &hex_project::Project| {
        revision_with_instrument_registry(project, Arc::clone(&table), &registry)
    };
    for paused in [false, true] {
        let mut editor =
            Editor::new(with_test_instrument(&project(1.0, 0, 8 << 32), id, 0), 8).unwrap();
        let note = editor.project().tracks()[0].midi_clips()[0].notes()[0].id();
        let original = revision(editor.project());
        let mut playback = SessionPlayback::new(Arc::clone(&original)).unwrap();
        let mut facade_old =
            crate::playback::Playback::try_from_prepared_with_session(original).unwrap();
        facade_old.apply(crate::PlaybackCommand::Play);
        facade_old.render(&mut [[0.0; 2]; 8]).unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        playback.render(&mut [[0.0; 2]; 8]).unwrap();
        if paused {
            playback.apply(SessionPlaybackCommand::Pause).unwrap();
            facade_old.apply(crate::PlaybackCommand::Pause);
        }
        let frame = playback.performance_frame();
        editor
            .execute(Command::UpdateMidiNote {
                note,
                start: BeatTime::ZERO,
                duration: BeatTime::from_raw(8 << 32),
                pitch: 61,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
        let edited = revision(editor.project());
        let mut facade_next =
            crate::playback::Playback::try_from_prepared_with_session(Arc::clone(&edited)).unwrap();
        let context = crate::AudioSourceContext {
            project_generation: 1,
            editor_revision: 1,
        };
        let (_, facade_report) = facade_next
            .stream_playback(context)
            .preview_stream_handoff_with_report(
                &mut facade_old.stream_playback(context),
                64,
                None,
                &mut |_| Ok(()),
            )
            .unwrap();
        assert_eq!(facade_report.running_audio_restart(), !paused);
        facade_next
            .try_inherit_runtime_from(&mut facade_old)
            .unwrap();
        assert_eq!(
            SessionPlayback::new(Arc::clone(&edited))
                .unwrap()
                .try_inherit_state_from(&mut playback),
            Err(SessionPlaybackError::MutableInstrumentChase),
            "strict planner; paused={paused}"
        );
        let mut candidate = SessionPlayback::new(Arc::clone(&edited)).unwrap();
        let (_, preview) = candidate
            .preview_stream_handoff_with_report(&mut playback, 64, None, &mut |_| Ok(()))
            .unwrap();
        let (_, ordinary) = candidate
            .preview_stream_handoff_with_policy_and_report(
                &mut playback,
                64,
                None,
                &mut |_| Ok(()),
                true,
                false,
            )
            .unwrap();
        assert_eq!(playback.performance_frame(), frame);
        let committed = candidate
            .try_inherit_state_live_from(&mut playback)
            .unwrap();
        assert_eq!(preview.running_audio_restart(), !paused);
        assert_eq!(
            ordinary.running_audio_restart(),
            preview.running_audio_restart()
        );
        assert_eq!(
            preview.running_audio_restart(),
            committed.running_audio_restart()
        );
        assert_eq!(candidate.playing(), !paused);
        assert_eq!(candidate.performance_frame(), frame);

        editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
        let later_revision = revision(editor.project());
        let mut later = SessionPlayback::new(Arc::clone(&later_revision)).unwrap();
        later.try_inherit_state_live_from(&mut candidate).unwrap();
        assert_eq!(later.performance_frame(), frame);

        let mut oracle = SessionPlayback::new(later_revision).unwrap();
        oracle.play_from(frame).unwrap();
        if paused {
            later.apply(SessionPlaybackCommand::Play).unwrap();
        }
        let mut actual = [[0.0; 2]; 64];
        let mut expected = actual;
        later.render(&mut actual).unwrap();
        oracle.render(&mut expected).unwrap();
        assert_eq!(
            actual, expected,
            "restart chases the edited note; paused={paused}"
        );
        assert!(actual.iter().flatten().any(|sample| *sample != 0.0));
    }
}

/// In-place bounces (Pulse #3784) bake an instrument's output into PCM, so a
/// live handoff must not carry that instrument's release tail on top of it.
/// A requested restart starts empty at the cursor; ordinary inheritance keeps
/// the removed note's tail.
#[test]
fn requested_live_restart_drops_a_removed_note_release_tail() {
    use hex_project::{Command, Editor};

    let id = hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback");
    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id,
            levels: [0.2, 0.4],
            release_frames: 64,
        },
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let revision = |project: &hex_project::Project| {
        revision_with_instrument_registry(project, Arc::clone(&table), &registry)
    };
    let mut editor =
        Editor::new(with_test_instrument(&project(1.0, 0, 8 << 32), id, 0), 8).unwrap();
    let note = editor.project().tracks()[0].midi_clips()[0].notes()[0].id();
    let sounding = revision(editor.project());
    editor.execute(Command::RemoveMidiNote(note)).unwrap();
    let removed = revision(editor.project());
    let handoff = |restart: bool| {
        let mut playback = SessionPlayback::new(Arc::clone(&sounding)).unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        playback.render(&mut [[0.0; 2]; 8]).unwrap();
        let frame = playback.performance_frame();
        let mut candidate = SessionPlayback::new(Arc::clone(&removed)).unwrap();
        if restart {
            candidate.request_live_restart();
        }
        assert_eq!(candidate.live_restart_requested(), restart);
        let (_, preview) = candidate
            .preview_stream_handoff_with_report(&mut playback, 64, None, &mut |_| Ok(()))
            .unwrap();
        let (_, ordinary) = candidate
            .preview_stream_handoff_with_policy_and_report(
                &mut playback,
                64,
                None,
                &mut |_| Ok(()),
                true,
                false,
            )
            .unwrap();
        assert_eq!(playback.performance_frame(), frame);
        let committed = candidate
            .try_inherit_state_live_from(&mut playback)
            .unwrap();
        assert_eq!(preview.running_audio_restart(), restart);
        assert_eq!(
            ordinary.running_audio_restart(),
            preview.running_audio_restart()
        );
        assert_eq!(
            preview.running_audio_restart(),
            committed.running_audio_restart()
        );
        assert!(candidate.playing());
        assert_eq!(candidate.performance_frame(), frame);
        let mut pcm = [[0.0; 2]; 64];
        candidate.render(&mut pcm).unwrap();
        pcm
    };
    assert!(
        handoff(false).iter().flatten().any(|sample| *sample != 0.0),
        "ordinary inheritance keeps the removed note's tail"
    );
    assert_eq!(handoff(true), [[0.0; 2]; 64], "restart carries no tail");
}

#[test]
fn mutable_instrument_adds_to_hybrid_audio_before_ordered_suffix_effects() {
    use hex_lattice_contract::NativeDeviceMetadata as _;

    let id = hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback");
    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id,
            levels: [0.2, 0.4],
            release_frames: 8,
        },
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let mut snapshot = project(1.0, 0, 8 << 32).snapshot();
    let midi_clips = match std::mem::replace(
        &mut snapshot.tracks[0].content,
        TrackContentSnapshot::Audio(Vec::new()),
    ) {
        TrackContentSnapshot::Instrument(midi_clips)
        | TrackContentSnapshot::Hybrid { midi_clips, .. } => midi_clips,
        TrackContentSnapshot::Audio(_) => panic!("fixture has Arrangement MIDI"),
    };
    let asset = 20;
    let audio_clip = 21;
    let effect = 22;
    snapshot.next_id = 23;
    snapshot.assets.push(AssetSnapshot {
        original_source: None,
        id: asset,
        path: "mutable-hybrid.wav".into(),
        source_sample_rate: 48_000,
        source_frames: 16,
        playback_frames: 16,
        conversion_version: hex_project::IDENTITY_CONVERSION,
    });
    snapshot.tracks[0].content = TrackContentSnapshot::Hybrid {
        audio_clips: vec![ClipSnapshot {
            active: true,
            name: "clip".into(),
            flavor: hex_project::ClipFlavor::default(),
            edit: None,
            gain: hex_project::ClipGain::UNITY,
            id: audio_clip,
            asset,
            start: 0,
            duration: 16,
            source_offset: 0,
            source_length: 16,
            looping: false,
            source_phase: 0,
            fade: ClipFade::neutral(16).unwrap(),
        }],
        midi_clips,
    };
    snapshot.tracks[0].instrument.as_mut().unwrap().state =
        hex_lattice_contract::NativeDeviceState::new(id, 1, vec![0]).unwrap();
    let gain_state = hex_devices::utility::UTILITY_DEFINITION
        .patch_parameter(
            &hex_devices::utility::UTILITY_DEFINITION
                .default_state()
                .unwrap(),
            hex_devices::utility::parameter::GAIN,
            hex_lattice_contract::ParameterValue::Float(0.5),
        )
        .unwrap();
    snapshot.tracks[0].devices = vec![DeviceSnapshot {
        modulation: Default::default(),
        parent: None,
        automation: Vec::new(),
        comparison: None,
        id: effect,
        bypassed: false,
        state: gain_state,
    }];
    snapshot.tracks[0].instrument_position = Some(0);
    routed_midi_fixture::separate_midi(&mut snapshot, 0);
    let project = Project::from_legacy_snapshot(snapshot).unwrap();
    let asset_id = project.assets().next().unwrap().id();
    let media = BTreeMap::from([(
        asset_id,
        crate::Audio::new(48_000, vec![[0.3, -0.1]; 16]).unwrap(),
    )]);
    let prepared = Arc::new(
        PreparedRevision::prepare_with_instrument_registry(
            &project, &media, table, None, &registry,
        )
        .unwrap(),
    );
    let mut playback = SessionPlayback::new(prepared).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut actual = [[0.0; 2]; 16];
    playback.render(&mut actual).unwrap();
    let bypassed = with_instrument_bypassed(&project, true);
    let bypassed_revision = Arc::new(
        PreparedRevision::prepare_with_instrument_registry(
            &bypassed,
            &media,
            Arc::new(hex_devices::SineTable::new()),
            None,
            &registry,
        )
        .unwrap(),
    );
    let mut audio_only = SessionPlayback::new(bypassed_revision).unwrap();
    audio_only.apply(SessionPlaybackCommand::Play).unwrap();
    let mut audio_only_output = [[0.0; 2]; 16];
    audio_only.render(&mut audio_only_output).unwrap();

    let mut feedback = 0.0_f32;
    let expected_difference: Vec<_> = (0..16)
        .map(|_| {
            let direct = 0.2_f32;
            feedback = feedback * 0.5 + direct * 0.25;
            let post_suffix = (direct + feedback) * 0.5;
            [post_suffix, post_suffix]
        })
        .collect();
    for ((actual, audio_only), expected) in actual
        .iter()
        .zip(audio_only_output.iter())
        .zip(expected_difference.iter())
    {
        assert!((actual[0] - audio_only[0] - expected[0]).abs() <= 1.0e-6);
        assert!((actual[1] - audio_only[1] - expected[1]).abs() <= 1.0e-6);
    }
}

#[test]
pub(super) fn mutable_prefix_refusal_rolls_back_audio_transport_and_occurrence_ledger() {
    let id = hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback");
    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id,
            levels: [0.2, 0.4],
            release_frames: 8,
        },
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let project = with_test_instrument(&project(0.5, 0, 8 << 32), id, 0);
    let prepared = revision_with_instrument_registry(&project, Arc::clone(&table), &registry);
    let mut playback = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    let track = playback.routes[0].id;
    playback.routes[0].pending = Some(PendingRoute {
        note_chase: crate::NoteChase::On,
        frame: 8,
        beat: BeatTime::ZERO,
        target: RouteTarget::Silence,
        scene: None,
        current_reservation: None,
    });
    playback.apply(SessionPlaybackCommand::Play).unwrap();

    let mut refused = [[1.0; 2]; 16];
    assert_eq!(
        playback.render(&mut refused),
        Err(SessionPlaybackError::MutableInstrumentActive)
    );
    assert_eq!(refused, [[0.0; 2]; 16]);
    assert_eq!(playback.performance_frame(), 0);
    assert_eq!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Arrangement)
    );
    assert!(playback.routes[0].pending.is_some());
    assert!(
        playback.mutable_instruments[0]
            .as_ref()
            .is_some_and(MutableInstrumentTrackRuntime::committed_is_quiescent)
    );

    playback.apply(SessionPlaybackCommand::Pause).unwrap();
    playback
        .apply(SessionPlaybackCommand::CancelPending { track: Some(track) })
        .unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut retried = [[0.0; 2]; 16];
    playback.render(&mut retried).unwrap();

    let mut reference = SessionPlayback::new(prepared).unwrap();
    reference.apply(SessionPlaybackCommand::Play).unwrap();
    let mut expected = [[0.0; 2]; 16];
    reference.render(&mut expected).unwrap();
    assert_eq!(retried, expected);
    assert_eq!(playback.performance_frame(), 16);
}

#[test]
fn legacy_arrangement_renderers_refuse_enabled_mutable_instruments_atomically() {
    let id = hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback");
    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id,
            levels: [0.2, 0.4],
            release_frames: 8,
        },
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let source = with_test_instrument(&project(1.0, 0, 8 << 32), id, 0);
    let revision = revision_with_instrument_registry(
        &source,
        Arc::new(hex_devices::SineTable::new()),
        &registry,
    );
    let expected_output = [[0.25, -0.25]; 8];

    let mut renderer = crate::Renderer::new(revision.arrangement());
    renderer.play();
    let mut direct_output = expected_output;
    assert!(renderer.render(&mut direct_output).is_err());
    assert_eq!(renderer.position(), 0);
    assert_eq!(direct_output, expected_output);

    let arrangement = Arc::clone(revision.arrangement());
    let mut playback = crate::playback::Playback::from_shared(arrangement);
    playback.apply(crate::playback::PlaybackCommand::Play);
    let mut shared_output = expected_output;
    assert!(playback.render(&mut shared_output).is_err());
    assert_eq!(playback.position(), 0);
    assert_eq!(shared_output, expected_output);
}

#[test]
fn mutable_short_note_releases_inside_query_and_matches_partitioned_render() {
    let id = hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback");
    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id,
            levels: [0.2, 0.4],
            release_frames: 8,
        },
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let source = with_test_instrument(&project(1.0, 0, 1 << 20), id, 0);
    let table = Arc::new(hex_devices::SineTable::new());
    let revision = revision_with_instrument_registry(&source, table, &registry);
    let mut whole = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let mut partitioned = SessionPlayback::new(revision).unwrap();
    whole.apply(SessionPlaybackCommand::Play).unwrap();
    partitioned.apply(SessionPlaybackCommand::Play).unwrap();

    let mut expected = [[0.0; 2]; 32];
    let mut actual = [[0.0; 2]; 32];
    whole.render(&mut expected).unwrap();
    partitioned.render(&mut actual[..5]).unwrap();
    partitioned.render(&mut actual[5..16]).unwrap();
    partitioned.render(&mut actual[16..]).unwrap();

    assert_eq!(actual, expected);
    assert!(actual[..8].iter().any(|frame| *frame != [0.0; 2]));
    assert!(actual[24..].iter().all(|frame| *frame == [0.0; 2]));
}

#[test]
fn mutable_coincident_release_and_attack_keep_order_across_segment_boundaries() {
    use hex_lattice::NativeInstrumentEvent;

    let source = project(1.0, 0, 8 << 32);
    let track = source.tracks().first().unwrap();
    let clip = track.midi_clips().first().unwrap();
    let note = clip.notes().first().unwrap();
    let factory = MutableFeedbackFactory {
        level: 0.2,
        release_frames: 8,
    };
    let render = |segments: &[usize]| {
        let mut runtime =
            crate::mutable_instrument::MutableInstrumentTrackRuntime::new(&factory, 48_000)
                .unwrap();
        let mut onsets = crate::mutable_instrument::MutableOnsetBuffer::default();
        for (iteration, start, end, velocity) in [(0, 0, 4, 100), (1, 4, 8, 80)] {
            onsets
                .push(crate::mutable_instrument::MutableNoteOnset {
                    key: crate::mutable_instrument::MutableOccurrenceKey::new(
                        track.id(),
                        clip.id(),
                        note.id(),
                        iteration,
                        0,
                    ),
                    onset_beat: hex_project::BeatTime::from_raw(start << 32),
                    gate_end_beat: hex_project::BeatTime::from_raw(end << 32),
                    source_start: start,
                    source_gate_end: end,
                    pitch: 60,
                    channel: 0,
                    velocity,
                    contributor_fade_in: false,
                    expression: Default::default(),
                })
                .unwrap();
        }
        let mut audio = [[0.0; 2]; 28];
        let mut events = Vec::new();
        let mut observed = Vec::new();
        let mut start = 0;
        for &end in segments {
            runtime.begin_block();
            runtime
                .process_segment(
                    track.id(),
                    start as u64,
                    start as u64,
                    &mut audio[start..end],
                    &mut onsets,
                    &[],
                    &mut events,
                )
                .unwrap();
            observed.extend(events.iter().map(|event| match *event {
                NativeInstrumentEvent::NoteOn {
                    frame_offset,
                    occurrence_id,
                    ..
                } => (start + frame_offset, true, occurrence_id),
                NativeInstrumentEvent::NoteOff {
                    frame_offset,
                    occurrence_id,
                } => (start + frame_offset, false, occurrence_id),
                NativeInstrumentEvent::NotePressure { .. }
                | NativeInstrumentEvent::Expression { .. }
                | NativeInstrumentEvent::ControlChange { .. }
                | NativeInstrumentEvent::PitchBend { .. }
                | NativeInstrumentEvent::NotePitchBend { .. }
                | NativeInstrumentEvent::NoteTimbre { .. } => panic!("unexpected expression"),
            }));
            runtime.commit_block();
            start = end;
        }
        assert_eq!(start, audio.len());
        (audio, observed)
    };
    let (whole, events) = render(&[28]);
    assert_eq!(
        events,
        [(0, true, 1), (4, false, 1), (4, true, 2), (8, false, 2)]
    );
    // Independent fixture oracle: the first release remains sounding for
    // eight frames, overlapping the second note, then its feedback tail fades.
    let mut feedback = 0.0_f32;
    for (index, frame) in whole.iter().enumerate() {
        let first = if index < 12 { 0.2_f32 } else { 0.0 };
        let second = if (4..16).contains(&index) {
            0.2_f32 * 0.8
        } else {
            0.0
        };
        let direct = first + second;
        feedback = feedback * 0.5 + direct * 0.25;
        assert_eq!(*frame, [direct + feedback; 2], "frame {index}");
        if index == 22 {
            feedback = 0.0;
        }
    }
    for segments in [&[4, 8, 28][..], &[1, 3, 4, 5, 8, 9, 28][..]] {
        let (audio, actual_events) = render(segments);
        assert_eq!(actual_events, events, "segments {segments:?}");
        assert_eq!(audio, whole, "segments {segments:?}");
    }
}

#[test]
fn mutable_event_order_is_canonical_for_same_frame_notes_and_partitions() {
    let source = project(1.0, 0, 8 << 32);
    let track = source.tracks().first().unwrap();
    let clip = track.midi_clips().first().unwrap();
    let note = clip.notes().first().unwrap();
    let factory = MutableFeedbackFactory {
        level: 0.2,
        release_frames: 8,
    };

    let mut all_onsets = crate::mutable_instrument::MutableOnsetBuffer::default();
    // Deliberately reverse chronology and reverse the identity tie order. A
    // host/source iterator must not affect event IDs or voice-slot assignment.
    for (iteration, source_start, pitch, velocity) in [
        (4, 2, 68, 55),
        (3, 0, 72, 47),
        (2, 0, 64, 73),
        (1, 0, 59, 91),
    ] {
        all_onsets
            .push(crate::mutable_instrument::MutableNoteOnset {
                key: crate::mutable_instrument::MutableOccurrenceKey::new(
                    track.id(),
                    clip.id(),
                    note.id(),
                    iteration,
                    0,
                ),
                onset_beat: hex_project::BeatTime::from_raw(source_start << 32),
                gate_end_beat: hex_project::BeatTime::from_raw(8 << 32),
                source_start,
                source_gate_end: 8,
                pitch,
                channel: 0,
                velocity,
                contributor_fade_in: false,
                expression: Default::default(),
            })
            .unwrap();
    }

    let mut whole =
        crate::mutable_instrument::MutableInstrumentTrackRuntime::new(&factory, 48_000).unwrap();
    whole.begin_block();
    let mut whole_output = [[0.0; 2]; 8];
    let mut whole_events = Vec::new();
    let mut whole_onsets = all_onsets.clone();
    whole
        .process_segment(
            track.id(),
            0,
            0,
            &mut whole_output,
            &mut whole_onsets,
            &[],
            &mut whole_events,
        )
        .unwrap();
    let ordered: Vec<_> = whole_events
        .iter()
        .filter_map(|event| match *event {
            hex_lattice::NativeInstrumentEvent::NoteOn {
                frame_offset,
                occurrence_id,
                pitch,
                velocity,
            } => Some((frame_offset, occurrence_id, pitch, velocity)),
            hex_lattice::NativeInstrumentEvent::NoteOff { .. }
            | hex_lattice::NativeInstrumentEvent::NotePressure { .. }
            | hex_lattice::NativeInstrumentEvent::Expression { .. }
            | hex_lattice::NativeInstrumentEvent::ControlChange { .. }
            | hex_lattice::NativeInstrumentEvent::PitchBend { .. }
            | hex_lattice::NativeInstrumentEvent::NotePitchBend { .. }
            | hex_lattice::NativeInstrumentEvent::NoteTimbre { .. } => None,
        })
        .collect();
    assert_eq!(
        ordered,
        [
            (0, 1, 59, 91),
            (0, 2, 64, 73),
            (0, 3, 72, 47),
            (2, 4, 68, 55)
        ]
    );

    let terms = [(59_u8, 91_u8), (64_u8, 73_u8), (72_u8, 47_u8)];
    let direct = terms
        .iter()
        .map(|(pitch, velocity)| {
            0.2_f32 * (f32::from(*velocity) / 100.0) * (1.0 + (f32::from(*pitch) - 60.0) * 0.01)
        })
        .sum::<f32>();
    assert_eq!(whole_output[0], [direct + direct * 0.25; 2]);
    whole.commit_block();

    let mut partitioned =
        crate::mutable_instrument::MutableInstrumentTrackRuntime::new(&factory, 48_000).unwrap();
    partitioned.begin_block();
    let mut partitioned_output = [[0.0; 2]; 8];
    let mut partitioned_events = Vec::new();
    let mut partitioned_onsets = all_onsets;
    for (start, end) in [(0_usize, 1_usize), (1, 2), (2, 5), (5, 8)] {
        partitioned
            .process_segment(
                track.id(),
                start as u64,
                start as u64,
                &mut partitioned_output[start..end],
                &mut partitioned_onsets,
                &[],
                &mut partitioned_events,
            )
            .unwrap();
    }
    assert_eq!(partitioned_output, whole_output);
    partitioned.commit_block();
}

#[test]
fn mutable_occurrence_proof_survives_gateoff_and_suppression_matches_full_signature() {
    let source = project(1.0, 0, 8 << 32);
    let track = source.tracks().first().unwrap();
    let clip = track.midi_clips().first().unwrap();
    let note = clip.notes().first().unwrap();
    let factory = MutableFeedbackFactory {
        level: 0.2,
        release_frames: 8,
    };
    let key = crate::mutable_instrument::MutableOccurrenceKey::new(
        track.id(),
        clip.id(),
        note.id(),
        3,
        2,
    );
    let onset = crate::mutable_instrument::MutableNoteOnset {
        key,
        onset_beat: BeatTime::from_raw(0),
        gate_end_beat: BeatTime::from_raw(1 << 32),
        source_start: 0,
        source_gate_end: 1,
        pitch: 60,
        channel: 4,
        velocity: 91,
        contributor_fade_in: false,
        expression: Default::default(),
    };

    let mut old =
        crate::mutable_instrument::MutableInstrumentTrackRuntime::new(&factory, 48_000).unwrap();
    old.begin_block();
    let mut onsets = crate::mutable_instrument::MutableOnsetBuffer::default();
    onsets.push(onset).unwrap();
    let mut events = Vec::new();
    let mut audio = [[0.0; 2]; 4];
    old.process_segment(track.id(), 0, 0, &mut audio, &mut onsets, &[], &mut events)
        .unwrap();
    assert!(matches!(
        events.as_slice(),
        [
            hex_lattice::NativeInstrumentEvent::NoteOn { .. },
            hex_lattice::NativeInstrumentEvent::NoteOff { .. }
        ]
    ));
    old.commit_block();
    let proof = old.proofs().next().expect("gateoff keeps the owned proof");
    let old_deadline = 1 + old.max_release_tail_frames();
    assert_eq!(
        proof.signature,
        crate::mutable_instrument::MutableOccurrenceSignature::new(onset)
    );
    assert_eq!(proof.onset_frame, 0);
    assert_eq!(proof.gate_end_frame, 1);
    assert_eq!(proof.quiet_deadline_frame, old_deadline);
    assert!(proof.note_off_sent);

    let suppression =
        crate::midi_template::MidiOccurrenceSuppression::mutable(proof.signature, old_deadline);
    let mut candidate =
        crate::mutable_instrument::MutableInstrumentTrackRuntime::new(&factory, 48_000).unwrap();
    candidate.begin_block();
    let mut candidate_onsets = crate::mutable_instrument::MutableOnsetBuffer::default();
    candidate_onsets.push(onset).unwrap();
    let mut candidate_events = Vec::new();
    let mut candidate_audio = [[0.0; 2]; 4];
    candidate
        .process_segment(
            track.id(),
            0,
            0,
            &mut candidate_audio,
            &mut candidate_onsets,
            &[suppression],
            &mut candidate_events,
        )
        .unwrap();
    assert!(candidate_events.is_empty());
    assert_eq!(candidate_audio, [[0.0; 2]; 4]);
    candidate.commit_block();

    // A changed authored payload is a distinct occurrence and must not be
    // hidden by a same-ID suppression token.
    let mut changed = onset;
    changed.pitch += 1;
    let mut changed_runtime =
        crate::mutable_instrument::MutableInstrumentTrackRuntime::new(&factory, 48_000).unwrap();
    changed_runtime.begin_block();
    let mut changed_onsets = crate::mutable_instrument::MutableOnsetBuffer::default();
    changed_onsets.push(changed).unwrap();
    let mut changed_events = Vec::new();
    let mut changed_audio = [[0.0; 2]; 4];
    changed_runtime
        .process_segment(
            track.id(),
            0,
            0,
            &mut changed_audio,
            &mut changed_onsets,
            &[suppression],
            &mut changed_events,
        )
        .unwrap();
    assert!(matches!(
        changed_events.first(),
        Some(hex_lattice::NativeInstrumentEvent::NoteOn { pitch: 61, .. })
    ));
}

#[test]
fn mutable_zero_tail_emits_exclusive_boundary_noteoff_once_across_partitions() {
    let source = project(1.0, 0, 8 << 32);
    let track = source.tracks().first().unwrap();
    let clip = track.midi_clips().first().unwrap();
    let note = clip.notes().first().unwrap();
    let factory = ZeroTailMutableFactory;
    let mut runtime =
        crate::mutable_instrument::MutableInstrumentTrackRuntime::new(&factory, 48_000).unwrap();
    let onset = crate::mutable_instrument::MutableNoteOnset {
        key: crate::mutable_instrument::MutableOccurrenceKey::new(
            track.id(),
            clip.id(),
            note.id(),
            0,
            0,
        ),
        onset_beat: BeatTime::from_raw(0),
        gate_end_beat: BeatTime::from_raw(1 << 32),
        source_start: 0,
        source_gate_end: 4,
        pitch: 60,
        channel: 0,
        velocity: 100,
        contributor_fade_in: false,
        expression: Default::default(),
    };
    let mut onsets = crate::mutable_instrument::MutableOnsetBuffer::default();
    onsets.push(onset).unwrap();
    let mut events = Vec::new();

    runtime.begin_block();
    let mut attack = [[0.0; 2]; 4];
    runtime
        .process_segment(track.id(), 0, 0, &mut attack, &mut onsets, &[], &mut events)
        .unwrap();
    assert!(matches!(
        events.as_slice(),
        [hex_lattice::NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            ..
        }]
    ));
    assert_eq!(attack, [[0.25; 2]; 4]);
    runtime.commit_block();
    let proof = runtime
        .proofs()
        .next()
        .expect("exclusive gate keeps its proof");
    assert_eq!(proof.quiet_deadline_frame, 4);
    assert!(!proof.note_off_sent);

    // The next runtime block begins exactly at the exclusive gate boundary.
    // Split it again to prove partition independence and one-shot delivery.
    runtime.begin_block();
    onsets.clear();
    let mut release_boundary = [[0.0; 2]; 1];
    runtime
        .process_segment(
            track.id(),
            4,
            4,
            &mut release_boundary,
            &mut onsets,
            &[],
            &mut events,
        )
        .unwrap();
    assert!(matches!(
        events.as_slice(),
        [hex_lattice::NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            ..
        }]
    ));
    assert_eq!(release_boundary, [[0.0; 2]; 1]);

    let mut after_boundary = [[0.0; 2]; 3];
    runtime
        .process_segment(
            track.id(),
            5,
            5,
            &mut after_boundary,
            &mut onsets,
            &[],
            &mut events,
        )
        .unwrap();
    assert!(
        events.is_empty(),
        "the boundary NoteOff is sent exactly once"
    );
    assert_eq!(after_boundary, [[0.0; 2]; 3]);
    assert!(runtime.speculative_is_quiescent());
    runtime.commit_block();
    assert!(runtime.committed_is_quiescent());
    assert!(runtime.proofs().next().is_none());
}

#[test]
fn mutable_query_accepts_full_bounded_sequential_onset_and_edge_budget() {
    let source = project(1.0, 0, 8 << 32);
    let track = source.tracks().first().unwrap();
    let clip = track.midi_clips().first().unwrap();
    let note = clip.notes().first().unwrap();
    let factory = MutableFeedbackFactory {
        level: 0.2,
        release_frames: 8,
    };
    let mut runtime =
        crate::mutable_instrument::MutableInstrumentTrackRuntime::new(&factory, 48_000).unwrap();
    runtime.begin_block();
    let mut onsets = crate::mutable_instrument::MutableOnsetBuffer::default();
    for iteration in 0..128_u64 {
        let source_start = iteration * 2;
        onsets
            .push(crate::mutable_instrument::MutableNoteOnset {
                key: crate::mutable_instrument::MutableOccurrenceKey::new(
                    track.id(),
                    clip.id(),
                    note.id(),
                    iteration,
                    0,
                ),
                onset_beat: hex_project::BeatTime::from_raw(source_start << 32),
                gate_end_beat: hex_project::BeatTime::from_raw((source_start + 1) << 32),
                source_start,
                source_gate_end: source_start + 1,
                pitch: 60,
                channel: 0,
                velocity: 100,
                contributor_fade_in: false,
                expression: Default::default(),
            })
            .unwrap();
    }
    let mut events = Vec::new();
    let mut frames = [[0.0; 2]; 256];
    runtime
        .process_segment(track.id(), 0, 0, &mut frames, &mut onsets, &[], &mut events)
        .unwrap();
    assert_eq!(events.len(), 256);
    let mut release = [[0.0; 2]; 16];
    onsets.clear();
    runtime
        .process_segment(
            track.id(),
            256,
            256,
            &mut release,
            &mut onsets,
            &[],
            &mut events,
        )
        .unwrap();
    assert!(runtime.speculative_is_quiescent());
    runtime.commit_block();
    assert!(runtime.committed_is_quiescent());
}

#[test]
fn mutable_runtime_host_storage_budget_is_checked_and_reported() {
    let factory = MutableFeedbackFactory {
        level: 0.2,
        release_frames: 8,
    };
    let runtime =
        crate::mutable_instrument::MutableInstrumentTrackRuntime::new(&factory, 48_000).unwrap();
    let ledger_bytes = runtime.ledger_capacity_bytes().unwrap();
    let runtime_pair_bytes = runtime.runtime_pair_storage_bytes().unwrap();
    let graph_host_bytes =
        crate::mutable_instrument::host_storage_upper_bound(hex_project::MAX_TRACKS).unwrap();
    // The owner slots include the vector headers. Charge their reserved heap
    // entries independently of the production frozen-parameter quote.
    let frozen_parameter_bytes_per_owner = hex_lattice_contract::MAX_PARAMETERS_PER_DEVICE
        * (std::mem::size_of::<hex_lattice_contract::ParameterId>()
            + std::mem::size_of::<Option<hex_lattice_contract::ParameterValue>>());

    assert_eq!(
        ledger_bytes,
        2 * MAX_SIMULTANEOUS_VOICES_PER_TRACK
            * std::mem::size_of::<crate::mutable_instrument::MutableOccurrence>()
    );
    assert_eq!(
        graph_host_bytes,
        hex_project::MAX_TRACKS
            * std::mem::size_of::<Option<crate::mutable_instrument::MutableInstrumentTrackRuntime>>(
            )
            + hex_project::MAX_TRACKS
                * 2
                * MAX_SIMULTANEOUS_VOICES_PER_TRACK
                * std::mem::size_of::<crate::mutable_instrument::MutableOccurrence>()
            + hex_lattice::MAX_STATEFUL_PROCESSORS
                * std::mem::size_of::<Option<crate::mutable_instrument::MutableInstrumentTail>>()
            + hex_lattice::MAX_STATEFUL_PROCESSORS
                * 2
                * MAX_SIMULTANEOUS_VOICES_PER_TRACK
                * std::mem::size_of::<crate::mutable_instrument::MutableOccurrence>()
            + (hex_project::MAX_TRACKS + hex_lattice::MAX_STATEFUL_PROCESSORS)
                * frozen_parameter_bytes_per_owner
            + std::mem::size_of::<crate::mutable_instrument::MutableOnsetBuffer>()
            + crate::mutable_instrument::MAX_MUTABLE_EVENTS_PER_QUERY
                * std::mem::size_of::<hex_lattice::NativeInstrumentEvent>()
            + std::mem::size_of::<
                Vec<Option<crate::mutable_instrument::MutableInstrumentTrackRuntime>>,
            >()
            + std::mem::size_of::<Vec<Option<crate::mutable_instrument::MutableInstrumentTail>>>()
            + std::mem::size_of::<Vec<hex_lattice::NativeInstrumentEvent>>()
            + (hex_project::MAX_TRACKS + hex_lattice::MAX_STATEFUL_PROCESSORS)
                * crate::mutable_instrument::MutableInstrumentTrackRuntime::source_live_storage_upper_bound()
    );
    assert!(
        crate::mutable_instrument::host_storage_upper_bound(hex_project::MAX_TRACKS + 1).is_none()
    );
    let one_track_increment = std::mem::size_of::<
        Option<crate::mutable_instrument::MutableInstrumentTrackRuntime>,
    >() + ledger_bytes
        + frozen_parameter_bytes_per_owner
        + crate::mutable_instrument::MutableInstrumentTrackRuntime::source_live_storage_upper_bound(
        );
    assert_eq!(
        crate::mutable_instrument::host_storage_upper_bound(1).unwrap()
            - crate::mutable_instrument::host_storage_upper_bound(0).unwrap(),
        one_track_increment
    );
    println!(
        "mutable storage bytes: track_owner={}, ledger_pair={}, onset_buffer={}, event={}, runtime_pair={}, max_graph_host={graph_host_bytes}",
        std::mem::size_of::<Option<crate::mutable_instrument::MutableInstrumentTrackRuntime>>(),
        ledger_bytes,
        std::mem::size_of::<crate::mutable_instrument::MutableOnsetBuffer>(),
        std::mem::size_of::<hex_lattice::NativeInstrumentEvent>(),
        runtime_pair_bytes,
    );
}

#[test]
pub(super) fn mutable_revision_handoff_keeps_old_gate_and_suffix_until_tail_quiet() {
    let id = hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback");
    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id,
            levels: [0.2, 0.4],
            release_frames: 8,
        },
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let base = with_test_instrument(&project(0.5, 0, 1 << 24), id, 0);
    let prepared = revision_with_instrument_registry(&base, Arc::clone(&table), &registry);
    let clip = prepared.session().midi_clips()[0].id;
    let start = || {
        let mut playback = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
        playback
            .apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        playback
    };

    let mut active = start();
    let mut reference = start();
    let mut prefix = [[0.0; 2]; 16];
    let mut reference_prefix = [[0.0; 2]; 16];
    active.render(&mut prefix).unwrap();
    reference.render(&mut reference_prefix).unwrap();
    assert_eq!(prefix, reference_prefix);
    let frame_before_refusal = active.performance_frame();
    assert_eq!(
        active.apply(SessionPlaybackCommand::Seek(0)),
        Err(SessionPlaybackError::MutableInstrumentActive)
    );
    assert_eq!(active.performance_frame(), frame_before_refusal);

    let mut candidate_snapshot = base.snapshot();
    candidate_snapshot.tracks[0].content = TrackContentSnapshot::Instrument(Vec::new());
    candidate_snapshot.session.cells.clear();
    let source_empty = legacy_project_from_snapshot(candidate_snapshot).unwrap();
    let source_empty_prepared =
        revision_with_instrument_registry(&source_empty, Arc::clone(&table), &registry);
    let mut candidate = SessionPlayback::new(source_empty_prepared).unwrap();
    candidate.try_inherit_state_from(&mut active).unwrap();
    // Empty authoring does not replace the retained source or its live history.
    assert!(
        candidate
            .tail_mutable_instruments
            .iter()
            .all(Option::is_none)
    );
    assert!(
        !candidate.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .committed_is_quiescent()
    );
    assert_eq!(
        candidate.apply(SessionPlaybackCommand::Seek(0)),
        Err(SessionPlaybackError::MutableInstrumentActive)
    );
    let mut actual_tail = [[0.0; 2]; 256];
    let mut expected_tail = [[0.0; 2]; 256];
    candidate.render(&mut actual_tail).unwrap();
    reference.render(&mut expected_tail).unwrap();
    assert_eq!(actual_tail, expected_tail);
    assert!(
        candidate
            .tail_mutable_instruments
            .iter()
            .all(Option::is_none)
    );
    assert!(
        candidate.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .committed_is_quiescent()
    );
    candidate.apply(SessionPlaybackCommand::Seek(0)).unwrap();
}
