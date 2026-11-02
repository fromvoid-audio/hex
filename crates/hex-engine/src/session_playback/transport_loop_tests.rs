use super::source_loop_tests::{arrangement_loop_project, prepare_mutable};
use super::*;

const Q: u64 = 1_u64 << 32;

#[test]
fn audio_loop_change_with_limiter_preserves_samples_and_clock() {
    use hex_lattice_contract::NativeDeviceMetadata;
    let mut snapshot = audio_project(0, 32, 4).snapshot();
    for asset in &mut snapshot.assets {
        asset.source_frames = 8192;
        asset.playback_frames = 8192;
    }
    snapshot.tracks[0].content = TrackContentSnapshot::Audio(vec![ClipSnapshot {
        active: true,
        name: "clip".into(),
        flavor: hex_project::ClipFlavor::default(),
        edit: None,
        gain: hex_project::ClipGain::UNITY,
        id: 6,
        asset: 4,
        start: 0,
        duration: 8192,
        source_offset: 0,
        source_length: 8192,
        looping: false,
        source_phase: 0,
        fade: ClipFade::neutral(8192).unwrap(),
    }]);
    snapshot.tracks[0].devices.push(DeviceSnapshot {
        automation: Vec::new(),
        comparison: None,
        id: 7,
        bypassed: false,
        state: hex_devices::limiter::DEFINITION.default_state().unwrap(),
    });
    snapshot.next_id = 8;
    let linear = legacy_project_from_snapshot(snapshot.clone()).unwrap();
    snapshot.loop_region = Some(LoopRegion::new(400, 2400, true).unwrap());
    let looped = legacy_project_from_snapshot(snapshot).unwrap();
    let source = crate::Audio::new(
        48_000,
        (0..8192)
            .map(|i| {
                let value = ((i as f32) * 0.017).sin() * 0.2;
                [value, -value * 0.7]
            })
            .collect(),
    )
    .unwrap();
    let media = audio_media(&linear, &source, &source);
    let prepare = |project: &Project| {
        Arc::new(
            PreparedRevision::prepare(
                project,
                &media,
                Arc::new(hex_devices::SineTable::new()),
                None,
            )
            .unwrap(),
        )
    };
    let loop_revision = prepare(&looped);
    // A loop edit while playing inside the new region continues exactly as if
    // that loop had always been set: same wrap, samples and clocks.
    let mut live = SessionPlayback::new(prepare(&linear)).unwrap();
    let mut live_oracle = SessionPlayback::new(Arc::clone(&loop_revision)).unwrap();
    for runtime in [&mut live, &mut live_oracle] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        runtime.render(&mut [[0.0; 2]; 600]).unwrap();
    }
    let mut live_candidate = SessionPlayback::new(Arc::clone(&loop_revision)).unwrap();
    live_candidate.try_inherit_state_from(&mut live).unwrap();
    let mut live_actual = vec![[0.0; 2]; 6000];
    let mut live_expected = live_actual.clone();
    for chunk in live_actual.chunks_mut(127) {
        live_candidate.render(chunk).unwrap();
    }
    for chunk in live_expected.chunks_mut(127) {
        live_oracle.render(chunk).unwrap();
    }
    assert_eq!(
        live_candidate.arrangement_frame(),
        live_oracle.arrangement_frame()
    );
    assert_eq!(
        live_candidate.performance_frame(),
        live_oracle.performance_frame()
    );
    assert_eq!(live_actual, live_expected);
    let mut tempo_snapshot = linear.snapshot();
    tempo_snapshot.tempo_points = vec![TempoPoint::new(
        BeatTime::ZERO,
        Tempo::new(90_000_000).unwrap(),
    )];
    let tempo_changed = legacy_project_from_snapshot(tempo_snapshot).unwrap();
    let mut paused = SessionPlayback::new(prepare(&linear)).unwrap();
    paused.apply(SessionPlaybackCommand::Play).unwrap();
    paused.render(&mut [[0.0; 2]; 600]).unwrap();
    paused.apply(SessionPlaybackCommand::Pause).unwrap();
    let mut tempo_candidate = SessionPlayback::new(prepare(&tempo_changed)).unwrap();
    // Strict planner refusal; the live path restarts instead (Pulse #3774).
    assert_eq!(
        tempo_candidate.try_inherit_state_from(&mut paused),
        Err(SessionPlaybackError::Arrangement(
            super::handoff::COMPENSATED_TIMING_CHANGED
        ))
    );
    // Live: paused or playing, the cursor keeps its beat under the new map
    // (Pulse #4118); playing, audio restarts as Play-from at that beat.
    assert_eq!(paused.arrangement_frame(), 600);
    let mut tempo_live = SessionPlayback::new(prepare(&tempo_changed)).unwrap();
    tempo_live.try_inherit_state_live_from(&mut paused).unwrap();
    assert!(!tempo_live.playing());
    assert_eq!(tempo_live.arrangement_frame(), 800);
    let mut running = SessionPlayback::new(prepare(&linear)).unwrap();
    running.apply(SessionPlaybackCommand::Play).unwrap();
    running.render(&mut [[0.0; 2]; 600]).unwrap();
    let mut relocated = SessionPlayback::new(prepare(&tempo_changed)).unwrap();
    relocated.try_inherit_state_live_from(&mut running).unwrap();
    assert!(relocated.playing());
    // 120 -> 90 bpm: frame 600 (1/40 beat) lands at frame 800 under 90 bpm.
    assert_eq!(relocated.arrangement_frame(), 800);
    let mut relocated_oracle = SessionPlayback::new(prepare(&tempo_changed)).unwrap();
    relocated_oracle.play_from(800).unwrap();
    let mut relocated_actual = vec![[0.0; 2]; 2048];
    let mut relocated_expected = relocated_actual.clone();
    relocated.render(&mut relocated_actual).unwrap();
    relocated_oracle.render(&mut relocated_expected).unwrap();
    assert_eq!(relocated_actual, relocated_expected);
    let mut donor = SessionPlayback::new(prepare(&linear)).unwrap();
    let mut oracle = SessionPlayback::new(Arc::clone(&loop_revision)).unwrap();
    for runtime in [&mut donor, &mut oracle] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        runtime.render(&mut [[0.0; 2]; 600]).unwrap();
        runtime.apply(SessionPlaybackCommand::Pause).unwrap();
        runtime.render(&mut [[0.0; 2]; 64]).unwrap();
    }
    let mut candidate = SessionPlayback::new(loop_revision).unwrap();
    candidate.try_inherit_state_from(&mut donor).unwrap();
    for runtime in [&mut candidate, &mut oracle] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
    }
    let mut actual = vec![[0.0; 2]; 6000];
    let mut expected = actual.clone();
    for chunk in actual.chunks_mut(127) {
        candidate.render(chunk).unwrap();
    }
    for chunk in expected.chunks_mut(127) {
        oracle.render(chunk).unwrap();
    }
    assert_eq!(candidate.arrangement_frame(), oracle.arrangement_frame());
    assert_eq!(candidate.performance_frame(), oracle.performance_frame());
    assert_eq!(actual, expected);
    assert!(actual.iter().flatten().any(|value| value.abs() > 0.001));
}

