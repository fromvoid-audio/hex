//! Actual ordinary command/history terminals with original native sources.
use super::*;

fn with_gain(state: &NativeDeviceState, gain: f32) -> NativeDeviceState {
    let captured = hex_plugin_state::decode(state).unwrap();
    let mut component = captured.component.unwrap().to_vec();
    assert_eq!(&component[..4], b"HXPS");
    component[8..12].copy_from_slice(&gain.to_le_bytes());
    let mut controller = captured.controller.map(<[u8]>::to_vec);
    if let Some(controller) = controller.as_mut() {
        assert_eq!(&controller[..4], b"HXCT");
        controller[4..8].copy_from_slice(&91_u32.to_le_bytes());
    }
    hex_plugin_state::encode(hex_plugin_state::State {
        identity: captured.identity,
        component: Some(&component),
        controller: controller.as_deref(),
    })
    .unwrap()
}
fn ordinary_terminal(h: &mut ResidentHarness) -> Event {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        h.render();
        // The headless driver advances only after the actual installed workers
        // have completed this callback's request prefixes. Tight-loop rendering
        // while native preparation runs would manufacture missed deadlines.
        workers_idle(h);
        if let Ok(event) = h.service.try_event()
            && event.request_complete
        {
            return event;
        }
        assert!(Instant::now() < end, "ordinary native edit terminal");
        std::thread::yield_now();
    }
}
fn workers_idle(h: &ResidentHarness) {
    wait(|| {
        h.callback
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .native_workers_idle_on_control()
            == Some(true)
    });
}

#[test]
#[ignore = "explicit original SDK helper and source receipts required"]
fn native_effect_repeated_state_history_reclaims_superseded_instances() {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    for playing in [false, true] {
        repeated_state_history(playing);
    }
}
fn repeated_state_history(playing: bool) {
    let (mut h, files, saved) = fixture(Start::Saved);
    let device = saved.project.main_devices()[0].id();
    let replacement = with_gain(saved.project.main_devices()[0].state(), 0.75);
    let original_bytes = fs::read(files.0.join("effect.hex")).unwrap();
    let (mut applied, _) = h.applied(0);
    if playing {
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
    }
    h.service
        .submit(Request::Edit(Command::ReplaceDeviceState {
            device,
            state: replacement.clone(),
        }))
        .unwrap();
    let replaced = ordinary_terminal(&mut h);
    assert!(replaced.error.is_none(), "{:?}", replaced.error);
    (applied, _) = h.applied(applied);
    // More changes than the fixed native pool can hold. Render each actual
    // finite outgoing tail and reclaim its original graph before the next edit;
    // a current/tail alias may never be treated as a reusable history instance.
    for cycle in 0..12 {
        for (request, expected) in [
            (Request::Undo, &saved.project),
            (Request::Redo, &replaced.state.project),
        ] {
            for _ in 0..8 {
                h.render_plugin_block();
                workers_idle(&h);
                h.pump();
            }
            h.service.submit(request).unwrap();
            let event = ordinary_terminal(&mut h);
            assert!(
                event.error.is_none(),
                "playing={playing}, cycle {cycle}: {:?}",
                event.error
            );
            assert_eq!(&event.state.project, expected);
            (applied, _) = h.applied(applied);
        }
    }
    assert!(
        h.plugin_records.load(Acquire) <= 3,
        "history must reclaim superseded sources"
    );
    assert_eq!(
        fs::read(files.0.join("effect.hex")).unwrap(),
        original_bytes
    );
}

