//! Actual constructor owners in the one active slot. No fake installed receipt,
//! raw Playback adoption or production admission bypass is used.
use super::*;
use crate::native_callback_owner::{ErrorCallbackOwner, Guard, NativeCallbackOwner};
use std::sync::Mutex;

type Data = Arc<Mutex<Option<Guard<NativeCallbackOwner>>>>;
type Errors = Arc<Mutex<Option<Guard<ErrorCallbackOwner>>>>;
// Real headless stream-handle owner and delayed backend closure aliases. The
// production lifecycle disposal function actually takes/drops this handle.
struct Backend {
    _data: Data,
    _errors: Errors,
    drops: Arc<AtomicU64>,
}
impl Drop for Backend {
    fn drop(&mut self) {
        self.drops.fetch_add(1, SeqCst);
    }
}
fn assert_zero(info: allocation_counter::AllocationInfo) {
    assert_eq!(
        (
            info.count_total,
            info.bytes_total,
            info.count_current,
            info.bytes_current
        ),
        (0, 0, 0, 0)
    );
}
#[test]
fn actual_active_session_audio_dormancy_requires_completed_handle_and_both_final_guard_releases() {
    let _serial = candidate_tests::serial();
    for errors_first in [false, true] {
        let Some((mut owner, clip)) = candidate_tests::candidate_with_session(true) else {
            return;
        };
        let identity = owner.identity();
        let (mut control, mut data, mut errors, retirement) = candidate_tests::anchored();
        let records = Arc::new(Mutex::new(Vec::new()));
        let r = records.clone();
        let runtime_address = owner
            .test_observe_runtime_drop(move |address| {
                r.lock()
                    .unwrap()
                    .push((address, std::thread::current().id()));
            })
            .unwrap();
        let data_address = data.owner_mut().unwrap() as *mut NativeCallbackOwner as usize;
        let error_address = errors.owner_mut().unwrap() as *mut ErrorCallbackOwner as usize;
        let r = records.clone();
        data.owner_mut().unwrap().test_drop_probe(move || {
            r.lock()
                .unwrap()
                .push((data_address, std::thread::current().id()))
        });
        let r = records.clone();
        errors.owner_mut().unwrap().test_drop_probe(move || {
            r.lock()
                .unwrap()
                .push((error_address, std::thread::current().id()))
        });
        data.owner_mut()
            .unwrap()
            .run(|callback, _| callback.test_admit_candidate = true);
        let domain = candidate_tests::pair();
        let external_lease = domain.0.catalog_lease().unwrap();
        let ticket = control
            .stage_audio_candidate(owner, vec![domain])
            .ok()
            .unwrap();
        assert_zero(allocation_counter::measure(|| {
            data.owner_mut()
                .unwrap()
                .run(|callback, _| callback.begin_block())
        }));
        data.owner_mut().unwrap().run(|callback, _| {
            let Some(ActivePublication::Candidate(active)) = callback.active.as_ref() else {
                panic!("actual active candidate missing")
            };
            assert_eq!(active.owner.identity(), identity);
        });
        // Fixture-only queue enablement: production candidate revision/ACK
        // metadata belongs to236. No applied-revision or ACK is fabricated.
        control.revision = 1;
        control
            .action_with_ticket(PlaybackAction::Session(
                hex_engine::SessionPlaybackCommand::LaunchClip {
                    clip: clip.unwrap(),
                    quantization: hex_engine::LaunchQuantization::Immediate,
                },
            ))
            .unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        let mut output = [[0.0; 2]; 3];
        assert_zero(allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|callback, _| {
                callback.begin_block();
                callback.render(&mut output).unwrap();
            })
        }));
        assert_eq!(output[0], [0.25 + 0.01 * 2.0; 2]); // actual Session source offset, not Arrangement

        assert!(control.status.playing.load(Relaxed));
        control.command(PlaybackCommand::Pause).unwrap();
        data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        });
        assert_eq!(output, [[0.0; 2]; 3]);
        assert!(!control.status.playing.load(Relaxed));
        // Priority Stop still reaches the real candidate under full command pressure.
        for _ in 0..COMMANDS {
            control.command(PlaybackCommand::Play).unwrap();
        }
        control.stop();
        assert_zero(allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|callback, _| {
                callback.begin_block();
                callback.render(&mut output).unwrap();
            })
        }));
        assert!(!control.status.playing.load(Relaxed));
        assert!(control.recover_audio_candidate(&ticket).is_none());
        let data = Arc::new(Mutex::new(Some(data)));
        let errors = Arc::new(Mutex::new(Some(errors)));
        let drops = Arc::new(AtomicU64::new(0));
        let mut stream = Some(Backend {
            _data: data.clone(),
            _errors: errors.clone(),
            drops: drops.clone(),
        });
        let ui_thread = std::thread::current().id();
        let handle = std::thread::spawn(move || {
            let mut retirement = retirement;
            let mut pending = false;
            let mut disposed = false;
            // Failed pause/play-retirement attempt is not closure.
            assert!(
                !crate::lifecycle::test_dispose_backend(
                    &mut stream,
                    &mut pending,
                    &mut disposed,
                    &mut retirement,
                    |_| Err(())
                )
                .unwrap()
            );
            assert!(stream.is_some());
            assert!(!disposed);
            assert!(
                !crate::lifecycle::test_dispose_backend(
                    &mut stream,
                    &mut pending,
                    &mut disposed,
                    &mut retirement,
                    |_| Ok(())
                )
                .unwrap()
            );
            assert!(stream.is_none());
            assert!(disposed);
            assert_eq!(drops.load(SeqCst), 1);
            (retirement, std::thread::current().id())
        });
        let (mut retirement, _) = handle.join().unwrap();
        assert!(records.lock().unwrap().is_empty());
        if errors_first {
            drop(errors.lock().unwrap().take());
            assert!(!retirement.advance().unwrap());
            drop(data.lock().unwrap().take());
        } else {
            drop(data.lock().unwrap().take());
            assert!(!retirement.advance().unwrap());
            drop(errors.lock().unwrap().take());
        }
        assert!(records.lock().unwrap().is_empty()); // backend only published Boxes
        let (mut retirement, cleanup_thread) = std::thread::spawn(move || {
            let _ = retirement.advance().unwrap();
            (retirement, std::thread::current().id())
        })
        .join()
        .unwrap();
        assert_ne!(cleanup_thread, ui_thread);
        assert_eq!(
            *records.lock().unwrap(),
            [
                (data_address, cleanup_thread),
                (error_address, cleanup_thread),
                (runtime_address, cleanup_thread)
            ]
        );
        assert!(control.recover_audio_candidate(&ticket).is_none());
        assert_eq!(control.audio_candidate_custody().occupied_slots, 1); // real external lease is Busy
        drop(external_lease);
        let ready = candidate_tests::recover(&mut control, &ticket);
        assert_eq!(ready.disposed.identity(), identity);
        assert_eq!(
            ready.return_kind,
            crate::AudioCandidateReturnKind::FinalActive
        );
        assert!(ready.disposed.closed_owner_groups() > 0);
        assert_eq!(control.audio_candidate_custody().occupied_slots, 0);
        assert!(control.recover_audio_candidate(&ticket).is_none());
        let next = candidate_tests::candidate().unwrap();
        let next_identity = next.identity();
        let refusal = control.stage_audio_candidate(next, vec![]).err().unwrap();
        assert_eq!(refusal.owner.identity(), next_identity); // final terminal root never reopens
        drop(refusal);
        drop(ready);
        drop(control);
        candidate_tests::finish(&mut retirement);
    }
}
#[test]
fn exposed_missing_handle_cannot_return_an_actual_active_owner_even_after_both_guards_release() {
    let _serial = candidate_tests::serial();
    let Some(owner) = candidate_tests::candidate() else {
        return;
    };
    let (mut control, mut data, errors, mut retirement) = candidate_tests::anchored();
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.test_admit_candidate = true);
    let ticket = control.stage_audio_candidate(owner, vec![]).ok().unwrap();
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    drop(data);
    drop(errors);
    let mut stream: Option<Backend> = None;
    assert!(
        crate::lifecycle::test_dispose_backend(
            &mut stream,
            &mut false,
            &mut false,
            &mut retirement,
            |_| Ok(())
        )
        .is_err()
    );
    assert!(!retirement.advance().unwrap());
    assert!(retirement.test_extracted_identity().is_none());
    assert!(control.recover_audio_candidate(&ticket).is_none());
    assert_eq!(control.audio_candidate_custody().occupied_slots, 1);
    // This explicit manual NativeRetirement driver owns terminal cleanup;
    // prevent a live-job announcement during negative-fixture teardown.
    retirement.root().announce_retirement();
    // Explicit control fixture teardown after asserting pinned failure. No
    // production retirement retry, READY result or disposal receipt exists.
}
#[test]
fn carrier_destructor_panic_after_actual_extraction_keeps_original_owner_and_never_publishes_ready()
{
    let _serial = candidate_tests::serial();
    let Some(owner) = candidate_tests::candidate() else {
        return;
    };
    let identity = owner.identity();
    let (mut control, mut data, mut errors, mut retirement) = candidate_tests::anchored();
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.test_admit_candidate = true);
    let ticket = control.stage_audio_candidate(owner, vec![]).ok().unwrap();
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    struct Payload(Arc<AtomicU64>);
    impl Drop for Payload {
        fn drop(&mut self) {
            self.0.fetch_add(1, SeqCst);
        }
    }
    let payload_drops = Arc::new(AtomicU64::new(0));
    let payload_counter = payload_drops.clone();
    let calls = Arc::new(AtomicU64::new(0));
    let counter = calls.clone();
    errors.owner_mut().unwrap().test_drop_probe(move || {
        counter.fetch_add(1, SeqCst);
        std::panic::panic_any(Payload(payload_counter.clone()));
    });
    let data = Arc::new(Mutex::new(Some(data)));
    let errors = Arc::new(Mutex::new(Some(errors)));
    let mut stream = Some(Backend {
        _data: data.clone(),
        _errors: errors.clone(),
        drops: Arc::new(AtomicU64::new(0)),
    });
    drop(data.lock().unwrap().take());
    drop(errors.lock().unwrap().take());
    let first = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::lifecycle::test_dispose_backend(
            &mut stream,
            &mut false,
            &mut false,
            &mut retirement,
            |_| Ok(()),
        )
    }))
    .err()
    .unwrap();
    let payload = first.as_ref() as *const dyn std::any::Any as *const ();
    assert_eq!(retirement.test_extracted_identity(), Some(identity));
    assert!(retirement.advance().is_err());
    assert_eq!(calls.load(SeqCst), 1);
    assert_eq!(payload_drops.load(SeqCst), 0);
    assert_eq!(
        first.as_ref() as *const dyn std::any::Any as *const (),
        payload
    );
    assert!(control.recover_audio_candidate(&ticket).is_none());
    assert_eq!(control.audio_candidate_custody().occupied_slots, 1);
    // First payload and actual owner remain in persistent test custody through
    // the assertions; production Retirement retains both indefinitely on failure.
    retirement.root().announce_retirement();
    drop(retirement);
    drop(first);
    assert_eq!(payload_drops.load(SeqCst), 1);
    drop(control);
}

