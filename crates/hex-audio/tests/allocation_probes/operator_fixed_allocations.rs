//! Fixed tuning and velocity sensitivity prepare before admission; callback sampling and seek stay bounded.
use allocation_counter::measure;
use hex_audio::prepared;
use hex_audio::spectrum_worker::{SpectrumStamp, SpectrumTargetId, spawn_spectrum_worker};
use hex_engine::{
    PreparedRevision, SineTable,
    playback::{Playback, PlaybackCommand},
};
use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::{
    collections::BTreeMap,
    error::Error,
    sync::{Arc, atomic::Ordering},
    time::Instant,
};

#[test]
fn fixed_and_velocity_operator_render_seek_and_shutdown_allocate_nothing_on_callback()
-> Result<(), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(48_000)?, 16)?;
    let Created::Track(track) = editor.execute(Command::InsertTrackWithDevice {
        name: "fixed operator".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state: hex_project::default_instrument_state()?,
    })?
    else {
        return Err("track missing".into());
    };
    let device = editor
        .project()
        .track(track)
        .and_then(|track| track.instrument())
        .ok_or("instrument missing")?
        .id();
    let class = hex_engine::NativeDeviceClass::Instrument;
    let mut state = hex_engine::default_native_device_state_for_class(
        class,
        DeviceTypeId::from_static("hex.instrument.operator"),
    )?;
    for (id, value) in [
        (29, ParameterValue::Float(1.0)),
        (33, ParameterValue::Float(1.0)),
        (34, ParameterValue::Float(1.0)),
        (35, ParameterValue::Float(1.0)),
        (36, ParameterValue::Float(0.0)),
        (37, ParameterValue::Float(0.001)),
        (38, ParameterValue::Float(0.001)),
        (63, ParameterValue::Float(220.0)),
        (64, ParameterValue::Float(2.0)),
        (62, ParameterValue::Bool(true)),
        (57, ParameterValue::Choice(1)),
        (74, ParameterValue::Float(0.75)),
    ] {
        state = hex_engine::patch_native_device_parameter_for_class(
            class,
            &state,
            ParameterId::from_static(id),
            value,
        )?;
    }
    editor.execute(Command::ReplaceDeviceState { device, state })?;
    let Created::Clip(clip) = editor.execute(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::from_quarters(8)?,
    })?
    else {
        return Err("clip missing".into());
    };
    editor.execute(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::from_quarters(4)?,
        pitch: 60,
        channel: 0,
        velocity: 64,
    })?;
    let graph = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &BTreeMap::new(),
        Arc::new(SineTable::new()),
        None,
    )?);
    let playback = Playback::try_from_prepared_with_session(Arc::clone(&graph))?;
    let mut oracle = Playback::try_from_prepared_with_session(Arc::clone(&graph))?;
    let (mut control, mut callback) = prepared::channel(48_000)?;
    control
        .publish(playback)
        .map_err(|_| "publication refused")?;
    control
        .command(PlaybackCommand::Play)
        .map_err(|_| "play refused")?;
    oracle.apply(PlaybackCommand::Play);
    let mut actual = [[0.0; 2]; 128];
    let mut expected = [[0.0; 2]; 128];
    for destination in [None, Some(24_000), Some(0)] {
        if let Some(frame) = destination {
            control
                .command(PlaybackCommand::PlayFrom(frame))
                .map_err(|_| "seek refused")?;
            oracle.apply(PlaybackCommand::PlayFrom(frame));
        }
        oracle.render(&mut expected)?;
        let mut rendered = None;
        let counts = measure(|| {
            callback.begin_block();
            rendered = Some(callback.render(&mut actual));
        });
        rendered.ok_or("render not called")??;
        assert_eq!(counts.count_total, 0);
        assert_eq!(counts.bytes_total, 0);
        assert_eq!(counts.count_current, 0);
        assert_eq!(counts.bytes_current, 0);
        assert_eq!(actual, expected);
        assert!(actual.iter().flatten().any(|value| value.abs() > 1.0e-6));
    }
    control.request_shutdown();
    let counts = measure(|| {
        callback.begin_block();
        callback.render(&mut actual).expect("shutdown render");
    });
    assert_eq!(counts.count_total, 0);
    assert_eq!(counts.bytes_total, 0);
    assert_eq!(counts.count_current, 0);
    assert_eq!(counts.bytes_current, 0);
    control.reclaim();
    Ok(())
}

