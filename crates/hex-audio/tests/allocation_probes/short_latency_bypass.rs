//! Actual callback publication against independent uninterrupted wet, aligned
//! dry, and fresh-processor references, with a parallel room tail.
use allocation_counter::measure;
use hex_audio::prepared;
use hex_engine::playback::{Playback, PlaybackCommand};
use hex_engine::{
    Audio, Frame, NodeSignalPhase, PreparedRevision, SessionPlayback, SessionPlaybackCommand,
    SineTable,
};
use hex_lattice::{
    DevicePreparationContext, EMPTY_DEVICE_RESOURCES, NativeParameterAutomation,
    ProcessorInstanceIdentity, TuningTable,
};
use hex_lattice_contract::{
    DeviceTypeId, NativeDeviceMetadata, NativeDeviceState, ParameterId, ParameterValue,
};
use hex_project::{BeatTime, Command, Created, DeviceId, Editor, Project, TempoMap};
use std::{
    collections::BTreeMap,
    sync::{Arc, atomic::Ordering},
};

const RATE: u32 = 48_000;
const FADE: usize = RATE.div_ceil(200) as usize;
const RESTART_FADE: usize = RATE.div_ceil(2_000) as usize;
const WARMUP: usize = 2048;
const EARLY_OFF: usize = 13;
const REPEAT_OFF: usize = 5;
const REVERSE: usize = 300;
const SETTLE: usize = 300;
const RESTART: usize = WARMUP + EARLY_OFF + REPEAT_OFF + REVERSE + SETTLE;
const END: usize = RESTART + 300;
const DRIVE: ParameterId = hex_devices::rattle::parameter::DRIVE;
const DRIVE_START: f32 = 0.2;
const DRIVE_END: f32 = 0.9;

#[test]
fn aligned_provider_publications_preserve_routed_pcm_tails_and_callback_ownership() {
    for (kind, automated) in [
        ("hex.rattle", false),
        ("hex.rattle", true),
        ("hex.overdrive", false),
        ("hex.saturator", false),
    ] {
        for location in 0..3 {
            for block in [64, 512] {
                check(kind, automated, location, block);
            }
        }
    }
}

