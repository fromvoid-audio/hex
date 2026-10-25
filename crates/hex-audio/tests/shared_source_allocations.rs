//! Shared source admission, receiver moves, Pause and scoped live retirement
//! through the real prepared callback. Construction and reclamation stay control-side.
#![cfg(feature = "allocation-probe")]
#[path = "../../hex-devices/tests/support/mod.rs"]
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
fn shared_mutable_receiver_handoff_pause_and_live_off_allocate_and_free_nothing_in_callback()
-> Result<(), Box<dyn Error>> {
    shared_source_callback(false)
}
#[test]
fn shared_analytic_receiver_handoff_pause_and_live_off_allocate_and_free_nothing_in_callback()
-> Result<(), Box<dyn Error>> {
    shared_source_callback(true)
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
    for track in [a, b] {
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
            pitch: 60,
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
    for step in 0..6 {
        match step {
            1 => {
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
            2 => control
                .command(PlaybackCommand::Pause)
                .map_err(|_| "command queue full")?,
            3 => {
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
            4 => input.set_key(0, None)?,
            5 => control.request_shutdown(),
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
        if step < 4 {
            assert!(
                output
                    .iter()
                    .flatten()
                    .any(|sample| sample.abs() > 0.000_001)
            );
        }
        if step == 5 {
            assert_eq!(output, [[0.0; 2]; 257]);
        }
        control.reclaim();
    }
    Ok(())
}
