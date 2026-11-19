use super::*;
use hex_tuning::{Audition, ProjectTuning};

fn stale_library_write(export: bool) {
    let directory = library_scratch();
    let mut session = Session::new(48_000, 8, 1_000_000).unwrap();
    let original = tuning(333.0).scale();
    hex_storage::tuning_library::save(&directory.0, 1, "original", original).unwrap();
    let slot = directory.0.join("slot-1.freqs");
    let original_bytes = std::fs::read(&slot).unwrap();
    for (generation, revision) in [(1, 0), (0, 1)] {
        let path = directory.0.join("export.freqs");
        let operation = if export {
            TuningLibraryOperation::Export {
                path: path.clone(),
                name: "export".into(),
            }
        } else {
            TuningLibraryOperation::Save {
                slot: 1,
                name: "replacement".into(),
            }
        };
        let (result, prepared) = run_tuning_library_operation_in(
            &mut session,
            TuningLibraryContext {
                request_generation: 0,
                request_revision: 0,
                operation_id: 1,
                current_generation: generation,
                current_revision: revision,
            },
            operation,
            None,
            &directory.0,
        );
        assert!(result.error.is_some(), "stale save/export must be refused");
        assert!(prepared.is_none());
        assert!(!path.exists(), "stale export must create no file");
        assert_eq!(std::fs::read(&slot).unwrap(), original_bytes);
    }
}

#[test]
fn review_stale_tuning_save_preserves_existing_slot_after_revision_or_generation_change() {
    stale_library_write(false);
}

#[test]
fn review_stale_tuning_export_creates_no_file_after_revision_or_generation_change() {
    stale_library_write(true);
}

#[test]
fn existing_tuning_recovery_does_not_block_slot_save() {
    let directory = library_scratch();
    let mut session = Session::new(48_000, 8, 1_000_000).unwrap();
    hex_storage::tuning_library::save(&directory.0, 1, "original", tuning(333.0).scale()).unwrap();
    for number in 1..=32 {
        std::fs::write(
            directory
                .0
                .join(format!("slot-1-recovery-{number:02}.freqs")),
            b"preserve",
        )
        .unwrap();
    }
    let path = directory.0.join("slot-1.freqs");
    let (result, prepared) = run_tuning_library_operation_in(
        &mut session,
        TuningLibraryContext {
            request_generation: 0,
            request_revision: 0,
            operation_id: 1,
            current_generation: 0,
            current_revision: 0,
        },
        TuningLibraryOperation::Save {
            slot: 1,
            name: "new".into(),
        },
        None,
        &directory.0,
    );
    assert!(result.error.is_none(), "{:?}", result.error);
    assert!(prepared.is_none());
    assert_eq!(
        hex_storage::tuning_library::read_file(&path).unwrap().name,
        "new"
    );
    assert_eq!(result.slots[0].name.as_deref(), Some("new"));
    for number in 1..=32 {
        assert_eq!(
            std::fs::read(
                directory
                    .0
                    .join(format!("slot-1-recovery-{number:02}.freqs"))
            )
            .unwrap(),
            b"preserve"
        );
    }
}

