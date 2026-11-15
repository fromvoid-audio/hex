//! Explicit native history intent through real preparation, publication and callback.
use super::resident_preview::ResidentHarness;
use super::*;
use hex_engine::native_device_resources::{PreparedDeviceResources, ResidentDeviceAssets};
use hex_project::{BeatTime, Command, Created};
use hex_session::device_gesture::{ParameterId, ParameterValue};
use hex_session::service::ResidentSeekHistory;
use std::collections::BTreeMap;

#[derive(Clone, Copy)]
enum HeldLocatorSeek {
    Accepted,
    StaleRevision,
    WrongOutput,
    Cancelled,
    ConsumedStop,
}

#[test]
fn held_locator_checkpoint_allows_actual_resident_seek_publication() {
    held_locator_seek(HeldLocatorSeek::Accepted);
}

#[test]
fn held_locator_checkpoint_refuses_stale_revision() {
    held_locator_seek(HeldLocatorSeek::StaleRevision);
}

#[test]
fn held_locator_checkpoint_refuses_wrong_output() {
    held_locator_seek(HeldLocatorSeek::WrongOutput);
}

#[test]
fn held_locator_checkpoint_refuses_cancelled_seek() {
    held_locator_seek(HeldLocatorSeek::Cancelled);
}

#[test]
fn held_locator_checkpoint_refuses_seek_after_consumed_stop() {
    held_locator_seek(HeldLocatorSeek::ConsumedStop);
}

fn held_locator_seek(case: HeldLocatorSeek) {
    use std::time::{Duration, Instant};
    let _native = native_fixture();
    let files = Files::new();
    let journal = files.0.join("held-locator.hex-journal");
    let mut h = ResidentHarness::with_recovery(8192, journal.clone());
    assert!(h.next().error.is_none());
    let mut applied = h.applied(0).0;
    let imported = h.run(Request::ImportToNewTrack {
        path: files.wav(48_000, 8192),
        before_track: None,
        start: 0,
    });
    applied = h.applied(applied).0;
    let track = imported.state.project.tracks()[0].id();
    let reverb = hex_engine::default_native_device_state(hex_engine::DeviceTypeId::from_static(
        "hex.reverb",
    ))
    .unwrap();
    h.run(Request::Edit(Command::InsertDevice {
        track,
        before: None,
        state: reverb,
    }));
    applied = h.applied(applied).0;
    let mut gate = hex_session::service::hold_next_checkpoint(journal.clone()).unwrap();
    h.service
        .submit(Request::Edit(Command::AddArrangementLocator { frame: 129 }))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match gate.try_entered() {
            Ok(()) => break,
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(error) => panic!("checkpoint gate: {error}"),
        }
        assert!(Instant::now() < deadline, "checkpoint did not enter");
        std::thread::yield_now();
    }
    let view = loop {
        if let Ok(view) = h.service.try_arrangement_edit_view() {
            break view;
        }
        assert!(Instant::now() < deadline, "committed locator view missing");
        std::thread::yield_now();
    };
    let state = view.state.upgrade().unwrap();
    let locator = state.project.arrangement_locators().first().unwrap();
    assert_eq!(locator.frame(), 129);
    let (locator_audio, metadata) = h.applied(applied);
    assert_eq!(metadata.context.editor_revision, state.revision);
    assert!(
        metadata.requires_preroll,
        "fixture must require a stateful seek"
    );
    assert!(
        h.service.try_event().is_err(),
        "checkpoint terminal is still held"
    );
    let mut intent = h.seek_intent(1, state.revision, None, None);
    let expected_output = intent.fence.output_identity;
    intent.destination_frame = locator.frame();
    match case {
        HeldLocatorSeek::Accepted => {}
        HeldLocatorSeek::StaleRevision => intent.project_revision += 1,
        HeldLocatorSeek::WrongOutput => intent.fence.output_identity += 1,
        HeldLocatorSeek::ConsumedStop => {
            h.audio.as_ref().unwrap().control.stop();
            h.render();
        }
        HeldLocatorSeek::Cancelled => intent
            .cancelled
            .store(true, std::sync::atomic::Ordering::Release),
    }
    let requested_revision = intent.project_revision;
    h.service.submit_resident_arrangement_seek(intent).unwrap();
    let result = h.seek_result();
    assert_eq!(result.request_id, 1);
    assert_eq!(result.project_generation, view.project_generation);
    assert_eq!(result.project_revision, requested_revision);
    assert!(
        h.service.try_event().is_err(),
        "seek terminal must remain ordered"
    );
    match case {
        HeldLocatorSeek::Accepted => {
            let hex_session::service::ResidentPublicationAcceptance::Submitted {
                audio_revision,
                output_identity,
            } = result.result.unwrap()
            else {
                panic!("seek must submit to the real resident callback");
            };
            assert_eq!(output_identity, expected_output);
            let (installed, metadata) = h.applied(locator_audio);
            assert_eq!(installed, audio_revision);
            assert_eq!(metadata.context.project_generation, view.project_generation);
            assert_eq!(metadata.context.editor_revision, state.revision);
            assert_eq!(metadata.seek_request, Some(1));
            let status = h.audio.as_ref().unwrap().status().unwrap();
            assert_eq!(
                h.audio.as_ref().unwrap().control.output_identity(),
                Some(expected_output)
            );
            assert_eq!(status.applied_revision.load(Acquire), audio_revision);
            assert!(status.playing.load(Acquire));
            assert!(status.position.load(Acquire) >= locator.frame());
        }
        _ => {
            let error = result.result.unwrap_err();
            let expected = match case {
                HeldLocatorSeek::StaleRevision | HeldLocatorSeek::Cancelled => {
                    "seek target changed or was cancelled"
                }
                HeldLocatorSeek::WrongOutput | HeldLocatorSeek::ConsumedStop => "runtime changed",
                HeldLocatorSeek::Accepted => unreachable!("accepted case handled above"),
            };
            assert!(error.contains(expected), "unexpected refusal: {error}");
            h.render();
            let status = h.audio.as_ref().unwrap().status().unwrap();
            assert_eq!(status.applied_revision.load(Acquire), locator_audio);
            assert!(!status.playing.load(Acquire));
        }
    }
    assert!(
        h.service.try_event().is_err(),
        "checkpoint terminal remains held after seek"
    );
    gate.release().unwrap();
    let locator_terminal = h.next();
    assert!(
        matches!(locator_terminal.created, Some(Created::ArrangementLocator(id)) if id == locator.id())
    );
    assert_eq!(locator_terminal.state.revision, state.revision);
    let seek_terminal = h.next();
    assert_eq!(
        seek_terminal.error.is_some(),
        matches!(
            case,
            HeldLocatorSeek::StaleRevision | HeldLocatorSeek::Cancelled
        )
    );
    assert_eq!(hex_storage::load_journal(&journal).unwrap(), *state.project);
}

