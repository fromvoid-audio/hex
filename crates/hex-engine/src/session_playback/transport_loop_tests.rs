use super::source_loop_tests::{arrangement_loop_project, prepare_mutable};
use super::*;

const Q: u64 = 1_u64 << 32;

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
            id: 100 + index,
            start_raw: 0,
            duration_raw: 2 * Q,
            pitch: 48 + (index % 12) as u8,
            channel: 0,
            velocity: 100,
        })
        .chain(std::iter::once(MidiNoteSnapshot {
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
    midi_clips[0].notes.push(duplicate);
    let project = legacy_project_from_snapshot(snapshot).expect("valid duplicated loop notes");
    let revision = prepare_mutable(&project);
    let track = &revision.arrangement().tracks()[0];
    let region = revision
        .arrangement()
        .loop_region()
        .expect("fixture transport loop");

    assert_eq!(
        super::transport_loop_admission::admit_track(track, region, 16, 1, 64),
        Err(SessionPlaybackError::MutableInstrumentLoopDensity)
    );
    assert_eq!(
        super::transport_loop_admission::admit_track(track, region, 16, 256, 1),
        Err(SessionPlaybackError::MutableInstrumentLoopDensity)
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
            id: first_note_id + index,
            start_raw: beat_raw_for_frames(17 + index),
            duration_raw: beat_raw_for_frames(1),
            pitch: 36 + (index % 48) as u8,
            channel: 0,
            velocity: 100,
        })
        .chain((0..64_u64).map(|index| MidiNoteSnapshot {
            id: first_note_id + 128 + index,
            start_raw: beat_raw_for_frames(450),
            duration_raw: beat_raw_for_frames(550),
            pitch: 36 + (index % 48) as u8,
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
    assert_eq!(
        tempo_candidate.try_inherit_state_from(&mut tempo_active),
        Err(SessionPlaybackError::MutableInstrumentActive),
        "tempo edit must refuse while a chased loop gate is active"
    );
    assert_eq!(tempo_active.arrangement_loop_pass, 1);
    let mut after_refusal = [[0.0; 2]; 128];
    let mut uninterrupted = [[0.0; 2]; 128];
    tempo_active
        .render(&mut after_refusal)
        .expect("old loop continues after tempo refusal");
    tempo_reference
        .render(&mut uninterrupted)
        .expect("uninterrupted loop reference");
    assert_eq!(after_refusal, uninterrupted);
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
