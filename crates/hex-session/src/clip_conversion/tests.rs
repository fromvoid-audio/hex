use super::*;
use hex_engine::{SessionPlayback, SessionPlaybackCommand};
use hex_project::{BeatTime, Command, Created, Tempo};

struct Folder(PathBuf);
impl Folder {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hex-bounce-test-{}-{}",
            std::process::id(),
            NEXT_BOUNCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Folder {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn beat(n: u64) -> BeatTime {
    BeatTime::from_raw(n << 32)
}
fn fixture() -> (Session, hex_project::TrackId, ClipId) {
    let mut session = Session::new(48_000, 32, 1_000_000).unwrap();
    let Created::Track(track) = session
        .command(Command::InsertTrackWithDevice {
            name: "bounce".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap()
    else {
        panic!()
    };
    let Created::Clip(clip) = session
        .command(Command::PlaceMidiClip {
            track,
            start: beat(1),
            length: beat(3),
        })
        .unwrap()
    else {
        panic!()
    };
    session
        .command(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: beat(1),
            pitch: 60,
            channel: 0,
            velocity: 110,
        })
        .unwrap();
    session
        .command(Command::SetTempoPoint {
            beat: beat(2),
            tempo: Tempo::new(90_000_000).unwrap(),
        })
        .unwrap();
    (session, track, clip)
}
fn pcm(revision: Arc<hex_engine::PreparedRevision>, frames: usize) -> Vec<hex_engine::Frame> {
    // The independent ordinary Arrangement owner is the source-sound oracle.
    let mut playback = SessionPlayback::new(revision).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut result = vec![[0.0; 2]; frames];
    for chunk in result.chunks_mut(137) {
        playback.render(chunk).unwrap();
    }
    result
}

#[test]
fn bounce_preserves_placement_sound_mix_history_and_saved_media() {
    for main_type in ["hex.utility", "hex.overdrive"] {
        bounce_with_main(main_type);
    }
}
fn bounce_with_main(main_type: &'static str) {
    let folder = Folder::new();
    let (mut session, track, clip) = fixture();
    session
        .command(Command::RenameClip {
            clip,
            name: "Rendered Chorus".into(),
        })
        .unwrap();
    // Non-neutral host gains must not be baked twice after conversion.
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
                .with_input_gain_db(-36.0)
                .unwrap()
                .with_output_gain_db(6.0)
                .unwrap(),
        })
        .unwrap();
    session.command(Command::Gain { track, gain: 0.7 }).unwrap();
    session
        .command(Command::Balance {
            track,
            balance: -0.3,
        })
        .unwrap();
    session.command(Command::MasterGain { gain: 0.6 }).unwrap();
    let effect = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap()
    .with_output_gain_db(-6.0)
    .unwrap();
    session
        .command(Command::InsertDevice {
            track,
            before: None,
            state: effect,
        })
        .unwrap();
    let main = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static(main_type),
    )
    .unwrap()
    .with_output_gain_db(-6.0)
    .unwrap();
    session
        .command(Command::InsertMainDevice {
            before: None,
            state: main,
        })
        .unwrap();
    let original = session.project().clone();
    let (start, frames) = original.midi_bounce_range(clip).unwrap();
    let before = session.compile_prepared_revision(None).unwrap();
    let expected = pcm(Arc::clone(&before), (start + frames) as usize);
    assert!(expected.iter().flatten().any(|sample| sample.abs() > 0.001));
    let (_, bounced) = session
        .bounce_midi_clip(clip, &folder.0, Some(&before))
        .unwrap();
    let audio = session.project().clip(clip).unwrap();
    assert_eq!(audio.name(), "Rendered Chorus");
    assert_eq!((audio.start(), audio.duration()), (start, frames));
    assert_eq!(
        session.project().track(track).unwrap().clips()[0].id(),
        clip
    );
    assert!(session.project().midi_clip(clip).is_none());
    assert_eq!(
        session.project().native_sources(),
        original.native_sources(),
        "render normalization must not change retained source state"
    );
    assert_eq!(session.project().main_devices(), original.main_devices());
    assert_eq!(
        session.project().track(track).unwrap().devices(),
        original.track(track).unwrap().devices()
    );
    let actual = pcm(Arc::clone(&bounced), expected.len());
    let error = actual
        .iter()
        .flatten()
        .zip(expected.iter().flatten())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0_f32, f32::max);
    assert!(error < 0.000_001, "bounce changed PCM: {error}");
    let path = session.project().asset(audio.asset()).unwrap().path();
    assert!(path.is_absolute() && path.is_file());
    assert_eq!(&fs::read(path).unwrap()[20..22], &3_u16.to_le_bytes());
    assert!(
        session
            .audio(audio.asset())
            .unwrap()
            .frames()
            .iter()
            .flatten()
            .any(|sample| sample.abs() > 1.0),
        "float bounce must preserve samples above full scale"
    );
    // Save/reopen must use generated media, not a transient resident-only source.
    let saved = folder.0.join("project.hex");
    hex_storage::save_new(&saved, session.project()).unwrap();
    let loaded = hex_storage::load(&saved).unwrap();
    let decoded = loaded
        .assets()
        .map(|asset| {
            (
                asset.id(),
                hex_media::decode_wav_file(
                    asset.path(),
                    hex_media::Limits::new(64 * 1024 * 1024 + 44, 8_000_000),
                )
                .unwrap(),
            )
        })
        .collect();
    let reopened = Session::from_loaded(loaded, 32, 1_000_000, decoded).unwrap();
    assert_eq!(
        pcm(
            reopened.compile_prepared_revision(None).unwrap(),
            expected.len()
        ),
        actual
    );
    session
        .undo_prepared_revision(Some(&bounced))
        .unwrap()
        .unwrap();
    let mut restored = session.project().snapshot();
    restored.next_id = original.snapshot().next_id;
    assert_eq!(restored, original.snapshot());
    session.redo_prepared_revision(None).unwrap().unwrap();
    assert!(session.project().clip(clip).is_some());
    let bounced_project = session.project().clone();
    let media_path = session.project().assets().next().unwrap().path().to_owned();
    let bytes = fs::read(&media_path).unwrap();
    let error = session
        .command(Command::ConvertAudioClipToMidi(clip))
        .unwrap_err();
    assert!(matches!(
        error,
        SessionError::Project(hex_project::Invalid(hex_project::AUDIO_TO_MIDI_UNSUPPORTED))
    ));
    assert_eq!(session.project(), &bounced_project);
    assert_eq!(fs::read(&media_path).unwrap(), bytes);
    assert!(session.undo());
    assert!(session.project().midi_clip(clip).is_some());
    assert!(session.redo());
    assert_eq!(session.project(), &bounced_project);
}

