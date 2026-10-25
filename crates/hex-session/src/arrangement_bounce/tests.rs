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
    for inplace in [false, true] {
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
        let original = session.project().clone();
        let revision = session.compile_prepared_revision(None).unwrap();
        let expected = pcm(revision.clone(), 80_000);
        let (_, bounced) = session
            .bounce_arrangement(
                BounceTarget::Clips(vec![clip]),
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
        if inplace {
            assert!(
                session
                    .project()
                    .track(track)
                    .unwrap()
                    .instrument()
                    .is_none()
            );
            let actual = pcm(bounced, 80_000);
            let max = actual
                .iter()
                .flatten()
                .zip(expected.iter().flatten())
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f32, f32::max);
            assert!(max < 1e-5, "baked device host gains apply once: {max}");
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
            pcm(reopened.compile_prepared_revision(None).unwrap(), 80_000),
            pcm(session.compile_prepared_revision(None).unwrap(), 80_000)
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
    let folder = Folder::new();
    let (mut session, _, clip) = fixture();
    let original = session.project().clone();
    let resident = session.resident_frames;
    let control = hex_export::ExportControl::default();
    assert!(control.cancel());
    assert!(
        session
            .bounce_arrangement(
                BounceTarget::Clips(vec![clip]),
                true,
                &folder.0,
                &control,
                None
            )
            .is_err()
    );
    assert_eq!(session.project(), &original);
    let occupied = folder.0.join("occupied");
    fs::write(&occupied, b"original file").unwrap();
    assert!(
        session
            .bounce_arrangement(
                BounceTarget::Clips(vec![clip]),
                true,
                &occupied,
                &hex_export::ExportControl::default(),
                None
            )
            .is_err()
    );
    assert_eq!(fs::read(&occupied).unwrap(), b"original file");
    let target = BounceTarget::Clips(vec![clip]);
    let mut files_seen = Vec::new();
    let result: Result<(), String> = session.bounce_arrangement_using(
        target,
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
        .plan_arrangement_bounce(&BounceTarget::Clips(vec![clip]), true)
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
        BounceTarget::Clips(vec![clip]),
        true,
        vec![BounceFile {
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
    let (mut session, track, clip) = fixture_at_rate(rate);
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
            BounceTarget::Clips(vec![clip]),
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