fn check(kind: &'static str, automated: bool, location: usize, block: usize) {
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 32).unwrap();
    let Created::Imported { asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "excitation".into(),
            path: "memory/aligned-bypass.wav".into(),
            source_sample_rate: RATE,
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
            source_sample_rate: RATE,
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
    if kind == "hex.saturator" {
        state = hex_devices::saturator::SATURATOR_DEFINITION
            .patch_parameter(
                &state,
                hex_devices::saturator::parameter::DRIVE,
                ParameterValue::Float(9.0),
            )
            .unwrap();
    }
    let command = match location {
        0 => Command::InsertDevice {
            track,
            before: None,
            state: state.clone(),
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
                state: state.clone(),
            }
        }
        _ => Command::InsertMainDevice {
            before: None,
            state: state.clone(),
        },
    };
    let Created::Device(device) = editor.execute(command).unwrap() else {
        panic!("device");
    };
    if automated {
        editor
            .execute(Command::ReplaceDeviceAutomation {
                device,
                parameter: DRIVE,
                points: vec![
                    hex_project::AutomationPoint {
                        beat: BeatTime::ZERO,
                        value: hex_project::AutomationValue::new(DRIVE_START).unwrap(),
                        outgoing: hex_project::AutomationInterpolation::Linear,
                    },
                    hex_project::AutomationPoint {
                        beat: BeatTime::ONE,
                        value: hex_project::AutomationValue::new(DRIVE_END).unwrap(),
                        outgoing: hex_project::AutomationInterpolation::Linear,
                    },
                ],
            })
            .unwrap();
    }
    let signal = |frame: usize| {
        [
            (frame as f32 * 0.031).sin() * 0.2,
            (frame as f32 * 0.023).cos() * -0.15,
        ]
    };
    let mut excitation: Vec<_> = (0..8192)
        .map(|frame| {
            if frame < 2200 {
                signal(frame)
            } else {
                [0.0; 2]
            }
        })
        .collect();
    let room_audio = Audio::new(RATE, excitation.clone()).unwrap();
    // Give the dry delay real input just before restart and give the new
    // processor an independently verifiable transient on the same route.
    let burst_start = RESTART - 23;
    for (offset, sample) in excitation[burst_start..RESTART + 80].iter_mut().enumerate() {
        *sample = signal(burst_start + offset);
    }
    let media = BTreeMap::from([
        (asset, Audio::new(RATE, excitation).unwrap()),
        (room_asset, room_audio),
    ]);
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
    let latency = dry_revision.arrangement().main_latency_frames();
    assert_eq!(wet_revision.arrangement().main_latency_frames(), latency);
    assert_ne!(latency, 0);
    let mut wet = SessionPlayback::new(Arc::clone(&wet_revision)).unwrap();
    let mut dry = SessionPlayback::new(Arc::clone(&dry_revision)).unwrap();
    wet.apply(SessionPlaybackCommand::Play).unwrap();
    dry.apply(SessionPlaybackCommand::Play).unwrap();
    let mut wet_pcm = vec![[0.0; 2]; END];
    let mut dry_pcm = vec![[0.0; 2]; END];
    wet.render(&mut wet_pcm).unwrap();
    dry.render(&mut dry_pcm[..RESTART]).unwrap();
    let tempo = automated.then_some(editor.project().tempo_map());
    let fresh_delta = fresh_effect_delta(&mut dry, &state, device, tempo, &mut dry_pcm[RESTART..]);
    let distinguishes_fresh = fresh_delta.iter().enumerate().any(|(offset, delta)| {
        let frame = RESTART + offset;
        offset >= latency as usize
            && (0..2).any(|lane| {
                (dry_pcm[frame][lane] + delta[lane] - wet_pcm[frame][lane]).abs() > 0.001
            })
    });
    assert!(
        distinguishes_fresh,
        "{kind}/{location}: restart must distinguish old history"
    );

    let (mut control, mut callback) = prepared::channel(RATE).unwrap();
    control
        .publish(Playback::try_from_prepared_with_session(Arc::clone(&wet_revision)).unwrap())
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 512];
    let mut cursor = 0;
    for expected in wet_pcm[..WARMUP].chunks(64) {
        let actual = &mut output[..expected.len()];
        render(&mut callback, actual);
        assert_eq!(actual, expected);
        cursor += actual.len();
    }

    let mut contrast = 0.0_f32;
    for (phase, (bypassed, count)) in [
        (true, EARLY_OFF),
        (true, REPEAT_OFF),
        (false, REVERSE),
        (true, SETTLE),
        (false, END - RESTART),
    ]
    .into_iter()
    .enumerate()
    {
        editor
            .execute(Command::SetDeviceBypass { device, bypassed })
            .unwrap();
        let applied = control
            .publish(Playback::try_from_prepared_with_session(prepare(&editor)).unwrap())
            .unwrap();
        let phase_start = cursor;
        for requested in (0..count).step_by(block) {
            let actual = &mut output[..(count - requested).min(block)];
            render(&mut callback, actual);
            for (local, value) in actual.iter().enumerate() {
                let frame = cursor + local;
                let phase_age = frame - phase_start + 1;
                let progress = |age: usize| age.min(FADE) as f32 / FADE as f32;
                let early_off = EARLY_OFF + REPEAT_OFF;
                let mix = match phase {
                    0 => 1.0 - progress(phase_age),
                    1 => 1.0 - progress(EARLY_OFF + phase_age),
                    2 => 1.0 - progress(early_off) + progress(early_off) * progress(phase_age),
                    3 => 1.0 - progress(phase_age),
                    // After a settled Off, keep the aligned dry lead before
                    // the fresh wet de-click. Off and reversals still use FADE.
                    4 => {
                        let wet_age = phase_age.saturating_sub(latency as usize);
                        wet_age.min(RESTART_FADE) as f32 / RESTART_FADE as f32
                    }
                    _ => unreachable!("test publication phase"),
                };
                if mix == 0.0 {
                    assert_eq!(
                        *value, dry_pcm[frame],
                        "{kind}/{location} dry frame {frame}"
                    );
                }
                for (lane, &actual_sample) in value.iter().enumerate() {
                    let difference = if frame < RESTART {
                        wet_pcm[frame][lane] - dry_pcm[frame][lane]
                    } else {
                        fresh_delta[frame - RESTART][lane]
                    };
                    contrast = contrast.max(difference.abs());
                    let expected = dry_pcm[frame][lane] + difference * mix;
                    assert!(
                        (actual_sample - expected).abs() < 3e-6,
                        "{kind}/{location} automated={automated} block={block} frame={frame}: {value:?} expected {expected}",
                    );
                }
            }
            cursor += actual.len();
            assert_eq!(control.status.recovery_position(), cursor as u64);
            assert_eq!(
                control.status.applied_revision.load(Ordering::Acquire),
                applied
            );
            assert!(control.status.rejected_action().is_none());
        }
        assert_eq!(control.reclaim(), 1);
    }
    assert_eq!(cursor, END);
    assert!(contrast > 0.01, "wet/dry fixture must be distinguishable");
    // With the primary excitation silent, the independent room still rings.
    if location != 2 {
        assert!(dry_pcm[END - 1].iter().any(|v| v.abs() > 1e-5));
    }
    control.request_shutdown();
    render(&mut callback, &mut output);
    control.reclaim();
}

