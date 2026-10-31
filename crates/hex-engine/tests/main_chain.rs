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
        .execute(Command::InsertTrackWithDevice {
            name: "mutable".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_engine::default_native_device_state_for_class(
                hex_engine::NativeDeviceClass::Instrument,
                hex_engine::DeviceTypeId::from_static("hex.instrument.sine"),
            )
            .unwrap(),
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
        peaks.input > 0.4 && peaks.input < 0.46,
        "actual Main input: {peaks:?}"
    );
    let output = peaks.output[0].max(peaks.output[1]);
    assert!(
        output > 0.3 && output < peaks.input,
        "actual DC-filter output: {peaks:?}"
    );
    assert_eq!(meter.take_peaks(), hex_engine::DevicePeaks::default());
}

#[test]
fn main_only_unavailable_session_cannot_adopt_an_unowned_fallback() {
    let (mut edit, media) = fixture(false);
    // No Track/Return effects or mutable instruments can mask Main's ownership.
    let track = edit.project().tracks()[0].id();
    // Author the instrument this removal control previously received implicitly.
    // The shared audio fixture remains an ordinary empty track.
    edit.execute(Command::InsertInstrument {
        track,
        state: hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Instrument,
            hex_engine::DeviceTypeId::from_static("hex.instrument.sine"),
        )
        .unwrap(),
    })
    .unwrap();
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

#[test]
fn main_renderer_metering_tracks_public_enable_disable_and_reenable_without_pcm_changes() {
    for compensated in [false, true] {
        let (edit, media) = fixture(compensated);
        let device = edit.project().main_devices()[0].id();
        let prepared = revision(&edit, &media);
        let project = prepared.arrangement();
        let meter = project
            .device_meter(device.value())
            .expect("actual Main tap");
        let mut renderer = hex_engine::Renderer::new(project);
        let mut unmetered = hex_engine::Renderer::new(project);
        renderer.play();
        unmetered.play();
        for enabled in [false, true, false, true] {
            assert_eq!(meter.take_peaks(), hex_engine::DevicePeaks::default());
            renderer.set_effect_metering(enabled);
            let mut actual = [[0.0; 2]; 64];
            let mut expected = actual;
            renderer.render(&mut actual).unwrap();
            unmetered.render(&mut expected).unwrap();
            assert_eq!(actual, expected, "telemetry changes PCM, enabled={enabled}");
            assert!(actual.iter().any(|f| f[0].abs() > 0.01));
            let peaks = meter.take_peaks();
            eprintln!(
                "MAIN_RENDERER_METER compensated={compensated} enabled={enabled} peaks={peaks:?}"
            );
            if enabled {
                assert!(
                    peaks.input > 0.01 && peaks.output.iter().any(|peak| *peak > 0.01),
                    "public Renderer enable must meter nonzero Main PCM: {peaks:?}"
                );
            } else {
                assert_eq!(
                    peaks,
                    hex_engine::DevicePeaks::default(),
                    "disabled Main telemetry must stay silent"
                );
            }
            assert_eq!(meter.take_peaks(), hex_engine::DevicePeaks::default());
            assert_eq!(renderer.position(), unmetered.position());
        }
    }
}

