use super::*;

#[test]
fn held_eq8_audition_transfers_from_mutable_release_suffix_without_transferring_filter_history() {
    use hex_lattice::{ProcessorAuditionSelection, ProcessorChainOwner};
    use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};

    let id = DeviceTypeId::from_static("test.instrument.mutable.feedback");
    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id,
            levels: [0.2, 0.4],
            release_frames: 128,
        },
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let mut source_snapshot = with_test_instrument(&project(0.5, 0, 1 << 32), id, 0).snapshot();
    let eq8_type = DeviceTypeId::from_static("hex.eq8");
    let mut eq8_state =
        crate::default_native_device_state_for_class(crate::NativeDeviceClass::Effect, eq8_type)
            .unwrap();
    for parameter in [1, 41] {
        eq8_state = crate::patch_native_device_parameter_for_class(
            crate::NativeDeviceClass::Effect,
            &eq8_state,
            ParameterId::from_static(parameter),
            ParameterValue::Bool(true),
        )
        .unwrap();
    }
    let eq8_id = source_snapshot.next_id;
    source_snapshot.next_id += 1;
    source_snapshot.tracks[0].devices.push(DeviceSnapshot {
        modulation: Default::default(),
        parent: None,
        automation: Vec::new(),
        comparison: None,
        id: eq8_id,
        bypassed: false,
        state: eq8_state,
    });
    source_snapshot.tracks[0].instrument_position = Some(0);
    let source_project = legacy_project_from_snapshot(source_snapshot).unwrap();
    let source_revision =
        revision_with_instrument_registry(&source_project, Arc::clone(&table), &registry);
    let mut previous = SessionPlayback::new(source_revision).unwrap();
    let mut reference_previous = SessionPlayback::new(revision_with_instrument_registry(
        &source_project,
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    reference_previous
        .apply(SessionPlaybackCommand::Play)
        .unwrap();
    let mut playing = [[0.0; 2]; 16];
    let mut reference_playing = playing;
    previous.render(&mut playing).unwrap();
    reference_previous.render(&mut reference_playing).unwrap();
    assert!(playing.iter().any(|frame| frame[0].abs() > 1.0e-6));
    assert_eq!(
        playing, reference_playing,
        "independent source runtimes must begin with matching PCM"
    );

    let identity = crate::PostNodeIdentity::new(ProcessorChainOwner::Track(1), eq8_id, eq8_type);
    let selection = ProcessorAuditionSelection { index: 0, lane: 0 };
    assert!(previous.select_processor_audition(identity, Some(selection)));
    let mut auditioned = [[0.0; 2]; 16];
    let mut reference_auditioned = auditioned;
    previous.render(&mut auditioned).unwrap();
    reference_previous
        .render(&mut reference_auditioned)
        .unwrap();
    assert!(previous.processor_audition_is_active(identity, selection));

    // Empty the candidate's authored source while preserving the track and
    // device identities; the old live mutable note and its suffix become a
    // release owner on the candidate graph.
    let mut candidate_snapshot = source_project.snapshot();
    // A new device identity deliberately retires the old source. Clearing
    // authored clips alone keeps the existing one-source kernel in place.
    candidate_snapshot.tracks[0].instrument.as_mut().unwrap().id = candidate_snapshot.next_id;
    candidate_snapshot.next_id += 1;
    candidate_snapshot.tracks[0].content = TrackContentSnapshot::Instrument(Vec::new());
    candidate_snapshot.session.cells.clear();
    let candidate_project = legacy_project_from_snapshot(candidate_snapshot).unwrap();
    let mut candidate = SessionPlayback::new(revision_with_instrument_registry(
        &candidate_project,
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    let mut reference_candidate = SessionPlayback::new(revision_with_instrument_registry(
        &candidate_project,
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    candidate.try_inherit_state_from(&mut previous).unwrap();
    reference_candidate
        .try_inherit_state_from(&mut reference_previous)
        .unwrap();
    let tail_slot = candidate
        .tail_mutable_instruments
        .iter()
        .position(Option::is_some)
        .expect("live mutable note should own a release tail");
    assert_eq!(candidate.tail_suffix_starts[tail_slot], 0);
    assert!(
        candidate.tail_groups[tail_slot]
            .as_mut()
            .is_some_and(|chain| chain.processor_audition_is_active(
                identity.owner,
                identity.device_id,
                identity.device_type,
                selection
            ))
    );

    // Place an older same-track tail with no held selector before the freshly
    // transferred tail. This catches selecting the first track match instead
    // of the exact current lease owner.
    let stale_slot = 0;
    let current_slot = if tail_slot == stale_slot {
        1
    } else {
        tail_slot
    };
    let track_id = candidate.tail_mutable_instruments[tail_slot]
        .as_ref()
        .expect("mutable release owner")
        .track_id;
    let current_chain = candidate.tail_groups[tail_slot]
        .take()
        .expect("release suffix chain");
    let current_mutable_tail = candidate.tail_mutable_instruments[tail_slot]
        .take()
        .expect("mutable release tail");
    let suffix_start = candidate.tail_suffix_starts[tail_slot];
    candidate.tail_groups[stale_slot] = Some(hex_lattice::NativeRuntimeChain::empty());
    candidate.tail_suffix_starts[stale_slot] = suffix_start;
    candidate.tail_mutable_instruments[stale_slot] = Some(MutableInstrumentTail::new(
        track_id,
        crate::mutable_instrument::MutableInstrumentTrackRuntime::new(
            &ZeroTailMutableFactory,
            48_000,
        )
        .unwrap(),
        Default::default(),
        None,
    ));
    candidate.tail_groups[current_slot] = Some(current_chain);
    candidate.tail_suffix_starts[current_slot] = suffix_start;
    candidate.tail_mutable_instruments[current_slot] = Some(current_mutable_tail);

    assert!(candidate.transfer_processor_audition_from_release_tail(identity, selection));
    assert!(candidate.processor_audition_is_active(identity, selection));
    assert!(
        candidate.tail_groups[current_slot]
            .as_mut()
            .is_some_and(|chain| chain.processor_audition_is_active(
                identity.owner,
                identity.device_id,
                identity.device_type,
                selection
            ))
    );

    candidate.cancel_retired_processor_auditions();
    reference_candidate.cancel_retired_processor_auditions();
    assert!(candidate.processor_audition_is_active(identity, selection));
    assert!(
        !candidate.tail_groups[current_slot]
            .as_mut()
            .is_some_and(|chain| chain.processor_audition_is_active(
                identity.owner,
                identity.device_id,
                identity.device_type,
                selection
            ))
    );
    assert!(
        !candidate.tail_groups[stale_slot]
            .as_mut()
            .is_some_and(|chain| chain.processor_audition_is_active(
                identity.owner,
                identity.device_id,
                identity.device_type,
                selection
            ))
    );
    let reference_slot = reference_candidate
        .tail_groups
        .iter()
        .position(Option::is_some)
        .expect("reference mutable release suffix");
    let mut actual_tail_pcm = [[0.1, -0.2]; 256];
    let mut reference_tail_pcm = actual_tail_pcm;
    let actual_chain = candidate.tail_groups[current_slot]
        .as_mut()
        .expect("actual release suffix chain");
    actual_chain.begin_block();
    actual_chain.process_block(&mut actual_tail_pcm).unwrap();
    actual_chain.commit_block();
    let reference_chain = reference_candidate.tail_groups[reference_slot]
        .as_mut()
        .expect("reference release suffix chain");
    reference_chain.begin_block();
    reference_chain
        .process_block(&mut reference_tail_pcm)
        .unwrap();
    reference_chain.commit_block();
    assert_eq!(
        actual_tail_pcm, reference_tail_pcm,
        "hard-clearing the transferred tail selector must preserve ordinary EQ filter history PCM"
    );
}

#[test]
fn mutable_device_replacement_suppresses_exact_candidate_occurrence_across_repeated_handoff() {
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
    let old_revision = revision_with_instrument_registry(&source, Arc::clone(&table), &registry);
    let track = source.tracks()[0].id();
    let clip_id = source.tracks()[0].midi_clips()[0].id();
    let note = source.tracks()[0].midi_clips()[0].notes()[0].id();
    let mut previous = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
    let mut reference = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
    for playback in [&mut previous, &mut reference] {
        playback.apply(SessionPlaybackCommand::Play).unwrap();
    }
    let mut prefix = [[0.0; 2]; 1];
    let mut reference_prefix = [[0.0; 2]; 1];
    previous.render(&mut prefix).unwrap();
    reference.render(&mut reference_prefix).unwrap();
    assert_eq!(prefix, reference_prefix);
    let proof = previous.mutable_instruments[0]
        .as_ref()
        .and_then(|runtime| runtime.proofs().next())
        .expect("old mutable dispatch retains an occurrence proof");
    assert_eq!(proof.signature.key().track, track);
    assert_eq!(proof.signature.key().clip, clip_id);
    assert_eq!(proof.signature.key().note, note);

    let changed_source = with_test_instrument(&source, id, 1);
    let mut replacement = changed_source.snapshot();
    replacement.tracks[0].instrument.as_mut().unwrap().id = replacement.next_id;
    replacement.next_id += 1;
    let changed_source = legacy_project_from_snapshot(replacement).unwrap();
    reference
        .apply(SessionPlaybackCommand::StopTrack {
            track,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    let changed_revision =
        revision_with_instrument_registry(&changed_source, Arc::clone(&table), &registry);
    let mut candidate = SessionPlayback::new(Arc::clone(&changed_revision)).unwrap();
    candidate.try_inherit_state_from(&mut previous).unwrap();
    let suppression = candidate
        .suppressions
        .iter()
        .find(|entry| entry.mutable_signature() == Some(proof.signature))
        .copied()
        .expect("exact pre-cut occurrence is suppressed for its full old/candidate tail");
    assert_eq!(suppression.track, track);
    assert!(suppression.until_frame >= proof.quiet_deadline_frame);
    let mut actual = [[0.0; 2]; 4];
    let mut expected = [[0.0; 2]; 4];
    candidate.render(&mut actual).unwrap();
    reference.render(&mut expected).unwrap();
    assert_eq!(actual, expected);

    // The carried old owner and its exact suppression move forward once;
    // subsequent publications do not create a second tail for the same voice.
    let mut next = SessionPlayback::new(Arc::clone(&changed_revision)).unwrap();
    next.try_inherit_state_from(&mut candidate).unwrap();
    assert_eq!(
        next.tail_mutable_instruments
            .iter()
            .flatten()
            .filter(|tail| tail.track_id == track)
            .count(),
        1
    );
    assert!(next.suppressions.iter().any(|entry| {
        entry.mutable_signature() == Some(proof.signature)
            && entry.until_frame >= suppression.until_frame
    }));
    let mut repeated = [[0.0; 2]; 4];
    let mut expected_repeated = [[0.0; 2]; 4];
    next.render(&mut repeated).unwrap();
    reference.render(&mut expected_repeated).unwrap();
    assert_eq!(repeated, expected_repeated);
}

#[test]
#[ignore = "manual release profile for repeated mutable-owner admission"]
fn profile_mutable_occurrence_suppression_handoff() {
    use std::hint::black_box;
    use std::time::Instant;

    const SAMPLES: usize = 100;
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
    let mut dense_snapshot = project(1.0, 0, 8 << 32).snapshot();
    let first_new_id = dense_snapshot.next_id;
    dense_snapshot.next_id += 63;
    let clips = match &mut dense_snapshot.tracks[0].content {
        TrackContentSnapshot::Instrument(clips)
        | TrackContentSnapshot::Hybrid {
            midi_clips: clips, ..
        } => clips,
        TrackContentSnapshot::Audio(_) => panic!("profile fixture has MIDI content"),
    };
    let notes = &mut clips[0].notes;
    for index in 1..64_u64 {
        notes.push(MidiNoteSnapshot {
            active: true,
            id: first_new_id + index - 1,
            start_raw: 0,
            duration_raw: 8 << 32,
            pitch: u8::try_from(40 + index).unwrap(),
            channel: 0,
            velocity: 100,
            expression: Default::default(),
            expression_points: Vec::new(),
        });
    }
    let dense_source = legacy_project_from_snapshot(dense_snapshot).unwrap();
    let source = with_test_instrument(&dense_source, id, 0);
    let mut playback = SessionPlayback::new(revision_with_instrument_registry(
        &source,
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 1];
    playback.render(&mut prefix).unwrap();
    assert_eq!(
        playback.mutable_instruments[0]
            .as_ref()
            .map_or(0, |runtime| runtime.proofs().count()),
        64
    );
    let mut timings = Vec::with_capacity(SAMPLES);
    for sample in 0..SAMPLES {
        // Candidate preparation and destruction of the previous publication
        // stay outside the measured handoff call.
        let state = u8::try_from((sample + 1) % 2).unwrap();
        let candidate_source = with_test_instrument(&source, id, state);
        let revision =
            revision_with_instrument_registry(&candidate_source, Arc::clone(&table), &registry);
        let mut candidate = SessionPlayback::new(revision).unwrap();
        let start = Instant::now();
        candidate
            .try_inherit_state_from(black_box(&mut playback))
            .unwrap();
        timings.push(start.elapsed().as_nanos());
        black_box(candidate.suppressions.len());
        playback = candidate;
    }
    timings.sort_unstable();
    let p50 = timings[SAMPLES / 2];
    let p95 = timings[(SAMPLES * 95).div_ceil(100) - 1];
    let p99 = timings[(SAMPLES * 99).div_ceil(100) - 1];
    println!(
        "mutable occurrence handoff: samples={SAMPLES} dense_active_occurrences=64 frame={} proofs={} suppressions={} ledger_pair_bytes={} max_graph_host_bytes={}",
        playback.performance_frame(),
        playback.mutable_instruments[0]
            .as_ref()
            .map_or(0, |runtime| runtime.proofs().count()),
        playback.suppressions.len(),
        2 * crate::mutable_instrument::MAX_MUTABLE_OCCURRENCE_PROOFS
            * std::mem::size_of::<crate::mutable_instrument::MutableOccurrence>(),
        crate::mutable_instrument::host_storage_upper_bound(hex_project::MAX_TRACKS).unwrap(),
    );
    println!(
        "handoff_p50_us={:.3} p95_us={:.3} p99_us={:.3} max_us={:.3}",
        p50 as f64 / 1_000.0,
        p95 as f64 / 1_000.0,
        p99 as f64 / 1_000.0,
        timings[SAMPLES - 1] as f64 / 1_000.0,
    );
}

#[test]
fn mutable_to_analytic_handoff_refuses_active_chase_then_preserves_old_gate_and_suffix() {
    // Keep the seven independent oracle/candidate owners out of this test's
    // debug frame. Production handoff still runs on the default test stack.
    #[inline(never)]
    fn boxed_playback(revision: Arc<PreparedRevision>) -> Box<SessionPlayback> {
        Box::new(SessionPlayback::new(revision).unwrap())
    }

    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    let mutable_id =
        hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback");
    let analytic_id =
        hex_lattice_contract::DeviceTypeId::from_static("test.instrument.continuity.analytic");
    let mutable = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id: mutable_id,
            levels: [0.2, 0.4],
            release_frames: 128,
        },
    };
    let analytic = ContinuityInstrumentDefinition {
        id: analytic_id,
        levels: [0.2, 0.4],
        release_frames: 8,
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 2] = [&mutable, &analytic];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());

    let mut source = with_test_instrument(&project(0.5, 0, 1 << 24), mutable_id, 0).snapshot();
    let neutral = UTILITY_DEFINITION.default_state().unwrap();
    let dc_remove = UTILITY_DEFINITION
        .patch_parameter(&neutral, parameter::DC_REMOVE, ParameterValue::Bool(true))
        .unwrap();
    let first_id = source.next_id;
    source.tracks[0].devices = vec![
        hex_project::DeviceSnapshot {
            modulation: Default::default(),
            parent: None,
            automation: Vec::new(),
            comparison: None,
            id: first_id,
            bypassed: false,
            state: neutral,
        },
        hex_project::DeviceSnapshot {
            modulation: Default::default(),
            parent: None,
            automation: Vec::new(),
            comparison: None,
            id: first_id + 1,
            bypassed: false,
            state: dc_remove,
        },
    ];
    source.next_id += 2;
    let midi_clips = match std::mem::replace(
        &mut source.tracks[0].content,
        TrackContentSnapshot::Audio(Vec::new()),
    ) {
        TrackContentSnapshot::Instrument(midi_clips)
        | TrackContentSnapshot::Hybrid { midi_clips, .. } => midi_clips,
        TrackContentSnapshot::Audio(_) => panic!("fixture has Arrangement MIDI"),
    };
    let asset = source.next_id;
    let audio_clip = asset + 1;
    source.next_id += 2;
    source.assets.push(AssetSnapshot {
        original_source: None,
        id: asset,
        path: "mutable-tail-input.wav".into(),
        source_sample_rate: 48_000,
        source_frames: 64,
        playback_frames: 64,
        conversion_version: hex_project::IDENTITY_CONVERSION,
    });
    source.tracks[0].content = TrackContentSnapshot::Hybrid {
        audio_clips: vec![ClipSnapshot {
            active: true,
            name: "clip".into(),
            flavor: hex_project::ClipFlavor::default(),
            edit: None,
            gain: hex_project::ClipGain::UNITY,
            id: audio_clip,
            asset,
            start: 0,
            duration: 64,
            source_offset: 0,
            source_length: 64,
            looping: false,
            source_phase: 0,
            fade: ClipFade::neutral(64).unwrap(),
        }],
        midi_clips,
    };
    source.tracks[0].instrument_position = Some(1);
    routed_midi_fixture::separate_midi(&mut source, 0);
    let old_project = Project::from_legacy_snapshot(source).unwrap();
    let asset_id = old_project.assets().next().unwrap().id();
    let media = BTreeMap::from([(
        asset_id,
        crate::Audio::new(48_000, vec![[0.1, 0.03]; 64]).unwrap(),
    )]);
    let prepare = |project: &Project, table: Arc<hex_devices::SineTable>| {
        Arc::new(
            PreparedRevision::prepare_with_instrument_registry(
                project, &media, table, None, &registry,
            )
            .unwrap(),
        )
    };
    let old_revision = prepare(&old_project, Arc::clone(&table));
    let mut previous = boxed_playback(Arc::clone(&old_revision));
    let mut reference = boxed_playback(old_revision);
    for playback in [&mut previous, &mut reference] {
        playback.apply(SessionPlaybackCommand::Play).unwrap();
    }
    let mut prefix = [[0.0; 2]; 16];
    let mut reference_prefix = [[0.0; 2]; 16];
    previous.render(&mut prefix).unwrap();
    reference.render(&mut reference_prefix).unwrap();
    assert_eq!(prefix, reference_prefix);
    assert!(prefix.iter().any(|frame| frame[0].abs() > 1.0e-6));

    // The unchanged, enabled analytic candidate would sound the same
    // pre-cut occurrence alongside the retained mutable owner. Refuse it
    // atomically until mutable occurrence suppression is available.
    let changed_source = with_test_instrument(&old_project, analytic_id, 0);
    let changed_revision = prepare(&changed_source, Arc::clone(&table));
    let mut refused = boxed_playback(changed_revision);
    let previous_frame = previous.performance_frame();
    assert_eq!(
        refused.try_inherit_state_from(&mut previous),
        Err(SessionPlaybackError::SourceHistoryUnavailable)
    );
    assert_eq!(previous.performance_frame(), previous_frame);
    assert!(
        previous.mutable_instruments[0]
            .as_ref()
            .is_some_and(|runtime| !runtime.committed_is_quiescent())
    );

    // A source-empty enabled analytic replacement is safe. The old held
    // occurrence keeps its original gate and captured DC-removal suffix.
    let mut tail_snapshot = old_project.snapshot();
    if let TrackContentSnapshot::Hybrid {
        audio_clips,
        midi_clips,
    } = &mut tail_snapshot.tracks[0].content
    {
        audio_clips.clear();
        midi_clips.clear();
    }
    if let TrackContentSnapshot::Hybrid { midi_clips, .. } = &mut tail_snapshot.tracks[1].content {
        midi_clips.clear();
    }
    tail_snapshot.session.cells.clear();
    let tail_project = Project::from_legacy_snapshot(tail_snapshot).unwrap();
    let mut tail_candidate = boxed_playback(prepare(&tail_project, Arc::clone(&table)));
    tail_candidate
        .try_inherit_state_from(&mut reference)
        .unwrap();

    let mut candidate_snapshot = old_project.snapshot();
    if let TrackContentSnapshot::Hybrid { midi_clips, .. } =
        &mut candidate_snapshot.tracks[0].content
    {
        midi_clips.clear();
    }
    if let TrackContentSnapshot::Hybrid { midi_clips, .. } =
        &mut candidate_snapshot.tracks[1].content
    {
        midi_clips.clear();
    }
    candidate_snapshot.session.cells.clear();
    let candidate_source = Project::from_legacy_snapshot(candidate_snapshot).unwrap();
    let empty_analytic = with_test_instrument(&candidate_source, analytic_id, 0);
    let mut replaced = empty_analytic.snapshot();
    replaced.tracks[0].instrument.as_mut().unwrap().id = replaced.next_id;
    replaced.next_id += 1;
    let empty_analytic = legacy_project_from_snapshot(replaced).unwrap();
    tail_candidate
        .apply(SessionPlaybackCommand::StopTrack {
            track: old_project.tracks()[0].id(),
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    let empty_revision = prepare(&empty_analytic, Arc::clone(&table));
    let mut candidate = boxed_playback(empty_revision);
    candidate.try_inherit_state_from(&mut previous).unwrap();
    let mut actual = [[0.0; 2]; 16];
    let mut old_tail_pcm = [[0.0; 2]; 16];
    let mut new_audio_pcm = [[0.0; 2]; 16];
    candidate.render(&mut actual).unwrap();
    tail_candidate.render(&mut old_tail_pcm).unwrap();
    let mut fresh_audio = boxed_playback(prepare(&candidate_source, Arc::clone(&table)));
    fresh_audio.apply(SessionPlaybackCommand::Seek(16)).unwrap();
    fresh_audio.apply(SessionPlaybackCommand::Play).unwrap();
    fresh_audio.render(&mut new_audio_pcm).unwrap();
    for ((actual, old_tail), new_audio) in actual
        .iter()
        .zip(old_tail_pcm.iter())
        .zip(new_audio_pcm.iter())
    {
        assert!((actual[0] - old_tail[0] - new_audio[0]).abs() <= 1.0e-6);
        assert!((actual[1] - old_tail[1] - new_audio[1]).abs() <= 1.0e-6);
    }
    assert!(actual.iter().any(|frame| frame[0].abs() > 1.0e-6));

    // A retained old mutable owner also fences a later candidate that would
    // reintroduce the still-sounding pre-cut occurrence.
    let later_active = with_test_instrument(&old_project, analytic_id, 0);
    let later_revision = prepare(&later_active, Arc::clone(&table));
    let mut retained_tail_chase = boxed_playback(later_revision);
    let tail_frame = candidate.performance_frame();
    assert_eq!(
        retained_tail_chase.try_inherit_state_from(&mut candidate),
        Err(SessionPlaybackError::MutableInstrumentActive)
    );
    assert_eq!(candidate.performance_frame(), tail_frame);
    assert!(
        candidate
            .tail_mutable_instruments
            .iter()
            .flatten()
            .any(|tail| !tail.runtime.committed_is_quiescent())
    );
}

#[test]
pub(super) fn mutable_handoff_keeps_old_suffix_and_refusal_is_retryable_after_tail_processing() {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    let id = hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback");
    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id,
            levels: [0.2, 0.4],
            release_frames: 128,
        },
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let base = with_test_instrument(&project(0.5, 0, 1 << 24), id, 0);
    let mut old_snapshot = base.snapshot();
    let neutral = UTILITY_DEFINITION.default_state().unwrap();
    let dc_remove = UTILITY_DEFINITION
        .patch_parameter(&neutral, parameter::DC_REMOVE, ParameterValue::Bool(true))
        .unwrap();
    let first_id = old_snapshot.next_id;
    old_snapshot.tracks[0].devices = vec![
        hex_project::DeviceSnapshot {
            modulation: Default::default(),
            parent: None,
            automation: Vec::new(),
            comparison: None,
            id: first_id,
            bypassed: false,
            state: neutral,
        },
        hex_project::DeviceSnapshot {
            modulation: Default::default(),
            parent: None,
            automation: Vec::new(),
            comparison: None,
            id: first_id + 1,
            bypassed: false,
            state: dc_remove,
        },
    ];
    old_snapshot.next_id += 2;
    old_snapshot.tracks[0].instrument_position = Some(1);
    let old_project = legacy_project_from_snapshot(old_snapshot).unwrap();
    let old_revision =
        revision_with_instrument_registry(&old_project, Arc::clone(&table), &registry);
    let clip = old_revision.session().midi_clips()[0].id;
    let start = || {
        let mut playback = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
        playback
            .apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        playback
    };
    let mut previous = start();
    let mut reference = start();
    let mut prefix = [[0.0; 2]; 16];
    let mut reference_prefix = [[0.0; 2]; 16];
    previous.render(&mut prefix).unwrap();
    reference.render(&mut reference_prefix).unwrap();
    assert_eq!(prefix, reference_prefix);

    // Keep the instrument enabled but remove candidate source occurrences.
    // Existing mutable voices must retain their original gate deadline.
    let mut candidate_snapshot = old_project.snapshot();
    // A new device identity deliberately retires the old source. Clearing
    // authored clips alone keeps the existing one-source kernel in place.
    candidate_snapshot.tracks[0].instrument.as_mut().unwrap().id = candidate_snapshot.next_id;
    candidate_snapshot.next_id += 1;
    candidate_snapshot.tracks[0].content = TrackContentSnapshot::Instrument(Vec::new());
    candidate_snapshot.session.cells.clear();
    let candidate_project = legacy_project_from_snapshot(candidate_snapshot).unwrap();
    let candidate_project = with_utility_filter_state_at(&candidate_project, 1, false, true);
    let candidate_revision =
        revision_with_instrument_registry(&candidate_project, Arc::clone(&table), &registry);
    let mut candidate = SessionPlayback::new(candidate_revision).unwrap();

    // A full preallocated tail bank refuses before either graph is mutated.
    for slot in &mut candidate.tail_groups {
        *slot = Some(hex_lattice::NativeRuntimeChain::empty());
    }
    let previous_frame = previous.performance_frame;
    let previous_routes = previous.routes.clone();
    let previous_proofs: Vec<_> = previous.mutable_instruments[0]
        .as_ref()
        .unwrap()
        .proofs()
        .map(|proof| {
            (
                proof.signature,
                proof.id,
                proof.onset_frame,
                proof.gate_end_frame,
                proof.quiet_deadline_frame,
                proof.note_off_sent,
            )
        })
        .collect();
    let previous_suppressions = previous.suppressions.clone();
    assert_eq!(
        candidate.try_inherit_state_from(&mut previous),
        Err(SessionPlaybackError::ReleaseBankFull)
    );
    assert_eq!(previous.performance_frame, previous_frame);
    assert_eq!(previous.routes, previous_routes);
    assert_eq!(previous.suppressions, previous_suppressions);
    assert_eq!(
        previous.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .proofs()
            .map(|proof| {
                (
                    proof.signature,
                    proof.id,
                    proof.onset_frame,
                    proof.gate_end_frame,
                    proof.quiet_deadline_frame,
                    proof.note_off_sent,
                )
            })
            .collect::<Vec<_>>(),
        previous_proofs
    );
    assert!(
        previous.mutable_instruments[0]
            .as_ref()
            .is_some_and(|runtime| !runtime.committed_is_quiescent())
    );
    for slot in &mut candidate.tail_groups {
        *slot = None;
    }
    // Replacing the source releases the old clip gate. The independent
    // stopped-track oracle retains the exact same original suffix history.
    reference
        .apply(SessionPlaybackCommand::StopTrack {
            track: old_project.tracks()[0].id(),
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    candidate.try_inherit_state_from(&mut previous).unwrap();
    assert!(previous.mutable_instruments[0].is_none());
    let tail_group = candidate
        .tail_mutable_instruments
        .iter()
        .position(Option::is_some)
        .unwrap();
    assert_eq!(candidate.tail_suffix_starts[tail_group], 1);
    assert_eq!(candidate.tail_groups[tail_group].as_ref().unwrap().len(), 2);

    // Fail after the mutable tail has processed but before the captured suffix
    // completes. The block transaction must roll its runtime back for retry.
    candidate.tail_suffix_starts[tail_group] = usize::MAX;
    let frame_before_failure = candidate.performance_frame;
    let mut failed = [[1.0; 2]; 16];
    assert_eq!(
        candidate.render(&mut failed),
        Err(SessionPlaybackError::DeviceRuntime)
    );
    assert_eq!(failed, [[0.0; 2]; 16]);
    assert_eq!(candidate.performance_frame, frame_before_failure);
    candidate.tail_suffix_starts[tail_group] = 1;

    let mut actual = [[0.0; 2]; 16];
    let mut expected = [[0.0; 2]; 16];
    candidate.render(&mut actual).unwrap();
    reference.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    assert!(actual.iter().any(|frame| frame[0].abs() > 1.0e-6));
    assert!(
        candidate.tail_mutable_instruments[tail_group]
            .as_ref()
            .is_some_and(|tail| !tail.runtime.committed_is_quiescent())
    );

    // A second publication carries the same still-live old owner without
    // recapturing or resetting its held occurrence.
    let mut next = SessionPlayback::new(Arc::clone(&candidate.revision)).unwrap();
    next.try_inherit_state_from(&mut candidate).unwrap();
    let mut next_pcm = [[0.0; 2]; 16];
    let mut reference_pcm = [[0.0; 2]; 16];
    next.render(&mut next_pcm).unwrap();
    reference.render(&mut reference_pcm).unwrap();
    assert_eq!(next_pcm, reference_pcm);
    assert!(
        next.tail_mutable_instruments
            .iter()
            .flatten()
            .any(|tail| { !tail.runtime.committed_is_quiescent() })
    );
}

#[test]
fn mutable_power_fades_held_and_already_released_feedback() {
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

    // Compare the held waveform against an independent instance of the
    // original feedback law before applying the host de-click.
    let base = with_test_instrument(&project(0.5, 0, 1 << 24), id, 0);
    let old_revision = revision_with_instrument_registry(&base, Arc::clone(&table), &registry);
    let clip = old_revision.session().midi_clips()[0].id;
    let mut previous = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
    previous
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 16];
    previous.render(&mut prefix).unwrap();

    let disabled = with_instrument_bypassed(&base, true);
    let disabled_revision =
        revision_with_instrument_registry(&disabled, Arc::clone(&table), &registry);
    let mut candidate = SessionPlayback::new(disabled_revision).unwrap();
    candidate.try_inherit_state_from(&mut previous).unwrap();
    let mut oracle = MutableFeedbackRuntime {
        key: 0.2_f32.to_bits(),
        level: 0.2,
        release_frames: 8,
        voices: [MutableFeedbackVoice::default(); 64],
        feedback: 0.0,
        tail_left: 0,
        storage: Box::default(),
    };
    let note_on = [hex_lattice::NativeInstrumentEvent::NoteOn {
        frame_offset: 0,
        occurrence_id: 1,
        pitch: 60,
        velocity: 100,
    }];
    let mut oracle_prefix = [[0.0; 2]; 16];
    hex_lattice::MutableInstrumentRuntime::process_block(
        &mut oracle,
        &note_on,
        &mut oracle_prefix,
        &hex_lattice::EMPTY_DEVICE_RESOURCES,
    );
    const FADE: usize = 240;
    let mut expected = [[0.0; 2]; 16];
    hex_lattice::MutableInstrumentRuntime::process_block(
        &mut oracle,
        &[],
        &mut expected,
        &hex_lattice::EMPTY_DEVICE_RESOURCES,
    );
    for (offset, frame) in expected.iter_mut().enumerate() {
        let gain = (FADE - offset) as f32 / FADE as f32;
        *frame = frame.map(|sample| sample * gain * 0.5);
    }
    let mut actual = [[0.0; 2]; 16];
    candidate.render(&mut actual).unwrap();
    assert_eq!(actual, expected);
    // A note already releasing keeps its sample history under the same fade.
    let short = with_test_instrument(&project(0.5, 0, 1 << 24), id, 0);
    let short_revision = revision_with_instrument_registry(&short, Arc::clone(&table), &registry);
    let short_clip = short_revision.session().midi_clips()[0].id;
    let mut old = SessionPlayback::new(Arc::clone(&short_revision)).unwrap();
    let mut reference = SessionPlayback::new(Arc::clone(&short_revision)).unwrap();
    for playback in [&mut old, &mut reference] {
        playback
            .apply(SessionPlaybackCommand::LaunchClip {
                clip: short_clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
    }
    let mut old_prefix = [[0.0; 2]; 96];
    let mut reference_prefix = [[0.0; 2]; 96];
    old.render(&mut old_prefix).unwrap();
    reference.render(&mut reference_prefix).unwrap();
    assert_eq!(old_prefix, reference_prefix);
    assert!(
        old.mutable_instruments[0]
            .as_ref()
            .is_some_and(|runtime| !runtime.committed_is_quiescent())
    );
    let disabled_short = with_instrument_bypassed(&short, true);
    let disabled_short_revision =
        revision_with_instrument_registry(&disabled_short, Arc::clone(&table), &registry);
    let mut next = SessionPlayback::new(disabled_short_revision).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut actual_release = [[0.0; 2]; 16];
    let mut expected_release = [[0.0; 2]; 16];
    next.render(&mut actual_release).unwrap();
    reference.render(&mut expected_release).unwrap();
    for (offset, frame) in expected_release.iter_mut().enumerate() {
        let gain = (FADE - offset) as f32 / FADE as f32;
        *frame = frame.map(|sample| sample * gain);
    }
    assert_eq!(actual_release, expected_release);
}

#[test]
fn activation_mute_cuts_held_mutable_gate_with_instrument_still_enabled() {
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
    for session in [false, true] {
        let base = with_test_instrument(&project(0.5, 0, 1 << 28), id, 0);
        let old_revision = revision_with_instrument_registry(&base, Arc::clone(&table), &registry);
        let clip = if session {
            old_revision.session().midi_clips()[0].id
        } else {
            base.tracks()[0].midi_clips()[0].id()
        };
        let mut old = SessionPlayback::new(old_revision).unwrap();
        if session {
            old.apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        }
        old.apply(SessionPlaybackCommand::Play).unwrap();
        old.render(&mut [[0.0; 2]; 16]).unwrap();
        let mut editor = Editor::new(base, 8).unwrap();
        editor
            .execute(Command::SetClipActive {
                clip,
                active: false,
            })
            .unwrap();
        let mut candidate = SessionPlayback::new(revision_with_instrument_registry(
            editor.project(),
            Arc::clone(&table),
            &registry,
        ))
        .unwrap();
        candidate.try_inherit_state_from(&mut old).unwrap();
        // Clip activation changes the authored gate, not the source DSP owner.
        assert!(
            candidate
                .tail_mutable_instruments
                .iter()
                .all(Option::is_none)
        );
        let source = candidate.mutable_instruments[0].as_ref().unwrap();
        assert!(source.proofs().all(|proof| proof.gate_end_frame == 16));
        let mut actual = [[0.0; 2]; 16];
        candidate.render(&mut actual).unwrap();
        assert!(actual[..8].iter().any(|frame| frame[0] != 0.0));
        assert!(
            candidate.mutable_instruments[0]
                .as_ref()
                .unwrap()
                .committed_is_quiescent()
        );
        assert!(candidate.revision.arrangement().tracks()[0].instrument_slot_enabled());
    }
}

#[test]
fn note_activation_cuts_the_held_mutable_gate() {
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
    for session in [false, true] {
        let base = with_test_instrument(&project(0.5, 0, 1 << 28), id, 0);
        let old_revision = revision_with_instrument_registry(&base, Arc::clone(&table), &registry);
        let clip = if session {
            old_revision.session().midi_clips()[0].id
        } else {
            base.tracks()[0].midi_clips()[0].id()
        };
        let note = base.midi_source(clip).unwrap().notes()[0].id();
        let mut old = SessionPlayback::new(old_revision).unwrap();
        if session {
            old.apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        }
        old.apply(SessionPlaybackCommand::Play).unwrap();
        old.render(&mut [[0.0; 2]; 16]).unwrap();
        let mut editor = Editor::new(base, 8).unwrap();
        editor
            .execute(Command::SetMidiNotesActive {
                clip,
                notes: vec![note],
                active: false,
            })
            .unwrap();
        let mut candidate = SessionPlayback::new(revision_with_instrument_registry(
            editor.project(),
            Arc::clone(&table),
            &registry,
        ))
        .unwrap();
        candidate.try_inherit_state_from(&mut old).unwrap();
        let source = candidate.mutable_instruments[0].as_ref().unwrap();
        assert!(source.proofs().all(|proof| proof.gate_end_frame == 16));
        let mut released = [[0.0; 2]; 16];
        candidate.render(&mut released).unwrap();
        assert!(
            candidate.mutable_instruments[0]
                .as_ref()
                .unwrap()
                .committed_is_quiescent()
        );
        assert!(candidate.revision.arrangement().tracks()[0].instrument_slot_enabled());
    }
}

#[test]
fn disabling_one_note_keeps_another_held_note_sounding() {
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
    let base = with_test_instrument(&project(0.5, 0, 1 << 28), id, 0);
    let mut editor = Editor::new(base, 8).unwrap();
    let clip = editor.project().tracks()[0].midi_clips()[0].id();
    let muted_note = editor.project().midi_clip(clip).unwrap().notes()[0].id();
    let Created::Note(other_note) = editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_raw(1 << 28),
            pitch: 64,
            channel: 0,
            velocity: 100,
        })
        .unwrap()
    else {
        panic!("note")
    };
    let old_revision =
        revision_with_instrument_registry(editor.project(), Arc::clone(&table), &registry);
    let mut old = SessionPlayback::new(old_revision).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 16]).unwrap();
    editor
        .execute(Command::SetMidiNotesActive {
            clip,
            notes: vec![muted_note],
            active: false,
        })
        .unwrap();
    let mut candidate = SessionPlayback::new(revision_with_instrument_registry(
        editor.project(),
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    candidate.try_inherit_state_from(&mut old).unwrap();
    let proofs = candidate.mutable_instruments[0]
        .as_ref()
        .unwrap()
        .proofs()
        .collect::<Vec<_>>();
    assert_eq!(
        proofs
            .iter()
            .find(|proof| proof.signature.key().note == muted_note)
            .unwrap()
            .gate_end_frame,
        16
    );
    assert!(
        proofs
            .iter()
            .find(|proof| proof.signature.key().note == other_note)
            .unwrap()
            .gate_end_frame
            > 16
    );
    let mut output = [[0.0; 2]; 16];
    candidate.render(&mut output).unwrap();
    assert!(output[8..].iter().any(|frame| frame[0] != 0.0));
}

#[test]
fn activation_time_span_shortens_mutable_prefix_and_retriggers_right_window() {
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
    for publish in [16usize, 48] {
        let base = with_test_instrument(&project(0.5, 0, 1 << 28), id, 0);
        let track = base.tracks()[0].id();
        let mut old = SessionPlayback::new(revision_with_instrument_registry(
            &base,
            Arc::clone(&table),
            &registry,
        ))
        .unwrap();
        old.apply(SessionPlaybackCommand::Play).unwrap();
        old.render(&mut vec![[0.0; 2]; publish]).unwrap();
        let mut editor = Editor::new(base, 8).unwrap();
        editor
            .execute(Command::SetTimeRangeActive {
                range: hex_project::TimeRange::new(vec![track], 32, 64).unwrap(),
                active: false,
            })
            .unwrap();
        let mut candidate = SessionPlayback::new(revision_with_instrument_registry(
            editor.project(),
            Arc::clone(&table),
            &registry,
        ))
        .unwrap();
        candidate.try_inherit_state_from(&mut old).unwrap();
        let expected_gate = 32u64.max(publish as u64);
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
                .proofs()
                .any(|proof| proof.gate_end_frame == expected_gate)
        );
        // A repeated publication retains the admitted shortened gate, including
        // the prospective proof carried under the new canonical signature.
        let mut next = SessionPlayback::new(Arc::clone(&candidate.revision)).unwrap();
        next.try_inherit_state_from(&mut candidate).unwrap();
        let mut output = vec![[0.0; 2]; 80 - publish];
        next.render(&mut output).unwrap();
        assert!(output[64 - publish..].iter().any(|frame| frame[0] != 0.0));
        assert!(
            next.mutable_instruments[0]
                .as_ref()
                .unwrap()
                .proofs()
                .any(|proof| proof.onset_frame == 64)
        );
        assert!(next.tail_mutable_instruments.iter().all(Option::is_none));
        assert!(
            next.mutable_instruments[0]
                .as_ref()
                .unwrap()
                .proofs()
                .filter(|proof| proof.onset_frame < 64)
                .all(|proof| proof.note_off_sent)
        );
    }
}

#[test]
fn activation_of_other_clip_keeps_existing_mutable_gate_deadline() {
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
    let base = with_test_instrument(&project(0.5, 0, 1 << 28), id, 0);
    let track = base.tracks()[0].id();
    let first = base.tracks()[0].midi_clips()[0].id();
    let mut editor = Editor::new(base, 8).unwrap();
    let Created::Clip(other) = editor.execute(Command::DuplicateMidiClip(first)).unwrap() else {
        panic!("clip")
    };
    let mut old = SessionPlayback::new(revision_with_instrument_registry(
        editor.project(),
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 16]).unwrap();
    let deadline = old.mutable_instruments[0]
        .as_ref()
        .unwrap()
        .proofs()
        .next()
        .unwrap()
        .gate_end_frame;
    editor
        .execute(Command::SetClipActive {
            clip: other,
            active: false,
        })
        .unwrap();
    let mut candidate = SessionPlayback::new(revision_with_instrument_registry(
        editor.project(),
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    candidate.try_inherit_state_from(&mut old).unwrap();
    assert!(
        candidate
            .tail_mutable_instruments
            .iter()
            .all(Option::is_none)
    );
    let proof = candidate.mutable_instruments[0]
        .as_ref()
        .unwrap()
        .proofs()
        .find(|proof| proof.signature.key().clip == first)
        .unwrap();
    assert_eq!(proof.gate_end_frame, deadline);
    assert!(candidate.revision.arrangement().tracks()[0].instrument_slot_enabled());
    assert!(candidate.revision.arrangement().tracks()[0].canonical_track_id() == Some(track));
}

#[test]
fn mutable_source_route_switch_releases_old_gate_without_resetting_kernel() {
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
    let revision = revision_with_instrument_registry(
        &source,
        Arc::new(hex_devices::SineTable::new()),
        &registry,
    );
    let clip = revision.session().midi_clips()[0].id;
    let track = revision.session().midi_clips()[0].track;
    let mut playback = SessionPlayback::new(revision).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut onset = [[0.0; 2]; 1];
    playback.render(&mut onset).unwrap();
    let cut = playback.performance_frame();
    assert_eq!(
        playback.apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        }),
        Ok(())
    );
    assert_eq!(playback.performance_frame(), cut);
    assert_eq!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Session { clip })
    );
    let mut oracle = MutableFeedbackRuntime {
        key: 0.2_f32.to_bits(),
        level: 0.2,
        release_frames: 8,
        voices: [MutableFeedbackVoice::default(); 64],
        feedback: 0.0,
        tail_left: 0,
        storage: Box::default(),
    };
    use hex_lattice::MutableInstrumentRuntime;
    use hex_lattice::NativeInstrumentEvent::{NoteOff, NoteOn};
    let mut oracle_prefix = [[0.0; 2]; 1];
    oracle.process_block(
        &[NoteOn {
            frame_offset: 0,
            occurrence_id: 1,
            pitch: 60,
            velocity: 100,
        }],
        &mut oracle_prefix,
        &hex_lattice::EMPTY_DEVICE_RESOURCES,
    );
    assert_eq!(onset, oracle_prefix);

    let mut release = [[0.0; 2]; 32];
    playback.render(&mut release).unwrap();
    let mut expected = [[0.0; 2]; 32];
    oracle.process_block(
        &[
            NoteOff {
                frame_offset: 0,
                occurrence_id: 1,
            },
            NoteOn {
                frame_offset: 0,
                occurrence_id: 2,
                pitch: 60,
                velocity: 100,
            },
            NoteOff {
                frame_offset: 6,
                occurrence_id: 2,
            },
        ],
        &mut expected,
        &hex_lattice::EMPTY_DEVICE_RESOURCES,
    );
    assert_eq!(
        release, expected,
        "one uninterrupted DSP handles both route generations"
    );
    assert!(
        playback.mutable_instruments[0]
            .as_ref()
            .is_some_and(MutableInstrumentTrackRuntime::committed_is_quiescent)
    );
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .expect("quiet mutable owner may switch routes without losing note events");
    assert!(matches!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Session { clip: active }) if active == clip
    ));
    let mut new_source = [[0.0; 2]; 1];
    playback.render(&mut new_source).unwrap();
    assert_eq!(new_source[0], [0.25, 0.25]);
}

#[test]
fn mutable_source_back_to_arrangement_does_not_chase_pre_cut_notes() {
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
    let mut snapshot = project(1.0, 0, 8 << 32).snapshot();
    let arrangement_clips = match &mut snapshot.tracks[0].content {
        TrackContentSnapshot::Instrument(clips) => clips,
        TrackContentSnapshot::Hybrid { midi_clips, .. } => midi_clips,
        TrackContentSnapshot::Audio(_) => panic!("Arrangement MIDI fixture expected"),
    };
    arrangement_clips[0].notes[0].duration_raw = 8 << 32;
    let SessionCellContentSnapshot::Midi(session_clip) = &mut snapshot.session.cells[0].content
    else {
        panic!("Session MIDI fixture expected");
    };
    session_clip.source.notes[0].duration_raw = 1 << 32;
    let source = legacy_project_from_snapshot(snapshot).unwrap();
    let source = with_test_instrument(&source, id, 0);
    let revision = revision_with_instrument_registry(
        &source,
        Arc::new(hex_devices::SineTable::new()),
        &registry,
    );
    let clip = revision.session().midi_clips()[0].id;
    let track = revision.session().midi_clips()[0].track;
    let mut playback = SessionPlayback::new(revision).unwrap();
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut session_audio = vec![[0.0; 2]; 24_064];
    playback.render(&mut session_audio).unwrap();
    assert!(
        playback.mutable_instruments[0]
            .as_ref()
            .is_some_and(MutableInstrumentTrackRuntime::committed_is_quiescent)
    );
    assert_eq!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Session { clip })
    );

    let cut = playback.performance_frame();
    assert_eq!(
        playback.apply(SessionPlaybackCommand::BackToArrangement {
            quantization: LaunchQuantization::Immediate,
        }),
        Ok(())
    );
    assert_eq!(playback.performance_frame(), cut);
    assert_eq!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Arrangement)
    );
    let mut no_chase = [[1.0; 2]; 64];
    playback.render(&mut no_chase).unwrap();
    assert_eq!(no_chase, [[0.0; 2]; 64]);
    assert!(
        playback.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .committed_is_quiescent()
    );

    playback
        .apply(SessionPlaybackCommand::Seek(16 * 24_000 + 1))
        .unwrap();
    playback
        .apply(SessionPlaybackCommand::BackToArrangement {
            quantization: LaunchQuantization::Immediate,
        })
        .expect("retry succeeds once the Arrangement destination is clear");
    assert_eq!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Arrangement)
    );
}

