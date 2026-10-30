//! Resident prepared callback probe for a real nonzero-latency stage clock.
//! The oracle uses content indices and the authored gain law, without a renderer
//! or envelope clock. Setup, oracle construction and reclamation are off callback.
use super::*;
use hex_lattice::{NativeDeviceDefinition, NativeDeviceRegistry, NativeInstrumentRegistry};
use hex_lattice_contract::{DeviceAutomationTarget, HostGain};
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationReadSelection, AutomationValue,
};

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