#[test]
fn registered_operator_lfo_free_sync_and_feedback_allocate_nothing_on_callback()
-> Result<(), Box<dyn Error>> {
    for sync in [false, true] {
        operator_modulation_allocations(Some(sync), false, false, true, false, None)?;
    }
    Ok(())
}

#[test]
fn registered_operator_feedback_voice_and_release_allocate_nothing_on_callback()
-> Result<(), Box<dyn Error>> {
    operator_modulation_allocations(None, false, false, true, false, None)
}

#[test]
fn registered_operator_filter_render_retarget_and_shutdown_allocate_nothing_on_callback()
-> Result<(), Box<dyn Error>> {
    for sync in [None, Some(false), Some(true)] {
        operator_modulation_allocations(sync, true, false, true, false, None)?;
    }
    Ok(())
}

#[test]
fn operator_filter_first_activation_import_allocates_and_deallocates_nothing()
-> Result<(), Box<dyn Error>> {
    for (sync, feedback) in [
        (None, false),
        (None, true),
        (Some(false), true),
        (Some(true), true),
    ] {
        operator_modulation_allocations(sync, true, true, feedback, false, None)?;
    }
    Ok(())
}

#[test]
fn registered_operator_analytic_enable_reversals_allocate_nothing_on_callback()
-> Result<(), Box<dyn Error>> {
    operator_modulation_allocations(None, false, false, false, false, None)
}

fn operator_modulation_allocations(
    lfo_sync: Option<bool>,
    filter: bool,
    activate_later: bool,
    feedback: bool,
    harmonics: bool,
    companion: Option<u32>,
) -> Result<(), Box<dyn Error>> {
    operator_modulation_with_volume(
        lfo_sync,
        filter,
        activate_later,
        feedback,
        harmonics,
        companion,
        false,
    )
}

#[test]
fn clip_volume_render_retarget_releases_and_shutdown_allocate_and_free_nothing()
-> Result<(), Box<dyn Error>> {
    for (sync, harmonics) in [(None, false), (Some(false), false), (Some(true), true)] {
        operator_modulation_with_volume(sync, true, true, true, harmonics, None, true)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)] // Existing fixture dimensions plus opt-in clip-volume owner.