#[test]
fn paused_loop_change_with_limiter_refuses_active_instrument_tail_without_corrupting_donor() {
    use hex_lattice_contract::NativeDeviceMetadata;
    let mut snapshot = project(1.0, 0, Q).snapshot();
    let id = snapshot.next_id;
    snapshot.next_id += 1;
    snapshot.tracks[0].devices.push(DeviceSnapshot {
        automation: Vec::new(),
        comparison: None,
        id,
        bypassed: false,
        state: hex_devices::limiter::DEFINITION.default_state().unwrap(),
    });
    let linear = legacy_project_from_snapshot(snapshot.clone()).unwrap();
    snapshot.loop_region = Some(LoopRegion::new(400, 2400, true).unwrap());
    let looped = legacy_project_from_snapshot(snapshot).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let loop_revision = revision(&looped, Arc::clone(&table));
    let mut donor = SessionPlayback::new(revision(&linear, Arc::clone(&table))).unwrap();
    let mut oracle = SessionPlayback::new(revision(&linear, Arc::clone(&table))).unwrap();
    for runtime in [&mut donor, &mut oracle] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        runtime.render(&mut [[0.0; 2]; 600]).unwrap();
        runtime.apply(SessionPlaybackCommand::Pause).unwrap();
        runtime.render(&mut [[0.0; 2]; 64]).unwrap();
    }
    let mut candidate = SessionPlayback::new(Arc::clone(&loop_revision)).unwrap();
    // Strict planner refusal; the live path restarts instead (Pulse #3774).
    assert_eq!(
        candidate.try_inherit_state_from(&mut donor),
        Err(SessionPlaybackError::Arrangement(
            super::handoff::COMPENSATED_TAILS_CHANGED
        ))
    );
    // The live path applies the loop edit at the paused cursor on a separate
    // donor, leaving this test's donor untouched for the checks below.
    let mut live_donor = SessionPlayback::new(revision(&linear, Arc::clone(&table))).unwrap();
    live_donor.apply(SessionPlaybackCommand::Play).unwrap();
    live_donor.render(&mut [[0.0; 2]; 600]).unwrap();
    live_donor.apply(SessionPlaybackCommand::Pause).unwrap();
    live_donor.render(&mut [[0.0; 2]; 64]).unwrap();
    let cursor = live_donor.arrangement_frame();
    let mut live = SessionPlayback::new(loop_revision).unwrap();
    live.try_inherit_state_live_from(&mut live_donor).unwrap();
    assert!(!live.playing());
    assert_eq!(live.arrangement_frame(), cursor);
    for runtime in [&mut donor, &mut oracle] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
    }
    let mut actual = vec![[0.0; 2]; 6000];
    let mut expected = actual.clone();
    for chunk in actual.chunks_mut(127) {
        donor.render(chunk).unwrap();
    }
    for chunk in expected.chunks_mut(127) {
        oracle.render(chunk).unwrap();
    }
    assert_eq!(donor.arrangement_frame(), oracle.arrangement_frame());
    assert_eq!(donor.performance_frame(), oracle.performance_frame());
    assert_eq!(actual, expected);
    assert!(actual.iter().flatten().any(|value| value.abs() > 0.001));
}

#[test]
fn loop_edited_behind_a_wrapped_cursor_continues_linearly() {
    let source = crate::Audio::new(48_000, vec![[0.25, 0.25]; 32]).unwrap();
    let media = audio_media(&audio_project(0, 32, 4), &source, &source);
    let looped = |start, end| {
        let mut snapshot = audio_project(0, 32, 4).snapshot();
        snapshot.loop_region = Some(LoopRegion::new(start, end, true).unwrap());
        Arc::new(
            PreparedRevision::prepare(
                &legacy_project_from_snapshot(snapshot).unwrap(),
                &media,
                Arc::new(hex_devices::SineTable::new()),
                None,
            )
            .unwrap(),
        )
    };
    let mut active = SessionPlayback::new(looped(0, 2048)).unwrap();
    active.apply(SessionPlaybackCommand::Play).unwrap();
    active.render(&mut [[0.0; 2]; 3072]).unwrap();
    assert_eq!(
        (active.arrangement_frame(), active.arrangement_loop_pass),
        (1024, 1)
    );

    let mut behind = SessionPlayback::new(looped(0, 512)).unwrap();
    behind.try_inherit_state_from(&mut active).unwrap();
    behind.render(&mut [[0.0; 2]; 1000]).unwrap();
    assert_eq!(
        behind.arrangement_frame(),
        2024,
        "no jump back into the new loop"
    );
    assert_eq!(behind.performance_frame(), 4072);

    let mut ahead = SessionPlayback::new(looped(4096, 4608)).unwrap();
    ahead.try_inherit_state_from(&mut behind).unwrap();
    ahead.render(&mut [[0.0; 2]; 3000]).unwrap();
    assert_eq!(ahead.arrangement_frame(), 4096 + (5024 - 4608));
    assert_eq!(ahead.arrangement_loop_pass, 2);
}

#[test]
fn post_loop_session_launch_admits_the_faster_canonical_tempo() {
    let mut snapshot = session_only_project(&project(1.0, 0, Q)).snapshot();
    snapshot.loop_region = Some(LoopRegion::new(0, 24_000, true).unwrap());
    snapshot.tempo_points.push(TempoPoint::new(
        BeatTime::from_quarters(2).unwrap(),
        Tempo::new(240_000_000).unwrap(),
    ));
    let looped = legacy_project_from_snapshot(snapshot.clone()).unwrap();
    snapshot.loop_region = None;
    let linear = legacy_project_from_snapshot(snapshot).unwrap();
    let prepare = |p: &Project| {
        Arc::new(
            PreparedRevision::prepare(p, &BTreeMap::new(), Arc::new(crate::SineTable::new()), None)
                .unwrap(),
        )
    };
    let mut output = [[0.0; 2]; 513];
    let mut expected = [[0.0; 2]; 513];
    for (source, buffer) in [(&looped, &mut output), (&linear, &mut expected)] {
        let revision = prepare(source);
        let clip = revision.session().midi_clips()[0].id;
        let mut runtime = SessionPlayback::new(revision).unwrap();
        runtime.apply(SessionPlaybackCommand::Seek(72_000)).unwrap();
        runtime
            .apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        runtime.render(buffer).unwrap();
    }
    assert_eq!(output, expected);
    assert!(output.iter().any(|frame| frame[0] != 0.0));
}

#[test]
fn interactive_cursor_after_loop_keeps_absolute_history_and_forward_pcm() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let linear = transport_loop_project(true);
            let mut snapshot = linear.snapshot();
            let region = LoopRegion::new(48_000, 96_000, true).unwrap();
            snapshot.loop_region = Some(region);
            let looped = legacy_project_from_snapshot(snapshot).unwrap();
            let revision = prepare_mutable(&looped);
            for target in [96_000, 100_003, 144_123] {
                for start_playing in [false, true] {
                    let mut actual =
                        SessionPlayback::prepare_interactive_arrangement_preroll_from_revision(
                            Arc::clone(&revision),
                            target,
                            target,
                            start_playing,
                            &std::sync::atomic::AtomicBool::new(false),
                        )
                        .unwrap();
                    let mut oracle =
                        SessionPlayback::prepare_interactive_arrangement_preroll_from_revision(
                            prepare_mutable(&linear),
                            target,
                            target,
                            start_playing,
                            &std::sync::atomic::AtomicBool::new(false),
                        )
                        .unwrap();
                    assert_eq!(actual.arrangement_frame(), target);
                    assert_eq!(actual.performance_frame(), target);
                    assert_eq!(
                        actual.prepared_revision().arrangement().loop_region(),
                        Some(region)
                    );
                    if !start_playing {
                        actual.apply(SessionPlaybackCommand::Play).unwrap();
                        oracle.apply(SessionPlaybackCommand::Play).unwrap();
                    }
                    let mut output = [[0.0; 2]; 513];
                    let mut expected = [[0.0; 2]; 513];
                    for chunk in output.chunks_mut(17) {
                        actual.render(chunk).unwrap();
                    }
                    oracle.render(&mut expected).unwrap();
                    assert_eq!(output, expected, "target {target}");
                    assert!(output.iter().any(|frame| frame[0] != 0.0));
                    let mut replacement = SessionPlayback::new(Arc::clone(&revision)).unwrap();
                    replacement.try_inherit_state_from(&mut actual).unwrap();
                    let mut continued = [[0.0; 2]; 257];
                    let mut expected = [[0.0; 2]; 257];
                    replacement.render(&mut continued).unwrap();
                    oracle.render(&mut expected).unwrap();
                    assert_eq!(continued, expected, "post-loop handoff at {target}");
                    actual = replacement;
                    actual.apply(SessionPlaybackCommand::Stop).unwrap();
                    assert_eq!(actual.arrangement_frame(), 0);
                    assert_eq!(actual.loop_region(), Some(region));
                }
            }
        })
        .unwrap()
        .join()
        .unwrap();
}

