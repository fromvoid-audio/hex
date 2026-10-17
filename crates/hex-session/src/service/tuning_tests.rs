use super::*;
use hex_tuning::{Audition, ProjectTuning};
use std::time::{Duration, Instant};

fn next(service: &Service) -> Event {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match service.try_event() {
            Ok(event) => return event,
            Err(mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(2))
            }
            Err(error) => panic!("worker did not reply: {error}"),
        }
    }
}
fn tuning(hz: f64) -> ProjectTuning {
    let mut scale = ProjectTuning::default().scale();
    scale.set_reference(60, hz, 0).unwrap();
    ProjectTuning::new(true, scale, Audition::default()).unwrap()
}
#[test]
fn discrete_tuning_edit_undo_redo_preserves_held_keyboard_lease_and_actual_phase() {
    let service = Service::new(48000, 16, 8_000_000).unwrap();
    let _ = next(&service);
    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "keys".into(),
        }))
        .unwrap();
    let added = next(&service);
    let track = added.state.project.tracks()[0].id();
    let input = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
    input.set_key(0, Some((60, 100, 1))).unwrap();
    service
        .submit(Request::PrepareComputerKeyboard {
            project_generation: 0,
            revision: added.state.revision,
            request_id: 1,
            track,
            input: Arc::clone(&input),
        })
        .unwrap();
    let mut source = next(&service).playback.unwrap();
    source.render(&mut [[0.0; 2]; 777]).unwrap();
    let mut phase = 0.0f64;
    let mut current_hz = 440.0 * 2.0f64.powf((60.0 - 69.0) / 12.0);
    // Independent phase oracle accumulates the effective frequency at accepted
    // publication boundaries, including Undo/Redo on the same physical lease.
    phase += current_hz * 777.0 / 48000.0;
    for (request, hz) in [
        (Request::Edit(Command::SetTuning(tuning(333.0))), 333.0),
        (Request::Undo, current_hz),
        (Request::Redo, 333.0),
    ] {
        service.submit(request).unwrap();
        let event = next(&service);
        assert!(event.error.is_none(), "{:?}", event.error);
        assert!(!input.is_cancelled());
        let mut target = event.playback.unwrap();
        target.try_inherit_runtime_from(&mut source).unwrap();
        source.cancel_computer_keyboard();
        drop(source);
        let mut actual = [[0.0; 2]; 137];
        target.render(&mut actual).unwrap();
        for (offset, frame) in actual.into_iter().enumerate() {
            let expected = ((std::f64::consts::TAU * (phase + hz * offset as f64 / 48000.0)).sin()
                * (100.0 / 127.0)
                * 0.1) as f32;
            assert!(
                (frame[0] - expected).abs() < 0.000002,
                "phase continuity {frame:?} vs {expected}"
            );
        }
        current_hz = hz;
        phase += current_hz * 137.0 / 48000.0;
        source = target;
    }
    service
        .submit(Request::Edit(Command::RenameTrack {
            track,
            name: "ordinary edit".into(),
        }))
        .unwrap();
    let ordinary = next(&service);
    assert!(ordinary.error.is_none());
    assert!(
        !input.is_cancelled(),
        "preparation must preserve the audible lease"
    );
    let mut replacement = ordinary.playback.unwrap();
    replacement.try_inherit_runtime_from(&mut source).unwrap();
    assert!(
        input.is_cancelled(),
        "ordinary edits still terminate the lease at publication"
    );
}
#[test]
fn control_preparation_refusal_preserves_canonical_history_and_keyboard_lease() {
    let service = Service::new(8000, 16, 8_000_000).unwrap();
    let _ = next(&service);
    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "keys".into(),
        }))
        .unwrap();
    let added = next(&service);
    let track = added.state.project.tracks()[0].id();
    let input = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
    service
        .submit(Request::PrepareComputerKeyboard {
            project_generation: 0,
            revision: added.state.revision,
            request_id: 1,
            track,
            input: Arc::clone(&input),
        })
        .unwrap();
    let _ = next(&service);
    // Entire native pitch domain is outside Nyquist: no keyboard candidate can
    // be prepared, although there are no authored notes to reject at compile.
    let mut scale = ProjectTuning::default().scale();
    scale.set_reference(0, 20000.0, 0).unwrap();
    service
        .submit(Request::Edit(Command::SetTuning(
            ProjectTuning::new(true, scale, Audition::default()).unwrap(),
        )))
        .unwrap();
    let refused = next(&service);
    assert!(refused.error.is_some());
    assert_eq!(refused.state.revision, added.state.revision);
    assert_eq!(refused.state.project, added.state.project);
    assert_eq!(refused.state.can_undo, added.state.can_undo);
    assert!(!input.is_cancelled());
}