fn operator_modulation_with_volume(
    lfo_sync: Option<bool>,
    filter: bool,
    activate_later: bool,
    feedback: bool,
    harmonics: bool,
    companion: Option<u32>,
    volume: bool,
) -> Result<(), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(48_000)?, 16)?;
    let Created::Track(track) = editor.execute(Command::InsertTrackWithDevice {
        name: "operator feedback allocation".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state: hex_project::default_instrument_state()?,
    })?
    else {
        return Err("track missing".into());
    };
    let device = editor
        .project()
        .track(track)
        .and_then(|track| track.instrument())
        .ok_or("instrument missing")?
        .id();
    let class = hex_engine::NativeDeviceClass::Instrument;
    let mut state = hex_engine::default_native_device_state_for_class(
        class,
        DeviceTypeId::from_static("hex.instrument.operator"),
    )?;
    for (id, value) in [
        (29, ParameterValue::Float(1.0)),
        (33, ParameterValue::Float(1.0)),
        (34, ParameterValue::Float(1.0)),
        (35, ParameterValue::Float(1.0)),
        (36, ParameterValue::Float(0.0)),
        (37, ParameterValue::Float(0.001)),
        (38, ParameterValue::Float(0.005)),
        (78, ParameterValue::Float(if feedback { 0.4 } else { 0.0 })),
    ] {
        state = hex_engine::patch_native_device_parameter_for_class(
            class,
            &state,
            ParameterId::from_static(id),
            value,
        )?;
    }
    if let Some(mode) = companion {
        for (id, value) in [
            (357, ParameterValue::Bool(false)),
            (358, ParameterValue::Float(0.125)),
            (359, ParameterValue::Bool(true)),
            (360, ParameterValue::Float(0.5)),
            (361, ParameterValue::Float(0.25)),
            (362, ParameterValue::Choice(mode)),
            (363, ParameterValue::Float(0.001)),
            (364, ParameterValue::Choice(0)),
        ] {
            state = hex_engine::patch_native_device_parameter_for_class(
                class,
                &state,
                ParameterId::from_static(id),
                value,
            )?;
        }
    }
    if filter && !activate_later {
        for (id, value) in [
            (20_001, ParameterValue::Bool(true)),
            (20_002, ParameterValue::Choice(4)),
            (20_005, ParameterValue::Float(800.0)),
            (20_006, ParameterValue::Float(75.0)),
            (20_007, ParameterValue::Float(35.0)),
        ] {
            state = hex_engine::patch_native_device_parameter_for_class(
                class,
                &state,
                ParameterId::from_static(id),
                value,
            )?;
        }
    }
    if let Some(sync) = lfo_sync {
        for (id, value) in [
            (82, ParameterValue::Bool(true)),
            (84, ParameterValue::Bool(sync)),
            (87, ParameterValue::Float(75.0)),
            (85, ParameterValue::Float(13.0)),
        ] {
            state = hex_engine::patch_native_device_parameter_for_class(
                class,
                &state,
                ParameterId::from_static(id),
                value,
            )?;
        }
    }
    editor.execute(Command::ReplaceDeviceState { device, state })?;
    let Created::Clip(clip) = editor.execute(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::from_quarters(8)?,
    })?
    else {
        return Err("clip missing".into());
    };
    let notes = if companion.is_some() { 64 } else { 1 };
    for index in 0..notes {
        editor.execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_ratio(1, 4)?,
            pitch: if companion.is_some() { 40 + index } else { 60 },
            channel: 0,
            velocity: 100,
        })?;
    }

    if volume {
        use hex_project::{MidiClipGain, MidiClipVolume, MidiClipVolumePoint};
        editor.execute(Command::ReplaceMidiClipVolume {
            clip,
            volume: MidiClipVolume::new(
                vec![
                    MidiClipVolumePoint::linear(BeatTime::ZERO, MidiClipGain::new(0.25)?),
                    MidiClipVolumePoint::linear(
                        BeatTime::from_ratio(1, 8)?,
                        MidiClipGain::new(0.75)?,
                    ),
                    MidiClipVolumePoint {
                        beat: BeatTime::from_ratio(1, 4)?,
                        incoming: MidiClipGain::new(0.5)?,
                        value: MidiClipGain::new(1.5)?,
                    },
                ],
                BeatTime::from_quarters(8)?,
            )?,
        })?;
        editor.execute(Command::SetMidiClipGain {
            clip,
            gain: hex_project::ClipGain::new(24_000)?,
        })?;
    }
    let mut revision = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &BTreeMap::new(),
        Arc::new(SineTable::new()),
        None,
    )?);
    let playback = Playback::try_from_prepared_with_session(Arc::clone(&revision))?;
    let mut oracle = Playback::try_from_prepared_with_session(Arc::clone(&revision))?;
    oracle.apply(PlaybackCommand::Play);
    let (mut control, mut callback) = prepared::channel(48_000)?;
    control
        .publish(playback)
        .map_err(|_| "publication refused")?;
    control
        .command(PlaybackCommand::Play)
        .map_err(|_| "play refused")?;
    let mut output = [[0.0; 2]; 128];
    let mut saw_active_feedback = false;
    for block in 0..80 {
        control.reclaim();
        let mut accepted = None;
        // Reversal occurs before the 240-frame coefficient transition finishes.
        // The second pair straddles note-off at frame 6000, covering release too.
        // Keep harmonic range reversals and LFO destination reversals distinct
        // when this fixture enables both; both release edits follow note-off.
        let lfo_release_edit = if harmonics { 50 } else { 48 };
        let edit = match block {
            5 | 46 => Some((513, ParameterValue::Bool(false))),
            6 | 47 => Some((513, ParameterValue::Bool(true))),
            2 | 45 if harmonics => Some((156, ParameterValue::Float(0.75))),
            3 | 48 if harmonics => Some((349, ParameterValue::Choice(0))),
            4 | 49 if harmonics => Some((349, ParameterValue::Choice(2))),
            7 if harmonics => Some((57, ParameterValue::Choice(0))),
            8 if harmonics => Some((57, ParameterValue::Choice(4))),
            at if lfo_sync.is_some() && (at == 10 || at == lfo_release_edit) => {
                Some((89, ParameterValue::Bool(false)))
            }
            at if lfo_sync.is_some() && (at == 11 || at == lfo_release_edit + 1) => {
                Some((89, ParameterValue::Bool(true)))
            }
            2..=4 if filter => Some((20005, ParameterValue::Float(200.0 * block as f32))),
            2..=4 if lfo_sync.is_some() => Some((87, ParameterValue::Float(20.0 * block as f32))),
            _ => None,
        };
        if let Some((parameter, value)) = edit {
            let current = editor
                .project()
                .track(track)
                .and_then(|t| t.instrument())
                .ok_or("instrument missing")?
                .state();
            let state = hex_engine::patch_native_device_parameter_for_class(
                class,
                current,
                ParameterId::from_static(parameter),
                value,
            )?;
            let state = if filter && activate_later && block == 2 {
                hex_engine::patch_native_device_parameter_for_class(
                    class,
                    &state,
                    ParameterId::from_static(20_001),
                    ParameterValue::Bool(true),
                )?
            } else {
                state
            };
            editor.execute(Command::ReplaceDeviceState { device, state })?;
            let next = Arc::new(PreparedRevision::prepare(
                editor.project(),
                &BTreeMap::new(),
                Arc::new(SineTable::new()),
                Some(revision.session()),
            )?);
            let candidate = Playback::try_from_prepared_with_session(Arc::clone(&next))?;
            let mut next_oracle = Playback::try_from_prepared_with_session(Arc::clone(&next))?;
            next_oracle
                .try_inherit_runtime_from(&mut oracle)
                .map_err(|_| "operator oracle transfer refused")?;
            accepted = Some(
                control
                    .publish_with_policy_and_context_generation(
                        candidate,
                        prepared::PublicationPolicy::InheritRuntime,
                        1,
                    )
                    .map_err(|e| e.reason)?,
            );
            oracle = next_oracle;
            revision = next;
        }
        let mut expected = [[0.0; 2]; 128];
        oracle.render(&mut expected)?;
        let counts = measure(|| {
            callback.begin_block();
            callback.render(&mut output).expect("operator render");
        });
        assert!(
            output == expected,
            "operator callback transfer PCM block={block} accepted={accepted:?} applied={}",
            control.status.applied_revision.load(Ordering::Acquire)
        );
        if let Some(accepted) = accepted {
            assert_eq!(
                control.status.applied_revision.load(Ordering::Acquire),
                accepted
            );
        }
        saw_active_feedback |= output.iter().any(|frame| frame[0].abs() > 1.0e-6);
        assert_eq!(counts.count_total, 0, "callback allocated");
        assert_eq!(counts.bytes_total, 0, "callback allocated bytes");
        assert_eq!(
            counts.count_current, 0,
            "callback changed live allocation count"
        );
        assert_eq!(
            counts.bytes_current, 0,
            "callback changed live allocation bytes"
        );
    }
    assert!(
        saw_active_feedback,
        "registered feedback runtime stayed silent"
    );
    control.request_shutdown();
    let counts = measure(|| {
        callback.begin_block();
        callback.render(&mut output).expect("shutdown render");
    });
    assert_eq!(counts.count_total, 0);
    assert_eq!(counts.bytes_total, 0);
    assert_eq!(counts.count_current, 0);
    assert_eq!(counts.bytes_current, 0);
    control.reclaim();
    Ok(())
}

