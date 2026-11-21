//! Signed direct Resident track-effect move with a fresh receiver SDK worker.
use super::*;
use hex_audio::plugin_effects::EffectBinding;
use hex_audio::prepared::FreshRuntimeCancellationResult;
use hex_project::DeviceSelectionIds;
use hex_session::service::{
    DeviceInsertConfirmation, DeviceInsertOutcome, GraphEditOperation, ViewState,
};

const BLOCK: u64 = 256;
const BRIDGE: u64 = 2 * BLOCK;
const FRAMES: u32 = 131_072;
const DONOR_GAIN: f32 = 0.5;
const RECEIVER_GAIN: f32 = 0.125;
const DONOR_INPUT: i16 = 1024;
const RECEIVER_INPUT: i16 = 2048;

fn status(h: &ResidentHarness) -> &hex_audio::prepared::Status {
    h.audio.as_ref().unwrap().status().unwrap()
}

fn assert_project_retained(event: &Event, original: &ViewState) {
    assert_eq!(event.state.project, original.project);
    assert_eq!(event.state.revision, original.revision);
    assert_eq!(event.state.can_undo, original.can_undo);
    assert_eq!(event.state.can_redo, original.can_redo);
}

fn assert_donor_pcm(h: &ResidentHarness) {
    assert_track_pcm_at_gain(h, None, RECEIVER_GAIN);
}

