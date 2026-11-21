//! A powered-off hosted Return keeps its aligned send in the current pass.
use super::*;
use hex_session::service::{ResidentGraphSubmission, ViewState};

const FADE: usize = 240;

fn assert_aligned_bypass(h: &ResidentHarness, latency: usize) {
    let (start, pcm) = render_rolling(h);
    for (offset, frame) in pcm.into_iter().enumerate() {
        let at = start + offset;
        assert_eq!(
            frame,
            [1.5 * dry_at(at, latency); 2],
            "direct dry and bypassed Return at frame {at}, delay {latency}"
        );
    }
}

fn assert_workerless_partition(h: &ResidentHarness, partition: usize) {
    const FRAMES: usize = 514;
    let start = status(h).position.load(Acquire) as usize;
    let mut pcm = [[0.0; 2]; FRAMES];
    let mut host = [0.0; 2 * FRAMES];
    let mut slot = h.callback.lock().unwrap();
    let callback = slot.as_mut().unwrap();
    let measured = allocation_counter::measure(|| {
        for chunk in pcm.chunks_mut(partition) {
            let output = &mut host[..chunk.len() * 2];
            callback.render_native_direct(chunk, output).unwrap();
            assert!(
                output
                    .chunks_exact(2)
                    .zip(chunk.iter())
                    .all(|(host, engine)| host == engine)
            );
        }
    });
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current
        ),
        (0, 0, 0, 0),
    );
    assert!(status(h).playing.load(Acquire));
    assert_eq!(status(h).position.load(Acquire), (start + FRAMES) as u64);
    for (offset, frame) in pcm.into_iter().enumerate() {
        assert_eq!(frame, [1.5 * dry_at(start + offset, BRIDGE); 2]);
    }
}

fn assert_aligned_mix(h: &ResidentHarness, wet_scale: f32, toward_bypass: bool) {
    let (start, pcm) = render_rolling(h);
    for (offset, frame) in pcm.into_iter().enumerate() {
        let progress = (offset + 1).min(FADE) as f32 / FADE as f32;
        let bypass = if toward_bypass {
            progress
        } else {
            1.0 - progress
        };
        let at = start + offset;
        let expected = dry_at(at, BRIDGE) * (1.0 + 0.5 * bypass + wet_scale * (1.0 - bypass));
        assert!(
            frame
                .into_iter()
                .all(|sample| (sample - expected).abs() < 2e-6),
            "aligned Return ramp at {at}: {frame:?} vs {expected}"
        );
    }
}

fn assert_original(event: &Event, current: &ViewState) {
    assert_eq!(event.state.project, current.project);
    assert_eq!(event.state.revision, current.revision);
    assert_eq!(event.state.can_undo, current.can_undo);
    assert_eq!(event.state.can_redo, current.can_redo);
}

fn queued_without_callback(
    h: &mut ResidentHarness,
    current: &ViewState,
    predecessor: AppliedAudioSource,
) -> ResidentGraphSubmission {
    let deadline = Instant::now() + Duration::from_millis(400);
    let mut pending = false;
    loop {
        h.pump();
        while let Ok(event) = h.service.try_event() {
            assert!(
                !event.request_complete,
                "running Return power completed before the callback: status={:?}, error={:?}",
                event.status, event.error
            );
            assert!(event.error.is_none(), "{:?}", event.error);
            assert!(event.device_insert_confirmation.is_none());
            assert_original(&event, current);
            pending = true;
        }
        if let Some((audio_revision, metadata)) = h.submitted.as_ref()
            && pending
            && *audio_revision > predecessor.audio_revision
        {
            assert_eq!(metadata.context.editor_revision, current.revision + 1);
            return ResidentGraphSubmission {
                audio_revision: *audio_revision,
                output_identity: predecessor.output_identity,
                context: metadata.context,
            };
        }
        assert!(
            Instant::now() < deadline,
            "missing Return power preparation"
        );
        std::thread::yield_now();
    }
}

