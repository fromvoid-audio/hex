//! Accepted manual VST3 input retains the exact aligned Arrangement origin.
use super::*;
use crate::plugin_worker::NativeParameterHold;
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_plugin_host::transport::ManualParameterOrigin;

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_manual_parameter_waits_for_final_main_delivery_across_compensated_cuts() {
    verify_delivery(true);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_plain_parameter_reserves_delivery_before_input_and_keeps_busy_refusal_atomic() {
    verify_delivery(false);
}

fn verify_delivery(held: bool) {
    const CHILD: &str = "HEX_ORDINARY_MANUAL_ORIGIN_DRIVER";
    if std::env::var_os(CHILD).is_none() {
        let name = std::thread::current().name().unwrap().to_owned();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--ignored", "--test-threads=1"])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "144")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(String::from_utf8_lossy(&result.stdout).contains("1 passed"));
        return;
    }

    let _serial = serial();
    for location in 0..3 {
        verify_location(location, held);
    }
}

fn verify_location(location: usize, held: bool) {
    let mut f = fixture_with_authored("processing", location, 8, &[7]);
    let upstream = &hex_devices::overdrive::OVERDRIVE_DEFINITION;
    let state = upstream
        .patch_parameter(
            &upstream.default_state().unwrap(),
            hex_devices::overdrive::parameter::DRY_WET,
            ParameterValue::Float(0.0),
        )
        .unwrap();
    let device = f
        .editor
        .project()
        .automation_devices()
        .find(|device| device.id().value() == f.binding.instance)
        .unwrap()
        .id();
    let insert = |editor: &mut Editor, before, state| {
        let command = match location {
            0 => Command::InsertDevice {
                track: editor.project().tracks()[0].id(),
                before,
                state,
            },
            1 => Command::InsertReturnDevice {
                return_track: editor.project().return_tracks()[0].id(),
                before,
                state,
            },
            _ => Command::InsertMainDevice { before, state },
        };
        editor.execute(command).unwrap();
    };
    insert(&mut f.editor, Some(device), state.clone());
    insert(&mut f.editor, None, state);
    let playback = compile(&f, true);
    assert_eq!(playback.main_output_latency_frames(), 11 + 16 + 11);
    publish(&mut f.control, playback, PublicationPolicy::InheritRuntime);
    let root = f.control.native_candidate_root();
    f.control.command(PlaybackCommand::Play).unwrap();
    assert_eq!(physical_block::<5>(&mut f), [[0.0; 2]; 5]);
    assert_eq!(physical_block::<3>(&mut f), [[0.0; 2]; 3]);
    replacement::wait_for_blocks(&root, f.binding, 1);
    assert_eq!(physical_block::<7>(&mut f), [[0.0; 2]; 7]);
    assert_eq!(physical_block::<1>(&mut f), [[0.0; 2]; 1]);
    replacement::wait_for_blocks(&root, f.binding, 2);

    let expected = f.control.status.native_effect_source().unwrap();
    let until = Instant::now() + Duration::from_secs(3);
    let id = if held { 7 } else { 11 };
    let (hold, target) = loop {
        let result = if held {
            f.control
                .native_source_parameter_hold_with_delivery_on_control(
                    expected,
                    f.binding,
                    NativeParameterHold::Begin {
                        id,
                        normalized: 0.75,
                    },
                )
        } else {
            f.control
                .begin_native_source_parameter_change_with_delivery_on_control(
                    expected, f.binding, id, 0.75,
                )
        };
        match result {
            Ok(pair) => break pair,
            Err("native preparation is busy" | "native parameter mailbox is busy") => {}
            Err(error) => panic!("observed manual input: {error}"),
        }
        assert!(Instant::now() < until);
        std::thread::yield_now();
    };
    physical_block::<5>(&mut f);
    physical_block::<3>(&mut f);
    replacement::wait_for_blocks(&root, f.binding, 3);
    processed(&mut f, &hold);
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        let refused = if held {
            f.control
                .native_source_parameter_hold_with_delivery_on_control(
                    expected,
                    f.binding,
                    NativeParameterHold::Change {
                        hold: &hold,
                        normalized: 0.5,
                    },
                )
        } else {
            f.control
                .begin_native_source_parameter_change_with_delivery_on_control(
                    expected, f.binding, id, 0.5,
                )
        };
        match refused {
            Err("drain the previous vst3 delivery witness first") => break,
            Err("native preparation is busy" | "native parameter mailbox is busy") => {}
            _ => panic!("busy witness must refuse before another input is admitted"),
        }
        assert!(Instant::now() < until);
    }
    assert!(
        f.control
            .try_native_source_parameter_delivery_on_control()
            .is_none()
    );
    physical_block::<6>(&mut f);
    assert_eq!(physical_block::<5>(&mut f), [[0.0; 2]; 5]);
    replacement::wait_for_blocks(&root, f.binding, 4);
    assert!(
        f.control
            .try_native_source_parameter_delivery_on_control()
            .is_none()
    );
    let leading = physical_block::<5>(&mut f);
    assert_eq!(&leading[..3], &[[0.0; 2]; 3]);
    let dry = if location == 1 { 0.25 } else { 0.0 };
    let base = [0.25 + dry, -0.5 - 2.0 * dry];
    let manual = [0.75 + dry, -1.5 - 2.0 * dry];
    assert_eq!(&leading[3..], &[base; 2]);
    replacement::wait_for_blocks(&root, f.binding, 5);
    assert_eq!(physical_block::<3>(&mut f), [base; 3]);
    assert!(
        f.control
            .try_native_source_parameter_delivery_on_control()
            .is_none()
    );
    assert_eq!(physical_block::<1>(&mut f), [manual; 1]);
    assert_eq!(
        f.control.try_native_source_parameter_delivery_on_control(),
        Some(crate::foreign_parameter_delivery::Receipt::Delivered {
            target,
            normalized: 0.75,
            origin: ManualParameterOrigin {
                arrangement_frame: 5,
                performance_frame: 5,
                loop_pass: 0,
            },
            delivery_performance_frame: 43,
        })
    );
    assert!(
        f.control
            .try_native_source_parameter_delivery_on_control()
            .is_none()
    );
    assert_eq!(hold.delivery_identity().unwrap().1, target.operation);
    assert_eq!(target.parameter, id);
    let until = Instant::now() + Duration::from_secs(3);
    let (update, next) = loop {
        let result = if held {
            f.control
                .native_source_parameter_hold_with_delivery_on_control(
                    expected,
                    f.binding,
                    NativeParameterHold::Change {
                        hold: &hold,
                        normalized: 0.5,
                    },
                )
        } else {
            f.control
                .begin_native_source_parameter_change_with_delivery_on_control(
                    expected, f.binding, id, 0.5,
                )
        };
        match result {
            Ok(pair) => break pair,
            Err("native preparation is busy" | "native parameter mailbox is busy") => {}
            Err(error) => panic!("observed update: {error}"),
        }
        assert!(Instant::now() < until);
    };
    assert_eq!(next.operation, target.operation + 1);
    assert_eq!(update.delivery_identity().unwrap().1, next.operation);
    let mut cursor = 44;
    let mut native_blocks = 5;
    let mut delivered = false;
    for _ in 0..8 {
        let start = cursor;
        let output = physical_block::<8>(&mut f);
        cursor += 8;
        native_blocks += 1;
        replacement::wait_for_blocks(&root, f.binding, native_blocks);
        if let Some(receipt) = f.control.try_native_source_parameter_delivery_on_control() {
            let crate::foreign_parameter_delivery::Receipt::Delivered {
                target,
                normalized,
                origin,
                delivery_performance_frame,
            } = receipt
            else {
                panic!("actual update must reach final Main: {receipt:?}")
            };
            assert_eq!(target, next);
            assert_eq!(normalized, 0.5);
            assert_eq!(origin.arrangement_frame, origin.performance_frame);
            assert_eq!(delivery_performance_frame, origin.performance_frame + 38);
            let offset = usize::try_from(delivery_performance_frame - start).unwrap();
            assert!(offset < output.len());
            assert_eq!(output[offset], [0.5 + dry, -1.0 - 2.0 * dry]);
            delivered = true;
            break;
        }
    }
    assert!(
        delivered,
        "actual update must produce its bounded output witness"
    );
    processed(&mut f, &update);
    if held {
        let until = Instant::now() + Duration::from_secs(3);
        let (release, end) = loop {
            match f
                .control
                .native_source_parameter_hold_with_delivery_on_control(
                    expected,
                    f.binding,
                    NativeParameterHold::Release { hold: &hold },
                ) {
                Ok(pair) => break pair,
                Err("native preparation is busy" | "native parameter mailbox is busy") => {}
                Err(error) => panic!("observed release: {error}"),
            }
            assert!(Instant::now() < until);
        };
        assert_eq!(end.operation, next.operation + 1);
        assert_eq!(release.delivery_identity().unwrap().1, end.operation);
        let mut released = false;
        for _ in 0..8 {
            let start = cursor;
            let output = physical_block::<8>(&mut f);
            cursor += 8;
            native_blocks += 1;
            replacement::wait_for_blocks(&root, f.binding, native_blocks);
            if let Some(receipt) = f.control.try_native_source_parameter_delivery_on_control() {
                let crate::foreign_parameter_delivery::Receipt::Released {
                    target,
                    origin,
                    delivery_performance_frame,
                } = receipt
                else {
                    panic!("actual release must reach final Main: {receipt:?}")
                };
                assert_eq!(target, end);
                assert_eq!(origin.arrangement_frame, origin.performance_frame);
                assert_eq!(delivery_performance_frame, origin.performance_frame + 38);
                let offset = usize::try_from(delivery_performance_frame - start).unwrap();
                assert!(offset < output.len());
                assert!(
                    output[..offset]
                        .iter()
                        .all(|frame| *frame == [0.5 + dry, -1.0 - 2.0 * dry])
                );
                assert!(
                    output[offset..].iter().all(|frame| *frame == base),
                    "authored PCM resumes exactly at the exclusive end"
                );
                let until = Instant::now() + Duration::from_secs(3);
                loop {
                    match f
                        .control
                        .poll_native_source_parameter_change_on_control(f.binding, &release)
                    {
                        Ok(crate::plugin_worker::NativeWorkerParameterProgress::Released {
                            sequence,
                            first_frame,
                        }) => {
                            assert_eq!(first_frame, sequence * 8);
                            assert_eq!(first_frame, origin.performance_frame + 11);
                            break;
                        }
                        Ok(crate::plugin_worker::NativeWorkerParameterProgress::Pending)
                        | Err("native preparation is busy") => {}
                        result => panic!("actual SDK release terminal: {result:?}"),
                    }
                    assert!(Instant::now() < until);
                }
                released = true;
                break;
            }
            assert!(
                output
                    .iter()
                    .all(|frame| *frame == [0.5 + dry, -1.0 - 2.0 * dry])
            );
        }
        assert!(
            released,
            "actual release must produce its bounded end witness"
        );
    }
    dispose(f);
}

fn processed(f: &mut OrdinaryFixture, request: &crate::plugin_worker::NativeWorkerParameterChange) {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        match f
            .control
            .poll_native_source_parameter_change_on_control(f.binding, request)
        {
            Ok(crate::plugin_worker::NativeWorkerParameterProgress::Processed(_)) => return,
            Ok(crate::plugin_worker::NativeWorkerParameterProgress::Pending)
            | Err("native preparation is busy") => {}
            result => panic!("actual SDK point terminal: {result:?}"),
        }
        assert!(Instant::now() < until);
        std::thread::yield_now();
    }
}
