//! Resident prepared callback probe for a real nonzero-latency stage clock.
//! The oracle uses content indices and the authored gain law, without a renderer
//! or envelope clock. Setup, oracle construction and reclamation are off callback.
use super::*;
use hex_lattice::{NativeDeviceDefinition, NativeDeviceRegistry, NativeInstrumentRegistry};
use hex_lattice_contract::{DeviceAutomationTarget, HostGain};
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationReadSelection, AutomationValue,
};
use std::sync::atomic::Ordering;

#[test]
fn limiter_output_gain_uses_delayed_content_clock_without_callback_allocations()
-> Result<(), Box<dyn Error>> {
    const SOURCE_FRAMES: usize = 4096;
    const LATENCY: usize = 288;
    const STEP_CONTENT_FRAME: usize = 1500;
    const SOURCE: Frame = [0.01, -0.02];
    const OUTPUT_FRAMES: usize = SOURCE_FRAMES + LATENCY;

    let mut editor = Editor::new(Project::new(RATE)?, 8)?;
    let Created::Imported { track, asset, .. } =
        editor.execute(Command::ImportPreparedAudioTrack {
            name: "stage clock".into(),
            path: "memory/stage-clock.wav".into(),
            source_sample_rate: RATE,
            source_frames: SOURCE_FRAMES as u64,
            playback_frames: SOURCE_FRAMES as u64,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })?
    else {
        return Err("expected audio track".into());
    };
    let limiter = hex_devices::limiter::DEFINITION.patch_parameter(
        &hex_devices::limiter::DEFINITION.default_state()?,
        hex_devices::limiter::parameter::LOOKAHEAD,
        ParameterValue::Choice(2),
    )?;
    let Created::Device(device) = editor.execute(Command::InsertDevice {
        track,
        before: None,
        state: limiter,
    })?
    else {
        return Err("expected limiter".into());
    };
    let step_beat = BeatTime::from_raw(1 << 28);
    assert_eq!(
        editor.project().tempo_map().beat_to_frame(step_beat)?,
        STEP_CONTENT_FRAME as u64,
    );
    let attenuated = HostGain::from_millidb(-6000)?;
    editor.execute(Command::ReplaceDeviceAutomationTarget {
        device,
        target: DeviceAutomationTarget::HostOutputGain,
        points: [(BeatTime::ZERO, HostGain::ZERO), (step_beat, attenuated)]
            .into_iter()
            .map(|(beat, gain)| {
                Ok(AutomationPoint {
                    beat,
                    value: AutomationValue::new(gain.normalized())?,
                    outgoing: AutomationInterpolation::Step,
                })
            })
            .collect::<Result<Vec<_>, hex_project::Invalid>>()?,
    })?;
    let media = BTreeMap::from([(asset, Audio::new(RATE, vec![SOURCE; SOURCE_FRAMES])?)]);
    let definitions: [&dyn NativeDeviceDefinition; 1] = [&hex_devices::limiter::DEFINITION];
    let effects = NativeDeviceRegistry::new(&definitions)?;
    let instruments = NativeInstrumentRegistry::new(&[])?;
    let revision = Arc::new(PreparedRevision::prepare_with_test_registries(
        editor.project(),
        &media,
        Arc::new(SineTable::new()),
        None,
        &effects,
        &instruments,
    )?);
    assert_eq!(revision.arrangement().main_latency_frames(), LATENCY as u32);
    // Below the Limiter ceiling, each output sample is exactly the original
    // content delayed by 288 frames. The gain step belongs to content frame 1500, so its
    // first attenuated output must be frame 1788, independent of callback boundaries.
    let expected: [Frame; OUTPUT_FRAMES] = std::array::from_fn(|frame| {
        if frame < LATENCY {
            [0.0; 2]
        } else {
            let gain = if frame - LATENCY < STEP_CONTENT_FRAME {
                1.0
            } else {
                attenuated.linear()
            };
            SOURCE.map(|sample| sample * gain)
        }
    });
    let expected_base: [Frame; OUTPUT_FRAMES] =
        std::array::from_fn(|frame| if frame < LATENCY { [0.0; 2] } else { SOURCE });
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(Playback::try_from_prepared_with_session(revision)?)
        .map_err(|_| "stage clock publication failed")?;
    control
        .command(PlaybackCommand::Play)
        .map_err(|_| "stage clock Play admission failed")?;
    let mut output = [[0.0; 2]; 511];
    for (phase, selection) in [
        AutomationReadSelection::Authored,
        AutomationReadSelection::Base,
        AutomationReadSelection::Authored,
    ]
    .into_iter()
    .enumerate()
    {
        if phase != 0 {
            control
                .command(PlaybackCommand::Stop)
                .map_err(|_| "override Stop admission failed")?;
            measured_block(&mut callback, &mut output[..37]);
            control.reclaim();
            editor.execute(Command::SetDeviceAutomationReadSelection {
                device,
                target: DeviceAutomationTarget::HostOutputGain,
                selection,
            })?;
            let replacement = Arc::new(PreparedRevision::prepare_with_test_registries(
                editor.project(),
                &media,
                Arc::new(SineTable::new()),
                None,
                &effects,
                &instruments,
            )?);
            control
                .publish(Playback::try_from_prepared_with_session(replacement)?)
                .map_err(|_| "override publication failed")?;
        }
        for replay in [false, true] {
            if replay || phase != 0 {
                control
                    .command(PlaybackCommand::Seek(0))
                    .map_err(|_| "stage clock Seek admission failed")?;
                control
                    .command(PlaybackCommand::Play)
                    .map_err(|_| "stage clock replay admission failed")?;
            }
            let mut from = 0;
            for length in [1, 63, 257, 17, 511].into_iter().cycle() {
                if from == OUTPUT_FRAMES {
                    break;
                }
                let length = length.min(OUTPUT_FRAMES - from);
                measured_block(&mut callback, &mut output[..length]);
                if selection == AutomationReadSelection::Base {
                    compare_output(&output[..length], &expected_base[from..from + length]);
                } else {
                    compare_output(&output[..length], &expected[from..from + length]);
                }
                from += length;
            }
        }
    }
    control
        .command(PlaybackCommand::Stop)
        .map_err(|_| "stage clock Stop admission failed")?;
    measured_block(&mut callback, &mut output[..37]);
    compare_output(&output[..37], &[[0.0; 2]; 37]);
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut output[..37]);
    control.reclaim();
    Ok(())
}