#[test]
#[ignore = "explicit original SDK helper and source receipts required"]
fn native_effect_state_replacement_preserves_device_and_one_history_edge() {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    let (mut h, files, saved) = fixture(Start::Saved);
    let original = &saved.project.main_devices()[0];
    let device = original.id();
    let replacement = with_gain(original.state(), 0.75);
    let bytes = fs::read(files.0.join("effect.hex")).unwrap();
    let (mut applied, _) = h.applied(0);
    h.service
        .submit(Request::Edit(Command::ReplaceDeviceState {
            device,
            state: replacement.clone(),
        }))
        .unwrap();
    let replaced = ordinary_terminal(&mut h);
    assert!(
        replaced.error.is_none(),
        "actual replacement must prepare its own matching native state: {:?}",
        replaced.error
    );
    assert_eq!(replaced.state.revision, saved.revision + 1);
    assert_eq!(replaced.state.project.main_devices()[0].id(), device);
    assert_eq!(
        replaced.state.project.main_devices()[0].state(),
        &replacement
    );
    (applied, _) = h.applied(applied);
    assert_eq!(h.plugin_records.load(Acquire), 2);
    assert_eq!(
        h.submitted_observation()
            .unwrap()
            .foreign_processor(device.value(), replacement.type_id()),
        Some(true)
    );
    h.service.submit(Request::Undo).unwrap();
    let undone = ordinary_terminal(&mut h);
    assert!(undone.error.is_none(), "{:?}", undone.error);
    assert_eq!(undone.state.project, saved.project);
    (applied, _) = h.applied(applied);
    h.service.submit(Request::Redo).unwrap();
    let redone = ordinary_terminal(&mut h);
    assert!(redone.error.is_none(), "{:?}", redone.error);
    assert_eq!(redone.state.project, replaced.state.project);
    (applied, _) = h.applied(applied);
    h.service
        .submit(Request::Edit(Command::RenameTrack {
            track: redone.state.project.tracks()[0].id(),
            name: "renamed".into(),
        }))
        .unwrap();
    let renamed = ordinary_terminal(&mut h);
    assert!(renamed.error.is_none(), "{:?}", renamed.error);
    h.applied(applied);
    if hex_plugin_state::decode(&replacement)
        .unwrap()
        .controller
        .is_some()
    {
        let trace = std::env::var_os("HEX_ORIGINAL_RESTORE_TRACE")
            .expect("controller fixture requires its explicit trace");
        let trace = fs::read(trace).unwrap();
        let observed: Vec<_> = trace
            .chunks_exact(4)
            .map(|part| i32::from_ne_bytes(part.try_into().unwrap()))
            .collect();
        assert_eq!(observed.len(), 15);
        assert_eq!(&observed[..3], &[1, 1, 1]);
        assert_eq!(
            &observed[5..8],
            &[
                i32::from_ne_bytes(0.75_f32.to_ne_bytes()),
                i32::from_ne_bytes(0.75_f32.to_ne_bytes()),
                91,
            ]
        );
    }
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let mut pcm = Vec::new();
    for _ in 0..6 {
        pcm.extend(h.render_plugin_block());
        workers_idle(&h);
    }
    assert!(pcm[..512].iter().all(|frame| *frame == [0.0; 2]));
    for (index, frame) in pcm.iter().enumerate().skip(512) {
        let scale = 0.5 + ((index - 512) / 256) as f32 * 0.125;
        assert_eq!(
            *frame,
            [0.25 * 0.75 * scale; 2],
            "redone native state sample {index}"
        );
    }
    assert_eq!(fs::read(files.0.join("effect.hex")).unwrap(), bytes);
}

#[test]
#[ignore = "explicit original SDK helper and source receipts required"]
fn native_effect_pending_state_edit_keeps_fifo_save_and_document_cancellation() {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    for cancel in [false, true] {
        let (mut h, files, saved) = fixture(Start::Saved);
        let original = &saved.project.main_devices()[0];
        let state = with_gain(original.state(), 0.75);
        let old_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
        h.service
            .submit(Request::Edit(Command::ReplaceDeviceState {
                device: original.id(),
                state: state.clone(),
            }))
            .unwrap();
        // Hold the actual new endpoint's callback ACK. No ordinary completion
        // may retire its UI receipt before the canonical replacement terminal.
        wait(|| {
            h.pump();
            h.plugin_queued.load(Acquire)
        });
        assert!(matches!(
            h.service.try_event(),
            Err(mpsc::TryRecvError::Empty)
        ));
        let path = files.0.join("queued-save.hex");
        h.service.submit(Request::SaveNew(path.clone())).unwrap();
        if cancel {
            h.service
                .submit(Request::NewWithAudioMode {
                    mode: DocumentAudioMode::Resident,
                    reservation: None,
                })
                .unwrap();
            // New is behind Save in the unchanged FIFO, but its accepted
            // cancellation signal must release the held earlier operation.
            let refused = h.next();
            assert!(refused.request_complete && refused.error.is_some());
            assert_eq!(refused.state.project, saved.project);
            assert_eq!(refused.state.revision, saved.revision);
        } else {
            let accepted = ordinary_terminal(&mut h);
            assert!(accepted.error.is_none(), "{:?}", accepted.error);
            assert!(!accepted.save_complete);
            assert_eq!(accepted.state.project.main_devices()[0].state(), &state);
        }
        let saved_event = ordinary_terminal(&mut h);
        assert!(
            saved_event.save_complete && saved_event.error.is_none(),
            "{:?}",
            saved_event.error
        );
        let loaded = hex_storage::load(&path).unwrap();
        assert_eq!(
            loaded.main_devices()[0].state(),
            if cancel { original.state() } else { &state }
        );
        if cancel {
            let empty = h.next();
            assert!(empty.project_replaced && empty.error.is_none());
            h.release_relinquished_output();
            h.wait_fresh_output(old_output);
            h.applied(0);
            assert_eq!(h.plugin_records.load(Acquire), 0);
        }
    }
}

