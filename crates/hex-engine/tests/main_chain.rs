//! Main histories follow the same actual publication path as other native chains.
use hex_devices::utility::{UTILITY_DEFINITION, parameter};
use hex_engine::{Audio, PreparedRevision, SessionPlayback, SessionPlaybackCommand, SineTable};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{Command, Created, Editor, Project};
use std::{collections::BTreeMap, sync::Arc};
fn fixture(compensated: bool) -> (Editor, BTreeMap<hex_project::AssetId, Audio>) {
    let mut edit = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Asset(asset) = edit
        .execute(Command::AddAsset {
            path: "memory/main-history.wav".into(),
            sample_rate: 48_000,
            frames: 1024,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let Created::Track(track) = edit
        .execute(Command::AddTrack {
            name: "source".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    edit.execute(Command::PlaceClip {
        track,
        asset,
        start: 0,
        duration: 1024,
        source_offset: 0,
        source_length: 1024,
        looping: false,
    })
    .unwrap();
    if compensated {
        let Created::Track(destination) = edit
            .execute(Command::AddTrack {
                name: "routed".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        edit.execute(Command::SetAudioFeed {
            source: hex_project::RoutingOwner::Track(track),
            destination,
            gain: 0.25,
        })
        .unwrap();
    }
    let state = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            parameter::DC_REMOVE,
            ParameterValue::Bool(true),
        )
        .unwrap();
    edit.execute(Command::InsertMainDevice {
        before: None,
        state,
    })
    .unwrap();
    let audio = Audio::new(
        48_000,
        (0..1024)
            .map(|n| [0.35 + (n as f32 * 0.07).sin() * 0.1, -0.2])
            .collect(),
    )
    .unwrap();
    (edit, BTreeMap::from([(asset, audio)]))
}
fn revision(edit: &Editor, media: &BTreeMap<hex_project::AssetId, Audio>) -> Arc<PreparedRevision> {
    Arc::new(
        PreparedRevision::prepare(edit.project(), media, Arc::new(SineTable::new()), None).unwrap(),
    )
}
#[test]
fn warmed_main_filter_history_survives_normal_session_handoff_bitwise() {
    for compensated in [false, true] {
        let (edit, media) = fixture(compensated);
        let revision = revision(&edit, &media);
        let mut donor = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let mut oracle = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        for runtime in [&mut donor, &mut oracle] {
            runtime.apply(SessionPlaybackCommand::Play).unwrap();
            runtime.render(&mut [[0.0; 2]; 128]).unwrap();
        }
        let mut candidate = SessionPlayback::new(revision).unwrap();
        candidate.try_inherit_state_from(&mut donor).unwrap();
        let mut expected = [[0.0; 2]; 64];
        let mut actual = expected;
        oracle.render(&mut expected).unwrap();
        candidate.render(&mut actual).unwrap();
        assert_eq!(
            actual, expected,
            "Main history must survive active normal handoff, compensated={compensated}"
        );
    }
}
#[test]
fn stopped_compensated_fast_handoff_allows_authored_main_chain_edits() {
    let (mut edit, media) = fixture(true);
    let mut donor = SessionPlayback::new(revision(&edit, &media)).unwrap();
    edit.execute(Command::InsertMainDevice {
        before: None,
        state: UTILITY_DEFINITION.default_state().unwrap(),
    })
    .unwrap();
    let changed = revision(&edit, &media);
    let mut candidate = SessionPlayback::new(Arc::clone(&changed)).unwrap();
    candidate.try_inherit_state_from(&mut donor).unwrap();
    candidate.apply(SessionPlaybackCommand::Play).unwrap();
    let mut oracle = SessionPlayback::new(changed).unwrap();
    oracle.apply(SessionPlaybackCommand::Play).unwrap();
    let mut expected = [[0.0; 2]; 64];
    let mut actual = expected;
    oracle.render(&mut expected).unwrap();
    candidate.render(&mut actual).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn main_effects_count_against_the_combined_native_runtime_budget() {
    let (mut edit, media) = fixture(false);
    let filter = edit.project().main_devices()[0].state().clone();
    for _ in 1..64 {
        edit.execute(Command::InsertMainDevice {
            before: None,
            state: filter.clone(),
        })
        .unwrap();
    }
    let first = edit.project().tracks()[0].id();
    let mut tracks = vec![first];
    for _ in 0..2 {
        let Created::Track(track) = edit
            .execute(Command::AddTrack {
                name: "effects".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        tracks.push(track);
    }
    for track in tracks {
        for _ in 0..64 {
            edit.execute(Command::InsertDevice {
                track,
                before: None,
                state: filter.clone(),
            })
            .unwrap();
        }
    }
    assert_eq!(hex_lattice::MAX_STATEFUL_PROCESSORS, 256);
    assert!(
        PreparedRevision::prepare(edit.project(), &media, Arc::new(SineTable::new()), None).is_ok(),
        "exact256effect boundary must remain admitted"
    );
    let Created::Track(track) = edit
        .execute(Command::AddTrack {
            name: "mutable".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let state = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        hex_engine::DeviceTypeId::from_static("hex.instrument.operator"),
    )
    .unwrap();
    let state = hex_engine::patch_native_device_parameter_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        &state,
        hex_devices::operator_state::parameter::GLOBAL_MODE,
        ParameterValue::Choice(1),
    )
    .unwrap();
    edit.execute(Command::RemoveInstrument { track }).unwrap();
    edit.execute(Command::InsertInstrument { track, state })
        .unwrap();
    assert!(
        PreparedRevision::prepare(edit.project(), &media, Arc::new(SineTable::new()), None)
            .is_err(),
        "Main64 +Track192 +mutable1 must refuse257 before publication"
    );
    let last = edit.project().main_devices().last().unwrap().id();
    edit.execute(Command::RemoveDevice(last)).unwrap();
    assert!(
        PreparedRevision::prepare(edit.project(), &media, Arc::new(SineTable::new()), None).is_ok(),
        "Main63 +Track192 +mutable1 must remain admitted at256"
    );
}

#[test]
fn main_meter_resolves_the_actual_rendered_device_tap() {
    let (edit, media) = fixture(false);
    let device = edit.project().main_devices()[0].id();
    let prepared = revision(&edit, &media);
    let meter = Arc::clone(
        prepared
            .arrangement()
            .device_meter(device.value())
            .expect("Main device tap"),
    );
    let mut playback =
        hex_engine::playback::Playback::try_from_prepared_with_session(prepared).unwrap();
    playback.set_device_metering(true);
    playback.apply(hex_engine::PlaybackCommand::Play);
    playback.render(&mut [[0.0; 2]; 64]).unwrap();
    let peaks = meter.take_peaks();
    assert!(
        peaks[0] > 0.4 && peaks[0] < 0.46,
        "actual Main input: {peaks:?}"
    );
    assert!(
        peaks[1] > 0.3 && peaks[1] < peaks[0],
        "actual DC-filter output: {peaks:?}"
    );
    assert_eq!(meter.take_peaks(), [0.0; 2]);
}

#[test]
fn main_only_unavailable_session_cannot_adopt_an_unowned_fallback() {
    let (mut edit, media) = fixture(false);
    // No Track/Return effects or mutable instruments can mask Main's ownership.
    let track = edit.project().tracks()[0].id();
    edit.execute(Command::RemoveInstrument { track }).unwrap();
    edit.execute(Command::SetLoopRegion {
        start: u64::MAX - 1,
        end: u64::MAX,
        enabled: true,
    })
    .unwrap();
    let prepared = revision(&edit, &media);
    let error =
        match hex_engine::playback::Playback::try_from_prepared_with_session(Arc::clone(&prepared))
        {
            Err(error @ hex_engine::SessionPlaybackError::SessionTempoUnavailable(_)) => error,
            _ => panic!("actual unsupported periodic Session timing must refuse"),
        };
    let mut playback =
        hex_engine::playback::Playback::from_prepared_with_session_unavailable(prepared, error);
    playback.apply(hex_engine::PlaybackCommand::Play);
    let mut frames = [[0.125; 2]; 64];
    assert!(
        playback.render(&mut frames).is_err(),
        "Main cannot silently enter the unowned native fallback"
    );
    assert_eq!(frames, [[0.125; 2]; 64]);
    assert_eq!(playback.position(), 0);
    assert_eq!(playback.performance_frame(), 0);
}

#[test]
fn warmed_legacy_main_history_cannot_be_silently_captured_as_fresh() {
    let (edit, media) = fixture(false);
    let prepared = revision(&edit, &media);
    let mut previous =
        hex_engine::playback::Playback::from_shared(Arc::clone(prepared.arrangement()));
    previous.apply(hex_engine::PlaybackCommand::Play);
    previous.render(&mut [[0.0; 2]; 128]).unwrap();
    let mut candidate =
        hex_engine::playback::Playback::try_from_prepared_with_session(prepared).unwrap();
    assert_eq!(
        candidate.try_inherit_runtime_from(&mut previous),
        Err(hex_engine::playback::PlaybackHandoffError::ArrangementStateCannotBeCaptured)
    );
    assert_eq!(previous.position(), 128);
    assert_eq!(previous.performance_frame(), 128);
}

#[test]
fn launched_session_audio_passes_through_main_effect_and_master_once() {
    let (mut edit, media) = fixture(false);
    let track = edit.project().tracks()[0].id();
    let clip = edit.project().tracks()[0].clips()[0].id();
    let asset = *media.keys().next().unwrap();
    edit.execute(Command::RemoveClip(clip)).unwrap();
    let Created::Scene(scene) = edit
        .execute(Command::AddScene {
            name: "live".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(clip) = edit
        .execute(Command::CreateSessionAudioClip {
            track,
            scene,
            name: "live audio".into(),
            asset,
            source_start: 0,
            source_end: 1024,
        })
        .unwrap()
    else {
        panic!("session clip")
    };
    let device = edit.project().main_devices()[0].id();
    let state = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            parameter::INVERT_RIGHT,
            ParameterValue::Bool(true),
        )
        .unwrap();
    edit.execute(Command::ReplaceDeviceState { device, state })
        .unwrap();
    edit.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    let mut runtime = SessionPlayback::new(revision(&edit, &media)).unwrap();
    runtime
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: hex_engine::LaunchQuantization::Immediate,
        })
        .unwrap();
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    let mut frames = [[0.0; 2]; 64];
    runtime.render(&mut frames).unwrap();
    let original = media.get(&asset).unwrap();
    for (index, frame) in frames.iter().enumerate() {
        let expected_left = (0.35 + (index as f32 * 0.07).sin() * 0.1) * 0.5;
        assert_eq!(*frame, [expected_left, 0.1]);
    }
    assert_eq!(original.sample_rate(), 48_000);
}