fn terminal_at_cut(
    h: &mut ResidentHarness,
    current: &ViewState,
    render_next: impl Fn(&ResidentHarness),
) -> Event {
    let deadline = Instant::now() + Duration::from_millis(400);
    loop {
        h.pump();
        while let Ok(event) = h.service.try_event() {
            assert!(
                event.device_insert_confirmation.is_none(),
                "this aligned Return should stay in the current pass"
            );
            if event.request_complete {
                return event;
            }
            assert!(event.error.is_none(), "{:?}", event.error);
            assert_original(&event, current);
        }
        assert!(
            Instant::now() < deadline,
            "missing actual Return power terminal"
        );
        render_next(h);
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn running_resident_return_power_off_waits_for_the_exact_cut_and_returns_the_worker() {
    let _native = native_fixture();
    let files = Files::new();
    let RollingHostedReturn {
        harness: mut h,
        inserted,
        track,
        bus,
        device,
        predecessor,
        binding,
    } = rolling_hosted_return(&files);
    h.enable_confirmed_device_inserts();
    let current = &inserted.state;
    let original_device = current.project.device(device).unwrap();
    let device_type = original_device.state().type_id();
    let original_worker = |h: &ResidentHarness| {
        assert_eq!(status(h).recording_source(), Some(predecessor));
        assert_eq!(h.plugin_records.load(Acquire), 1);
        let (live, accepted, _) = h.plugin_progress.lock().unwrap().source_bindings[0].unwrap();
        assert_eq!((live, accepted), (device.value(), binding));
    };
    let mut installed = None;
    for cancel in [true, false] {
        let next_wet = assert_aligned_wet(&h) + 0.0625 - 1.0;
        h.service
            .submit(Request::Edit(Command::SetDeviceBypass {
                device,
                bypassed: true,
            }))
            .unwrap();
        let exact = queued_without_callback(&mut h, current, predecessor);
        original_worker(&h);
        assert_aligned_mix(&h, next_wet, true);
        original_worker(&h);

        if cancel {
            let deadline = Instant::now() + Duration::from_millis(200);
            loop {
                match h
                    .audio
                    .as_ref()
                    .unwrap()
                    .control
                    .cancel_fresh_runtime_publication(exact.audio_revision)
                {
                    FreshRuntimeCancellationResult::Busy => {
                        assert!(Instant::now() < deadline, "Return cancellation stayed busy");
                        std::thread::yield_now();
                    }
                    result => {
                        assert_eq!(result, FreshRuntimeCancellationResult::Cancelled);
                        break;
                    }
                }
            }
            assert_aligned_mix(&h, next_wet + 0.0625, false);
            let event = terminal_at_cut(&mut h, current, |h| {
                assert_aligned_wet(h);
            });
            let receipt = event.device_insert_complete.unwrap();
            assert_eq!(receipt.submission, exact);
            assert_eq!(receipt.outcome, DeviceInsertOutcome::Cancelled);
            assert_original(&event, current);
            original_worker(&h);
            assert_aligned_wet(&h);
        } else {
            let event = terminal_at_cut(&mut h, current, |h| assert_aligned_bypass(h, BRIDGE));
            assert!(event.error.is_none(), "{:?}", event.error);
            let receipt = event.device_insert_complete.unwrap();
            assert_eq!(receipt.submission, exact);
            assert_eq!(receipt.outcome, DeviceInsertOutcome::Installed);
            installed = Some((event, exact));
        }
    }

    let (powered_off, exact) = installed.unwrap();
    let applied = status(&h).recording_source().unwrap();
    assert_eq!(applied.output_identity, exact.output_identity);
    assert_eq!(applied.context, exact.context);
    assert_eq!(applied.audio_revision, exact.audio_revision);
    assert_ne!(applied, predecessor);
    assert_eq!(powered_off.state.revision, current.revision + 1);
    assert_eq!(powered_off.state.project.tracks(), current.project.tracks());
    let [kept_bus] = powered_off.state.project.return_tracks() else {
        panic!("the original Return is still routed");
    };
    assert_eq!(kept_bus.id(), bus);
    let [off] = kept_bus.devices() else {
        panic!("the original authored device is still in its slot");
    };
    assert_eq!(off.id(), device);
    assert!(off.bypassed());
    assert_eq!(off.state(), original_device.state());
    let observation = h.submitted_observation().unwrap();
    assert_eq!(
        observation.processor_status(device.value(), device_type),
        Some(hex_engine::PreparedDeviceProcessing::PoweredOff)
    );
    assert_eq!(observation.main_latency().unwrap().arrival_frames, 0);
    assert!(status(&h).native_effect_source().is_none());

    let deadline = Instant::now() + Duration::from_millis(400);
    while h.plugin_records.load(Acquire) != 0 {
        assert_aligned_bypass(&h, BRIDGE);
        h.pump();
        assert!(
            Instant::now() < deadline,
            "original Return worker stayed warm"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(h.plugin_progress.lock().unwrap().source_bindings[0].is_none());
    for _ in 0..3 {
        assert_aligned_bypass(&h, BRIDGE);
    }
    for partition in [1, 257] {
        assert_workerless_partition(&h, partition);
    }

    let renamed = h.run(Request::Edit(Command::RenameTrack {
        track,
        name: "dry refresh".into(),
    }));
    assert_eq!(renamed.state.revision, powered_off.state.revision + 1);
    let deadline = Instant::now() + Duration::from_millis(400);
    let refreshed = loop {
        h.pump();
        assert_aligned_bypass(&h, BRIDGE);
        assert_eq!(h.plugin_records.load(Acquire), 0);
        if let Some(source) = status(&h).recording_source()
            && source.context.editor_revision == renamed.state.revision
        {
            break source;
        }
        assert!(
            Instant::now() < deadline,
            "workerless refresh did not apply"
        );
        std::thread::sleep(Duration::from_millis(1));
    };
    assert_eq!(refreshed.output_identity, applied.output_identity);
    assert!(refreshed.audio_revision > applied.audio_revision);
    h.pump();
    assert_eq!(
        h.submitted_observation()
            .unwrap()
            .processor_status(device.value(), device_type),
        Some(hex_engine::PreparedDeviceProcessing::PoweredOff)
    );
    assert!(status(&h).native_effect_source().is_none());

    h.audio.as_ref().unwrap().control.stop();
    render(&h);
    assert!(!status(&h).playing.load(Acquire));
    assert_eq!(status(&h).position.load(Acquire), 0);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    assert_aligned_bypass(&h, 0);
}
