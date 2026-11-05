//! Canonical output-trim revisions through prepared publication and real callbacks.
use allocation_counter::measure;
use hex_audio::prepared::{self, PublicationPolicy};
use hex_engine::{
    Audio, Frame, PreparedRevision, SineTable,
    playback::{Playback, PlaybackCommand},
};
use hex_project::{Command, Created, Editor, Project};
use std::{
    collections::BTreeMap,
    error::Error,
    sync::{Arc, atomic::Ordering::Acquire},
};

fn render(callback: &mut prepared::Callback, pcm: &mut [Frame]) -> Result<(), Box<dyn Error>> {
    let mut outcome = None;
    let counts = measure(|| {
        callback.begin_block();
        outcome = Some(callback.render(pcm));
    });
    outcome.ok_or("no render")??;
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0)
    );
    Ok(())
}

#[test]
fn direct_utility_controls_ramp_for_five_ms_across_prepared_publication()
-> Result<(), Box<dyn Error>> {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    const SOURCE: Frame = [0.25, -0.125];
    const FRAMES: u64 = 4096;
    let neutral = UTILITY_DEFINITION.default_state()?;
    for rate in [48_000, 192_000] {
        let ramp_frames = rate / 200;
        for block in [1, 7, 64, 512] {
            let mut editor = Editor::new(Project::new(rate)?, 16)?;
            let Created::Imported { track, asset, .. } =
                editor.execute(Command::ImportAudioTrack {
                    name: "utility ramp source".into(),
                    path: "source.wav".into(),
                    sample_rate: rate,
                    frames: FRAMES,
                })?
            else {
                return Err("import".into());
            };
            let Created::Device(device) = editor.execute(Command::InsertDevice {
                track,
                before: None,
                state: neutral.clone(),
            })?
            else {
                return Err("device".into());
            };
            let media = BTreeMap::from([(asset, Audio::new(rate, vec![SOURCE; FRAMES as usize])?)]);
            let prepare = |editor: &Editor| -> Result<Playback, Box<dyn Error>> {
                let revision = PreparedRevision::prepare(
                    editor.project(),
                    &media,
                    Arc::new(SineTable::new()),
                    None,
                )?;
                Ok(Playback::try_from_prepared_with_session(Arc::new(
                    revision,
                ))?)
            };
            for (parameter, value, target) in [
                (
                    parameter::GAIN,
                    ParameterValue::Float(0.25),
                    [0.0625, -0.03125],
                ),
                (parameter::MUTE, ParameterValue::Bool(true), [0.0, 0.0]),
                (
                    parameter::CHANNEL,
                    ParameterValue::Choice(3),
                    [-0.125, 0.25],
                ),
            ] {
                editor.execute(Command::ReplaceDeviceState {
                    device,
                    state: neutral.clone(),
                })?;
                let (mut control, mut callback) = prepared::channel(rate)?;
                control
                    .publish(prepare(&editor)?)
                    .map_err(|_| "publish neutral")?;
                control.command(PlaybackCommand::Play).map_err(|_| "play")?;
                let mut first = [SOURCE];
                render(&mut callback, &mut first)?;
                assert_eq!(first, [SOURCE]);

                let state = UTILITY_DEFINITION.patch_parameter(&neutral, parameter, value)?;
                editor.execute(Command::ReplaceDeviceState { device, state })?;
                let revision = control
                    .publish(prepare(&editor)?)
                    .map_err(|_| "publish edit")?;
                let mut output = vec![[0.0; 2]; ramp_frames as usize + 1];
                for chunk in output.chunks_mut(block) {
                    render(&mut callback, chunk)?;
                }
                assert_eq!(control.status.applied_revision.load(Acquire), revision);
                assert_eq!(control.status.rejected_revision(), None);
                assert_eq!(output[ramp_frames as usize - 1..], [target; 2]);
                for (frame, actual) in output.iter().take(ramp_frames as usize).enumerate() {
                    let fraction = (frame + 1) as f32 / ramp_frames as f32;
                    for channel in 0..2 {
                        let expected =
                            SOURCE[channel] + (target[channel] - SOURCE[channel]) * fraction;
                        assert!(
                            (actual[channel] - expected).abs() < 1.0e-7,
                            "{rate} hz, block {block}, {parameter:?}, frame {frame}: {actual:?}"
                        );
                    }
                }
                control.reclaim();
                control.request_shutdown();
                render(&mut callback, &mut first)?;
                assert_eq!(control.reclaim(), 1);
            }
        }
    }
    Ok(())
}

