//! Finite bounce uses the production mutable source-loop renderer without
//! allocating or retiring owners inside a render block. No device is opened.
use super::*;

#[test]
fn finite_mutable_source_bounce_render_blocks_are_allocation_and_deallocation_free()
-> Result<(), Box<dyn Error>> {
    let definition = MutableFixtureDefinition;
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions)?;
    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let Created::Track(track) = editor.execute(Command::InsertTrackWithDevice {
        name: "finite mutable".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state: hex_project::default_instrument_state()?,
    })?
    else {
        return Err("expected track".into());
    };
    let Created::Clip(clip) = editor.execute(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::from_quarters(4)?,
    })?
    else {
        return Err("expected clip".into());
    };
    editor.execute(Command::AddMidiNote {
        clip,
        start: BeatTime::from_quarters(1)?,
        duration: BeatTime::from_quarters(2)?,
        pitch: 60,
        channel: 0,
        velocity: 100,
    })?;
    editor.execute(Command::SetMidiClipSource {
        clip,
        source_length: BeatTime::from_quarters(4)?,
        source_start: BeatTime::ZERO,
        loop_start: BeatTime::from_quarters(1)?,
        loop_end: BeatTime::from_quarters(2)?,
        looping: true,
        length: BeatTime::from_quarters(4)?,
    })?;
    let instrument = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    editor.execute(Command::ReplaceDeviceState {
        device: instrument,
        state: hex_lattice_contract::NativeDeviceState::new(MUTABLE_FIXTURE_TYPE, 1, vec![1])?,
    })?;
    let revision = Arc::new(PreparedRevision::prepare_with_test_instrument_registry(
        editor.project(),
        &BTreeMap::new(),
        Arc::new(SineTable::new()),
        None,
        &registry,
    )?);
    let mut playback = hex_engine::SessionPlayback::new_finite_arrangement_bounce(
        revision,
        None,
        u64::from(RATE) * 2,
    )?;
    playback.apply(SessionPlaybackCommand::Play)?;
    let mut output = [[0.0; 2]; 128];
    let mut heard = false;
    for _ in 0..(RATE as usize * 2).div_ceil(output.len()) {
        let allocations = measure(|| playback.render(&mut output).unwrap());
        assert_eq!(allocations.count_total, 0);
        assert_eq!(allocations.bytes_total, 0);
        assert_eq!(allocations.count_current, 0);
        assert_eq!(allocations.bytes_current, 0);
        heard |= output.iter().flatten().any(|sample| sample.abs() > 0.0);
    }
    assert!(
        heard,
        "the allocation probe must exercise real sounding source loops"
    );
    Ok(())
}