#[test]
fn removing_warmed_main_preserves_zero_input_residual_once() {
    for compensated in [false, true] {
        for action in ["remove", "bypass", "replace"] {
            assert_main_residual(compensated, action);
        }
    }
}
fn assert_main_residual(compensated: bool, action: &str) {
    let (mut edit, mut media) = fixture(compensated);
    // Past warmup, canonical input really is zero; only the old Main has PCM.
    for audio in media.values_mut() {
        let mut frames = vec![[0.0; 2]; 1024];
        frames[..128].fill([0.25, -0.125]);
        *audio = Audio::new(48_000, frames).unwrap();
    }
    edit.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    let prepared = revision(&edit, &media);
    let mut donor = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    let mut oracle = SessionPlayback::new(prepared).unwrap();
    for runtime in [&mut donor, &mut oracle] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        runtime.render(&mut [[0.0; 2]; 128]).unwrap();
    }
    let device = edit.project().main_devices()[0].id();
    match action {
        "bypass" => {
            edit.execute(Command::SetDeviceBypass {
                device,
                bypassed: true,
            })
            .unwrap();
        }
        "replace" => {
            edit.execute(Command::RemoveDevice(device)).unwrap();
            let state = UTILITY_DEFINITION
                .patch_parameter(
                    &UTILITY_DEFINITION.default_state().unwrap(),
                    parameter::INVERT_RIGHT,
                    ParameterValue::Bool(true),
                )
                .unwrap();
            edit.execute(Command::InsertMainDevice {
                before: None,
                state,
            })
            .unwrap();
        }
        _ => {
            edit.execute(Command::RemoveDevice(device)).unwrap();
        }
    }
    let mut candidate = SessionPlayback::new(revision(&edit, &media)).unwrap();
    candidate.try_inherit_state_from(&mut donor).unwrap();
    let track = edit.project().tracks()[0].id();
    let plan = candidate
        .output_capture_plan(&[hex_engine::OutputCaptureId::Track(track)], 64)
        .unwrap();
    let mut expected = [[0.0; 2]; 64];
    let mut actual = expected;
    oracle.render(&mut expected).unwrap();
    assert!(
        expected.iter().flatten().any(|sample| sample.abs() > 0.001),
        "real residual"
    );
    let mut tap = [[9.0; 2]; 64];
    let mut buffers: [&mut [[f32; 2]]; 1] = [&mut tap];
    let mut capture = hex_engine::OutputCapture::new(&plan, &mut buffers);
    candidate
        .render_with_capture(&mut actual, &mut capture)
        .unwrap();
    assert_eq!(
        actual, expected,
        "post-current-Main, premaster exactly once: {action}/{compensated}"
    );
    assert_eq!(
        capture.output(0).unwrap(),
        &[[0.0; 2]; 64],
        "old Main must not pollute pre-Main captures"
    );
    candidate.apply(SessionPlaybackCommand::Stop).unwrap();
    candidate.apply(SessionPlaybackCommand::Seek(512)).unwrap();
    candidate.apply(SessionPlaybackCommand::Play).unwrap();
    candidate.render(&mut actual).unwrap();
    assert_eq!(actual, [[0.0; 2]; 64], "stop/seek retires the old residual");
}

#[test]
fn repeated_main_detachment_exhausts_shared_bank_atomically() {
    let (mut edit, media) = fixture(false);
    let state = edit.project().main_devices()[0].state().clone();
    let mut donor = SessionPlayback::new(revision(&edit, &media)).unwrap();
    donor.apply(SessionPlaybackCommand::Play).unwrap();
    donor.render(&mut [[0.0; 2]; 1]).unwrap();
    for index in 0..=64 {
        let id = edit.project().main_devices()[0].id();
        edit.execute(Command::RemoveDevice(id)).unwrap();
        edit.execute(Command::InsertMainDevice {
            before: None,
            state: state.clone(),
        })
        .unwrap();
        let mut candidate = SessionPlayback::new(revision(&edit, &media)).unwrap();
        let position = donor.performance_frame();
        let result = candidate.try_inherit_state_from(&mut donor);
        if index == 64 {
            assert!(result.is_err(), "existing 64-slot bank must remain bounded");
            assert_eq!(donor.performance_frame(), position);
            let mut output = [[0.0; 2]; 1];
            donor.render(&mut output).unwrap();
            assert!(output[0].iter().any(|sample| sample.abs() > 0.001));
        } else {
            result.unwrap();
            candidate.render(&mut [[0.0; 2]; 1]).unwrap();
            donor = candidate;
        }
    }
}