#[test]
fn explicit_native_history_matches_linear_pcm_and_keeps_refused_old_owner() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(history_case)
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn streamed_pending_fresh_seek_preserves_undo_and_redo() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| streamed_pending_history_case(false))
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn streamed_pending_section_preserves_undo_and_redo() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| streamed_pending_history_case(true))
        .unwrap()
        .join()
        .unwrap();
}

fn streamed_pending_history_case(section: bool) {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(4);
    assert!(h.next().error.is_none());
    let mut applied = h.applied(0).0;
    let imported = h.run(Request::ImportToNewTrack {
        path: files.wav(48_000, 32_768),
        before_track: None,
        start: 0,
    });
    applied = h.applied(applied).0;
    assert!(
        imported
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .sources()
            .values()
            .any(|source| matches!(source, hex_session::AudioSourceBinding::VerifiedFile(_)))
    );
    let track = imported.state.project.tracks()[0].id();
    let before = if section {
        let configured = h.run(Request::Edit(Command::SetLoopRegion {
            start: 129,
            end: 513,
            enabled: true,
        }));
        applied = h.applied(applied).0;
        configured.state.project.snapshot()
    } else {
        imported.state.project.snapshot()
    };
    let renamed = h.run(Request::Edit(Command::RenameTrack {
        track,
        name: "history target".into(),
    }));
    applied = h.applied(applied).0;
    let after = renamed.state.project.snapshot();
    assert_ne!(before, after);
    let mut revision = renamed.state.revision;
    let mut history = (renamed.state.can_undo, renamed.state.can_redo);

    for (request_id, action, expected, retry_expected) in [
        (1, Request::Undo, &after, &before),
        (2, Request::Redo, &before, &after),
    ] {
        let mut intent = h.seek_intent(request_id, revision, None, None);
        intent.destination_frame = 129;
        if section {
            intent.section = Some(hex_session::arrangement_seek::SectionLaunch {
                region: hex_project::LoopRegion::new(129, 513, true).unwrap(),
                quantization: hex_engine::LaunchQuantization::Immediate,
            });
        }
        h.service.submit_resident_arrangement_seek(intent).unwrap();
        let submitted = h.next();
        assert!(submitted.error.is_none(), "{:?}", submitted.error);
        let accepted = h.seek_result().result.unwrap();
        let hex_session::service::ResidentPublicationAcceptance::Submitted {
            audio_revision, ..
        } = accepted
        else {
            panic!("seek must be submitted before callback installation")
        };
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

        h.service.submit(action).unwrap();
        let refused = h.next();
        assert!(
            refused
                .error
                .as_deref()
                .is_some_and(|error| error.contains("publication already pending")),
            "{:?}",
            refused.error
        );
        assert_eq!(refused.state.revision, revision);
        assert_eq!(&refused.state.project.snapshot(), expected);
        assert_eq!((refused.state.can_undo, refused.state.can_redo), history);
        if request_id == 1 {
            assert!(refused.state.can_undo);
        } else {
            assert!(refused.state.can_redo);
        }

        let (installed, metadata) = h.applied(applied);
        assert_eq!(installed, audio_revision);
        assert_eq!(metadata.seek_request, Some(request_id));
        if section {
            let status = h.audio.as_ref().unwrap().status().unwrap();
            assert!(status.playing.load(Acquire));
            assert!((129..513).contains(&status.position.load(Acquire)));
            assert_eq!(h.render(), [[0.25; 2]; 64]);
        }
        applied = installed;
        let retried = h.run(if request_id == 1 {
            Request::Undo
        } else {
            Request::Redo
        });
        assert_eq!(&retried.state.project.snapshot(), retry_expected);
        revision = retried.state.revision;
        history = (retried.state.can_undo, retried.state.can_redo);
        applied = h.applied(applied).0;
    }
}

