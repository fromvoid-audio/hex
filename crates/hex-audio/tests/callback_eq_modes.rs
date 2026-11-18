//! Mode first-publication, rapid targets and lifecycle through the real callback.
#![cfg(feature = "allocation-probe")]
#[path = "allocation_probes/callback_allocations/probe.rs"]
mod probe;
use hex_audio::prepared;
use hex_devices::eq::{EqState, MODE, Node, NodeId, NodeParameter, OUTPUT_GAIN};
use hex_engine::{
    Audio, PreparedRevision, SineTable,
    playback::{Playback, PlaybackCommand},
};
use hex_lattice_contract::{DeviceAutomationTarget as Target, ParameterValue};
use hex_project::{
    AutomationInterpolation as Curve, AutomationPoint, AutomationReadSelection as Read,
    AutomationValue, BeatTime, Command, Created, Editor, Project,
};
use probe::{compare_output, measured_block, measured_shutdown};
use std::{
    collections::BTreeMap,
    error::Error,
    sync::{Arc, atomic::Ordering},
};
const RATE: u32 = 48000;
const BLOCK: usize = 1024;
fn close(a: &[[f32; 2]], b: &[[f32; 2]]) {
    for (a, b) in a.iter().zip(b) {
        for c in 0..2 {
            assert!((a[c] - b[c]).abs() < 3e-6, "{a:?} != {b:?}");
        }
    }
}
#[test]
fn mode_first_assignment_reading_rapid_curves_undo_bypass_and_shutdown_allocate_nothing()
-> Result<(), Box<dyn Error>> {
    for mode in [1, 2] {
        let mut e = Editor::new(Project::new(RATE)?, 32)?;
        let Created::Imported { track, asset, .. } =
            e.execute(Command::ImportPreparedAudioTrack {
                name: "eq modes".into(),
                path: "memory/eq-modes.wav".into(),
                source_sample_rate: RATE,
                source_frames: (BLOCK * 32) as u64,
                playback_frames: (BLOCK * 32) as u64,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            })?
        else {
            return Err("audio track".into());
        };
        let Created::Device(device) = e.execute(Command::InsertDevice {
            track,
            before: None,
            state: EqState::default().encode()?,
        })?
        else {
            return Err("eq".into());
        };
        let entry = e.project().device(device).ok_or("eq missing")?;
        let expected = entry.state().clone();
        let variant = entry.active_variant();
        let next = e.project().next_identity();
        let mut settings = EqState::default();
        for bank in 0..5 {
            settings.add(Node::new(
                NodeId::new(next + u64::from(bank))?,
                700.0,
                f32::from(bank) * 3.0 - 6.0,
                bank,
            )?)?;
        }
        e.execute(Command::ReplaceDeviceStateWithIdentityReservation {
            device,
            expected_state: expected,
            expected_variant: variant,
            state: settings.encode()?,
            automation: None,
            expected_next_id: next,
            reserved_ids: 5,
        })?;
        let parameter = MODE;
        let patched = hex_lattice_contract::NativeDeviceMetadata::patch_parameter(
            &hex_devices::eq::EQ_DEFINITION,
            &settings.encode()?,
            parameter,
            ParameterValue::Choice(mode),
        )?;
        let mut canonical = Editor::new(e.project().clone(), 4)?;
        canonical.execute(Command::ReplaceDeviceState {
            device,
            state: patched,
        })?;
        let mut raw = Editor::new(e.project().clone(), 4)?;
        raw.execute(Command::ReplaceDeviceState {
            device,
            state: EqState::default().encode()?,
        })?;
        let pcm = (0..BLOCK * 32)
            .map(|i| {
                let x = (i as f32 * std::f32::consts::TAU * 441.0 / RATE as f32).sin() * 0.1
                    + (i as f32 * std::f32::consts::TAU * 1733.0 / RATE as f32).sin() * 0.08;
                [x, -x * 0.5]
            })
            .collect();
        let media = BTreeMap::from([(asset, Audio::new(RATE, pcm)?)]);
        let table = Arc::new(SineTable::new());
        let prepare = |project: &Project| -> Result<_, Box<dyn Error>> {
            Ok(Arc::new(PreparedRevision::prepare(
                project,
                &media,
                Arc::clone(&table),
                None,
            )?))
        };
        let (mut control, mut callback) = prepared::channel(RATE)?;
        let (mut old_control, mut old_callback) = prepared::channel(RATE)?;
        let (mut canonical_control, mut canonical_callback) = prepared::channel(RATE)?;
        let (mut raw_control, mut raw_callback) = prepared::channel(RATE)?;
        for (owner, project) in [
            (&mut control, e.project()),
            (&mut old_control, e.project()),
            (&mut canonical_control, canonical.project()),
            (&mut raw_control, raw.project()),
        ] {
            owner
                .publish(Playback::try_from_prepared_with_session(prepare(project)?)?)
                .map_err(|_| "publish")?;
            owner.command(PlaybackCommand::Play).map_err(|_| "play")?;
        }
        let mut out = [[[0.0; 2]; BLOCK]; 4];
        {
            let mut tick = |out: &mut [[[f32; 2]; BLOCK]; 4]| {
                measured_block(&mut callback, &mut out[0]);
                measured_block(&mut old_callback, &mut out[1]);
                measured_block(&mut canonical_callback, &mut out[2]);
                measured_block(&mut raw_callback, &mut out[3]);
            };
            tick(&mut out);
            compare_output(&out[0], &out[1]);
            e.execute(Command::ReplaceDeviceAutomation {
                device,
                parameter,
                points: vec![AutomationPoint {
                    beat: BeatTime::ZERO,
                    value: AutomationValue::new(mode as f32 / 2.0)?,
                    outgoing: Curve::Step,
                }],
            })?;
            let authored = e.project().snapshot();
            assert!(e.undo());
            assert!(e.redo());
            assert_eq!(e.project().snapshot(), authored);
            let path = std::env::temp_dir().join(format!(
                "hex-eq-modes-{}-{}.hex",
                std::process::id(),
                parameter.value()
            ));
            hex_storage::save_new(&path, e.project())?;
            let loaded = hex_storage::load(&path);
            std::fs::remove_file(path)?;
            let loaded = loaded?;
            assert_eq!(&loaded, e.project());
            let revision = control
                .publish(Playback::try_from_prepared_with_session(prepare(&loaded)?)?)
                .map_err(|_| "first node publish")?;
            let before = control.status.position.load(Ordering::Acquire);
            tick(&mut out);
            assert_eq!(
                control.status.position.load(Ordering::Acquire),
                before + BLOCK as u64
            );
            assert_eq!(
                control.status.applied_revision.load(Ordering::Acquire),
                revision
            );
            assert!(control.status.playing.load(Ordering::Acquire));
            assert!(
                out[0][BLOCK - 64..]
                    .iter()
                    .zip(&out[1][BLOCK - 64..])
                    .any(|(a, b)| (a[0] - b[0]).abs() > 1e-5),
                "mode {mode} first assignment must change rendered audio"
            );
            close(&out[0][BLOCK - 64..], &out[2][BLOCK - 64..]);
            control.reclaim();
            for selection in [Read::Base, Read::Authored] {
                e.execute(Command::SetDeviceAutomationReadSelection {
                    device,
                    target: Target::Parameter(parameter),
                    selection,
                })?;
                let revision = control
                    .publish(Playback::try_from_prepared_with_session(prepare(
                        e.project(),
                    )?)?)
                    .map_err(|_| "read publish")?;
                tick(&mut out);
                tick(&mut out);
                assert_eq!(
                    control.status.applied_revision.load(Ordering::Acquire),
                    revision
                );
                close(
                    &out[0][BLOCK - 64..],
                    &out[if selection == Read::Base { 1 } else { 2 }][BLOCK - 64..],
                );
                control.reclaim();
            }
            e.execute(Command::ReplaceDeviceAutomation {
                device,
                parameter: OUTPUT_GAIN,
                points: vec![AutomationPoint {
                    beat: BeatTime::ZERO,
                    value: AutomationValue::new(1.0 / 16.0)?,
                    outgoing: Curve::Step,
                }],
            })?;
            e.execute(Command::SetDeviceAutomationReadSelection {
                device,
                target: Target::Parameter(OUTPUT_GAIN),
                selection: Read::Base,
            })?;
            let bank = if mode == 1 { 1 } else { 3 };
            let node_parameter = NodeId::new(next + bank)?.parameter(NodeParameter::Gain)?;
            e.execute(Command::ReplaceDeviceAutomation {
                device,
                parameter: node_parameter,
                points: vec![AutomationPoint {
                    beat: BeatTime::ZERO,
                    value: AutomationValue::new(
                        NodeParameter::Gain
                            .normalized(ParameterValue::Float(bank as f32 * 3.0 - 6.0))?,
                    )?,
                    outgoing: Curve::Step,
                }],
            })?;
            e.execute(Command::SetDeviceAutomationReadSelection {
                device,
                target: Target::Parameter(node_parameter),
                selection: Read::Base,
            })?;
            control
                .publish(Playback::try_from_prepared_with_session(prepare(
                    e.project(),
                )?)?)
                .map_err(|_| "retained output row")?;
            tick(&mut out);
            // Adding unchanged node/output rows must preserve the effective
            // routing and its complete history while Mode differs from Base.
            close(&out[0], &out[2]);
            control.reclaim();
            let before_delete = e.project().snapshot();
            e.execute(Command::ReplaceDeviceAutomation {
                device,
                parameter: MODE,
                points: Vec::new(),
            })?;
            assert_eq!(
                e.project()
                    .device(device)
                    .ok_or("eq missing")?
                    .automation()
                    .len(),
                2
            );
            let revision = control
                .publish(Playback::try_from_prepared_with_session(prepare(
                    e.project(),
                )?)?)
                .map_err(|_| "mode row removal")?;
            tick(&mut out);
            assert_eq!(
                control.status.applied_revision.load(Ordering::Acquire),
                revision
            );
            close(&out[0][BLOCK - 64..], &out[1][BLOCK - 64..]);
            control.reclaim();
            assert!(e.undo());
            assert_eq!(e.project().snapshot(), before_delete);
            let revision = control
                .publish(Playback::try_from_prepared_with_session(prepare(
                    e.project(),
                )?)?)
                .map_err(|_| "node undo")?;
            tick(&mut out);
            tick(&mut out);
            assert_eq!(
                control.status.applied_revision.load(Ordering::Acquire),
                revision
            );
            close(&out[0][BLOCK - 64..], &out[2][BLOCK - 64..]);
            control.reclaim();
            // The compiled curve changes faster than the single fade, then
            // holds the original mode. Compare the real callback with the
            // ordinary Session renderer at exactly the same content frames.
            let position = control.status.position.load(Ordering::Acquire);
            let mut points = vec![AutomationPoint {
                beat: BeatTime::ZERO,
                value: AutomationValue::new(mode as f32 / 2.0)?,
                outgoing: Curve::Step,
            }];
            for index in 0..=12 {
                points.push(AutomationPoint {
                    beat: BeatTime::from_ratio(position + 32 + index * 61, u64::from(RATE) / 2)?,
                    value: AutomationValue::new(if index == 12 {
                        mode as f32 / 2.0
                    } else {
                        (index % 3) as f32 / 2.0
                    })?,
                    outgoing: Curve::Step,
                });
            }
            e.execute(Command::ReplaceDeviceAutomation {
                device,
                parameter: MODE,
                points,
            })?;
            let rapid = prepare(e.project())?;
            let mut reference = hex_engine::SessionPlayback::new(Arc::clone(&rapid))?;
            reference.apply(hex_engine::SessionPlaybackCommand::Play)?;
            let mut prefix = vec![[0.0; 2]; position as usize];
            for block in prefix.chunks_mut(128) {
                reference.render(block)?;
            }
            let revision = control
                .publish(Playback::try_from_prepared_with_session(rapid)?)
                .map_err(|_| "rapid mode publish")?;
            let mut expected = [[0.0; 2]; BLOCK];
            for _ in 0..4 {
                reference.render(&mut expected)?;
                tick(&mut out);
                close(&out[0], &expected);
                assert!(control.status.playing.load(Ordering::Acquire));
                assert_eq!(
                    control.status.applied_revision.load(Ordering::Acquire),
                    revision
                );
                control.reclaim();
            }
            close(&out[0], &out[2]);
            for bypassed in [true, false] {
                e.execute(Command::SetDeviceBypass { device, bypassed })?;
                let revision = control
                    .publish(Playback::try_from_prepared_with_session(prepare(
                        e.project(),
                    )?)?)
                    .map_err(|_| "bypass")?;
                tick(&mut out);
                tick(&mut out);
                assert_eq!(
                    control.status.applied_revision.load(Ordering::Acquire),
                    revision
                );
                close(
                    &out[0][BLOCK - 64..],
                    &out[if bypassed { 3 } else { 2 }][BLOCK - 64..],
                );
                control.reclaim();
            }
            e.execute(Command::RemoveDevice(device))?;
            let revision = control
                .publish(Playback::try_from_prepared_with_session(prepare(
                    e.project(),
                )?)?)
                .map_err(|_| "device removal")?;
            tick(&mut out);
            assert_eq!(
                control.status.applied_revision.load(Ordering::Acquire),
                revision
            );
            assert!(out[0].iter().flatten().all(|x| x.is_finite()));
            control.reclaim();
        }
        for (owner, cb) in [
            (&mut control, &mut callback),
            (&mut old_control, &mut old_callback),
            (&mut canonical_control, &mut canonical_callback),
            (&mut raw_control, &mut raw_callback),
        ] {
            owner.request_shutdown();
            measured_shutdown(cb, &mut out[0]);
            owner.reclaim();
        }
    }
    Ok(())
}
