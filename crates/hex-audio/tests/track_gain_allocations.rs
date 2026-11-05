//! Real callback handoff for static track-volume candidates; no UI or device I/O.
#![cfg(feature = "allocation-probe")]
use allocation_counter::measure;
use hex_audio::prepared::{self, PublicationPolicy};
use hex_engine::{
    PreparedRevision, SineTable,
    playback::{Playback, PlaybackCommand},
};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::{
    collections::BTreeMap,
    error::Error,
    sync::{Arc, atomic::Ordering},
};

fn prepare(editor: &Editor) -> Result<Arc<PreparedRevision>, Box<dyn Error>> {
    Ok(Arc::new(PreparedRevision::prepare(
        editor.project(),
        &BTreeMap::new(),
        Arc::new(SineTable::new()),
        None,
    )?))
}

#[test]
fn held_track_gain_candidates_inherit_running_voice_and_allocate_nothing_in_callback()
-> Result<(), Box<dyn Error>> {
    gain_callback(false, false, false)
}

#[test]
fn mutable_held_track_gain_candidates_inherit_running_voice_without_callback_allocation()
-> Result<(), Box<dyn Error>> {
    gain_callback(true, false, false)
}

#[test]
fn launched_session_gain_candidates_keep_phase_and_routes_without_callback_allocation()
-> Result<(), Box<dyn Error>> {
    gain_callback(false, true, false)?;
    gain_callback(true, true, false)
}

#[test]
fn soloed_muted_tracks_remain_audible_across_gain_handoffs_without_callback_allocation()
-> Result<(), Box<dyn Error>> {
    for mutable in [false, true] {
        for session in [false, true] {
            gain_callback(mutable, session, true)?;
        }
    }
    Ok(())
}