fn edit(h: &mut ResidentHarness, applied: &mut u64, command: Command) -> Event {
    let event = h.run(Request::Edit(command));
    assert!(event.error.is_none(), "{:?}", event.error);
    *applied = h.applied(*applied).0;
    event
}

fn assert_pcm(
    h: &ResidentHarness,
    graph: &Arc<hex_engine::PreparedRevision>,
    resources: &PreparedDeviceResources,
) {
    let frame = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .performance_frame
        .load(Acquire);
    let mut oracle =
        hex_engine::SessionPlayback::new_with_device_resources(Arc::clone(graph), resources)
            .unwrap();
    oracle
        .apply(hex_engine::SessionPlaybackCommand::Play)
        .unwrap();
    for chunk in vec![[0.0; 2]; frame as usize].chunks_mut(127) {
        oracle.render(chunk).unwrap();
    }
    let mut expected = [[0.0; 2]; 64];
    oracle.render(&mut expected).unwrap();
    assert_eq!(
        h.render(),
        expected,
        "native history continuation at {frame}"
    );
}

fn history_case() {
    let _native = native_fixture();
    let files = Files::new();
    let frames = 32_768;
    let mut h = ResidentHarness::with_frame_limit(4);
    assert!(h.next().error.is_none());
    let mut applied = h.applied(0).0;
    let imported = h.run(Request::ImportToNewTrack {
        path: files.wav(48_000, frames),
        before_track: None,
        start: 0,
    });
    assert!(imported.error.is_none(), "{:?}", imported.error);
    applied = h.applied(applied).0;
    assert_eq!(imported.state.project.tracks().len(), 1);
    assert_eq!(imported.state.project.assets().count(), 1);
    let track = imported.state.project.tracks()[0].id();
    let asset = imported.state.project.assets().next().unwrap().id();
    assert!(
        imported
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .sources()
            .values()
            .any(|source| matches!(source, hex_session::AudioSourceBinding::VerifiedFile(_)))
    );
    let filter = hex_engine::default_native_device_state(hex_engine::DeviceTypeId::from_static(
        "hex.utility",
    ))
    .unwrap();
    let filter = hex_engine::patch_native_device_parameter(
        &filter,
        ParameterId::new(11).unwrap(),
        ParameterValue::Bool(true),
    )
    .unwrap();
    edit(
        &mut h,
        &mut applied,
        Command::InsertDevice {
            track,
            before: None,
            state: filter.clone(),
        },
    );
    let returned = edit(
        &mut h,
        &mut applied,
        Command::AddReturnTrack {
            name: "history tail".into(),
        },
    );
    let Some(Created::ReturnTrack(return_track)) = returned.created else {
        panic!("return")
    };
    edit(
        &mut h,
        &mut applied,
        Command::InsertReturnDevice {
            return_track,
            before: None,
            state: filter,
        },
    );
    edit(
        &mut h,
        &mut applied,
        Command::SetReturnSend {
            track,
            return_track,
            gain: 0.5,
        },
    );
    let state = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        hex_engine::DeviceTypeId::from_static("hex.instrument.operator"),
    )
    .unwrap();
    let state = hex_engine::patch_native_device_parameter_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        &state,
        ParameterId::new(78).unwrap(),
        ParameterValue::Float(0.3),
    )
    .unwrap();
    let instrument = edit(
        &mut h,
        &mut applied,
        Command::InsertTrackWithDevice {
            name: "feedback".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state,
        },
    );
    let Some(Created::Track(midi)) = instrument.created else {
        panic!("instrument")
    };
    let placed = edit(
        &mut h,
        &mut applied,
        Command::PlaceMidiClip {
            track: midi,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        },
    );
    let Some(Created::Clip(clip)) = placed.created else {
        panic!("clip")
    };
    let authored = edit(
        &mut h,
        &mut applied,
        Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        },
    );
    let cancelled = AtomicBool::new(false);
    let resources = PreparedDeviceResources::prepare_on_control(
        &ResidentDeviceAssets::default(),
        &authored.state.project,
        1 << 20,
        &cancelled,
    )
    .unwrap();
    let media = BTreeMap::from([(
        asset,
        hex_engine::Audio::new(48_000, vec![[0.25; 2]; frames as usize]).unwrap(),
    )]);
    let graph = Arc::new(
        hex_engine::PreparedRevision::prepare_with_device_resources(
            &authored.state.project,
            &media,
            Arc::new(hex_engine::SineTable::new()),
            None,
            &hex_engine::audio_transform::AudioTransformPreparation::default(),
            &resources,
            0,
            &cancelled,
        )
        .unwrap(),
    );
    for (id, destination, playing) in [(1, 512, true), (3, 24_004, false)] {
        let mut intent = h.seek_intent(id, authored.state.revision, None, None);
        intent.history = ResidentSeekHistory::Preserve;
        intent.destination_frame = destination;
        intent.start_playing = playing;
        h.service.submit_resident_arrangement_seek(intent).unwrap();
        assert!(h.next().error.is_none());
        let result = h.seek_result();
        let hex_session::service::ResidentPublicationAcceptance::Submitted {
            audio_revision, ..
        } = result.result.unwrap()
        else {
            panic!("native submission")
        };
        let (revision, metadata) = h.applied(applied);
        assert_eq!(revision, audio_revision);
        assert_eq!(metadata.seek_request, Some(id));
        applied = revision;
        if !playing {
            h.audio
                .as_mut()
                .unwrap()
                .control
                .command(PlaybackCommand::Play)
                .unwrap();
        }
        // The existing native transport declick is a separate law. Compare
        // history after its fixed settle window, using the actual callback clock.
        let start = h
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .performance_frame
            .load(Acquire);
        let settled = start + (FADE_BLOCKS * 64) as u64;
        let deadline = Instant::now() + Duration::from_secs(10);
        while h
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .performance_frame
            .load(Acquire)
            < settled
        {
            // A resumed streamed owner may wait for its first real page. A
            // render call alone does not prove an advancing transport block.
            h.render();
            assert!(Instant::now() < deadline, "streamed resume did not advance");
            std::thread::yield_now();
        }
        assert_pcm(&h, &graph, &resources);
        if id == 1 {
            h.refuse.store(true, std::sync::atomic::Ordering::Release);
            let mut refused = h.seek_intent(2, authored.state.revision, None, None);
            refused.history = ResidentSeekHistory::Preserve;
            refused.destination_frame = 24_004;
            h.service.submit_resident_arrangement_seek(refused).unwrap();
            assert!(h.next().error.is_none());
            assert!(
                h.seek_result()
                    .result
                    .unwrap_err()
                    .contains("fixture final admission refusal")
            );
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
            h.refuse.store(false, std::sync::atomic::Ordering::Release);
            assert_pcm(&h, &graph, &resources);
        }
    }
    let mut invalid = h.seek_intent(4, authored.state.revision, None, None);
    invalid.history = ResidentSeekHistory::Preserve;
    invalid.section = Some(hex_session::arrangement_seek::SectionLaunch {
        // A one-shot launch matches this project without an enabled loop, so
        // the rejection reaches the history/section incompatibility itself.
        region: hex_project::LoopRegion::new(129, 513, false).unwrap(),
        quantization: hex_engine::LaunchQuantization::Immediate,
    });
    h.service.submit_resident_arrangement_seek(invalid).unwrap();
    let _event = h.next();
    assert!(
        h.seek_result()
            .result
            .unwrap_err()
            .contains("history-preserving sections")
    );
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
}