#[test]
fn static_effect_output_trim_publication_preserves_pcm_and_allocates_nothing()
-> Result<(), Box<dyn Error>> {
    for block in [1_usize, 7, 64, 512] {
        let mut editor = Editor::new(Project::new(48000)?, 16)?;
        let Created::Imported { track, asset, .. } = editor.execute(Command::ImportAudioTrack {
            name: "source".into(),
            path: "source.wav".into(),
            sample_rate: 48000,
            frames: 131072,
        })?
        else {
            return Err("import".into());
        };
        let Created::Device(device) = editor.execute(Command::InsertDevice {
            track,
            before: None,
            state: hex_engine::default_native_device_state(hex_engine::DeviceTypeId::from_static(
                "hex.utility",
            ))?,
        })?
        else {
            return Err("device".into());
        };
        let media = BTreeMap::from([(asset, Audio::new(48000, vec![[0.25, -0.125]; 131072])?)]);
        let prepare = |e: &Editor| -> Result<Playback, Box<dyn Error>> {
            Ok(Playback::try_from_prepared_with_session(Arc::new(
                PreparedRevision::prepare(e.project(), &media, Arc::new(SineTable::new()), None)?,
            ))?)
        };
        let (mut control, mut callback) = prepared::channel(48000)?;
        control
            .publish_with_policy_and_context_generation(
                prepare(&editor)?,
                PublicationPolicy::InheritRuntime,
                1,
            )
            .map_err(|_| "publish")?;
        control.command(PlaybackCommand::Play).map_err(|_| "play")?;
        let mut pcm = vec![[0.0; 2]; block];
        for _ in 0..1024_usize.div_ceil(block) {
            render(&mut callback, &mut pcm)?;
        }
        control.reclaim();
        assert_eq!(*pcm.last().ok_or("empty")?, [0.25, -0.125]);
        let mut previous_gain = 1.0_f32;
        for (index, db) in [-6.0, 0.0, -12.0].into_iter().enumerate() {
            let state = editor
                .project()
                .device(device)
                .ok_or("device")?
                .state()
                .clone()
                .with_output_gain_db(db)?;
            editor.execute(Command::ReplaceDeviceState { device, state })?;
            let revision = control
                .publish_with_policy_and_context_generation(
                    prepare(&editor)?,
                    PublicationPolicy::InheritRuntime,
                    1,
                )
                .map_err(|_| "gain publication")?;
            let position = control.status.position.load(Acquire);
            let total = if index < 2 {
                73_usize.div_ceil(block)
            } else {
                512_usize.div_ceil(block)
            };
            let target = 10.0_f32.powf(db / 20.0);
            for n in 0..total {
                render(&mut callback, &mut pcm)?;
                assert_eq!(control.status.applied_revision.load(Acquire), revision);
                for (i, sample) in pcm.iter().enumerate() {
                    let elapsed = n * block + i + 1;
                    let gain = if elapsed >= 240 {
                        target
                    } else {
                        (f64::from(previous_gain)
                            + (f64::from(target) - f64::from(previous_gain)) * elapsed as f64
                                / 240.0) as f32
                    };
                    for channel in 0..2 {
                        let source = if channel == 0 { 0.25 } else { -0.125 };
                        assert!(
                            (sample[channel] - source * gain).abs() < 1e-7,
                            "block{block} gain{db} sample{elapsed}: {sample:?}"
                        );
                    }
                }
            }
            assert_eq!(
                control.status.position.load(Acquire),
                position + (total * block) as u64
            );
            previous_gain = pcm.last().ok_or("empty")?[0] * 4.0;
            control.reclaim();
        }
    }
    Ok(())
}

