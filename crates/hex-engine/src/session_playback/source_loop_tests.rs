use super::*;
use hex_project::{MidiSourceSnapshot, TrackContentSnapshot};

const Q: u64 = 1_u64 << 32;
const NOTE_BEFORE_START: u64 = 1_000_000;
const NOTE_INTRO_ONLY: u64 = 1_000_001;
const NOTE_REPEATED: u64 = 1_000_002;

pub(super) fn mutable_definition() -> MutableFeedbackDefinition {
    MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback"),
            levels: [0.2, 0.4],
            release_frames: 8,
        },
    }
}

fn session_loop_project() -> Project {
    let base = session_only_project(&project(1.0, 0, 4 * Q));
    let mut snapshot = base.snapshot();
    let Some(SessionCellContentSnapshot::Midi(clip)) = snapshot
        .session
        .cells
        .first_mut()
        .map(|cell| &mut cell.content)
    else {
        panic!("fixture Session MIDI clip")
    };
    clip.source = MidiSourceSnapshot {
        one_shot_boundary: hex_project::MidiOneShotBoundary::SourceEnd,
        source_length_raw: 4 * Q,
        source_start_raw: Q / 2,
        loop_start_raw: Q,
        loop_end_raw: 2 * Q,
        looping: true,
        notes: vec![
            MidiNoteSnapshot {
                active: true,
                id: NOTE_BEFORE_START,
                start_raw: Q / 4,
                duration_raw: 2 * Q,
                pitch: 72,
                channel: 0,
                velocity: 100,
            },
            MidiNoteSnapshot {
                active: true,
                id: NOTE_INTRO_ONLY,
                start_raw: 3 * Q / 4,
                duration_raw: 2 * Q,
                pitch: 60,
                channel: 0,
                velocity: 100,
            },
            MidiNoteSnapshot {
                active: true,
                id: NOTE_REPEATED,
                start_raw: 3 * Q / 2,
                duration_raw: 2 * Q,
                pitch: 64,
                channel: 0,
                velocity: 100,
            },
        ],
    };
    snapshot.next_id = NOTE_REPEATED + 1;
    let track = snapshot.tracks.first_mut().expect("fixture track");
    track.instrument.as_mut().expect("fixture instrument").state =
        hex_lattice_contract::NativeDeviceState::new(
            hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback"),
            1,
            vec![0],
        )
        .expect("valid test instrument state");
    legacy_project_from_snapshot(snapshot).expect("valid Session source-loop project")
}

fn session_expanded_oracle_project() -> Project {
    let mut snapshot = session_loop_project().snapshot();
    let Some(SessionCellContentSnapshot::Midi(clip)) = snapshot
        .session
        .cells
        .first_mut()
        .map(|cell| &mut cell.content)
    else {
        panic!("fixture Session MIDI clip")
    };
    clip.source.source_start_raw = 0;
    clip.source.loop_start_raw = 0;
    clip.source.loop_end_raw = 4 * Q;
    clip.source.looping = false;
    // The source-start note is intentionally absent from this expanded oracle:
    // the existing contract does not chase an onset authored before source_start.
    clip.source.notes = vec![
        MidiNoteSnapshot {
            active: true,
            id: NOTE_INTRO_ONLY,
            start_raw: Q / 4,
            duration_raw: 5 * Q / 4,
            pitch: 60,
            channel: 0,
            velocity: 100,
        },
        MidiNoteSnapshot {
            active: true,
            id: NOTE_REPEATED,
            start_raw: Q,
            duration_raw: Q / 2,
            pitch: 64,
            channel: 0,
            velocity: 100,
        },
        MidiNoteSnapshot {
            active: true,
            id: NOTE_REPEATED + 1,
            start_raw: 2 * Q,
            duration_raw: Q / 2,
            pitch: 64,
            channel: 0,
            velocity: 100,
        },
        MidiNoteSnapshot {
            active: true,
            id: NOTE_REPEATED + 2,
            start_raw: 3 * Q,
            duration_raw: Q / 2,
            pitch: 64,
            channel: 0,
            velocity: 100,
        },
    ];
    snapshot.next_id = NOTE_REPEATED + 3;
    legacy_project_from_snapshot(snapshot).expect("valid expanded source-loop oracle")
}

