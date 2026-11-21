//! Signed hosted graph edits on the production verified-file publication path.
use super::unsealed_admission::{
    HeldVerifiedEq, eq_gesture, expect_verified_sample, hold_verified_eq_preview,
};
use super::*;
use hex_audio::audio_diagnostics::{AudioDiagnosticKind, AudioDiagnosticsFlags};
use hex_session::device_gesture::{GestureKey, GestureRequest, ParameterValue};

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn verified_running_main_vst3_replacement_installs_at_its_exact_receipt() {
    assert_verified_running_vst3_replacement(VerifiedReplacementOwner::Main);
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn verified_running_return_vst3_replacement_installs_at_its_exact_receipt() {
    assert_verified_running_vst3_replacement(VerifiedReplacementOwner::Return);
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn verified_running_track_vst3_replacement_installs_at_its_exact_receipt() {
    assert_verified_running_vst3_replacement(VerifiedReplacementOwner::Track);
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn verified_running_track_vst3_replacement_undo_restores_a_fresh_worker() {
    assert_verified_running_vst3_replacement(VerifiedReplacementOwner::TrackHistory {
        redo: false,
    });
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn verified_running_track_vst3_replacement_redo_restores_a_fresh_worker() {
    assert_verified_running_vst3_replacement(VerifiedReplacementOwner::TrackHistory { redo: true });
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn verified_running_main_vst3_replacement_undo_restores_a_fresh_worker() {
    assert_verified_running_vst3_replacement(VerifiedReplacementOwner::MainHistory { redo: false });
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn verified_running_main_vst3_replacement_redo_restores_a_fresh_worker() {
    assert_verified_running_vst3_replacement(VerifiedReplacementOwner::MainHistory { redo: true });
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn verified_running_return_vst3_replacement_undo_restores_a_fresh_worker() {
    assert_verified_running_vst3_replacement(VerifiedReplacementOwner::ReturnHistory {
        redo: false,
    });
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn verified_running_return_vst3_replacement_redo_restores_a_fresh_worker() {
    assert_verified_running_vst3_replacement(VerifiedReplacementOwner::ReturnHistory {
        redo: true,
    });
}

enum VerifiedReplacementOwner {
    Main,
    Return,
    Track,
    MainHistory { redo: bool },
    ReturnHistory { redo: bool },
    TrackHistory { redo: bool },
}

fn replacement_effects(
    project: &hex_project::Project,
    destination: PluginEffectDestination,
) -> &[hex_project::Device] {
    match destination {
        PluginEffectDestination::Main => project.main_devices(),
        PluginEffectDestination::Track(id) => project.track(id).unwrap().devices(),
        PluginEffectDestination::Return(id) => project
            .return_tracks()
            .iter()
            .find(|track| track.id() == id)
            .unwrap()
            .devices(),
        _ => panic!("effect fixture destination"),
    }
}

fn assert_verified_running_vst3_replacement(owner: VerifiedReplacementOwner) {
    use hex_session::service::{DeviceInsertOutcome, GraphEditOperation};
    use std::sync::atomic::AtomicUsize;

    let _native = native_fixture();
    let files = Files::new();
    let directory = files.0.canonicalize().unwrap();
    hex_ui::write_plugin_effect_test_catalog(&directory, &selection(), "Fx", 0).unwrap();
    let progress = Arc::new(Mutex::new(
        crate::native_audio::plugins::PreparationObservation::default(),
    ));
    let records = Arc::new(AtomicUsize::new(0));
    let mut h = super::super::verified_gain::native_harness_with(|sink| {
        sink.plugins = crate::native_audio::plugins::Plugins::with_fixture(host(), None);
        sink.plugins.catalog = Some(directory.join("plugin-catalog.json"));
        sink.plugins.progress = Some(Arc::clone(&progress));
        sink.plugins.records = Some(Arc::clone(&records));
    });
    h.next();
    h.open();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.open();
    h.applied(h.context.unwrap());
    let imported = h.run(Request::Import(files.wav(48_000, 131_072)));
    h.applied(h.context.unwrap());
    let track = imported.state.project.tracks()[0].id();
    let (state, destination) = match owner {
        VerifiedReplacementOwner::Main | VerifiedReplacementOwner::MainHistory { .. } => {
            (imported.state, PluginEffectDestination::Main)
        }
        VerifiedReplacementOwner::Track | VerifiedReplacementOwner::TrackHistory { .. } => {
            (imported.state, PluginEffectDestination::Track(track))
        }
        VerifiedReplacementOwner::Return | VerifiedReplacementOwner::ReturnHistory { .. } => {
            let added = h.run(Request::Edit(Command::AddReturnTrack {
                name: "replacement send".into(),
            }));
            let Some(Created::ReturnTrack(return_track)) = added.created else {
                panic!("return track");
            };
            h.applied(h.context.unwrap());
            let routed = h.run(Request::Edit(Command::SetReturnSend {
                track,
                return_track,
                gain: 0.5,
            }));
            assert!(routed.error.is_none(), "{:?}", routed.error);
            h.applied(h.context.unwrap());
            (routed.state, PluginEffectDestination::Return(return_track))
        }
    };
    let submit = |h: &Harness, input| {
        h.service
            .as_ref()
            .unwrap()
            .submit(Request::InsertPluginEffect(input))
            .unwrap();
    };
    submit(&h, request(&state, destination));
    let original = loop {
        let event = h.next_with_audio();
        assert!(event.error.is_none(), "{:?}", event.error);
        if let Some(terminal) = event.plugin_effect_complete.first() {
            assert!(terminal.accepted);
            break event;
        }
    };
    let predecessor = h.applied(h.context.unwrap());
    let old = replacement_effects(&original.state.project, destination)[0].id();
    let binding = || progress.lock().unwrap().source_bindings[0];
    wait(|| binding().is_some());
    let (_, old_binding, _) = binding().unwrap();
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let status = h.audio.as_ref().unwrap().status().unwrap();
    let expected = |block| {
        let effect = 0.125 + block as f32 * 0.03125;
        match owner {
            VerifiedReplacementOwner::Main
            | VerifiedReplacementOwner::MainHistory { .. }
            | VerifiedReplacementOwner::Track
            | VerifiedReplacementOwner::TrackHistory { .. } => effect,
            // The same authored 0.25 reaches the dry and wet routes. Return
            // receives half of it, so the measured ramp must be above dry.
            VerifiedReplacementOwner::Return | VerifiedReplacementOwner::ReturnHistory { .. } => {
                0.25 + effect * 0.5
            }
        }
    };
    for block in 0..4 {
        let mut sample = 0.0;
        wait(|| {
            sample = render_hosted_quantum(&h);
            status.performance_frame.load(Acquire) == (block + 1) * 256
        });
        wait(|| {
            binding()
                .and_then(|(_, _, completed)| completed)
                .is_some_and(|completed| completed > block)
        });
        assert_eq!(sample, block.checked_sub(2).map_or(0.0, expected));
    }
    assert!(status.playing.load(Acquire));
    assert_eq!(status.applied_audio_source(), Some(predecessor));
    let replacement = || {
        let target = match destination {
            PluginEffectDestination::Main => {
                PluginEffectDestination::ReplaceMainEffect { replacing: old }
            }
            PluginEffectDestination::Track(track) => PluginEffectDestination::ReplaceTrackEffect {
                track,
                replacing: old,
            },
            PluginEffectDestination::Return(return_track) => {
                PluginEffectDestination::ReplaceReturnEffect {
                    return_track,
                    replacing: old,
                }
            }
            _ => panic!("effect fixture destination"),
        };
        request(&original.state, target)
    };
    let approved = std::cell::Cell::new(None);
    let pump = || {
        let frame = status.performance_frame.load(Acquire);
        let pcm = render_hosted_quantum(&h);
        if approved.get().is_none()
            && status.applied_audio_source() == Some(predecessor)
            && status.performance_frame.load(Acquire) == frame + 256
        {
            assert_eq!(binding().unwrap().1, old_binding);
            assert_eq!(pcm, expected(frame / 256 - 2));
        }
        wait(|| {
            h.backend
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .native_workers_idle_on_control()
                == Some(true)
        });
    };
    let next = || {
        let mut next = None;
        wait(|| {
            if let Ok(event) = h.service.as_ref().unwrap().try_event() {
                next = Some(event);
                return true;
            }
            pump();
            false
        });
        next.unwrap()
    };
    let assert_predecessor = |event: &Event| {
        assert_eq!(event.state.project, original.state.project);
        assert_eq!(event.state.revision, original.state.revision);
        assert_eq!(event.state.can_undo, original.state.can_undo);
        assert_eq!(event.state.can_redo, original.state.can_redo);
        assert_eq!(status.applied_audio_source(), Some(predecessor));
        assert_eq!(binding().unwrap().1, old_binding);
        assert!(status.playing.load(Acquire));
    };
    let cancelled = replacement();
    let cancellation = Arc::clone(&cancelled.control);
    submit(&h, cancelled);
    wait(|| {
        if let Ok(event) = h.service.as_ref().unwrap().try_event() {
            assert_predecessor(&event);
            assert!(
                event.plugin_effect_complete.is_empty(),
                "running Verified {destination:?} replacement stopped before private preparation: {:?}",
                event.error
            );
            assert!(event.error.is_none(), "{:?}", event.error);
        }
        pump();
        assert_eq!(status.applied_audio_source(), Some(predecessor));
        progress
            .lock()
            .unwrap()
            .newest_source
            .is_some_and(|(id, pending, accepted)| {
                id == original.state.project.next_identity() && pending != old_binding && !accepted
            })
    });
    assert!(cancellation.cancel());
    loop {
        let event = next();
        assert_predecessor(&event);
        if let Some(terminal) = event.plugin_effect_complete.first() {
            assert!(Arc::ptr_eq(&terminal.control, &cancellation));
            assert!(!terminal.accepted);
            break;
        }
    }
    wait(|| {
        pump();
        records.load(Acquire) == 1
    });

    let candidate = replacement();
    let operation = Arc::clone(&candidate.control);
    submit(&h, candidate);
    let installed = loop {
        let event = next();
        if let Some(offer) = &event.device_insert_confirmation {
            assert_eq!(offer.operation(), GraphEditOperation::ReplaceDevice);
            assert_predecessor(&event);
            assert!(approved.replace(Some(offer.submission())).is_none());
            assert!(offer.proceed());
        }
        if let Some(terminal) = event.plugin_effect_complete.first() {
            assert!(Arc::ptr_eq(&terminal.control, &operation));
            if !terminal.accepted {
                assert_predecessor(&event);
            }
            assert!(
                terminal.accepted,
                "running Verified {destination:?} replacement refused: {:?}",
                event.error
            );
            break event;
        }
        assert!(event.error.is_none(), "{:?}", event.error);
        assert_eq!(event.state.project, original.state.project);
    };
    let receipt = installed.device_insert_complete.unwrap();
    let applied = status.applied_audio_source().unwrap();
    assert_eq!(receipt.outcome, DeviceInsertOutcome::Installed);
    assert_eq!(receipt.submission.context, applied.context);
    assert_eq!(receipt.submission.audio_revision, applied.audio_revision);
    assert_eq!(
        receipt.submission.output_identity,
        predecessor.output_identity
    );
    if let Some(consented) = approved.get() {
        assert_eq!(receipt.submission.context, consented.context);
        assert_eq!(
            receipt.submission.output_identity,
            consented.output_identity
        );
        assert_ne!(receipt.submission.audio_revision, consented.audio_revision);
    }
    assert_eq!(installed.state.revision, original.state.revision + 1);
    let [fresh] = replacement_effects(&installed.state.project, destination) else {
        panic!("fresh effect in the original slot");
    };
    assert_eq!(fresh.id().value(), original.state.project.next_identity());
    assert!(installed.state.project.device(old).is_none());
    if let PluginEffectDestination::Track(track) = destination {
        let before = original.state.project.track(track).unwrap();
        let after = installed.state.project.track(track).unwrap();
        assert_eq!(after.clips(), before.clips());
        assert_eq!(after.source_id(), before.source_id());
        assert_eq!(after.instrument_position(), before.instrument_position());
    } else {
        assert_eq!(
            installed.state.project.tracks(),
            original.state.project.tracks()
        );
    }
    wait(|| {
        pump();
        records.load(Acquire) == 1
            && binding().is_some_and(|(device, binding, _)| {
                device == fresh.id().value() && binding != old_binding
            })
    });
    let redo = match owner {
        VerifiedReplacementOwner::MainHistory { redo }
        | VerifiedReplacementOwner::ReturnHistory { redo }
        | VerifiedReplacementOwner::TrackHistory { redo } => Some(redo),
        _ => None,
    };
    if let Some(redo) = redo {
        let retired_on_undo = binding().unwrap().1;
        let restored = assert_verified_replacement_history(
            &h,
            (&original, destination),
            &installed,
            &progress,
            &records,
            old_binding,
            ReplacementHistoryDirection::Undo,
        );
        if redo {
            assert_verified_replacement_history(
                &h,
                (&installed, destination),
                &restored,
                &progress,
                &records,
                retired_on_undo,
                ReplacementHistoryDirection::Redo,
            );
        }
    }
}

enum ReplacementHistoryDirection {
    Undo,
    Redo,
}

fn assert_verified_replacement_history(
    h: &Harness,
    (target, destination): (&Event, PluginEffectDestination),
    incumbent: &Event,
    progress: &Mutex<crate::native_audio::plugins::PreparationObservation>,
    records: &std::sync::atomic::AtomicUsize,
    retired_target_binding: hex_audio::plugin_effects::EffectBinding,
    direction: ReplacementHistoryDirection,
) -> Event {
    use hex_session::service::{DeviceInsertOutcome, GraphEditOperation};

    let (request, name, can_redo) = match direction {
        ReplacementHistoryDirection::Undo => (Request::Undo, "Undo", true),
        ReplacementHistoryDirection::Redo => (Request::Redo, "Redo", false),
    };
    let ramp_increment = if matches!(destination, PluginEffectDestination::Return(_)) {
        0.015625
    } else {
        0.03125
    };
    let status = h.audio.as_ref().unwrap().status().unwrap();
    let predecessor = status.applied_audio_source().unwrap();
    let accepted = || progress.lock().unwrap().source_bindings[0].unwrap();
    let (incumbent_device, incumbent_binding, _) = accepted();
    assert_ne!(incumbent_binding, retired_target_binding);
    let [effect] = replacement_effects(&incumbent.state.project, destination) else {
        panic!("replacement in the occupied effect slot");
    };
    assert_eq!(effect.id().value(), incumbent_device);
    let [target_effect] = replacement_effects(&target.state.project, destination) else {
        panic!("original effect");
    };
    let preparing_target = || {
        progress
            .lock()
            .unwrap()
            .newest_source
            .is_some_and(|(device, binding, accepted)| {
                device == target_effect.id().value()
                    && binding != retired_target_binding
                    && binding != incumbent_binding
                    && !accepted
            })
    };
    let assert_canonical = |event: &Event| {
        assert_eq!(event.state.project, incumbent.state.project);
        assert_eq!(event.state.revision, incumbent.state.revision);
        assert_eq!(event.state.can_undo, incumbent.state.can_undo);
        assert_eq!(event.state.can_redo, incumbent.state.can_redo);
    };
    let assert_predecessor = |event: &Event| {
        assert_canonical(event);
        assert_eq!(status.applied_audio_source(), Some(predecessor));
        assert_eq!(accepted().1, incumbent_binding);
        assert!(status.playing.load(Acquire));
    };
    let pump = || {
        let frame = status.performance_frame.load(Acquire);
        let sample = render_hosted_quantum(h);
        wait(|| {
            h.backend
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .native_workers_idle_on_control()
                == Some(true)
        });
        (status.performance_frame.load(Acquire) == frame + 256).then_some(sample)
    };
    // Clear the bridge fill and one cut fade, then measure the replacement's
    // own signed block ramp rather than accepting its dry input as evidence.
    let mut warmup = [0.0; 5];
    for slot in &mut warmup {
        wait(|| {
            let Some(sample) = pump() else {
                return false;
            };
            *slot = sample;
            true
        });
    }
    assert_eq!(warmup[4] - warmup[3], ramp_increment);
    let mut last = warmup[4];
    assert_predecessor(incumbent);

    let service = h.service.as_ref().unwrap();
    service.submit(request).unwrap();
    let mut offer = None;
    let mut heard_predecessor = false;
    let mut pending = false;
    let deadline = Instant::now() + Duration::from_secs(3);
    let restored =
        'terminal: loop {
            assert!(
                Instant::now() < deadline,
                "running {destination:?} replacement {name} stalled"
            );
            while let Ok(event) = service.try_event() {
                if let Some(error) = event.error.as_deref() {
                    assert_predecessor(&event);
                    assert!(event.device_insert_complete.is_none_or(|receipt| {
                        receipt.outcome != DeviceInsertOutcome::Installed
                    }));
                    panic!("running Verified {destination:?} replacement {name} refused: {error}");
                }
                if event.request_complete {
                    break 'terminal event;
                }
                assert_canonical(&event);
                pending = true;
                if let Some(confirmation) = &event.device_insert_confirmation {
                    assert_predecessor(&event);
                    assert_eq!(confirmation.operation(), GraphEditOperation::ReplaceDevice);
                    let sample = pump().expect("predecessor block before replacement consent");
                    assert_eq!(sample - last, ramp_increment);
                    heard_predecessor |= preparing_target();
                    assert!(offer.replace(confirmation.submission()).is_none());
                    assert!(confirmation.proceed());
                }
            }
            if let Some(sample) = pump()
                && offer.is_none()
                && status.applied_audio_source() == Some(predecessor)
            {
                assert_eq!(accepted().1, incumbent_binding);
                assert_eq!(sample - last, ramp_increment);
                heard_predecessor |= pending && preparing_target();
                last = sample;
            }
            std::thread::sleep(Duration::from_millis(1));
        };
    assert!(pending && heard_predecessor);
    let receipt = restored.device_insert_complete.unwrap();
    let exact = status.applied_audio_source().unwrap();
    assert_eq!(receipt.outcome, DeviceInsertOutcome::Installed);
    assert_eq!(receipt.submission.context, exact.context);
    assert_eq!(receipt.submission.audio_revision, exact.audio_revision);
    assert_eq!(receipt.submission.output_identity, exact.output_identity);
    assert_eq!(exact.output_identity, predecessor.output_identity);
    assert_ne!(exact, predecessor);
    if let Some(consented) = offer {
        assert_eq!(receipt.submission.context, consented.context);
        assert_ne!(receipt.submission.audio_revision, consented.audio_revision);
    }
    assert_eq!(restored.state.revision, incumbent.state.revision + 1);
    assert_eq!(
        replacement_effects(&restored.state.project, destination),
        std::slice::from_ref(target_effect)
    );
    assert_eq!(
        restored.state.project.tracks(),
        target.state.project.tracks()
    );
    assert_eq!(restored.state.can_redo, can_redo);
    assert!(restored.state.can_undo);
    wait(|| {
        let _ = pump();
        let Some((device, binding, _)) = progress.lock().unwrap().source_bindings[0] else {
            return false;
        };
        records.load(Acquire) == 1
            && device == target_effect.id().value()
            && binding != retired_target_binding
            && binding != incumbent_binding
    });
    assert!(status.playing.load(Acquire));
    restored
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn verified_running_vst3_copy_keeps_the_original_until_exact_install() {
    use hex_project::{DeviceChainOwner, DeviceSelectionIds};
    use hex_session::service::{DeviceClipboardAction, DeviceInsertOutcome};

    fn status(h: &Harness) -> &hex_audio::prepared::Status {
        h.audio.as_ref().unwrap().status().unwrap()
    }

    let _native = native_fixture();
    let progress = Arc::new(Mutex::new(
        crate::native_audio::plugins::PreparationObservation::default(),
    ));
    let mut h = super::super::verified_gain::native_harness_with(|sink| {
        sink.plugins = crate::native_audio::plugins::Plugins::with_fixture(host(), None);
        sink.plugins.progress = Some(Arc::clone(&progress));
    });
    let files = Files::new();
    h.next();
    h.open();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.open();
    h.applied(h.context.unwrap());
    let imported = h.run(Request::Import(files.wav(48_000, 131_072)));
    h.applied(h.context.unwrap());
    let submit = |h: &Harness, request| h.service.as_ref().unwrap().submit(request).unwrap();
    submit(
        &h,
        Request::InsertPluginEffect(request(&imported.state, PluginEffectDestination::Main)),
    );
    let inserted = loop {
        let event = h.next_with_audio();
        assert!(event.error.is_none(), "{:?}", event.error);
        if let Some(terminal) = event.plugin_effect_complete.first() {
            assert!(terminal.accepted);
            break event;
        }
    };
    let predecessor = h.applied(h.context.unwrap());
    let original = &inserted.state.project.main_devices()[0];
    let binding = || progress.lock().unwrap().source_bindings[0];
    wait(|| binding().is_some());
    let (_, original_binding, _) = binding().unwrap();
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for block in 0..4 {
        let mut sample = 0.0;
        wait(|| {
            sample = render_hosted_quantum(&h);
            status(&h).performance_frame.load(Acquire) == (block + 1) * 256
        });
        wait(|| {
            binding()
                .and_then(|(_, _, completed)| completed)
                .is_some_and(|completed| completed > block)
        });
        let expected = if block < 2 {
            0.0
        } else {
            0.125 + (block - 2) as f32 * 0.03125
        };
        assert_eq!(sample, expected);
    }
    assert_eq!(binding().unwrap().1, original_binding);
    let clipboard = |action| Request::DeviceClipboard {
        project_generation: predecessor.context.project_generation,
        revision: inserted.state.revision,
        action,
    };
    let approved = std::cell::Cell::new(false);
    let next_running_event = |h: &Harness| {
        let mut event = None;
        wait(|| {
            if let Ok(received) = h.service.as_ref().unwrap().try_event() {
                event = Some(received);
                return true;
            }
            let frame = status(h).performance_frame.load(Acquire);
            let sample = render_hosted_quantum(h);
            if !approved.get() && status(h).performance_frame.load(Acquire) == frame + 256 {
                assert_eq!(status(h).applied_audio_source(), Some(predecessor));
                let expected = 0.125 + (frame / 256 - 2) as f32 * 0.03125;
                assert_eq!(sample, expected, "preparing a copy changed predecessor PCM");
            }
            wait(|| {
                h.backend
                    .lock()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .native_workers_idle_on_control()
                    == Some(true)
            });
            false
        });
        event.unwrap()
    };
    let offer = |h: &Harness| loop {
        let pending = next_running_event(h);
        assert!(
            pending.error.is_none(),
            "Verified VST3 copy refused: {:?}",
            pending.error
        );
        assert!(!pending.request_complete);
        assert_eq!(pending.state.project, inserted.state.project);
        if let Some(offer) = pending.device_insert_confirmation {
            assert_eq!(status(h).applied_audio_source(), Some(predecessor));
            break offer;
        }
    };
    let duplicate = || {
        clipboard(DeviceClipboardAction::Duplicate {
            source: DeviceChainOwner::Main,
            selected: DeviceSelectionIds::new(&[original.id()]).unwrap(),
            destination: DeviceChainOwner::Main,
            before: None,
        })
    };
    // Hold actual Root Work longer than Session's ten-second preparation
    // budget. Only the private edit may expire; the playing source is retained.
    let (held, observed) = std::sync::mpsc::sync_channel(1);
    let (release, resume) = std::sync::mpsc::sync_channel(1);
    let backend = Arc::clone(&h.backend);
    let holder = std::thread::spawn(move || {
        backend
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .with_native_source_work_held_on_control(|| {
                held.send(()).unwrap();
                resume.recv_timeout(Duration::from_secs(15)).unwrap();
            })
            .unwrap();
    });
    observed.recv_timeout(Duration::from_secs(1)).unwrap();
    submit(&h, duplicate());
    let deadline = Instant::now() + Duration::from_secs(11);
    let expired = loop {
        if let Ok(event) = h.service.as_ref().unwrap().try_event()
            && event.request_complete
        {
            break Some(event);
        }
        if Instant::now() >= deadline {
            break None;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    release.send(()).unwrap();
    holder.join().unwrap();
    let expired = expired.expect("staged copy must expire while Root Work is still held");
    assert_eq!(
        expired.error.as_deref(),
        Some("verified vst3 preparation timed out")
    );
    assert_eq!(expired.state.project, inserted.state.project);
    assert_eq!(expired.state.revision, inserted.state.revision);
    assert_eq!(expired.state.can_undo, inserted.state.can_undo);
    assert_eq!(expired.state.can_redo, inserted.state.can_redo);
    assert_eq!(status(&h).applied_audio_source(), Some(predecessor));
    assert!(status(&h).playing.load(Acquire));
    assert_eq!(binding().unwrap().1, original_binding);

    submit(&h, duplicate());
    assert!(offer(&h).cancel());
    let cancelled = next_running_event(&h);
    assert_eq!(
        cancelled.device_insert_complete.unwrap().outcome,
        DeviceInsertOutcome::Cancelled
    );
    assert_eq!(cancelled.state.project, inserted.state.project);
    assert_eq!(cancelled.state.revision, inserted.state.revision);
    assert_eq!(cancelled.state.can_undo, inserted.state.can_undo);
    assert_eq!(cancelled.state.can_redo, inserted.state.can_redo);
    assert_eq!(status(&h).applied_audio_source(), Some(predecessor));
    assert_eq!(binding().unwrap().1, original_binding);

    h.run(clipboard(DeviceClipboardAction::Copy {
        owner: DeviceChainOwner::Main,
        selected: DeviceSelectionIds::new(&[original.id()]).unwrap(),
    }));
    submit(
        &h,
        clipboard(DeviceClipboardAction::Paste {
            owner: DeviceChainOwner::Main,
            before: None,
        }),
    );
    let consent = offer(&h);
    assert!(consent.proceed());
    approved.set(true);
    let installed = loop {
        let event = next_running_event(&h);
        assert!(event.error.is_none(), "{:?}", event.error);
        if event.request_complete {
            break event;
        }
        assert_eq!(event.state.project, inserted.state.project);
    };
    let receipt = installed.device_insert_complete.unwrap();
    assert_eq!(receipt.outcome, DeviceInsertOutcome::Installed);
    let applied = status(&h).applied_audio_source().unwrap();
    assert_eq!(receipt.submission.audio_revision, applied.audio_revision);
    assert_eq!(receipt.submission.context, applied.context);
    assert_eq!(applied.output_identity, predecessor.output_identity);
    assert_eq!(installed.state.revision, inserted.state.revision + 1);
    let [retained, copy] = installed.state.project.main_devices() else {
        panic!("original and its copy");
    };
    assert_eq!(retained, original);
    assert_ne!(copy.id(), original.id());
    assert_eq!(copy.state(), original.state());
    let bindings = progress.lock().unwrap().source_bindings;
    assert_eq!(bindings[0].unwrap().1, original_binding);
    let (copied_device, copied_binding, _) = bindings[1].unwrap();
    assert_eq!(copied_device, copy.id().value());
    assert_ne!(copied_binding, original_binding);
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn verified_hosted_removal_undo_installs_a_fresh_catalog_worker() {
    let _native = native_fixture();
    let files = Files::new();
    let directory = files.0.canonicalize().unwrap();
    let mut selection = selection();
    let module = directory.join("history.vst3");
    super::ui::copy_original_module(&selection.module, &module);
    selection.module = module.clone();
    hex_ui::write_plugin_effect_test_catalog(&directory, &selection, "Fx", 0).unwrap();
    let progress = Arc::new(Mutex::new(
        crate::native_audio::plugins::PreparationObservation::default(),
    ));
    let mut h = super::super::verified_gain::native_harness_with(|sink| {
        sink.plugins = crate::native_audio::plugins::Plugins::with_fixture(host(), None);
        sink.plugins.catalog = Some(directory.join("plugin-catalog.json"));
        sink.plugins.progress = Some(Arc::clone(&progress));
    });
    h.next();
    h.open();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.open();
    h.applied(h.context.unwrap());
    let imported = h.run(Request::Import(files.wav(48_000, 131_072)));
    let track = imported.state.project.tracks()[0].id();
    h.applied(h.context.unwrap());
    let submit = |h: &Harness, request| h.service.as_ref().unwrap().submit(request).unwrap();
    let mut insertion = request(&imported.state, PluginEffectDestination::Track(track));
    insertion.selection = selection;
    submit(&h, Request::InsertPluginEffect(insertion));
    let inserted = loop {
        let event = h.next_with_audio();
        assert!(event.error.is_none(), "{:?}", event.error);
        if let Some(terminal) = event.plugin_effect_complete.first() {
            assert!(terminal.accepted);
            break event;
        }
    };
    let Some(Created::Device(vst)) = inserted.created else {
        panic!("fixture VST3");
    };
    h.applied(h.context.unwrap());
    wait(|| progress.lock().unwrap().source_bindings[0].is_some());
    let original = progress.lock().unwrap().source_bindings[0].unwrap().1;

    let removed = h.run_verified_device_insert(Command::RemoveDevice(vst));
    assert!(removed.state.project.device(vst).is_none());
    let predecessor = h.applied(h.context.unwrap());
    await_verified_retirement(&h, &progress);
    let applied = |h: &Harness| {
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_audio_source()
    };
    let terminal = |h: &mut Harness| loop {
        let event = h.next_with_audio();
        if event.request_complete {
            break event;
        }
    };
    let transport = |h: &mut Harness, command| {
        h.audio.as_mut().unwrap().control.command(command).unwrap();
    };
    let settle_transport = |h: &mut Harness, command| {
        transport(h, command);
        wait(|| {
            h.render();
            h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .paused_input_interval()
                .is_some_and(|(source, _)| source == predecessor)
        });
    };
    let check_refused = |event: &Event, h: &Harness| {
        assert!(event.error.is_some());
        assert_eq!(event.state.project, removed.state.project);
        assert_eq!(event.state.revision, removed.state.revision);
        assert_eq!(event.state.can_undo, removed.state.can_undo);
        assert_eq!(event.state.can_redo, removed.state.can_redo);
        assert_eq!(applied(h), Some(predecessor));
    };

    settle_transport(&mut h, PlaybackCommand::Stop);

    let unavailable = directory.join("temporarily-unavailable.vst3");
    fs::rename(&module, &unavailable).unwrap();
    submit(&h, Request::Undo);
    let refused = terminal(&mut h);
    assert!(
        refused
            .error
            .as_deref()
            .is_some_and(|message| message.contains("bundle missing")),
        "Undo refusal: {:?}",
        refused.error,
    );
    check_refused(&refused, &h);
    fs::rename(&unavailable, &module).unwrap();

    submit(&h, Request::Undo);
    let pending = h.next_with_audio();
    assert!(pending.error.is_none(), "{:?}", pending.error);
    assert!(!pending.request_complete);
    assert_eq!(pending.state.project, removed.state.project);
    assert_eq!(applied(&h), Some(predecessor));
    transport(&mut h, PlaybackCommand::Play);
    let interrupted = terminal(&mut h);
    check_refused(&interrupted, &h);
    settle_transport(&mut h, PlaybackCommand::Pause);

    submit(&h, Request::Undo);
    let pending = h.next_with_audio();
    assert!(pending.error.is_none(), "{:?}", pending.error);
    assert!(!pending.request_complete);
    assert_eq!(pending.state.project, removed.state.project);
    assert_eq!(applied(&h), Some(predecessor));
    let undone = terminal(&mut h);
    assert!(
        undone.error.is_none(),
        "catalog VST3 Undo refused: {:?}",
        undone.error
    );
    assert_eq!(
        undone.state.project.device(vst),
        inserted.state.project.device(vst)
    );
    let restored_source = h.applied(h.context.unwrap());
    let receipt = undone.device_insert_complete.unwrap();
    assert_eq!(
        receipt.outcome,
        hex_session::service::DeviceInsertOutcome::Installed
    );
    assert_eq!(
        receipt.submission.audio_revision,
        restored_source.audio_revision
    );
    assert_eq!(receipt.submission.context, restored_source.context);
    assert_eq!(
        receipt.submission.output_identity,
        predecessor.output_identity
    );
    let (_, restored, accepted) = progress.lock().unwrap().newest_source.unwrap();
    assert!(accepted);
    assert_ne!(restored, original, "Undo reused the removed VST3 worker");

    submit(&h, Request::Redo);
    let pending = h.next();
    assert!(!pending.request_complete);
    assert_eq!(pending.state.project, undone.state.project);
    assert_eq!(applied(&h), Some(restored_source));
    let redone = terminal(&mut h);
    assert!(redone.error.is_none(), "{:?}", redone.error);
    assert_eq!(redone.state.project, removed.state.project);
    assert_eq!(redone.state.revision, undone.state.revision + 1);
    let removed_source = h.applied(h.context.unwrap());
    let receipt = redone.device_insert_complete.unwrap();
    assert_eq!(
        receipt.outcome,
        hex_session::service::DeviceInsertOutcome::Installed
    );
    assert_eq!(
        receipt.submission.audio_revision,
        removed_source.audio_revision
    );
    assert_eq!(removed_source.output_identity, predecessor.output_identity);
    await_verified_retirement(&h, &progress);
}

pub(super) fn await_verified_retirement(
    h: &Harness,
    progress: &Mutex<crate::native_audio::plugins::PreparationObservation>,
) {
    let deadline = Instant::now() + Duration::from_secs(1);
    while progress.lock().unwrap().newest_source.is_some() {
        assert!(
            Instant::now() < deadline,
            "removed Verified VST3 stayed warm for history"
        );
        h.render();
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn verified_file_track_removal_keeps_one_compensated_hosted_return_worker() {
    use hex_session::service::DeviceInsertOutcome;

    const QUANTUM: u64 = 256;
    const LATENCY: u64 = 2 * QUANTUM;
    const FADE: u64 = 240;

    let _native = native_fixture();
    let first = Files::new();
    let second = Files::new();
    let directory = first.0.canonicalize().unwrap();
    hex_ui::write_plugin_effect_test_catalog(&directory, &selection(), "Fx", 0).unwrap();
    let progress = Arc::new(Mutex::new(
        crate::native_audio::plugins::PreparationObservation::default(),
    ));
    let mut h = super::super::verified_gain::native_harness_with(|sink| {
        sink.plugins = crate::native_audio::plugins::Plugins::with_fixture(host(), None);
        sink.plugins.progress = Some(Arc::clone(&progress));
        sink.plugins.catalog = Some(directory.join("plugin-catalog.json"));
    });
    h.next();
    h.open();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.open();
    h.applied(h.context.unwrap());
    let imported = h.run(Request::Import(first.wav(48_000, 131_072)));
    let removed = imported.state.project.tracks()[0].id();
    h.applied(h.context.unwrap());
    let imported = h.run(Request::Import(second.wav(48_000, 131_072)));
    let kept = imported.state.project.tracks()[1].id();
    h.applied(h.context.unwrap());
    let added = h.run(Request::Edit(Command::AddReturnTrack {
        name: "shared".into(),
    }));
    let Some(Created::ReturnTrack(bus)) = added.created else {
        panic!("shared Return");
    };
    h.applied(h.context.unwrap());
    let mut state = added.state;
    for track in [removed, kept] {
        let sent = h.run(Request::Edit(Command::SetReturnSend {
            track,
            return_track: bus,
            gain: 0.5,
        }));
        assert!(sent.error.is_none(), "{:?}", sent.error);
        state = sent.state;
        h.applied(h.context.unwrap());
    }
    h.service
        .as_ref()
        .unwrap()
        .submit(Request::InsertPluginEffect(request(
            &state,
            PluginEffectDestination::Return(bus),
        )))
        .unwrap();
    let before = loop {
        let event = h.next_with_audio();
        assert!(event.error.is_none(), "{:?}", event.error);
        if let Some(terminal) = event.plugin_effect_complete.first() {
            assert!(terminal.accepted);
            break event;
        }
    };
    let Some(Created::Device(device)) = before.created else {
        panic!("Return VST3");
    };
    let predecessor = h.applied(h.context.unwrap());
    wait(|| progress.lock().unwrap().source_bindings[0].is_some());
    let binding = progress.lock().unwrap().source_bindings[0].unwrap().1;
    fn status(h: &Harness) -> &hex_audio::prepared::Status {
        h.audio.as_ref().unwrap().status().unwrap()
    }
    let render = |h: &Harness| {
        let status = status(h);
        let start = status.performance_frame.load(Acquire);
        let mut pcm = [0.0; 512];
        wait(|| {
            pcm = render_hosted_pcm(h);
            let position = status.performance_frame.load(Acquire);
            assert!(position <= start + QUANTUM);
            if position == start {
                std::thread::sleep(Duration::from_millis(1));
            }
            position == start + QUANTUM
        });
        wait(|| {
            let (actual_device, actual_binding, blocks) =
                progress.lock().unwrap().source_bindings[0].unwrap();
            assert_eq!((actual_device, actual_binding), (device.value(), binding));
            assert!(blocks.unwrap_or(0) <= start / QUANTUM + 1);
            blocks.is_some_and(|blocks| blocks == start / QUANTUM + 1)
        });
        (start, pcm)
    };
    let expected = |input: u64, removed_gain: f32| {
        let sum = 0.25 + 0.25 * removed_gain;
        let effect_gain = 0.5 + (input / QUANTUM) as f32 * 0.125;
        sum + sum * 0.5 * effect_gain
    };
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for _ in 0..4 {
        let (start, pcm) = render(&h);
        let sample = start
            .checked_sub(LATENCY)
            .map_or(0.0, |input| expected(input, 1.0));
        assert!(pcm.iter().all(|value| (*value - sample).abs() < 1e-5));
    }
    let pending = h.run(Request::Edit(Command::RemoveTrack(removed)));
    assert!(!pending.request_complete);
    assert_eq!(pending.state.project, before.state.project);
    wait(|| status(&h).publication_in_flight());

    let mut fade_start = None;
    let mut successor = None;
    for _ in 0..12 {
        let (start, pcm) = render(&h);
        for (offset, frame) in pcm.chunks_exact(2).enumerate() {
            let input = start + offset as u64 - LATENCY;
            if fade_start.is_none() && (frame[0] - expected(input, 1.0)).abs() > 1e-5 {
                assert_eq!(
                    offset, 0,
                    "the callback starts the fade at a block boundary"
                );
                fade_start = Some(input);
            }
            let gain = fade_start.map_or(1.0, |first| {
                FADE.saturating_sub(input - first + 1) as f32 / FADE as f32
            });
            let sample = expected(input, gain);
            assert!(
                frame.iter().all(|value| (*value - sample).abs() < 1e-5),
                "input frame {input}: {frame:?} vs {sample}"
            );
        }
        let applied = status(&h).applied_audio_source().unwrap();
        if applied != predecessor {
            let fade_start = fade_start.expect("drained track fade");
            assert!(start >= fade_start + LATENCY + FADE - 1);
            assert_eq!(applied.output_identity, predecessor.output_identity);
            successor = Some(applied);
            break;
        }
    }
    let successor = successor.expect("exact successor");
    let completed = h.next();
    assert!(completed.request_complete && completed.error.is_none());
    assert!(completed.device_insert_confirmation.is_none());
    let receipt = completed.device_insert_complete.unwrap();
    assert_eq!(receipt.outcome, DeviceInsertOutcome::Installed);
    assert_eq!(receipt.submission.audio_revision, successor.audio_revision);
    assert_eq!(completed.state.revision, before.state.revision + 1);
    assert!(completed.state.project.track(removed).is_none());
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn verified_hosted_removal_fades_and_resumes_on_the_same_output() {
    use hex_session::service::{DeviceInsertOutcome, GraphEditOperation};

    const FADE: usize = 128;
    const QUANTUM: u64 = 256;

    let _native = native_fixture();
    let files = Files::new();
    let directory = files.0.canonicalize().unwrap();
    hex_ui::write_plugin_effect_test_catalog(&directory, &selection(), "Fx", 0).unwrap();
    let progress = Arc::new(Mutex::new(
        crate::native_audio::plugins::PreparationObservation::default(),
    ));
    let mut h = super::super::verified_gain::native_harness_with(|sink| {
        sink.plugins = crate::native_audio::plugins::Plugins::with_fixture(host(), None);
        sink.plugins.progress = Some(Arc::clone(&progress));
        sink.plugins.catalog = Some(directory.join("plugin-catalog.json"));
    });
    h.next();
    h.open();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.open();
    h.applied(h.context.unwrap());
    let imported = h.run(Request::Import(files.wav(48_000, 131_072)));
    let track = imported.state.project.tracks()[0].id();
    h.applied(h.context.unwrap());
    h.service
        .as_ref()
        .unwrap()
        .submit(Request::InsertPluginEffect(request(
            &imported.state,
            PluginEffectDestination::Track(track),
        )))
        .unwrap();
    let inserted = loop {
        let event = h.next_with_audio();
        assert!(event.error.is_none(), "{:?}", event.error);
        if let Some(terminal) = event.plugin_effect_complete.first() {
            assert!(terminal.accepted);
            break event;
        }
    };
    let Some(Created::Device(vst)) = inserted.created else {
        panic!("fixture VST3");
    };
    let predecessor = h.applied(h.context.unwrap());
    fn status(h: &Harness) -> &hex_audio::prepared::Status {
        h.audio.as_ref().unwrap().status().unwrap()
    }
    wait(|| progress.lock().unwrap().source_bindings[0].is_some());
    let binding = progress.lock().unwrap().source_bindings[0].unwrap().1;
    let observe = || {
        let observation = progress.lock().unwrap();
        let (device, current, blocks) = observation.source_bindings[0].unwrap();
        assert_eq!((device, current), (vst.value(), binding));
        blocks.unwrap_or(0)
    };
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let original_sample = |frame: u64| {
        // The signed effect steps its gain per quantum and has a two-quantum bridge.
        (frame / QUANTUM)
            .checked_sub(2)
            .map_or(0.0, |input| 0.25 * (0.5 + input as f32 * 0.125))
    };
    let play_original = |h: &Harness| {
        let status = status(h);
        let start = status.performance_frame.load(Acquire);
        let mut pcm = [0.0; 512];
        wait(|| {
            pcm = render_hosted_pcm(h);
            let current = status.performance_frame.load(Acquire);
            if current == start {
                std::thread::sleep(Duration::from_millis(1));
            }
            current > start
        });
        assert_eq!(status.performance_frame.load(Acquire), start + QUANTUM);
        wait(|| observe() > start / QUANTUM);
        let sample = original_sample(start);
        assert!(pcm.iter().all(|value| (*value - sample).abs() < 1e-5));
    };
    for _ in 0..4 {
        play_original(&h);
    }
    for proceed in [false, true] {
        let pending = h.run(Request::Edit(Command::RemoveDevice(vst)));
        assert!(!pending.request_complete);
        assert_eq!(pending.state.project, inserted.state.project);
        wait(|| status(&h).publication_in_flight());
        play_original(&h);
        let proposed = h.next();
        let offer = proposed
            .device_insert_confirmation
            .expect("exact removal offer");
        assert_eq!(offer.operation(), GraphEditOperation::RemoveDevice);
        assert_eq!(
            offer.submission().output_identity,
            predecessor.output_identity
        );
        assert_eq!(proposed.state.project, inserted.state.project);
        assert_eq!(status(&h).applied_audio_source(), Some(predecessor));
        play_original(&h);
        if !proceed {
            assert!(offer.cancel());
            let cancelled = h.next();
            assert_eq!(cancelled.state.project, inserted.state.project);
            assert_eq!(
                cancelled.device_insert_complete.unwrap().outcome,
                DeviceInsertOutcome::Cancelled
            );
            play_original(&h);
            continue;
        }
        assert!(offer.proceed());
        wait(|| status(&h).publication_in_flight());
        let deadline = Instant::now() + Duration::from_millis(300);
        let (cut_start, faded) = loop {
            let start = status(&h).position.load(Acquire);
            let faded = render_hosted_pcm(&h);
            let position = status(&h).position.load(Acquire);
            if position == start + FADE as u64 {
                break (start, faded);
            }
            assert_eq!(
                position,
                start + QUANTUM,
                "while waiting for the approved cut"
            );
            assert_eq!(status(&h).applied_audio_source(), Some(predecessor));
            let sample = original_sample(start);
            assert!(faded.iter().all(|value| (*value - sample).abs() < 1e-5));
            wait(|| observe() > start / QUANTUM);
            assert!(Instant::now() < deadline, "approved cut did not start");
        };
        assert_eq!(status(&h).applied_audio_source(), Some(predecessor));
        let sample = original_sample(cut_start);
        for (index, frame) in faded.chunks_exact(2).enumerate() {
            let expected = sample * FADE.saturating_sub(index + 1) as f32 / FADE as f32;
            assert!(frame.iter().all(|value| (*value - expected).abs() < 1e-5));
        }
        let resumed = render_hosted_pcm(&h);
        assert_eq!(
            status(&h).position.load(Acquire),
            cut_start + FADE as u64 + QUANTUM
        );
        for (index, frame) in resumed.chunks_exact(2).enumerate() {
            let expected = 0.25 * (index + 1).min(FADE) as f32 / FADE as f32;
            assert!(frame.iter().all(|value| (*value - expected).abs() < 1e-5));
        }
        let removed = h.next();
        assert!(removed.error.is_none(), "{:?}", removed.error);
        let receipt = removed.device_insert_complete.unwrap();
        assert_eq!(receipt.outcome, DeviceInsertOutcome::Installed);
        assert!(removed.state.project.device(vst).is_none());
        assert_eq!(removed.state.revision, inserted.state.revision + 1);
        let active = status(&h).applied_audio_source().unwrap();
        assert_eq!(active.context, h.context.unwrap());
        assert_eq!(active.audio_revision, receipt.submission.audio_revision);
        assert_eq!(active.output_identity, predecessor.output_identity);
        await_verified_retirement(&h, &progress);
        assert!(status(&h).playing.load(Acquire));
    }
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn verified_hosted_insert_keeps_the_eq_gesture_commit() {
    let _native = native_fixture();
    let files = Files::new();
    let HeldVerifiedEq {
        mut h,
        operation,
        device,
        key,
        resume_facts,
        progress,
        ..
    } = hold_verified_eq_preview(&files);
    let committed = commit_held_eq(&mut h, key, device);
    let chosen = h.context.unwrap();
    resume_facts.send(()).unwrap();
    wait(|| {
        let observed = progress.lock().unwrap();
        observed.facts_ready_for_session || observed.facts_attempt_from_session
    });
    h.run(Request::QueryDeviceAutomationCloseSafety { request_id: 27 });
    assert!(
        !progress.lock().unwrap().facts_attempt_from_session,
        "native facts must wait for the exact canonical EQ receipt"
    );
    let status = h.audio.as_ref().unwrap().status().unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let applied = loop {
        render_hosted_quantum(&h);
        if let Some(source) = status.applied_audio_source()
            && source.context == chosen
        {
            break source;
        }
        assert!(Instant::now() < deadline, "canonical EQ was not applied");
        std::thread::sleep(Duration::from_millis(1));
    };
    loop {
        let Ok(mut event) = h.service.as_ref().unwrap().try_event() else {
            render_hosted_quantum(&h);
            assert!(Instant::now() < deadline, "VST3 insertion did not finish");
            std::thread::sleep(Duration::from_millis(1));
            continue;
        };
        assert!(event.error.is_none(), "{:?}", event.error);
        if let Some(offer) = event.device_insert_confirmation.take() {
            assert!(offer.proceed());
        }
        if let Some(terminal) = event.plugin_effect_complete.first() {
            assert!(Arc::ptr_eq(&terminal.control, &operation) && terminal.accepted);
            assert_eq!(event.state.revision, committed.state.revision + 1);
            assert_eq!(
                event.state.project.device(device),
                committed.state.project.device(device)
            );
            let installed = status.applied_audio_source().unwrap();
            assert_eq!(
                installed.context,
                event
                    .state
                    .canonical_audio_source
                    .as_ref()
                    .unwrap()
                    .context()
            );
            assert!(installed.audio_revision > applied.audio_revision);
            break;
        }
    }
}

fn commit_held_eq(h: &mut Harness, key: GestureKey, device: hex_project::DeviceId) -> Event {
    let committed = eq_gesture(
        h,
        GestureRequest::Commit {
            key,
            sequence: 2,
            manual_edit: true,
            value: ParameterValue::Float(0.5),
        },
    );
    assert!(committed.gesture_ack.as_ref().unwrap().accepted);
    assert!(committed.plugin_effect_complete.is_empty());
    assert_eq!(
        hex_engine::native_device_parameter(
            committed.state.project.device(device).unwrap().state(),
            hex_engine::eq::OUTPUT_GAIN,
        )
        .unwrap(),
        ParameterValue::Float(0.5),
    );
    assert_ne!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_audio_source()
            .unwrap()
            .context,
        h.context.unwrap(),
        "the canonical EQ callback has not run",
    );
    committed
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn verified_hosted_insert_cancel_and_fault_preserve_the_accepted_eq() {
    let _native = native_fixture();
    for fault in [false, true] {
        let files = Files::new();
        let HeldVerifiedEq {
            mut h,
            operation,
            device,
            key,
            resume_facts: _hold,
            ..
        } = hold_verified_eq_preview(&files);
        let committed = commit_held_eq(&mut h, key, device);
        let context = h.context.unwrap();
        let output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
        if fault {
            h.backend
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .report_device_error()
                .unwrap();
        } else {
            assert!(operation.cancel());
        }
        loop {
            let event = h.next();
            assert_eq!(event.state.project, committed.state.project);
            assert_eq!(event.state.revision, committed.state.revision);
            if let Some(terminal) = event.plugin_effect_complete.first() {
                assert!(Arc::ptr_eq(&terminal.control, &operation) && !terminal.accepted);
                if fault {
                    assert!(
                        event
                            .error
                            .as_deref()
                            .unwrap()
                            .contains("audio output fault")
                    );
                }
                break;
            }
        }
        if fault {
            h.audio.take().unwrap().control.close();
            h.backend.lock().unwrap().take();
            let restarted = h.run(Request::RestartAudio {
                project_generation: context.project_generation,
                request_id: 19,
            });
            assert_eq!(restarted.state.project, committed.state.project);
            h.open();
            assert_ne!(
                h.audio.as_ref().unwrap().control.output_identity(),
                Some(output)
            );
            h.applied(context);
            h.audio
                .as_mut()
                .unwrap()
                .control
                .command(PlaybackCommand::Play)
                .unwrap();
        } else {
            assert_eq!(h.applied(context).output_identity, output);
        }
        expect_verified_sample(&h, 0.125);
    }
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn verified_hosted_insert_installs_original_worker_and_retains_it_for_eq_edit() {
    let _native = native_fixture();
    let progress = Arc::new(Mutex::new(
        crate::native_audio::plugins::PreparationObservation::default(),
    ));
    let (reached, facts) = mpsc::sync_channel(1);
    let (_resume, resumed) = mpsc::sync_channel(1);
    let mut h = super::super::verified_gain::native_harness_with(|sink| {
        sink.plugins = crate::native_audio::plugins::Plugins::with_fixture(host(), None);
        sink.plugins.progress = Some(Arc::clone(&progress));
        sink.plugins.facts_gate = Some(crate::native_audio::plugins::FactsGate::new(
            reached, resumed,
        ));
    });
    let files = Files::new();
    let send = |h: &Harness, request| h.service.as_ref().unwrap().submit(request).unwrap();
    h.next();
    h.open();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.open();
    h.applied(h.context.unwrap());
    let imported = h.run(Request::Import(files.wav(48_000, 131_072)));
    let track = imported.state.project.tracks()[0].id();
    h.applied(h.context.unwrap());
    let inserted = h.run_verified_device_insert(Command::InsertDevice {
        track,
        before: None,
        state: hex_engine::eq::EqState::default().encode().unwrap(),
    });
    let Some(Created::Device(eq)) = inserted.created else {
        panic!("fixture EQ")
    };
    h.applied(h.context.unwrap());
    let state = inserted.state;
    let destination = PluginEffectDestination::Track(track);
    let mut invalid = request(&state, destination);
    invalid.selection.executable_sha256 = "0".repeat(64);
    let invalid_operation = Arc::clone(&invalid.control);
    send(&h, Request::InsertPluginEffect(invalid));
    let refused = h.next_with_audio();
    assert!(refused.error.is_some());
    assert_eq!(refused.state.project, state.project);
    assert_eq!(refused.state.revision, state.revision);
    assert!(Arc::ptr_eq(
        &refused.plugin_effect_complete[0].control,
        &invalid_operation
    ));
    assert!(!refused.plugin_effect_complete[0].accepted);

    let cancelled = request(&state, destination);
    let cancelled_operation = Arc::clone(&cancelled.control);
    send(&h, Request::InsertPluginEffect(cancelled));
    assert!(h.next_with_audio().plugin_effect_complete.is_empty());
    facts.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(cancelled_operation.cancel());
    loop {
        let event = h.next_with_audio();
        assert_eq!(event.state.project, state.project);
        assert_eq!(event.state.revision, state.revision);
        assert!(event.device_insert_complete.is_none());
        if let Some(terminal) = event.plugin_effect_complete.first() {
            assert!(Arc::ptr_eq(&terminal.control, &cancelled_operation));
            assert!(!terminal.accepted);
            break;
        }
    }

    let insert = request(&state, destination);
    let operation = Arc::clone(&insert.control);
    h.audio
        .as_ref()
        .unwrap()
        .control
        .set_audio_diagnostics(AudioDiagnosticsFlags::READINESS);
    send(&h, Request::InsertPluginEffect(insert));
    let accepted = loop {
        let mut event = h.next_with_audio();
        if let Some(error) = event.error.as_ref() {
            let control = &mut h.audio.as_mut().unwrap().control;
            let handoff: Vec<_> = std::iter::from_fn(|| control.take_audio_diagnostic())
                .filter_map(|trace| match trace.kind {
                    AudioDiagnosticKind::StreamPreflightFailed { handoff, .. } => handoff,
                    _ => None,
                })
                .collect();
            panic!("verified insert refused: {error}; handoff={handoff:?}");
        }
        if let Some(offer) = event.device_insert_confirmation.take() {
            assert!(offer.proceed());
        }
        if let Some(terminal) = event
            .plugin_effect_complete
            .iter()
            .find(|terminal| Arc::ptr_eq(&terminal.control, &operation))
        {
            assert!(terminal.accepted);
            break event;
        }
        assert_eq!(event.state.project, state.project);
    };
    let Some(Created::Device(vst)) = accepted.created else {
        panic!("fixture VST3")
    };
    assert_eq!(
        accepted.device_insert_complete.unwrap().outcome,
        hex_session::service::DeviceInsertOutcome::Installed
    );
    let started = h.applied(h.context.unwrap());
    wait(|| progress.lock().unwrap().source_bindings[0].is_some());
    let (device, binding, _) = progress.lock().unwrap().source_bindings[0].unwrap();
    assert_eq!(device, vst.value());
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let render = |h: &Harness, block: u64| {
        let status = h.audio.as_ref().unwrap().status().unwrap();
        let start = block * 256;
        assert_eq!(status.performance_frame.load(Acquire), start);
        let mut sample = 0.0;
        wait(|| {
            sample = render_hosted_quantum(h);
            let position = status.performance_frame.load(Acquire);
            assert!(position <= start + 256);
            if position == start {
                std::thread::sleep(Duration::from_millis(1));
            }
            position == start + 256
        });
        wait(|| {
            progress.lock().unwrap().source_bindings[0].is_some_and(
                |(actual_device, actual_binding, count)| {
                    assert_eq!((actual_device, actual_binding), (device, binding));
                    count.is_some_and(|count| count > block)
                },
            )
        });
        sample
    };
    let original = |block: u64| {
        // The verified WAV is 0.25. The original SDK fixture increases gain
        // by 0.125 per block, delivered two physical quanta after input.
        block
            .checked_sub(2)
            .map_or(0.0, |input| 0.25 * (0.5 + input as f32 * 0.125))
    };
    for block in 0..4 {
        let sample = render(&h, block);
        assert!(
            (sample - original(block)).abs() < 1e-5,
            "block {block}: {sample}"
        );
    }

    h.run(Request::SetDeviceParameter {
        device: eq,
        parameter: hex_engine::eq::OUTPUT_GAIN,
        value: ParameterValue::Float(0.5),
    });
    let target = h.context.unwrap();
    let mut first = None;
    for block in 4..32 {
        let sample = render(&h, block);
        let applied = h
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_audio_source()
            .unwrap();
        if applied.context == target {
            assert!(applied.audio_revision > started.audio_revision);
            let first = *first.get_or_insert(block);
            // Allow the EQ handoff ramp and the bridge's two queued quanta
            // to finish before comparing steady-state output.
            if block >= first + 5 {
                assert!(
                    (sample - 0.5 * original(block)).abs() < 1e-5,
                    "block {block}: {sample}"
                );
                if block > first + 5 {
                    return;
                }
            }
        }
        // Let the Session publisher prepare and queue the exact successor
        // between deterministic synthetic host callbacks.
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("verified eq successor did not render");
}

pub(super) fn render_hosted_quantum(h: &Harness) -> f32 {
    render_hosted_pcm(h)[510]
}

fn render_hosted_pcm(h: &Harness) -> [f32; 512] {
    let mut block = [[0.0; 2]; 256];
    let mut host = [0.0; 512];
    let mut slot = h.backend.lock().unwrap();
    let backend = slot.as_mut().unwrap();
    let counts = allocation_counter::measure(|| {
        backend.render_native_direct(&mut block, &mut host).unwrap();
    });
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0),
    );
    host
}