fn transport_loop_project(expanded: bool) -> Project {
    let mut snapshot = project(1.0, 0, 5 * Q).snapshot();
    let note_id = snapshot.next_id;
    let track = snapshot.tracks.first_mut().expect("fixture track");
    track.instrument.as_mut().expect("fixture instrument").state =
        hex_lattice_contract::NativeDeviceState::new(
            hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback"),
            1,
            vec![0],
        )
        .expect("valid instrument state");
    let (TrackContentSnapshot::Hybrid { midi_clips, .. }
    | TrackContentSnapshot::Instrument(midi_clips)) = &mut track.content
    else {
        panic!("fixture Arrangement MIDI clip")
    };
    let clip = midi_clips.first_mut().expect("fixture clip");
    clip.length_raw = 10 * Q;
    clip.source_length_raw = 10 * Q;
    clip.source_start_raw = 0;
    clip.loop_start_raw = 0;
    clip.loop_end_raw = 10 * Q;
    clip.looping = false;
    if expanded {
        clip.notes = [(Q, 2 * Q), (4 * Q, Q), (6 * Q, Q), (8 * Q, Q)]
            .into_iter()
            .enumerate()
            .map(|(index, (start_raw, duration_raw))| MidiNoteSnapshot {
                active: true,
                id: note_id + index as u64,
                start_raw,
                duration_raw,
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .collect();
        snapshot.next_id += 4;
    } else {
        clip.notes = vec![MidiNoteSnapshot {
            active: true,
            id: note_id,
            start_raw: Q,
            duration_raw: 2 * Q,
            pitch: 60,
            channel: 0,
            velocity: 100,
        }];
        snapshot.next_id += 1;
        snapshot.loop_region =
            Some(LoopRegion::new(2 * 24_000, 4 * 24_000, true).expect("valid transport loop"));
    }
    legacy_project_from_snapshot(snapshot).expect("valid transport-loop fixture")
}

#[test]
fn shared_source_transport_loop_admits_one_aggregate_voice_and_release_envelope() {
    let mut snapshot = transport_loop_project(false).snapshot();
    let mut contributor = snapshot.tracks[0].clone();
    contributor.id = snapshot.next_id;
    snapshot.next_id += 1;
    let note_count = 11_u64;
    let first_note_id = snapshot.next_id;
    match &mut snapshot.tracks[0].content {
        TrackContentSnapshot::Hybrid { midi_clips, .. }
        | TrackContentSnapshot::Instrument(midi_clips) => {
            let clip = midi_clips.first_mut().expect("receiver clip");
            clip.notes = (0..note_count)
                .map(|index| MidiNoteSnapshot {
                    active: true,
                    id: first_note_id + index,
                    start_raw: 2 * Q,
                    duration_raw: Q,
                    pitch: 36 + (index % 48) as u8,
                    channel: 0,
                    velocity: 100,
                })
                .collect();
        }
        TrackContentSnapshot::Audio(_) => panic!("MIDI source track"),
    };
    let second_clip_id = first_note_id + note_count;
    let second_note_id = second_clip_id + 1;
    match &mut contributor.content {
        TrackContentSnapshot::Hybrid { midi_clips, .. }
        | TrackContentSnapshot::Instrument(midi_clips) => {
            let clip = midi_clips.first_mut().expect("contributor clip");
            clip.id = second_clip_id;
            clip.notes = (0..note_count)
                .map(|index| MidiNoteSnapshot {
                    active: true,
                    id: second_note_id + index,
                    start_raw: 2 * Q,
                    duration_raw: Q,
                    pitch: 36 + (index % 48) as u8,
                    channel: 0,
                    velocity: 100,
                })
                .collect();
        }
        TrackContentSnapshot::Audio(_) => panic!("MIDI source track"),
    }
    snapshot.next_id = second_note_id + note_count;
    snapshot.loop_region = Some(LoopRegion::new(48_000, 48_128, true).expect("short loop"));
    snapshot.tracks.push(contributor);
    let project = Project::from_snapshot(snapshot).expect("shared source loop project");
    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback"),
            levels: [0.2, 0.4],
            release_frames: 256,
        },
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry =
        hex_lattice::NativeInstrumentRegistry::new(&definitions).expect("instrument registry");
    let revision = Arc::new(
        PreparedRevision::prepare_with_instrument_registry(
            &project,
            &BTreeMap::new(),
            Arc::new(crate::SineTable::new()),
            None,
            &registry,
        )
        .expect("statically admitted shared source"),
    );

    let region = revision.arrangement().loop_region().expect("fixture loop");
    let receiver = revision.arrangement().tracks()[0]
        .canonical_track_id()
        .expect("receiver id");
    let factory = revision
        .session()
        .mutable_instrument_factory(receiver)
        .expect("receiver runtime factory");
    for track in revision.arrangement().tracks() {
        assert_eq!(
            super::transport_loop_admission::admit_track(
                track,
                region,
                factory.max_release_tail_frames(),
                factory.max_events_per_block(),
                factory.max_active_occurrences(),
                false,
            ),
            Ok(44),
            "each contributor stays below the 64 voice limit"
        );
    }

    // Each authored track peaks at 11 voices, so static source admission is
    // valid at 22. At the first seam, the initial 11 releases overlap three
    // repeated 11-note tails (44 per contributor); the combined 88 must be
    // charged to one runtime even though either track is individually below 64.
    assert_eq!(
        SessionPlayback::new(revision).err(),
        Some(SessionPlaybackError::MutableInstrumentLoopDensity)
    );
}

fn render_transport_project(project: &Project, frames: usize, partitions: &[usize]) -> Vec<Frame> {
    let revision = prepare_mutable(project);
    let mut playback = SessionPlayback::new(revision).expect("mutable loop admitted");
    playback
        .apply(SessionPlaybackCommand::Play)
        .expect("start transport loop");
    let mut output = vec![[0.0; 2]; frames];
    let mut offset = 0;
    let mut partition = 0;
    while offset < output.len() {
        let requested = partitions[partition % partitions.len()].max(1);
        let end = offset.saturating_add(requested).min(output.len());
        playback
            .render(&mut output[offset..end])
            .expect("render transport loop");
        offset = end;
        partition += 1;
    }
    output
}

fn beat_raw_for_frames(frames: u64) -> u64 {
    ((u128::from(frames) * u128::from(Q) + 12_000) / 24_000) as u64
}

fn combined_loop_expanded_oracle_project() -> Project {
    let mut snapshot = arrangement_loop_project().snapshot();
    let first_note_id = snapshot.next_id;
    let (TrackContentSnapshot::Hybrid { midi_clips, .. }
    | TrackContentSnapshot::Instrument(midi_clips)) = &mut snapshot.tracks[0].content
    else {
        panic!("fixture Arrangement MIDI clip")
    };
    let clip = &mut midi_clips[0];
    clip.length_raw = 9 * Q;
    clip.source_length_raw = 9 * Q;
    clip.source_start_raw = 0;
    clip.loop_start_raw = 0;
    clip.loop_end_raw = 9 * Q;
    clip.looping = false;
    // Explicitly authored linear oracle for the combined source and transport
    // sequence. Quarter-beat units include source gates clipped at transport
    // seams and fresh attacks chased at each seam.
    clip.notes = [
        (4_u64, 6_u64),
        (8, 10),
        (12, 13),
        (13, 14),
        (16, 18),
        (20, 21),
        (21, 22),
        (24, 26),
        (28, 29),
        (29, 30),
        (32, 34),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (onset, end))| MidiNoteSnapshot {
        active: true,
        id: first_note_id + index as u64,
        start_raw: onset * Q / 4,
        duration_raw: (end - onset) * Q / 4,
        pitch: 60,
        channel: 0,
        velocity: 100,
    })
    .collect();
    snapshot.next_id = first_note_id + 11;
    snapshot.loop_region = None;
    legacy_project_from_snapshot(snapshot).expect("valid expanded combined-loop oracle")
}

