//! Actual prepared publication, compared against independent uninterrupted wet
//! and delayed-dry graphs, including a parallel room tail. No physical output.
use allocation_counter::measure;
use hex_audio::prepared;
use hex_engine::playback::{Playback, PlaybackCommand};
use hex_engine::{Audio, PreparedRevision, SessionPlayback, SessionPlaybackCommand, SineTable};
use hex_lattice_contract::{DeviceTypeId, NativeDeviceMetadata, ParameterValue};
use hex_project::{Command, Created, Editor, Project};
use std::{collections::BTreeMap, sync::Arc};

#[test]
fn configured_limiter_publications_preserve_latency_automation_pcm_and_callback_ownership() {
    for lookahead in 0..3 {
        for mode in 0..3 {
            for routing in 0..2 {
                for location in 0..3 {
                    check(lookahead, mode, routing, location, 64);
                    if lookahead == 2 && mode == 2 && routing == 1 {
                        check(lookahead, mode, routing, location, 512);
                    }
                }
            }
        }
    }
}

fn check(lookahead: u32, mode: u32, routing: u32, location: usize, block: usize) {
    let kind = "hex.limiter";
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Imported { asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "excitation".into(),
            path: "memory/aligned-bypass.wav".into(),
            source_sample_rate: 48_000,
            source_frames: 8192,
            playback_frames: 8192,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("import");
    };
    let track = editor.project().tracks()[0].id();
    let Created::Imported {
        asset: room_asset, ..
    } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "independent room excitation".into(),
            path: "memory/room-excitation.wav".into(),
            source_sample_rate: 48_000,
            source_frames: 8192,
            playback_frames: 8192,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("room import");
    };
    let room_track = editor.project().tracks()[1].id();
    // An unrelated room keeps sounding after the source becomes silent. Its
    // routed output must be identical in both counterfactuals and every handoff.
    let Created::ReturnTrack(room) = editor
        .execute(Command::AddReturnTrack {
            name: "room".into(),
        })
        .unwrap()
    else {
        panic!("room");
    };
    editor
        .execute(Command::SetReturnSend {
            track: room_track,
            return_track: room,
            gain: 0.25,
        })
        .unwrap();
    let definition = &hex_devices::reverb::REVERB_DEFINITION;
    let room_state = definition
        .patch_parameter(
            &definition.default_state().unwrap(),
            hex_devices::reverb::parameter::DRY_WET,
            ParameterValue::Float(100.0),
        )
        .unwrap();
    editor
        .execute(Command::InsertReturnDevice {
            return_track: room,
            before: None,
            state: room_state,
        })
        .unwrap();
    let mut state = hex_engine::default_native_device_state(DeviceTypeId::from_static(kind))
        .unwrap()
        .with_output_gain_db(6.0)
        .unwrap();
    for (parameter, value) in [
        (6, ParameterValue::Choice(lookahead)),
        (8, ParameterValue::Choice(mode)),
        (7, ParameterValue::Choice(routing)),
        (1, ParameterValue::Float(12.0)),
    ] {
        state = hex_engine::patch_native_device_parameter(
            &state,
            hex_lattice_contract::ParameterId::from_static(parameter),
            value,
        )
        .unwrap();
    }
    let command = match location {
        0 => Command::InsertDevice {
            track,
            before: None,
            state,
        },
        1 => {
            let Created::ReturnTrack(target) = editor
                .execute(Command::AddReturnTrack {
                    name: "drive".into(),
                })
                .unwrap()
            else {
                panic!("return");
            };
            editor
                .execute(Command::SetReturnSend {
                    track,
                    return_track: target,
                    gain: 0.5,
                })
                .unwrap();
            Command::InsertReturnDevice {
                return_track: target,
                before: None,
                state,
            }
        }
        _ => Command::InsertMainDevice {
            before: None,
            state,
        },
    };
    let Created::Device(device) = editor.execute(command).unwrap() else {
        panic!("device");
    };
    {
        editor
            .execute(Command::ReplaceDeviceAutomationTarget {
                device,
                target: hex_lattice_contract::DeviceAutomationTarget::HostOutputGain,
                points: vec![
                    hex_project::AutomationPoint {
                        beat: hex_project::BeatTime::ZERO,
                        value: hex_project::AutomationValue::new(0.75).unwrap(),
                        outgoing: hex_project::AutomationInterpolation::Linear,
                    },
                    hex_project::AutomationPoint {
                        beat: hex_project::BeatTime::from_raw(1 << 32),
                        value: hex_project::AutomationValue::new(0.9).unwrap(),
                        outgoing: hex_project::AutomationInterpolation::Linear,
                    },
                ],
            })
            .unwrap();
    }
    let audio = Audio::new(
        48_000,
        (0..8192)
            .map(|i| {
                if i < 2200 {
                    [
                        (i as f32 * 0.031).sin() * 0.2,
                        (i as f32 * 0.023).cos() * -0.15,
                    ]
                } else {
                    [0.0; 2]
                }
            })
            .collect(),
    )
    .unwrap();
    let media = BTreeMap::from([(asset, audio.clone()), (room_asset, audio)]);
    let table = Arc::new(SineTable::new());
    let prepare = |editor: &Editor| {
        Arc::new(
            PreparedRevision::prepare(editor.project(), &media, Arc::clone(&table), None).unwrap(),
        )
    };
    let wet_revision = prepare(&editor);
    editor
        .execute(Command::SetDeviceBypass {
            device,
            bypassed: true,
        })
        .unwrap();
    let dry_revision = prepare(&editor);
    assert_eq!(
        wet_revision.arrangement().main_latency_frames(),
        dry_revision.arrangement().main_latency_frames()
    );
    let mut wet = SessionPlayback::new(Arc::clone(&wet_revision)).unwrap();
    let mut dry = SessionPlayback::new(Arc::clone(&dry_revision)).unwrap();
    wet.apply(SessionPlaybackCommand::Play).unwrap();
    dry.apply(SessionPlaybackCommand::Play).unwrap();
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    control
        .publish(Playback::try_from_prepared_with_session(Arc::clone(&wet_revision)).unwrap())
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut actual = [[0.0; 2]; 512];
    let mut wet_pcm = actual;
    let mut dry_pcm = actual;
    let mut position = 0;
    while position < 2048 {
        render(
            &mut callback,
            &mut wet,
            &mut dry,
            &mut actual[..64],
            &mut wet_pcm[..64],
            &mut dry_pcm[..64],
        );
        assert_eq!(&actual[..64], &wet_pcm[..64]);
        position += 64;
    }
    let mut offset = 0;
    let mut contrast = 0.0_f32;
    for (bypassed, count) in [
        (true, 13),
        (true, 5),
        (false, 300),
        (true, 300),
        (false, 300),
    ] {
        editor
            .execute(Command::SetDeviceBypass { device, bypassed })
            .unwrap();
        let next = prepare(&editor);
        control
            .publish(Playback::try_from_prepared_with_session(next).unwrap())
            .unwrap();
        let mut rendered = 0;
        while rendered < count {
            let n = (count - rendered).min(block);
            render(
                &mut callback,
                &mut wet,
                &mut dry,
                &mut actual[..n],
                &mut wet_pcm[..n],
                &mut dry_pcm[..n],
            );
            for local in 0..n {
                let i = offset + rendered + local;
                let mix = if i < 18 {
                    1.0 - (i + 1) as f32 / 240.0
                } else if i < 318 {
                    0.925 + 0.075 * (i - 17).min(240) as f32 / 240.0
                } else if i < 618 {
                    1.0 - (i - 317).min(240) as f32 / 240.0
                } else {
                    (i - 617).min(240) as f32 / 240.0
                };
                for lane in 0..2 {
                    contrast = contrast.max((wet_pcm[local][lane] - dry_pcm[local][lane]).abs());
                    let expected =
                        dry_pcm[local][lane] + (wet_pcm[local][lane] - dry_pcm[local][lane]) * mix;
                    assert!(
                        (actual[local][lane] - expected).abs() < 3e-6,
                        "{kind}/{location} lookahead={lookahead} mode={mode} routing={routing} block={block} frame={i}: {:?} expected {expected}",
                        actual[local]
                    );
                }
            }
            rendered += n;
            position += n;
            assert_eq!(control.status.recovery_position(), position as u64);
            assert!(control.status.rejected_action().is_none());
        }
        offset += count;
        control.reclaim();
    }
    assert!(contrast > 0.01, "wet/dry fixture must be distinguishable");
    // The source has been silent for over700 frames. A fully dry primary path
    // is silent, while the independent parallel room still contributes a tail.
    // Main placement processes that room too; only track/Return can isolate it.
    if location != 2 {
        assert!(dry_pcm[..1].iter().flatten().any(|v| v.abs() > 1e-5));
    }
    control.request_shutdown();
    zero(measure(|| {
        callback.begin_block();
        callback.render(&mut actual).unwrap();
    }));
    control.reclaim();
}

fn render(
    callback: &mut prepared::Callback,
    wet: &mut SessionPlayback,
    dry: &mut SessionPlayback,
    actual: &mut [[f32; 2]],
    wet_pcm: &mut [[f32; 2]],
    dry_pcm: &mut [[f32; 2]],
) {
    wet.render(wet_pcm).unwrap();
    dry.render(dry_pcm).unwrap();
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