#[test]
#[ignore = "explicit original SDK helper and source receipts required"]
fn native_effect_failed_state_restore_preserves_last_confirmed_state_and_history() {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    let (mut h, files, saved) = fixture(Start::Saved);
    let original = &saved.project.main_devices()[0];
    let decoded = hex_plugin_state::decode(original.state()).unwrap();
    let mut component = decoded.component.unwrap().to_vec();
    component[0] = b'!'; // Valid host envelope; actual original provider refuses its state.
    let refused = hex_plugin_state::encode(hex_plugin_state::State {
        component: Some(&component),
        ..decoded
    })
    .unwrap();
    let before = fs::read(files.0.join("effect.hex")).unwrap();
    let (applied, _) = h.applied(0);
    h.service
        .submit(Request::Edit(Command::ReplaceDeviceState {
            device: original.id(),
            state: refused,
        }))
        .unwrap();
    let terminal = ordinary_terminal(&mut h);
    assert!(terminal.error.is_some());
    assert_eq!(terminal.state.project, saved.project);
    assert_eq!(terminal.state.revision, saved.revision);
    assert_eq!(
        (terminal.state.can_undo, terminal.state.can_redo),
        (saved.can_undo, saved.can_redo)
    );
    wait(|| {
        h.pump();
        h.plugin_records.load(Acquire) == 1
    });
    assert_eq!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(Acquire),
        applied
    );
    assert_eq!(fs::read(files.0.join("effect.hex")).unwrap(), before);
}

#[test]
#[ignore = "explicit original SDK helper and source receipts required"]
fn native_effect_track_group_duplicate_has_independent_instances_and_one_undo_redo() {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    let files = Files::new();
    let wave = files.wav(48_000, 2048);
    let bytes = fs::read(&wave).unwrap();
    let mut h = ResidentHarness::with_plugins(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
    );
    let mut state = h.next().state;
    let (mut applied, _) = h.applied(0);
    for _ in 0..2 {
        let imported = h.run(Request::Import(wave.clone()));
        (applied, _) = h.applied(applied);
        let track = imported.state.project.tracks().last().unwrap().id();
        h.service
            .submit(Request::InsertPluginEffect(request(
                &imported.state,
                PluginEffectDestination::Track(track),
            )))
            .unwrap();
        assert!(h.next().error.is_none());
        let inserted = terminal(&mut h);
        assert!(inserted.error.is_none(), "{:?}", inserted.error);
        state = inserted.state;
        (applied, _) = h.applied(applied);
    }
    let source_tracks = state.project.tracks().to_vec();
    let selected = hex_project::TrackSelectionIds::new(
        &source_tracks
            .iter()
            .map(|track| track.id())
            .collect::<Vec<_>>(),
    )
    .unwrap();
    h.service
        .submit(Request::TrackClipboard {
            project_generation: 0,
            revision: state.revision,
            action: hex_session::service::TrackClipboardAction::Duplicate {
                tracks: selected,
                without_clips: false,
            },
        })
        .unwrap();
    let duplicated = ordinary_terminal(&mut h);
    assert!(duplicated.error.is_none(), "{:?}", duplicated.error);
    assert_eq!(duplicated.state.revision, state.revision + 1);
    assert_eq!(duplicated.state.project.tracks().len(), 4);
    (applied, _) = h.applied(applied);
    assert_eq!(h.plugin_records.load(Acquire), 4);
    h.service.submit(Request::Undo).unwrap();
    let undone = ordinary_terminal(&mut h);
    assert!(undone.error.is_none(), "{:?}", undone.error);
    assert_eq!(undone.state.project.tracks(), source_tracks);
    (applied, _) = h.applied(applied);
    h.service.submit(Request::Redo).unwrap();
    let redone = ordinary_terminal(&mut h);
    assert!(redone.error.is_none(), "{:?}", redone.error);
    assert_eq!(
        redone.state.project.tracks(),
        duplicated.state.project.tracks()
    );
    (applied, _) = h.applied(applied);
    let device = redone.state.project.tracks()[3].devices()[0].id();
    let state = with_gain(redone.state.project.device(device).unwrap().state(), 0.75);
    h.service
        .submit(Request::Edit(Command::ReplaceDeviceState { device, state }))
        .unwrap();
    let changed = ordinary_terminal(&mut h);
    assert!(changed.error.is_none(), "{:?}", changed.error);
    h.applied(applied);
    for (original, copied) in source_tracks
        .iter()
        .zip(&changed.state.project.tracks()[2..])
    {
        assert_ne!(original.id(), copied.id());
        assert_ne!(original.devices()[0].id(), copied.devices()[0].id());
        assert_eq!(original.clips()[0].asset(), copied.clips()[0].asset());
        assert_eq!(original.clips()[0].start(), copied.clips()[0].start());
        assert_eq!(original.clips()[0].duration(), copied.clips()[0].duration());
    }
    assert_eq!(&changed.state.project.tracks()[..2], source_tracks);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let mut pcm = Vec::new();
    for _ in 0..6 {
        pcm.extend(h.render_plugin_block());
        workers_idle(&h);
    }
    assert!(pcm[..512].iter().all(|frame| *frame == [0.0; 2]));
    for (index, frame) in pcm.iter().enumerate().skip(512) {
        let scale = 0.5 + ((index - 512) / 256) as f32 * 0.125;
        assert_eq!(
            *frame,
            [0.25 * 3.75 * scale; 2],
            "independent copy sample {index}"
        );
    }
    assert_eq!(fs::read(wave).unwrap(), bytes);
}