fn assert_track_pcm_at_gain(h: &ResidentHarness, receiver_start: Option<u64>, receiver_gain: f32) {
    let frame = status(h).position.load(Acquire);
    let start = receiver_start.unwrap_or(0);
    assert!(frame >= start + BRIDGE);
    let pcm = instruments::render_replay_block(h);
    assert_eq!(status(h).position.load(Acquire), frame + BLOCK);
    // Each fresh SDK instance begins its private quantum ramp independently.
    // Both direct tracks align at the existing bridge.
    let wet = 0.5 + ((frame - start - BRIDGE) / BLOCK) as f32 * 0.125;
    let donor = f32::from(DONOR_INPUT) / 32768.0 * DONOR_GAIN;
    let receiver = f32::from(RECEIVER_INPUT) / 32768.0 * receiver_gain;
    let expected = if receiver_start.is_some() {
        donor + receiver * wet
    } else {
        donor * wet + receiver
    };
    assert!(
        pcm.iter().all(|sample| *sample == [expected; 2]),
        "receiver_start={receiver_start:?} frame={frame}: first sample {:?}, expected {expected}",
        pcm[0],
    );
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn running_resident_track_vst3_effect_move_installs_a_fresh_receiver_worker() {
    run_track_effect_move(MoveCase::Fresh);
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn confirmed_resident_track_vst3_effect_move_installs_the_prepared_worker() {
    run_track_effect_move(MoveCase::Confirmed);
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn cancelling_a_running_track_effect_move_restores_the_original_wet_route() {
    run_track_effect_move(MoveCase::Cancelled);
}

#[derive(PartialEq, Eq)]
enum MoveCase {
    Fresh,
    Confirmed,
    Cancelled,
}

fn run_track_effect_move(case: MoveCase) {
    let approve_interruption = case == MoveCase::Confirmed;
    let _native = native_fixture();
    let files = Files::new();
    let directory = files.0.canonicalize().unwrap();
    hex_ui::write_plugin_effect_test_catalog(&directory, &selection(), "Fx", 0).unwrap();
    let mut h = ResidentHarness::with_plugins_at_rate_and_catalog(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: BRIDGE as u32,
        }),
        48_000,
        directory.join("plugin-catalog.json"),
        8 * u64::from(FRAMES),
    );
    assert!(h.next().error.is_none());
    let (empty, _) = h.applied(0);
    let first = h.run(Request::Import(files.constant_wav(
        "donor.wav",
        48_000,
        FRAMES,
        &[DONOR_INPUT],
    )));
    let donor = first.state.project.tracks()[0].id();
    let (first_revision, _) = h.applied(empty);
    let pair = h.run(Request::Import(files.constant_wav(
        "receiver.wav",
        48_000,
        FRAMES,
        &[RECEIVER_INPUT],
    )));
    let [_, receiver] = pair.state.project.tracks() else {
        panic!("two direct audio tracks");
    };
    let receiver = receiver.id();
    let (pair_revision, _) = h.applied(first_revision);
    let pair_revision = if approve_interruption {
        // An independent third row deliberately stays on the existing
        // approved interrupted move policy.
        h.run(Request::Edit(Command::AddTrack {
            name: "third".into(),
        }));
        h.applied(pair_revision).0
    } else {
        pair_revision
    };
    let mixed = h.run(Request::Edit(Command::SetTrackGains {
        gains: vec![(donor, DONOR_GAIN), (receiver, RECEIVER_GAIN)],
    }));
    let (mix_revision, _) = h.applied(pair_revision);
    h.run(Request::InsertPluginEffect(request(
        &mixed.state,
        PluginEffectDestination::Track(donor),
    )));
    let inserted = terminal(&mut h);
    assert!(inserted.error.is_none(), "{:?}", inserted.error);
    assert!(inserted.plugin_effect_complete[0].accepted);
    let Some(Created::Device(device)) = inserted.created else {
        panic!("sole original track effect");
    };
    h.applied(mix_revision);
    let before = &inserted.state;
    let selected = DeviceSelectionIds::new(&[device]).unwrap();
    let mut target = hex_project::Editor::new(before.project.as_ref().clone(), 8).unwrap();
    target
        .execute(Command::MoveTrackDevices {
            source: donor,
            selected: selected.clone(),
            destination: receiver,
        })
        .unwrap();
    let live_worker = |h: &ResidentHarness| {
        let [Some((live, worker, _)), None] = h.plugin_progress.lock().unwrap().source_bindings
        else {
            panic!("exactly one accepted SDK worker");
        };
        assert_eq!(live, device.value());
        worker
    };
    wait(|| h.plugin_progress.lock().unwrap().source_bindings[0].is_some());
    let original = live_worker(&h);
    let receiver_owner = hex_engine::ProcessorChainOwner::Track(receiver.value());
    let predecessor = status(&h).recording_source().unwrap();
    assert_eq!(original.root, predecessor.output_identity);
    assert_eq!(predecessor.context.editor_revision, before.revision);
    assert_eq!(
        h.submitted_observation()
            .unwrap()
            .main_latency()
            .unwrap()
            .arrival_frames,
        BRIDGE as u32,
    );
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for _ in 0..4 {
        instruments::render_replay_block(&h);
    }
    assert_donor_pcm(&h);
    assert_eq!(status(&h).recording_source(), Some(predecessor));

    let prepared = h
        .device_insert_probe()
        .observe_next_foreign_effect_binding(device);
    let (publication_ready, enqueue, _busy) = h.probe_next_native_publication();
    let (submitted, resume) = h.probe_next_submitted_device_insert();
    h.service
        .submit(Request::MoveTrackDevices {
            project_generation: predecessor.context.project_generation,
            revision: before.revision,
            source: donor,
            selected,
            destination: receiver,
        })
        .unwrap();
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
            assert_project_retained(&event, before);
            if event.request_complete {
                assert_eq!(status(&h).recording_source(), Some(predecessor));
                assert_eq!(live_worker(&h), original);
                assert_donor_pcm(&h);
                panic!(
                    "hosted effect move ended before submission: error={:?}, receipt={:?}, prepared={:?}",
                    event.error,
                    event.device_insert_complete,
                    prepared.try_recv(),
                );
            }
            assert!(event.error.is_none(), "{:?}", event.error);
        }
        assert!(Instant::now() < deadline, "hosted effect did not stage");
        // Fresh source preparation can wait for native mailbox progress on
        // Session. Pace the old worker only while publication is still gated.
        if !enqueued {
            assert_donor_pcm(&h);
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    let candidate = prepared
        .try_recv()
        .expect("prepared binding before submission");
    assert_eq!(exact.output_identity, predecessor.output_identity);
    assert_eq!(exact.context.editor_revision, before.revision + 1);
    assert!(exact.audio_revision > predecessor.audio_revision);
    assert_eq!(status(&h).recording_source(), Some(predecessor));
    assert_eq!(live_worker(&h), original);
    resume.send(()).unwrap();
    let pending = h.next();
    assert!(pending.error.is_none(), "{:?}", pending.error);
    assert!(
        !pending.request_complete,
        "move did not wait for its callback"
    );
    assert_project_retained(&pending, before);
    let (prepared_owner, fresh) = candidate.expect("prepared receiver worker");
    assert_eq!(
        prepared_owner, receiver_owner,
        "receiver must own its processor"
    );
    assert_ne!(fresh, original, "the receiver must start with fresh DSP");

    if case == MoveCase::Cancelled {
        let fading = instruments::render_replay_block(&h);
        let dry = f32::from(RECEIVER_INPUT) / 32768.0 * RECEIVER_GAIN;
        assert!(fading[0][0] > dry);
        assert_eq!(fading[BLOCK as usize - 1], [dry; 2]);
        assert_eq!(status(&h).recording_source(), Some(predecessor));
        wait(|| {
            match h
                .audio
                .as_ref()
                .unwrap()
                .control
                .cancel_fresh_runtime_publication(exact.audio_revision)
            {
                FreshRuntimeCancellationResult::Busy => false,
                outcome => {
                    assert_eq!(outcome, FreshRuntimeCancellationResult::Cancelled);
                    true
                }
            }
        });
    }

    let deadline = Instant::now() + Duration::from_millis(800);
    let mut receiver_start = None;
    let terminal = 'receipt: loop {
        h.pump();
        while let Ok(event) = h.service.try_event() {
            if event.request_complete {
                break 'receipt event;
            }
            assert_project_retained(&event, before);
            if let Some(offer) = event.device_insert_confirmation {
                assert!(
                    case != MoveCase::Cancelled,
                    "pending fresh move offered a restart"
                );
                assert_eq!(offer.operation(), GraphEditOperation::MoveDevice);
                assert_eq!(offer.submission(), exact);
                if approve_interruption {
                    verify_confirmed_move(
                        &mut h,
                        before,
                        target.project(),
                        offer,
                        device,
                        original,
                        candidate,
                    );
                    return;
                }
                assert!(offer.cancel());
            }
        }
        assert!(Instant::now() < deadline, "no terminal for {exact:?}");
        let frame = status(&h).position.load(Acquire);
        let pcm = instruments::render_replay_block(&h);
        if receiver_start.is_none()
            && status(&h)
                .recording_source()
                .is_some_and(|source| source.audio_revision == exact.audio_revision)
        {
            receiver_start = Some(frame);
            assert_eq!(pcm, [[0.0; 2]; BLOCK as usize], "fresh bridge begins empty");
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    assert!(
        !approve_interruption,
        "move ended before offering confirmation: {:?}, {:?}",
        terminal.device_insert_complete, terminal.error,
    );
    let completion = terminal
        .device_insert_complete
        .expect("exact move completion");
    assert_eq!(completion.submission, exact);
    if case == MoveCase::Cancelled {
        assert_eq!(completion.outcome, DeviceInsertOutcome::Cancelled);
        assert_project_retained(&terminal, before);
        assert_eq!(status(&h).recording_source(), Some(predecessor));
        for _ in 0..4 {
            instruments::render_replay_block(&h);
        }
        assert_eq!(live_worker(&h), original);
        assert_donor_pcm(&h);
        return;
    }
    let installed = completion.outcome == DeviceInsertOutcome::Installed;
    if !installed {
        assert_project_retained(&terminal, before);
        assert_eq!(status(&h).recording_source(), Some(predecessor));
    }
    let terminal_poll = h.control_polls.load(Acquire);
    for _ in 0..4 {
        instruments::render_replay_block(&h);
    }
    let deadline = Instant::now() + Duration::from_millis(800);
    loop {
        h.pump();
        if h.control_polls.load(Acquire) > terminal_poll && h.plugin_records.load(Acquire) == 1 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "provisional worker did not retire"
        );
        instruments::render_replay_block(&h);
        std::thread::sleep(Duration::from_millis(2));
    }
    if !installed {
        assert_eq!(live_worker(&h), original);
        assert_donor_pcm(&h);
        panic!(
            "running effect did not install: {:?} for {exact:?}, error={:?}. Prepared={candidate:?}. Original {original:?} and donor PCM survived.",
            completion.outcome, terminal.error,
        );
    }
    assert!(terminal.error.is_none(), "{:?}", terminal.error);
    assert_eq!(terminal.state.project.as_ref(), target.project());
    assert_eq!(terminal.state.revision, exact.context.editor_revision);
    let applied = status(&h).recording_source().unwrap();
    assert_eq!(applied.output_identity, exact.output_identity);
    assert_eq!(applied.audio_revision, exact.audio_revision);
    assert_eq!(applied.context, exact.context);
    assert_eq!(live_worker(&h), fresh);
    let receiver_start = receiver_start.expect("exact callback cut frame");
    assert_track_pcm_at_gain(&h, Some(receiver_start), RECEIVER_GAIN);

    const UPDATED_GAIN: f32 = RECEIVER_GAIN * 0.5;
    let refreshed = h.run(Request::Edit(Command::SetTrackGains {
        gains: vec![(receiver, UPDATED_GAIN)],
    }));
    assert!(refreshed.request_complete && refreshed.error.is_none());
    let deadline = Instant::now() + Duration::from_millis(800);
    loop {
        h.pump();
        if let Some(installed) = status(&h).recording_source()
            && installed.audio_revision > applied.audio_revision
        {
            assert_eq!(installed.output_identity, applied.output_identity);
            assert_eq!(installed.context.editor_revision, refreshed.state.revision);
            break;
        }
        assert!(Instant::now() < deadline, "refresh did not install");
        instruments::render_replay_block(&h);
    }
    for _ in 0..4 {
        instruments::render_replay_block(&h);
    }
    assert_eq!(live_worker(&h), fresh);
    assert_track_pcm_at_gain(&h, Some(receiver_start), UPDATED_GAIN);
}

fn verify_confirmed_move(
    h: &mut ResidentHarness,
    before: &ViewState,
    target: &hex_project::Project,
    offer: DeviceInsertConfirmation,
    device: hex_project::DeviceId,
    original: EffectBinding,
    prepared: Option<(hex_engine::ProcessorChainOwner, EffectBinding)>,
) {
    let predecessor = status(h).recording_source().unwrap();
    let refused = offer.submission();
    assert_eq!(predecessor.context.editor_revision, before.revision);
    assert_eq!(predecessor.output_identity, refused.output_identity);
    let (_, fresh) = prepared.expect("initial offer's prepared receiver worker");
    assert_ne!(fresh, original);
    let retry_worker = h
        .device_insert_probe()
        .observe_next_foreign_effect_binding(device);
    let (submitted, resume) = h.probe_next_submitted_device_insert();
    assert!(offer.proceed());

    let mut retry = None;
    let deadline = Instant::now() + Duration::from_millis(800);
    let terminal = 'receipt: loop {
        h.pump();
        if retry.is_none()
            && let Ok(submission) = submitted.try_recv()
        {
            assert_eq!(submission.output_identity, refused.output_identity);
            assert_eq!(submission.context, refused.context);
            assert!(submission.audio_revision > refused.audio_revision);
            assert_eq!(status(h).recording_source(), Some(predecessor));
            assert_eq!(
                retry_worker.try_recv().expect("confirmed prepared worker"),
                prepared
            );
            retry = Some(submission);
            resume.send(()).unwrap();
        }
        while let Ok(event) = h.service.try_event() {
            if event.request_complete {
                break 'receipt event;
            }
            assert_project_retained(&event, before);
            assert!(event.device_insert_confirmation.is_none());
        }
        assert!(Instant::now() < deadline, "no confirmed Move terminal");
        // The confirmed preparation republishes the same snapshot. Hold
        // the real callback until observing the exact retry submission.
        if retry.is_some() {
            instruments::render_replay_block(h);
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    let completion = terminal.device_insert_complete;
    if !completion.is_some_and(|receipt| receipt.outcome == DeviceInsertOutcome::Installed) {
        eprintln!(
            "confirmed Move refused: retry={retry:?}, completion={completion:?}, error={:?}",
            terminal.error,
        );
        assert_project_retained(&terminal, before);
        assert_eq!(status(h).recording_source(), Some(predecessor));
        for _ in 0..4 {
            instruments::render_replay_block(h);
        }
        assert_donor_pcm(h);
        assert!(
            h.plugin_progress.lock().unwrap().source_bindings[0]
                .is_some_and(|(id, binding, _)| id == device.value() && binding == original)
        );
        panic!("confirmed Move refused; original {original:?} and donor PCM survived");
    }
    let installed = completion.unwrap();
    assert_eq!(Some(installed.submission), retry);
    assert!(terminal.error.is_none(), "{:?}", terminal.error);
    assert_eq!(terminal.state.project.as_ref(), target);
    assert_eq!(
        terminal.state.revision,
        installed.submission.context.editor_revision
    );
    let applied = status(h).recording_source().unwrap();
    assert_eq!(
        applied.output_identity,
        installed.submission.output_identity
    );
    assert_eq!(applied.audio_revision, installed.submission.audio_revision);
    assert_eq!(applied.context, installed.submission.context);
    assert!(status(h).playing.load(Acquire));

    let terminal_poll = h.control_polls.load(Acquire);
    for _ in 0..4 {
        instruments::render_replay_block(h);
    }
    let deadline = Instant::now() + Duration::from_millis(800);
    loop {
        h.pump();
        if h.control_polls.load(Acquire) > terminal_poll
            && h.plugin_records.load(Acquire) == 1
            && h.plugin_progress.lock().unwrap().source_bindings[0]
                .is_some_and(|(id, binding, _)| id == device.value() && binding == fresh)
        {
            break;
        }
        assert!(Instant::now() < deadline, "original worker did not return");
        instruments::render_replay_block(h);
        std::thread::sleep(Duration::from_millis(2));
    }
    // An approved interruption starts the fresh instance at its own block
    // zero, so use its exact next-block gain increment at the receiver.
    let first = instruments::render_replay_block(h);
    let second = instruments::render_replay_block(h);
    let donor = f32::from(DONOR_INPUT) / 32768.0 * DONOR_GAIN;
    let receiver = f32::from(RECEIVER_INPUT) / 32768.0 * RECEIVER_GAIN;
    assert!(first[0][0] > donor);
    assert!(first.iter().all(|sample| *sample == [first[0][0]; 2]));
    assert!(
        second
            .iter()
            .all(|sample| *sample == [first[0][0] + receiver * 0.125; 2])
    );
}