#[test]
fn refused_bounce_leaves_project_and_redo_and_files_untouched() {
    let folder = Folder::new();
    let (mut session, track, clip) = fixture();
    session.command(Command::Gain { track, gain: 0.8 }).unwrap();
    assert!(session.undo());
    let original = session.project().clone();
    let blocked = folder.0.join("file");
    fs::write(&blocked, b"preserve").unwrap();
    assert!(session.bounce_midi_clip(clip, &blocked, None).is_err());
    assert_eq!(session.project(), &original);
    assert!(session.can_redo());
    assert_eq!(fs::read(&blocked).unwrap(), b"preserve");
    // A successful create-new render followed by final admission refusal cleans only
    // that uncommitted generated file, retaining the original source and history.
    assert!(
        session
            .bounce_midi_clip_with_admission(clip, &folder.0, None, |_, _| Err(
                SessionError::Invalid("injected final bounce refusal")
            ),)
            .is_err()
    );
    assert_eq!(session.project(), &original);
    assert!(session.can_redo());
    assert_eq!(fs::read_dir(&folder.0).unwrap().count(), 1);
    assert_eq!(fs::read(&blocked).unwrap(), b"preserve");
    session.command(Command::RemoveMidiClip(clip)).unwrap();
    session.undo();
    session.redo();
    let original = session.project().clone();
    assert!(session.bounce_midi_clip(clip, &folder.0, None).is_err());
    assert_eq!(session.project(), &original);
}

