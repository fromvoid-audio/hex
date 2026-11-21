//! Signed rolling Resident replacement Undo on one direct equal-latency Main slot.
use super::*;

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn rolling_resident_main_vst3_replacement_undo_waits_for_its_exact_receipt() {
    let _native = native_fixture();
    let files = Files::new();
    let (mut h, original, installed, original_binding, destination) =
        installed_replacement(&files, ReplacementOwner::Main, 131_072);
    let incumbent = binding(&h).unwrap();
    let predecessor = status(&h).recording_source().unwrap();
    let target_id = occupied_effect(&original, destination).id().value();
    let ramp = 0.03125;
    let sample = |h: &ResidentHarness| instruments::render_replay_block(h)[0][0];
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let mut last = settled_ramp(&mut h, predecessor, incumbent, ramp);
    let assert_incumbent = |h: &ResidentHarness| {
        assert!(status(h).playing.load(Acquire));
        assert_eq!(status(h).recording_source(), Some(predecessor));
        assert_eq!(binding(h), Some(incumbent));
    };
    let hear_incumbent = |h: &ResidentHarness, previous: &mut f32| {
        assert_incumbent(h);
        let next = sample(h);
        assert_eq!(next - *previous, ramp);
        *previous = next;
    };

    let (prepared, enqueue, _) = h.probe_next_native_publication();
    h.service.submit(Request::Undo).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut pending = false;
    while prepared.try_recv().is_err() {
        h.pump();
        while let Ok(event) = h.service.try_event() {
            assert_history_unchanged(&event, &installed);
            assert!(event.device_insert_complete.is_none());
            hear_incumbent(&h, &mut last);
            if let Some(error) = event.error {
                panic!(
                    "rolling Resident Main replacement Undo refused before its callback: {error}"
                );
            }
            assert!(!event.request_complete);
            pending = true;
        }
        hear_incumbent(&h, &mut last);
        assert!(
            Instant::now() < deadline,
            "rolling Main replacement Undo did not prepare"
        );
        std::thread::yield_now();
    }
    let (device, fresh, accepted) = h.plugin_progress.lock().unwrap().newest_source.unwrap();
    assert_eq!(device, target_id);
    assert!(!accepted);
    assert_ne!(fresh, original_binding);
    assert_ne!(fresh, incumbent.1);
    hear_incumbent(&h, &mut last);
    hear_incumbent(&h, &mut last);
    enqueue.send(()).unwrap();

    let mut offered = None;
    let deadline = Instant::now() + Duration::from_secs(3);
    let restored = 'receipt: loop {
        h.pump();
        while let Ok(event) = h.service.try_event() {
            if let Some(error) = &event.error {
                assert_history_unchanged(&event, &installed);
                assert_incumbent(&h);
                panic!("rolling Resident Main replacement Undo refused at publication: {error}");
            }
            if event.request_complete {
                break 'receipt event;
            }
            assert_history_unchanged(&event, &installed);
            pending = true;
            if let Some(offer) = &event.device_insert_confirmation {
                assert_incumbent(&h);
                assert_eq!(offer.operation(), GraphEditOperation::ReplaceDevice);
                assert!(offered.replace(offer.submission()).is_none());
                assert!(offer.proceed());
            }
        }
        sample(&h);
        assert!(
            Instant::now() < deadline,
            "rolling Main replacement Undo missed its receipt"
        );
        std::thread::yield_now();
    };
    assert!(pending);
    let receipt = restored
        .device_insert_complete
        .expect("exact rolling history receipt");
    let current = status(&h).recording_source().unwrap();
    assert_eq!(receipt.outcome, DeviceInsertOutcome::Installed);
    assert_eq!(receipt.submission.context, current.context);
    assert_eq!(receipt.submission.audio_revision, current.audio_revision);
    assert_eq!(receipt.submission.output_identity, current.output_identity);
    assert_eq!(current.output_identity, predecessor.output_identity);
    assert_ne!(current, predecessor);
    if let Some(offered) = offered {
        assert_eq!(receipt.submission.context, offered.context);
        assert_ne!(receipt.submission.audio_revision, offered.audio_revision);
    }
    let mut expected = original.state.project.snapshot();
    expected.next_id = expected
        .next_id
        .max(installed.state.project.next_identity());
    assert_eq!(restored.state.project.snapshot(), expected);
    assert_eq!(restored.state.revision, installed.state.revision + 1);
    assert!(restored.state.can_undo && restored.state.can_redo);

    wait(|| {
        h.pump();
        sample(&h);
        h.plugin_records.load(Acquire) == 1 && binding(&h) == Some((target_id, fresh))
    });
    settled_ramp(&mut h, current, (target_id, fresh), ramp);
}

fn settled_ramp(
    h: &mut ResidentHarness,
    source: hex_audio::prepared::AppliedAudioSource,
    worker: (u64, EffectBinding),
    step: f32,
) -> f32 {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut previous = None;
    loop {
        h.pump();
        let sample = instruments::render_completed_block(h)[0][0];
        let playing = status(h).playing.load(Acquire);
        let applied = status(h).recording_source();
        let accepted = binding(h);
        if playing && applied == Some(source) && accepted == Some(worker) && sample > 0.0 {
            if previous.is_some_and(|prior| sample - prior == step) {
                return sample;
            }
            previous = Some(sample);
        } else {
            previous = None;
        }
        assert!(
            Instant::now() < deadline,
            "signed Main ramp did not settle: playing={playing}, sample={sample}, source={applied:?}, binding={accepted:?}, fault={:?}",
            status(h).render_fault_snapshot(),
        );
    }
}
