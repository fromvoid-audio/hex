use hex_engine::{
    Audio, PreparedRevision, Renderer, SessionPlayback, SessionPlaybackCommand, SineTable, compile,
};
use hex_project::{
    BeatTime, Command, Created, Editor, GainInterpolation, GainPoint, Project, Tempo, TempoPoint,
};
use std::{collections::BTreeMap, sync::Arc};
struct Fixture {
    editor: Editor,
    track: hex_project::TrackId,
    asset: hex_project::AssetId,
    frames: usize,
}
fn fixture(frames: usize) -> Fixture {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 16).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "source".into(),
            path: "memory.wav".into(),
            source_sample_rate: 48000,
            source_frames: frames as u64,
            playback_frames: frames as u64,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("import")
    };
    editor.execute(Command::Gain { track, gain: 0.0 }).unwrap();
    Fixture {
        editor,
        track,
        asset,
        frames,
    }
}
fn point(beat: BeatTime, gain: f32, outgoing: GainInterpolation) -> GainPoint {
    GainPoint {
        incoming_gain: None,
        beat,
        gain,
        outgoing,
    }
}
fn lane(f: &mut Fixture, points: Vec<GainPoint>) {
    f.editor
        .execute(Command::ReplaceTrackGainEnvelope {
            track: f.track,
            points,
        })
        .unwrap();
}
fn media(f: &Fixture) -> BTreeMap<hex_project::AssetId, Audio> {
    BTreeMap::from([(
        f.asset,
        Audio::new(48000, vec![[0.25, -0.5]; f.frames]).unwrap(),
    )])
}
fn revision(f: &Fixture) -> Arc<PreparedRevision> {
    Arc::new(
        PreparedRevision::prepare(
            f.editor.project(),
            &media(f),
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    )
}
fn playback(f: &Fixture) -> SessionPlayback {
    let mut p = SessionPlayback::new(revision(f)).unwrap();
    p.apply(SessionPlaybackCommand::Play).unwrap();
    p
}
fn arrangement(f: &Fixture) -> Vec<[f32; 2]> {
    let compiled = compile::project(f.editor.project(), &media(f)).unwrap();
    let mut renderer = Renderer::new(&compiled);
    renderer.play();
    let mut output = vec![[0.0; 2]; f.frames];
    renderer.render(&mut output).unwrap();
    output
}

#[test]
fn beat_linear_tempo_split_and_partitions_match_independent_sample_oracle() {
    let mut f = fixture(72002);
    let mut snapshot = f.editor.project().snapshot();
    snapshot.tempo_points = vec![
        TempoPoint::new(BeatTime::ZERO, Tempo::new(120_000_000).unwrap()),
        TempoPoint::new(BeatTime::ONE, Tempo::new(60_000_000).unwrap()),
    ];
    f.editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 16).unwrap();
    lane(
        &mut f,
        vec![
            point(BeatTime::ZERO, 0.0, GainInterpolation::Linear),
            point(
                BeatTime::from_quarters(2).unwrap(),
                2.0,
                GainInterpolation::Step,
            ),
        ],
    );
    let expected: Vec<_> = (0..f.frames)
        .map(|frame| {
            let gain = if frame <= 24000 {
                frame as f64 / 24000.0
            } else if frame < 72000 {
                1.0 + (frame - 24000) as f64 / 48000.0
            } else {
                2.0
            } as f32;
            [0.25 * gain, -0.5 * gain]
        })
        .collect();
    assert_eq!(arrangement(&f), expected);
    let r = revision(&f);
    let capacity = r.arrangement().gain_automation_capacity();
    assert_eq!((capacity.authored_points, capacity.prepared_knots), (2, 3));
    assert!(capacity.allocated_knot_slots >= 3);
    assert!(capacity.allocated_knot_bytes > 0);
    let mut p = playback(&f);
    let mut actual = vec![[0.0; 2]; f.frames];
    p.render(&mut actual).unwrap();
    assert_eq!(actual, expected);
    let mut p = playback(&f);
    for chunk in actual.chunks_mut(17) {
        p.render(chunk).unwrap();
    }
    assert_eq!(actual, expected);
    let mut linear = SessionPlayback::new_linear_arrangement_export(r).unwrap();
    linear.apply(SessionPlaybackCommand::Play).unwrap();
    linear.render(&mut actual).unwrap();
    assert_eq!(actual, expected);
    let mut snapshot = f.editor.project().snapshot();
    snapshot.meter_points = vec![
        hex_project::MeterPoint::new(BeatTime::ZERO, hex_project::Meter::new(7, 8).unwrap()),
        hex_project::MeterPoint::new(BeatTime::ONE, hex_project::Meter::new(5, 4).unwrap()),
    ];
    f.editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 8).unwrap();
    assert_eq!(
        arrangement(&f),
        expected,
        "meter presentation cannot change the gain time law"
    );
}
#[test]
fn endpoints_step_coincident_latest_beat_and_zero_static_gain_are_truthful() {
    let mut f = fixture(32);
    lane(
        &mut f,
        vec![
            point(BeatTime::from_raw(1), 0.25, GainInterpolation::Step),
            point(BeatTime::from_raw(2), 0.5, GainInterpolation::Step),
            point(
                BeatTime::from_ratio(1, 24000).unwrap(),
                1.0,
                GainInterpolation::Step,
            ),
        ],
    );
    let expected: Vec<_> = (0..32)
        .map(|frame| {
            if frame == 0 {
                [0.125, -0.25]
            } else {
                [0.25, -0.5]
            }
        })
        .collect();
    assert_eq!(arrangement(&f), expected);
    let mut p = playback(&f);
    let mut output = vec![[0.0; 2]; 32];
    p.render(&mut output).unwrap();
    assert_eq!(output, expected);
    assert_eq!(
        f.editor
            .project()
            .track(f.track)
            .unwrap()
            .gain_envelope()
            .len(),
        3
    );
    lane(
        &mut f,
        vec![point(BeatTime::ONE, 0.75, GainInterpolation::Linear)],
    );
    assert_eq!(arrangement(&f), vec![[0.1875, -0.375]; 32]);
    let compiled = compile::project(f.editor.project(), &media(&f)).unwrap();
    let mut untouched = [[9.0; 2]; 1];
    assert!(
        compiled.tracks()[0]
            .render_additive(0, &mut untouched)
            .is_err()
    );
    assert_eq!(untouched, [[9.0; 2]; 1]);
    f.editor
        .execute(Command::ClearTrackGainEnvelope { track: f.track })
        .unwrap();
    assert_eq!(arrangement(&f), vec![[0.0; 2]; 32]);
}
#[test]
fn transport_clock_survives_session_ownership_pause_seek_wrap_and_linear_export() {
    let mut f = fixture(600);
    let one_frame = BeatTime::from_ratio(1, 24000).unwrap();
    lane(
        &mut f,
        vec![
            point(BeatTime::ZERO, 0.5, GainInterpolation::Step),
            point(one_frame, 1.0, GainInterpolation::Step),
        ],
    );
    let Created::Scene(scene) = f
        .editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(clip) = f
        .editor
        .execute(Command::CreateSessionAudioClip {
            track: f.track,
            scene,
            name: "session".into(),
            asset: f.asset,
            source_start: 0,
            source_end: 600,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    let mut p = playback(&f);
    p.render(&mut [[0.0; 2]; 4]).unwrap();
    p.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: hex_engine::LaunchQuantization::Immediate,
    })
    .unwrap();
    let mut output = [[0.0; 2]; 4];
    p.render(&mut output).unwrap();
    assert_eq!(output, [[0.25, -0.5]; 4]);
    p.apply(SessionPlaybackCommand::Pause).unwrap();
    p.render(&mut output).unwrap();
    assert_eq!(output, [[0.0; 2]; 4]);
    p.apply(SessionPlaybackCommand::Play).unwrap();
    p.render(&mut output).unwrap();
    assert_eq!(output, [[0.25, -0.5]; 4]);
    p.apply(SessionPlaybackCommand::Seek(0)).unwrap();
    p.render(&mut output).unwrap();
    assert_eq!(output[0], [0.125, -0.25]);
    p.apply(SessionPlaybackCommand::Stop).unwrap();
    p.apply(SessionPlaybackCommand::Play).unwrap();
    p.render(&mut output).unwrap();
    assert_eq!(output[0], [0.125, -0.25]);
    f.editor
        .execute(Command::SetLoopRegion {
            start: 0,
            end: 2,
            enabled: true,
        })
        .unwrap();
    let mut p = playback(&f);
    p.render(&mut output).unwrap();
    assert_eq!(
        output,
        [[0.125, -0.25], [0.25, -0.5], [0.125, -0.25], [0.25, -0.5]]
    );
    let mut linear = SessionPlayback::new_linear_arrangement_export(revision(&f)).unwrap();
    linear.apply(SessionPlaybackCommand::Play).unwrap();
    linear.render(&mut output).unwrap();
    assert_eq!(
        output,
        [[0.125, -0.25], [0.25, -0.5], [0.25, -0.5], [0.25, -0.5]]
    );
}
#[test]
fn gain_balance_mute_is_once_before_sends_return_feeds_and_master() {
    let mut f = fixture(8);
    f.editor
        .execute(Command::Balance {
            track: f.track,
            balance: 0.5,
        })
        .unwrap();
    lane(
        &mut f,
        vec![point(BeatTime::ZERO, 0.5, GainInterpolation::Step)],
    );
    let mut buses = Vec::new();
    for _ in 0..2 {
        let Created::ReturnTrack(id) = f
            .editor
            .execute(Command::AddReturnTrack { name: "bus".into() })
            .unwrap()
        else {
            panic!("return")
        };
        buses.push(id);
    }
    f.editor
        .execute(Command::SetReturnSend {
            track: f.track,
            return_track: buses[0],
            gain: 0.5,
        })
        .unwrap();
    f.editor
        .execute(Command::SetReturnFeed {
            source: buses[0],
            destination: buses[1],
            gain: 0.5,
        })
        .unwrap();
    f.editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    let expected = vec![[0.0546875, -0.21875]; 8];
    assert_eq!(arrangement(&f), expected);
    let mut output = vec![[0.0; 2]; 8];
    playback(&f).render(&mut output).unwrap();
    assert_eq!(output, expected);
    let state = hex_engine::patch_native_device_parameter(
        &hex_engine::default_native_device_state(hex_engine::DeviceTypeId::from_static(
            "hex.utility",
        ))
        .unwrap(),
        hex_lattice_contract::ParameterId::new(6).unwrap(),
        hex_lattice_contract::ParameterValue::Bool(true),
    )
    .unwrap();
    f.editor
        .execute(Command::InsertDevice {
            track: f.track,
            before: None,
            state,
        })
        .unwrap();
    let inverted = vec![[0.0546875, 0.21875]; 8];
    assert_eq!(arrangement(&f), inverted);
    playback(&f).render(&mut output).unwrap();
    assert_eq!(output, inverted);
    f.editor
        .execute(Command::Mute {
            track: f.track,
            muted: true,
        })
        .unwrap();
    assert_eq!(arrangement(&f), vec![[0.0; 2]; 8]);
}
#[test]
fn automated_hybrid_track_matches_static_unity_oracle_and_tempo_edit_reprepares_at_position() {
    let mut f = fixture(48000);
    let Created::Track(source) = f
        .editor
        .execute(Command::AddTrack {
            name: "silent midi source".into(),
        })
        .unwrap()
    else {
        panic!("source track")
    };
    f.editor
        .execute(Command::InsertInstrument {
            track: f.track,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap();
    f.editor
        .execute(Command::SetMidiSource {
            destination: f.track,
            source: Some(source),
        })
        .unwrap();
    let Created::Clip(clip) = f
        .editor
        .execute(Command::PlaceMidiClip {
            track: source,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("midi")
    };
    f.editor
        .execute(Command::AddMidiNotes {
            clip,
            notes: vec![hex_project::MidiNoteInsert {
                active: true,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 100,
            }],
        })
        .unwrap();
    let mut static_snapshot = f.editor.project().snapshot();
    static_snapshot.tracks[0].gain = 1.0;
    let oracle = Fixture {
        editor: Editor::new(Project::from_snapshot(static_snapshot).unwrap(), 8).unwrap(),
        track: f.track,
        asset: f.asset,
        frames: f.frames,
    };
    lane(
        &mut f,
        vec![point(BeatTime::ZERO, 1.0, GainInterpolation::Step)],
    );
    assert_eq!(arrangement(&f), arrangement(&oracle));
    let mut p = playback(&f);
    let mut actual = vec![[0.0; 2]; f.frames];
    p.render(&mut actual).unwrap();
    assert_eq!(actual, arrangement(&oracle));
    lane(
        &mut f,
        vec![
            point(BeatTime::ZERO, 0.0, GainInterpolation::Linear),
            point(BeatTime::ONE, 1.0, GainInterpolation::Step),
        ],
    );
    let mut old = playback(&f);
    old.render(&mut [[0.0; 2]; 12000]).unwrap();
    let beat = f
        .editor
        .project()
        .tempo_map()
        .frame_to_beat(12_000)
        .unwrap();
    let mut snapshot = f.editor.project().snapshot();
    snapshot.tempo_points = vec![TempoPoint::new(
        BeatTime::ZERO,
        Tempo::new(60_000_000).unwrap(),
    )];
    f.editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 8).unwrap();
    // Keep the old beat under the new map, without implicit gain smoothing.
    let frame = f.editor.project().tempo_map().beat_to_frame(beat).unwrap();
    assert_eq!(frame, 24_000);
    let r = revision(&f);
    let mut next = SessionPlayback::new(r).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(next.arrangement_frame(), frame);
    let mut sample = [[0.0; 2]; 1];
    next.render(&mut sample).unwrap();
    // The retained shared MIDI source keeps its performance clock while the
    // Arrangement gain follows the remapped musical cursor. Use an independent
    // unity-gain render at that source clock and apply the beat-1/2 gain.
    let mut unity_snapshot = f.editor.project().snapshot();
    unity_snapshot.tracks[0].gain = 1.0;
    unity_snapshot.tracks[0].gain_envelope.clear();
    let unity = Fixture {
        editor: Editor::new(Project::from_snapshot(unity_snapshot).unwrap(), 8).unwrap(),
        track: f.track,
        asset: f.asset,
        frames: f.frames,
    };
    let source = arrangement(&unity)[12_000];
    assert_eq!(sample[0], [source[0] * 0.5, source[1] * 0.5]);
}

#[test]
fn detached_effect_tail_freezes_the_effective_transition_gain_and_current_gain_edit_keeps_history()
{
    let mut f = fixture(64);
    let state = hex_engine::patch_native_device_parameter(
        &hex_engine::default_native_device_state(hex_engine::DeviceTypeId::from_static(
            "hex.utility",
        ))
        .unwrap(),
        hex_lattice_contract::ParameterId::new(11).unwrap(),
        hex_lattice_contract::ParameterValue::Bool(true),
    )
    .unwrap();
    f.editor
        .execute(Command::InsertDevice {
            track: f.track,
            before: None,
            state,
        })
        .unwrap();
    lane(
        &mut f,
        vec![
            point(BeatTime::ZERO, 0.0, GainInterpolation::Linear),
            point(
                BeatTime::from_ratio(8, 24000).unwrap(),
                1.0,
                GainInterpolation::Step,
            ),
        ],
    );
    let mut old = playback(&f);
    old.render(&mut [[0.0; 2]; 4]).unwrap();
    let mut snapshot = f.editor.project().snapshot();
    snapshot.tracks[0].gain_envelope.clear();
    snapshot.tracks[0].gain = 0.5;
    let p = Project::from_snapshot(snapshot).unwrap();
    let mut samples = vec![[0.25, -0.5]; 64];
    samples[4..].fill([0.0; 2]);
    let mut oracle = SessionPlayback::new(Arc::new(
        PreparedRevision::prepare(
            &p,
            &BTreeMap::from([(f.asset, Audio::new(48000, samples).unwrap())]),
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    ))
    .unwrap();
    oracle.apply(SessionPlaybackCommand::Play).unwrap();
    oracle.render(&mut [[0.0; 2]; 4]).unwrap();
    let mut expected = [[0.0; 2]; 12];
    oracle.render(&mut expected).unwrap();
    assert!(expected.iter().flatten().any(|value| value.abs() > 1e-6));
    f.editor.execute(Command::RemoveTrack(f.track)).unwrap();
    let mut next = SessionPlayback::new(revision(&f)).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut actual = [[0.0; 2]; 12];
    next.render(&mut actual).unwrap();
    assert_eq!(actual, expected);
    // Separate same-chain handoff: editing the lane cannot reset processor history.
    assert!(f.editor.undo());
    let mut old = playback(&f);
    old.render(&mut [[0.0; 2]; 4]).unwrap();
    lane(
        &mut f,
        vec![point(BeatTime::ZERO, 0.75, GainInterpolation::Step)],
    );
    let mut oracle = playback(&f);
    oracle.render(&mut [[0.0; 2]; 4]).unwrap();
    oracle.render(&mut expected).unwrap();
    let mut next = SessionPlayback::new(revision(&f)).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    next.render(&mut actual).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn readonly_preview_matches_compiled_pcm_law_at_tempo_cuts_and_coincident_frames() {
    let mut f = fixture(60002);
    f.editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::ONE,
            tempo: Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    lane(
        &mut f,
        vec![
            point(BeatTime::from_raw(1), 0.1, GainInterpolation::Step),
            point(BeatTime::from_raw(2), 0.25, GainInterpolation::Linear),
            point(
                BeatTime::from_quarters(2).unwrap(),
                1.0,
                GainInterpolation::Step,
            ),
        ],
    );
    let preview = hex_engine::GainAutomationPreview::prepare(f.editor.project()).unwrap();
    let graph = compile::project(f.editor.project(), &media(&f)).unwrap();
    let track = &graph.tracks()[0];
    assert_eq!(
        preview
            .knots(f.track)
            .iter()
            .map(|k| (k.frame(), k.gain(), k.outgoing()))
            .collect::<Vec<_>>(),
        track
            .gain_envelope_knots()
            .iter()
            .map(|k| (k.frame(), k.gain(), k.outgoing()))
            .collect::<Vec<_>>()
    );
    assert_eq!(preview.value_at(f.track, 0), Some(0.25));
    let pcm = arrangement(&f);
    for frame in [0, 12000, 24000, 48000, 60000] {
        let gain = preview.value_at(f.track, frame).unwrap();
        assert_eq!(pcm[frame as usize], [0.25 * gain, -0.5 * gain]);
    }

    for frame in [0, 12000, 24000, 48000, 60000] {
        assert_eq!(
            preview.value_at(f.track, frame),
            track.prepared_gain_at(frame)
        );
    }
    assert!((preview.value_at(f.track, 24000).unwrap() - 0.625).abs() < 1e-6);
    assert_eq!(
        f.editor.project().track(f.track).unwrap().gain_envelope()[0]
            .beat
            .raw(),
        1
    );
    assert_eq!(
        f.editor.project().track(f.track).unwrap().gain_envelope()[1]
            .beat
            .raw(),
        2
    );
}

#[path = "gain_envelope/touch.rs"]
mod touch;
