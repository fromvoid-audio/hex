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
fn native_late_finished_survives_close_and_both_guard_release_orders_once() {
    let _serial = crate::prepared::candidate_tests::serial();
    for errors_first in [false, true] {
        let (control, callback, target) =
            crate::device_native_parameter_input_tests::fixture(false);
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
                crate::device_native_parameter_input_tests::value(-6.0),
            )
            .unwrap();
        let mut output = [[0.0; 2]; 16];
        counted(|| {
            data.owner_mut().unwrap().run(|c, _| {
                c.begin_block();
                c.render(&mut output).unwrap();
                c.finish_gain_recording_quantum(true);
            })
        });
        client.native_parameter_release().unwrap();
        counted(|| {
            data.owner_mut().unwrap().run(|c, _| {
                c.begin_block();
                c.render(&mut output).unwrap();
            })
        });
        let mut token = counted(|| client.close_retaining_native_parameter_receipts()).unwrap();
        assert_eq!(token.output_identity(), target.source.output_identity);
        assert!(client.close_retaining_native_parameter_receipts().is_none());
        publication.close();
        assert!(!retirement.advance().unwrap());
        assert!(token.try_take().is_none());
        // The sink completes the already rendered final quantum after close.
        counted(|| {
            data.owner_mut()
                .unwrap()
                .run(|c, _| c.finish_gain_recording_quantum(true))
        });
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
        assert_eq!(batch.terminal(), ScalarReceiptTerminal::Finished);
        let receipts = batch.receipts().collect::<Vec<_>>();
        assert!(
            matches!(receipts.as_slice(), [ScalarReceipt::NativeParameter(crate::native_parameter_input::Receipt::Applied { target: applied, .. }), ScalarReceipt::NativeParameter(crate::native_parameter_input::Receipt::Finished { target: finished, .. })] if *applied == target && *finished == target)
        );
        assert!(token.try_take().is_none());
        crate::prepared::candidate_tests::finish(&mut retirement);
    }
}
#[test]
fn missing_actual_terminal_is_interrupted_and_failure_publication_is_once_only() {
    let _serial = crate::prepared::candidate_tests::serial();
    let (control, callback, target) = crate::device_native_parameter_input_tests::fixture(false);
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
            crate::device_native_parameter_input_tests::value(-6.0),
        )
        .unwrap();
    let mut token = client.close_retaining_native_parameter_receipts().unwrap();
    publication.close();
    drop(data);
    drop(errors);
    assert!(dispose(&mut retirement));
    retirement.fail_scalar_receipts();
    let batch = token.try_take().unwrap().unwrap();
    assert_eq!(batch.terminal(), ScalarReceiptTerminal::Interrupted);
    assert_eq!(batch.receipts().count(), 0);
    crate::prepared::candidate_tests::finish(&mut retirement);
}