#[test]
fn transport_wrap_matches_authored_pcm_at_fixed_callback_sizes() {
    let mut source_loop = arrangement_loop_project().snapshot();
    source_loop.loop_region = Some(LoopRegion::new(30_000, 78_000, true).unwrap());
    let source_loop = legacy_project_from_snapshot(source_loop).unwrap();
    let expanded = combined_loop_expanded_oracle_project();
    // Include the first wrap and its freshly chased gate, with the reference
    // generated from independently authored linear occurrences.
    let expected = render_transport_project(&expanded, 80_000, &[256]);
    assert!(expected.iter().flatten().any(|sample| *sample != 0.0));
    for block in [1, 17, 64, 128, 256, 512] {
        let actual = render_transport_project(&source_loop, expected.len(), &[block]);
        assert_eq!(actual, expected, "block size {block}");
    }
}

#[test]
fn mutable_source_and_transport_loops_match_authored_linear_occurrences_across_wraps() {
    let mut source_loop = arrangement_loop_project().snapshot();
    source_loop.loop_region =
        Some(LoopRegion::new(30_000, 78_000, true).expect("valid transport loop"));
    let source_loop = legacy_project_from_snapshot(source_loop).expect("combined loop project");
    let expanded = combined_loop_expanded_oracle_project();
    let whole = render_transport_project(&source_loop, 210_000, &[210_000]);
    let expanded_whole = render_transport_project(&expanded, 210_000, &[210_000]);
    let partitioned = render_transport_project(&source_loop, 210_000, &[1, 17, 127, 256]);
    assert_eq!(whole, expanded_whole);
    assert_eq!(whole, partitioned);

    // Gate boundaries are authored in quarter-beat units (6,000 frames).
    // Transport wraps clip the beat-three gates at beat 3.25 and chase the
    // active beat-one source occurrence at each subsequent seam.
    let expected = [
        (4, 6, 0, 0),
        (8, 10, 1, 0),
        (12, 13, 2, 0),
        (13, 14, 0, 1),
        (16, 18, 1, 1),
        (20, 21, 2, 1),
        (21, 22, 0, 2),
        (24, 26, 1, 2),
        (28, 29, 2, 2),
        (29, 30, 0, 3),
        (32, 34, 1, 3),
    ];
    let mut playback = SessionPlayback::new(prepare_mutable(&source_loop))
        .expect("combined source and transport loop admission");
    playback
        .apply(SessionPlaybackCommand::Play)
        .expect("start combined loop proof run");
    let mut offset = 0_usize;
    let runtime_index = 0;
    let mut seen_ids = Vec::new();
    for (onset_units, gate_units, iteration, loop_pass) in expected {
        let onset_frame = usize::try_from(onset_units * 6_000).expect("bounded fixture frame");
        let mut before = vec![[0.0; 2]; onset_frame - offset];
        playback
            .render(&mut before)
            .expect("render to authored onset");
        offset = onset_frame;
        let next_id = playback.mutable_instruments[runtime_index]
            .as_ref()
            .expect("mutable proof runtime")
            .next_occurrence_id();
        let mut onset = [[0.0; 2]; 1];
        playback.render(&mut onset).expect("render authored onset");
        offset += 1;
        let runtime = playback.mutable_instruments[runtime_index]
            .as_ref()
            .expect("mutable proof runtime");
        assert_eq!(runtime.next_occurrence_id(), next_id + 1);
        let proof = runtime
            .occurrence_proofs()
            .iter()
            .find(|proof| proof.onset_frame == onset_frame as u64)
            .copied()
            .expect("one occurrence at each authored onset");
        assert_eq!(proof.id, seen_ids.len() as u64 + 1);
        assert_eq!(proof.signature.key().iteration, iteration);
        assert_eq!(proof.signature.key().loop_pass, loop_pass);
        assert_eq!(proof.gate_end_frame, gate_units * 6_000);
        seen_ids.push(proof.id);
    }
    assert_eq!(seen_ids, (1..=expected.len() as u64).collect::<Vec<_>>());
    assert_eq!(
        playback.mutable_instruments[runtime_index]
            .as_ref()
            .expect("mutable proof runtime")
            .next_occurrence_id(),
        expected.len() as u64
    );

    // Carry an active source occurrence across a compatible prepared revision
    // handoff during the third transport pass and compare its continuation.
    let mut active = SessionPlayback::new(prepare_mutable(&source_loop)).expect("active loop");
    let mut reference = SessionPlayback::new(prepare_mutable(&source_loop)).expect("loop oracle");
    for candidate in [&mut active, &mut reference] {
        candidate
            .apply(SessionPlaybackCommand::Play)
            .expect("start handoff fixture");
    }
    let mut prefix = vec![[0.0; 2]; 195_000];
    let mut reference_prefix = prefix.clone();
    active
        .render(&mut prefix)
        .expect("render active loop prefix");
    reference
        .render(&mut reference_prefix)
        .expect("render reference prefix");
    assert_eq!(prefix, reference_prefix);
    let active_proof = active.mutable_instruments[0]
        .as_ref()
        .expect("active mutable runtime")
        .occurrence_proofs()
        .iter()
        .find(|proof| !proof.note_off_sent)
        .copied()
        .expect("active source gate at handoff");
    let mut successor = SessionPlayback::new(prepare_mutable(&source_loop)).expect("successor");
    successor
        .try_inherit_state_from(&mut active)
        .expect("compatible combined-loop revision handoff");
    let inherited = successor.mutable_instruments[0]
        .as_ref()
        .expect("successor mutable runtime")
        .occurrence_proofs()
        .iter()
        .find(|proof| proof.id == active_proof.id)
        .copied()
        .expect("active occurrence identity survives handoff");
    assert_eq!(inherited, active_proof);
    let mut actual = vec![[0.0; 2]; 60_000];
    let mut expected_audio = actual.clone();
    successor.render(&mut actual).expect("render successor");
    reference
        .render(&mut expected_audio)
        .expect("render uninterrupted loop");
    assert_eq!(actual, expected_audio);
    assert_eq!(successor.arrangement_loop_pass, 4);
    assert_eq!(
        successor.mutable_instruments[0]
            .as_ref()
            .expect("successor mutable runtime")
            .next_occurrence_id(),
        reference.mutable_instruments[0]
            .as_ref()
            .expect("reference mutable runtime")
            .next_occurrence_id()
    );
}

#[test]
fn finite_bounce_preserves_mutable_source_phase_and_ignores_transport_wrapping() {
    for offset in [0, Q / 4, 5 * Q / 4] {
        let mut snapshot = arrangement_loop_project().snapshot();
        let (TrackContentSnapshot::Hybrid { midi_clips, .. }
        | TrackContentSnapshot::Instrument(midi_clips)) = &mut snapshot.tracks[0].content
        else {
            panic!("fixture midi");
        };
        midi_clips[0].playback_offset_raw = offset;
        snapshot.loop_region = Some(LoopRegion::new(0, 24_000, true).unwrap());
        let looped = legacy_project_from_snapshot(snapshot.clone()).unwrap();
        snapshot.loop_region = None;
        let reference = legacy_project_from_snapshot(snapshot).unwrap();
        let mut bounce =
            SessionPlayback::new_finite_arrangement_bounce(prepare_mutable(&looped), None, 120_000)
                .unwrap();
        let mut reference = SessionPlayback::new(prepare_mutable(&reference)).unwrap();
        assert!(bounce.is_linear_arrangement_export());
        bounce.apply(SessionPlaybackCommand::Play).unwrap();
        reference.apply(SessionPlaybackCommand::Play).unwrap();
        let mut rendered = 0;
        for count in [1, 137, 511].into_iter().cycle() {
            let count = count.min(120_000 - rendered);
            let mut actual = vec![[0.0; 2]; count];
            let mut expected = vec![[0.0; 2]; count];
            bounce.render(&mut actual).unwrap();
            reference.render(&mut expected).unwrap();
            assert_eq!(
                actual, expected,
                "finite source phase offset={offset} at {rendered}"
            );
            rendered += count;
            if rendered == 120_000 {
                break;
            }
        }
        assert_eq!(bounce.arrangement_loop_pass, 0);
        let playback = crate::playback::Playback::from_arrangement_preroll(bounce).unwrap();
        assert_eq!(playback.automation_capture_loop(), None);
        for frames in [0, MAX_FINITE_BOUNCE_FRAMES + 1, u64::MAX] {
            assert!(
                SessionPlayback::new_finite_arrangement_bounce(
                    prepare_mutable(&looped),
                    None,
                    frames,
                )
                .is_err()
            );
        }
    }
}

