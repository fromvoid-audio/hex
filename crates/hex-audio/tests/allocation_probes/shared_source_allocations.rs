//! Shared source admission, receiver moves, Pause and scoped live retirement
//! through the real prepared callback. Construction and reclamation stay control-side.
#[path = "../../../hex-devices/tests/support/mod.rs"]
mod support;
use hex_audio::prepared::{self, PublicationPolicy};
use hex_engine::{
    PreparedRevision, SineTable,
    computer_keyboard::ComputerKeyboardInput,
    playback::{Playback, PlaybackCommand},
};
use hex_lattice_contract::ParameterValue;
use hex_project::{BeatTime, Command, Created, Editor, Project, TrackId};
use std::{collections::BTreeMap, error::Error, sync::Arc};

fn prepared(project: &Project) -> Result<Arc<PreparedRevision>, Box<dyn Error>> {
    Ok(Arc::new(PreparedRevision::prepare(
        project,
        &BTreeMap::new(),
        Arc::new(SineTable::new()),
        None,
    )?))
}
fn playback(
    project: &Project,
    revision: Arc<PreparedRevision>,
    track: TrackId,
    input: Arc<ComputerKeyboardInput>,
) -> Result<Playback, Box<dyn Error>> {
    let mut playback = Playback::try_from_prepared_with_session(revision)?;
    playback.prepare_computer_keyboard(project, track, input)?;
    Ok(playback)
}

