//! Canonical output-trim revisions through prepared publication and real callbacks.
#![cfg(feature = "allocation-probe")]
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
