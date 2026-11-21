//! Signed SDK witness for cutting a hosted instrument to a fresh Track worker.
use super::*;
use hex_audio::plugin_effects::EffectBinding;
use hex_audio::prepared::{AppliedAudioSource, FreshRuntimeCancellationResult};
use hex_project::{ClipId, DeviceSelectionIds, TrackId};
use hex_session::service::{DeviceInsertOutcome, ResidentGraphSubmission, ViewState};

const BRIDGE: u64 = 512;
const BLOCK: u64 = 256;
const DONOR_GAIN: f32 = 0.25;
const RECEIVER_GAIN: f32 = 0.75;

#[test]
#[ignore = "requires explicit source-bound SDK receipts"]
fn direct_vst3_cut_ends_the_old_gate_and_starts_a_fresh_receiver() {
    verify_direct_vst3_cut(AfterCut::Refresh);
}

#[test]
#[ignore = "requires explicit source-bound SDK receipts"]
fn second_live_vst3_move_back_prepares_a_third_worker() {
    verify_direct_vst3_cut(AfterCut::MoveBack);
}

#[test]
#[ignore = "requires explicit source-bound SDK receipts"]
fn undo_and_redo_a_vst3_cut_install_fresh_workers_on_their_exact_receipts() {
    verify_direct_vst3_cut(AfterCut::History);
}

enum AfterCut {
    Refresh,
    MoveBack,
    History,
}