#[test]
fn compensated_shared_analytic_receiver_has_no_callback_allocations() -> Result<(), Box<dyn Error>>
{
    let mut editor = Editor::new(Project::new(48_000)?, 16)?;
    let state = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        hex_devices::SINE_INSTRUMENT_TYPE_ID,
    )?;
    let Created::NativeSource(source) = editor.execute(Command::CreateNativeSource {
        name: "shared".into(),
        devices: vec![state],
        instrument_index: 0,
    })?
    else {
        return Err("source".into());
    };
    let mut tracks = Vec::new();
    for pitch in [60, 67] {
        let Created::Track(track) = editor.execute(Command::AddMidiTrack {
            name: "midi".into(),
        })?
        else {
            return Err("track".into());
        };
        editor.execute(Command::BindTrackSource {
            track,
            source: Some(source),
        })?;
        let Created::Clip(clip) = editor.execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(4)?,
        })?
        else {
            return Err("clip".into());
        };
        editor.execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch,
            velocity: 96,
            channel: 0,
        })?;
        tracks.push(track);
    }
    editor.execute(Command::SetNativeSourceReceiver {
        source,
        track: Some(tracks[0]),
    })?;
    let limiter = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.limiter"),
    )?;
    editor.execute(Command::InsertDevice {
        track: tracks[0],
        before: None,
        state: limiter,
    })?;
    let revision = prepared(editor.project())?;
    let mut reference = hex_engine::SessionPlayback::new(Arc::clone(&revision))?;
    reference.apply(hex_engine::SessionPlaybackCommand::Play)?;
    let mut expected = [[0.0; 2]; 512];
    for frame in expected.chunks_mut(1) {
        reference.render(frame)?;
    }
    let observation = hex_engine::PreparedPlaybackObservation::prepare_on_control(&revision)?;
    observation.set_render_work_enabled(true);
    let (mut control, mut callback) = prepared::channel(48_000)?;
    control
        .publish(Playback::try_from_prepared_with_session(revision)?)
        .map_err(|error| error.reason)?;
    control
        .command(PlaybackCommand::Play)
        .map_err(|_| "command queue full")?;
    let mut output = [[0.0; 2]; 512];
    let mut result = None;
    let stats = allocation_counter::measure(|| {
        callback.begin_block();
        result = Some(callback.render(&mut output));
        callback.finish_gain_recording_quantum(true);
    });
    result.ok_or("render missing")??;
    assert_eq!(
        (
            stats.count_total,
            stats.bytes_total,
            stats.count_current,
            stats.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(output, expected);
    assert!(output.iter().flatten().any(|sample| *sample != 0.0));
    assert!(
        observation
            .render_work_meters()
            .any(|(_, meter)| meter.sample().is_some())
    );
    control.stop();
    control.reclaim();
    Ok(())
}

#[test]
fn shared_mutable_receiver_handoff_pause_and_live_off_allocate_and_free_nothing_in_callback()
-> Result<(), Box<dyn Error>> {
    shared_source_callback(false)
}
#[test]
fn shared_analytic_receiver_handoff_pause_and_live_off_allocate_and_free_nothing_in_callback()
-> Result<(), Box<dyn Error>> {
    shared_source_callback(true)
}
#[test]
fn shared_source_live_pressure_refusal_allocates_and_frees_nothing_in_callback()
-> Result<(), Box<dyn Error>> {
    shared_source_live_pressure(false)?;
    shared_source_live_pressure(true)
}

fn shared_source_live_pressure(simultaneous: bool) -> Result<(), Box<dyn Error>> {
    use hex_devices::operator_state::parameter;
    use hex_lattice_contract::ParameterId;

    let mut editor = Editor::new(Project::new(48_000)?, 32)?;
    let registry = hex_devices::native_instrument_registry()?;
    let state = registry.default_state(hex_devices::operator_state::OPERATOR_TYPE_ID)?;
    let state =
        registry.patch_parameter(&state, parameter::GLOBAL_MODE, ParameterValue::Choice(1))?;
    let mut state = registry.patch_parameter(
        &state,
        ParameterId::from_static(601),
        ParameterValue::Choice(7),
    )?;
    for operator in 1..=4 {
        state = registry.patch_parameter(
            &state,
            parameter::envelope(operator, parameter::EnvelopeField::Release),
            ParameterValue::Float(if simultaneous { 0.001 } else { 1.0 }),
        )?;
    }
    let Created::Track(receiver) = editor.execute(Command::InsertTrackWithDevice {
        name: "receiver".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state,
    })?
    else {
        return Err("receiver missing".into());
    };
    let source = editor
        .project()
        .track(receiver)
        .and_then(|track| track.source_id())
        .ok_or("source missing")?;
    let Created::Track(contributor) = editor.execute(Command::AddMidiTrack {
        name: "contributor".into(),
    })?
    else {
        return Err("contributor missing".into());
    };
    editor.execute(Command::BindTrackSource {
        track: contributor,
        source: Some(source),
    })?;
    for (track, pitches) in [
        (receiver, vec![60, 61]),
        (contributor, (0..45).collect::<Vec<u8>>()),
    ] {
        let Created::Clip(clip) = editor.execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })?
        else {
            return Err("clip missing".into());
        };
        for pitch in pitches {
            editor.execute(Command::AddMidiNote {
                clip,
                // A late peer occupies a physical slot in the first case;
                // simultaneous onsets leave it eligible for voice stealing.
                start: if !simultaneous && track == receiver {
                    BeatTime::from_ratio(1, 128)?
                } else {
                    BeatTime::ZERO
                },
                duration: if !simultaneous && track == receiver {
                    BeatTime::from_ratio(127, 128)?
                } else {
                    BeatTime::ONE
                },
                pitch,
                velocity: 100,
                channel: 0,
            })?;
        }
    }
    let Created::Scene(scene) = editor.execute(Command::AddScene {
        name: "pressure".into(),
    })?
    else {
        return Err("scene missing".into());
    };
    let Created::SessionClip(session_clip) = editor.execute(Command::CreateSessionMidiClip {
        track: receiver,
        scene,
        name: "pressure".into(),
        source_length: BeatTime::ONE,
    })?
    else {
        return Err("session clip missing".into());
    };
    for pitch in 60..78 {
        editor.execute(Command::AddMidiNote {
            clip: session_clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch,
            velocity: 100,
            channel: 0,
        })?;
    }
    let input = Arc::new(ComputerKeyboardInput::default());
    let initial = playback(
        editor.project(),
        prepared(editor.project())?,
        receiver,
        Arc::clone(&input),
    )?;
    let (mut control, mut callback) = prepared::channel(48_000)?;
    control
        .publish_with_policy_and_context_generation(initial, PublicationPolicy::InheritRuntime, 7)
        .map_err(|error| error.reason)?;
    control
        .command(PlaybackCommand::Play)
        .map_err(|_| "play queue full")?;
    let mut output = [[0.0; 2]; 256];
    let render = |callback: &mut prepared::Callback,
                  output: &mut [[f32; 2]; 256]|
     -> Result<(), Box<dyn Error>> {
        let mut result = None;
        let counts = allocation_counter::measure(|| {
            callback.begin_block();
            result = Some(callback.render(output));
        });
        result.ok_or("render missing")??;
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
    };
    render(&mut callback, &mut output)?;
    assert!(
        output
            .iter()
            .flatten()
            .any(|sample| sample.abs() > 0.000_001)
    );
    editor.execute(Command::Mute {
        track: contributor,
        muted: true,
    })?;
    let candidate = playback(
        editor.project(),
        prepared(editor.project())?,
        receiver,
        Arc::clone(&input),
    )?;
    let muted_revision = control
        .publish_with_policy_and_context_generation(candidate, PublicationPolicy::InheritRuntime, 7)
        .map_err(|error| error.reason)?;
    render(&mut callback, &mut output)?;
    assert_eq!(
        control
            .status
            .applied_revision
            .load(std::sync::atomic::Ordering::Acquire),
        muted_revision
    );
    assert!(
        output
            .iter()
            .flatten()
            .any(|sample| sample.abs() > 0.000_001),
        "the shared source must remain audible during the mute transition"
    );
    render(&mut callback, &mut output)?;
    if simultaneous {
        assert!(
            output
                .iter()
                .flatten()
                .all(|sample| sample.abs() <= 0.000_001),
            "stolen peer and faded contributors must be silent before live input"
        );
        input.set_key(0, Some((67, 100, 1)))?;
        render(&mut callback, &mut output)?;
        assert!(
            input.rejected(),
            "a live note with no reserved capacity must report refusal"
        );
        assert!(!input.is_cancelled());
        assert!(control.status.render_fault_snapshot().is_none());
        control.stop();
        render(&mut callback, &mut output)?;
        assert!(
            input.is_cancelled(),
            "stop must remain effective under pressure"
        );
        control.reclaim();
        return Ok(());
    }
    assert!(
        output
            .iter()
            .flatten()
            .any(|sample| sample.abs() > 0.000_001),
        "the peer must remain audible after the mute fade before live input"
    );
    for key in 0..17 {
        input.set_key(key, Some((67 + key as u8, 100, key as u64 + 1)))?;
    }
    render(&mut callback, &mut output)?;
    assert!(
        output
            .iter()
            .flatten()
            .any(|sample| sample.abs() > 0.000_001),
        "live input or the peer must remain audible before the Session launch"
    );
    assert!(input.rejected(), "live press must report capacity refusal");
    assert!(!input.is_cancelled());
    assert!(control.status.rejected_revision().is_none());
    assert!(control.status.render_fault_snapshot().is_none());
    let ticket = control
        .session_command_with_ticket(hex_engine::SessionPlaybackCommand::LaunchClip {
            clip: session_clip,
            quantization: hex_engine::LaunchQuantization::Immediate,
        })
        .map_err(|_| "session command queue full")?;
    render(&mut callback, &mut output)?;
    assert_eq!(
        control.status.rejected_action(),
        Some((
            ticket,
            hex_engine::playback::PlaybackActionRejection::SharedSourceVoiceCapacity
        ))
    );
    assert!(
        output
            .iter()
            .flatten()
            .any(|sample| sample.abs() > 0.000_001)
    );
    assert!(control.status.render_fault_snapshot().is_none());
    let mut queue_full = false;
    for _ in 0..1024 {
        if control.command(PlaybackCommand::Pause).is_err() {
            queue_full = true;
            break;
        }
    }
    assert!(queue_full, "command queue must reach capacity");
    control.stop();
    render(&mut callback, &mut output)?;
    assert!(input.is_cancelled(), "stop must cancel held live keys");
    assert_eq!(output, [[0.0; 2]; 256], "stop must silence the source");
    assert!(control.status.render_fault_snapshot().is_none());
    control.request_shutdown();
    render(&mut callback, &mut output)?;
    control.reclaim();
    Ok(())
}