#[test]
fn mutable_transport_loops_admit_while_linear_export_still_refuses_source_loops() {
    let mut snapshot = arrangement_loop_project().snapshot();
    snapshot.loop_region = Some(LoopRegion::new(0, 48_000, true).expect("valid transport loop"));
    let transport_loop =
        legacy_project_from_snapshot(snapshot).expect("valid transport loop project");
    assert!(SessionPlayback::new(prepare_mutable(&transport_loop)).is_ok());
    assert_eq!(
        SessionPlayback::new_linear_arrangement_export(prepare_mutable(&transport_loop)).err(),
        Some(SessionPlaybackError::MutableInstrumentLoopUnsupported)
    );
}

#[test]
fn mutable_transport_wrap_chases_as_fresh_attack_without_first_entry_duplicate() {
    let project = transport_loop_project(false);
    let revision = prepare_mutable(&project);
    let mut playback = SessionPlayback::new(revision).expect("admitted bounded transport loop");
    playback
        .apply(SessionPlaybackCommand::Play)
        .expect("start transport loop");
    let render = |playback: &mut SessionPlayback, count: usize| {
        let mut frames = vec![[0.0; 2]; count];
        playback.render(&mut frames).expect("render loop segment");
    };

    render(&mut playback, 48_000);
    assert_eq!(
        playback.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .next_occurrence_id(),
        1
    );
    render(&mut playback, 48_000);
    assert_eq!(
        playback.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .next_occurrence_id(),
        1
    );
    render(&mut playback, 1);
    let wrapped = playback.mutable_instruments[0]
        .as_ref()
        .expect("wrapped runtime")
        .proofs()
        .find(|proof| proof.signature.key().loop_pass == 1)
        .expect("fresh occurrence on the first actual wrap");
    // The wrapped attack starts at performance frame 96k. Its remaining
    // authored gate is 24k frames, so it ends at 120k (not 144k, which would
    // incorrectly shift the note's full original duration).
    assert_eq!(wrapped.gate_end_frame, 5 * 24_000);
    assert_eq!(wrapped.onset_frame, 4 * 24_000);

    let whole = render_transport_project(&project, 200_000, &[200_000]);
    let partitioned = render_transport_project(&project, 200_000, &[1, 17, 127, 256]);
    let expanded = render_transport_project(&transport_loop_project(true), 200_000, &[200_000]);
    assert_eq!(whole, partitioned);
    assert_eq!(partitioned, expanded);
}

#[test]
fn interactive_start_one_frame_before_wrap_matches_authored_attack_and_note_off() {
    let looped = transport_loop_project(false);
    let revision = prepare_mutable(&looped);
    let mut snapshot = transport_loop_project(true).snapshot();
    let (TrackContentSnapshot::Hybrid { midi_clips, .. }
    | TrackContentSnapshot::Instrument(midi_clips)) = &mut snapshot.tracks[0].content
    else {
        panic!("fixture Arrangement MIDI clip")
    };
    let note = &mut midi_clips[0].notes[0];
    note.start_raw = beat_raw_for_frames(1);
    note.duration_raw = Q;
    midi_clips[0].notes.truncate(1);
    let oracle = legacy_project_from_snapshot(snapshot).expect("valid linear oracle");
    let expected = render_transport_project(&oracle, 24_065, &[256]);
    assert_eq!(expected[0], [0.0; 2]);
    assert!(
        expected[1..24_001]
            .iter()
            .flatten()
            .any(|sample| *sample != 0.0)
    );
    assert!(
        expected[24_041..]
            .iter()
            .flatten()
            .all(|sample| *sample == 0.0)
    );

    for block in [1, 17, 64, 128, 256, 512] {
        let mut playback = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        playback.play_from(95_999).unwrap();
        playback.render(&mut []).unwrap();
        assert_eq!(playback.arrangement_frame(), 95_999);
        let mut actual = vec![[0.0; 2]; expected.len()];
        for chunk in actual.chunks_mut(block) {
            playback.render(chunk).unwrap();
        }
        assert_eq!(actual, expected, "block size {block}");
        assert_eq!(playback.arrangement_frame(), 72_064);
        assert_eq!(playback.performance_frame(), 120_064);
    }
}

#[test]
fn mutable_transport_density_allows_empty_short_loops_and_rejects_dense_short_loops() {
    let mut empty = transport_loop_project(false).snapshot();
    empty.loop_region = Some(LoopRegion::new(0, 1, true).expect("valid one-frame loop"));
    let (TrackContentSnapshot::Hybrid { midi_clips, .. }
    | TrackContentSnapshot::Instrument(midi_clips)) = &mut empty.tracks[0].content
    else {
        panic!("fixture Arrangement MIDI clip")
    };
    midi_clips[0].notes.clear();
    let empty = legacy_project_from_snapshot(empty).expect("valid empty short loop");
    assert!(SessionPlayback::new(prepare_mutable(&empty)).is_ok());

    let mut dense = transport_loop_project(false).snapshot();
    dense.loop_region = Some(LoopRegion::new(0, 1, true).expect("valid one-frame loop"));
    let (TrackContentSnapshot::Hybrid { midi_clips, .. }
    | TrackContentSnapshot::Instrument(midi_clips)) = &mut dense.tracks[0].content
    else {
        panic!("fixture Arrangement MIDI clip")
    };
    midi_clips[0].notes[0].start_raw = 0;
    midi_clips[0].notes[0].duration_raw = Q;
    let dense = legacy_project_from_snapshot(dense).expect("valid dense short loop");
    assert_eq!(
        SessionPlayback::new(prepare_mutable(&dense)).err(),
        Some(SessionPlaybackError::MutableInstrumentLoopDensity)
    );
}

#[test]
fn mutable_transport_admission_counts_intro_releases_at_the_first_wrap_seam() {
    let mut snapshot = project(1.0, 0, 5 * Q).snapshot();
    let track = &mut snapshot.tracks[0];
    track.instrument.as_mut().expect("fixture instrument").state =
        hex_lattice_contract::NativeDeviceState::new(
            hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback"),
            1,
            vec![0],
        )
        .expect("valid test instrument state");
    let (TrackContentSnapshot::Hybrid { midi_clips, .. }
    | TrackContentSnapshot::Instrument(midi_clips)) = &mut track.content
    else {
        panic!("fixture Arrangement MIDI clip")
    };
    let clip = &mut midi_clips[0];
    clip.notes = (0..63)
        .map(|index| MidiNoteSnapshot {
            active: true,
            id: 100 + index,
            start_raw: 0,
            duration_raw: 2 * Q,
            pitch: 36 + index as u8,
            channel: 0,
            velocity: 100,
        })
        .chain(std::iter::once(MidiNoteSnapshot {
            active: true,
            id: 200,
            start_raw: 2 * Q,
            duration_raw: Q,
            pitch: 60,
            channel: 0,
            velocity: 100,
        }))
        .collect();
    clip.length_raw = 5 * Q;
    clip.source_length_raw = 5 * Q;
    clip.source_start_raw = 0;
    clip.loop_start_raw = 0;
    clip.loop_end_raw = 5 * Q;
    clip.looping = false;
    snapshot.next_id = 201;
    snapshot.loop_region = Some(LoopRegion::new(48_004, 48_013, true).expect("short loop"));
    let project = legacy_project_from_snapshot(snapshot).expect("valid seam-density project");

    // The 63 intro gates end before loop_start and release across the first
    // wrap. The single active loop note keeps steady-state density below 64,
    // while its old release plus fresh wrapped attack pushes the first seam
    // above the track's 64-voice limit.
    assert_eq!(
        SessionPlayback::new(prepare_mutable(&project)).err(),
        Some(SessionPlaybackError::MutableInstrumentLoopDensity)
    );
}

