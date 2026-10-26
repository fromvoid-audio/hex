use super::*;
use hex_project::{BeatTime, ClipId, TrackId};
struct Folder(PathBuf);
impl Folder {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "hex-arrangement-bounce-{}-{}",
            std::process::id(),
            NEXT_JOB.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}
impl Drop for Folder {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn fixture() -> (Session, TrackId, ClipId) {
    fixture_at_rate(48_000)
}
fn fixture_at_rate(rate: u32) -> (Session, TrackId, ClipId) {
    let mut session = Session::new(rate, 32, 2_000_000).unwrap();
    let Created::Track(track) = session
        .command(Command::InsertTrackWithDevice {
            name: "source".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!()
    };
    let Created::Clip(clip) = session
        .command(Command::PlaceMidiClip {
            track,
            start: BeatTime::ONE,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!()
    };
    session
        .command(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 110,
        })
        .unwrap();
    (session, track, clip)
}
fn pcm(revision: Arc<hex_engine::PreparedRevision>, frames: usize) -> Vec<hex_engine::Frame> {
    let mut runtime = hex_engine::SessionPlayback::new(revision).unwrap();
    runtime
        .apply(hex_engine::SessionPlaybackCommand::Play)
        .unwrap();
    let mut samples = vec![[0.0; 2]; frames];
    for chunk in samples.chunks_mut(137) {
        runtime.render(chunk).unwrap();
    }
    samples
}
#[test]
fn selected_bounce_keeps_actual_release_pcm_and_one_history_step_without_double_processing() {
    for (inplace, whole_track) in [(false, false), (true, false), (true, true)] {
        let folder = Folder::new();
        let (mut session, track, clip) = fixture();
        session.command(Command::Gain { track, gain: 0.7 }).unwrap();
        let effect = hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Effect,
            hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
        )
        .unwrap();
        let effect = hex_engine::patch_native_device_parameter_for_class(
            hex_engine::NativeDeviceClass::Effect,
            &effect,
            hex_lattice_contract::ParameterId::from_static(11),
            hex_lattice_contract::ParameterValue::Bool(true),
        )
        .unwrap();
        session
            .command(Command::InsertDevice {
                track,
                before: None,
                state: effect,
            })
            .unwrap();
        let instrument = session
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap();
        session
            .command(Command::ReplaceDeviceState {
                device: instrument.id(),
                state: instrument
                    .state()
                    .clone()
                    .with_input_gain_db(-6.0)
                    .unwrap()
                    .with_output_gain_db(24.0)
                    .unwrap(),
            })
            .unwrap();
        let total_frames = if whole_track { 180_000 } else { 80_000 };
        if whole_track {
            let Created::Clip(second) = session
                .command(Command::PlaceMidiClip {
                    track,
                    start: BeatTime::from_raw(3 << 32),
                    length: BeatTime::ONE,
                })
                .unwrap()
            else {
                panic!("second midi clip")
            };
            session
                .command(Command::AddMidiNote {
                    clip: second,
                    start: BeatTime::ZERO,
                    duration: BeatTime::ONE,
                    pitch: 67,
                    channel: 0,
                    velocity: 91,
                })
                .unwrap();
            let path = folder.0.join("original.wav");
            grouping::write_fixture(&path, 48_000, &[[0.2, -0.1]; 64]);
            let prepared = PreparedImport::new(
                hex_media::decode_wav_file(&path, hex_media::Limits::new(10_000, 1000)).unwrap(),
            )
            .unwrap();
            session
                .import_prepared_audio_clip_revision(prepared, track, 144_000, None)
                .unwrap();
        }
        let original = session.project().clone();
        let revision = session.compile_prepared_revision(None).unwrap();
        let expected = pcm(revision.clone(), total_frames);
        let (_, bounced) = session
            .bounce_arrangement(
                if whole_track {
                    BounceTarget::Track(track)
                } else {
                    BounceTarget::Clips(vec![clip])
                },
                inplace,
                &folder.0,
                &hex_export::ExportControl::default(),
                Some(&revision),
            )
            .unwrap();
        let destination = if inplace {
            track
        } else {
            session.project().tracks()[1].id()
        };
        let audio = &session.project().track(destination).unwrap().clips()[0];
        assert!(
            audio.duration() > 24_000,
            "clip release must extend past original end"
        );
        let samples = session.audio(audio.asset()).unwrap().frames();
        assert!(
            samples.iter().flatten().any(|s| s.abs() > 1.0),
            "float PCM retains headroom above full scale"
        );
        assert!(
            samples[24_000..].iter().flatten().any(|s| s.abs() > 1e-6),
            "tail contains actual audio"
        );
        if whole_track {
            assert_eq!(audio.start(), 24_000);
            assert!(
                audio.start() + audio.duration() >= 144_064,
                "all material and gaps are covered"
            );
            assert!(
                session
                    .project()
                    .track(track)
                    .unwrap()
                    .midi_clips()
                    .is_empty()
            );
            assert!(session.project().track(track).unwrap().devices().is_empty());
            assert_eq!(
                fs::read(folder.0.join("original.wav")).unwrap().len(),
                44 + 64 * 8
            );
        }
        if inplace {
            assert!(
                session
                    .project()
                    .track(track)
                    .unwrap()
                    .instrument()
                    .is_none()
            );
            let actual = pcm(bounced, total_frames);
            let retained_end = usize::try_from(audio.start() + audio.duration()).unwrap();
            assert!(
                retained_end < expected.len(),
                "fixture exercises trimmed residuals"
            );
            let max = actual[..retained_end]
                .iter()
                .flatten()
                .zip(expected[..retained_end].iter().flatten())
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f32, f32::max);
            assert!(max < 1e-5, "baked device host gains apply once: {max}");
            assert!(
                actual[retained_end..]
                    .iter()
                    .flatten()
                    .all(|sample| *sample == 0.0)
            );
            // The original track's 0.7 mixer gain remains live. Only residuals
            // at/below the bounce's -80 dBFS pre-mixer threshold may disappear.
            assert!(
                expected[retained_end..]
                    .iter()
                    .flatten()
                    .all(|sample| sample.abs() <= 0.7 * 0.0001)
            );
        } else {
            assert!(session.project().midi_clip(clip).is_some());
        }
        let saved = folder.0.join("project.hex");
        hex_storage::save_new(&saved, session.project()).unwrap();
        let loaded = hex_storage::load(&saved).unwrap();
        assert_eq!(loaded.snapshot(), session.project().snapshot());
        let decoded = loaded
            .assets()
            .map(|a| {
                (
                    a.id(),
                    hex_media::decode_wav_file(
                        a.path(),
                        hex_media::Limits::new(64 * 1024 * 1024 + 44, 8_000_000),
                    )
                    .unwrap(),
                )
            })
            .collect();
        let reopened = Session::from_loaded(loaded, 32, 2_000_000, decoded).unwrap();
        assert_eq!(
            pcm(
                reopened.compile_prepared_revision(None).unwrap(),
                total_frames
            ),
            pcm(
                session.compile_prepared_revision(None).unwrap(),
                total_frames
            )
        );
        assert_eq!(
            session.editor.next_history_requires_stopped_bounce(false),
            inplace
        );
        let after = session.project().clone();
        assert!(session.undo());
        assert_eq!(
            session.project().snapshot().tracks,
            original.snapshot().tracks
        );
        assert_eq!(
            session.editor.next_history_requires_stopped_bounce(true),
            inplace
        );
        assert!(session.redo());
        assert_eq!(session.project().snapshot().tracks, after.snapshot().tracks);
    }
}
#[test]
fn exact_range_retains_prerange_instrument_state_and_stops_without_tail() {
    let folder = Folder::new();
    let (mut session, track, _) = fixture();
    let revision = session.compile_prepared_revision(None).unwrap();
    let expected = pcm(revision.clone(), 70_000);
    let start = 30_013;
    let end = 50_007;
    session
        .bounce_arrangement(
            BounceTarget::Range(hex_project::TimeRange::new(vec![track], start, end).unwrap()),
            false,
            &folder.0,
            &hex_export::ExportControl::default(),
            Some(&revision),
        )
        .unwrap();
    let audio = &session.project().tracks()[1].clips()[0];
    assert_eq!(audio.start(), start);
    assert_eq!(audio.duration(), end - start);
    let actual = session.audio(audio.asset()).unwrap().frames();
    let max = actual
        .iter()
        .flatten()
        .zip(expected[start as usize..end as usize].iter().flatten())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0_f32, f32::max);
    assert!(
        max < 1e-5,
        "range uses causal preroll instead of retriggering: {max}"
    );
    assert!(
        session
            .project()
            .plan_arrangement_bounce(
                &BounceTarget::Range(hex_project::TimeRange::new(vec![track], start, end).unwrap()),
                true
            )
            .is_err(),
        "partial in-place bake must not alter unrelated source"
    );
}
#[test]
fn failures_cancellation_and_second_output_preserve_model_history_media_and_original_files() {
    for whole_track in [false, true] {
        bounce_failure_case(whole_track);
    }
}

