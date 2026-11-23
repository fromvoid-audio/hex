use super::*;

fn open_resident_fixture(
    cx: &mut gpui::TestAppContext,
) -> super::super::worker_ui_fixture::WorkerUiFixture {
    let mut fixture = super::super::worker_ui_fixture::open_with_section_support(
        cx,
        vec![hex_project::Command::AddTrack {
            name: "eq track".into(),
        }],
        false,
        true,
    );
    let editor = fixture.editor.clone();
    let deadline = crate::test_wait::hang_deadline();
    loop {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let ready = editor.update(cx, |view, cx| {
            view.tick(cx);
            view.normal_start_source().is_some() && view.current_seek_requires_preroll().is_some()
        });
        if ready {
            break;
        }
        assert!(Instant::now() < deadline, "resident fixture did not settle");
        std::thread::yield_now();
    }
    fixture
}

fn wait_until_applied_advances(
    fixture: &mut super::super::worker_ui_fixture::WorkerUiFixture,
    cx: &mut gpui::TestAppContext,
    previous_applied: u64,
) -> u64 {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let applied = fixture.editor.read_with(cx, |view, _| {
            view.audio_status()
                .map_or(0, |status| status.applied_revision.load(Acquire))
        });
        if applied > previous_applied {
            return applied;
        }
        assert!(
            Instant::now() < deadline,
            "audio callback did not apply revision past {previous_applied}"
        );
        std::thread::yield_now();
    }
}

#[gpui::test]
fn eq_preview_applied_before_worker_submitted_preserves_preroll_and_allows_toggle_play(
    cx: &mut gpui::TestAppContext,
) {
    let mut fixture = open_resident_fixture(cx);
    let editor = fixture.editor.clone();

    // Add EQ8 (active bands by default) to the track and settle it cleanly.
    let initial_applied = editor.update(cx, |view, _| {
        let track = view.state.as_ref().unwrap().project.tracks()[0].id();
        let applied = view.audio_status().unwrap().applied_revision.load(Acquire);
        assert!(view.enqueue(Request::AddNativeDevice {
            track,
            class: hex_engine::NativeDeviceClass::Effect,
            type_id: Box::new(hex_lattice_contract::DeviceTypeId::from_static("hex.eq8")),
            before: None,
        }));
        applied
    });
    let _ = wait_until_applied_advances(&mut fixture, cx, initial_applied);
    let deadline = crate::test_wait::hang_deadline();
    loop {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let ready = editor.update(cx, |view, cx| {
            view.tick(cx);
            view.state
                .as_ref()
                .is_some_and(|state| !state.project.tracks()[0].devices().is_empty())
                && view.current_seek_requires_preroll().is_some()
        });
        if ready {
            break;
        }
        assert!(Instant::now() < deadline, "EQ8 device add did not settle");
        std::thread::yield_now();
    }

    let (before_preview_applied, initial_preroll, held_rx) = editor.update(cx, |view, _| {
        assert_eq!(
            view.document_audio_mode,
            hex_session::DocumentAudioMode::Resident
        );
        let state = view.state.as_ref().unwrap();
        let device = state.project.tracks()[0].devices()[0].id();
        let applied = view.audio_status().unwrap().applied_revision.load(Acquire);
        let preroll = view.current_seek_requires_preroll();
        assert!(
            preroll.is_some(),
            "fixture must establish known preroll metadata before EQ drag"
        );
        // Withhold `ResidentSubmitted` / `worker_submitted` events so CoreAudio
        // applies the EQ preview graph before the UI drains submission metadata.
        let held_rx = std::mem::replace(&mut view.audio_events, std::sync::mpsc::channel().1);
        let key = view
            .device_gesture
            .begin_pair(
                view.project_generation,
                device,
                hex_lattice_contract::ParameterId::from_static(3),
                hex_lattice_contract::ParameterId::from_static(4),
                hex_session::device_gesture::ParameterPairValue {
                    first: hex_session::device_gesture::ParameterValue::Float(1_000.0),
                    second: hex_session::device_gesture::ParameterValue::Float(0.0),
                },
            )
            .unwrap();
        assert!(key.token > 0);
        let _ = view.pump_device_gesture();
        (applied, preroll, held_rx)
    });

    // Let Begin ACK drain and submit `change_pair` while `audio_events`
    // (`ResidentSubmitted` / `worker_submitted`) remain withheld.
    editor.update(cx, |view, _| {
        view.device_gesture
            .change_pair(hex_session::device_gesture::ParameterPairValue {
                first: hex_session::device_gesture::ParameterValue::Float(1_200.0),
                second: hex_session::device_gesture::ParameterValue::Float(3.0),
            })
            .unwrap();
        let _ = view.pump_device_gesture();
    });

    let deadline = crate::test_wait::hang_deadline();
    let preview_applied = loop {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let applied = editor.update(cx, |view, cx| {
            view.tick(cx);
            view.audio_status()
                .map_or(0, |status| status.applied_revision.load(Acquire))
        });
        if applied > before_preview_applied {
            break applied;
        }
        assert!(
            Instant::now() < deadline,
            "EQ8 preview did not apply revision past {before_preview_applied}"
        );
        std::thread::yield_now();
    };
    editor.update(cx, |view, cx| {
        // Trigger the exact `current_applied` fallback path (which fires when
        // PublicationApplied beats `worker_submitted`).
        let source = view.normal_start_source().unwrap();
        assert_eq!(source.audio_revision, preview_applied);
        view.install_current_applied_fallback(source);
        view.tick(cx);

        let audible = view.audible_playback.as_ref().unwrap();
        assert_eq!(audible.audio_revision, preview_applied);
        assert_eq!(
            audible.worker_requires_preroll, initial_preroll,
            "same-revision EQ preview apply before worker_submitted must retain preroll metadata"
        );
        assert_eq!(view.current_seek_requires_preroll(), initial_preroll);

        // Pressing Space during the drag starts playback cleanly with no error.
        view.toggle_play();
        assert!(view.error.is_none(), "unexpected error: {:?}", view.error);
        assert!(matches!(
            view.last_transport_command,
            Some(PlaybackCommand::PlayFromWithNoteChase { .. })
        ));
    });

    for _ in 0..4 {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
    }
    editor.update(cx, |view, cx| {
        assert!(
            view.audio_status().unwrap().playing.load(Acquire),
            "audio callback must enter playing state"
        );
        // Restore withheld `ResidentSubmitted` events and confirm late backfill.
        view.audio_events = held_rx;
        view.tick(cx);
        assert_eq!(
            view.audible_playback.as_ref().unwrap().audio_revision,
            preview_applied
        );
        assert_eq!(view.current_seek_requires_preroll(), initial_preroll);
        assert!(view.error.is_none(), "unexpected error: {:?}", view.error);
    });
    fixture.finish(cx);
}

