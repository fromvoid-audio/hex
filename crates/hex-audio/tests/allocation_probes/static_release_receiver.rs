//! Original Sampler release versus an independently authored note-off, through
//! actual prepared publication. An unrelated source and a later onset continue.
use allocation_counter::measure;
use hex_audio::prepared;
use hex_engine::{
    PreparedRevision, SessionPlayback, SessionPlaybackCommand, SineTable,
    native_device_resources::{
        PreparedDeviceResources, ResidentDeviceAssets, ResourcePreparationError,
    },
    playback::{Playback, PlaybackCommand},
};
use hex_lattice_contract::{
    DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, NativeDeviceMetadata, ParameterValue,
};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::{
    collections::BTreeMap,
    sync::{Arc, atomic::AtomicBool},
};

fn prepare(
    project: &Project,
    store: &ResidentDeviceAssets,
) -> (Arc<PreparedRevision>, PreparedDeviceResources) {
    let cancel = AtomicBool::new(false);
    let resources =
        PreparedDeviceResources::prepare_on_control(store, project, 20_000_000, &cancel).unwrap();
    let revision = PreparedRevision::prepare_with_device_resources(
        project,
        &BTreeMap::new(),
        Arc::new(SineTable::new()),
        None,
        &hex_engine::audio_transform::AudioTransformPreparation::default(),
        &resources,
        resources.compiler_scan_work_on_control(project).unwrap(),
        &cancel,
    )
    .unwrap();
    (Arc::new(revision), resources)
}

#[test]
fn static_receiver_rejoin_matches_authored_note_off_and_future_onset_without_callback_allocations()
{
    for case in 0..3 {
        for block in [1, 7, 64, 512] {
            check(case, block);
        }
    }
}

#[test]
fn gate_suffix_rejoin_retains_detector_history_and_source_pcm() {
    for block in [1, 7, 64, 512] {
        check(3, block);
    }
}

#[test]
fn unproved_stateful_suffix_refuses_reenable_and_keeps_previous_release() {
    check(4, 17);
}

#[test]
fn sampler_release_rejoin_preserves_its_return_send_and_other_feeds() {
    for block in [1, 7, 64, 512] {
        check(5, block);
    }
}

#[test]
fn unproved_compensated_bypass_refuses_without_a_dropout() {
    check(6, 17);
}

