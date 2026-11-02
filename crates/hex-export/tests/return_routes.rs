use hex_engine::{Audio, PreparedRevision, SineTable};
use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
use hex_project::{Command, Created, Editor, Project};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

const RATE: u32 = 48_000;
static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("hex-export-return-{}-{id}", std::process::id()));
        fs::create_dir(&path).expect("create isolated export directory");
        Self(path)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn prepared_export_renders_effectful_return_before_master_once() {
    assert_return_export(false, false);
}
#[test]
fn prepared_export_includes_authored_additive_return_feed_before_master_once() {
    assert_return_export(true, false);
}
#[test]
fn prepared_export_applies_gain_automation_with_return_feed_and_zero_base_once() {
    assert_return_export(true, true);
}
fn assert_return_export(authored_feed: bool, automation: bool) {
    let samples = [[0.25, -0.5]];
    let mut editor = Editor::new(Project::new(RATE).expect("project"), 16).expect("editor");
    let asset = match editor
        .execute(Command::AddAsset {
            path: "memory/return-export.wav".into(),
            sample_rate: RATE,
            frames: 1,
        })
        .expect("asset")
    {
        Created::Asset(id) => id,
        _ => panic!("expected audio asset"),
    };
    let track = match editor
        .execute(Command::AddTrack {
            name: "source".into(),
        })
        .expect("track")
    {
        Created::Track(id) => id,
        _ => panic!("expected audio track"),
    };
    editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 0,
            duration: 1,
            source_offset: 0,
            source_length: 1,
            looping: false,
        })
        .expect("clip");
    editor
        .execute(Command::Gain { track, gain: 0.5 })
        .expect("track gain");
    let return_track = match editor
        .execute(Command::AddReturnTrack {
            name: "utility".into(),
        })
        .expect("return")
    {
        Created::ReturnTrack(id) => id,
        _ => panic!("expected return track"),
    };
    editor
        .execute(Command::SetReturnSend {
            track,
            return_track,
            gain: 0.5,
        })
        .expect("send");
    editor
        .execute(Command::MasterGain { gain: 0.5 })
        .expect("master gain");
    editor
        .execute(Command::InsertReturnDevice {
            return_track,
            before: None,
            state: hex_engine::patch_native_device_parameter(
                &hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.utility"))
                    .expect("Utility defaults"),
                ParameterId::from_static(6),
                ParameterValue::Bool(true),
            )
            .expect("Utility state"),
        })
        .expect("return effect");

    if authored_feed {
        let Created::ReturnTrack(destination) = editor
            .execute(Command::AddReturnTrack {
                name: "destination".into(),
            })
            .unwrap()
        else {
            panic!("return")
        };
        editor
            .execute(Command::SetReturnFeed {
                source: return_track,
                destination,
                gain: 0.5,
            })
            .unwrap();
    }
    if automation {
        editor.execute(Command::Gain { track, gain: 0.0 }).unwrap();
        editor
            .execute(Command::ReplaceTrackGainEnvelope {
                track,
                points: vec![hex_project::GainPoint {
                    incoming_gain: None,
                    beat: hex_project::BeatTime::ZERO,
                    gain: 1.0,
                    outgoing: hex_project::GainInterpolation::Step,
                }],
            })
            .unwrap();
    }
    let media = BTreeMap::from([(asset, Audio::new(RATE, samples.to_vec()).expect("audio"))]);
    let revision = Arc::new(
        PreparedRevision::prepare(editor.project(), &media, Arc::new(SineTable::new()), None)
            .expect("prepared return revision"),
    );
    let directory = TestDir::new();
    let path = directory.0.join("return.wav");
    let report = hex_export::render_prepared_new(&path, Arc::clone(&revision), 0, 1)
        .expect("prepared Return export");
    assert_eq!(report.frames, 1);
    let bytes = fs::read(path).expect("read export");
    let left = i16::from_le_bytes([bytes[44], bytes[45]]);
    let right = i16::from_le_bytes([bytes[46], bytes[47]]);
    let expected = if automation {
        [0.21875_f32, -0.0625_f32]
    } else if authored_feed {
        [0.109375_f32, -0.03125_f32]
    } else {
        [0.09375_f32, -0.0625_f32]
    };
    for (actual, expected) in [left, right].into_iter().zip(expected) {
        let pcm = (expected * 32_767.0).round() as i16;
        assert!((i32::from(actual) - i32::from(pcm)).abs() <= 1);
    }

    // Capture actual post-fader/Return outputs from the same graph, excluding
    // master gain. These independent PCM expectations distinguish send input,
    // Return output, dry track and Main instead of mirroring capture buffers.
    // Automation replaces the base fader, including when that base is zero.
    let track_output = if automation {
        [0.25_f32, -0.5]
    } else {
        [0.125_f32, -0.25]
    };
    // The post-fader send is halved; Utility inverts the right channel.
    let return_output = if automation {
        [0.125_f32, 0.25]
    } else {
        [0.0625_f32, 0.125]
    };
    for (name, id, expected) in [
        (
            "track",
            hex_engine::OutputCaptureId::Track(track),
            track_output,
        ),
        (
            "return",
            hex_engine::OutputCaptureId::Return(return_track),
            return_output,
        ),
    ] {
        let control = hex_export::ExportControl::default();
        let target = directory.0.join(format!("stem-{name}.wav"));
        hex_export::render_prepared_controlled(
            &target,
            Arc::clone(&revision),
            0,
            1,
            hex_export::ExportSignal::CapturedMix(vec![id]),
            &control,
        )
        .expect("captured output export");
        let data = fs::read(target).unwrap();
        for (channel, expected) in expected.into_iter().enumerate() {
            let offset = 44 + channel * 2;
            let actual = i16::from_le_bytes([data[offset], data[offset + 1]]);
            assert_pcm16_near(actual, expected, "captured output");
        }
        assert_eq!(
            control.progress(),
            hex_export::ExportProgress {
                processed_frames: 1,
                total_frames: 1
            }
        );
    }
    let batch = directory.0.join("stems");
    let stems = vec![
        hex_export::Stem {
            id: hex_engine::OutputCaptureId::Return(return_track),
            name: "same/name".into(),
        },
        hex_export::Stem {
            id: hex_engine::OutputCaptureId::Track(track),
            name: "same/name".into(),
        },
    ];
    hex_export::render_stems_new(
        &batch,
        Arc::clone(&revision),
        0,
        1,
        &stems,
        &hex_export::ExportControl::default(),
    )
    .unwrap();
    let files: Vec<_> = fs::read_dir(&batch)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(files.len(), 2);
    for (kind, id, expected) in [
        ("track", track.value(), track_output),
        ("return", return_track.value(), return_output),
    ] {
        let data = fs::read(batch.join(format!("{kind}-{id}-same_name.wav"))).unwrap();
        assert_eq!(data.len(), 48);
        for (channel, expected) in expected.into_iter().enumerate() {
            let offset = 44 + channel * 2;
            let actual = i16::from_le_bytes([data[offset], data[offset + 1]]);
            assert_pcm16_near(actual, expected, "captured output");
        }
    }
    // Refuse the whole existing batch; never merge/delete files in it.
    fs::write(batch.join("unrelated.txt"), b"preserve me").unwrap();
    assert!(
        hex_export::render_stems_new(
            &batch,
            Arc::clone(&revision),
            0,
            1,
            &stems,
            &hex_export::ExportControl::default()
        )
        .is_err()
    );
    assert_eq!(
        fs::read(batch.join("unrelated.txt")).unwrap(),
        b"preserve me"
    );
    let duplicate = directory.0.join("duplicates");
    let repeated = vec![stems[0].clone(), stems[0].clone()];
    assert!(
        hex_export::render_stems_new(
            &duplicate,
            Arc::clone(&revision),
            0,
            1,
            &repeated,
            &hex_export::ExportControl::default()
        )
        .is_err()
    );
    assert!(!duplicate.exists());
    assert!(!fs::read_dir(&directory.0).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".hex-stems-")
    }));
    let cancelled = hex_export::ExportControl::default();
    cancelled.cancel();
    let target = directory.0.join("cancelled.wav");
    assert!(matches!(
        hex_export::render_prepared_controlled(
            &target,
            revision,
            0,
            1,
            hex_export::ExportSignal::Main,
            &cancelled
        ),
        Err(hex_export::ExportError::Cancelled)
    ));
    assert!(!target.exists());
}