#[gpui::test]
fn fast_edit_applied_before_worker_submitted_allows_play_defers_seek_and_clears_stale_error(
    cx: &mut gpui::TestAppContext,
) {
    let mut fixture = open_resident_fixture(cx);
    let editor = fixture.editor.clone();

    let (initial_applied, initial_revision, held_rx) = editor.update(cx, |view, _| {
        let track = view.state.as_ref().unwrap().project.tracks()[0].id();
        let initial_revision = view.state.as_ref().unwrap().revision;
        let initial_applied = view.audio_status().unwrap().applied_revision.load(Acquire);
        // Withhold `ResidentSubmitted` so `PublicationApplied` lands first.
        let held_rx = std::mem::replace(&mut view.audio_events, std::sync::mpsc::channel().1);
        assert!(
            view.enqueue(Request::Edit(hex_project::Command::RenameTrack {
                track,
                name: "fast edit".into(),
            }))
        );
        (initial_applied, initial_revision, held_rx)
    });

    // Advance until both canonical state revision and audio callback applied_revision advance.
    let edited_applied = wait_until_applied_advances(&mut fixture, cx, initial_applied);
    let deadline = crate::test_wait::hang_deadline();
    loop {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let updated = editor.update(cx, |view, cx| {
            view.tick(cx);
            view.state
                .as_ref()
                .is_some_and(|state| state.revision > initial_revision)
        });
        if updated {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "canonical edit event not received"
        );
        std::thread::yield_now();
    }

    editor.update(cx, |view, _| {
        // Trigger the exact `current_applied` fallback path for the new revision
        // before `ResidentSubmitted` arrives.
        let source = view.normal_start_source().unwrap();
        assert_eq!(source.audio_revision, edited_applied);
        view.install_current_applied_fallback(source);

        let audible = view.audible_playback.as_ref().unwrap();
        assert_eq!(audible.audio_revision, edited_applied);
        assert_eq!(
            audible.worker_requires_preroll, None,
            "stub reconstructed before worker_submitted has no preroll yet"
        );

        // Seeking during this sub-frame window defers cleanly until metadata arrives.
        view.error = Some("playback source is unavailable · cannot seek safely".into());
        view.request_seek(4_800);
        assert_eq!(
            view.deferred_metadata_seek,
            Some((4_800, view.project_generation))
        );
        assert!(view.error.is_none(), "seek deferral cleared stale error");
    });

    // Deliver the withheld `ResidentSubmitted` metadata: confirm it backfills
    // `worker_requires_preroll` and dispatches the deferred seek cleanly.
    editor.update(cx, |view, cx| {
        view.audio_events = held_rx;
        view.tick(cx);
        assert_eq!(
            view.audible_playback.as_ref().unwrap().audio_revision,
            edited_applied
        );
        assert!(
            view.audible_playback
                .as_ref()
                .unwrap()
                .worker_requires_preroll
                .is_some(),
            "late worker_submitted must backfill worker_requires_preroll"
        );
        assert!(
            view.deferred_metadata_seek.is_none(),
            "deferred seek must dispatch once metadata arrives"
        );
        assert!(view.error.is_none(), "unexpected error: {:?}", view.error);
    });
    for _ in 0..4 {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
    }

    // Now reproduce TogglePlay while `worker_requires_preroll` is still `None`
    // (after a second fast edit where `PublicationApplied` beats `worker_submitted`),
    // verifying Play starts immediately AND clears a prior stale unavailable error.
    let (second_applied_before, second_revision_before, held_rx_2) =
        editor.update(cx, |view, cx| {
            view.tick(cx);
            let track = view.state.as_ref().unwrap().project.tracks()[0].id();
            let rev = view.state.as_ref().unwrap().revision;
            let applied = view.audio_status().unwrap().applied_revision.load(Acquire);
            let held = std::mem::replace(&mut view.audio_events, std::sync::mpsc::channel().1);
            assert!(
                view.enqueue(Request::Edit(hex_project::Command::RenameTrack {
                    track,
                    name: "fast edit 2".into(),
                }))
            );
            (applied, rev, held)
        });
    let second_applied = wait_until_applied_advances(&mut fixture, cx, second_applied_before);
    let deadline = crate::test_wait::hang_deadline();
    loop {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let updated = editor.update(cx, |view, cx| {
            view.tick(cx);
            view.state
                .as_ref()
                .is_some_and(|state| state.revision > second_revision_before)
        });
        if updated {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "second canonical edit event not received"
        );
        std::thread::yield_now();
    }
    editor.update(cx, |view, _| {
        let source = view.normal_start_source().unwrap();
        assert_eq!(source.audio_revision, second_applied);
        view.install_current_applied_fallback(source);
        assert_eq!(
            view.audible_playback
                .as_ref()
                .unwrap()
                .worker_requires_preroll,
            None
        );
        view.error = Some("playback source is unavailable · cannot seek safely".into());
        view.toggle_play();
        assert!(
            view.error.is_none(),
            "TogglePlay must succeed and clear stale unavailable error: {:?}",
            view.error
        );
        assert!(matches!(
            view.last_transport_command,
            Some(PlaybackCommand::PlayFromWithNoteChase { .. })
        ));
    });
    for _ in 0..4 {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
    }
    editor.update(cx, |view, cx| {
        assert!(view.audio_status().unwrap().playing.load(Acquire));
        view.audio_events = held_rx_2;
        view.tick(cx);
        assert!(
            view.audible_playback
                .as_ref()
                .unwrap()
                .worker_requires_preroll
                .is_some()
        );
        assert!(view.error.is_none(), "stale error must remain cleared");
    });
    fixture.finish(cx);
}

