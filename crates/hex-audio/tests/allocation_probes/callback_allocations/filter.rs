//! Filter's modulated kernel and scratch history execute within callback bounds.
use super::*;
use hex_lattice_contract::{ParameterId, ParameterValue};

#[test]
fn modulated_filter_callback_is_allocation_free_and_reopens() -> Result<(), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(RATE)?, 8)?;
    let Created::Imported { track, asset, .. } =
        editor.execute(Command::ImportPreparedAudioTrack {
            name: "filter".into(),
            path: "memory/filter.wav".into(),
            source_sample_rate: RATE,
            source_frames: (BLOCK * 3) as u64,
            playback_frames: (BLOCK * 3) as u64,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })?
    else {
        return Err("expected audio track".into());
    };
    let mut state = hex_engine::default_native_device_state(
        hex_lattice_contract::DeviceTypeId::from_static("hex.filter"),
    )?;
    for (index, value) in [(7, 2.0), (8, 15.0), (10, 180.0), (11, -1.0)] {
        state = hex_engine::patch_native_device_parameter(
            &state,
            ParameterId::from_static(index),
            ParameterValue::Float(value),
        )?;
    }
    editor.execute(Command::InsertDevice {
        track,
        before: None,
        state,
    })?;
    assert!(editor.undo());
    assert!(editor.redo());
    let path = std::env::temp_dir().join(format!("hex-filter-{}.hex", std::process::id()));
    hex_storage::save_new(&path, editor.project())?;
    let reopened = hex_storage::load(&path);
    std::fs::remove_file(path)?;
    let reopened = reopened?;
    assert_eq!(&reopened, editor.project());
    let imported: Vec<_> = (0..BLOCK * 3)
        .map(|index| {
            let sample = (std::f32::consts::TAU * 3000.0 * index as f32 / RATE as f32).sin() * 0.5;
            [sample; 2]
        })
        .collect();
    let media = BTreeMap::from([(asset, Audio::new(RATE, imported)?)]);
    let prepared = Arc::new(PreparedRevision::prepare(
        &reopened,
        &media,
        Arc::new(SineTable::new()),
        None,
    )?);
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(Playback::try_from_prepared_with_session(prepared)?)
        .map_err(|_| "publication")?;
    control.command(PlaybackCommand::Play).map_err(|_| "play")?;
    let mut output = [[0.0; 2]; BLOCK];
    measured_block(&mut callback, &mut output);
    measured_block(&mut callback, &mut output);
    assert!(output.iter().flatten().all(|sample| sample.is_finite()));
    assert!(
        output
            .iter()
            .any(|frame| (frame[0] - frame[1]).abs() > 1e-5)
    );
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut output);
    control.reclaim();
    Ok(())
}