#[test]
fn registered_eq8_effect_callback_allocates_and_deallocates_nothing() -> Result<(), Box<dyn Error>>
{
    let mut editor = Editor::new(Project::new(48_000)?, 16)?;
    let Created::Track(track) = editor.execute(Command::InsertTrackWithDevice {
        name: "eq eight allocation".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state: hex_project::default_instrument_state()?,
    })?
    else {
        return Err("track missing".into());
    };
    let class = hex_engine::NativeDeviceClass::Effect;
    let mut state = hex_engine::default_native_device_state_for_class(
        class,
        DeviceTypeId::from_static("hex.eq8"),
    )?;
    for (id, value) in [
        (1, ParameterValue::Bool(true)),
        (2, ParameterValue::Choice(3)),
        (3, ParameterValue::Float(1_200.0)),
        (4, ParameterValue::Float(8.0)),
        (5, ParameterValue::Float(1.2)),
    ] {
        state = hex_engine::patch_native_device_parameter_for_class(
            class,
            &state,
            ParameterId::from_static(id),
            value,
        )?;
    }
    let Created::Device(eq8_device) = editor.execute(Command::InsertDevice {
        track,
        before: None,
        state,
    })?
    else {
        return Err("eq8 device missing".into());
    };
    let Created::Clip(clip) = editor.execute(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::from_quarters(8)?,
    })?
    else {
        return Err("clip missing".into());
    };
    editor.execute(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::from_quarters(4)?,
        pitch: 60,
        channel: 0,
        velocity: 100,
    })?;

    let table = Arc::new(SineTable::new());
    let mut revision = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &BTreeMap::new(),
        Arc::clone(&table),
        None,
    )?);
    let playback = Playback::try_from_prepared_with_session(Arc::clone(&revision))?;
    let mut oracle = Playback::try_from_prepared_with_session(Arc::clone(&revision))?;
    oracle.apply(PlaybackCommand::Play);
    let initial_stamp = SpectrumStamp {
        target: SpectrumTargetId(0),
        generation: 0,
        sample_rate_hz: 48_000,
        applied_revision: 0,
    };
    let (capture, _reader, reaper) = spawn_spectrum_worker(initial_stamp)?;
    let (mut control, mut callback) = prepared::channel_with_spectrum(48_000, Some(capture))?;
    let activation_floor = control
        .publish_with_policy_and_context_generation(
            playback,
            hex_audio::prepared::PublicationPolicy::InheritRuntime,
            1,
        )
        .map_err(|_| "publication refused")?;
    let target_identity = hex_engine::PostNodeIdentity::new(
        hex_engine::ProcessorChainOwner::Track(track.value()),
        eq8_device.value(),
        DeviceTypeId::from_static("hex.eq8"),
    );
    control
        .command(PlaybackCommand::Play)
        .map_err(|_| "play refused")?;
    let mut output = [[0.0; 2]; 128];
    let mut expected = [[0.0; 2]; 128];
    for _ in 0..8 {
        oracle.render(&mut expected)?;
        let measured = measure(|| {
            callback.begin_block();
            callback.render(&mut output).expect("eq8 unobserved render");
        });
        assert_eq!(output, expected);
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0),
            "unobserved EQ8 callback allocation"
        );
    }
    control
        .spectrum_target
        .select(target_identity, 1, activation_floor, 48_000)
        .map_err(|_| "spectrum target refused")?;
    let mut active_output = false;
    let mut accepted_capture = false;
    let mut rejected_capture = false;
    for block_index in 0..48 {
        oracle.render(&mut expected)?;
        let measured = measure(|| {
            callback.begin_block();
            callback.render(&mut output).expect("eq8 render");
            let finite = output.iter().flatten().all(|sample| sample.is_finite());
            match callback.finish_spectrum_block(Instant::now(), finite && block_index != 47) {
                Some(
                    hex_audio::spectrum_worker::CaptureOutcome::Queued { .. }
                    | hex_audio::spectrum_worker::CaptureOutcome::QueueFull { .. },
                ) => {
                    accepted_capture = true;
                }
                Some(hex_audio::spectrum_worker::CaptureOutcome::Skipped { .. }) => {
                    rejected_capture = true;
                }
                _ => {}
            }
        });
        assert_eq!(
            output, expected,
            "spectrum observation changed rendered audio"
        );
        active_output |= output.iter().flatten().any(|sample| sample.abs() > 1.0e-6);
        assert_eq!(measured.count_total, 0, "callback allocation count");
        assert_eq!(measured.bytes_total, 0, "callback allocation bytes");
        assert_eq!(measured.count_current, 0, "callback net allocation count");
        assert_eq!(measured.bytes_current, 0, "callback net allocation bytes");
    }
    assert!(
        active_output,
        "registered EQ8 path produced no audible frames"
    );
    assert!(
        accepted_capture,
        "matching EQ8 context never submitted output"
    );
    assert!(
        rejected_capture,
        "invalid output did not advance the capture gap"
    );

    // Recompile and hand off several compatible EQ8 parameter edits while
    // playback is active. The callback owns only the bounded runtime transfer;
    // project editing, preparation, oracle work and graph reclamation stay on
    // the control side.
    for (parameter, value) in [
        (4, ParameterValue::Float(10.0)),
        (5, ParameterValue::Float(2.4)),
        (4, ParameterValue::Float(5.0)),
    ] {
        let track_snapshot = editor.project().track(track).ok_or("track missing")?;
        let current = track_snapshot
            .devices()
            .iter()
            .find(|device| device.id() == eq8_device)
            .ok_or("eq8 missing")?
            .state();
        let state = hex_engine::patch_native_device_parameter_for_class(
            class,
            current,
            ParameterId::from_static(parameter),
            value,
        )?;
        editor.execute(Command::ReplaceDeviceState {
            device: eq8_device,
            state,
        })?;
        let next_revision = Arc::new(PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::clone(&table),
            Some(revision.session()),
        )?);
        let candidate = Playback::try_from_prepared_with_session(Arc::clone(&next_revision))?;
        let mut oracle_candidate =
            Playback::try_from_prepared_with_session(Arc::clone(&next_revision))?;
        oracle_candidate
            .try_inherit_runtime_from(&mut oracle)
            .map_err(|_| "oracle runtime handoff refused")?;
        let accepted_revision = control
            .publish_with_policy_and_context_generation(
                candidate,
                hex_audio::prepared::PublicationPolicy::InheritRuntime,
                1,
            )
            .map_err(|error| error.reason)?;
        oracle_candidate.render(&mut expected)?;
        let measured = measure(|| {
            callback.begin_block();
            callback
                .render(&mut output)
                .expect("eq8 parameter handoff render");
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0),
            "compatible EQ8 parameter handoff callback allocation"
        );
        assert_eq!(
            control.status.applied_revision.load(Ordering::Acquire),
            accepted_revision
        );
        assert_eq!(output, expected, "compatible EQ8 handoff changed PCM");
        assert!(output.iter().flatten().all(|sample| sample.is_finite()));
        assert!(output.iter().flatten().any(|sample| sample.abs() > 1.0e-6));
        control.reclaim();
        oracle = oracle_candidate;
        revision = next_revision;
    }

    control.request_shutdown();
    let measured = measure(|| {
        callback.begin_block();
        callback.render(&mut output).expect("eq8 shutdown render");
    });
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
    control.reclaim();
    drop(callback);
    drop(control);
    reaper
        .shutdown_and_join()
        .map_err(|_| "spectrum worker did not join")?;
    Ok(())
}

#[test]
fn harmonic_first_paint_range_cache_builtin_reversal_and_release_allocate_nothing()
-> Result<(), Box<dyn Error>> {
    for filter in [false, true] {
        operator_modulation_allocations(None, filter, false, false, true, None)?;
        operator_modulation_allocations(None, filter, false, true, true, None)?;
        operator_modulation_allocations(Some(false), filter, false, true, true, None)?;
    }
    Ok(())
}

#[test]
fn advanced_operator_companions_keep_sixty_four_voice_callbacks_allocation_free()
-> Result<(), Box<dyn Error>> {
    // Covers held loop/sync, early Trigger completion followed by physical NoteOff,
    // free-running phase, all note modifiers, live spectrum reversals and release.
    for mode in [1, 3, 4] {
        operator_modulation_allocations(None, false, false, false, true, Some(mode))?;
    }
    Ok(())
}