#[test]
fn mutable_prepared_playback_admits_bounded_arrangement_transport_loops() {
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

    let mut arrangement_loop = project(0.5, 0, 1 << 24).snapshot();
    arrangement_loop.tracks[0]
        .instrument
        .as_mut()
        .unwrap()
        .state = hex_lattice_contract::NativeDeviceState::new(id, 1, vec![0]).unwrap();
    arrangement_loop.loop_region = Some(LoopRegion::new(0, 48_000, true).unwrap());
    let arrangement_loop = legacy_project_from_snapshot(arrangement_loop).unwrap();
    let prepared =
        revision_with_instrument_registry(&arrangement_loop, Arc::clone(&table), &registry);
    assert!(SessionPlayback::new(prepared).is_ok());

    let mut session_loop = session_only_project(&project(0.5, 0, 1 << 24)).snapshot();
    session_loop.tracks[0].instrument.as_mut().unwrap().state =
        hex_lattice_contract::NativeDeviceState::new(id, 1, vec![0]).unwrap();
    let SessionCellContentSnapshot::Midi(clip) = &mut session_loop.session.cells[0].content else {
        panic!("fixture has a Session MIDI source")
    };
    clip.source.looping = true;
    let session_loop = legacy_project_from_snapshot(session_loop).unwrap();
    let prepared = revision_with_instrument_registry(&session_loop, table, &registry);
    assert!(SessionPlayback::new(prepared).is_ok());
}