fn bounce_failure_case(whole_track: bool) {
    let folder = Folder::new();
    let (mut session, track, clip) = fixture();
    let target = if whole_track {
        BounceTarget::Track(track)
    } else {
        BounceTarget::Clips(vec![clip])
    };
    let original = session.project().clone();
    let resident = session.resident_frames;
    let control = hex_export::ExportControl::default();
    assert!(control.cancel());
    assert!(
        session
            .bounce_arrangement(target.clone(), true, &folder.0, &control, None)
            .is_err()
    );
    assert_eq!(session.project(), &original);
    let occupied = folder.0.join("occupied");
    fs::write(&occupied, b"original file").unwrap();
    assert!(
        session
            .bounce_arrangement(
                target.clone(),
                true,
                &occupied,
                &hex_export::ExportControl::default(),
                None
            )
            .is_err()
    );
    assert_eq!(fs::read(&occupied).unwrap(), b"original file");

    let mut files_seen = Vec::new();
    let result: Result<(), String> = session.bounce_arrangement_using(
        target.clone(),
        true,
        &folder.0,
        &hex_export::ExportControl::default(),
        |_, _, _| Ok(()),
        |path, _, item, _, _| {
            fs::write(path, b"not a wav").unwrap();
            files_seen.push(path.to_path_buf());
            Ok(item.end - item.start)
        },
        |_, _, _| panic!("decode failure must not commit"),
    );
    assert!(result.is_err());
    assert!(files_seen.iter().all(|p| !p.exists()));
    assert_eq!(session.project(), &original);
    assert_eq!(session.resident_frames, resident);
    let item = session
        .project()
        .plan_arrangement_bounce(&target, true)
        .unwrap()
        .remove(0);
    let path = folder.0.join("prepared.wav");
    // Use the durable real renderer for the final-preparation failure boundary.
    let revision = session.compile_prepared_revision(None).unwrap();
    hex_export::render_bounce_new(&path, revision, item.start, item.end - item.start, 1.0).unwrap();
    let prepared = PreparedImport::new(
        hex_media::decode_wav_file(&path, hex_media::Limits::new(1_000_000, 100_000)).unwrap(),
    )
    .unwrap();
    let result = session.adopt_arrangement_bounce_using(
        target.clone(),
        true,
        vec![BounceFile {
            source: hex_project::BounceSource::Arrangement,
            track: item.track,
            start: item.start,
            frames: item.end - item.start,
            path,
        }],
        vec![prepared],
        None,
        |_| Err::<(), _>(SessionError::Invalid("publication refused")),
    );
    assert!(result.is_err());
    assert_eq!(session.project(), &original);
    assert_eq!(session.resident_frames, resident);
}