#[test]
fn streamed_section_launch_tracks_audible_clock_with_main_latency() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let _native = native_fixture();
            let files = Files::new();
            let mut h = ResidentHarness::with_frame_limit(4);
            assert!(h.next().error.is_none());
            let mut applied = h.applied(0).0;
            let imported = h.run(Request::ImportToNewTrack {
                path: files.wav(48_000, 32_768),
                before_track: None,
                start: 0,
            });
            applied = h.applied(applied).0;
            assert!(
                imported
                    .state
                    .canonical_audio_source
                    .as_ref()
                    .unwrap()
                    .sources()
                    .values()
                    .any(|source| matches!(
                        source,
                        hex_session::AudioSourceBinding::VerifiedFile(_)
                    ))
            );
            let configured = h.run(Request::Edit(Command::SetLoopRegion {
                start: 16_000,
                end: 16_384,
                enabled: true,
            }));
            assert!(configured.error.is_none(), "{:?}", configured.error);
            applied = h.applied(applied).0;
            let limiter = hex_engine::default_native_device_state_for_class(
                hex_engine::NativeDeviceClass::Effect,
                hex_engine::DeviceTypeId::from_static("hex.limiter"),
            )
            .unwrap();
            let limiter = hex_engine::patch_native_device_parameter(
                &limiter,
                ParameterId::from_static(6),
                ParameterValue::Choice(2),
            )
            .unwrap();
            let edited = h.run(Request::Edit(Command::InsertMainDevice {
                before: None,
                state: limiter,
            }));
            assert!(edited.error.is_none(), "{:?}", edited.error);
            applied = h.applied(applied).0;
            h.audio
                .as_mut()
                .unwrap()
                .control
                .command(PlaybackCommand::Play)
                .unwrap();
            for _ in 0..8 {
                h.render();
            }
            let mut intent = h.seek_intent(1, edited.state.revision, None, None);
            intent.destination_frame = 16_000;
            intent.section = Some(hex_session::arrangement_seek::SectionLaunch {
                region: hex_project::LoopRegion::new(16_000, 16_384, true).unwrap(),
                quantization: hex_engine::LaunchQuantization::Immediate,
            });
            h.service.submit_resident_arrangement_seek(intent).unwrap();
            let submitted = h.next();
            assert!(submitted.error.is_none(), "{:?}", submitted.error);
            let hex_session::service::ResidentPublicationAcceptance::Submitted {
                audio_revision,
                ..
            } = h.seek_result().result.unwrap()
            else {
                panic!("section publication must be submitted")
            };
            let deadline = Instant::now() + Duration::from_secs(10);
            let first = loop {
                h.pump();
                let pcm = h.render();
                let status = h.audio.as_ref().unwrap().status().unwrap();
                if status.applied_revision.load(Acquire) == audio_revision {
                    break pcm;
                }
                assert_eq!(status.applied_revision.load(Acquire), applied);
                assert!(
                    Instant::now() < deadline,
                    "streamed section did not install"
                );
                std::thread::yield_now();
            };
            let status = h.audio.as_ref().unwrap().status().unwrap();
            assert_eq!(status.position.load(Acquire), 16_064);
            assert_eq!(status.audible_position.load(Acquire), 16_000);
            assert!(first.iter().flatten().all(|sample| sample.is_finite()));
            let mut heard = false;
            for block in 2..=12 {
                let pcm = h.render();
                assert!(pcm.iter().flatten().all(|sample| sample.is_finite()));
                if block >= 7 {
                    heard |= pcm.iter().flatten().any(|sample| sample.abs() > 0.001);
                }
                let elapsed = block * 64;
                assert_eq!(status.position.load(Acquire), 16_000 + elapsed % 384);
                assert_eq!(
                    status.audible_position.load(Acquire),
                    16_000 + elapsed.saturating_sub(288) % 384
                );
            }
            assert!(heard, "streamed audio must be heard after the delay fills");
        })
        .unwrap()
        .join()
        .unwrap();
}