#[test]
fn mutable_transport_admission_respects_lower_device_event_and_voice_limits() {
    let mut snapshot = transport_loop_project(false).snapshot();
    let duplicate_id = snapshot.next_id;
    snapshot.next_id += 1;
    let (TrackContentSnapshot::Hybrid { midi_clips, .. }
    | TrackContentSnapshot::Instrument(midi_clips)) = &mut snapshot.tracks[0].content
    else {
        panic!("fixture Arrangement MIDI clip")
    };
    let mut duplicate = midi_clips[0].notes[0].clone();
    duplicate.id = duplicate_id;
    // Same simultaneous event/voice pressure on a distinct valid pitch.
    duplicate.pitch += 1;
    midi_clips[0].notes.push(duplicate);
    let project = legacy_project_from_snapshot(snapshot).expect("valid duplicated loop notes");
    let revision = prepare_mutable(&project);
    let track = &revision.arrangement().tracks()[0];
    let region = revision
        .arrangement()
        .loop_region()
        .expect("fixture transport loop");

    assert_eq!(
        super::transport_loop_admission::admit_track(track, region, 16, 1, 64, false),
        Err(SessionPlaybackError::MutableInstrumentLoopDensity)
    );
    assert_eq!(
        super::transport_loop_admission::admit_track(track, region, 16, 256, 1, false),
        Err(SessionPlaybackError::MutableInstrumentLoopDensity)
    );
    assert_eq!(
        super::transport_loop_admission::admit_track(track, region, 16, 256, 1, true),
        Err(SessionPlaybackError::MutableInstrumentLoopDensity),
        "two held voices exceed one stealing voice too"
    );
}

#[test]
fn stealing_transport_loop_admission_bounds_held_voices_not_seam_releases() {
    // The single chased loop note releases for longer than its silent gap,
    // so each wrap's fresh chase overlaps the previous release.
    let revision = prepare_mutable(&transport_loop_project(false));
    let track = &revision.arrangement().tracks()[0];
    let region = revision
        .arrangement()
        .loop_region()
        .expect("fixture transport loop");
    let admit = |voices, steals| {
        super::transport_loop_admission::admit_track(track, region, 30_000, 256, voices, steals)
    };
    assert_eq!(admit(64, false), Ok(2), "releases cross the seam");
    assert_eq!(
        admit(1, false),
        Err(SessionPlaybackError::MutableInstrumentLoopDensity)
    );
    assert_eq!(
        admit(1, true),
        Ok(2),
        "only one voice is ever held; the envelope keeps the release"
    );
}

#[test]
fn finite_arrangement_admission_bounds_aggregate_edges_across_clips() {
    let mut snapshot = project(1.0, 0, 5 * Q).snapshot();
    let track = &mut snapshot.tracks[0];
    track.instrument.as_mut().expect("fixture instrument").state =
        hex_lattice_contract::NativeDeviceState::new(
            hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback"),
            1,
            vec![0],
        )
        .expect("valid test instrument state");
    let (TrackContentSnapshot::Hybrid { midi_clips, .. }
    | TrackContentSnapshot::Instrument(midi_clips)) = &mut track.content
    else {
        panic!("fixture Arrangement MIDI clip")
    };
    let template = midi_clips[0].clone();
    let long_length = beat_raw_for_frames(250);
    midi_clips[0].length_raw = long_length;
    midi_clips[0].source_length_raw = long_length;
    midi_clips[0].loop_end_raw = long_length;
    midi_clips[0].notes = (0..48)
        .map(|index| MidiNoteSnapshot {
            active: true,
            id: 100 + index,
            start_raw: 0,
            duration_raw: long_length,
            pitch: 36 + (index % 48) as u8,
            channel: 0,
            velocity: 100,
        })
        .collect();
    let short_length = beat_raw_for_frames(2);
    let note_length = beat_raw_for_frames(1);
    let short_region_start = long_length + beat_raw_for_frames(1);
    for index in 0..128_u64 {
        let start_raw = short_region_start + index * short_length;
        let mut clip = template.clone();
        clip.id = 1_000 + index;
        clip.start_raw = start_raw;
        clip.length_raw = short_length;
        clip.source_length_raw = short_length;
        clip.source_start_raw = 0;
        clip.loop_start_raw = 0;
        clip.loop_end_raw = short_length;
        clip.looping = false;
        clip.notes = vec![MidiNoteSnapshot {
            active: true,
            id: 2_000 + index,
            start_raw: 0,
            duration_raw: note_length,
            pitch: 60,
            channel: 0,
            velocity: 100,
        }];
        midi_clips.push(clip);
    }
    snapshot.next_id = 2_128;
    snapshot.loop_region = Some(LoopRegion::new(600, 800, true).expect("valid transport loop"));
    let project = legacy_project_from_snapshot(snapshot).expect("valid adjacent clip fixture");

    // One finite 256-frame query spans 48 long notes and 128 short clips. Its
    // aggregate NoteOn/NoteOff edges exceed the host capacity, although no
    // more than 48 voices overlap. Reject at candidate preparation, before a
    // transport-loop runtime can be constructed.
    assert_eq!(
        super::source_loop_tests::try_prepare_mutable(&project).unwrap_err(),
        crate::Invalid("mutable midi event density exceeds the instrument capacity per query")
    );
}

#[test]
fn transport_loop_edge_proof_catches_wrap_edges_after_finite_admission() {
    let mut snapshot = project(1.0, 0, 5 * Q).snapshot();
    let first_note_id = snapshot.next_id;
    let track = &mut snapshot.tracks[0];
    track.instrument.as_mut().expect("fixture instrument").state =
        hex_lattice_contract::NativeDeviceState::new(
            hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback"),
            1,
            vec![0],
        )
        .expect("valid test instrument state");
    let (TrackContentSnapshot::Hybrid { midi_clips, .. }
    | TrackContentSnapshot::Instrument(midi_clips)) = &mut track.content
    else {
        panic!("fixture Arrangement MIDI clip")
    };
    let clip = &mut midi_clips[0];
    clip.length_raw = beat_raw_for_frames(2_000);
    clip.source_length_raw = clip.length_raw;
    clip.loop_start_raw = 0;
    clip.loop_end_raw = clip.source_length_raw;
    clip.looping = false;
    clip.notes = (0..128_u64)
        .map(|index| MidiNoteSnapshot {
            active: true,
            id: first_note_id + index,
            start_raw: beat_raw_for_frames(17 + index),
            duration_raw: beat_raw_for_frames(1),
            pitch: 36 + (index % 48) as u8,
            channel: 0,
            velocity: 100,
        })
        .chain((0..64_u64).map(|index| MidiNoteSnapshot {
            active: true,
            id: first_note_id + 128 + index,
            start_raw: beat_raw_for_frames(450),
            duration_raw: beat_raw_for_frames(550),
            pitch: 36 + index as u8,
            channel: 0,
            velocity: 100,
        }))
        .collect();
    snapshot.next_id = first_note_id + 192;
    snapshot.loop_region = Some(LoopRegion::new(0, 800, true).expect("valid transport loop"));
    let project = legacy_project_from_snapshot(snapshot).expect("valid edge-density project");

    // The finite Arrangement has at most 256 edges in any 256-frame window,
    // at most 128 onsets, and at most 64 release-inclusive voices. On a
    // transport wrap, however, the 64 gate-offs clamped to loop end map to
    // phase zero alongside the 256 short-note edges in phases 17 through 145.
    let finite = prepare_mutable(&project);
    assert_eq!(
        SessionPlayback::new(finite).err(),
        Some(SessionPlaybackError::MutableInstrumentLoopDensity)
    );
}

