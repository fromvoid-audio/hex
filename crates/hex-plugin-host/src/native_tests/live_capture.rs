//! Original SDK main-lane state capture while the same child keeps processing.
use super::*;

fn poll_complete(instance: &mut Prepared) -> NativeDeviceState {
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(state) = instance.poll_state_capture_on_worker().unwrap() {
            return state;
        }
        assert!(Instant::now() < until, "actual SDK capture completion");
        thread::sleep(Duration::from_millis(1));
    }
}

fn wait_held(case: &Case) {
    let until = Instant::now() + Duration::from_secs(2);
    while case.observation().is_none_or(|values| values[0] != 22) {
        assert!(
            Instant::now() < until,
            "actual SDK capture did not enter its hold"
        );
        thread::sleep(Duration::from_millis(1));
    }
}

fn exchange(instance: &mut Prepared, n: u64) {
    let block = request(instance.configuration(), n);
    #[cfg(feature = "allocation-probe")]
    let output = {
        let mut result = None;
        let measured = allocation_counter::measure(|| {
            result = Some(instance.exchange_on_worker(block));
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        result.unwrap().unwrap()
    };
    #[cfg(not(feature = "allocation-probe"))]
    let output = instance.exchange_on_worker(block).unwrap();
    assert_eq!(
        (output.binding, output.sequence, output.first_frame),
        (block.binding, n, n * 8)
    );
    for (actual, input) in output.pcm[..8].iter().zip(block.pcm) {
        assert_eq!(
            *actual,
            [
                input[0] * (0.5 + n as f32 * 0.125),
                input[1] * (0.5 + n as f32 * 0.125)
            ]
        );
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_live_capture_keeps_same_processing_instance_and_historical_startup() {
    let case = Case::new();
    let mut instance = Prepared::prepare(
        &build(),
        source(),
        options(),
        PreparationCancellation::new_on_control().unwrap(),
        case.setup(0),
    )
    .unwrap();
    let child = instance.child_id_on_control();
    let initial = instance.ready().initial_state.clone();
    assert!(instance.poll_state_capture_on_worker().is_err());
    exchange(&mut instance, 0);
    exchange(&mut instance, 1);
    instance.begin_state_capture_on_worker().unwrap();
    assert!(
        instance.begin_state_capture_on_worker().is_err(),
        "overlap preserves admitted operation"
    );
    assert!(
        instance.stop_and_reap_on_control().is_err(),
        "capture streams still belong to main"
    );
    exchange(&mut instance, 2);
    let captured = poll_complete(&mut instance);
    let state = decode(&captured).unwrap();
    let count = u32::from_le_bytes(state.component.unwrap()[4..8].try_into().unwrap());
    assert!(
        (2..=3).contains(&count),
        "state was captured during concurrent PCM, not at an invented sample boundary"
    );
    assert_ne!(captured, initial);
    assert_eq!(instance.ready().initial_state, initial);
    assert_eq!(instance.child_id_on_control(), child);
    instance.begin_state_capture_on_worker().unwrap();
    let repeated = poll_complete(&mut instance);
    assert_eq!(
        u32::from_le_bytes(
            decode(&repeated).unwrap().component.unwrap()[4..8]
                .try_into()
                .unwrap()
        ),
        3
    );
    assert!(instance.poll_state_capture_on_worker().is_err());
    let previous = instance.configuration().binding;
    let mut instance = instance
        .quiesce_retained_on_control()
        .unwrap()
        .resume_on_control(
            Binding {
                epoch: previous.epoch + 1,
                ..previous
            },
            Some(options().presentation),
        )
        .unwrap();
    instance.begin_state_capture_on_worker().unwrap();
    poll_complete(&mut instance);
    exchange(&mut instance, 3);
    let retired = instance.stop_and_reap_on_control().unwrap();
    assert!(retired.status.success());
    assert_eq!(retired.terminal.unwrap().blocks, 4);
    let observed = case.observation().unwrap();
    assert_eq!((observed[1], observed[8], observed[9]), (0, 2, 2));
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_live_controller_capture_restores_into_an_independent_processing_instance() {
    let case = Case::new();
    let selected = source();
    let saved = startup_state(&selected, true, 3);
    let mut instance = Pending::prepare_with_restore(
        &build(),
        selected,
        options().preparation,
        Some(saved.clone()),
        PreparationCancellation::new_on_control().unwrap(),
        case.setup_restore(37),
    )
    .unwrap()
    .complete_on_control(Some(options().presentation))
    .unwrap();
    let block = request(instance.configuration(), 0);
    instance.exchange_on_worker(block).unwrap();
    let child = instance.child_id_on_control();
    instance.begin_state_capture_on_worker().unwrap();
    let block = request(instance.configuration(), 1);
    instance.exchange_on_worker(block).unwrap();
    let captured = poll_complete(&mut instance);
    assert_eq!(
        &decode(&captured).unwrap().component.unwrap()[8..12],
        &0.75_f32.to_le_bytes()
    );
    assert_eq!(
        &decode(&captured).unwrap().controller.unwrap()[4..8],
        &91_u32.to_le_bytes()
    );
    let observed = case.restore_observation();
    assert_eq!(
        &observed[..3],
        &[1, 1, 1],
        "live capture never restores/synchronizes again"
    );
    assert_eq!(&observed[10..13], &[2, 2, 1]);
    let independent = Case::new();
    let mut restored = Pending::prepare_with_restore(
        &build(),
        source(),
        options().preparation,
        Some(captured),
        PreparationCancellation::new_on_control().unwrap(),
        independent.setup_restore(37),
    )
    .unwrap()
    .complete_on_control(Some(options().presentation))
    .unwrap();
    assert_ne!(
        instance.child_id_on_control(),
        restored.child_id_on_control()
    );
    assert_eq!(instance.child_id_on_control(), child);
    assert_eq!(instance.configuration().restore, Some(saved));
    let input = request(restored.configuration(), 0);
    let actual = restored.exchange_on_worker(input).unwrap();
    assert_eq!(&actual.pcm[..3], &[[0.0; 2]; 3]);
    for (index, frame) in actual.pcm[..8].iter().enumerate().skip(3) {
        assert_eq!(*frame, input.pcm[index - 3].map(|value| value * 0.75 * 0.5));
    }
    assert_eq!(independent.restore_observation()[7], 91);
    assert!(
        restored
            .stop_and_reap_on_control()
            .unwrap()
            .status
            .success()
    );
    assert!(
        instance
            .stop_and_reap_on_control()
            .unwrap()
            .status
            .success()
    );
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_live_held_sdk_capture_does_not_wait_or_stop_pcm() {
    let case = Case::new();
    let mut instance = Prepared::prepare(
        &build(),
        source(),
        options(),
        PreparationCancellation::new_on_control().unwrap(),
        case.setup(71),
    )
    .unwrap();
    let child = instance.child_id_on_control();
    let initial = instance.ready().initial_state.clone();
    instance.begin_state_capture_on_worker().unwrap();
    wait_held(&case);
    for n in 0..4 {
        assert_eq!(instance.poll_state_capture_on_worker().unwrap(), None);
        exchange(&mut instance, n);
    }
    assert_eq!(instance.child_id_on_control(), child);
    assert_eq!(instance.completed_blocks_on_control(), 4);
    assert_eq!(
        case.observation().unwrap()[9],
        0,
        "SDK processing never stopped"
    );
    fs::write(case.release(), b"release actual SDK capture").unwrap();
    let captured = poll_complete(&mut instance);
    assert_eq!(
        decode(&captured).unwrap().controller,
        decode(&initial).unwrap().controller
    );
    assert_eq!(instance.ready().initial_state, initial);
    exchange(&mut instance, 4);
    let retired = instance.stop_and_reap_on_control().unwrap();
    assert!(retired.status.success());
    assert_eq!(retired.terminal.unwrap().blocks, 5);
    assert_eq!(case.observation().unwrap()[1], 0);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_live_capture_refusal_exception_cancel_and_deadline_keep_confirmed_state() {
    for fault in [69, 70, 71] {
        let case = Case::new();
        let context = PreparationCancellation::new_on_control().unwrap();
        let token = context.token_on_control();
        let mut instance =
            Prepared::prepare(&build(), source(), options(), context, case.setup(fault)).unwrap();
        let initial = instance.ready().initial_state.clone();
        instance.begin_state_capture_on_worker().unwrap();
        if fault == 71 {
            wait_held(&case);
            token.cancel_on_control();
        }
        let until = Instant::now() + Duration::from_secs(2);
        let error = loop {
            match instance.poll_state_capture_on_worker() {
                Err(error) => break error,
                Ok(None) => {
                    assert!(Instant::now() < until);
                    thread::sleep(Duration::from_millis(1));
                }
                Ok(Some(_)) => panic!("refused capture returned accepted state"),
            }
        };
        assert!(
            match fault {
                69 => matches!(error, Error::Worker(6)),
                70 => matches!(error, Error::Worker(12)),
                _ => matches!(error, Error::Cancelled),
            },
            "{fault}: {error:?}"
        );
        assert_eq!(instance.ready().initial_state, initial);
        let retired = instance.cancel_and_reap_on_control().unwrap();
        assert!(!retired.status.success() && retired.terminal.is_none());
        assert!(instance.child_id_on_control().is_none());
    }
    let case = Case::new();
    let mut instance = Prepared::prepare(
        &build(),
        source(),
        options(),
        PreparationCancellation::new_on_control().unwrap(),
        case.setup(71),
    )
    .unwrap();
    let initial = instance.ready().initial_state.clone();
    instance.begin_state_capture_on_worker().unwrap();
    wait_held(&case);
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let block = request(
            instance.configuration(),
            instance.completed_blocks_on_control(),
        );
        match instance.exchange_on_worker(block) {
            Ok(_) => {
                assert!(Instant::now() < until);
                thread::sleep(Duration::from_millis(1));
            }
            Err(error) => {
                assert!(matches!(error, Error::Timeout));
                break;
            }
        }
    }
    assert_eq!(instance.ready().initial_state, initial);
    assert_eq!(
        instance.cancel_and_reap_on_control().unwrap().status.code(),
        Some(9)
    );
}

fn overflow_capture_recovers(variant: u32) {
    let case = Case::new();
    let mut instance = Prepared::prepare(
        &build(),
        source(),
        options(),
        PreparationCancellation::new_on_control().unwrap(),
        case.setup(variant),
    )
    .unwrap();
    let child = instance.child_id_on_control();
    exchange(&mut instance, 0);
    instance.begin_state_capture_on_worker().unwrap();
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        match instance.poll_state_capture_on_worker() {
            Err(crate::Error::CaptureRefused(
                crate::protocol::StateCaptureRefusal::ExceedsLimit { observed_bytes },
            )) => {
                assert_eq!(observed_bytes, if variant == 95 { 5000 } else { 5032 });
                break;
            }
            Ok(None) => {
                assert!(Instant::now() < until);
                thread::sleep(Duration::from_millis(1));
            }
            other => panic!("expected overflow refusal, got {other:?}"),
        }
    }
    assert_eq!(instance.child_id_on_control(), child);
    exchange(&mut instance, 1);
    instance.begin_state_capture_on_worker().unwrap();
    assert!(
        decode(&poll_complete(&mut instance))
            .unwrap()
            .component
            .is_some()
    );
    exchange(&mut instance, 2);
    assert!(
        instance
            .stop_and_reap_on_control()
            .unwrap()
            .status
            .success()
    );
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_component_overflow_live_capture_keeps_instance() {
    overflow_capture_recovers(95);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_controller_overflow_live_capture_keeps_instance() {
    overflow_capture_recovers(96);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_overflow_with_invalid_stream_use_is_still_terminal_when_live() {
    let case = Case::new();
    let mut instance = Prepared::prepare(
        &build(),
        source(),
        options(),
        PreparationCancellation::new_on_control().unwrap(),
        case.setup(97),
    )
    .unwrap();
    instance.begin_state_capture_on_worker().unwrap();
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        match instance.poll_state_capture_on_worker() {
            Err(Error::Worker(6)) => break,
            Ok(None) => {
                assert!(Instant::now() < until);
                thread::sleep(Duration::from_millis(1));
            }
            other => panic!("expected terminal capture error, got {other:?}"),
        }
    }
    let retired = instance.cancel_and_reap_on_control().unwrap();
    assert!(!retired.status.success() && retired.terminal.is_none());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_overflow_with_unrelated_sdk_error_is_still_terminal_when_live() {
    let case = Case::new();
    let mut instance = Prepared::prepare(
        &build(),
        source(),
        options(),
        PreparationCancellation::new_on_control().unwrap(),
        case.setup(98),
    )
    .unwrap();
    instance.begin_state_capture_on_worker().unwrap();
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        match instance.poll_state_capture_on_worker() {
            Err(Error::Worker(6)) => break,
            Ok(None) => {
                assert!(Instant::now() < until);
                thread::sleep(Duration::from_millis(1));
            }
            other => panic!("expected terminal capture error, got {other:?}"),
        }
    }
    let retired = instance.cancel_and_reap_on_control().unwrap();
    assert!(!retired.status.success() && retired.terminal.is_none());
}
