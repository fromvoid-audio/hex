//! A signed editor input is witnessed only on the final compensated Main sample.
use super::*;
use crate::foreign_parameter_delivery::{AbortReason, editor::Receipt};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_plugin_host::{protocol::ParameterEditKind, transport::ManualParameterOrigin};

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn raw_zero_editor_value_waits_for_its_exact_compensated_main_sample() {
    const CHILD: &str = "HEX_ORDINARY_EDITOR_MAIN_DRIVER";
    if std::env::var_os(CHILD).is_none() {
        let temporary =
            std::env::temp_dir().join(format!("hex-editor-main-{}", std::process::id()));
        std::fs::create_dir(&temporary).unwrap();
        let name = std::thread::current().name().unwrap().to_owned();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--ignored", "--test-threads=1"])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "89")
            .env("HEX_ORIGINAL_INSTANCE_RELEASE", temporary.join("release"))
            .output()
            .unwrap();
        std::fs::remove_dir_all(temporary).unwrap();
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
    let mut f = fixture("processing");
    let overdrive = &hex_devices::overdrive::OVERDRIVE_DEFINITION;
    let dry = overdrive
        .patch_parameter(
            &overdrive.default_state().unwrap(),
            hex_devices::overdrive::parameter::DRY_WET,
            ParameterValue::Float(0.0),
        )
        .unwrap();
    let device = f.editor.project().main_devices()[0].id();
    for before in [Some(device), None] {
        f.editor
            .execute(Command::InsertMainDevice {
                before,
                state: dry.clone(),
            })
            .unwrap();
    }
    let playback = compile(&f, true);
    assert_eq!(playback.main_output_latency_frames(), 38);
    publish(&mut f.control, playback, PublicationPolicy::InheritRuntime);
    let root = f.control.native_candidate_root();
    f.control.command(PlaybackCommand::Play).unwrap();
    physical_block::<5>(&mut f);
    physical_block::<3>(&mut f);
    replacement::wait_for_blocks(&root, f.binding, 1);
    physical_block::<7>(&mut f);
    physical_block::<1>(&mut f);
    replacement::wait_for_blocks(&root, f.binding, 2);

    let source = f.control.status.native_effect_source().unwrap();
    let until = Instant::now() + Duration::from_secs(2);
    let target = loop {
        match f
            .control
            .arm_native_source_editor_delivery_on_control(source, f.binding)
        {
            Ok(target) => break target,
            Err("native preparation is busy") => {}
            Err(reason) => panic!("{reason}"),
        }
        assert!(Instant::now() < until);
        std::thread::yield_now();
    };
    std::fs::write(
        std::env::var_os("HEX_ORIGINAL_INSTANCE_RELEASE").unwrap(),
        b"editor",
    )
    .unwrap();
    let mut ordered = Vec::new();
    let mut poll = |f: &mut OrdinaryFixture, stage| {
        match f
            .control
            .poll_native_source_parameter_edits_on_control(f.binding)
        {
            Ok(Some(batch)) => {
                ordered.extend_from_slice(batch.as_slice());
                if stage && batch.ready > 0 {
                    return true;
                }
            }
            Ok(None) | Err("native preparation is busy" | "native edit observation is busy") => {}
            Err(reason) => panic!("{reason}"),
        }
        !stage
            && ordered
                .iter()
                .any(|point| point.kind == ParameterEditKind::Perform)
    };
    while !poll(&mut f, true) {
        assert!(
            Instant::now() < until,
            "signed editor input never entered the worker queue"
        );
        std::thread::yield_now();
    }
    physical_block::<5>(&mut f);
    physical_block::<3>(&mut f);
    replacement::wait_for_blocks(&root, f.binding, 3);
    while !poll(&mut f, false) {
        assert!(Instant::now() < until);
        std::thread::yield_now();
    }
    assert!(
        f.control
            .try_native_source_editor_delivery_on_control()
            .is_none()
    );
    physical_block::<6>(&mut f);
    physical_block::<5>(&mut f);
    replacement::wait_for_blocks(&root, f.binding, 4);
    physical_block::<5>(&mut f);
    replacement::wait_for_blocks(&root, f.binding, 5);
    let before = physical_block::<3>(&mut f);
    assert!(
        f.control
            .try_native_source_editor_delivery_on_control()
            .is_none()
    );
    assert_eq!(
        f.control
            .native_source_editor_confirmed_sequence_on_control(target),
        None
    );
    let sound = physical_block::<1>(&mut f)[0];
    let Some(Receipt::Confirmed(confirmed)) =
        f.control.try_native_source_editor_delivery_on_control()
    else {
        panic!("the first affected final Main sample must confirm the editor point");
    };
    assert_eq!(confirmed.target, target);
    assert_eq!(confirmed.main_performance_frame, 43);
    assert_eq!(
        confirmed.input.origin,
        Some(ManualParameterOrigin {
            arrangement_frame: 5,
            performance_frame: 5,
            loop_pass: 0,
        })
    );
    assert_eq!(
        (
            confirmed.input.sequence,
            confirmed.input.first_frame,
            confirmed.input.callback_offset
        ),
        (2, 16, 2)
    );
    let [point] = confirmed.input.performs.as_slice() else {
        panic!("one actual editor input")
    };
    let ordered = ordered
        .iter()
        .find(|point| point.kind == ParameterEditKind::Perform)
        .unwrap();
    assert_eq!(
        (point.id, point.sequence, point.gesture, point.normalized),
        (0, ordered.sequence, ordered.gesture, 0.75)
    );
    assert_eq!(ordered.first_frame, 16);
    assert_eq!(sound, [0.5625, -1.125]);
    assert_ne!(before[2], sound);
    assert_eq!(
        f.control
            .native_source_editor_confirmed_sequence_on_control(target),
        Some(2)
    );
    assert!(
        f.control
            .try_native_source_editor_delivery_on_control()
            .is_none()
    );
    assert!(
        f.control
            .cancel_native_source_editor_delivery_on_control(target)
    );
    physical_block::<4>(&mut f);
    assert_eq!(
        f.control.try_native_source_editor_delivery_on_control(),
        Some(Receipt::Aborted {
            target,
            reason: AbortReason::Cancelled
        })
    );
    assert!(
        f.control
            .try_native_source_editor_delivery_on_control()
            .is_none()
    );
    dispose(f);
}