fn verify_direct_vst3_cut(after_cut: AfterCut) {
    const CHILD: &str = "HEX_TEST_HELD_VST3_TRACK_CUT_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let name = std::thread::current().name().unwrap().to_owned();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--ignored", "--nocapture"])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "127")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    let _native = native_fixture();
    let catalog = Files::new();
    let directory = catalog.0.canonicalize().unwrap();
    let mut selected = selection();
    let module = directory.join("original.vst3");
    super::super::ui::copy_original_module(&selected.module, &module);
    selected.module = module.clone();
    hex_ui::write_plugin_effect_test_catalog(&directory, &selected, "Instrument|Synth", 0).unwrap();
    let mut h = ResidentHarness::with_plugins_and_catalog(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: BRIDGE as u32,
        }),
        directory.join("plugin-catalog.json"),
    );
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let (donor, donor_clip) = add_lane(&mut h, &mut applied, "held source");
    let (receiver, receiver_clip) = add_lane(&mut h, &mut applied, "destination");
    let donor_gate = BeatTime::from_quarters(1).unwrap();
    let new_onset = BeatTime::from_quarters(3).unwrap();
    let new_gate = BeatTime::from_ratio(1, 8).unwrap();
    for (clip, start, duration, velocity) in [
        (donor_clip, BeatTime::ZERO, donor_gate, 63),
        (
            receiver_clip,
            BeatTime::ZERO,
            BeatTime::from_ratio(5, 4).unwrap(),
            119,
        ),
        (receiver_clip, new_onset, new_gate, 85),
    ] {
        // The controlled SDK variant requires pitch 60 and channel 0 and
        // distinguishes simultaneous named note IDs by summed velocity.
        h.run(Request::Edit(Command::AddMidiNote {
            clip,
            start,
            duration,
            velocity,
            pitch: 60,
            channel: 0,
        }));
        (applied, _) = h.applied(applied);
    }
    let mixed = h.run(Request::Edit(Command::SetTrackGains {
        gains: vec![(donor, DONOR_GAIN), (receiver, RECEIVER_GAIN)],
    }));
    (applied, _) = h.applied(applied);
    let mut insert = request(&mixed.state, PluginEffectDestination::Instrument(donor));
    insert.selection = selected;
    h.run(Request::InsertPluginEffect(insert));
    let before = terminal(&mut h);
    assert!(before.plugin_effect_complete[0].accepted);
    let instrument = before
        .state
        .project
        .track(donor)
        .unwrap()
        .instrument()
        .unwrap();
    let device = instrument.id();
    h.applied(applied);
    assert_eq!(before.state.project.tracks().len(), 2);
    assert!(
        before
            .state
            .project
            .tracks()
            .iter()
            .all(|track| track.clips().is_empty() && track.devices().is_empty())
    );
    assert_eq!(
        h.submitted_observation()
            .unwrap()
            .processor_latency(device.value(), instrument.state().type_id())
            .unwrap()
            .stage_frames,
        BRIDGE as u32
    );

    let authored = instrument.state().clone();
    let worker = live_binding(&h, device);
    h.enable_confirmed_device_inserts();
    let control = &mut h.audio.as_mut().unwrap().control;
    control.set_audio_diagnostics(hex_audio::audio_diagnostics::AudioDiagnosticsFlags::ALL);
    control.command(PlaybackCommand::Play).unwrap();
    for _ in 0..5 {
        render_completed_block(&h);
    }
    let wet = DONOR_GAIN * 63.0 / 127.0;
    assert_sample_level(&render_completed_block(&h), wet);
    let predecessor = status(&h).recording_source().unwrap();
    let forward = || Request::MoveTrackDevices {
        project_generation: predecessor.context.project_generation,
        revision: before.state.revision,
        source: donor,
        selected: DeviceSelectionIds::new(&[device]).unwrap(),
        destination: receiver,
    };
    if matches!(after_cut, AfterCut::Refresh) {
        // The running worker has already loaded this private fixture. Hide
        // the module briefly so the fresh worker cannot pass file inspection.
        let parked = directory.join("parked.vst3");
        std::fs::rename(&module, &parked).unwrap();
        h.service.submit(forward()).unwrap();
        let failure = h.next();
        std::fs::rename(parked, &module).unwrap();
        assert!(failure.request_complete && failure.error.is_some());
        assert_eq!(failure.state.project, before.state.project);
        assert_eq!(failure.state.revision, before.state.revision);
        assert_eq!(status(&h).recording_source(), Some(predecessor));
        assert_eq!(live_binding(&h, device), worker);
        assert_sample_level(&render_continuous_block(&h).1, wet);
    }
    let mut expected = hex_project::Editor::new(before.state.project.as_ref().clone(), 8).unwrap();
    expected
        .execute(Command::MoveTrackDevices {
            source: donor,
            selected: DeviceSelectionIds::new(&[device]).unwrap(),
            destination: receiver,
        })
        .unwrap();

    let route = ObservedRoute {
        destination: receiver,
        device,
        incumbent: worker,
    };
    let (cancelled, abandoned) = stage_move(&mut h, &before.state, forward(), route, wet);
    wait(|| {
        match h
            .audio
            .as_ref()
            .unwrap()
            .control
            .cancel_fresh_runtime_publication(cancelled.audio_revision)
        {
            FreshRuntimeCancellationResult::Busy => false,
            state => {
                assert_eq!(state, FreshRuntimeCancellationResult::Cancelled);
                true
            }
        }
    });
    let deadline = Instant::now() + Duration::from_millis(800);
    loop {
        assert_sample_level(&render_continuous_block(&h).1, wet);
        h.pump();
        if let Ok(event) = h.service.try_event() {
            assert_eq!(event.state.project, before.state.project);
            assert_eq!(event.state.revision, before.state.revision);
            if event.request_complete {
                let completion = event.device_insert_complete.unwrap();
                assert_eq!(completion.submission, cancelled);
                assert_eq!(completion.outcome, DeviceInsertOutcome::Cancelled);
                break;
            }
        }
        assert!(
            Instant::now() < deadline,
            "cancelled move receipt timed out"
        );
    }
    assert_eq!(status(&h).recording_source(), Some(predecessor));
    assert_eq!(live_binding(&h, device), worker);

    let (installed, fresh) = install_move(
        &mut h,
        &before.state,
        forward(),
        expected.project(),
        route,
        wet,
    );
    assert_ne!(fresh, abandoned);
    assert_eq!(
        installed
            .project
            .track(receiver)
            .unwrap()
            .instrument()
            .unwrap()
            .state(),
        &authored
    );
    let map = installed.project.tempo_map();
    let donor_off = map.beat_to_frame(donor_gate).unwrap() + BRIDGE;
    let receiver_on = map.beat_to_frame(new_onset).unwrap() + BRIDGE;
    let receiver_off = map
        .beat_to_frame(BeatTime::from_raw(new_onset.raw() + new_gate.raw()))
        .unwrap()
        + BRIDGE;
    assert!(
        cursor(&h) < donor_off,
        "the original note must have been held at the cut"
    );

    let destination_binding = match after_cut {
        AfterCut::Refresh => {
            let previous = status(&h).recording_source().unwrap();
            let refreshed = h.run(Request::Edit(Command::SetTrackGains {
                gains: vec![(donor, 0.5)],
            }));
            assert!(refreshed.request_complete && refreshed.error.is_none());
            let deadline = Instant::now() + Duration::from_millis(800);
            loop {
                assert_sample_level(&render_continuous_block(&h).1, 0.0);
                if let Some(source) = status(&h).recording_source()
                    && source.audio_revision > previous.audio_revision
                {
                    assert_eq!(source.context.editor_revision, refreshed.state.revision);
                    assert_eq!(source.output_identity, predecessor.output_identity);
                    break;
                }
                assert!(Instant::now() < deadline && cursor(&h) < receiver_on);
            }
            fresh
        }
        AfterCut::MoveBack | AfterCut::History => {
            let undo = if matches!(after_cut, AfterCut::History) {
                Request::Undo
            } else {
                Request::MoveTrackDevices {
                    project_generation: predecessor.context.project_generation,
                    revision: installed.revision,
                    source: receiver,
                    selected: DeviceSelectionIds::new(&[device]).unwrap(),
                    destination: donor,
                }
            };
            let (undone, returned) = install_move(
                &mut h,
                &installed,
                undo,
                &before.state.project,
                ObservedRoute {
                    destination: donor,
                    device,
                    incumbent: fresh,
                },
                0.0,
            );
            assert_ne!(returned, worker);
            if matches!(after_cut, AfterCut::MoveBack) {
                assert_eq!(
                    undone
                        .project
                        .track(donor)
                        .unwrap()
                        .instrument()
                        .unwrap()
                        .state(),
                    &authored
                );
                return;
            }
            let (_, redone) = install_move(
                &mut h,
                &undone,
                Request::Redo,
                &installed.project,
                ObservedRoute {
                    destination: receiver,
                    device,
                    incumbent: returned,
                },
                0.0,
            );
            assert_ne!(redone, fresh);
            redone
        }
    };
    assert_eq!(live_binding(&h, device), destination_binding);
    let deadline = Instant::now() + Duration::from_secs(2);
    assert!(
        cursor(&h) < receiver_on,
        "the new onset must still be in the future"
    );
    while cursor(&h) < receiver_off + BLOCK {
        assert!(Instant::now() < deadline);
        let (start, output) = render_continuous_block(&h);
        for (offset, frame) in output.into_iter().enumerate() {
            let at = start + offset as u64;
            let expected = if (receiver_on..receiver_off).contains(&at) {
                RECEIVER_GAIN * 85.0 / 127.0
            } else {
                0.0
            };
            assert!(
                frame
                    .into_iter()
                    .all(|sample| (sample - expected).abs() < 1e-5),
                "frame {at}: {frame:?}, expected {expected}"
            );
        }
    }
    assert_eq!(status(&h).render_fault_snapshot(), None);
}