fn gain_callback(mutable: bool, session: bool, solo_muted: bool) -> Result<(), Box<dyn Error>> {
    let rate = 48_000;
    let mut editor = Editor::new(Project::new(rate)?, 16)?;
    let Created::Track(track) = editor.execute(Command::InsertTrackWithDevice {
        name: "held volume".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state: hex_project::default_instrument_state().expect("fixture instrument state"),
    })?
    else {
        return Err("track".into());
    };
    let (clip, launch) = if session {
        let Created::Scene(scene) = editor.execute(Command::AddScene {
            name: "held cell".into(),
        })?
        else {
            return Err("scene".into());
        };
        let Created::SessionClip(clip) = editor.execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "sustain".into(),
            source_length: BeatTime::from_quarters(16)?,
        })?
        else {
            return Err("session clip".into());
        };
        (
            clip,
            Some(hex_engine::SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: hex_engine::LaunchQuantization::Immediate,
            }),
        )
    } else {
        let Created::Clip(clip) = editor.execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(16)?,
        })?
        else {
            return Err("clip".into());
        };
        (clip, None)
    };
    editor.execute(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::from_quarters(16)?,
        pitch: 60,
        channel: 0,
        velocity: 100,
    })?;
    if mutable {
        let instrument = editor
            .project()
            .track(track)
            .ok_or("track")?
            .instrument()
            .ok_or("instrument")?
            .id();
        let mut state = hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Instrument,
            hex_lattice_contract::DeviceTypeId::from_static("hex.instrument.operator"),
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
                hex_lattice_contract::ParameterId::from_static(id),
                hex_lattice_contract::ParameterValue::Float(value),
            )?;
        }
        editor.execute(Command::ReplaceDeviceState {
            device: instrument,
            state,
        })?;
    }
    if solo_muted {
        editor.execute(Command::Mute { track, muted: true })?;
        editor.execute(Command::ToggleSolo {
            track,
            exclusive: true,
        })?;
        assert!(editor.project().track(track).ok_or("track")?.muted());
    }
    let revision = prepare(&editor)?;
    let mut oracle = Playback::try_from_prepared_with_session(Arc::clone(&revision))?;
    if let Some(launch) = launch {
        oracle
            .try_apply_action(hex_engine::PlaybackAction::LaunchAndPlay(launch))
            .map_err(|error| format!("oracle launch: {error:?}"))?;
    } else {
        oracle.apply(PlaybackCommand::Play);
    }
    let (mut control, mut callback) = prepared::channel(rate)?;
    control
        .publish_with_policy_and_context_generation(
            Playback::try_from_prepared_with_session(revision)?,
            PublicationPolicy::InheritRuntime,
            1,
        )
        .map_err(|_| "initial publication")?;
    if let Some(launch) = launch {
        control
            .launch_and_play_with_ticket(launch)
            .map_err(|_| "launch")?;
    } else {
        control.command(PlaybackCommand::Play).map_err(|_| "play")?;
    }
    let mut output = [[0.0; 2]; 128];
    let mut expected = [[0.0; 2]; 128];
    callback.begin_block();
    callback.render(&mut output)?;
    oracle.render(&mut expected)?;
    assert_eq!(output, expected);
    assert!(output.iter().flatten().any(|v| v.abs() > 1e-5));
    control.reclaim();
    for gain in [0.0, 1.0, 2.0, 0.25, 0.0, 0.75] {
        editor.execute(Command::Gain { track, gain })?;
        let revision = prepare(&editor)?;
        let mut next = Playback::try_from_prepared_with_session(Arc::clone(&revision))?;
        next.try_inherit_runtime_from(&mut oracle)
            .map_err(|e| format!("oracle inheritance: {e:?}"))?;
        oracle = next;
        oracle.render(&mut expected)?;
        let sequence = control
            .publish_with_policy_and_context_generation(
                Playback::try_from_prepared_with_session(revision)?,
                PublicationPolicy::InheritRuntime,
                1,
            )
            .map_err(|_| "preview publication")?;
        let mut rendered = None;
        let info = measure(|| {
            callback.begin_block();
            rendered = Some(callback.render(&mut output));
        });
        rendered.ok_or("missing render")??;
        assert_eq!(info.count_total, 0, "gain handoff allocated");
        assert_eq!(info.bytes_total, 0, "gain handoff allocated bytes");
        assert_eq!(
            info.count_current, 0,
            "gain handoff deallocated on callback"
        );
        assert_eq!(info.bytes_current, 0, "gain handoff changed live bytes");
        assert_eq!(
            control.status.applied_revision.load(Ordering::Acquire),
            sequence
        );
        assert_eq!(
            output, expected,
            "held voice must continue at its current frame, without restart"
        );
        assert!(output.iter().flatten().any(|v| v.abs() > 1e-5));
        control.reclaim();
    }
    // Let a final fade to zero finish, retaining the same allocation and
    // callback/reference checks used for the rapid retargets above.
    editor.execute(Command::Gain { track, gain: 0.0 })?;
    let revision = prepare(&editor)?;
    let mut next = Playback::try_from_prepared_with_session(Arc::clone(&revision))?;
    next.try_inherit_runtime_from(&mut oracle)
        .map_err(|e| format!("oracle inheritance: {e:?}"))?;
    oracle = next;
    control
        .publish_with_policy_and_context_generation(
            Playback::try_from_prepared_with_session(revision)?,
            PublicationPolicy::InheritRuntime,
            1,
        )
        .map_err(|_| "final publication")?;
    for block in 0..2 {
        let mut rendered = None;
        let info = measure(|| {
            callback.begin_block();
            rendered = Some(callback.render(&mut output));
        });
        rendered.ok_or("missing render")??;
        assert_eq!(
            (
                info.count_total,
                info.bytes_total,
                info.count_current,
                info.bytes_current
            ),
            (0, 0, 0, 0)
        );
        oracle.render(&mut expected)?;
        assert_eq!(output, expected);
        if block == 1 {
            assert!(output[112..].iter().flatten().all(|v| *v == 0.0));
        }
    }
    Ok(())
}
