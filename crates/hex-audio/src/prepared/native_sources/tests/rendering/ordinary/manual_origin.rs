//! Accepted manual VST3 input retains the exact aligned Arrangement origin.
use super::*;
use crate::plugin_worker::NativeParameterHold;
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_plugin_host::transport::ManualParameterOrigin;

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_manual_parameter_keeps_compensated_arrangement_origin_across_cuts() {
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
            state,
        })
        .unwrap();
    let playback = compile(&f, true);
    assert_eq!(playback.main_output_latency_frames(), 11 + 16);
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
    let _hold = loop {
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

    physical_block::<5>(&mut f);
    physical_block::<3>(&mut f);
    replacement::wait_for_blocks(&root, f.binding, 3);
    physical_block::<6>(&mut f);
    let expected_pcm = physical_block::<5>(&mut f);
    assert_eq!(&expected_pcm[..2], &[[0.25, -0.5]; 2]);
    assert_eq!(&expected_pcm[2..], &[[0.75, -1.5]; 3]);
    replacement::wait_for_blocks(&root, f.binding, 4);
    let mut accepted = None;
    allocation_free(|| {
        f.data.owner_mut().unwrap().run(|callback, _| {
            accepted = callback
                .native_sources
                .take_accepted_manual_parameter_for_test(f.binding);
            assert_eq!(
                callback
                    .native_sources
                    .take_accepted_manual_parameter_for_test(f.binding),
                None
            );
        });
    });
    let accepted = accepted.unwrap();
    assert_eq!(accepted.binding, f.binding);
    assert_eq!(
        (
            accepted.sequence,
            accepted.first_frame,
            accepted.callback_offset
        ),
        (2, 16, 2)
    );
    assert_eq!(accepted.input.point.normalized, 0.75);
    assert_eq!(
        accepted.origin,
        Some(ManualParameterOrigin {
            // Input clock has already traversed Overdrive's exact 11 frames.
            arrangement_frame: 5,
            performance_frame: 5,
            loop_pass: 0,
        })
    );
    dispose(f);
}
