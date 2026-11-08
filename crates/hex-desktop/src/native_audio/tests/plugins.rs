//! Actual Session/desktop/native Root workflow; no device or app execution.
mod automation;
mod instruments;
mod performance_profile;
mod state;
mod ui;
use super::resident_preview::ResidentHarness;
use super::*;
use hex_audio::plugin_effects::{EffectPresentation, Hosting};
use hex_project::{Command, Created};
use hex_session::service::{
    PluginEffectControl, PluginEffectDestination, PluginEffectInsertion, PluginEffectSelection,
};

fn host() -> Hosting {
    if let Some(app) = std::env::var_os("HEX_TEST_VST3_LAUNCH_INSTANCE_APP") {
        return Hosting::inspect_source_launch_fixture_on_control(&PathBuf::from(app)).unwrap();
    }
    Hosting::inspect_fixture_on_control(&PathBuf::from(
        std::env::var_os("HEX_VST3_INSTANCE_RECEIPT").unwrap(),
    ))
    .unwrap()
}
fn selection() -> PluginEffectSelection {
    let receipt = PathBuf::from(std::env::var_os("HEX_VST3_MACOS_RECEIPT").unwrap());
    let module = receipt
        .parent()
        .unwrap()
        .join("hex-vst3-host.app/Contents/Resources/fixtures/processing.vst3");
    let cid = [
        0x48, 0x45, 0x58, 0x50, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12,
    ];
    let executable = "Contents/MacOS/fixture".to_string();
    let (code, info) = Hosting::inspect_fixture_binary(&module, &executable, cid).unwrap();
    let hex = |bytes: [u8; 32]| bytes.iter().map(|b| format!("{b:02x}")).collect();
    PluginEffectSelection {
        module,
        executable,
        cid,
        executable_sha256: hex(code),
        info_sha256: hex(info),
    }
}
fn installed_selection(
    catalog: &std::path::Path,
    bundle: &str,
    role: hex_plugin_catalog::PluginRole,
) -> PluginEffectSelection {
    let catalog = hex_plugin_catalog::Catalog::load(catalog).unwrap();
    let selected = catalog
        .rows("")
        .into_iter()
        .find_map(|row| {
            (row.module.file_name()?.to_str()? == bundle)
                .then_some(row.selection?)
                .filter(|selection| selection.role == role)
        })
        .unwrap();
    PluginEffectSelection {
        module: selected.module.clone(),
        executable: selected.executable_relative_path.clone(),
        cid: selected.cid,
        executable_sha256: selected.executable_sha256.clone(),
        info_sha256: selected.info_sha256.clone(),
    }
}

fn request(
    state: &hex_session::service::ViewState,
    destination: PluginEffectDestination,
) -> PluginEffectInsertion {
    PluginEffectInsertion {
        context: AudioSourceContext {
            project_generation: state
                .canonical_audio_source
                .as_ref()
                .unwrap()
                .context()
                .project_generation,
            editor_revision: state.revision,
        },
        selection: selection(),
        destination,
        before: None,
        control: Arc::new(PluginEffectControl::default()),
    }
}
fn wait(mut ready: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(
            Instant::now() < end,
            "actual native coordinator progress timed out"
        );
        std::thread::yield_now();
    }
}
fn terminal(h: &mut ResidentHarness) -> Event {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        h.render();
        if let Ok(event) = h.service.try_event()
            && !event.plugin_effect_complete.is_empty()
        {
            return event;
        }
        assert!(
            Instant::now() < end,
            "matching native insertion terminal timed out"
        );
        std::thread::yield_now();
    }
}

fn restored_terminal(h: &mut ResidentHarness) -> Event {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        h.render();
        if let Ok(event) = h.service.try_event()
            && event.stopped_playback_restore_complete
        {
            return event;
        }
        assert!(Instant::now() < end, "stopped restore terminal timed out");
        std::thread::yield_now();
    }
}

