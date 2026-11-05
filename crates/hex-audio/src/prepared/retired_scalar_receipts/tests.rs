//! Actual endpoint retirement and late sink completion, without a physical audio device.
use super::*;
use crate::native_callback_owner::{NativeRetirement, reserve_native_worker};

fn counted<T>(action: impl FnOnce() -> T) -> T {
    let mut result = None;
    let measured = allocation_counter::measure(|| result = Some(action()));
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
    result.unwrap()
}
fn dispose(retirement: &mut NativeRetirement) -> bool {
    crate::lifecycle::test_dispose_backend(
        &mut Some(()),
        &mut false,
        &mut false,
        retirement,
        |_| Ok(()),
    )
    .unwrap()
}
#[test]
fn native_finished_or_late_cancellation_survives_both_guard_release_orders_once() {
    let _serial = crate::prepared::candidate_tests::serial();
    for (errors_first, complete_before_close) in
        [(false, false), (true, false), (false, true), (true, true)]
    {
        let (control, callback, target) =
            crate::device::native_parameter_input_tests::fixture(false);
        let root = control.native_candidate_root();
        let (mut data, errors, terminals, mut client, mut publication) =
            reserve_native_worker(control, callback, None);
        let mut retirement = NativeRetirement::new(terminals, root);
        publication
            .control()
            .unwrap()
            .enable_native_custody()
            .unwrap();
        client
            .native_parameter_begin(
                target,
                crate::device::native_parameter_input_tests::value(0.5),
            )
            .unwrap();
        let mut output = [[0.0; 2]; 16];
        counted(|| {
            data.owner_mut().unwrap().run(|c, _| {
                c.begin_block();
                c.render(&mut output).unwrap();
                c.finish_native_parameter_quantum(true);
            })
        });
        client.native_parameter_release().unwrap();
        counted(|| {
            data.owner_mut().unwrap().run(|c, _| {
                c.begin_block();
                c.render(&mut output).unwrap();
            })
        });
        if complete_before_close {
            counted(|| {
                data.owner_mut()
                    .unwrap()
                    .run(|c, _| c.finish_native_parameter_quantum(true))
            });
        }
        let mut token = counted(|| client.close_retaining_native_parameter_receipts()).unwrap();
        assert_eq!(token.output_identity(), target.source.output_identity);
        assert!(client.close_retaining_native_parameter_receipts().is_none());
        publication.close();
        assert!(!retirement.advance().unwrap());
        assert!(token.try_take().is_none());
        // Native input rechecks close at the final sink and honestly cancels late completion.
        if !complete_before_close {
            counted(|| {
                data.owner_mut()
                    .unwrap()
                    .run(|c, _| c.finish_native_parameter_quantum(true))
            });
        }
        if errors_first {
            drop(errors);
            assert!(!dispose(&mut retirement));
            assert!(token.try_take().is_none());
            drop(data);
        } else {
            drop(data);
            assert!(!dispose(&mut retirement));
            assert!(token.try_take().is_none());
            drop(errors);
        }
        assert!(dispose(&mut retirement));
        let batch = token.try_take().unwrap().unwrap();
        assert_eq!(batch.target(), ScalarReceiptTarget::NativeParameter(target));
        let receipts = batch.receipts().collect::<Vec<_>>();
        assert!(
            matches!(receipts.first(), Some(ScalarReceipt::NativeParameter(crate::native_parameter_input::Receipt::Applied {target: applied,..})) if *applied == target)
        );
        if complete_before_close {
            assert_eq!(batch.terminal(), ScalarReceiptTerminal::Finished);
            assert!(
                matches!(receipts.last(), Some(ScalarReceipt::NativeParameter(crate::native_parameter_input::Receipt::Finished {target: finished,..})) if *finished == target)
            );
        } else {
            assert_eq!(batch.terminal(), ScalarReceiptTerminal::Aborted);
            assert!(
                matches!(receipts.last(), Some(ScalarReceipt::NativeParameter(crate::native_parameter_input::Receipt::Aborted {target: aborted, reason: crate::native_parameter_input::AbortReason::Cancelled})) if *aborted == target)
            );
        }
        assert_eq!(receipts.len(), 2);
        assert!(token.try_take().is_none());
        crate::prepared::candidate_tests::finish(&mut retirement);
    }
}
#[test]
fn missing_actual_terminal_is_interrupted_and_failure_publication_is_once_only() {
    let _serial = crate::prepared::candidate_tests::serial();
    let (control, callback, target) = crate::device::native_parameter_input_tests::fixture(false);
    let root = control.native_candidate_root();
    let (data, errors, terminals, mut client, mut publication) =
        reserve_native_worker(control, callback, None);
    let mut retirement = NativeRetirement::new(terminals, root);
    publication
        .control()
        .unwrap()
        .enable_native_custody()
        .unwrap();
    client
        .native_parameter_begin(
            target,
            crate::device::native_parameter_input_tests::value(0.5),
        )
        .unwrap();
    let mut token = client.close_retaining_native_parameter_receipts().unwrap();
    publication.close();
    drop(data);
    drop(errors);
    assert!(dispose(&mut retirement));
    retirement.fail_retained_receipts();
    let batch = token.try_take().unwrap().unwrap();
    assert_eq!(batch.terminal(), ScalarReceiptTerminal::Interrupted);
    assert_eq!(batch.receipts().count(), 0);
    crate::prepared::candidate_tests::finish(&mut retirement);
}