fn next(service: &Service) -> Event {
    // Blocks on the event channel; bounded only against hangs.
    match service.events.recv_timeout(crate::test_wait::HANG_BOUND) {
        Ok(event) => event,
        Err(error) => panic!("next: worker event absent: {error}"),
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
        .submit(Request::Edit(Command::InsertTrackWithDevice {
            name: "keys".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
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
        .submit(Request::Edit(Command::InsertTrackWithDevice {
            name: "keys".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
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

#[test]
fn grouped_previews_skip_cancel_and_commit_retarget_the_actual_keyboard_without_history_noise() {
    use crate::tuning_gesture::{TuningGestureKey, TuningGestureRequest as Gesture};
    let service = Service::new(48000, 16, 8_000_000).unwrap();
    let _ = next(&service);
    service
        .submit(Request::Edit(Command::InsertTrackWithDevice {
            name: "audible".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        }))
        .unwrap();
    let initial = next(&service);
    let track = initial.state.project.tracks()[0].id();
    let revision = initial.state.revision;
    let input = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
    input.set_key(0, Some((60, 100, 1))).unwrap();
    service
        .submit(Request::PrepareComputerKeyboard {
            project_generation: 0,
            revision,
            request_id: 1,
            track,
            input: Arc::clone(&input),
        })
        .unwrap();
    let mut actual = next(&service).playback.unwrap();
    actual.render(&mut [[0.0; 2]; 777]).unwrap();
    let neutral = 440.0 * 2.0_f64.powf((60.0 - 69.0) / 12.0);
    let mut phase = neutral * 777.0 / 48000.0;
    let key = TuningGestureKey {
        generation: 0,
        token: 1,
    };
    service
        .submit(Request::TuningGesture(Gesture::Begin { key }))
        .unwrap();
    assert!(next(&service).tuning_gesture_ack.unwrap().accepted);
    service
        .submit(Request::TuningGesture(Gesture::Change {
            key,
            sequence: 1,
            tuning: tuning(333.0),
        }))
        .unwrap();
    let skipped = next(&service);
    assert_eq!(skipped.state.project, initial.state.project);
    assert_eq!(skipped.state.revision, revision);
    drop(skipped); // accepted worker preview is deliberately never installed
    for (request, hz, committed) in [
        (
            Gesture::Change {
                key,
                sequence: 2,
                tuning: tuning(196.0),
            },
            196.0,
            false,
        ),
        (Gesture::Cancel { key }, neutral, false),
    ] {
        service.submit(Request::TuningGesture(request)).unwrap();
        let event = next(&service);
        assert!(event.tuning_gesture_ack.unwrap().accepted);
        assert_eq!(event.state.revision, revision + u64::from(committed));
        let mut candidate = event.playback.unwrap();
        candidate.try_inherit_runtime_from(&mut actual).unwrap();
        let mut output = [[0.0; 2]; 137];
        candidate.render(&mut output).unwrap();
        for (offset, frame) in output.iter().enumerate() {
            let expected = (std::f64::consts::TAU * (phase + hz * offset as f64 / 48000.0)).sin()
                * (100.0 / 127.0)
                * 0.1;
            assert!((f64::from(frame[0]) - expected).abs() < 0.000002);
        }
        phase += hz * 137.0 / 48000.0;
        actual = candidate;
        assert!(!input.is_cancelled());
    }
    let key = TuningGestureKey {
        generation: 0,
        token: 2,
    };
    service
        .submit(Request::TuningGesture(Gesture::Begin { key }))
        .unwrap();
    let _ = next(&service);
    service
        .submit(Request::TuningGesture(Gesture::Commit {
            key,
            sequence: 1,
            tuning: tuning(333.0),
        }))
        .unwrap();
    let committed = next(&service);
    assert!(committed.tuning_gesture_ack.unwrap().accepted);
    assert_eq!(committed.state.revision, revision + 1);
    let mut candidate = committed.playback.unwrap();
    candidate.try_inherit_runtime_from(&mut actual).unwrap();
    assert!(!input.is_cancelled());
    service.submit(Request::Undo).unwrap();
    let undone = next(&service);
    assert_eq!(
        undone.state.project, initial.state.project,
        "whole gesture is exactly one undo"
    );
}

struct LibraryScratch(PathBuf);
impl Drop for LibraryScratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn library_scratch() -> LibraryScratch {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let directory = LibraryScratch(std::env::temp_dir().join(format!(
        "hex-tuning-library-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )));
    std::fs::create_dir(&directory.0).unwrap();
    directory
}
#[test]
fn review_identical_library_load_preserves_redo_without_replacement() {
    let directory = library_scratch();
    let mut session = Session::new(48_000, 8, 1_000_000).unwrap();
    session.command(Command::SetTuning(tuning(333.0))).unwrap();
    assert!(session.undo());
    assert!(session.can_redo());
    let original = session.project().clone();
    hex_storage::tuning_library::save(&directory.0, 1, "same", original.tuning().scale()).unwrap();
    let (result, prepared) = run_tuning_library_operation_in(
        &mut session,
        TuningLibraryContext {
            request_generation: 0,
            request_revision: 0,
            operation_id: 1,
            current_generation: 0,
            current_revision: 0,
        },
        TuningLibraryOperation::Load { slot: 1 },
        None,
        &directory.0,
    );
    assert!(result.error.is_none());
    assert_eq!(result.loaded_name.as_deref(), Some("same"));
    assert!(
        prepared.is_none(),
        "identical load must not publish a new revision"
    );
    assert_eq!(session.project(), &original);
    assert!(session.can_redo());
}
#[test]
fn review_identical_import_keeps_worker_revision_clean_and_audio_unchanged() {
    let directory = library_scratch();
    let service = Service::new(48_000, 8, 1_000_000).unwrap();
    let initial = next(&service);
    let path = directory.0.join("same.freqs");
    hex_storage::tuning_library::export_file(&path, "same", initial.state.project.tuning().scale())
        .unwrap();
    service
        .submit(Request::TuningLibrary {
            project_generation: 0,
            revision: initial.state.revision,
            operation_id: 1,
            operation: TuningLibraryOperation::Import { path },
        })
        .unwrap();
    let event = next(&service);
    assert!(event.error.is_none());
    assert_eq!(event.state.revision, initial.state.revision);
    assert!(!event.state.unsaved);
    assert!(event.playback.is_none());
    assert_eq!(event.state.project, initial.state.project);
    assert_eq!(
        event.tuning_library.unwrap().loaded_name.as_deref(),
        Some("same")
    );
}

#[test]
fn review_lifecycle_canonical_edit_publishes_actual_rebased_tuning_pcm() {
    use crate::tuning_gesture::{TuningGestureKey, TuningGestureRequest as Gesture};
    use hex_project::{BeatTime, Created};
    for audition_active in [false, true] {
        let service = Service::new(48_000, 16, 8_000_000).unwrap();
        let _ = next(&service);
        service
            .submit(Request::Edit(Command::InsertTrackWithDevice {
                name: "notes".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().expect("fixture instrument state"),
            }))
            .unwrap();
        let added = next(&service);
        let track = added.state.project.tracks()[0].id();
        service
            .submit(Request::Edit(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::ONE,
            }))
            .unwrap();
        let placed = next(&service);
        let Some(Created::Clip(clip)) = placed.created else {
            panic!("clip expected")
        };
        service
            .submit(Request::Edit(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 100,
            }))
            .unwrap();
        let authored = next(&service);
        assert!(authored.error.is_none());
        if audition_active {
            service
                .submit(Request::PrepareTuningAudition {
                    revision: authored.state.revision,
                    input: Arc::new(hex_engine::tuning_audition::TuningAuditionInput::new(0)),
                })
                .unwrap();
            assert!(next(&service).error.is_none());
        }
        let key = TuningGestureKey {
            generation: 0,
            token: 1,
        };
        service
            .submit(Request::TuningGesture(Gesture::Begin { key }))
            .unwrap();
        assert!(next(&service).tuning_gesture_ack.unwrap().accepted);
        service
            .submit(Request::TuningGesture(Gesture::Change {
                key,
                sequence: 1,
                tuning: tuning(333.0),
            }))
            .unwrap();
        let preview = next(&service);
        assert!(preview.error.is_none());
        let mut expected_playback = preview.playback.unwrap();
        expected_playback.apply(hex_engine::playback::PlaybackCommand::Play);
        let mut expected = [[0.0; 2]; 256];
        expected_playback.render(&mut expected).unwrap();
        assert!(expected.iter().any(|frame| frame[0] != 0.0));
        service
            .submit(Request::Edit(Command::RenameTrack {
                track,
                name: "renamed".into(),
            }))
            .unwrap();
        let rebased = next(&service);
        assert!(rebased.error.is_none(), "{:?}", rebased.error);
        assert_eq!(
            rebased
                .tuning_preview
                .as_ref()
                .unwrap()
                .state
                .project
                .tuning(),
            tuning(333.0)
        );
        assert_eq!(rebased.state.project.tuning(), ProjectTuning::default());
        let mut actual_playback = rebased.playback.unwrap();
        actual_playback.apply(hex_engine::playback::PlaybackCommand::Play);
        let mut actual = [[0.0; 2]; 256];
        actual_playback.render(&mut actual).unwrap();
        assert_eq!(
            actual, expected,
            "preview metadata must describe the graph actually published"
        );
    }
}

#[test]
fn reference_audition_continues_during_device_parameter_preview() {
    use crate::device_gesture::{GestureKey, GestureRequest, GestureTarget, ParameterValue};
    let service = Service::new(48_000, 16, 8_000_000).unwrap();
    let _ = next(&service);
    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "reference".into(),
        }))
        .unwrap();
    let added = next(&service);
    let track = added.state.project.tracks()[0].id();
    service
        .submit(Request::Edit(Command::AddReturnTrack {
            name: "return".into(),
        }))
        .unwrap();
    let added = next(&service);
    let return_track = added.state.project.return_tracks()[0].id();
    let input = Arc::new(hex_engine::tuning_audition::TuningAuditionInput::new(0));
    input.set_key(0, Some((60, 100, 1))).unwrap();
    service
        .submit(Request::PrepareTuningAudition {
            revision: added.state.revision,
            input,
        })
        .unwrap();
    assert!(next(&service).error.is_none());
    let key = GestureKey {
        generation: 0,
        token: 1,
    };
    service
        .submit(Request::DeviceGesture(GestureRequest::Begin {
            key,
            target: GestureTarget::ReturnSend {
                track,
                return_track,
            },
        }))
        .unwrap();
    assert!(next(&service).gesture_ack.unwrap().accepted);
    service
        .submit(Request::DeviceGesture(GestureRequest::Change {
            key,
            sequence: 1,
            value: ParameterValue::Float(0.3),
        }))
        .unwrap();
    let preview = next(&service);
    assert!(preview.gesture_ack.as_ref().unwrap().accepted);
    assert!(preview.error.is_none(), "{:?}", preview.error);
    assert_eq!(
        preview
            .device_preview
            .unwrap()
            .state
            .project
            .track(track)
            .unwrap()
            .sends()[0]
            .gain(),
        0.3
    );
    let mut playback = preview.playback.unwrap();
    let mut frames = [[0.0; 2]; 128];
    playback.render(&mut frames).unwrap();
    assert!(frames.iter().any(|frame| frame[0] != 0.0));
}
