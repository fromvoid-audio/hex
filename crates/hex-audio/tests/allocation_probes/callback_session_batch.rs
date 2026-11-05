//! Real compensated Session callback with allocation and independent PCM checks.
use super::callback_allocations::probe;
use hex_audio::prepared;
use hex_engine::{
    Audio, LaunchQuantization, PreparedRevision, SessionPlaybackCommand, SineTable,
    playback::Playback,
};
use hex_lattice::{NativeDeviceDefinition, NativeDeviceRegistry, NativeInstrumentRegistry};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{Command, Created, Editor, Project};
use std::{collections::BTreeMap, error::Error, sync::Arc};

#[test]
fn compensated_session_audio_batches_without_callback_allocations() -> Result<(), Box<dyn Error>> {
    const RATE: u32 = 48_000;
    const LATENCY: usize = 288;
    const SOURCE: [f32; 2] = [0.01, -0.02];
    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let Created::Scene(scene) = editor.execute(Command::AddScene {
        name: "session".into(),
    })?
    else {
        return Err("scene".into());
    };
    let mut media = BTreeMap::new();
    for index in 0..4 {
        let Created::Imported { track, asset, .. } =
            editor.execute(Command::ImportPreparedAudioTrack {
                name: format!("track {index}"),
                path: format!("memory/source-{index}.wav").into(),
                source_sample_rate: RATE,
                source_frames: 1024,
                playback_frames: 1024,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            })?
        else {
            return Err("track".into());
        };
        editor.execute(Command::CreateSessionAudioClip {
            track,
            scene,
            name: "clip".into(),
            asset,
            source_start: 0,
            source_end: 1024,
        })?;
        media.insert(asset, Audio::new(RATE, vec![SOURCE; 1024])?);
        if index == 0 {
            let limiter = hex_devices::limiter::DEFINITION.patch_parameter(
                &hex_devices::limiter::DEFINITION.default_state()?,
                hex_devices::limiter::parameter::LOOKAHEAD,
                ParameterValue::Choice(2),
            )?;
            editor.execute(Command::InsertDevice {
                track,
                before: None,
                state: limiter,
            })?;
        }
    }
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
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(Playback::try_from_prepared_with_session(revision)?)
        .map_err(|_| "publish")?;
    control
        .launch_and_play_with_ticket(SessionPlaybackCommand::LaunchScene {
            scene,
            quantization: LaunchQuantization::Immediate,
        })
        .map_err(|_| "launch")?;
    let mut output = [[0.0; 2]; 512];
    let mut offset = 0;
    for count in [1, 63, 257, 512] {
        probe::measured_block(&mut callback, &mut output[..count]);
        let expected: Vec<_> = (0..count)
            .map(|i| {
                if offset + i < LATENCY {
                    [0.0; 2]
                } else {
                    SOURCE.map(|sample| sample * 4.0)
                }
            })
            .collect();
        probe::compare_output(&output[..count], &expected);
        offset += count;
    }
    control.stop();
    probe::measured_shutdown(&mut callback, &mut output[..64]);
    control.reclaim();
    Ok(())
}