fn gain_fixture() -> (Control, Callback, crate::gain_recording::Target) {
    use hex_engine::{Audio, PreparedRevision, SineTable, playback::Playback};
    use hex_project::{Command, Created, Editor, Project};
    let mut editor = Editor::new(Project::new(48000).unwrap(), 8).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "touch".into(),
            path: "memory.wav".into(),
            source_sample_rate: 48000,
            source_frames: 4096,
            playback_frames: 4096,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("import")
    };
    editor.execute(Command::Gain { track, gain: 0.0 }).unwrap();
    let revision = Arc::new(
        PreparedRevision::prepare_for_gain_recording(
            editor.project(),
            &std::collections::BTreeMap::from([(
                asset,
                Audio::new(48000, vec![[0.25, -0.5]; 4096]).unwrap(),
            )]),
            Arc::new(SineTable::new()),
            None,
            track,
        )
        .unwrap(),
    );
    let (mut control, mut callback) = super::super::channel(48000).unwrap();
    let revision = control
        .publish(Playback::try_from_prepared_with_session(revision).unwrap())
        .unwrap();
    control.command(hex_engine::PlaybackCommand::Play).unwrap();
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 16]).unwrap();
    callback.finish_gain_recording_quantum(true);
    let target = crate::gain_recording::Target {
        track_id: track.value(),
        project_generation: 0,
        canonical_revision: 1,
        audio_revision: revision,
        recording_epoch: 1,
        transport_epoch: control.status.transport_epoch.load(Acquire),
        gesture_serial: 1,
        sample_rate: 48000,
    };
    (control, callback, target)
}
#[test]
fn gain_late_sink_finished_survives_close_and_callback_first_retirement() {
    let _serial = crate::prepared::candidate_tests::serial();
    for callback_first in [false, true] {
        let (control, callback, target) = gain_fixture();
        let root = control.native_candidate_root();
        let (mut data, errors, terminals, mut client, mut publication) =
            reserve_native_worker(control, callback, None);
        let mut retirement = NativeRetirement::new(terminals, root);
        publication
            .control()
            .unwrap()
            .enable_native_custody()
            .unwrap();
        client.gain_begin(target, 0.8).unwrap();
        let mut output = [[0.0; 2]; 16];
        counted(|| {
            data.owner_mut().unwrap().run(|c, _| {
                c.begin_block();
                c.render(&mut output).unwrap();
                c.finish_gain_recording_quantum(true);
            })
        });
        let gain = 0.8 * 16.0 / 240.0;
        assert!((output[15][0] - 0.25 * gain).abs() < 1e-6);
        client.gain_release().unwrap();
        let mut release = [[0.0; 2]; 240];
        counted(|| {
            data.owner_mut().unwrap().run(|c, _| {
                c.begin_block();
                c.render(&mut release).unwrap();
            })
        });
        assert_eq!(release[239], [0.0; 2]);
        let mut token = if callback_first {
            None
        } else {
            Some(counted(|| client.close_retaining_gain_receipts()).unwrap())
        };
        publication.close();
        if let Some(token) = token.as_mut() {
            assert!(!retirement.advance().unwrap());
            assert!(token.try_take().is_none());
        }
        counted(|| {
            data.owner_mut()
                .unwrap()
                .run(|c, _| c.finish_gain_recording_quantum(true))
        });
        drop(data);
        drop(errors);
        assert!(!retirement.advance().unwrap()); // matched completed stream witness still required
        if callback_first {
            assert!(!dispose(&mut retirement)); // callbacks destroyed while client remains live
            token = Some(client.close_retaining_gain_receipts().unwrap());
        }
        assert!(dispose(&mut retirement));
        let mut token = token.unwrap();
        let batch = token.try_take().unwrap().unwrap();
        assert_eq!(batch.terminal(), ScalarReceiptTerminal::Finished);
        let receipts = batch.receipts().collect::<Vec<_>>();
        assert!(
            matches!(receipts.as_slice(), [ScalarReceipt::Gain(crate::gain_recording::Receipt::Applied {target: a, performance_frame:16, content_frame:16, ..}), ScalarReceipt::Gain(crate::gain_recording::Receipt::Finished {target:b,performance_frame:32, content_frame:32,..})] if *a==target && *b==target)
        );
        assert!(token.try_take().is_none());
        crate::prepared::candidate_tests::finish(&mut retirement);
    }
}
#[test]
fn published_receipts_survive_later_destructor_panic_and_failure_is_distinct_from_abort() {
    let _serial = crate::prepared::candidate_tests::serial();
    for fail_before_collection in [false, true] {
        let (control, callback, target) = gain_fixture();
        let root = control.native_candidate_root();
        let (data, errors, terminals, mut client, mut publication) =
            reserve_native_worker(control, callback, None);
        let mut retirement = NativeRetirement::new(terminals, root);
        publication
            .control()
            .unwrap()
            .enable_native_custody()
            .unwrap();
        client.gain_begin(target, 0.8).unwrap();
        let mut token = client.close_retaining_gain_receipts().unwrap();
        publication.test_drop_probe(|| std::panic::panic_any(1234_u64));
        publication.close();
        drop(data);
        drop(errors);
        if fail_before_collection {
            retirement.fail_retained_receipts();
        }
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| dispose(&mut retirement)))
                .is_err()
        );
        retirement.fail_retained_receipts();
        if fail_before_collection {
            assert!(matches!(
                token.try_take(),
                Some(Err(RetiredReceiptFailure::Unavailable))
            ));
        } else {
            assert_eq!(
                token.try_take().unwrap().unwrap().terminal(),
                ScalarReceiptTerminal::Interrupted
            );
        }
        assert!(retirement.advance().is_err()); // no destructor retry or false release
        // Fixture teardown does not assert successful native disposal.
    }
}

