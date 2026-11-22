//! The first observed input waits for a real compensated Arrangement origin.
use super::*;
use crate::plugin_worker::{NativeParameterHold, NativeWorkerParameterProgress};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn first_observed_point_and_hold_wait_through_compensated_preroll() {
    const CHILD: &str = "HEX_ORDINARY_MANUAL_PREROLL_DRIVER";
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
    for held in [true, false] {
        let mut f = fixture_with_authored("processing", 2, 8, &[7]);
        let definition = &hex_devices::overdrive::OVERDRIVE_DEFINITION;
        let state = definition
            .patch_parameter(
                &definition.default_state().unwrap(),
                hex_devices::overdrive::parameter::DRY_WET,
                ParameterValue::Float(0.0),
            )
            .unwrap();
        let device = f.editor.project().main_devices()[0].id();
        f.editor
            .execute(Command::InsertMainDevice {
                before: Some(device),
                state,
            })
            .unwrap();
        let playback = compile(&f, true);
        assert_eq!(playback.main_output_latency_frames(), 27);
        publish(&mut f.control, playback, PublicationPolicy::InheritRuntime);
        let root = f.control.native_candidate_root();
        f.control.command(PlaybackCommand::Play).unwrap();
        assert_eq!(physical_block::<8>(&mut f), [[0.0; 2]; 8]);
        replacement::wait_for_blocks(&root, f.binding, 1);
        let source = f.control.status.native_effect_source().unwrap();
        let until = Instant::now() + Duration::from_secs(3);
        let (request, target) = loop {
            let result = if held {
                f.control
                    .native_source_parameter_hold_with_delivery_on_control(
                        source,
                        f.binding,
                        NativeParameterHold::Begin {
                            id: 7,
                            normalized: 0.75,
                        },
                    )
            } else {
                f.control
                    .begin_native_source_parameter_change_with_delivery_on_control(
                        source, f.binding, 11, 0.75,
                    )
            };
            match result {
                Ok(pair) => break pair,
                Err("native preparation is busy" | "native parameter mailbox is busy") => {}
                Err(reason) => panic!("pre-roll input admission: {reason}"),
            }
            assert!(Instant::now() < until);
        };
        assert_eq!(physical_block::<8>(&mut f), [[0.0; 2]; 8]);
        replacement::wait_for_blocks(&root, f.binding, 2);
        assert_eq!(physical_block::<8>(&mut f), [[0.0; 2]; 8]);
        replacement::wait_for_blocks(&root, f.binding, 3);
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            match f
                .control
                .poll_native_source_parameter_change_on_control(f.binding, &request)
            {
                Ok(NativeWorkerParameterProgress::Processed(input)) => {
                    assert_eq!(
                        (input.sequence, input.first_frame),
                        (2, 16),
                        "origin-less quantum must keep the observed input queued"
                    );
                    break;
                }
                Ok(NativeWorkerParameterProgress::Pending) | Err("native preparation is busy") => {}
                result => panic!("pre-roll original SDK terminal: {result:?}"),
            }
            assert!(Instant::now() < until);
        }
        let output = physical_block::<8>(&mut f);
        replacement::wait_for_blocks(&root, f.binding, 4);
        assert_eq!(&output[..3], &[[0.0; 2]; 3]);
        assert_eq!(&output[3..], &[[0.25, -0.5]; 5]);
        assert!(
            f.control
                .try_native_source_parameter_delivery_on_control()
                .is_none()
        );
        assert_eq!(physical_block::<1>(&mut f), [[0.75, -1.5]; 1]);
        assert_eq!(
            f.control.try_native_source_parameter_delivery_on_control(),
            Some(crate::foreign_parameter_delivery::Receipt::Delivered {
                target,
                normalized: 0.75,
                origin: hex_plugin_host::transport::ManualParameterOrigin {
                    arrangement_frame: 5,
                    performance_frame: 5,
                    loop_pass: 0
                },
                delivery_performance_frame: 32,
            })
        );
        assert!(
            f.control
                .try_native_source_parameter_delivery_on_control()
                .is_none()
        );
        dispose(f);
    }
}
