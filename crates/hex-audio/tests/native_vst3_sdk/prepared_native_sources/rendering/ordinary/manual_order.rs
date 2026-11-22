//! A later native operation must not erase an earlier input awaiting Main.
use super::*;
use crate::plugin_worker::{
    NativeParameterHold, NativeWorkerParameterChange, NativeWorkerParameterProgress,
};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn scheduled_manual_input_survives_a_later_unobserved_sdk_change_before_main_delivery() {
    const CHILD: &str = "HEX_ORDINARY_MANUAL_ORDER_DRIVER";
    if std::env::var_os(CHILD).is_none() {
        let name = std::thread::current().name().unwrap().to_owned();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                &name,
                "--ignored",
                "--test-threads=1",
                "--nocapture",
            ])
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
    let mut f = fixture_with_authored("processing", 2, 8, &[7]);
    let definition = &hex_devices::overdrive::OVERDRIVE_DEFINITION;
    let state = definition
        .patch_parameter(
            &definition.default_state().unwrap(),
            hex_devices::overdrive::parameter::DRY_WET,
            ParameterValue::Float(0.0),
        )
        .unwrap();
    f.editor
        .execute(Command::InsertMainDevice {
            before: None,
            state,
        })
        .unwrap();
    let playback = compile(&f, true);
    assert_eq!(playback.main_output_latency_frames(), 27);
    publish(&mut f.control, playback, PublicationPolicy::InheritRuntime);
    let root = f.control.native_candidate_root();
    f.control.command(PlaybackCommand::Play).unwrap();
    for count in 1..=2 {
        assert_eq!(physical_block::<8>(&mut f), [[0.0; 2]; 8]);
        replacement::wait_for_blocks(&root, f.binding, count);
    }
    let source = f.control.status.native_effect_source().unwrap();
    let until = Instant::now() + Duration::from_secs(3);
    let (hold, target) = loop {
        match f
            .control
            .native_source_parameter_hold_with_delivery_on_control(
                source,
                f.binding,
                NativeParameterHold::Begin {
                    id: 7,
                    normalized: 0.75,
                },
            ) {
            Ok(pair) => break pair,
            Err("native preparation is busy" | "native parameter mailbox is busy") => {}
            Err(reason) => panic!("observed hold: {reason}"),
        }
        assert!(Instant::now() < until);
    };
    physical_block::<8>(&mut f);
    replacement::wait_for_blocks(&root, f.binding, 3);
    processed(&mut f, &hold);
    let until = Instant::now() + Duration::from_secs(3);
    let later = loop {
        match f.control.native_source_parameter_hold_on_control(
            source,
            f.binding,
            NativeParameterHold::Change {
                hold: &hold,
                normalized: 0.5,
            },
        ) {
            Ok(request) => break request,
            Err("native preparation is busy" | "native parameter mailbox is busy") => {}
            Err(reason) => panic!("later SDK input: {reason}"),
        }
        assert!(Instant::now() < until);
    };
    physical_block::<8>(&mut f);
    replacement::wait_for_blocks(&root, f.binding, 4);
    processed(&mut f, &later);
    physical_block::<8>(&mut f);
    replacement::wait_for_blocks(&root, f.binding, 5);
    assert!(
        f.control
            .try_native_source_parameter_delivery_on_control()
            .is_none()
    );
    let output = physical_block::<8>(&mut f);
    replacement::wait_for_blocks(&root, f.binding, 6);
    assert_eq!(&output[..3], &[[0.25, -0.5]; 3]);
    assert_eq!(&output[3..], &[[0.75, -1.5]; 5]);
    assert_eq!(
        f.control.try_native_source_parameter_delivery_on_control(),
        Some(crate::foreign_parameter_delivery::Receipt::Delivered {
            target,
            normalized: 0.75,
            origin: hex_plugin_host::transport::ManualParameterOrigin {
                arrangement_frame: 16,
                performance_frame: 16,
                loop_pass: 0
            },
            delivery_performance_frame: 43,
        })
    );
    assert!(
        f.control
            .try_native_source_parameter_delivery_on_control()
            .is_none()
    );
    dispose(f);
}

fn processed(f: &mut OrdinaryFixture, request: &NativeWorkerParameterChange) {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        match f
            .control
            .poll_native_source_parameter_change_on_control(f.binding, request)
        {
            Ok(NativeWorkerParameterProgress::Processed(_)) => return,
            Ok(NativeWorkerParameterProgress::Pending) | Err("native preparation is busy") => {}
            result => panic!("actual SDK input terminal: {result:?}"),
        }
        assert!(Instant::now() < until);
        std::thread::yield_now();
    }
}
