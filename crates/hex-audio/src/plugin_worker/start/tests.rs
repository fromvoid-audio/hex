//! Real SDK source with deterministic failure at the std thread-spawn seam.
use super::*;
use std::time::Instant;

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn native_start_refusal_retains_original_source_and_any_queue_pair_until_observed_disposal() {
    let binding = Binding {
        root: 31,
        instance: 2,
        graph: 12,
        epoch: 1,
    };
    for refuse_spawn in [false, true] {
        let instance = super::super::native_tests::original_instance_for_binding(binding);
        let child = instance.child_id_on_control().unwrap();
        if !refuse_spawn {
            instance.cancellation_on_control().cancel_on_control();
        }
        let called = std::cell::Cell::new(false);
        let result = NativeWorker::start_retained_with(instance, |guard, _| {
            called.set(true);
            drop(guard);
            Err(std::io::Error::other("original thread creation refusal"))
        });
        let mut failure = match result {
            Err(failure) => failure,
            Ok(_) => panic!("native startup must refuse"),
        };
        assert_eq!(called.get(), refuse_spawn);
        assert_eq!(failure.binding(), binding);
        if refuse_spawn {
            assert!(matches!(
                failure.cause(),
                Error::IoAt("native transport worker", _)
            ));
            let Custody::Pair {
                reaper,
                _callback: callback,
                native,
            } = &mut failure.custody
            else {
                panic!("actual failed pair");
            };
            assert!(native.is_none());
            assert!(callback.matches_control(&reaper.control));
            assert!(matches!(
                callback.stage(&[[0.25; 2]; 8], &[], &mut [[0.0; 2]; 8]),
                Err(Fault::Revoked)
            ));
            let _ = reaper.join_on_control();
            let state = reaper.worker.as_ref().unwrap();
            let Some(Processor::Native(actual)) = state.processor.as_ref() else {
                panic!("actual retained instance");
            };
            assert_eq!(actual.child_id_on_control(), Some(child));
            assert_eq!(actual.configuration().binding, binding);
            assert_eq!(actual.completed_blocks_on_control(), 0);
        } else {
            assert!(matches!(failure.cause(), Error::Cancelled));
            assert!(matches!(failure.custody, Custody::Source(_)));
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        let disposal = loop {
            if let Some(disposal) = failure.poll_disposal_on_control().unwrap() {
                break disposal;
            }
            assert!(
                Instant::now() < deadline,
                "actual native construction disposal"
            );
            std::thread::yield_now();
        };
        assert!(matches!(disposal, PreparationDisposal::Reaped(_)));
        if let Custody::Pair {
            reaper,
            _callback: callback,
            native,
        } = &failure.custody
        {
            assert!(callback.matches_control(&reaper.control));
            assert!(native.is_some());
            assert!(reaper.worker.as_ref().unwrap().processor.is_none());
        }
        let original = failure.take_cause_on_control();
        if refuse_spawn {
            assert!(matches!(
                original,
                Error::IoAt("native transport worker", _)
            ));
        } else {
            assert!(matches!(original, Error::Cancelled));
        }
        // Taking the cause did not extract or destroy either endpoint.
        if let Custody::Pair {
            reaper,
            _callback: callback,
            ..
        } = &failure.custody
        {
            assert!(callback.matches_control(&reaper.control));
        }
        // Receipt did not destroy queues; final enclosing control disposal does.
        drop(failure);
    }
}