#[test]
fn detached_receipt_token_does_not_hold_output_lease_or_endpoint_destruction() {
    let _serial = crate::prepared::candidate_tests::serial();
    let lease = crate::lifecycle::Lease::acquire().unwrap();
    let (control, callback, target) = gain_fixture();
    let root = control.native_candidate_root();
    let (data, errors, terminals, mut client, mut publication) =
        reserve_native_worker(control, callback, None);
    let retirement = NativeRetirement::new(terminals, root);
    publication
        .control()
        .unwrap()
        .enable_native_custody()
        .unwrap();
    let destroyed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observed = Arc::clone(&destroyed);
    let caller = std::thread::current().id();
    client.test_drop_probe(move || {
        assert_ne!(std::thread::current().id(), caller);
        observed.store(true, Release);
    });
    client.gain_begin(target, 0.5).unwrap();
    let mut token = client.close_retaining_gain_receipts().unwrap();
    publication.close();
    let mut stream = crate::lifecycle::StreamOwner::prepared_custody(lease, None, retirement);
    stream.expose_headless_backend();
    drop(data);
    drop(errors);
    drop(stream); // Existing single lifecycle executor receives the entire owner.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let replacement = loop {
        if let Ok(lease) = crate::lifecycle::Lease::acquire() {
            break lease;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "retained token held native lease"
        );
        std::thread::yield_now();
    };
    assert!(destroyed.load(Acquire));
    assert_eq!(
        token.try_take().unwrap().unwrap().terminal(),
        ScalarReceiptTerminal::Interrupted
    );
    drop(token); // The replacement lease was already acquired while this reader survived.
    drop(replacement);
}