#[test]
fn shared_source_higher_arrangement_peak_refuses_publication_without_callback_allocation()
-> Result<(), Box<dyn Error>> {
    use hex_devices::operator_state::parameter;
    use hex_engine::playback::PlaybackHandoffRejection;

    let mut editor = Editor::new(Project::new(48_000)?, 32)?;
    let registry = hex_devices::native_instrument_registry()?;
    let state = registry.default_state(hex_devices::operator_state::OPERATOR_TYPE_ID)?;
    let state =
        registry.patch_parameter(&state, parameter::GLOBAL_MODE, ParameterValue::Choice(1))?;
    let Created::Track(receiver) = editor.execute(Command::InsertTrackWithDevice {
        name: "receiver".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state,
    })?
    else {
        return Err("receiver missing".into());
    };
    let source = editor
        .project()
        .track(receiver)
        .and_then(|track| track.source_id())
        .ok_or("source missing")?;
    let Created::Track(contributor) = editor.execute(Command::AddMidiTrack {
        name: "contributor".into(),
    })?
    else {
        return Err("contributor missing".into());
    };
    editor.execute(Command::BindTrackSource {
        track: contributor,
        source: Some(source),
    })?;
    let mut receiver_clip = None;
    for track in [receiver, contributor] {
        let Created::Clip(clip) = editor.execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })?
        else {
            return Err("clip missing".into());
        };
        editor.execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            velocity: 100,
            channel: 0,
        })?;
        if track == receiver {
            receiver_clip = Some(clip);
        }
    }
    let input = Arc::new(ComputerKeyboardInput::default());
    let initial = playback(
        editor.project(),
        prepared(editor.project())?,
        receiver,
        Arc::clone(&input),
    )?;
    let (mut control, mut callback) = prepared::channel(48_000)?;
    let original_revision = control
        .publish_with_policy_and_context_generation(initial, PublicationPolicy::InheritRuntime, 7)
        .map_err(|error| error.reason)?;
    control
        .command(PlaybackCommand::Play)
        .map_err(|_| "play queue full")?;
    let mut output = [[0.0; 2]; 128];
    let render = |callback: &mut prepared::Callback,
                  output: &mut [[f32; 2]; 128]|
     -> Result<(), Box<dyn Error>> {
        let mut result = None;
        let counts = allocation_counter::measure(|| {
            callback.begin_block();
            result = Some(callback.render(output));
        });
        result.ok_or("render missing")??;
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
    };
    render(&mut callback, &mut output)?;
    for key in 0..16 {
        input.set_key(key, Some((60 + key as u8, 100, key as u64 + 1)))?;
    }
    render(&mut callback, &mut output)?;
    assert!(!input.rejected(), "the fixture must admit its live keys");
    let clip = receiver_clip.ok_or("receiver clip missing")?;
    for pitch in 0..50 {
        editor.execute(Command::AddMidiNote {
            clip,
            start: BeatTime::from_ratio(1, 16)?,
            duration: BeatTime::from_ratio(1, 2)?,
            pitch,
            velocity: 100,
            channel: 0,
        })?;
    }
    let candidate = playback(
        editor.project(),
        prepared(editor.project())?,
        receiver,
        Arc::clone(&input),
    )?;
    let candidate_revision = control
        .publish_with_policy_and_context_generation(candidate, PublicationPolicy::InheritRuntime, 7)
        .map_err(|error| error.reason)?;
    render(&mut callback, &mut output)?;
    assert_eq!(
        control.status.rejected_revision(),
        Some((
            candidate_revision,
            PlaybackHandoffRejection::SharedSourceVoiceCapacity
        ))
    );
    assert_eq!(
        control
            .status
            .applied_revision
            .load(std::sync::atomic::Ordering::Acquire),
        original_revision
    );
    assert!(!input.is_cancelled());
    assert!(
        output
            .iter()
            .flatten()
            .any(|sample| sample.abs() > 0.000_001)
    );
    assert!(control.status.render_fault_snapshot().is_none());
    control.stop();
    render(&mut callback, &mut output)?;
    control.request_shutdown();
    render(&mut callback, &mut output)?;
    control.reclaim();
    Ok(())
}