#[test]
fn session_conversion_retains_cell_name_identity_and_source_window() {
    let folder = Folder::new();
    let (mut session, track, arrangement) = fixture();
    let Created::Scene(scene) = session
        .command(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    session
        .command(Command::CopyArrangementMidiToSession {
            source: arrangement,
            scene,
        })
        .unwrap();
    let SessionCellContent::Midi(source) = session.project().session_cell(track, scene).unwrap()
    else {
        panic!()
    };
    let clip = source.id();
    session
        .command(Command::RenameSessionMidiClip {
            clip,
            name: "phrase".into(),
        })
        .unwrap();
    session
        .command(Command::SetSessionMidiClipSource {
            clip,
            source_length: beat(3),
            source_start: beat(1),
            loop_start: beat(1),
            loop_end: beat(2),
            looping: true,
        })
        .unwrap();
    session
        .command(Command::AddMidiNote {
            clip,
            start: beat(1),
            duration: BeatTime::from_raw(1 << 31),
            pitch: 65,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let original = session.project().clone();
    let (_, frames) = original.midi_bounce_range(clip).unwrap();
    session.bounce_midi_clip(clip, &folder.0, None).unwrap();
    let audio = session.project().session_audio_clip(clip).unwrap();
    assert_eq!(audio.name(), "phrase");
    assert_eq!(audio.source_end(), frames);
    assert!(
        session
            .audio(audio.asset())
            .unwrap()
            .frames()
            .iter()
            .flatten()
            .any(|sample| sample.abs() > 0.001)
    );
    assert!(session.project().midi_clip(arrangement).is_some());
    let bounced_project = session.project().clone();
    assert!(matches!(
        session.command(Command::ConvertAudioClipToMidi(clip)),
        Err(SessionError::Project(hex_project::Invalid(
            hex_project::AUDIO_TO_MIDI_UNSUPPORTED
        )))
    ));
    assert_eq!(session.project(), &bounced_project);
    assert!(session.undo());
    let mut restored = session.project().snapshot();
    restored.next_id = original.snapshot().next_id;
    assert_eq!(restored, original.snapshot());
}
#[test]
fn looped_and_mutable_bounces_chase_history_and_isolate_other_clips() {
    for mutable in [false, true] {
        let folder = Folder::new();
        let (mut session, track, clip) = fixture();
        let instrument = session
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        if mutable {
            let state = hex_engine::default_native_device_state_for_class(
                hex_engine::NativeDeviceClass::Instrument,
                hex_lattice_contract::DeviceTypeId::from_static("hex.instrument.operator"),
            )
            .unwrap();
            let state = hex_engine::patch_native_device_parameter_for_class(
                hex_engine::NativeDeviceClass::Instrument,
                &state,
                hex_lattice_contract::ParameterId::from_static(78),
                hex_lattice_contract::ParameterValue::Float(0.4),
            )
            .unwrap();
            session
                .command(Command::ReplaceDeviceState {
                    device: instrument,
                    state,
                })
                .unwrap();
        }
        session
            .command(Command::SetMidiClipSource {
                clip,
                source_length: beat(1),
                source_start: BeatTime::ZERO,
                loop_start: BeatTime::ZERO,
                loop_end: beat(1),
                looping: true,
                length: beat(3),
            })
            .unwrap();
        let before = session.compile_prepared_revision(None).unwrap();
        let (start, frames) = session.project().midi_bounce_range(clip).unwrap();
        let expected = pcm(before, (start + frames) as usize);
        let Created::Track(other_track) = session
            .command(Command::InsertTrackWithDevice {
                name: "independent voice".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("other track")
        };
        let Created::Clip(other) = session
            .command(Command::PlaceMidiClip {
                track: other_track,
                start: beat(1),
                length: beat(3),
            })
            .unwrap()
        else {
            panic!()
        };
        session
            .command(Command::AddMidiNote {
                clip: other,
                start: BeatTime::ZERO,
                duration: beat(2),
                pitch: 72,
                channel: 0,
                velocity: 120,
            })
            .unwrap();
        session.bounce_midi_clip(clip, &folder.0, None).unwrap();
        let bounced = session.project().clip(clip).unwrap();
        let actual = session.audio(bounced.asset()).unwrap().frames();
        let error = actual
            .iter()
            .flatten()
            .zip(expected[start as usize..].iter().flatten())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0_f32, f32::max);
        assert!(
            error < 0.000_001,
            "mutable loop bounce changed PCM: {error}"
        );
        if !mutable {
            assert!(
                actual[frames as usize / 2..]
                    .iter()
                    .flatten()
                    .any(|sample| sample.abs() > 0.001)
            );
        }
        assert!(session.project().midi_clip(other).is_some());
    }
}

#[test]
fn effects_before_instrument_refuse_without_changing_source_or_redo() {
    let folder = Folder::new();
    let (mut session, track, clip) = fixture();
    let instrument = session
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let effect = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap();
    session
        .command(Command::InsertDevice {
            track,
            before: Some(instrument),
            state: effect,
        })
        .unwrap();
    let original = session.project().clone();
    assert!(
        session
            .bounce_midi_clip(clip, &folder.0, None)
            .unwrap_err()
            .contains("after the instrument")
    );
    assert_eq!(session.project(), &original);
    assert_eq!(fs::read_dir(&folder.0).unwrap().count(), 0);
}

#[test]
fn isolated_clip_source_omits_unrelated_retained_source_media_without_changing_live_state() {
    let (session, track, clip) = fixture();
    let mut editor = hex_project::Editor::new(session.project().clone(), 4).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "unassigned-original.wav".into(),
            sample_rate: 48_000,
            frames: 16,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let state = hex_project::default_instrument_state()
        .unwrap()
        .with_asset_references(vec![hex_lattice_contract::DeviceAssetReference {
            slot: hex_lattice_contract::DeviceAssetSlot::new(1).unwrap(),
            asset: hex_lattice_contract::DeviceAssetKey::new(asset.value()).unwrap(),
        }])
        .unwrap();
    editor
        .execute(Command::CreateNativeSource {
            name: "unassigned".into(),
            devices: vec![state],
            instrument_index: 0,
        })
        .unwrap();
    let original = editor.project().clone();
    let (isolated, owner, _) = isolated_source(&original, clip).unwrap();
    assert_eq!(owner, track);
    assert_eq!(isolated.native_sources().len(), 1);
    assert_eq!(
        isolated.track(track).unwrap().source_id(),
        original.track(track).unwrap().source_id()
    );
    assert!(isolated.asset(asset).is_none());
    assert_eq!(editor.project(), &original);
    assert!(editor.project().asset(asset).is_some());
    assert_eq!(editor.project().native_sources().len(), 2);
}

#[test]
fn isolated_conversion_admits_projects_with_automatic_names_on_dropped_tracks() {
    let (mut session, track, clip) = fixture();
    let Created::Track(other) = session
        .command(Command::InsertDefaultTrack { before_track: None })
        .unwrap()
    else {
        panic!("default track")
    };
    let original = session.project().clone();
    assert!(original.track(other).unwrap().name_is_automatic());
    let (isolated, owner, _) = isolated_source(&original, clip).unwrap();
    assert_eq!(owner, track);
    assert!(isolated.track(other).is_none());
    assert!(isolated.snapshot().automatic_track_names.is_empty());
    assert_eq!(session.project(), &original);
}