#[test]
fn transformed_warp_root_gain_lane_and_playable_routing_match_native_linear_and_export() {
    use hex_project::{BeatsPreserve, BeatsTail, WarpMode};
    for mode in [
        WarpMode::Texture {
            grain_ms: 8,
            fluctuation: 25,
            seed: 37,
        },
        WarpMode::Beats {
            preserve: BeatsPreserve::Transients,
            tail: BeatsTail::Forward,
            decay: 100,
        },
    ] {
        assert_transformed_warp_export(mode);
    }
}
fn assert_transformed_warp_export(mode: hex_project::WarpMode) {
    use hex_project::{
        BeatTime, GainInterpolation, GainPoint, RoutingOwner, SourceGainPoint, Tempo,
        default_audio_warp,
    };
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 32).unwrap();
    let Created::Imported { clip, asset, track } = editor
        .execute(Command::ImportAudioTrack {
            name: "warped".into(),
            path: "warped.wav".into(),
            sample_rate: RATE,
            frames: 256,
        })
        .unwrap()
    else {
        panic!("import")
    };
    editor
        .execute(Command::SetClipLooping {
            clip,
            looping: true,
        })
        .unwrap();
    editor
        .execute(Command::SetClipSourcePhase { clip, phase: 7 })
        .unwrap();
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 8,
            fade_out: 16,
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipGain {
            clip,
            millidb: -6_000,
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 137 })
        .unwrap();
    let settings = default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        mode,
    )
    .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(settings),
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipSourceGain {
            clip,
            points: vec![
                SourceGainPoint {
                    frame: 0,
                    gain_q16: 32_768,
                },
                SourceGainPoint {
                    frame: 256,
                    gain_q16: 65_536,
                },
            ],
        })
        .unwrap();
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::ZERO,
            tempo: Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    let Created::Track(target) = editor
        .execute(Command::AddTrack {
            name: "input".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::SetAudioFeed {
            source: RoutingOwner::Track(track),
            destination: target,
            gain: 0.5,
        })
        .unwrap();
    editor
        .execute(Command::Gain {
            track: target,
            gain: 0.5,
        })
        .unwrap();
    editor.execute(Command::Gain { track, gain: 0.0 }).unwrap();
    editor
        .execute(Command::ReplaceTrackGainEnvelope {
            track,
            points: vec![
                GainPoint {
                    incoming_gain: None,
                    beat: BeatTime::ZERO,
                    gain: 0.25,
                    outgoing: GainInterpolation::Linear,
                },
                GainPoint {
                    incoming_gain: None,
                    beat: BeatTime::ONE,
                    gain: 0.75,
                    outgoing: GainInterpolation::Step,
                },
            ],
        })
        .unwrap();
    let mut buses = Vec::new();
    for _ in 0..2 {
        let Created::ReturnTrack(bus) = editor
            .execute(Command::AddReturnTrack { name: "bus".into() })
            .unwrap()
        else {
            panic!("return")
        };
        buses.push(bus);
    }
    editor
        .execute(Command::ReturnGain {
            return_track: buses[0],
            gain: 0.5,
        })
        .unwrap();
    editor
        .execute(Command::SetReturnSend {
            track,
            return_track: buses[0],
            gain: 0.25,
        })
        .unwrap();
    editor
        .execute(Command::SetReturnFeed {
            source: buses[0],
            destination: buses[1],
            gain: 0.5,
        })
        .unwrap();
    editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    let media = BTreeMap::from([(
        asset,
        Audio::new(
            RATE,
            (0..256)
                .map(|i| {
                    let x = (i as f32 * 0.173).sin() * 0.25;
                    [x, -x * 0.75]
                })
                .collect(),
        )
        .unwrap(),
    )]);
    let revision = Arc::new(
        PreparedRevision::prepare(editor.project(), &media, Arc::new(SineTable::new()), None)
            .unwrap(),
    );
    let view = &revision.arrangement().audio_clip_views()[0];
    let length = usize::try_from(view.length).unwrap();
    assert_eq!(length, 512);
    let clip_gain = editor.project().clip(clip).unwrap().gain().linear();
    // Main gets source + target(1/4) + first Return(1/8) + second Return(1/16).
    let expected = view
        .audio
        .frames()
        .iter()
        .enumerate()
        .map(|(i, frame)| {
            let lane = 0.25 + 0.5 * i as f32 / RATE as f32;
            frame.map(|sample| sample * clip_gain * lane * 1.4375 * 0.5)
        })
        .collect::<Vec<_>>();
    assert!(expected.iter().flatten().any(|sample| sample.abs() > 0.001));
    let mut native =
        hex_engine::playback::Playback::try_from_prepared_with_session(Arc::clone(&revision))
            .unwrap();
    native.apply(hex_engine::playback::PlaybackCommand::Play);
    let mut actual = vec![[0.; 2]; length];
    for block in actual.chunks_mut(31) {
        native.render(block).unwrap();
    }
    let mut linear =
        hex_engine::SessionPlayback::new_linear_arrangement_export(Arc::clone(&revision)).unwrap();
    linear
        .apply(hex_engine::SessionPlaybackCommand::Play)
        .unwrap();
    let mut offline = vec![[0.; 2]; length];
    for block in offline.chunks_mut(47) {
        linear.render(block).unwrap();
    }
    assert_eq!(actual, offline);
    for (actual, expected) in actual.iter().flatten().zip(expected.iter().flatten()) {
        assert!((actual - expected).abs() < 1e-6, "{actual} != {expected}");
    }
    let directory = TestDir::new();
    let path = directory.0.join("musical-routing.wav");
    hex_export::render_prepared_new(&path, revision, 0, length as u64).unwrap();
    let bytes = fs::read(path).unwrap();
    for (pcm, expected) in bytes[44..].chunks_exact(2).zip(actual.iter().flatten()) {
        let sample = i16::from_le_bytes([pcm[0], pcm[1]]);
        let expected = (expected * 32_767.).round() as i16;
        assert!((i32::from(sample) - i32::from(expected)).abs() <= 1);
    }
}