/// Read the target's routed input and aligned dry output from the always-off
/// reference. A pristine factory sees just the new input, never the old pass.
fn fresh_effect_delta(
    dry: &mut SessionPlayback,
    state: &NativeDeviceState,
    device: DeviceId,
    tempo: Option<&TempoMap>,
    output: &mut [Frame],
) -> Vec<Frame> {
    let mut input = vec![[0.0; 2]; output.len()];
    let mut aligned_dry = input.clone();
    let mut identity = None;
    let mut next_input = 0;
    let mut next_output = 0;
    dry.render_with_node_signal_observer(
        output,
        Some(&mut |node, phase, offset, pcm| {
            if node.device_id != device.value() {
                return;
            }
            identity = Some(ProcessorInstanceIdentity::new(node.owner, node.device_id).unwrap());
            let (samples, next) = match phase {
                NodeSignalPhase::Input => (&mut input, &mut next_input),
                NodeSignalPhase::Output => (&mut aligned_dry, &mut next_output),
            };
            assert_eq!(offset, *next, "continuous target tap");
            *next += pcm.len();
            samples[offset..*next].copy_from_slice(pcm);
        }),
    )
    .unwrap();
    assert_eq!((next_input, next_output), (output.len(), output.len()));
    let registry = hex_devices::native_registry().unwrap();
    let parameters = if tempo.is_some() { &[DRIVE][..] } else { &[] };
    let prepared = registry
        .prepare_runtime_instance_with_automation(
            identity.unwrap(),
            state,
            &DevicePreparationContext {
                sample_rate: RATE,
                tuning: &TuningTable::default(),
                resources: &EMPTY_DEVICE_RESOURCES,
            },
            parameters,
        )
        .unwrap();
    let mut fresh = prepared
        .processor
        .as_ref()
        .and_then(|factory| factory.legacy())
        .unwrap()
        .create_runtime();
    if let Some(tempo) = tempo {
        fresh
            .process_block_with_automation(
                &mut input,
                None,
                &FreshDrive(tempo),
                &EMPTY_DEVICE_RESOURCES,
            )
            .unwrap();
    } else {
        fresh.process_block(&mut input, &EMPTY_DEVICE_RESOURCES);
    }
    let gain = state.output_gain_linear();
    for (wet, dry) in input.iter_mut().zip(aligned_dry) {
        for (sample, dry) in wet.iter_mut().zip(dry) {
            *sample = *sample * gain - dry;
        }
    }
    input
}

struct FreshDrive<'a>(&'a TempoMap);

impl NativeParameterAutomation for FreshDrive<'_> {
    fn parameter_ids(&self) -> &[ParameterId] {
        &[DRIVE]
    }

    fn value_at(&self, parameter: ParameterId, offset: usize) -> Option<ParameterValue> {
        assert_eq!(parameter, DRIVE);
        let beat = self.0.frame_to_beat((RESTART + offset) as u64).unwrap();
        let fraction = beat.raw() as f64 / BeatTime::ONE.raw() as f64;
        let drive =
            f64::from(DRIVE_START) + (f64::from(DRIVE_END) - f64::from(DRIVE_START)) * fraction;
        Some(ParameterValue::Float(drive as f32))
    }
}

fn render(callback: &mut prepared::Callback, output: &mut [Frame]) {
    zero(measure(|| {
        callback.begin_block();
        callback.render(output).unwrap();
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