#[derive(Clone, Copy)]
struct ObservedRoute {
    destination: TrackId,
    device: hex_project::DeviceId,
    incumbent: EffectBinding,
}

fn live_binding(h: &ResidentHarness, device: hex_project::DeviceId) -> EffectBinding {
    let [Some((id, binding, _)), None] = h.plugin_progress.lock().unwrap().source_bindings else {
        panic!("one accepted SDK worker");
    };
    assert_eq!(id, device.value());
    binding
}

fn stage_move(
    h: &mut ResidentHarness,
    before: &ViewState,
    request: Request,
    route: ObservedRoute,
    before_sample: f32,
) -> (ResidentGraphSubmission, EffectBinding) {
    let predecessor = status(h).recording_source().unwrap();
    let prepared_worker = h
        .device_insert_probe()
        .observe_next_foreign_binding(route.destination, route.device);
    let (publication_ready, enqueue, _) = h.probe_next_native_publication();
    let (submitted, resume) = h.probe_next_submitted_device_insert();
    h.service.submit(request).unwrap();
    let deadline = Instant::now() + Duration::from_millis(800);
    let mut enqueued = false;
    let exact = loop {
        h.pump();
        if let Ok(exact) = submitted.try_recv() {
            break exact;
        }
        if !enqueued && publication_ready.try_recv().is_ok() {
            enqueue.send(()).unwrap();
            enqueued = true;
        }
        if let Ok(event) = h.service.try_event() {
            assert!(
                !event.request_complete && event.error.is_none(),
                "{:?}",
                event.error
            );
            assert_eq!(event.state.project, before.project);
        }
        assert!(Instant::now() < deadline, "move did not reach submission");
        if !enqueued {
            assert_sample_level(&render_continuous_block(h).1, before_sample);
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let (owner, fresh) = prepared_worker
        .try_recv()
        .unwrap()
        .expect("prepared worker");
    assert_eq!(
        owner,
        hex_engine::ProcessorChainOwner::Track(route.destination.value()),
        "the prepared worker must belong to the destination track"
    );
    assert_ne!(
        fresh, route.incumbent,
        "a running move must prepare a fresh worker"
    );
    assert_eq!(exact.output_identity, predecessor.output_identity);
    assert_eq!(exact.context.editor_revision, before.revision + 1);
    assert_eq!(live_binding(h, route.device), route.incumbent);
    resume.send(()).unwrap();
    let pending = h.next();
    assert!(pending.error.is_none(), "{:?}", pending.error);
    assert!(!pending.request_complete);
    assert_eq!(pending.state.project, before.project);
    assert_eq!(pending.state.revision, before.revision);
    assert_eq!(status(h).recording_source(), Some(predecessor));
    (exact, fresh)
}

fn install_move(
    h: &mut ResidentHarness,
    before: &ViewState,
    request: Request,
    expected: &hex_project::Project,
    route: ObservedRoute,
    before_sample: f32,
) -> (Arc<ViewState>, EffectBinding) {
    let (exact, fresh) = stage_move(h, before, request, route, before_sample);
    let applied = AppliedAudioSource {
        audio_revision: exact.audio_revision,
        context: exact.context,
        output_identity: exact.output_identity,
    };
    let deadline = Instant::now() + Duration::from_millis(800);
    let mut heard_fade = false;
    let mut saw_cut = false;
    let finished = loop {
        let (_, pcm) = render_continuous_block(h);
        if status(h).recording_source() == Some(applied) {
            saw_cut = true;
            assert_sample_level(&pcm, 0.0);
        } else if before_sample > 0.0 && pcm[0][0] > 0.0 && pcm[BLOCK as usize - 1][0] == 0.0 {
            heard_fade = true;
        }
        h.pump();
        if let Ok(event) = h.service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            assert!(event.device_insert_confirmation.is_none());
            if event.request_complete {
                break event;
            }
            assert_eq!(event.state.project, before.project);
        }
        assert!(
            Instant::now() < deadline,
            "move did not reach its exact callback receipt"
        );
    };
    let completion = finished.device_insert_complete.unwrap();
    assert_eq!(completion.submission, exact);
    assert_eq!(completion.outcome, DeviceInsertOutcome::Installed);
    assert!(saw_cut);
    assert!(heard_fade || before_sample == 0.0);
    assert_eq!(finished.state.project.as_ref(), expected);
    assert_eq!(finished.state.revision, before.revision + 1);
    assert_eq!(live_binding(h, route.device), fresh);
    let terminal_poll = h.control_polls.load(Acquire);
    let deadline = Instant::now() + Duration::from_millis(800);
    loop {
        h.pump();
        if h.control_polls.load(Acquire) > terminal_poll && h.plugin_records.load(Acquire) == 1 {
            break;
        }
        assert!(Instant::now() < deadline, "old worker did not return");
        assert_sample_level(&render_continuous_block(h).1, 0.0);
    }
    (finished.state, fresh)
}

fn add_lane(h: &mut ResidentHarness, applied: &mut u64, name: &str) -> (TrackId, ClipId) {
    let Created::Track(track) = h
        .run(Request::Edit(Command::AddMidiTrack { name: name.into() }))
        .created
        .unwrap()
    else {
        panic!("MIDI lane");
    };
    (*applied, _) = h.applied(*applied);
    let Created::Clip(clip) = h
        .run(Request::Edit(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(4).unwrap(),
        }))
        .created
        .unwrap()
    else {
        panic!("Arrangement clip");
    };
    (*applied, _) = h.applied(*applied);
    (track, clip)
}

#[track_caller]
fn assert_sample_level(frames: &[hex_engine::Frame], expected: f32) {
    for (index, frame) in frames.iter().enumerate() {
        assert!(
            frame.iter().all(|sample| (*sample - expected).abs() < 1e-5),
            "frame {index}: {frame:?}, expected {expected}"
        );
    }
}

fn status(h: &ResidentHarness) -> &hex_audio::prepared::Status {
    h.audio.as_ref().unwrap().status().unwrap()
}

fn cursor(h: &ResidentHarness) -> u64 {
    status(h).performance_frame.load(Acquire)
}

fn render_continuous_block(h: &ResidentHarness) -> (u64, [hex_engine::Frame; BLOCK as usize]) {
    let frame = cursor(h);
    let arrangement = status(h).position.load(Acquire);
    let pcm = render_replay_block(h);
    assert_eq!(cursor(h), frame + BLOCK);
    assert_eq!(status(h).position.load(Acquire), arrangement + BLOCK);
    (frame, pcm)
}