#[test]
fn mutable_transport_handoff_ignores_pre_loop_release_only_candidate() {
    let mut old_snapshot = project(1.0, 0, 5 * Q).snapshot();
    let track = &mut old_snapshot.tracks[0];
    track.instrument.as_mut().expect("fixture instrument").state =
        hex_lattice_contract::NativeDeviceState::new(
            hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback"),
            1,
            vec![0],
        )
        .expect("valid instrument state");
    let (TrackContentSnapshot::Hybrid { midi_clips, .. }
    | TrackContentSnapshot::Instrument(midi_clips)) = &mut track.content
    else {
        panic!("fixture Arrangement MIDI clip")
    };
    midi_clips[0].notes = vec![MidiNoteSnapshot {
        active: true,
        id: 42,
        start_raw: beat_raw_for_frames(47_996),
        duration_raw: beat_raw_for_frames(1),
        pitch: 60,
        channel: 0,
        velocity: 100,
    }];
    old_snapshot.next_id = 43;
    old_snapshot.loop_region = Some(LoopRegion::new(48_000, 48_001, true).expect("short loop"));
    let old_project = legacy_project_from_snapshot(old_snapshot).expect("valid release fixture");
    let mut new_snapshot = old_project.snapshot();
    let (TrackContentSnapshot::Hybrid { midi_clips, .. }
    | TrackContentSnapshot::Instrument(midi_clips)) = &mut new_snapshot.tracks[0].content
    else {
        panic!("fixture Arrangement MIDI clip")
    };
    midi_clips[0].notes[0].velocity = 101;
    let new_project = legacy_project_from_snapshot(new_snapshot).expect("valid edited fixture");

    let mut previous = SessionPlayback::new(prepare_mutable(&old_project)).expect("old loop");
    previous
        .apply(SessionPlaybackCommand::Play)
        .expect("start old loop");
    let mut through_wrap = vec![[0.0; 2]; 48_001];
    previous
        .render(&mut through_wrap)
        .expect("render release through the first wrap");
    assert_eq!(previous.arrangement_loop_pass, 1);
    assert!(
        previous.mutable_instruments[0]
            .as_ref()
            .expect("old mutable runtime")
            .proofs()
            .any(|proof| proof.note_off_sent
                && proof.quiet_deadline_frame > previous.performance_frame)
    );

    let mut candidate = SessionPlayback::new(prepare_mutable(&new_project)).expect("new loop");
    candidate
        .try_inherit_state_from(&mut previous)
        .expect("release-only source span must not be treated as a new chase");
    assert_eq!(candidate.arrangement_loop_pass, 1);
}

#[test]
fn resident_session_batch_loop_overflow_preserves_pending_launch_for_retry() {
    let mut fixture = crate::signal_graph::tests::fixture(&[&[7], &[], &[], &[]], 32);
    fixture
        .editor
        .execute(Command::SetLoopRegion {
            start: 4,
            end: 24,
            enabled: true,
        })
        .unwrap();
    let Created::Scene(scene) = fixture
        .editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    for &track in &fixture.tracks {
        let asset = fixture.editor.project().track(track).unwrap().clips()[0].asset();
        fixture
            .editor
            .execute(Command::CreateSessionAudioClip {
                track,
                scene,
                name: "clip".into(),
                asset,
                source_start: 0,
                source_end: 32,
            })
            .unwrap();
    }
    let revision = crate::signal_graph::tests::prepare(&fixture).unwrap();
    let mut failed = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let mut reference = SessionPlayback::new(revision).unwrap();
    for runtime in [&mut failed, &mut reference] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        runtime
            .apply(SessionPlaybackCommand::LaunchScene {
                scene,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        runtime.arrangement_frame = 23;
        runtime.performance_frame = 23;
        runtime.arrangement_loop_pass = u64::MAX;
    }
    let mut rejected = [[7.0; 2]; 2];
    assert_eq!(
        failed.render(&mut rejected),
        Err(SessionPlaybackError::ArithmeticOverflow)
    );
    assert_eq!(rejected, [[0.0; 2]; 2]);
    assert_eq!(
        (
            failed.performance_frame,
            failed.arrangement_frame,
            failed.arrangement_loop_pass
        ),
        (23, 23, u64::MAX)
    );
    failed.arrangement_loop_pass = 0;
    reference.arrangement_loop_pass = 0;
    let mut actual = [[0.0; 2]; 16];
    let mut expected = [[0.0; 2]; 16];
    failed.render(&mut actual).unwrap();
    reference.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    assert!(actual.iter().any(|frame| *frame != [0.0; 2]));
    for &track in &fixture.tracks {
        assert_eq!(failed.owner(track), reference.owner(track));
    }
    assert_eq!(
        (
            failed.performance_frame,
            failed.arrangement_frame,
            failed.arrangement_loop_pass
        ),
        (
            reference.performance_frame,
            reference.arrangement_frame,
            reference.arrangement_loop_pass
        )
    );
}

#[test]
fn resident_batch_loop_overflow_rolls_back_before_retry() {
    let mut fixture = crate::signal_graph::tests::fixture(&[&[7]], 32);
    fixture
        .editor
        .execute(Command::SetLoopRegion {
            start: 4,
            end: 24,
            enabled: true,
        })
        .unwrap();
    let revision = crate::signal_graph::tests::prepare(&fixture).unwrap();
    let mut failed = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let mut reference = SessionPlayback::new(revision).unwrap();
    for runtime in [&mut failed, &mut reference] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        assert_eq!(runtime.resident_source_scratch.len(), 1);
        runtime.arrangement_frame = 23;
        runtime.performance_frame = 23;
        runtime.arrangement_loop_pass = u64::MAX;
    }
    let mut rejected = [[7.0; 2]; 2];
    assert_eq!(
        failed.render(&mut rejected),
        Err(SessionPlaybackError::ArithmeticOverflow)
    );
    assert_eq!(rejected, [[0.0; 2]; 2]);
    assert_eq!(
        (
            failed.performance_frame,
            failed.arrangement_frame,
            failed.arrangement_loop_pass
        ),
        (23, 23, u64::MAX)
    );
    failed.arrangement_loop_pass = 0;
    reference.arrangement_loop_pass = 0;
    let mut actual = [[0.0; 2]; 16];
    let mut expected = [[0.0; 2]; 16];
    failed.render(&mut actual).unwrap();
    reference.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(
        (
            failed.performance_frame,
            failed.arrangement_frame,
            failed.arrangement_loop_pass
        ),
        (
            reference.performance_frame,
            reference.arrangement_frame,
            reference.arrangement_loop_pass
        )
    );
}

#[test]
fn mutable_transport_pass_overflow_rolls_back_block_and_clock_for_retry() {
    let revision = prepare_mutable(&transport_loop_project(false));
    let mut failed = SessionPlayback::new(Arc::clone(&revision)).expect("admitted transport loop");
    let mut oracle = SessionPlayback::new(revision).expect("admitted transport loop oracle");
    for playback in [&mut failed, &mut oracle] {
        playback
            .apply(SessionPlaybackCommand::Play)
            .expect("start transport loop");
        playback.arrangement_frame = 4 * 24_000 - 1;
        playback.performance_frame = playback.arrangement_frame;
        playback.arrangement_loop_pass = u64::MAX;
    }

    let mut rejected = [[7.0; 2]; 2];
    assert_eq!(
        failed.render(&mut rejected),
        Err(SessionPlaybackError::ArithmeticOverflow)
    );
    assert_eq!(rejected, [[0.0; 2]; 2]);
    assert_eq!(failed.performance_frame(), 4 * 24_000 - 1);
    assert_eq!(failed.arrangement_frame(), 4 * 24_000 - 1);
    assert_eq!(failed.arrangement_loop_pass, u64::MAX);
    let runtime = failed.mutable_instruments[0]
        .as_mut()
        .expect("mutable instrument runtime");
    assert!(runtime.occurrence_proofs().is_empty());

    failed.arrangement_loop_pass = 0;
    oracle.arrangement_loop_pass = 0;
    let mut retried = [[0.0; 2]; 2];
    let mut reference = [[0.0; 2]; 2];
    failed.render(&mut retried).expect("retry wrapped block");
    oracle
        .render(&mut reference)
        .expect("render oracle wrapped block");
    assert_eq!(retried, reference);
    assert_eq!(failed.arrangement_loop_pass, oracle.arrangement_loop_pass);
}