#[test]
fn automatic_ranges_refuse_audible_unbounded_delay_only_on_relevant_paths() {
    use hex_engine::OutputCaptureId;
    use hex_export::{ExportControl, ExportSignal, Stem, controlled_frame_count};
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 16).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "impulse".into(),
            path: "memory.wav".into(),
            source_sample_rate: RATE,
            source_frames: 1,
            playback_frames: 1,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("fixture track")
    };
    let Created::Track(dry_track) = editor
        .execute(Command::AddTrack { name: "dry".into() })
        .unwrap()
    else {
        panic!("dry track")
    };
    let mut state =
        hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.delay")).unwrap();
    for (id, value) in [
        (1, ParameterValue::Float(10.0)),
        (2, ParameterValue::Float(0.5)),
        (3, ParameterValue::Float(1.0)),
        (5, ParameterValue::Bool(false)),
        (15, ParameterValue::Bool(false)),
        (26, ParameterValue::Bool(false)),
    ] {
        state =
            hex_engine::patch_native_device_parameter(&state, ParameterId::from_static(id), value)
                .unwrap();
    }
    let delay_state = state.clone();
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state,
        })
        .unwrap()
    else {
        panic!("delay")
    };
    let media = BTreeMap::from([(asset, Audio::new(RATE, vec![[0.5, -0.5]]).unwrap())]);
    let prepare = |editor: &Editor| {
        Arc::new(
            PreparedRevision::prepare(editor.project(), &media, Arc::new(SineTable::new()), None)
                .unwrap(),
        )
    };
    let revision = prepare(&editor);
    let delayed = ExportSignal::CapturedMix(vec![OutputCaptureId::Track(track)]);
    let dry = ExportSignal::CapturedMix(vec![OutputCaptureId::Track(dry_track)]);
    let stems = ExportSignal::Stems(vec![Stem {
        id: OutputCaptureId::Track(track),
        name: "impulse".into(),
    }]);
    let directory = TestDir::new();
    let path = directory.0.join("delay.wav");
    hex_export::render_prepared_controlled(
        &path,
        Arc::clone(&revision),
        0,
        600,
        delayed.clone(),
        &ExportControl::default(),
    )
    .unwrap();
    let bytes = fs::read(&path).unwrap();
    let pcm: Vec<_> = bytes[44..]
        .chunks_exact(4)
        .map(|x| {
            [
                i16::from_le_bytes([x[0], x[1]]),
                i16::from_le_bytes([x[2], x[3]]),
            ]
        })
        .collect();
    assert_pcm16_near(pcm[480][0], 0.5, "delayed left");
    assert_pcm16_near(pcm[480][1], -0.5, "delayed right");
    for signal in [&ExportSignal::Main, &delayed, &stems] {
        let error = controlled_frame_count(revision.arrangement(), 0, None, signal).unwrap_err();
        assert!(error.to_string().contains("explicit range"), "{error}");
        assert_eq!(
            controlled_frame_count(revision.arrangement(), 0, Some(600), signal).unwrap(),
            600
        );
    }
    assert_eq!(
        controlled_frame_count(revision.arrangement(), 0, None, &dry).unwrap(),
        1
    );
    let oversized = ExportSignal::CapturedMix(vec![
        OutputCaptureId::Track(dry_track);
        hex_engine::MAX_OUTPUT_CAPTURES + 1
    ]);
    assert!(controlled_frame_count(revision.arrangement(), 0, None, &oversized).is_err());
    editor
        .execute(Command::SetDeviceBypass {
            device,
            bypassed: true,
        })
        .unwrap();
    let bypassed = prepare(&editor);
    for signal in [&ExportSignal::Main, &delayed, &stems] {
        assert_eq!(
            controlled_frame_count(bypassed.arrangement(), 0, None, signal).unwrap(),
            1
        );
    }

    let Created::ReturnTrack(bus) = editor
        .execute(Command::AddReturnTrack {
            name: "delay return".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    editor
        .execute(Command::SetReturnSend {
            track,
            return_track: bus,
            gain: 1.0,
        })
        .unwrap();
    editor
        .execute(Command::InsertReturnDevice {
            return_track: bus,
            before: None,
            state: delay_state.clone(),
        })
        .unwrap();
    let returned = prepare(&editor);
    let return_signal = ExportSignal::CapturedMix(vec![OutputCaptureId::Return(bus)]);
    assert!(controlled_frame_count(returned.arrangement(), 0, None, &ExportSignal::Main).is_err());
    assert!(controlled_frame_count(returned.arrangement(), 0, None, &return_signal).is_err());
    assert_eq!(
        controlled_frame_count(returned.arrangement(), 0, None, &delayed).unwrap(),
        1
    );
    assert_eq!(
        controlled_frame_count(returned.arrangement(), 0, None, &dry).unwrap(),
        1
    );

    let Created::Track(group) = editor
        .execute(Command::GroupTracks {
            tracks: hex_project::TrackSelectionIds::new(&[track]).unwrap(),
            name: "group".into(),
        })
        .unwrap()
    else {
        panic!("group")
    };
    editor
        .execute(Command::InsertDevice {
            track: group,
            before: None,
            state: delay_state,
        })
        .unwrap();
    let grouped = prepare(&editor);
    let group_signal = ExportSignal::CapturedMix(vec![OutputCaptureId::Track(group)]);
    assert!(controlled_frame_count(grouped.arrangement(), 0, None, &group_signal).is_err());
    assert_eq!(
        controlled_frame_count(grouped.arrangement(), 0, None, &delayed).unwrap(),
        1
    );
}

/// PCM16 export applies ±1 LSB TPDF dither, so each sample lands within one
/// LSB of plain rounding.
fn assert_pcm16_near(actual: i16, expected: f32, context: &str) {
    let rounded = (expected.clamp(-1.0, 1.0) * 32_767.0).round() as i32;
    assert!(
        (i32::from(actual) - rounded).abs() <= 1,
        "{context}: {actual} not within one lsb of {rounded}"
    );
}