#[test]
fn effect_trim_keeps_mutable_note_phase_against_uninterrupted_playback_without_allocations()
-> Result<(), Box<dyn Error>> {
    use hex_lattice_contract::{ParameterId, ParameterValue};
    use hex_project::{BeatTime, TrackDeviceSlot};
    for block in [1_usize, 7, 64, 512] {
        let mut editor = Editor::new(Project::new(48000)?, 16)?;
        let mut state = hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Instrument,
            hex_engine::DeviceTypeId::from_static("hex.instrument.operator"),
        )?;
        for (id, value) in [
            (29, 1.0),
            (33, 1.0),
            (34, 1.0),
            (35, 1.0),
            (36, 0.0),
            (37, 0.001),
            (38, 0.005),
            (78, 0.4),
        ] {
            state = hex_engine::patch_native_device_parameter_for_class(
                hex_engine::NativeDeviceClass::Instrument,
                &state,
                ParameterId::from_static(id),
                ParameterValue::Float(value),
            )?;
        }
        let Created::Track(track) = editor.execute(Command::InsertTrackWithDevice {
            name: "held".into(),
            before_track: None,
            slot: TrackDeviceSlot::Instrument,
            state,
        })?
        else {
            return Err("track".into());
        };
        let Created::Clip(clip) = editor.execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(16)?,
        })?
        else {
            return Err("clip".into());
        };
        editor.execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_quarters(16)?,
            pitch: 60,
            channel: 0,
            velocity: 100,
        })?;
        let Created::Device(device) = editor.execute(Command::InsertDevice {
            track,
            before: None,
            state: hex_engine::default_native_device_state(hex_engine::DeviceTypeId::from_static(
                "hex.utility",
            ))?,
        })?
        else {
            return Err("effect".into());
        };
        let prepare = |e: &Editor| -> Result<Arc<PreparedRevision>, Box<dyn Error>> {
            Ok(Arc::new(PreparedRevision::prepare(
                e.project(),
                &BTreeMap::new(),
                Arc::new(SineTable::new()),
                None,
            )?))
        };
        let revision = prepare(&editor)?;
        let mut uninterrupted = Playback::try_from_prepared_with_session(Arc::clone(&revision))?;
        uninterrupted.apply(PlaybackCommand::Play);
        let (mut control, mut callback) = prepared::channel(48000)?;
        control
            .publish_with_policy_and_context_generation(
                Playback::try_from_prepared_with_session(revision)?,
                PublicationPolicy::InheritRuntime,
                1,
            )
            .map_err(|_| "publication")?;
        control.command(PlaybackCommand::Play).map_err(|_| "play")?;
        let mut actual = vec![[0.0; 2]; block];
        let mut expected = actual.clone();
        let mut remaining = 30000;
        while remaining > 0 {
            let n = remaining.min(block);
            render(&mut callback, &mut actual[..n])?;
            uninterrupted.render(&mut expected[..n])?;
            assert_eq!(actual[..n], expected[..n], "warm note phase");
            remaining -= n;
        }
        control.reclaim();
        assert_eq!(control.status.position.load(Acquire), 30000);
        let state = editor
            .project()
            .device(device)
            .ok_or("device")?
            .state()
            .clone()
            .with_output_gain_db(-6.0)?;
        editor.execute(Command::ReplaceDeviceState { device, state })?;
        let sequence = control
            .publish_with_policy_and_context_generation(
                Playback::try_from_prepared_with_session(prepare(&editor)?)?,
                PublicationPolicy::InheritRuntime,
                1,
            )
            .map_err(|_| "trim publication")?;
        let target = 10.0_f32.powf(-6.0 / 20.0);
        let mut offset = 0;
        let mut oracle_peak = 0.0_f32;
        while offset < 256 {
            let n = (256 - offset).min(block);
            render(&mut callback, &mut actual[..n])?;
            uninterrupted.render(&mut expected[..n])?;
            oracle_peak = expected[..n]
                .iter()
                .flatten()
                .fold(oracle_peak, |peak, value| peak.max(value.abs()));
            assert_eq!(control.status.applied_revision.load(Acquire), sequence);
            for i in 0..n {
                let elapsed = offset + i + 1;
                let gain = if elapsed >= 240 {
                    target
                } else {
                    (1.0 + (f64::from(target) - 1.0) * elapsed as f64 / 240.0) as f32
                };
                for channel in 0..2 {
                    assert!(
                        (actual[i][channel] - expected[i][channel] * gain).abs() < 1e-7,
                        "block{block} frame{elapsed}: actual{:?} oracle{:?} gain{gain}",
                        actual[i],
                        expected[i]
                    );
                }
            }
            offset += n;
        }
        assert!(oracle_peak > 1e-5, "audible held-note oracle");
        assert_eq!(control.status.position.load(Acquire), 30256);
        control.reclaim();
    }
    Ok(())
}