pub(super) fn arrangement_loop_project() -> Project {
    let mut snapshot = project(1.0, 0, 5 * Q).snapshot();
    let track = snapshot.tracks.first_mut().expect("fixture track");
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
    let clip = midi_clips.first_mut().expect("fixture Arrangement clip");
    clip.length_raw = 9 * Q / 2;
    clip.source_length_raw = 5 * Q;
    clip.source_start_raw = Q / 2;
    clip.loop_start_raw = Q;
    clip.loop_end_raw = 2 * Q;
    clip.looping = true;
    clip.notes = vec![MidiNoteSnapshot {
        active: true,
        id: 3,
        start_raw: 3 * Q / 2,
        duration_raw: 2 * Q,
        pitch: 60,
        channel: 0,
        velocity: 100,
    }];
    legacy_project_from_snapshot(snapshot).expect("valid Arrangement source-loop project")
}

fn arrangement_expanded_oracle_project() -> Project {
    let mut snapshot = arrangement_loop_project().snapshot();
    let first_note_id = snapshot.next_id;
    let track = snapshot.tracks.first_mut().expect("fixture track");
    let (TrackContentSnapshot::Hybrid { midi_clips, .. }
    | TrackContentSnapshot::Instrument(midi_clips)) = &mut track.content
    else {
        panic!("fixture Arrangement MIDI clip")
    };
    let clip = midi_clips.first_mut().expect("fixture Arrangement clip");
    clip.length_raw = 9 * Q / 2;
    clip.source_length_raw = 5 * Q;
    clip.source_start_raw = 0;
    clip.loop_start_raw = 0;
    clip.loop_end_raw = 5 * Q;
    clip.looping = false;
    clip.notes = (0..4)
        .map(|iteration| MidiNoteSnapshot {
            active: true,
            id: first_note_id + iteration,
            start_raw: (iteration + 1) * Q,
            duration_raw: Q / 2,
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .collect();
    snapshot.next_id = first_note_id + 4;
    legacy_project_from_snapshot(snapshot).expect("valid expanded Arrangement oracle")
}

pub(super) fn try_prepare_mutable(
    project: &Project,
) -> Result<Arc<PreparedRevision>, crate::Invalid> {
    let definition = mutable_definition();
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions)
        .expect("valid test instrument registry");
    PreparedRevision::prepare_with_instrument_registry(
        project,
        &BTreeMap::new(),
        Arc::new(hex_devices::SineTable::new()),
        None,
        &registry,
    )
    .map(Arc::new)
}

pub(super) fn prepare_mutable(project: &Project) -> Arc<PreparedRevision> {
    try_prepare_mutable(project).expect("mutable source loop prepares")
}

fn render_session_loop(partitions: &[usize]) -> (Vec<Frame>, SessionPlayback) {
    render_session_project(&session_loop_project(), partitions)
}

fn render_session_project(
    project: &Project,
    partitions: &[usize],
) -> (Vec<Frame>, SessionPlayback) {
    let revision = prepare_mutable(project);
    let clip = revision.session().midi_clips()[0].id;
    let mut playback =
        SessionPlayback::new(revision).expect("mutable Session source loop admitted");
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .expect("launch Session source loop");
    playback
        .apply(SessionPlaybackCommand::Play)
        .expect("start Session source loop");
    let mut output = vec![[0.0; 2]; 80_001];
    let mut offset = 0;
    let mut partition = 0;
    while offset < output.len() {
        let requested = partitions[partition % partitions.len()].max(1);
        let end = offset.saturating_add(requested).min(output.len());
        playback
            .render(&mut output[offset..end])
            .expect("render Session source loop");
        offset = end;
        partition += 1;
    }
    (output, playback)
}

fn render_arrangement_project(
    project: &Project,
    partitions: &[usize],
) -> (Vec<Frame>, SessionPlayback) {
    let revision = prepare_mutable(project);
    let mut playback = SessionPlayback::new(revision).expect("mutable Arrangement admitted");
    playback
        .apply(SessionPlaybackCommand::Play)
        .expect("start Arrangement playback");
    let mut output = vec![[0.0; 2]; 120_001];
    let mut offset = 0;
    let mut partition = 0;
    while offset < output.len() {
        let requested = partitions[partition % partitions.len()].max(1);
        let end = offset.saturating_add(requested).min(output.len());
        playback
            .render(&mut output[offset..end])
            .expect("render Arrangement source loop");
        offset = end;
        partition += 1;
    }
    (output, playback)
}

#[test]
fn session_loop_has_trimmed_intro_repeats_clipped_gates_and_partition_invariant_pcm() {
    let whole = render_session_loop(&[80_001]).0;
    let authored_oracle = render_session_project(&session_expanded_oracle_project(), &[80_001]).0;
    let (partitioned, partitioned_playback) = render_session_loop(&[1, 17, 127, 256]);
    assert_eq!(whole, partitioned);
    assert_eq!(whole, authored_oracle);

    // Independent authored-event oracle: source start .5, loop [1, 2),
    // note 1 starts .25 before source_start and is not chased; note 2 starts
    // .75 once (before loop_start, with its gate clipped at loop_end); note 3
    // starts 1.5 in the intro plus at 2.0, 3.0 and 4.0. At 120 bpm one beat
    // is 24,000 frames.
    let onset_frames = [6_000, 24_000, 48_000, 72_000];
    for frame in onset_frames {
        assert!(
            whole[frame].iter().any(|sample| *sample > 0.0),
            "onset {frame}"
        );
    }
    assert!(whole[5_999].iter().all(|sample| *sample == 0.0));
    assert!(whole[18_000].iter().any(|sample| *sample > 0.0));
    assert!(whole[47_000].iter().all(|sample| *sample == 0.0));
    assert!(whole[71_000].iter().all(|sample| *sample == 0.0));

    let runtime = partitioned_playback
        .mutable_instruments
        .first()
        .and_then(Option::as_ref)
        .expect("prepared mutable runtime");
    assert_eq!(runtime.next_occurrence_id(), 4);
    let repeated = runtime
        .occurrence_proofs()
        .iter()
        .find(|proof| proof.signature.key().note.value() == NOTE_REPEATED)
        .expect("latest loop note occurrence remains active");
    assert_eq!(repeated.signature.key().iteration, 2);
    assert_eq!(repeated.id, 4);

    let revision = prepare_mutable(&session_loop_project());
    let clip = revision.session().midi_clips()[0].id;
    let mut occurrences = SessionPlayback::new(revision).expect("source loop admission");
    occurrences
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .expect("launch source");
    occurrences
        .apply(SessionPlaybackCommand::Play)
        .expect("start source");
    for (frames, iteration, id, gate_end) in [
        (24_001, 0, 2, 36_000),
        (24_000, 1, 3, 60_000),
        (24_000, 2, 4, 84_000),
    ] {
        let mut segment = vec![[0.0; 2]; frames];
        occurrences
            .render(&mut segment)
            .expect("render source-loop occurrence");
        let proof = occurrences.mutable_instruments[0]
            .as_ref()
            .expect("mutable runtime")
            .occurrence_proofs()
            .iter()
            .find(|proof| {
                proof.signature.key().note.value() == NOTE_REPEATED
                    && proof.signature.key().iteration == iteration
            })
            .copied()
            .expect("active authored loop occurrence");
        assert_eq!(proof.id, id);
        assert_eq!(proof.gate_end_frame, gate_end);
    }
}

#[test]
fn mutable_arrangement_source_loops_admit_finite_occurrences_and_clip_gate_at_loop_end() {
    let revision = prepare_mutable(&arrangement_loop_project());
    let track = revision.arrangement().tracks().first().expect("track");
    let mut spans = Vec::new();
    track
        .for_each_mutable_midi_span(0, 5 * 24_000, |span| {
            spans.push((span.start, span.gate_end, span.iteration));
        })
        .expect("finite mutable Arrangement loop spans");
    assert_eq!(
        spans,
        [
            (24_000, 36_000, 0),
            (48_000, 60_000, 1),
            (72_000, 84_000, 2),
            (96_000, 108_000, 3),
        ]
    );

    let whole = render_arrangement_project(&arrangement_loop_project(), &[120_001]).0;
    let oracle = render_arrangement_project(&arrangement_expanded_oracle_project(), &[120_001]).0;
    let (partitioned, playback) =
        render_arrangement_project(&arrangement_loop_project(), &[1, 17, 127, 256]);
    assert_eq!(whole, oracle);
    assert_eq!(whole, partitioned);
    for frame in [24_000, 48_000, 72_000, 96_000] {
        assert!(whole[frame].iter().any(|sample| *sample > 0.0));
    }
    assert!(whole[107_999].iter().any(|sample| *sample > 0.0));
    assert!(
        whole[108_032..]
            .iter()
            .flatten()
            .all(|sample| *sample == 0.0)
    );
    assert_eq!(
        playback.mutable_instruments[0]
            .as_ref()
            .expect("mutable runtime")
            .next_occurrence_id(),
        4
    );
}

#[test]
fn source_loop_identity_exhaustion_rolls_back_audio_clocks_and_ledger_for_retry() {
    let revision = prepare_mutable(&session_loop_project());
    let clip = revision.session().midi_clips()[0].id;
    let mut failed = SessionPlayback::new(Arc::clone(&revision)).expect("loop admission");
    let mut oracle = SessionPlayback::new(revision).expect("loop oracle admission");
    for playback in [&mut failed, &mut oracle] {
        playback
            .apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .expect("launch source");
        playback
            .apply(SessionPlaybackCommand::Play)
            .expect("start source");
    }

    failed.mutable_instruments[0]
        .as_mut()
        .expect("mutable runtime")
        .set_next_occurrence_id_for_test(u64::MAX);
    let mut rejected = [[7.0; 2]; 6_001];
    assert_eq!(
        failed.render(&mut rejected),
        Err(SessionPlaybackError::MutableInstrumentDiagnostic(
            "mutable instrument occurrence identity exhausted"
        ))
    );
    assert!(rejected.iter().flatten().all(|sample| *sample == 0.0));
    assert_eq!(failed.performance_frame(), 0);
    assert_eq!(failed.arrangement_frame(), 0);
    let failed_runtime = failed.mutable_instruments[0]
        .as_mut()
        .expect("mutable runtime");
    assert!(failed_runtime.occurrence_proofs().is_empty());
    assert_eq!(failed_runtime.next_occurrence_id(), u64::MAX);

    failed_runtime.set_next_occurrence_id_for_test(0);
    let mut retried = [[0.0; 2]; 6_001];
    let mut expected = [[0.0; 2]; 6_001];
    failed
        .render(&mut retried)
        .expect("retry source loop block");
    oracle
        .render(&mut expected)
        .expect("oracle source loop block");
    assert_eq!(retried, expected);
    assert_eq!(failed.performance_frame(), oracle.performance_frame());
    assert_eq!(
        failed.mutable_instruments[0]
            .as_ref()
            .expect("mutable runtime")
            .occurrence_proofs(),
        oracle.mutable_instruments[0]
            .as_ref()
            .expect("oracle runtime")
            .occurrence_proofs()
    );
}

#[test]
fn source_loop_beat_domain_exhaustion_rolls_back_audio_clocks_and_ledger_for_retry() {
    let revision = prepare_mutable(&session_loop_project());
    let clip = revision.session().midi_clips()[0].id;
    let mut failed = SessionPlayback::new(Arc::clone(&revision)).expect("loop admission");
    let mut oracle = SessionPlayback::new(revision).expect("loop oracle admission");
    for playback in [&mut failed, &mut oracle] {
        playback
            .apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .expect("launch source");
        playback
            .apply(SessionPlaybackCommand::Play)
            .expect("start source");
    }

    let frames = 24_001;
    failed.performance_frame = u64::MAX - frames as u64;
    let mut rejected = [[7.0; 2]; 24_001];
    assert_eq!(
        failed.render(&mut rejected),
        Err(SessionPlaybackError::Midi(
            crate::MidiTemplateError::BeatDomainExhausted
        ))
    );
    assert!(rejected.iter().flatten().all(|sample| *sample == 0.0));
    assert_eq!(failed.performance_frame(), u64::MAX - frames as u64);
    assert_eq!(failed.arrangement_frame(), 0);
    let failed_runtime = failed.mutable_instruments[0]
        .as_mut()
        .expect("mutable runtime");
    assert!(failed_runtime.occurrence_proofs().is_empty());
    assert_eq!(failed_runtime.next_occurrence_id(), 0);

    failed.performance_frame = 0;
    let mut retried = [[0.0; 2]; 24_001];
    let mut expected = [[0.0; 2]; 24_001];
    failed
        .render(&mut retried)
        .expect("retry source loop block");
    oracle
        .render(&mut expected)
        .expect("oracle source loop block");
    assert_eq!(retried, expected);
    assert_eq!(failed.performance_frame(), oracle.performance_frame());
    assert_eq!(
        failed.mutable_instruments[0]
            .as_ref()
            .expect("mutable runtime")
            .occurrence_proofs(),
        oracle.mutable_instruments[0]
            .as_ref()
            .expect("oracle runtime")
            .occurrence_proofs()
    );
}

#[test]
fn mutable_source_loop_admission_keeps_period_and_density_bounds() {
    let mut short = session_loop_project().snapshot();
    let Some(SessionCellContentSnapshot::Midi(clip)) = short
        .session
        .cells
        .first_mut()
        .map(|cell| &mut cell.content)
    else {
        panic!("fixture Session MIDI clip")
    };
    clip.source.source_start_raw = 0;
    clip.source.loop_start_raw = 0;
    clip.source.loop_end_raw = 1;
    clip.source.notes.clear();
    let short = legacy_project_from_snapshot(short).expect("valid tiny raw loop");
    assert!(try_prepare_mutable(&short).is_err());

    let mut dense = session_loop_project().snapshot();
    let Some(SessionCellContentSnapshot::Midi(clip)) = dense
        .session
        .cells
        .first_mut()
        .map(|cell| &mut cell.content)
    else {
        panic!("fixture Session MIDI clip")
    };
    clip.source.source_start_raw = 0;
    clip.source.loop_start_raw = 0;
    clip.source.loop_end_raw = Q;
    clip.source.notes = (0..129)
        .map(|index| MidiNoteSnapshot {
            active: true,
            id: 2_000_000 + index,
            // 129 gates per period, with exact adjacency when pitch zero
            // repeats. Preserve density without invalid same-pitch authoring.
            start_raw: (index / 128) * (Q / 4),
            duration_raw: Q / 4,
            pitch: u8::try_from(index % 128).expect("bounded pitch"),
            channel: 0,
            velocity: 100,
        })
        .collect();
    dense.next_id = 2_000_129;
    let dense = legacy_project_from_snapshot(dense).expect("valid dense source loop");
    assert!(try_prepare_mutable(&dense).is_err());
}

#[test]
fn once_boundary_renders_the_original_source_window_in_both_views_across_blocks() {
    use hex_project::MidiOneShotBoundary as Boundary;
    for arrangement in [false, true] {
        let mut snapshot = if arrangement {
            arrangement_loop_project()
        } else {
            session_loop_project()
        }
        .snapshot();
        let (notes, loop_end) = if arrangement {
            let TrackContentSnapshot::Hybrid { midi_clips, .. } = &mut snapshot.tracks[0].content
            else {
                panic!("hybrid")
            };
            let clip = &mut midi_clips[0];
            clip.looping = false;
            clip.one_shot_boundary = Boundary::LoopEnd;
            (&mut clip.notes, clip.loop_end_raw)
        } else {
            let SessionCellContentSnapshot::Midi(clip) = &mut snapshot.session.cells[0].content
            else {
                panic!("midi")
            };
            clip.source.looping = false;
            clip.source.one_shot_boundary = Boundary::LoopEnd;
            (&mut clip.source.notes, clip.source.loop_end_raw)
        };
        notes.push(MidiNoteSnapshot {
            active: true,
            id: NOTE_REPEATED + 1,
            start_raw: 3 * Q,
            duration_raw: Q / 2,
            pitch: 67,
            channel: 0,
            velocity: 100,
        });
        snapshot.next_id = NOTE_REPEATED + 2;
        let candidate = legacy_project_from_snapshot(snapshot.clone()).unwrap();
        let notes = if arrangement {
            let TrackContentSnapshot::Hybrid { midi_clips, .. } = &mut snapshot.tracks[0].content
            else {
                panic!("hybrid")
            };
            midi_clips[0].one_shot_boundary = Boundary::SourceEnd;
            &mut midi_clips[0].notes
        } else {
            let SessionCellContentSnapshot::Midi(clip) = &mut snapshot.session.cells[0].content
            else {
                panic!("midi")
            };
            clip.source.one_shot_boundary = Boundary::SourceEnd;
            &mut clip.source.notes
        };
        // Independent event oracle: explicitly retain only audible notes and cap
        // their gates. The candidate keeps the original later notes unchanged.
        notes.retain(|note| note.start_raw < loop_end);
        for note in notes {
            note.duration_raw = note.duration_raw.min(loop_end - note.start_raw);
        }
        let oracle = legacy_project_from_snapshot(snapshot).unwrap();
        let render = if arrangement {
            render_arrangement_project
        } else {
            render_session_project
        };
        let whole = render(&candidate, &[120001]).0;
        assert_eq!(whole, render(&candidate, &[1, 17, 127, 256]).0);
        assert_eq!(whole, render(&oracle, &[120001]).0);
        let mut outputs = Vec::new();
        for project in [&candidate, &oracle] {
            let revision = prepare_mutable(project);
            let clip = (!arrangement).then(|| revision.session().midi_clips()[0].id);
            let mut playback = SessionPlayback::new(revision).unwrap();
            if let Some(clip) = clip {
                playback
                    .apply(SessionPlaybackCommand::LaunchClip {
                        clip,
                        quantization: LaunchQuantization::Immediate,
                    })
                    .unwrap();
            }
            playback.apply(SessionPlaybackCommand::Play).unwrap();
            let mut output = vec![[0.0; 2]; 24001];
            playback.render(&mut output).unwrap();
            let before = playback.performance_frame();
            assert_eq!(
                playback.apply(SessionPlaybackCommand::Seek(60000)),
                Err(SessionPlaybackError::MutableInstrumentActive)
            );
            assert_eq!(playback.performance_frame(), before);
            let mut release = vec![[0.0; 2]; 24032];
            playback.render(&mut release).unwrap();
            output.extend(release);
            for frame in [60000, 0] {
                playback.apply(SessionPlaybackCommand::Seek(frame)).unwrap();
                let mut block = vec![[0.0; 2]; 48032];
                playback.render(&mut block).unwrap();
                output.extend(block);
            }
            outputs.push(output);
        }
        assert_eq!(outputs[0], outputs[1]);

        assert!(whole[48032..].iter().flatten().all(|sample| *sample == 0.0));
    }
}
