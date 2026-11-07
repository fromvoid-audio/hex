//! Accepted manual VST3 input retains the exact aligned Arrangement origin.
use super::*;
use crate::plugin_worker::NativeParameterHold;
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_plugin_host::transport::ManualParameterOrigin;

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_manual_parameter_waits_for_final_main_delivery_across_compensated_cuts() {
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
    let mut f = fixture_with_authored("processing", 2, 8, &[7]);
    let upstream = &hex_devices::overdrive::OVERDRIVE_DEFINITION;
    let state = upstream
        .patch_parameter(
            &upstream.default_state().unwrap(),
            hex_devices::overdrive::parameter::DRY_WET,
            ParameterValue::Float(0.0),
        )
        .unwrap();
    let device = f.editor.project().main_devices()[0].id();
    f.editor
        .execute(Command::InsertMainDevice {
            before: Some(device),
            state: state.clone(),
        })
        .unwrap();
    f.editor
        .execute(Command::InsertMainDevice {
            before: None,
            state,
        })
        .unwrap();
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
    let hold = loop {
        match f.control.native_source_parameter_hold_on_control(
            expected,
            f.binding,
            NativeParameterHold::Begin {
                id: 7,
                normalized: 0.75,
            },
        ) {
            Ok(token) => break token,
            Err("native preparation is busy" | "native parameter mailbox is busy") => {}
            Err(error) => panic!("manual input: {error}"),
        }
        assert!(Instant::now() < until);
        std::thread::yield_now();
    };

    let target = loop {
        match f
            .control
            .begin_native_source_parameter_delivery_on_control(expected, f.binding, &hold)
        {
            Ok(target) => break target,
            Err(_) if Instant::now() < until => std::thread::yield_now(),
            Err(error) => panic!("delivery observation: {error}"),
        }
    };
    physical_block::<5>(&mut f);
    physical_block::<3>(&mut f);
    replacement::wait_for_blocks(&root, f.binding, 3);
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
    assert_eq!(&leading[3..], &[[0.25, -0.5]; 2]);
    replacement::wait_for_blocks(&root, f.binding, 5);
    assert_eq!(physical_block::<3>(&mut f), [[0.25, -0.5]; 3]);
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
    dispose(f);
}