#[test]
fn control_first_abandonment_finishes_actual_active_owner_after_busy_dependency_without_ui_poll() {
    let _serial = candidate_tests::serial();
    let Some(owner) = candidate_tests::candidate() else {
        return;
    };
    let (mut control, mut data, errors, mut retirement) = candidate_tests::anchored();
    let domain = candidate_tests::pair();
    let external = domain.0.catalog_lease().unwrap();
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.test_admit_candidate = true);
    let _ticket = control
        .stage_audio_candidate(owner, vec![domain])
        .ok()
        .unwrap();
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    let root = retirement.root();
    // This manual headless NativeRetirement driver is the actual guaranteed
    // terminal owner, so announcement precedes abandoning the ordinary Control.
    root.announce_retirement();
    drop(control);
    let drops = Arc::new(AtomicU64::new(0));
    let counted = drops.clone();
    let data = Arc::new(Mutex::new(Some(data)));
    let errors = Arc::new(Mutex::new(Some(errors)));
    let mut stream = Some(Backend {
        _data: data,
        _errors: errors,
        drops,
    });
    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
    let (done_tx, done_rx) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        let mut pending = false;
        let mut disposed = false;
        assert!(
            !crate::lifecycle::test_dispose_backend(
                &mut stream,
                &mut pending,
                &mut disposed,
                &mut retirement,
                |_| Ok(())
            )
            .unwrap()
        );
        assert!(disposed);
        entered_tx.send(()).unwrap();
        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        while !crate::lifecycle::test_dispose_backend(
            &mut stream,
            &mut pending,
            &mut disposed,
            &mut retirement,
            |_| Ok(()),
        )
        .unwrap()
        {
            assert!(
                Instant::now() < deadline,
                "active abandonment remained retained"
            );
            std::thread::yield_now();
        }
        done_tx.send(std::thread::current().id()).unwrap();
    });
    entered_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    assert_eq!(root.report().occupied_slots, 1);
    assert_eq!(counted.load(SeqCst), 1);
    assert!(done_rx.try_recv().is_err());
    drop(external);
    // No UI reclaim/recover/advance call: the real control-side driver advances
    // all physical phases and original-bank destruction after dependency release.
    let destructor_thread = done_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    assert_ne!(destructor_thread, std::thread::current().id());
    worker.join().unwrap();
    assert_eq!(root.report().occupied_slots, 0);
}

#[test]
fn foreign_terminal_root_refuses_without_taking_the_actual_active_constructor_owner() {
    let _serial = candidate_tests::serial();
    let Some(owner) = candidate_tests::candidate() else {
        return;
    };
    let (mut control, callback) = channel(48_000).unwrap();
    let (mut data, errors, terminals) =
        crate::native_callback_owner::reserve_native(callback, control.status.clone());
    control.enable_native_custody().unwrap();
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.test_admit_candidate = true);
    let ticket = control.stage_audio_candidate(owner, vec![]).ok().unwrap();
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    let foreign = crate::candidate_custody::CandidateRoot::new(RETIRED).unwrap();
    let mut retirement = crate::native_callback_owner::NativeRetirement::new(terminals, foreign);
    drop(data);
    drop(errors);
    assert!(retirement.advance().is_err());
    assert!(retirement.test_extracted_identity().is_none());
    assert!(control.recover_audio_candidate(&ticket).is_none());
    assert_eq!(control.audio_candidate_custody().occupied_slots, 1);
    // Only explicit controlled negative-fixture teardown follows; no receipt or
    // successful cleanup of the original mismatched root is manufactured.
    control.native_candidate_root().announce_retirement();
}