#[test]
fn limiter_native_targets_publish_audibly_without_callback_allocations()
-> Result<(), Box<dyn Error>> {
    limiter_publication_case(LimiterPublication::Original)
}
#[test]
fn limiter_additional_value_targets_publish_audibly_without_callback_allocations()
-> Result<(), Box<dyn Error>> {
    limiter_publication_case(LimiterPublication::Additional)
}
#[test]
fn limiter_lookahead_publishes_queued_audio_at_fixed_latency_without_callback_allocations()
-> Result<(), Box<dyn Error>> {
    limiter_publication_case(LimiterPublication::Lookahead)
}
#[derive(Clone, Copy)]
enum LimiterPublication {
    Original,
    Additional,
    Lookahead,
}
fn limiter_publication_case(case: LimiterPublication) -> Result<(), Box<dyn Error>> {
    let original = matches!(case, LimiterPublication::Original);
    let lookahead = matches!(case, LimiterPublication::Lookahead);
    const BLOCK: usize = 1024;
    use hex_devices::limiter::{DEFINITION as D, parameter as p};
    use hex_lattice_contract::NativeDeviceMetadata;
    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let Created::Imported { track, asset, .. } =
        editor.execute(Command::ImportPreparedAudioTrack {
            name: "limiter automation".into(),
            path: "memory/limiter.wav".into(),
            source_sample_rate: RATE,
            source_frames: (BLOCK * 12) as u64,
            playback_frames: (BLOCK * 12) as u64,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })?
    else {
        return Err("audio track".into());
    };
    let Created::Device(device) = editor.execute(Command::InsertDevice {
        track,
        before: None,
        state: if lookahead {
            D.patch_parameter(&D.default_state()?, p::RELEASE, ParameterValue::Float(1.0))?
        } else {
            D.default_state()?
        },
    })?
    else {
        return Err("limiter".into());
    };
    if lookahead {
        editor.execute(Command::ReplaceDeviceAutomation {
            device,
            parameter: p::LOOKAHEAD,
            points: vec![AutomationPoint {
                beat: BeatTime::ZERO,
                value: AutomationValue::new(0.0)?,
                outgoing: AutomationInterpolation::Step,
            }],
        })?;
    }
    let media = BTreeMap::from([(
        asset,
        Audio::new(
            RATE,
            (0..BLOCK * 12)
                .map(|i| {
                    if lookahead && i % 400 < 5 {
                        [2.0, -1.0]
                    } else {
                        [0.1, -0.05]
                    }
                })
                .collect(),
        )?,
    )]);
    let table = Arc::new(SineTable::new());
    let prepare = |project: &Project| -> Result<_, Box<dyn Error>> {
        let revision = Arc::new(PreparedRevision::prepare(
            project,
            &media,
            Arc::clone(&table),
            None,
        )?);
        if lookahead && project.device(device).is_some() {
            assert_eq!(revision.arrangement().main_latency_frames(), 288);
        }
        Ok(revision)
    };
    let (mut control, mut callback) = prepared::channel(RATE)?;
    let (mut old_control, mut old_callback) = prepared::channel(RATE)?;
    for owner in [&mut control, &mut old_control] {
        owner
            .publish(Playback::try_from_prepared_with_session(prepare(
                editor.project(),
            )?)?)
            .map_err(|_| "publish")?;
        owner.command(PlaybackCommand::Play).map_err(|_| "play")?;
    }
    let mut out = [[0.0; 2]; BLOCK];
    let mut old_out = out;
    measured_block(&mut callback, &mut out);
    measured_block(&mut old_callback, &mut old_out);
    assert_eq!(out, old_out);
    let original_lanes: &[_] = if lookahead {
        &[(p::LOOKAHEAD, 1.0, 1.0)]
    } else if original {
        &[
            (p::GAIN, 0.5, 0.75),
            (p::CEILING, 1.0, 0.875),
            (p::RELEASE, 0.6, 0.4),
        ]
    } else {
        &[]
    };
    let extra_lanes: &[_] = if original || lookahead {
        &[]
    } else {
        &[
            (p::AUTO_RELEASE, 0.0, 1.0),
            (p::LINK, 0.2, 0.8),
            (p::MAXIMIZE, 0.0, 1.0),
            (p::THRESHOLD, 0.5, 0.25),
            (p::OUTPUT, 1.0, 0.625),
            (p::LINK_MID_SIDE, 0.0, 1.0),
        ]
    };
    for &(parameter, a, b) in original_lanes.iter().chain(extra_lanes) {
        editor.execute(Command::ReplaceDeviceAutomation {
            device,
            parameter,
            points: vec![
                AutomationPoint {
                    beat: BeatTime::ZERO,
                    value: AutomationValue::new(a)?,
                    outgoing: AutomationInterpolation::Linear,
                },
                AutomationPoint {
                    beat: editor
                        .project()
                        .tempo_map()
                        .frame_to_beat((BLOCK * 2) as u64)?,
                    value: AutomationValue::new(b)?,
                    outgoing: AutomationInterpolation::Step,
                },
            ],
        })?;
    }
    let authored = editor.project().snapshot();
    assert!(editor.undo());
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), authored);
    let path = std::env::temp_dir().join(format!(
        "hex-limiter-automation-{}-{original}-{lookahead}.hex",
        std::process::id()
    ));
    hex_storage::save_new(&path, editor.project())?;
    let loaded = hex_storage::load(&path);
    std::fs::remove_file(path)?;
    let loaded = loaded?;
    assert_eq!(&loaded, editor.project());
    let revision = control
        .publish(Playback::try_from_prepared_with_session(prepare(&loaded)?)?)
        .map_err(|_| "automation publication")?;
    measured_block(&mut callback, &mut out);
    measured_block(&mut old_callback, &mut old_out);
    assert_eq!(
        control.status.applied_revision.load(Ordering::Acquire),
        revision
    );
    if lookahead {
        assert_eq!(
            &out[..288],
            &old_out[..288],
            "queued old output survives warm-up"
        );
    }
    if !original && !lookahead {
        // Boolean lanes step at their authored endpoint. Cross that content
        // point before judging whether the maximize controls became audible.
        measured_block(&mut callback, &mut out);
        measured_block(&mut old_callback, &mut old_out);
    }
    assert!(out.iter().flatten().all(|v| v.is_finite()));
    assert!(
        out.iter()
            .zip(&old_out)
            .any(|(a, b)| (a[0] - b[0]).abs() > 1e-4)
    );
    control.reclaim();
    let targets: Vec<_> = editor
        .project()
        .device(device)
        .ok_or("limiter disappeared")?
        .automation()
        .iter()
        .map(|lane| lane.target)
        .collect();
    for selection in [
        AutomationReadSelection::Base,
        AutomationReadSelection::Authored,
    ] {
        for &target in &targets {
            editor.execute(Command::SetDeviceAutomationReadSelection {
                device,
                target,
                selection,
            })?;
        }
        let revision = control
            .publish(Playback::try_from_prepared_with_session(prepare(
                editor.project(),
            )?)?)
            .map_err(|_| "read selection publication")?;
        // Advance beyond the native gain ramp before judging the new reading.
        for _ in 0..2 {
            measured_block(&mut callback, &mut out);
            measured_block(&mut old_callback, &mut old_out);
        }
        assert_eq!(
            control.status.applied_revision.load(Ordering::Acquire),
            revision
        );
        let actual = out.last().ok_or("missing output")?[0];
        let base = old_out.last().ok_or("missing base output")?[0];
        match selection {
            AutomationReadSelection::Base => assert!((actual - base).abs() < 2e-4),
            AutomationReadSelection::Authored if lookahead => assert!(
                out.iter()
                    .zip(&old_out)
                    .any(|(a, b)| (a[0] - b[0]).abs() > 1e-3)
            ),
            AutomationReadSelection::Authored => assert!(actual > base + 1e-3),
        }
        assert!(out.iter().flatten().all(|v| v.is_finite()));
        control.reclaim();
    }
    for bypassed in [true, false] {
        editor.execute(Command::SetDeviceBypass { device, bypassed })?;
        control
            .publish(Playback::try_from_prepared_with_session(prepare(
                editor.project(),
            )?)?)
            .map_err(|_| "bypass publication")?;
        measured_block(&mut callback, &mut out);
        assert!(out.iter().flatten().all(|v| v.is_finite()));
        assert!(out.iter().flatten().any(|v| v.abs() > 0.0));
        control.reclaim();
    }
    editor.execute(Command::RemoveDevice(device))?;
    control
        .publish(Playback::try_from_prepared_with_session(prepare(
            editor.project(),
        )?)?)
        .map_err(|_| "removal publication")?;
    measured_block(&mut callback, &mut out);
    control.reclaim();
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut out);
    control.reclaim();
    old_control.request_shutdown();
    measured_shutdown(&mut old_callback, &mut old_out);
    old_control.reclaim();
    Ok(())
}