#[test]
fn mutable_transport_wrap_pass_and_live_gate_survive_revision_handoff() {
    let project = transport_loop_project(false);
    let revision = prepare_mutable(&project);
    let mut active = SessionPlayback::new(Arc::clone(&revision)).expect("admitted loop");
    let mut reference = SessionPlayback::new(revision).expect("admitted loop reference");
    for playback in [&mut active, &mut reference] {
        playback
            .apply(SessionPlaybackCommand::Play)
            .expect("start transport");
    }
    let mut prefix = vec![[0.0; 2]; 4 * 24_000 + 1];
    let mut reference_prefix = prefix.clone();
    active
        .render(&mut prefix)
        .expect("render through first wrap");
    reference
        .render(&mut reference_prefix)
        .expect("render reference through first wrap");
    assert_eq!(prefix, reference_prefix);
    assert_eq!(active.arrangement_loop_pass, 1);
    assert!(
        active.mutable_instruments[0]
            .as_ref()
            .expect("mutable runtime")
            .proofs()
            .any(|proof| proof.signature.key().loop_pass == 1 && !proof.note_off_sent)
    );

    let mut changed_bounds = project.snapshot();
    changed_bounds.loop_region = Some(LoopRegion::new(48_000, 120_000, true).expect("new loop"));
    let changed_bounds = legacy_project_from_snapshot(changed_bounds).expect("valid new bounds");
    let mut refused =
        SessionPlayback::new(prepare_mutable(&changed_bounds)).expect("new loop density admitted");
    assert_eq!(
        refused.try_inherit_state_from(&mut active),
        Err(SessionPlaybackError::MutableInstrumentLoopBoundsActive)
    );
    assert_eq!(active.arrangement_loop_pass, 1);

    let mut tempo_active =
        SessionPlayback::new(prepare_mutable(&project)).expect("active tempo fixture");
    let mut tempo_reference =
        SessionPlayback::new(prepare_mutable(&project)).expect("tempo oracle");
    for playback in [&mut tempo_active, &mut tempo_reference] {
        playback
            .apply(SessionPlaybackCommand::Play)
            .expect("start tempo fixture");
    }
    let mut tempo_prefix = vec![[0.0; 2]; 4 * 24_000 + 1];
    let mut tempo_oracle_prefix = tempo_prefix.clone();
    tempo_active
        .render(&mut tempo_prefix)
        .expect("render tempo fixture through wrap");
    tempo_reference
        .render(&mut tempo_oracle_prefix)
        .expect("render tempo oracle through wrap");
    assert_eq!(tempo_prefix, tempo_oracle_prefix);

    let mut changed_tempo = project.snapshot();
    changed_tempo.tempo_points[0] = hex_project::TempoPoint::new(
        hex_project::BeatTime::ZERO,
        hex_project::Tempo::new(60_000_000).expect("valid changed tempo"),
    );
    let changed_tempo = legacy_project_from_snapshot(changed_tempo).expect("valid tempo edit");
    let mut tempo_candidate = SessionPlayback::new(prepare_mutable(&changed_tempo))
        .expect("tempo-edited loop density admitted");
    // Pulse #4117: the tempo edit no longer refuses. The chased loop gate is
    // released at the cut, keeping the same occurrence, and the remapped
    // pre-cut onset is suppressed rather than chased again.
    let chased = tempo_active.mutable_instruments[0]
        .as_ref()
        .expect("mutable runtime")
        .proofs()
        .find(|proof| proof.signature.key().loop_pass == 1 && !proof.note_off_sent)
        .expect("chased loop gate")
        .id;
    tempo_candidate
        .try_inherit_state_from(&mut tempo_active)
        .expect("tempo edit keeps the chased loop gate's beat");
    let proofs = |playback: &SessionPlayback| {
        playback.mutable_instruments[0]
            .as_ref()
            .expect("carried mutable runtime")
            .proofs()
            .collect::<Vec<_>>()
    };
    let cut = tempo_candidate.performance_frame();
    assert!(
        proofs(&tempo_candidate)
            .iter()
            .any(|proof| proof.id == chased && proof.gate_end_frame == cut)
    );
    tempo_candidate
        .render(&mut [[0.0; 2]; 128])
        .expect("render after the tempo edit");
    assert!(
        proofs(&tempo_candidate)
            .iter()
            .all(|proof| proof.id == chased && proof.note_off_sent)
    );
    let mut successor = SessionPlayback::new(prepare_mutable(&project)).expect("same loop");
    successor
        .try_inherit_state_from(&mut active)
        .expect("preserve loop pass on handoff");
    assert_eq!(successor.arrangement_loop_pass, 1);
    let mut actual = [[0.0; 2]; 128];
    let mut expected = [[0.0; 2]; 128];
    successor.render(&mut actual).expect("render successor");
    reference
        .render(&mut expected)
        .expect("render uninterrupted oracle");
    assert_eq!(actual, expected);
}

/// Pulse #3731: a loop edit while an Arrangement mutable-instrument note is
/// sounding was refused, so playback kept the old loop. The strict planner
/// still refuses (above); the live publication restarts audio at the cursor.
/// With the cursor inside, before or after the new loop and at different
/// block sizes, the result equals a fresh Play-from at that cursor: the held
/// note is chased once, with no stuck or duplicated voice.
#[test]
fn live_loop_edit_during_a_held_arrangement_note_restarts_at_the_cursor() {
    let project = transport_loop_project(false);
    // The held note spans 24_000..72_000; the edit lands at frame 30_000.
    // Cursor inside, before and after the new loop, each at its own block size.
    for (start, end, block) in [
        (24_000, 120_000, 7),
        (36_000, 144_000, 64),
        (0, 27_000, 509),
    ] {
        let mut changed = project.snapshot();
        changed.loop_region = Some(LoopRegion::new(start, end, true).expect("new loop"));
        let changed = legacy_project_from_snapshot(changed).expect("valid new bounds");
        let mut active = SessionPlayback::new(prepare_mutable(&project)).expect("old loop");
        active
            .apply(SessionPlaybackCommand::Play)
            .expect("start transport");
        active
            .render(&mut vec![[0.0; 2]; 30_000])
            .expect("render into the held note");
        assert!(
            active.mutable_instruments[0]
                .as_ref()
                .expect("mutable runtime")
                .proofs()
                .any(|proof| !proof.note_off_sent),
            "the note is held at the edit"
        );
        let revision = prepare_mutable(&changed);
        let mut candidate = SessionPlayback::new(Arc::clone(&revision)).expect("new loop");
        candidate
            .try_inherit_state_live_from(&mut active)
            .expect("live loop edit applies");
        let context = format!("loop {start}..{end}, block {block}");
        assert!(candidate.playing(), "{context}");
        assert_eq!(candidate.arrangement_frame(), 30_000, "{context}");
        assert_eq!(
            candidate.loop_region(),
            Some(LoopRegion::new(start, end, true).expect("new loop")),
            "{context}"
        );
        let mut oracle = SessionPlayback::new(revision).expect("oracle");
        oracle.play_from(30_000).expect("oracle play-from");
        // Through the held note's end (72_000) and past each new loop edge.
        let mut actual = vec![[0.0; 2]; 43_000];
        let mut expected = actual.clone();
        for (actual, expected) in actual.chunks_mut(block).zip(expected.chunks_mut(block)) {
            candidate.render(actual).expect("render after edit");
            oracle.render(expected).expect("render oracle");
        }
        assert!(
            actual == expected,
            "restart equals play-from at the cursor; {context}"
        );
        assert!(
            actual[..block]
                .iter()
                .flatten()
                .any(|sample| *sample != 0.0),
            "the held note is chased; {context}"
        );
    }
}
