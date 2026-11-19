//! Real compensated Session callback with allocation and independent PCM checks.
use super::callback_allocations::probe;
use hex_audio::prepared;
use hex_engine::{
    Audio, LaunchQuantization, PreparedPlaybackObservation, PreparedRevision,
    SessionPlaybackCommand, SineTable,
    playback::{Playback, PlaybackCommand},
};
use hex_lattice::{NativeDeviceDefinition, NativeDeviceRegistry, NativeInstrumentRegistry};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{Command, Created, Editor, Project};
use std::{collections::BTreeMap, error::Error, sync::Arc};

#[test]
fn compensated_session_audio_batches_without_callback_allocations() -> Result<(), Box<dyn Error>> {
    check_compensated_audio_batch(true, false, false)
}

#[test]
fn compensated_dry_arrangement_tracks_batch_without_callback_allocations()
-> Result<(), Box<dyn Error>> {
    check_compensated_audio_batch(false, false, false)
}

#[test]
fn compensated_resident_return_sends_batch_without_callback_allocations()
-> Result<(), Box<dyn Error>> {
    check_compensated_audio_batch(false, true, false)
}

#[test]
fn compensated_session_return_sends_batch_without_callback_allocations()
-> Result<(), Box<dyn Error>> {
    check_compensated_audio_batch(true, true, false)
}

#[test]
fn compensated_nonunity_session_return_sends_batch_without_callback_allocations()
-> Result<(), Box<dyn Error>> {
    check_compensated_audio_batch(true, true, true)
}

#[test]
fn compensated_nonunity_return_sends_batch_without_callback_allocations()
-> Result<(), Box<dyn Error>> {
    check_compensated_audio_batch(false, true, true)
}

fn check_compensated_audio_batch(
    session: bool,
    return_latency: bool,
    nonunity: bool,
) -> Result<(), Box<dyn Error>> {
    const RATE: u32 = 48_000;
    const SOURCE: [f32; 2] = [0.01, -0.02];
    let (lookahead, latency) = if session { (2, 288) } else { (0, 72) };
    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let return_track = if return_latency {
        let Created::ReturnTrack(bus) = editor.execute(Command::AddReturnTrack {
            name: "return".into(),
        })?
        else {
            return Err("return track".into());
        };
        let limiter = hex_devices::limiter::DEFINITION.patch_parameter(
            &hex_devices::limiter::DEFINITION.default_state()?,
            hex_devices::limiter::parameter::LOOKAHEAD,
            ParameterValue::Choice(lookahead),
        )?;
        editor.execute(Command::InsertReturnDevice {
            return_track: bus,
            before: None,
            state: limiter,
        })?;
        Some(bus)
    } else {
        None
    };
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
        if nonunity {
            editor.execute(Command::Gain { track, gain: 0.5 })?;
        }
        if session {
            editor.execute(Command::CreateSessionAudioClip {
                track,
                scene,
                name: "clip".into(),
                asset,
                source_start: 0,
                source_end: 1024,
            })?;
        }
        media.insert(asset, Audio::new(RATE, vec![SOURCE; 1024])?);
        if let Some(bus) = return_track {
            editor.execute(Command::SetReturnSend {
                track,
                return_track: bus,
                gain: 0.25,
            })?;
        }
        if index == 0 && !return_latency {
            let limiter = hex_devices::limiter::DEFINITION.patch_parameter(
                &hex_devices::limiter::DEFINITION.default_state()?,
                hex_devices::limiter::parameter::LOOKAHEAD,
                ParameterValue::Choice(lookahead),
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
    assert_eq!(revision.arrangement().main_latency_frames(), latency as u32);
    let observation = PreparedPlaybackObservation::prepare_on_control(&revision)?;
    observation.set_render_work_enabled(true);
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(Playback::try_from_prepared_with_session(revision)?)
        .map_err(|_| "publish")?;
    if session {
        control
            .launch_and_play_with_ticket(SessionPlaybackCommand::LaunchScene {
                scene,
                quantization: LaunchQuantization::Immediate,
            })
            .map_err(|_| "launch")?;
    } else {
        control
            .command(PlaybackCommand::PlayFrom(0))
            .map_err(|_| "play")?;
    }
    let mut output = [[0.0; 2]; 512];
    let mut offset = 0;
    for count in [1, 63, 257, 512, 128] {
        probe::measured_block(&mut callback, &mut output[..count]);
        let expected: Vec<_> = (0..count)
            .map(|i| {
                if offset + i < latency {
                    [0.0; 2]
                } else {
                    SOURCE.map(|sample| {
                        sample
                            * if nonunity {
                                2.5
                            } else if return_latency {
                                5.0
                            } else {
                                4.0
                            }
                    })
                }
            })
            .collect();
        probe::compare_output(&output[..count], &expected);
        offset += count;
    }
    assert!(
        observation
            .render_work_meters()
            .all(|(_, meter)| meter.sample().is_some())
    );
    control.stop();
    probe::measured_shutdown(&mut callback, &mut output[..64]);
    control.reclaim();
    Ok(())
}