#[test]
fn shared_source_authored_release_pressure_refuses_publication_without_callback_allocation()
-> Result<(), Box<dyn Error>> {
    use hex_devices::operator_state::parameter;
    use hex_engine::playback::PlaybackHandoffRejection;
    let mut editor = Editor::new(Project::new(48_000)?, 32)?;
    let registry = hex_devices::native_instrument_registry()?;
    let mut state = registry.default_state(hex_devices::operator_state::OPERATOR_TYPE_ID)?;
    state = registry.patch_parameter(&state, parameter::GLOBAL_MODE, ParameterValue::Choice(1))?;
    for operator in 1..=4 {
        state = registry.patch_parameter(
            &state,
            parameter::envelope(operator, parameter::EnvelopeField::Release),
            ParameterValue::Float(1.0),
        )?;
    }
    let Created::Track(receiver) = editor.execute(Command::InsertTrackWithDevice {
        name: "receiver".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state,
    })?
    else {
        return Err("receiver missing".into());
    };
    let source = editor
        .project()
        .track(receiver)
        .and_then(|track| track.source_id())
        .ok_or("source missing")?;
    let Created::Track(contributor) = editor.execute(Command::AddMidiTrack {
        name: "contributor".into(),
    })?
    else {
        return Err("contributor missing".into());
    };
    editor.execute(Command::BindTrackSource {
        track: contributor,
        source: Some(source),
    })?;
    let mut contributor_clip = None;
    for track in [receiver, contributor] {
        let Created::Clip(clip) = editor.execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })?
        else {
            return Err("clip missing".into());
        };
        let count = if track == contributor { 48 } else { 1 };
        for pitch in 0..count {
            editor.execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch,
                velocity: 100,
                channel: 0,
            })?;
        }
        if track == contributor {
            contributor_clip = Some(clip);
        }
    }
    let initial = Playback::try_from_prepared_with_session(prepared(editor.project())?)?;
    let (mut control, mut callback) = prepared::channel(48_000)?;
    let original_revision = control.publish(initial).map_err(|error| error.reason)?;
    control
        .command(PlaybackCommand::Play)
        .map_err(|_| "play queue full")?;
    let mut output = [[0.0; 2]; 128];
    let render = |callback: &mut prepared::Callback,
                  output: &mut [[f32; 2]; 128]|
     -> Result<(), Box<dyn Error>> {
        let mut result = None;
        let counts = allocation_counter::measure(|| {
            callback.begin_block();
            result = Some(callback.render(output));
        });
        result.ok_or("render missing")??;
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
    };
    render(&mut callback, &mut output)?;
    assert!(
        output
            .iter()
            .flatten()
            .any(|sample| sample.abs() > 0.000_001)
    );
    let clip = contributor_clip.ok_or("contributor clip missing")?;
    let notes: Vec<_> = editor
        .project()
        .midi_source(clip)
        .ok_or("source missing")?
        .notes()
        .iter()
        .map(|note| note.id())
        .collect();
    if notes.len() != 48 {
        return Err("fixture did not create 48 authored notes".into());
    }
    for note in notes {
        editor.execute(Command::RemoveMidiNote(note))?;
    }
    for pitch in 32..80 {
        editor.execute(Command::AddMidiNote {
            clip,
            start: BeatTime::from_ratio(1, 16)?,
            duration: BeatTime::from_ratio(1, 2)?,
            pitch,
            velocity: 100,
            channel: 0,
        })?;
    }
    let candidate = Playback::try_from_prepared_with_session(prepared(editor.project())?)?;
    let candidate_revision = control.publish(candidate).map_err(|error| error.reason)?;
    render(&mut callback, &mut output)?;
    assert_eq!(
        control.status.rejected_revision(),
        Some((
            candidate_revision,
            PlaybackHandoffRejection::SharedSourceVoiceCapacity
        ))
    );
    assert_eq!(
        control
            .status
            .applied_revision
            .load(std::sync::atomic::Ordering::Acquire),
        original_revision
    );
    assert!(
        output
            .iter()
            .flatten()
            .any(|sample| sample.abs() > 0.000_001)
    );
    assert!(control.status.render_fault_snapshot().is_none());
    control.stop();
    render(&mut callback, &mut output)?;
    control.request_shutdown();
    render(&mut callback, &mut output)?;
    control.reclaim();
    Ok(())
}