#[gpui::test]
fn toggle_play_during_inflight_publication_defers_and_starts_once_applied(
    cx: &mut gpui::TestAppContext,
) {
    let mut fixture = open_resident_fixture(cx);
    let editor = fixture.editor.clone();

    let (initial_applied, initial_revision) = editor.update(cx, |view, _| {
        let track = view.state.as_ref().unwrap().project.tracks()[0].id();
        let initial_revision = view.state.as_ref().unwrap().revision;
        let initial_applied = view.audio_status().unwrap().applied_revision.load(Acquire);
        assert!(
            view.enqueue(Request::Edit(hex_project::Command::RenameTrack {
                track,
                name: "inflight edit".into(),
            }))
        );
        (initial_applied, initial_revision)
    });

    // Wait for the canonical Event (so `view.state.revision` advances ahead of
    // the audio callback's applied source) WITHOUT rendering any audio block yet!
    let deadline = crate::test_wait::hang_deadline();
    loop {
        let updated = editor.update(cx, |view, cx| {
            view.tick(cx);
            view.state
                .as_ref()
                .is_some_and(|state| state.revision > initial_revision)
        });
        if updated {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "canonical edit event not received"
        );
        std::thread::yield_now();
    }

    editor.update(cx, |view, _| {
        assert!(
            view.normal_start_source().is_none(),
            "audio callback has not yet applied the new revision"
        );
        view.audible_playback = None;
        view.error = Some("playback source is unavailable · cannot seek safely".into());
        view.toggle_play();
        assert!(
            view.error.is_none(),
            "deferring TogglePlay during in-flight edit must not report unavailable error: {:?}",
            view.error
        );
        assert!(view.deferred_toggle_play.is_some());
    });

    let _ = wait_until_applied_advances(&mut fixture, cx, initial_applied);
    let deadline = crate::test_wait::hang_deadline();
    loop {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let playing = editor.update(cx, |view, cx| {
            view.tick(cx);
            view.deferred_toggle_play.is_none()
                && view.audio_status().unwrap().playing.load(Acquire)
        });
        if playing {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "deferred TogglePlay did not start once publication applied"
        );
        std::thread::yield_now();
    }
    editor.read_with(cx, |view, _| {
        assert!(view.error.is_none(), "unexpected error: {:?}", view.error);
    });
    fixture.finish(cx);
}
