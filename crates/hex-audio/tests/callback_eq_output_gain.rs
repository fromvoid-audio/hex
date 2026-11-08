//! Real flexible-EQ output gain publication and callback allocation boundary.
#![cfg(feature = "allocation-probe")]
#[path = "allocation_probes/callback_allocations/probe.rs"]
mod probe;
use hex_audio::prepared;
use hex_engine::{
    Audio, PreparedRevision, SineTable,
    playback::{Playback, PlaybackCommand},
};
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationReadSelection, AutomationValue, BeatTime,
    Command, Created, Editor, Project,
};
use probe::{compare_output, measured_block, measured_shutdown};
use std::{
    collections::BTreeMap,
    error::Error,
    sync::{Arc, atomic::Ordering},
};
const RATE: u32 = 48000;
const BLOCK: usize = 1024;
#[test]
fn flexible_eq_output_gain_publication_reading_history_and_storage_allocate_nothing()
-> Result<(), Box<dyn Error>> {
    use hex_devices::eq::{EQ_DEFINITION as D, OUTPUT_GAIN};
    use hex_lattice_contract::NativeDeviceMetadata;
    let mut e = Editor::new(Project::new(RATE)?, 16)?;
    let Created::Imported { track, asset, .. } = e.execute(Command::ImportPreparedAudioTrack {
        name: "eq output".into(),
        path: "memory/eq-output.wav".into(),
        source_sample_rate: RATE,
        source_frames: (BLOCK * 12) as u64,
        playback_frames: (BLOCK * 12) as u64,
        conversion_version: hex_project::IDENTITY_CONVERSION,
    })?
    else {
        return Err("audio track".into());
    };
    let Created::Device(device) = e.execute(Command::InsertDevice {
        track,
        before: None,
        state: D.default_state()?,
    })?
    else {
        return Err("eq".into());
    };
    let media = BTreeMap::from([(asset, Audio::new(RATE, vec![[0.5, -0.25]; BLOCK * 12])?)]);
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
    for owner in [&mut control, &mut old_control] {
        owner
            .publish(Playback::try_from_prepared_with_session(prepare(
                e.project(),
            )?)?)
            .map_err(|_| "publish")?;
        owner.command(PlaybackCommand::Play).map_err(|_| "play")?;
    }
    let mut out = [[0.0; 2]; BLOCK];
    let mut old_out = out;
    measured_block(&mut callback, &mut out);
    measured_block(&mut old_callback, &mut old_out);
    compare_output(&out, &old_out);
    e.execute(Command::ReplaceDeviceAutomation {
        device,
        parameter: OUTPUT_GAIN,
        points: vec![AutomationPoint {
            beat: BeatTime::ZERO,
            value: AutomationValue::new(0.5 / 16.0)?,
            outgoing: AutomationInterpolation::Step,
        }],
    })?;
    let authored = e.project().snapshot();
    assert!(e.undo());
    assert!(e.redo());
    assert_eq!(e.project().snapshot(), authored);
    let path = std::env::temp_dir().join(format!("hex-eq-output-{}.hex", std::process::id()));
    hex_storage::save_new(&path, e.project())?;
    let loaded = hex_storage::load(&path);
    std::fs::remove_file(path)?;
    let loaded = loaded?;
    assert_eq!(&loaded, e.project());
    let revision = control
        .publish(Playback::try_from_prepared_with_session(prepare(&loaded)?)?)
        .map_err(|_| "automation publish")?;
    measured_block(&mut callback, &mut out);
    measured_block(&mut old_callback, &mut old_out);
    assert_eq!(
        control.status.applied_revision.load(Ordering::Acquire),
        revision
    );
    for (frame, old) in out[BLOCK - 64..].iter().zip(&old_out[BLOCK - 64..]) {
        for c in 0..2 {
            assert!((frame[c] - old[c] * 0.5).abs() < 1e-6);
        }
    }
    assert!(control.status.playing.load(Ordering::Acquire));
    control.reclaim();
    for (selection, ratio) in [
        (AutomationReadSelection::Base, 1.0),
        (AutomationReadSelection::Authored, 0.5),
    ] {
        e.execute(Command::SetDeviceAutomationReadSelection {
            device,
            target: hex_lattice_contract::DeviceAutomationTarget::Parameter(OUTPUT_GAIN),
            selection,
        })?;
        let revision = control
            .publish(Playback::try_from_prepared_with_session(prepare(
                e.project(),
            )?)?)
            .map_err(|_| "reading publish")?;
        for _ in 0..2 {
            measured_block(&mut callback, &mut out);
            measured_block(&mut old_callback, &mut old_out);
        }
        assert_eq!(
            control.status.applied_revision.load(Ordering::Acquire),
            revision
        );
        for (frame, old) in out.iter().zip(&old_out) {
            for c in 0..2 {
                assert!((frame[c] - old[c] * ratio).abs() < 1e-6);
            }
        }
        control.reclaim();
    }
    for bypassed in [true, false] {
        e.execute(Command::SetDeviceBypass { device, bypassed })?;
        control
            .publish(Playback::try_from_prepared_with_session(prepare(
                e.project(),
            )?)?)
            .map_err(|_| "bypass")?;
        measured_block(&mut callback, &mut out);
        assert!(out.iter().flatten().all(|v| v.is_finite()));
        control.reclaim();
    }
    e.execute(Command::RemoveDevice(device))?;
    control
        .publish(Playback::try_from_prepared_with_session(prepare(
            e.project(),
        )?)?)
        .map_err(|_| "removal")?;
    measured_block(&mut callback, &mut out);
    control.reclaim();
    for (owner, cb) in [
        (&mut control, &mut callback),
        (&mut old_control, &mut old_callback),
    ] {
        owner.request_shutdown();
        measured_shutdown(cb, &mut out);
        owner.reclaim();
    }
    Ok(())
}