fn check(case: usize, block: usize) {
    let mut editor = Editor::new(Project::new(8000).unwrap(), 32).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::ImportDeviceAsset {
            path: "memory/release.wav".into(),
            sample_rate: 8000,
            frames: 4,
        })
        .unwrap()
    else {
        panic!("asset");
    };
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "sampler".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    let mut sampler = hex_devices::sampler::state::SamplerState::for_source_frames(4).unwrap();
    sampler.mode = hex_devices::sampler::state::AuthoredMode::Classic {
        loop_enabled: true,
        loop_region: sampler.region,
    };
    sampler.envelope = hex_devices::sampler::Envelope {
        attack_seconds: 0.0,
        decay_seconds: 0.0,
        sustain: 1.0,
        release_seconds: 0.1,
    };
    let state = sampler
        .to_native()
        .unwrap()
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(1).unwrap(),
            asset: DeviceAssetKey::new(asset.value()).unwrap(),
        }])
        .unwrap();
    editor
        .execute(Command::InsertInstrument { track, state })
        .unwrap();
    let device = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let utility = &hex_devices::utility::UTILITY_DEFINITION;
    use hex_devices::utility::parameter as p;
    let mut state = utility.default_state().unwrap();
    if case == 1 {
        state = utility
            .patch_parameter(&state, p::WIDTH, ParameterValue::Float(2.0))
            .unwrap();
        state = utility
            .patch_parameter(&state, p::BALANCE, ParameterValue::Float(0.5))
            .unwrap();
    } else if case == 3 {
        state = hex_engine::default_native_device_state(hex_devices::gate::GATE_TYPE_ID).unwrap();
    } else if case == 4 {
        state =
            hex_engine::default_native_device_state(hex_devices::compressor::COMPRESSOR_TYPE_ID)
                .unwrap();
    } else if case == 2 {
        state = utility
            .patch_parameter(&state, p::INVERT_LEFT, ParameterValue::Bool(true))
            .unwrap();
    }
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state,
        })
        .unwrap();
    if case == 2 {
        let state = utility
            .patch_parameter(
                &utility.default_state().unwrap(),
                p::WIDTH,
                ParameterValue::Float(0.5),
            )
            .unwrap();
        editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state,
            })
            .unwrap();
    }
    let Created::Track(unrelated) = editor
        .execute(Command::AddMidiTrack {
            name: "unrelated".into(),
        })
        .unwrap()
    else {
        panic!("unrelated");
    };
    editor
        .execute(Command::Gain {
            track: unrelated,
            gain: 0.25,
        })
        .unwrap();
    if case == 5 {
        let Created::ReturnTrack(bus) = editor
            .execute(Command::AddReturnTrack {
                name: "shared return".into(),
            })
            .unwrap()
        else {
            panic!("return");
        };
        for (track, gain) in [(track, 0.5), (unrelated, 0.25)] {
            editor
                .execute(Command::SetReturnSend {
                    track,
                    return_track: bus,
                    gain,
                })
                .unwrap();
        }
    }
    if case == 6 {
        editor
            .execute(Command::InsertMainDevice {
                before: None,
                state: hex_engine::default_native_device_state(
                    hex_lattice_contract::DeviceTypeId::from_static("hex.limiter"),
                )
                .unwrap(),
            })
            .unwrap();
    }
    let Created::Clip(other_clip) = editor
        .execute(Command::PlaceMidiClip {
            track: unrelated,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip");
    };
    editor
        .execute(Command::AddMidiNote {
            clip: other_clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 67,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip");
    };
    let mut oracle_editor = Editor::new(editor.project().clone(), 32).unwrap();
    let off = editor.project().tempo_map().frame_to_beat(129).unwrap();
    let future = editor.project().tempo_map().frame_to_beat(300).unwrap();
    let duration = editor.project().tempo_map().frame_to_beat(16).unwrap();
    let oracle_gate = if case == 6 { BeatTime::ONE } else { off };
    for (target, gate) in [
        (&mut editor, BeatTime::ONE),
        (&mut oracle_editor, oracle_gate),
    ] {
        target
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: gate,
                pitch: 60,
                channel: 0,
                velocity: 127,
            })
            .unwrap();
        target
            .execute(Command::AddMidiNote {
                clip,
                start: future,
                duration,
                pitch: 60,
                channel: 0,
                velocity: 127,
            })
            .unwrap();
    }
    let mut store = ResidentDeviceAssets::default();
    store
        .insert_on_control(
            (
                asset,
                hex_engine::OwnedAudioSource::try_resident(
                    8000,
                    vec![[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25], [0.25, 0.125]],
                )
                .ok()
                .unwrap(),
            ),
            0,
            hex_engine::AudioStorageLedgerReservation {
                data_rows: 256,
                handle_rows: 64,
                root_rows: 32,
                max_retained_bytes: 20_000_000,
                max_construction_peak_bytes: 20_000_000,
            },
            4,
            &AtomicBool::new(false),
            || Ok::<_, ResourcePreparationError>(()),
        )
        .unwrap();
    let (revision, resources) = prepare(editor.project(), &store);
    let (oracle_revision, oracle_resources) = prepare(oracle_editor.project(), &store);
    let mut oracle =
        SessionPlayback::new_with_device_resources(oracle_revision, &oracle_resources).unwrap();
    oracle.apply(SessionPlaybackCommand::Play).unwrap();
    let (mut control, mut callback) = prepared::channel(8000).unwrap();
    control
        .configure_device_resource_publication_on_control(
            hex_engine::AudioStorageLedgerReservation {
                data_rows: 512,
                handle_rows: 256,
                root_rows: 128,
                max_retained_bytes: 20_000_000,
                max_construction_peak_bytes: 20_000_000,
            },
        )
        .unwrap();
    control
        .publish(Playback::try_from_prepared_with_device_resources(revision, &resources).unwrap())
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut actual = [[0.0; 2]; 512];
    let mut expected = actual;
    let mut position = 0;
    render(
        &mut callback,
        &mut oracle,
        &mut actual[..129],
        &mut expected[..129],
    );
    assert_eq!(&actual[..129], &expected[..129]);
    position += 129;
    for (bypassed, count) in [(true, 13), (false, 13), (true, 7), (false, 13)] {
        editor
            .execute(Command::SetDeviceBypass { device, bypassed })
            .unwrap();
        let (revision, resources) = prepare(editor.project(), &store);
        let previous_revision = control
            .status
            .applied_revision
            .load(std::sync::atomic::Ordering::Acquire);
        let applied = control
            .publish(
                Playback::try_from_prepared_with_device_resources(revision, &resources).unwrap(),
            )
            .unwrap();
        let mut done = 0;
        while done < count {
            let n = (count - done).min(block);
            render(
                &mut callback,
                &mut oracle,
                &mut actual[..n],
                &mut expected[..n],
            );
            assert_eq!(
                &actual[..n],
                &expected[..n],
                "case={case} block={block} bypassed={bypassed}"
            );
            done += n;
            position += n;
            assert_eq!(control.status.recovery_position(), position as u64);
        }
        if (case == 4 && !bypassed) || (case == 6 && bypassed) {
            assert_eq!(
                control.status.rejected_revision(),
                Some((
                    applied,
                    hex_engine::PlaybackHandoffRejection::SourceHistoryUnavailable
                ))
            );
            assert_eq!(
                control
                    .status
                    .applied_revision
                    .load(std::sync::atomic::Ordering::Acquire),
                previous_revision
            );
            assert_eq!(control.reclaim(), 1, "refused owner returns to control");
            if case == 6 {
                // A terminal compensated-source refusal must not obstruct
                // priority Stop or chase the later onset from a fresh kernel.
                control.stop();
                oracle.apply(SessionPlaybackCommand::Stop).unwrap();
                for frames in [1, 7, 64, 512, 512, 64] {
                    render(
                        &mut callback,
                        &mut oracle,
                        &mut actual[..frames],
                        &mut expected[..frames],
                    );
                    assert_eq!(&actual[..frames], &expected[..frames]);
                }
                assert!(actual[..64].iter().all(|frame| *frame == [0.0; 2]));
            }
            control.request_shutdown();
            zero(measure(|| callback.begin_block()));
            assert_eq!(control.reclaim(), 1);
            return;
        }
        assert_eq!(
            control
                .status
                .applied_revision
                .load(std::sync::atomic::Ordering::Acquire),
            applied
        );
        assert!(control.status.rejected_revision().is_none());
        control.reclaim();
    }
    while position < 5000 {
        let n = (5000 - position).min(block);
        render(
            &mut callback,
            &mut oracle,
            &mut actual[..n],
            &mut expected[..n],
        );
        assert_eq!(
            &actual[..n],
            &expected[..n],
            "case={case} block={block} frame={position}: release, future onset or unrelated source changed"
        );
        position += n;
    }
    assert_eq!(
        actual[0], [0.0; 2],
        "all notes end without a chased/stuck gate"
    );
    control.request_shutdown();
    zero(measure(|| {
        callback.begin_block();
        callback.render(&mut actual).unwrap();
    }));
    control.reclaim();
}

fn render(
    callback: &mut prepared::Callback,
    oracle: &mut SessionPlayback,
    actual: &mut [[f32; 2]],
    expected: &mut [[f32; 2]],
) {
    oracle.render(expected).unwrap();
    zero(measure(|| {
        callback.begin_block();
        callback.render(actual).unwrap();
    }));
}
fn zero(info: allocation_counter::AllocationInfo) {
    assert_eq!(
        (
            info.count_total,
            info.bytes_total,
            info.count_current,
            info.bytes_current
        ),
        (0, 0, 0, 0)
    );
}