#[test]
fn multi_track_failure_and_cancellation_after_render_preserve_the_entire_transaction() {
    let folder = Folder::new();
    let (mut session, _, first) = fixture();
    let Created::Track(second_track) = session
        .command(Command::InsertTrackWithDevice {
            name: "second".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!()
    };
    let Created::Clip(second) = session
        .command(Command::PlaceMidiClip {
            track: second_track,
            start: BeatTime::ONE,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!()
    };
    let original = session.project().clone();
    let resident = session.resident_frames;
    for cancel in [false, true] {
        let control = hex_export::ExportControl::default();
        let mut rendered = Vec::new();
        let result: Result<(), String> = session.bounce_arrangement_using(
            BounceTarget::Clips(vec![first, second]),
            false,
            &folder.0,
            &control,
            |session, project, _| {
                let waveforms = std::cell::RefCell::new(
                    session.audio_clip_waveforms().map_err(|e| e.to_string())?,
                );
                crate::prepare_with_waveforms(
                    project,
                    &session.media_bank(),
                    session.sine_table(),
                    None,
                    &session.audio_transforms,
                    &waveforms,
                )
                .map(Arc::new)
                .map_err(|e| e.to_string())
            },
            |path, revision, item, tails, control| {
                if !rendered.is_empty() {
                    return Err("second output failed".into());
                }
                let report = hex_export::render_arrangement_bounce_new(
                    path,
                    revision,
                    None,
                    item.start,
                    item.end - item.start,
                    tails,
                    control,
                )
                .map_err(|e| e.to_string())?;
                rendered.push(path.to_owned());
                if cancel {
                    assert!(control.cancel());
                }
                Ok(report.frames)
            },
            |_, _, _| panic!("partial render must not commit"),
        );
        assert!(result.is_err());
        assert_eq!(rendered.len(), 1);
        assert!(!rendered[0].exists());
        assert_eq!(session.project(), &original);
        assert_eq!(session.resident_frames, resident);
    }
}

#[test]
fn selected_source_isolation_excludes_other_clips_tracks_main_and_returns() {
    let folder = Folder::new();
    let (mut session, track, clip) = fixture();
    let expected = pcm(session.compile_prepared_revision(None).unwrap(), 80_000);
    let Created::Clip(other) = session
        .command(Command::PlaceMidiClip {
            track,
            start: BeatTime::from_quarters(3).unwrap(),
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!()
    };
    session
        .command(Command::AddMidiNote {
            clip: other,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 72,
            velocity: 127,
            channel: 0,
        })
        .unwrap();
    let utility = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap()
    .with_output_gain_db(12.0)
    .unwrap();
    session
        .command(Command::InsertMainDevice {
            before: None,
            state: utility,
        })
        .unwrap();
    let Created::ReturnTrack(bus) = session
        .command(Command::AddReturnTrack {
            name: "return".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    session
        .command(Command::SetReturnSend {
            track,
            return_track: bus,
            gain: 1.0,
        })
        .unwrap();
    session
        .command(Command::AddTrack {
            name: "unrelated".into(),
        })
        .unwrap();
    let target = BounceTarget::Clips(vec![clip]);
    let item = session
        .project()
        .plan_arrangement_bounce(&target, false)
        .unwrap()
        .remove(0);
    let source = isolated(session.project(), &target, &item).unwrap();
    assert_eq!(source.tracks().len(), 1);
    assert!(source.main_devices().is_empty());
    assert!(source.return_tracks().is_empty());
    assert!(source.tracks()[0].sends().is_empty());
    assert_eq!(source.tracks()[0].midi_clips().len(), 1);
    assert!(source.midi_clip(other).is_none());
    let before = session.project().clone();
    session
        .bounce_arrangement(
            target,
            false,
            &folder.0,
            &hex_export::ExportControl::default(),
            None,
        )
        .unwrap();
    assert_eq!(
        session.project().track(track).unwrap(),
        before.track(track).unwrap()
    );
    let audio = &session.project().tracks()[1].clips()[0];
    let samples = session.audio(audio.asset()).unwrap().frames();
    let expected = &expected[audio.start() as usize..(audio.start() + audio.duration()) as usize];
    let maximum = samples
        .iter()
        .flatten()
        .zip(expected.iter().flatten())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0_f32, f32::max);
    assert!(
        maximum < 1e-5,
        "unrelated track/bus processing is not baked: {maximum}"
    );
    assert!(
        audio.start() + audio.duration() < 72_000,
        "unselected later note is excluded"
    );
}

#[test]
fn directed_audio_and_midi_bounces_preserve_accepted_content_pcm() {
    for midi in [false, true] {
        for range in [false, true] {
            let folder = Folder::new();
            let (mut session, track, clip) = if midi {
                let (mut session, track, clip) = fixture();
                session
                    .command(Command::AddMidiNote {
                        clip,
                        start: BeatTime::ZERO,
                        duration: BeatTime::from_ratio(1, 4).unwrap(),
                        pitch: 67,
                        channel: 0,
                        velocity: 83,
                    })
                    .unwrap();
                (session, track, clip)
            } else {
                let mut session = Session::new(48_000, 32, 2_000_000).unwrap();
                let audio = Audio::new(
                    48_000,
                    (0..24_000)
                        .map(|i| {
                            let x = (i as f32 * 0.173).sin() * (i as f32 / 48_000.0);
                            [x, -x]
                        })
                        .collect(),
                )
                .unwrap();
                let prepared = PreparedImport {
                    path: folder.0.join("source.wav"),
                    source_sample_rate: 48_000,
                    source_frames: 24_000,
                    conversion_version: hex_project::IDENTITY_CONVERSION,
                    waveform: waveform::summarize(&audio, 1_024).unwrap(),
                    source_audio: audio.clone(),
                    audio,
                };
                let Created::Imported { track, clip, .. } = session.import(prepared).unwrap()
                else {
                    panic!("import")
                };
                (session, track, clip)
            };
            let forward = pcm(session.compile_prepared_revision(None).unwrap(), 80_000);
            session.command(Command::ReverseClips(vec![clip])).unwrap();
            let revision = session.compile_prepared_revision(None).unwrap();
            let expected = pcm(revision.clone(), 80_000);
            assert_ne!(expected, forward, "asymmetric fixture exercises reflection");
            let target = if range {
                let start = if midi { 30_013 } else { 1_013 };
                BounceTarget::Range(
                    hex_project::TimeRange::new(vec![track], start, start + 10_007).unwrap(),
                )
            } else {
                BounceTarget::Clips(vec![clip])
            };
            let (_, bounced) = session
                .bounce_arrangement(
                    target,
                    !range,
                    &folder.0,
                    &hex_export::ExportControl::default(),
                    Some(&revision),
                )
                .unwrap();
            let (actual, expected) = if range {
                let audio = &session.project().tracks()[1].clips()[0];
                (
                    session.audio(audio.asset()).unwrap().frames().to_vec(),
                    expected[audio.start() as usize..(audio.start() + audio.duration()) as usize]
                        .to_vec(),
                )
            } else {
                (pcm(bounced, 80_000), expected)
            };
            let error = actual
                .iter()
                .flatten()
                .zip(expected.iter().flatten())
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f32, f32::max);
            assert!(error < 1e-5, "directed source survives bounce: {error}");
        }
    }
}

#[path = "tests/grouping.rs"]
mod grouping;

#[test]
fn clipped_thirty_second_delay_tail_adopts_media_and_undo_redo_as_one_edit() {
    use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
    let folder = Folder::new();
    let rate = 8_000;
    let (mut session, track, _clip) = fixture_at_rate(rate);
    let class = hex_engine::NativeDeviceClass::Effect;
    let mut state = hex_engine::default_native_device_state_for_class(
        class,
        DeviceTypeId::from_static("hex.delay"),
    )
    .unwrap();
    for (id, value) in [
        (1, ParameterValue::Float(1000.0)),
        (2, ParameterValue::Float(0.95)),
        (3, ParameterValue::Float(0.5)),
        (5, ParameterValue::Bool(false)),
        (6, ParameterValue::Bool(false)),
        (15, ParameterValue::Bool(false)),
    ] {
        state = hex_engine::patch_native_device_parameter_for_class(
            class,
            &state,
            ParameterId::from_static(id),
            value,
        )
        .unwrap();
    }
    session
        .command(Command::InsertDevice {
            track,
            before: None,
            state,
        })
        .unwrap();
    let original = session.project().snapshot();
    let revision = session.compile_prepared_revision(None).unwrap();
    session
        .bounce_arrangement(
            BounceTarget::Track(track),
            true,
            &folder.0,
            &hex_export::ExportControl::default(),
            Some(&revision),
        )
        .unwrap();
    let bounced = session.project().snapshot();
    let audio = &session.project().track(track).unwrap().clips()[0];
    let expected_frames = u64::from(rate) / 2 + u64::from(rate) * 30;
    assert_eq!(audio.duration(), expected_frames);
    let samples = session.audio(audio.asset()).unwrap().frames();
    assert_eq!(samples.len() as u64, expected_frames);
    assert!(
        samples[samples.len() - 256..]
            .iter()
            .flatten()
            .any(|s| s.abs() > 1e-6),
        "delay is still sounding at the cutoff"
    );
    let project_path = folder.0.join("capped.hex");
    hex_storage::save_new(&project_path, session.project()).unwrap();
    assert_eq!(
        hex_storage::load(&project_path).unwrap().snapshot(),
        bounced
    );
    assert!(session.undo());
    assert_eq!(session.project().snapshot().tracks, original.tracks);
    assert!(session.redo());
    assert_eq!(session.project().snapshot().tracks, bounced.tracks);
}

fn console_fixture(folder: &Folder, arrangement: bool) -> (Session, TrackId, Vec<ClipId>) {
    let (mut session, track, midi) = fixture_at_rate(8_000);
    session.command(Command::Gain { track, gain: 0.7 }).unwrap();
    let effect = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap()
    .with_output_gain_db(6.0)
    .unwrap();
    session
        .command(Command::InsertDevice {
            track,
            before: None,
            state: effect.clone(),
        })
        .unwrap();
    session
        .command(Command::InsertMainDevice {
            before: None,
            state: effect.with_output_gain_db(-6.0).unwrap(),
        })
        .unwrap();
    let Created::Scene(scene) = session
        .command(Command::AddScene {
            name: "midi scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(notes) = session
        .command(Command::CopyArrangementMidiToSession {
            source: midi,
            scene,
        })
        .unwrap()
    else {
        panic!("cell")
    };
    session
        .command(Command::SetSessionMidiClipSource {
            clip: notes,
            source_length: BeatTime::ONE,
            source_start: BeatTime::ZERO,
            loop_start: BeatTime::ZERO,
            loop_end: BeatTime::ONE,
            looping: arrangement,
        })
        .unwrap();
    session
        .command(Command::RenameClip {
            clip: notes,
            name: "CaseSensitive MIDI".into(),
        })
        .unwrap();
    if !arrangement {
        session
            .command(Command::SetClipActive {
                clip: notes,
                active: false,
            })
            .unwrap();
    }
    let source = folder.0.join("original-console.wav");
    let samples: Vec<_> = (0..64)
        .map(|i| [i as f32 / 256.0, -(i as f32) / 128.0])
        .collect();
    grouping::write_fixture(&source, 8_000, &samples);
    let decoded = hex_media::decode_wav_file(&source, hex_media::Limits::new(10000, 1000)).unwrap();
    let (created, _) = session
        .import_prepared_audio_clip_revision(
            PreparedImport::new(decoded).unwrap(),
            track,
            12_000,
            None,
        )
        .unwrap();
    let Created::Imported { clip: audio, .. } = created else {
        panic!("audio")
    };
    let mut cells = vec![notes];
    for looping in [false, true] {
        let Created::Scene(scene) = session
            .command(Command::AddScene {
                name: format!("audio {looping}"),
            })
            .unwrap()
        else {
            panic!("scene")
        };
        let Created::SessionClip(id) = session
            .command(Command::CopyArrangementAudioClipToSession {
                source: audio,
                scene,
            })
            .unwrap()
        else {
            panic!("cell")
        };
        session
            .command(Command::SetAudioClipGain {
                clip: id,
                millidb: -3000,
            })
            .unwrap();
        if looping {
            session
                .command(Command::SetSessionAudioClipLoop {
                    clip: id,
                    loop_range: Some(hex_project::SessionAudioLoop::new(10, 30).unwrap()),
                })
                .unwrap();
            session
                .command(Command::SetClipActive {
                    clip: id,
                    active: false,
                })
                .unwrap();
        }
        cells.push(id);
    }
    let Created::Scene(stop) = session
        .command(Command::AddScene {
            name: "stop".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    session
        .command(Command::SetSessionStopCell { track, scene: stop })
        .unwrap();
    if !arrangement {
        session.command(Command::RemoveMidiClip(midi)).unwrap();
        session.command(Command::RemoveClip(audio)).unwrap();
    }
    (session, track, cells)
}
fn console_pcm(session: &Session, clip: ClipId, frames: usize) -> Vec<hex_engine::Frame> {
    let mut snapshot = session.project().snapshot();
    for cell in &mut snapshot.session.cells {
        match &mut cell.content {
            hex_project::SessionCellContentSnapshot::Midi(c) if c.id == clip.value() => {
                c.active = true
            }
            hex_project::SessionCellContentSnapshot::Audio(c) if c.id == clip.value() => {
                c.active = true
            }
            _ => {}
        }
    }
    let project = Project::from_snapshot(snapshot).unwrap();
    let waveforms = std::cell::RefCell::new(session.audio_clip_waveforms().unwrap());
    let revision = Arc::new(
        crate::prepare_with_waveforms(
            &project,
            &session.media_bank(),
            session.sine_table(),
            None,
            &session.audio_transforms,
            &waveforms,
        )
        .unwrap(),
    );
    let mut runtime = hex_engine::SessionPlayback::new(revision).unwrap();
    runtime
        .apply(hex_engine::SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: hex_engine::LaunchQuantization::Immediate,
        })
        .unwrap();
    runtime
        .apply(hex_engine::SessionPlaybackCommand::Play)
        .unwrap();
    let mut samples = vec![[0.0; 2]; frames];
    for chunk in samples.chunks_mut(137) {
        runtime.render(chunk).unwrap();
    }
    samples
}
#[test]
fn whole_track_bounce_replaces_every_console_source_keeps_loops_cells_and_atomic_history() {
    for arrangement in [false, true] {
        let folder = Folder::new();
        let (mut session, track, cells) = console_fixture(&folder, arrangement);
        let original = session.project().clone();
        let original_bytes = fs::read(folder.0.join("original-console.wav")).unwrap();
        let plan = original
            .plan_arrangement_bounce(&BounceTarget::Track(track), true)
            .unwrap();
        assert_eq!(plan.len(), 3 + usize::from(arrangement));
        let expected:Vec<_>=cells.iter().map(|id| {
            let item=plan.iter().find(|item|matches!(item.source,hex_project::BounceSource::Console{clip,..} if clip==*id)).unwrap();
            (*id,item.end as usize,console_pcm(&session,*id,item.end as usize))
        }).collect();
        let (_, revision) = session
            .bounce_arrangement(
                BounceTarget::Track(track),
                true,
                &folder.0,
                &hex_export::ExportControl::default(),
                None,
            )
            .unwrap();
        let owner = session.project().track(track).unwrap();
        assert!(owner.instrument().is_none() && owner.devices().is_empty());
        assert!(owner.midi_clips().is_empty());
        assert_eq!(owner.clips().len(), usize::from(arrangement));
        assert_eq!(owner.gain(), original.track(track).unwrap().gain());
        assert_eq!(session.project().scenes(), original.scenes());
        assert_eq!(session.project().main_devices(), original.main_devices());
        for (clip, frames, expected) in expected {
            let c = session.project().session_audio_clip(clip).unwrap();
            let before = original
                .session_cells()
                .iter()
                .find(|cell| match cell.content() {
                    hex_project::SessionCellContent::Midi(c) => c.id() == clip,
                    hex_project::SessionCellContent::Audio(c) => c.id() == clip,
                    _ => false,
                })
                .unwrap();
            match before.content() {
                hex_project::SessionCellContent::Midi(old) => {
                    assert_eq!(
                        (c.id(), c.name(), c.active(), c.flavor()),
                        (old.id(), old.name(), old.active(), old.flavor())
                    );
                    assert_eq!(
                        c.loop_range(),
                        if old.source().looping() {
                            Some(hex_project::SessionAudioLoop::new(4000, 8000).unwrap())
                        } else {
                            None
                        }
                    );
                }
                hex_project::SessionCellContent::Audio(old) => {
                    assert_eq!(
                        (c.id(), c.name(), c.active(), c.flavor()),
                        (old.id(), old.name(), old.active(), old.flavor())
                    );
                    if old.loop_range().is_some() {
                        assert_eq!(
                            c.loop_range(),
                            Some(hex_project::SessionAudioLoop::new(30, 50).unwrap())
                        );
                    } else {
                        assert!(c.loop_range().is_none());
                    }
                }
                _ => panic!("source"),
            }
            assert_eq!(c.gain(), hex_project::ClipGain::UNITY);
            assert!(c.source_end() >= frames as u64);
            let actual = console_pcm(&session, clip, frames);
            let maximum = actual
                .iter()
                .flatten()
                .zip(expected.iter().flatten())
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f32, f32::max);
            assert!(maximum < 1e-5, "cell {clip:?} differs by {maximum}");
            if let Some(range) = c.loop_range() {
                let actual = console_pcm(
                    &session,
                    clip,
                    (range.end() + 2 * (range.end() - range.start())) as usize,
                );
                let period = (range.end() - range.start()) as usize;
                assert_eq!(
                    &actual[range.start() as usize..range.end() as usize],
                    &actual[range.end() as usize..range.end() as usize + period]
                );
            }
        }
        assert_eq!(
            fs::read(folder.0.join("original-console.wav")).unwrap(),
            original_bytes
        );
        let after = session.project().clone();
        assert!(session.editor.next_history_requires_stopped_bounce(false));
        assert!(session.undo());
        let mut restored = session.project().snapshot();
        let initial = original.snapshot();
        assert!(
            restored.next_id >= initial.next_id,
            "history retains the allocator high-water mark"
        );
        restored.next_id = initial.next_id;
        assert_eq!(restored, initial);
        assert!(session.redo());
        assert_eq!(session.project(), &after);
        let saved = folder.0.join("console-bounce.hex");
        hex_storage::save_new(&saved, session.project()).unwrap();
        let loaded = hex_storage::load(&saved).unwrap();
        assert_eq!(loaded, after);
        let decoded = loaded
            .assets()
            .map(|a| {
                (
                    a.id(),
                    hex_media::decode_wav_file(
                        a.path(),
                        hex_media::Limits::new(64 * 1024 * 1024 + 44, 8_000_000),
                    )
                    .unwrap(),
                )
            })
            .collect();
        let reopened = Session::from_loaded(loaded, 32, 2_000_000, decoded).unwrap();
        for clip in cells {
            assert_eq!(
                console_pcm(&reopened, clip, 10_000),
                console_pcm(&session, clip, 10_000)
            );
        }
        assert_eq!(
            pcm(revision, 20_000),
            pcm(session.compile_prepared_revision(None).unwrap(), 20_000)
        );
    }
}

#[test]
fn whole_track_console_second_output_cancel_and_final_publication_fail_without_partial_changes() {
    for failure in 0..3 {
        let folder = Folder::new();
        let (mut session, track, _) = console_fixture(&folder, true);
        let original = session.project().clone();
        let resident = session.resident_frames;
        let media = session.media.len();
        let bytes = fs::read(folder.0.join("original-console.wav")).unwrap();
        let control = hex_export::ExportControl::default();
        let target = BounceTarget::Track(track);
        let mut paths = Vec::new();
        let result: Result<(Created, ()), String> = session.bounce_arrangement_using(
            target.clone(),
            true,
            &folder.0,
            &control,
            |session, project, _| {
                let waveforms = std::cell::RefCell::new(
                    session.audio_clip_waveforms().map_err(|e| e.to_string())?,
                );
                crate::prepare_with_waveforms(
                    project,
                    &session.media_bank(),
                    session.sine_table(),
                    None,
                    &session.audio_transforms,
                    &waveforms,
                )
                .map(Arc::new)
                .map_err(|e| e.to_string())
            },
            |path, revision, item, tails, control| {
                if failure == 0 && paths.len() == 1 {
                    return Err("second console output failed".into());
                }
                let frames = hex_export::render_arrangement_bounce_new(
                    path,
                    revision,
                    None,
                    item.start,
                    item.end - item.start,
                    tails,
                    control,
                )
                .map_err(|e| e.to_string())?
                .frames;
                paths.push(path.to_owned());
                if failure == 1 && paths.len() == 2 {
                    assert!(control.cancel());
                }
                Ok(frames)
            },
            |session, prepared, files| {
                session
                    .adopt_arrangement_bounce_using(target, true, files, prepared, None, |_| {
                        Err::<(), _>(SessionError::Invalid("final publication refused"))
                    })
                    .map_err(|e| e.to_string())
            },
        );
        assert!(result.is_err());
        assert_eq!(
            paths.len(),
            if failure == 0 {
                1
            } else if failure == 1 {
                2
            } else {
                4
            }
        );
        assert!(paths.iter().all(|p| !p.exists()));
        assert_eq!(session.project(), &original);
        assert_eq!(session.resident_frames, resident);
        assert_eq!(session.media.len(), media);
        assert_eq!(
            fs::read(folder.0.join("original-console.wav")).unwrap(),
            bytes
        );
    }
}
