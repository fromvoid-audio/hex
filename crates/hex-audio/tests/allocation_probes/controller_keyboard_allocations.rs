//! Actual callback publication preserves a held keyboard during controller activation.
use allocation_counter::measure;
use hex_audio::prepared::{self, PublicationPolicy};
use hex_engine::{
    NativeDeviceClass, PreparedRevision, SineTable, computer_keyboard::ComputerKeyboardInput,
    live_expression::LiveExpressionInput, playback::Playback,
};
use hex_lattice_contract::DeviceTypeId;
use hex_project::{Command, Created, Editor, Project};
use std::{collections::BTreeMap, error::Error, sync::Arc};

#[test]
fn held_keyboard_controller_publication_note_off_and_retirement_have_zero_callback_allocations()
-> Result<(), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(48_000)?, 8)?;
    let Created::Track(track) = editor.execute(Command::InsertTrackWithDevice {
        name: "vector keyboard".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state: hex_project::default_instrument_state()?,
    })?
    else {
        return Err("track missing".into());
    };
    let instrument = editor
        .project()
        .track(track)
        .and_then(|track| track.instrument())
        .ok_or("instrument missing")?
        .id();
    let state = hex_engine::default_native_device_state_for_class(
        NativeDeviceClass::Instrument,
        DeviceTypeId::from_static("hex.instrument.operator"),
    )?;
    editor.execute(Command::ReplaceDeviceState {
        device: instrument,
        state,
    })?;
    let revision = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &BTreeMap::new(),
        Arc::new(SineTable::new()),
        None,
    )?);
    let keys = Arc::new(ComputerKeyboardInput::default());
    let oracle_keys = Arc::new(ComputerKeyboardInput::default());
    for input in [&keys, &oracle_keys] {
        input.set_key(0, Some((60, 100, 1)))?;
    }
    let mut playback = Playback::try_from_prepared_with_session(Arc::clone(&revision))?;
    playback.prepare_computer_keyboard(editor.project(), track, Arc::clone(&keys))?;
    let mut oracle = Playback::try_from_prepared_with_session(Arc::clone(&revision))?;
    oracle.prepare_computer_keyboard(editor.project(), track, Arc::clone(&oracle_keys))?;
    let controllers = Arc::new(LiveExpressionInput::new(0, track, instrument));
    let (mut control, mut callback) = prepared::channel(48_000)?;
    control
        .publish_with_policy_and_context_generation(playback, PublicationPolicy::InheritRuntime, 0)
        .map_err(|error| error.reason)?;
    let mut actual = [[0.0; 2]; 257];
    let mut expected = actual;
    for step in 0..4 {
        if step == 1 {
            let mut candidate = Playback::try_from_prepared_with_session(Arc::clone(&revision))?;
            candidate.prepare_live_expression(editor.project(), Arc::clone(&controllers))?;
            candidate.prepare_computer_keyboard_for_controller_activation(
                editor.project(),
                track,
                Arc::clone(&keys),
            )?;
            control
                .publish_with_policy_and_context_generation(
                    candidate,
                    PublicationPolicy::InheritRuntime,
                    0,
                )
                .map_err(|error| error.reason)?;
            // Independent uninterrupted engine oracle: attach on control, keeping
            // its live keyboard history in place instead of publishing a graph.
            oracle.prepare_live_expression(editor.project(), Arc::clone(&controllers))?;
        } else if step == 2 {
            keys.set_key(0, None)?;
            oracle_keys.set_key(0, None)?;
        } else if step == 3 {
            keys.cancel();
            oracle_keys.cancel();
        }
        oracle.render(&mut expected)?;
        let mut rendered = None;
        let counts = measure(|| {
            callback.begin_block();
            rendered = Some(callback.render(&mut actual));
        });
        rendered.ok_or("callback did not render")??;
        assert_eq!(
            (
                counts.count_total,
                counts.bytes_total,
                counts.count_current,
                counts.bytes_current
            ),
            (0, 0, 0, 0),
            "step {step}"
        );
        assert_eq!(
            actual, expected,
            "held phase/envelope and original physical-key note off must survive controller publication"
        );
        if step < 3 {
            assert!(
                !keys.is_cancelled(),
                "successful controller activation must retain the keyboard lease"
            );
            assert!(actual.iter().flatten().any(|sample| *sample != 0.0));
        }
        control.reclaim(); // old graph destruction belongs to control
        if step < 3 {
            assert!(
                !keys.is_cancelled(),
                "retirement cannot cancel the new owner"
            );
        }
    }
    control.request_shutdown();
    let mut rendered = None;
    let counts = measure(|| {
        callback.begin_block();
        rendered = Some(callback.render(&mut actual));
    });
    rendered.ok_or("shutdown did not render")??;
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0)
    );
    control.reclaim();
    Ok(())
}