fn shared_source_callback(analytic: bool) -> Result<(), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(48_000)?, 32)?;
    let Created::Track(a) = editor.execute(Command::InsertTrackWithDevice {
        name: "source".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state: hex_project::default_instrument_state()?,
    })?
    else {
        return Err("source track missing".into());
    };
    let device = editor
        .project()
        .track(a)
        .ok_or("track missing")?
        .instrument()
        .ok_or("instrument missing")?
        .id();
    let registry = hex_devices::native_instrument_registry()?;
    let mut state = if analytic {
        support::legacy_operator_state()
    } else {
        registry.default_state(hex_devices::operator_state::OPERATOR_TYPE_ID)?
    };
    for operator in 1..=4 {
        state = registry.patch_parameter(
            &state,
            hex_devices::operator_state::parameter::envelope(
                operator,
                hex_devices::operator_state::parameter::EnvelopeField::Release,
            ),
            ParameterValue::Float(0.001),
        )?;
    }
    editor.execute(Command::ReplaceDeviceState { device, state })?;
    let source = editor
        .project()
        .track(a)
        .and_then(|track| track.source_id())
        .ok_or("source missing")?;
    let Created::Track(b) = editor.execute(Command::AddMidiTrack {
        name: "contributor".into(),
    })?
    else {
        return Err("contributor missing".into());
    };
    editor.execute(Command::BindTrackSource {
        track: b,
        source: Some(source),
    })?;
    for (track, pitch) in [(a, 60), (b, 67)] {
        let Created::Clip(clip) = editor.execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(4)?,
        })?
        else {
            return Err("clip missing".into());
        };
        editor.execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_quarters(3)?,
            pitch,
            velocity: 100,
            channel: 0,
        })?;
    }
    let input = Arc::new(ComputerKeyboardInput::default());
    input.set_key(0, Some((64, 100, 1)))?;
    let revision = prepared(editor.project())?;
    assert_eq!(
        !revision.has_arrangement_mutable_instruments(),
        analytic,
        "exercise the intended native provider class"
    );
    let initial = playback(editor.project(), revision, b, Arc::clone(&input))?;
    let (mut control, mut callback) = prepared::channel(48_000)?;
    control
        .publish_with_policy_and_context_generation(initial, PublicationPolicy::InheritRuntime, 7)
        .map_err(|error| error.reason)?;
    control
        .command(PlaybackCommand::Play)
        .map_err(|_| "command queue full")?;
    let mut output = [[0.0; 2]; 257];
    for step in 0..10 {
        let mut published = None;
        match step {
            1 => {
                editor.execute(Command::Mute {
                    track: b,
                    muted: true,
                })?;
                let candidate = playback(
                    editor.project(),
                    prepared(editor.project())?,
                    b,
                    Arc::clone(&input),
                )?;
                control
                    .publish_with_policy_and_context_generation(
                        candidate,
                        PublicationPolicy::InheritRuntime,
                        7,
                    )
                    .map_err(|error| error.reason)?;
            }
            2 => {
                editor.execute(Command::Mute {
                    track: b,
                    muted: false,
                })?;
                let candidate = playback(
                    editor.project(),
                    prepared(editor.project())?,
                    b,
                    Arc::clone(&input),
                )?;
                control
                    .publish_with_policy_and_context_generation(
                        candidate,
                        PublicationPolicy::InheritRuntime,
                        7,
                    )
                    .map_err(|error| error.reason)?;
            }
            3 => {
                // The earlier mute released the old contributor voice. Start
                // a fresh peer after moving the receiver so that the following
                // mute and unmute must preserve a physically sounding voice.
                input.set_key(0, None)?;
                input.set_key(1, Some((67, 100, 2)))?;
                editor.execute(Command::SetNativeSourceReceiver {
                    source,
                    track: Some(b),
                })?;
                let candidate = playback(
                    editor.project(),
                    prepared(editor.project())?,
                    b,
                    Arc::clone(&input),
                )?;
                control
                    .publish_with_policy_and_context_generation(
                        candidate,
                        PublicationPolicy::InheritRuntime,
                        7,
                    )
                    .map_err(|error| error.reason)?;
            }
            4 | 5 => {
                editor.execute(Command::Mute {
                    track: a,
                    muted: step == 4,
                })?;
                let candidate = playback(
                    editor.project(),
                    prepared(editor.project())?,
                    b,
                    Arc::clone(&input),
                )?;
                published = Some(
                    control
                        .publish_with_policy_and_context_generation(
                            candidate,
                            PublicationPolicy::InheritRuntime,
                            7,
                        )
                        .map_err(|error| error.reason)?,
                );
            }
            6 => control
                .command(PlaybackCommand::Pause)
                .map_err(|_| "command queue full")?,
            7 => {
                editor.execute(Command::BindTrackSource {
                    track: a,
                    source: None,
                })?;
                let candidate = playback(
                    editor.project(),
                    prepared(editor.project())?,
                    b,
                    Arc::clone(&input),
                )?;
                control
                    .publish_with_policy_and_context_generation(
                        candidate,
                        PublicationPolicy::InheritRuntime,
                        7,
                    )
                    .map_err(|error| error.reason)?;
            }
            8 => input.set_key(1, None)?,
            9 => control.request_shutdown(),
            _ => {}
        }
        let mut result = None;
        let stats = allocation_counter::measure(|| {
            callback.begin_block();
            result = Some(callback.render(&mut output));
        });
        result.ok_or("render missing")??;
        assert_eq!(
            (
                stats.count_total,
                stats.bytes_total,
                stats.count_current,
                stats.bytes_current
            ),
            (0, 0, 0, 0),
            "step {step}: {stats:?}"
        );
        // Step 7 unbinds the source under a held live key. Its history cannot
        // transfer, so the live handoff restarts audio (Pulse #3784) and the
        // held voice is cut until the key is pressed again; it must still be
        // applied, not refused, and allocate nothing.
        if [1, 2, 4, 5, 7].contains(&step) {
            assert!(
                control.status.rejected_revision().is_none(),
                "step {step}: {:?}",
                control.status.rejected_revision()
            );
        }
        if let Some(revision) = published {
            assert_eq!(
                control
                    .status
                    .applied_revision
                    .load(std::sync::atomic::Ordering::Acquire),
                revision
            );
        }
        assert!(
            control.status.render_fault_snapshot().is_none(),
            "step {step}: {:?}",
            control.status.render_fault_snapshot()
        );
        if step < 6 {
            assert!(
                output
                    .iter()
                    .flatten()
                    .any(|sample| sample.abs() > 0.000_001),
                "step {step}"
            );
        }
        if step == 9 {
            assert_eq!(output, [[0.0; 2]; 257]);
        }
        control.reclaim();
    }
    Ok(())
}