/// These effect-only edits reuse an already installed immutable file domain.
/// Keep the simulated device held until the existing precommit worker queues
/// its graph; CPU scheduling must not choose the musical cut in a PCM oracle.
fn same_source_edit(h: &mut ResidentHarness, request: Request) -> Event {
    h.service.submit(request).unwrap();
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        if let Ok(event) = h.service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            assert!(event.playback.is_none());
            return event;
        }
        assert!(
            Instant::now() < end,
            "same-source effect edit did not queue without advancing the device"
        );
        std::thread::yield_now();
    }
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn native_effect_coordinator_waits_for_actual_endpoint_then_commits_track_return_main() {
    let _native = native_fixture();
    for location in 0..3 {
        // A headless sink has no physical-device contribution. This original
        // zero-latency fixture has a known2Q copied bridge and no downstream
        // processor; file input acquisition contributes0. No hardware claim.
        let mut h = ResidentHarness::with_plugins(
            host(),
            Some(EffectPresentation {
                input_frames: 0,
                output_frames: 512,
            }),
        );
        assert!(h.next().error.is_none());
        let (mut applied, _) = h.applied(0);
        let files = Files::new();
        let mut state = h.run(Request::Import(files.wav(48_000, 2048))).state;
        (applied, _) = h.applied(applied);
        let track = state.project.tracks()[0].id();
        let destination = if location == 1 {
            let added = h.run(Request::Edit(Command::AddReturnTrack {
                name: "effect".into(),
            }));
            (applied, _) = h.applied(applied);
            let Some(Created::ReturnTrack(bus)) = added.created else {
                panic!("return")
            };
            state = h
                .run(Request::Edit(Command::SetReturnSend {
                    track,
                    return_track: bus,
                    gain: 0.5,
                }))
                .state;
            (applied, _) = h.applied(applied);
            PluginEffectDestination::Return(bus)
        } else if location == 0 {
            PluginEffectDestination::Track(track)
        } else {
            PluginEffectDestination::Main
        };
        let input = request(&state, destination);
        h.service
            .submit(Request::InsertPluginEffect(input.clone()))
            .unwrap();
        let pending = h.next();
        assert!(pending.error.is_none(), "{:?}", pending.error);
        assert!(pending.request_complete && pending.plugin_effect_complete.is_empty());
        assert_eq!(pending.state.project, state.project);
        // Hold the real callback. Observe an actual queued endpoint, then prove
        // it did not become canonical while its installation is still withheld.
        wait(|| h.plugin_queued.load(Acquire));
        assert!(matches!(
            h.service.try_event(),
            Err(mpsc::TryRecvError::Empty)
        ));
        let accepted = terminal(&mut h);
        assert!(accepted.error.is_none(), "{:?}", accepted.error);
        assert!(!accepted.request_complete);
        assert_eq!(accepted.plugin_effect_complete.len(), 1);
        assert!(accepted.plugin_effect_complete[0].accepted);
        assert!(Arc::ptr_eq(
            &accepted.plugin_effect_complete[0].control,
            &input.control
        ));
        assert_eq!(accepted.state.revision, state.revision + 1);
        assert!(
            matches!(accepted.created, Some(Created::Device(id)) if id.value() == state.project.next_identity())
        );
        assert_eq!(
            accepted.state.project.track(track).unwrap().clips(),
            state.project.track(track).unwrap().clips(),
            "effect insertion changed authored clips"
        );
        assert_eq!(
            accepted.state.project.assets().collect::<Vec<_>>(),
            state.project.assets().collect::<Vec<_>>(),
            "effect insertion changed original media references"
        );
        h.applied(applied);
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
        let completed_before_play = h.plugin_completed.load(Acquire);
        let mut output = Vec::new();
        for block in 0..10 {
            output.extend(h.render_plugin_block());
            wait(|| h.plugin_completed.load(Acquire) > block);
        }
        assert!(output[..512].iter().all(|frame| *frame == [0.0; 2]));
        assert_eq!(
            completed_before_play, 0,
            "fresh stopped effect dispatched unexpected DSP"
        );
        // The independent original SDK fixture deliberately ramps its gain by
        // native block:0.5 + block/8. This distinguishes replayed/dropped blocks
        // from a renderer that merely happens to output the first correct gain.
        let expected_at = |index: usize| {
            let scale = 0.5 + ((index - 512) / 256) as f32 * 0.125;
            if location == 1 {
                [0.25 + 0.125 * scale; 2]
            } else {
                [0.25 * scale; 2]
            }
        };
        let mismatch = output
            .iter()
            .enumerate()
            .skip(512)
            .find(|(index, frame)| **frame != expected_at(*index));
        assert!(
            mismatch.is_none(),
            "native PCM differs at location{location}: first={mismatch:?}, completed={}, fault={:?}",
            h.plugin_completed.load(Acquire),
            h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .render_fault_snapshot()
        );
    }
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn retained_native_document_retires_the_outgoing_effect_after_exact_installation() {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_plugins(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
    );
    assert!(h.next().error.is_none());
    let (initial, _) = h.applied(0);
    let imported = h.run(Request::Import(files.wav(48_000, 2048)));
    let (imported_revision, _) = h.applied(initial);
    h.service
        .submit(Request::InsertPluginEffect(request(
            &imported.state,
            PluginEffectDestination::Main,
        )))
        .unwrap();
    assert!(h.next().error.is_none());
    let inserted = terminal(&mut h);
    assert!(inserted.plugin_effect_complete[0].accepted);
    let (applied, metadata) = h.applied(imported_revision);
    let old = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .recording_source()
        .unwrap();
    assert_eq!(old.audio_revision, applied);
    assert_eq!(old.context, metadata.context);
    wait(|| h.plugin_progress.lock().unwrap().accepted);
    let original = h.plugin_progress.lock().unwrap().binding.unwrap();
    let new_project = || Request::NewWithAudioMode {
        mode: DocumentAudioMode::Resident,
        reservation: None,
    };

    h.refuse.store(true, std::sync::atomic::Ordering::Release);
    h.service.submit(new_project()).unwrap();
    let refused = h.next();
    assert!(!refused.project_replaced && refused.error.is_some());
    assert_eq!(refused.state.project, inserted.state.project);
    assert_eq!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .recording_source(),
        Some(old)
    );
    assert!(!h.plugin_progress.lock().unwrap().retiring);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for block in 0..3 {
        let pcm = h.render_plugin_block();
        wait(|| h.plugin_completed.load(Acquire) > block);
        assert_eq!(pcm, [[if block == 2 { 0.125 } else { 0.0 }; 2]; 256]);
    }
    h.audio.as_ref().unwrap().control.stop();
    h.render_plugin_block();

    h.refuse.store(false, std::sync::atomic::Ordering::Release);
    let new = h.run(new_project());
    let before_installation = *h.plugin_progress.lock().unwrap();
    assert_eq!(before_installation.binding, Some(original));
    assert!(!before_installation.retiring);
    let applied = super::resident_lifecycle::finish_replacement(&mut h, &new, old.output_identity);
    let audio = h.audio.as_ref().unwrap();
    assert_eq!(audio.control.output_identity(), Some(old.output_identity));
    assert!(applied > old.audio_revision);
    assert!(new.state.project.main_devices().is_empty());
    wait(|| {
        h.pump();
        h.render_plugin_block();
        h.plugin_records.load(Acquire) == 0 && h.plugin_progress.lock().unwrap().binding.is_none()
    });
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    assert_eq!(h.render_plugin_block(), [[0.0; 2]; 256]);
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn native_effect_coordinator_restores_committed_effect_after_stop() {
    let _native = native_fixture();
    for location in 0..3 {
        let mut h = ResidentHarness::with_plugins(
            host(),
            Some(EffectPresentation {
                input_frames: 0,
                output_frames: 512,
            }),
        );
        assert!(h.next().error.is_none());
        let (mut applied, _) = h.applied(0);
        let files = Files::new();
        let mut state = h.run(Request::Import(files.wav(48_000, 2048))).state;
        (applied, _) = h.applied(applied);
        let track = state.project.tracks()[0].id();
        let destination = match location {
            0 => PluginEffectDestination::Track(track),
            1 => {
                let added = h.run(Request::Edit(Command::AddReturnTrack {
                    name: "effect".into(),
                }));
                (applied, _) = h.applied(applied);
                let Some(Created::ReturnTrack(bus)) = added.created else {
                    panic!("return")
                };
                state = h
                    .run(Request::Edit(Command::SetReturnSend {
                        track,
                        return_track: bus,
                        gain: 0.5,
                    }))
                    .state;
                (applied, _) = h.applied(applied);
                PluginEffectDestination::Return(bus)
            }
            _ => PluginEffectDestination::Main,
        };
        h.service
            .submit(Request::InsertPluginEffect(request(&state, destination)))
            .unwrap();
        assert!(h.next().error.is_none());
        let accepted = terminal(&mut h);
        assert!(accepted.error.is_none(), "{:?}", accepted.error);
        assert!(accepted.plugin_effect_complete[0].accepted);
        (applied, _) = h.applied(applied);
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
        h.render_plugin_block();
        wait(|| h.plugin_completed.load(Acquire) == 1);
        h.audio.as_ref().unwrap().control.stop();
        h.render_plugin_block();
        h.service.submit(Request::RestoreStoppedPlayback).unwrap();
        let pending = h.next();
        assert!(pending.error.is_none(), "{:?}", pending.error);
        assert!(!pending.stopped_playback_restore_complete);
        assert_eq!(pending.state.project, accepted.state.project);
        assert!(matches!(
            h.service.try_event(),
            Err(mpsc::TryRecvError::Empty)
        ));
        assert!(
            h.audio
                .as_mut()
                .unwrap()
                .control
                .command(PlaybackCommand::Play)
                .is_err(),
            "held restore fence admitted Play before returns and native ACK"
        );
        let end = Instant::now() + Duration::from_secs(10);
        let restored = loop {
            h.pump();
            h.render();
            if let Ok(event) = h.service.try_event()
                && event.stopped_playback_restore_complete
            {
                break event;
            }
            assert!(Instant::now() < end, "stopped restore terminal timed out");
            std::thread::yield_now();
        };
        assert!(restored.error.is_none(), "{:?}", restored.error);
        assert!(restored.stopped_playback_restore_complete);
        assert_eq!(restored.state.project, accepted.state.project);
        assert_eq!(restored.state.revision, accepted.state.revision);
        assert_eq!(restored.state.can_undo, accepted.state.can_undo);
        let (_, metadata) = h.applied(applied);
        assert!(metadata.stopped_restore);
        // A new endpoint epoch has a fresh transport completion prefix; the
        // native child's independent DSP gain-ramp counter remains at1.
        wait(|| h.plugin_completed.load(Acquire) == 0);
        wait(|| {
            h.audio
                .as_mut()
                .unwrap()
                .control
                .command(PlaybackCommand::Play)
                .is_ok()
        });
        for block in 0..3 {
            let output = h.render_plugin_block();
            wait(|| h.plugin_completed.load(Acquire) > block);
            if block < 2 {
                // Return-path compensation delays the dry path too, as in the
                // original insertion PCM oracle above.
                assert!(output.iter().all(|frame| *frame == [0.0; 2]));
            } else {
                // The original child keeps its gain-ramp history across Stop.
                let expected = if location == 1 {
                    0.25 + 0.125 * 0.625
                } else {
                    0.25 * 0.625
                };
                assert!(
                    output.iter().all(|frame| *frame == [expected; 2]),
                    "location{location}, first={:?}, expected={expected}",
                    output[0]
                );
            }
        }
    }
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn native_effect_pending_restore_document_change_does_not_strand_next_output() {
    let _native = native_fixture();
    let mut h = ResidentHarness::with_plugins(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
    );
    let initial = h.next();
    assert!(initial.error.is_none());
    let (applied, _) = h.applied(0);
    let old_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    h.service
        .submit(Request::InsertPluginEffect(request(
            &initial.state,
            PluginEffectDestination::Main,
        )))
        .unwrap();
    assert!(h.next().error.is_none());
    let accepted = terminal(&mut h);
    assert!(accepted.error.is_none(), "{:?}", accepted.error);
    h.applied(applied);
    h.audio.as_ref().unwrap().control.stop();
    h.render();
    h.service.submit(Request::RestoreStoppedPlayback).unwrap();
    let pending = h.next();
    assert!(pending.error.is_none());
    assert!(!pending.stopped_playback_restore_complete);
    // Hold the old callback's fence ACK, then replace the document on the
    // canonical worker. Closing output custody must not strand the request.
    let new = h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::Resident,
        reservation: None,
    });
    assert!(new.project_replaced && new.stopped_playback_restore_complete);
    assert!(!new.state.can_undo);
    h.release_relinquished_output();
    h.wait_fresh_output(old_output);
    h.applied(0);
    h.service
        .submit(Request::InsertPluginEffect(request(
            &new.state,
            PluginEffectDestination::Main,
        )))
        .unwrap();
    assert!(h.next().error.is_none());
    let next = terminal(&mut h);
    assert!(next.error.is_none(), "{:?}", next.error);
    assert!(next.plugin_effect_complete[0].accepted);
    assert_eq!(next.state.revision, new.state.revision + 1);
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn native_effect_history_reuses_original_after_undo_with_and_without_stop() {
    let _native = native_fixture();
    for stop in [false, true] {
        native_effect_history_case(stop, false);
    }
    native_effect_history_case(true, true);
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn native_effect_history_after_observed_tail_terminal() {
    let _native = native_fixture();
    native_effect_history_case(false, true);
}

fn native_effect_history_case(stop_while_undone: bool, drain_tail: bool) {
    let mut h = ResidentHarness::with_plugins(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
    );
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let files = Files::new();
    let original = h.run(Request::Import(files.wav(48_000, 4096))).state;
    (applied, _) = h.applied(applied);
    let track = original.project.tracks()[0].id();
    h.service
        .submit(Request::InsertPluginEffect(request(
            &original,
            PluginEffectDestination::Main,
        )))
        .unwrap();
    assert!(h.next().error.is_none());
    let inserted = terminal(&mut h);
    assert!(inserted.error.is_none(), "{:?}", inserted.error);
    (applied, _) = h.applied(applied);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    h.render_plugin_block();
    wait(|| h.plugin_completed.load(Acquire) == 1);
    let undone = same_source_edit(&mut h, Request::Undo);
    (applied, _) = h.applied(applied);
    assert!(undone.state.project.main_devices().is_empty());
    assert_eq!(
        undone.state.project.track(track).unwrap().clips(),
        original.project.track(track).unwrap().clips()
    );
    assert_eq!(
        undone.state.project.assets().collect::<Vec<_>>(),
        original.project.assets().collect::<Vec<_>>()
    );
    assert!(undone.state.can_redo);
    if drain_tail {
        let end = Instant::now() + Duration::from_secs(10);
        let mut blocks = 0;
        while h
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .native_effect_source()
            .is_some()
        {
            let before = h.plugin_completed.load(Acquire);
            h.render_plugin_block();
            blocks += 1;
            if h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .native_effect_source()
                .is_some()
            {
                wait(|| h.plugin_completed.load(Acquire) > before);
            }
            assert!(
                Instant::now() < end,
                "actual native tail terminal timed out"
            );
        }
        assert!(
            blocks > 0,
            "control did not exercise the real outgoing tail"
        );
    }
    let processed_before_stop = h.plugin_completed.load(Acquire);
    if stop_while_undone {
        h.audio.as_ref().unwrap().control.stop();
        h.render_plugin_block();
        let mut restored = h.run(Request::RestoreStoppedPlayback);
        if !restored.stopped_playback_restore_complete {
            restored = restored_terminal(&mut h);
        }
        assert!(restored.error.is_none(), "{:?}", restored.error);
        assert!(restored.stopped_playback_restore_complete);
        (applied, _) = h.applied(applied);
    }
    let redone = same_source_edit(&mut h, Request::Redo);
    assert!(
        redone.error.is_none(),
        "stop_while_undone={stop_while_undone}: {:?}",
        redone.error
    );
    assert_eq!(
        redone.state.project.snapshot(),
        inserted.state.project.snapshot()
    );
    assert_eq!(redone.state.revision, inserted.state.revision + 2);
    assert!(!redone.state.can_redo);
    let end = Instant::now() + Duration::from_secs(10);
    while h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .applied_revision
        .load(Acquire)
        <= applied
    {
        h.pump();
        let before = h.plugin_completed.load(Acquire);
        h.render_plugin_block();
        let status = h.audio.as_ref().unwrap().status().unwrap();
        assert!(
            status
                .rejected_revision()
                .is_none_or(|(revision, _)| revision <= applied),
            "redo callback refusal: {:?}",
            status.rejected_revision()
        );
        if status.native_effect_source().is_some() && status.playing.load(Acquire) {
            wait(|| h.plugin_completed.load(Acquire) > before);
        }
        assert!(Instant::now() < end, "actual history handoff timed out");
    }
    h.applied(applied);
    assert_eq!(
        h.plugin_records.load(Acquire),
        1,
        "history loaded a substitute source"
    );
    if stop_while_undone {
        wait(|| h.plugin_completed.load(Acquire) == 0);
        wait(|| {
            h.audio
                .as_mut()
                .unwrap()
                .control
                .command(PlaybackCommand::Play)
                .is_ok()
        });
        for block in 0..3 {
            let output = h.render_plugin_block();
            wait(|| h.plugin_completed.load(Acquire) > block);
            let expected = if block < 2 {
                0.0
            } else {
                0.25 * (0.5 + processed_before_stop as f32 * 0.125)
            };
            assert!(
                output.iter().all(|frame| *frame == [expected; 2]),
                "history PCM: stop={stop_while_undone}, drained={drain_tail}, old DSP blocks={processed_before_stop}, block={block}, first={:?}, expected={expected}",
                output[0]
            );
        }
    }
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn native_effect_coordinator_keeps_registered_sampler_and_file_pcm_across_stop() {
    use hex_project::BeatTime;
    let _native = native_fixture();
    let mut h = ResidentHarness::with_plugins(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
    );
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let files = Files::new();
    let sampler_files = Files::new();
    h.run(Request::Import(files.wav(48_000, 4096)));
    (applied, _) = h.applied(applied);
    let added = h.run(Request::Edit(Command::AddMidiTrack {
        name: "sampler".into(),
    }));
    let Some(Created::Track(track)) = added.created else {
        panic!("sampler track")
    };
    (applied, _) = h.applied(applied);
    h.run(Request::LoadFirstSampler {
        request_id: 1,
        project_generation: 0,
        revision: added.state.revision,
        track,
        empty_sampler: None,
        path: sampler_files.wav(48_000, 4096),
        cancel: Arc::new(AtomicBool::new(false)),
    });
    (applied, _) = h.applied(applied);
    let placed = h.run(Request::Edit(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::ONE,
    }));
    let Some(Created::Clip(clip)) = placed.created else {
        panic!("sampler midi clip")
    };
    (applied, _) = h.applied(applied);
    let original = h
        .run(Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 127,
        }))
        .state;
    (applied, _) = h.applied(applied);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let dry: Vec<_> = (0..3).flat_map(|_| h.render_plugin_block()).collect();
    assert!(
        dry.iter().any(|frame| frame[0] > 0.25),
        "registered sampler did not contribute beside the original file clip"
    );
    h.audio.as_ref().unwrap().control.stop();
    h.render_plugin_block();
    assert!(
        h.run(Request::RestoreStoppedPlayback)
            .stopped_playback_restore_complete
    );
    (applied, _) = h.applied(applied);
    h.service
        .submit(Request::InsertPluginEffect(request(
            &original,
            PluginEffectDestination::Main,
        )))
        .unwrap();
    assert!(h.next().error.is_none());
    let accepted = terminal(&mut h);
    assert!(accepted.error.is_none(), "{:?}", accepted.error);
    assert!(accepted.plugin_effect_complete[0].accepted);
    assert_eq!(accepted.state.revision, original.revision + 1);
    assert_eq!(accepted.state.project.tracks(), original.project.tracks());
    assert_eq!(
        accepted.state.project.assets().collect::<Vec<_>>(),
        original.project.assets().collect::<Vec<_>>()
    );
    (applied, _) = h.applied(applied);
    for epoch in 0..2 {
        if epoch > 0 {
            h.audio.as_ref().unwrap().control.stop();
            h.render_plugin_block();
            let mut restored = h.run(Request::RestoreStoppedPlayback);
            if !restored.stopped_playback_restore_complete {
                restored = restored_terminal(&mut h);
            }
            assert!(restored.error.is_none(), "{:?}", restored.error);
            assert_eq!(restored.state.project, accepted.state.project);
            assert_eq!(restored.state.revision, accepted.state.revision);
            (applied, _) = h.applied(applied);
        }
        wait(|| h.plugin_completed.load(Acquire) == 0);
        wait(|| {
            h.audio
                .as_mut()
                .unwrap()
                .control
                .command(PlaybackCommand::Play)
                .is_ok()
        });
        let mut output = Vec::new();
        for block in 0..5 {
            output.extend(h.render_plugin_block());
            wait(|| h.plugin_completed.load(Acquire) > block);
        }
        assert!(output[..512].iter().all(|frame| *frame == [0.0; 2]));
        for (index, (actual, reference)) in output[512..].iter().zip(&dry).enumerate() {
            let scale = 0.5 + (epoch * 5 + index / 256) as f32 * 0.125;
            assert_eq!(
                *actual,
                reference.map(|sample| sample * scale),
                "original mixed PCM: epoch={epoch}, frame={index}"
            );
        }
    }
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn native_effect_coordinator_live_bypass_and_removal_keep_original_clock_and_tail_pcm() {
    let _native = native_fixture();
    let mut h = ResidentHarness::with_plugins(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
    );
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let files = Files::new();
    let original = h.run(Request::Import(files.wav(48_000, 4096))).state;
    (applied, _) = h.applied(applied);
    h.service
        .submit(Request::InsertPluginEffect(request(
            &original,
            PluginEffectDestination::Main,
        )))
        .unwrap();
    assert!(h.next().error.is_none());
    let inserted = terminal(&mut h);
    assert!(inserted.error.is_none(), "{:?}", inserted.error);
    let device = inserted.state.project.main_devices()[0].id();
    (applied, _) = h.applied(applied);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    h.render_plugin_block();
    wait(|| h.plugin_completed.load(Acquire) == 1);
    for bypassed in [true, false] {
        let changed = same_source_edit(
            &mut h,
            Request::Edit(Command::SetDeviceBypass { device, bypassed }),
        );
        assert_eq!(
            changed.state.project.device(device).unwrap().bypassed(),
            bypassed
        );
        assert_eq!(changed.state.project.tracks(), original.project.tracks());
        (applied, _) = h.applied(applied);
        let start = h
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .performance_frame
            .load(Acquire);
        for block in 0..3 {
            let before = h.plugin_completed.load(Acquire);
            let output = h.render_plugin_block();
            wait(|| h.plugin_completed.load(Acquire) > before);
            for (offset, frame) in output.into_iter().enumerate() {
                let absolute = start + block * 256 + offset as u64;
                let expected = if absolute < 512 {
                    0.0
                } else if bypassed {
                    0.25
                } else {
                    0.25 * (0.5 + ((absolute - 512) / 256) as f32 * 0.125)
                };
                assert_eq!(frame, [expected; 2], "bypass={bypassed}, frame={absolute}");
            }
        }
        assert_eq!(h.plugin_records.load(Acquire), 1);
    }
    let cut = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .performance_frame
        .load(Acquire);
    let removed = same_source_edit(&mut h, Request::Edit(Command::RemoveDevice(device)));
    assert!(removed.state.project.main_devices().is_empty());
    assert_eq!(removed.state.project.tracks(), original.project.tracks());
    assert_eq!(
        removed.state.project.assets().collect::<Vec<_>>(),
        original.project.assets().collect::<Vec<_>>()
    );
    h.applied(applied);
    let start = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .performance_frame
        .load(Acquire);
    for block in 0..4 {
        let before = h.plugin_completed.load(Acquire);
        let output = h.render_plugin_block();
        if h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .native_effect_source()
            .is_some()
        {
            wait(|| h.plugin_completed.load(Acquire) > before);
        }
        for (offset, frame) in output.into_iter().enumerate() {
            let absolute = start + block * 256 + offset as u64;
            let tail = if absolute >= 512 && absolute < cut + 512 {
                0.25 * (0.5 + ((absolute - 512) / 256) as f32 * 0.125)
            } else {
                0.0
            };
            assert_eq!(
                frame,
                [0.25 + tail; 2],
                "removed Main original tail: cut={cut}, frame={absolute}"
            );
        }
    }
    assert!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .native_effect_source()
            .is_none()
    );
    assert_eq!(
        h.plugin_records.load(Acquire),
        1,
        "original source must remain available for actual Undo"
    );
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn native_effect_coordinator_reorder_waits_for_original_zero_input_prefix_and_undo() {
    let _native = native_fixture();
    let mut h = ResidentHarness::with_plugins(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
    );
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let files = Files::new();
    let original = h.run(Request::Import(files.wav(48_000, 4096))).state;
    (applied, _) = h.applied(applied);
    h.service
        .submit(Request::InsertPluginEffect(request(
            &original,
            PluginEffectDestination::Main,
        )))
        .unwrap();
    assert!(h.next().error.is_none());
    let inserted = terminal(&mut h);
    assert!(inserted.error.is_none(), "{:?}", inserted.error);
    let native = inserted.state.project.main_devices()[0].id();
    (applied, _) = h.applied(applied);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    h.render_plugin_block();
    wait(|| h.plugin_completed.load(Acquire) == 1);
    let class = hex_engine::NativeDeviceClass::Effect;
    let utility_type = hex_engine::native_device_catalog_for_class(class)
        .unwrap()
        .into_iter()
        .find(|entry| entry.type_id.as_str() == Ok("hex.utility"))
        .unwrap()
        .type_id;
    let added = same_source_edit(
        &mut h,
        Request::Edit(Command::InsertMainDevice {
            before: None,
            state: hex_engine::default_native_device_state_for_class(class, utility_type).unwrap(),
        }),
    );
    let Some(Created::Device(utility)) = added.created else {
        panic!("utility")
    };
    (applied, _) = h.applied(applied);
    let moved = same_source_edit(
        &mut h,
        Request::Edit(Command::MoveMainDevice {
            device: utility,
            before: Some(native),
        }),
    );
    assert_eq!(
        moved
            .state
            .project
            .main_devices()
            .iter()
            .map(|device| device.id())
            .collect::<Vec<_>>(),
        [utility, native]
    );
    assert_eq!(moved.state.project.tracks(), original.project.tracks());
    let before = h.plugin_completed.load(Acquire);
    h.render_plugin_block();
    wait(|| h.plugin_completed.load(Acquire) > before);
    assert_eq!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(Acquire),
        applied,
        "one endpoint cannot be current and an outgoing tail before its real zero-input prefix"
    );
    let end = Instant::now() + Duration::from_secs(10);
    while h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .applied_revision
        .load(Acquire)
        <= applied
    {
        let before = h.plugin_completed.load(Acquire);
        h.render_plugin_block();
        wait(|| h.plugin_completed.load(Acquire) > before);
        assert!(
            h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .rejected_revision()
                .is_none(),
            "actual reorder was refused"
        );
        assert!(
            Instant::now() < end,
            "actual native reorder did not reach its zero-input boundary"
        );
    }
    (applied, _) = h.applied(applied);
    assert!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .performance_frame
            .load(Acquire)
            >= 4096
    );
    assert_eq!(h.plugin_records.load(Acquire), 1);
    let undone = same_source_edit(&mut h, Request::Undo);
    assert_eq!(
        undone
            .state
            .project
            .main_devices()
            .iter()
            .map(|device| device.id())
            .collect::<Vec<_>>(),
        [native, utility]
    );
    assert_eq!(undone.state.revision, moved.state.revision + 1);
    h.applied(applied);
    let before = h.plugin_completed.load(Acquire);
    assert_eq!(h.render_plugin_block(), [[0.0; 2]; 256]);
    wait(|| h.plugin_completed.load(Acquire) > before);
    assert_eq!(
        h.plugin_records.load(Acquire),
        1,
        "reorder/Undo replaced the original native source"
    );
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn parameter_inventory_reaches_session_from_the_hosted_controller() {
    const CHILD: &str = "HEX_DESKTOP_TEST_PARAMETER_INVENTORY_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let name = std::thread::current().name().unwrap().to_owned();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--ignored", "--nocapture"])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "37")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }
    let _native = native_fixture();
    let mut h = ResidentHarness::with_plugins(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
    );
    let initial = h.next();
    h.applied(0);
    h.run(Request::InsertPluginEffect(request(
        &initial.state,
        PluginEffectDestination::Main,
    )));
    let accepted = terminal(&mut h);
    assert!(accepted.error.is_none(), "{:?}", accepted.error);
    let (_, metadata) = h.applied(1);
    let device = accepted.state.project.main_devices()[0].id();
    let query = hex_session::service::PluginParameterQuery {
        context: metadata.context,
        device,
        output_identity: h.audio.as_ref().unwrap().control.output_identity().unwrap(),
        request_id: 1,
        page_first: None,
    };
    let admitted = h.run(Request::QueryPluginParameters(query));
    assert_eq!(admitted.state.revision, accepted.state.revision);
    let received = h
        .next()
        .plugin_parameters
        .expect("native controller inventory");
    assert_eq!(received.query, query);
    let source = received.source.expect("exact loaded controller source");
    let signature = received
        .descriptor_signature
        .expect("complete descriptor signature");
    assert_eq!(received.total, 1);
    let rows = received.rows.expect("original controller reply");
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].id, rows[0].name.as_str()), (7, "gain"));
    assert!(!rows[0].value.is_empty());
    let selected = hex_session::service::PluginParameterQuery {
        page_first: Some(0),
        request_id: 2,
        ..query
    };
    h.run(Request::QueryPluginParameters(selected));
    let page = h.next().plugin_parameters.expect("single live page");
    assert_eq!(page.query, selected);
    assert_eq!(page.source, Some(source));
    assert_eq!(page.descriptor_signature, Some(signature));
    assert_eq!(page.total, 1);
    assert_eq!(page.rows.unwrap(), rows);
    let stale = hex_session::service::PluginParameterQuery {
        context: initial
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .context(),
        request_id: 3,
        ..query
    };
    let refused = h
        .run(Request::QueryPluginParameters(stale))
        .plugin_parameters
        .unwrap();
    assert_eq!(refused.query, stale);
    assert!(refused.rows.is_err());
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn native_effect_real_work_contention_waits_then_keeps_exact_accept_or_refusal() {
    let _native = native_fixture();
    for known_presentation in [false, true] {
        let (reached, facts) = mpsc::sync_channel(1);
        let (resume, resumed) = mpsc::sync_channel(1);
        let mut h = ResidentHarness::with_plugins_paused_at_facts(
            host(),
            known_presentation.then_some(EffectPresentation {
                input_frames: 0,
                output_frames: 512,
            }),
            crate::native_audio::plugins::FactsGate::new(reached, resumed),
        );
        let initial = h.next();
        assert!(initial.error.is_none());
        h.applied(0);
        let input = request(&initial.state, PluginEffectDestination::Main);
        h.service
            .submit(Request::InsertPluginEffect(input.clone()))
            .unwrap();
        facts.recv_timeout(Duration::from_secs(10)).unwrap();
        let (held, observed_hold) = mpsc::sync_channel(1);
        let (release, released) = mpsc::sync_channel(1);
        let holder = h.hold_native_work_on_control(held, released);
        observed_hold.recv_timeout(Duration::from_secs(10)).unwrap();
        let before_attempt = h.plugin_progress.lock().unwrap().attempts;
        resume.send(()).unwrap();
        wait(|| h.plugin_progress.lock().unwrap().attempts > before_attempt);
        let pending = h.next();
        release.send(()).unwrap();
        holder.join().unwrap();
        assert!(
            pending.error.is_none(),
            "actual Work contention became a terminal: {:?}",
            pending.error
        );
        assert!(pending.request_complete && pending.plugin_effect_complete.is_empty());
        assert_eq!(pending.state.project, initial.state.project);
        assert_eq!(pending.state.revision, initial.state.revision);
        let finished = terminal(&mut h);
        assert!(Arc::ptr_eq(
            &finished.plugin_effect_complete[0].control,
            &input.control
        ));
        assert_eq!(
            finished.plugin_effect_complete[0].accepted,
            known_presentation
        );
        if known_presentation {
            assert!(finished.error.is_none(), "{:?}", finished.error);
            assert_eq!(finished.state.revision, initial.state.revision + 1);
            assert_eq!(finished.state.project.main_devices().len(), 1);
        } else {
            assert!(
                finished.error.as_ref().unwrap().contains("presentation"),
                "{:?}",
                finished.error
            );
            assert_eq!(finished.state.project, initial.state.project);
            assert_eq!(finished.state.revision, initial.state.revision);
            assert!(!finished.state.can_undo);
        }
    }
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn native_effect_coordinator_refuses_unknown_presentation_without_canonical_or_history_edit() {
    let _native = native_fixture();
    let mut h = ResidentHarness::with_plugins(host(), None);
    let initial = h.next();
    assert!(initial.error.is_none());
    h.applied(0);
    let input = request(&initial.state, PluginEffectDestination::Main);
    h.service
        .submit(Request::InsertPluginEffect(input.clone()))
        .unwrap();
    let mut refused = h.next();
    if refused.plugin_effect_complete.is_empty() {
        refused = terminal(&mut h);
    }
    assert!(
        refused.error.as_ref().unwrap().contains("presentation"),
        "unexpected native refusal: {:?}",
        refused.error
    );
    assert_eq!(refused.state.project, initial.state.project);
    assert!(!refused.state.can_undo);
    assert!(!refused.plugin_effect_complete[0].accepted);
    assert!(Arc::ptr_eq(
        &refused.plugin_effect_complete[0].control,
        &input.control
    ));
    let end = Instant::now() + Duration::from_secs(10);
    while h.plugin_records.load(Acquire) != 0 {
        h.render();
        assert!(
            Instant::now() < end,
            "original refused source did not reach actual report recovery"
        );
        std::thread::yield_now();
    }
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn native_effect_coordinator_cancel_or_stop_before_ack_preserves_history_and_recovers_original() {
    let _native = native_fixture();
    for stop in [false, true] {
        let mut h = ResidentHarness::with_plugins(
            host(),
            Some(EffectPresentation {
                input_frames: 0,
                output_frames: 512,
            }),
        );
        let initial = h.next();
        assert!(initial.error.is_none(), "{:?}", initial.error);
        h.applied(0);
        let first = request(&initial.state, PluginEffectDestination::Main);
        h.service
            .submit(Request::InsertPluginEffect(first.clone()))
            .unwrap();
        let pending = h.next();
        assert!(pending.error.is_none(), "{:?}", pending.error);
        assert!(pending.plugin_effect_complete.is_empty());
        wait(|| h.plugin_queued.load(Acquire));
        if stop {
            h.audio.as_ref().unwrap().control.stop();
        } else {
            assert!(first.control.cancel());
        }
        let refused = terminal(&mut h);
        assert!(!refused.request_complete);
        assert!(refused.error.is_some());
        assert_eq!(refused.state.project, initial.state.project);
        assert_eq!(refused.state.revision, initial.state.revision);
        assert!(!refused.state.can_undo);
        assert_eq!(refused.plugin_effect_complete.len(), 1);
        assert!(!refused.plugin_effect_complete[0].accepted);
        assert!(Arc::ptr_eq(
            &refused.plugin_effect_complete[0].control,
            &first.control
        ));
        let end = Instant::now() + Duration::from_secs(10);
        while h.plugin_records.load(Acquire) != 0 {
            h.render();
            assert!(
                Instant::now() < end,
                "original source/endpoint did not reach report recovery"
            );
            std::thread::yield_now();
        }
        let next = request(&refused.state, PluginEffectDestination::Main);
        h.service
            .submit(Request::InsertPluginEffect(next.clone()))
            .unwrap();
        let pending = h.next();
        assert!(pending.error.is_none(), "{:?}", pending.error);
        let accepted = if pending.plugin_effect_complete.is_empty() {
            terminal(&mut h)
        } else {
            pending
        };
        assert!(accepted.error.is_none(), "{:?}", accepted.error);
        assert_eq!(accepted.state.revision, initial.state.revision + 1);
        assert!(
            matches!(accepted.created, Some(Created::Device(id)) if id.value() == initial.state.project.next_identity())
        );
        assert!(accepted.plugin_effect_complete[0].accepted);
        assert!(Arc::ptr_eq(
            &accepted.plugin_effect_complete[0].control,
            &next.control
        ));
    }
}

#[test]
#[ignore = "requires signed SDK receipts and an explicit installed effect catalog entry"]
fn installed_vst3_effect_inserts_and_renders_in_desktop_audio_graph() {
    let _native = native_fixture();
    let bundle = std::env::var("HEX_TEST_VST3_EFFECT_BUNDLE").unwrap();
    let catalog = PathBuf::from(std::env::var_os("HEX_TEST_VST3_EFFECT_CATALOG").unwrap());
    let files = Files::new();
    let rate = 44_100;
    let mut h = ResidentHarness::with_plugins_at_rate(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
        rate,
    );
    assert!(h.next().error.is_none());
    let (applied, _) = h.applied(0);
    let imported = h.run(Request::Import(files.wav(rate, 8192)));
    let (applied, _) = h.applied(applied);
    let mut insertion = request(&imported.state, PluginEffectDestination::Main);
    insertion.selection =
        installed_selection(&catalog, &bundle, hex_plugin_catalog::PluginRole::Effect);
    h.service
        .submit(Request::InsertPluginEffect(insertion))
        .unwrap();
    let accepted = terminal(&mut h);
    assert!(accepted.error.is_none(), "{:?}", accepted.error);
    assert!(accepted.plugin_effect_complete[0].accepted);
    let device = &accepted.state.project.main_devices()[0];
    let state = hex_plugin_state::decode(device.state()).unwrap();
    assert!(state.component.is_some());
    h.applied(applied);
    assert_eq!(
        h.submitted_observation()
            .unwrap()
            .foreign_processor(device.id().value(), device.state().type_id()),
        Some(true)
    );
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let mut peak = 0.0_f32;
    for block in 0..24 {
        for sample in h.render_plugin_block().iter().flatten() {
            peak = peak.max(sample.abs());
        }
        wait(|| h.plugin_completed.load(Acquire) > block);
    }
    assert!(peak > 0.001, "{bundle} returned silent PCM");
    println!("{bundle} arrangement effect at {rate} hz peak={peak}");
}

#[test]
#[ignore = "explicit original SDK helper receipt required"]
fn native_effect_metadata_hold_survives_profile_refusal_without_opening_a_device() {
    let (sender, _events) = mpsc::sync_channel(4);
    let mut sink = NativeAudioSink::new(sender);
    sink.plugins = crate::native_audio::plugins::Plugins::with_fixture(host(), None);
    assert!(!sink.plugins.pool_held);
    let held = sink.native_sampler_hold();
    assert_eq!(
        held.retained_bytes,
        crate::audio_policy::PLUGIN_CONTROL_DATA_BYTES
    );
    assert_eq!(
        held.construction_bytes,
        crate::audio_policy::PLUGIN_CONTROL_DATA_BYTES
    );
    sink.plugins
        .refuse_profile("fixture backend profile unavailable".into());
    sink.plugins.fresh_output();
    assert!(!sink.plugins.pool_held);
    let retained = sink.native_sampler_hold();
    assert_eq!(retained.retained_bytes, held.retained_bytes);
    assert_eq!(retained.construction_bytes, held.construction_bytes);
}

#[test]
#[ignore = "run with source-bound SDK receipts and variant 105 plus its release path"]
fn native_latency_restart_rebuilds_compensation_on_the_original_source() {
    let _native = native_fixture();
    assert_eq!(
        std::env::var("HEX_ORIGINAL_INSTANCE_VARIANT").as_deref(),
        Ok("105")
    );
    let release = PathBuf::from(std::env::var_os("HEX_ORIGINAL_INSTANCE_RELEASE").unwrap());
    assert!(!release.exists());
    let mut h = ResidentHarness::with_plugins(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
    );
    assert!(h.next().error.is_none());
    let (applied, _) = h.applied(0);
    let files = Files::new();
    let path = files.wav(48_000, 4096);
    let mut wav = fs::read(&path).unwrap();
    wav[44..].fill(0);
    wav[44 + 256 * 2..44 + 257 * 2].copy_from_slice(&8192i16.to_le_bytes());
    fs::write(&path, wav).unwrap();
    h.run(Request::Import(path.clone()));
    let (applied, _) = h.applied(applied);
    let state = h.run(Request::Import(path)).state;
    let (applied, _) = h.applied(applied);
    assert_eq!(state.project.tracks().len(), 2);
    let track = state.project.tracks()[0].id();
    h.service
        .submit(Request::InsertPluginEffect(request(
            &state,
            PluginEffectDestination::Track(track),
        )))
        .unwrap();
    assert!(h.next().error.is_none());
    let accepted = terminal(&mut h);
    assert!(accepted.error.is_none(), "{:?}", accepted.error);
    assert!(accepted.plugin_effect_complete[0].accepted);
    let (applied, _) = h.applied(applied);
    let audio = h.audio.as_mut().unwrap();
    let before = audio.status().unwrap().native_effect_source().unwrap();
    audio.control.command(PlaybackCommand::Play).unwrap();
    h.render_plugin_block();
    wait(|| h.plugin_completed.load(Acquire) == 1);
    wait(|| h.plugin_progress.lock().unwrap().observed_latency_receipt);
    let binding = h.plugin_progress.lock().unwrap().binding.unwrap();

    fs::write(release, b"request latency restart").unwrap();
    wait(|| {
        h.pump();
        h.latency_restart == Some(before)
    });
    let audio = h.audio.as_mut().unwrap();
    assert!(audio.status().unwrap().playing.load(Acquire));
    audio.control.command(PlaybackCommand::Stop).unwrap();
    h.render_plugin_block();
    assert!(
        !h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .playing
            .load(Acquire)
    );
    h.service.submit(Request::RestoreStoppedPlayback).unwrap();
    let restored = restored_terminal(&mut h);
    assert!(restored.error.is_none(), "{:?}", restored.error);
    assert_eq!(restored.state.revision, accepted.state.revision);
    assert_eq!(restored.state.project, accepted.state.project);
    let (fresh, metadata) = h.applied(applied);
    assert!(metadata.stopped_restore);
    let audio = h.audio.as_mut().unwrap();
    let after = audio.status().unwrap().native_effect_source().unwrap();
    assert_eq!(after.audio_revision, fresh);
    assert_eq!(after.context, before.context);
    assert_eq!(after.output_identity, before.output_identity);
    wait(|| {
        h.plugin_progress
            .lock()
            .unwrap()
            .binding
            .is_some_and(|updated| {
                updated.instance == binding.instance && updated.epoch > binding.epoch
            })
            && h.plugin_completed.load(Acquire) == 0
    });
    // Actual callback installation can precede the worker's fence release.
    wait(|| {
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::PlayFrom(0))
            .is_ok()
    });
    let mut pcm = Vec::new();
    for block in 0..4 {
        pcm.extend(h.render_plugin_block());
        wait(|| h.plugin_completed.load(Acquire) > block);
    }
    let nonzero: Vec<_> = pcm
        .iter()
        .enumerate()
        .filter(|(_, frame)| **frame != [0.0; 2])
        .collect();
    // Both tracks have the same input impulse. The fixture's additional
    // seven frames must shift both paths, and neither path may disappear.
    assert_eq!(nonzero.len(), 1, "{nonzero:?}");
    assert_eq!(nonzero[0].0, 256 + 512 + 7);
    assert!(nonzero[0].1[0] > 0.25);
}
