//! A signed editor input is witnessed only on the final compensated Main sample.
use super::*;
use crate::foreign_parameter_delivery::editor::{AbortReason, Receipt, TouchPlan};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_plugin_host::{protocol::ParameterEditKind, transport::ManualParameterOrigin};

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn raw_zero_editor_value_waits_for_its_exact_compensated_main_sample() {
    verify_editor_delivery(false);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn editor_touch_and_its_distinct_restoration_reach_compensated_main_once() {
    verify_editor_delivery(true);
}

fn verify_editor_delivery(reading: bool) {
    const CHILD: &str = "HEX_ORDINARY_EDITOR_MAIN_DRIVER";
    if std::env::var_os(CHILD).is_none() {
        let temporary =
            std::env::temp_dir().join(format!("hex-editor-main-{}", std::process::id()));
        std::fs::create_dir(&temporary).unwrap();
        let name = std::thread::current().name().unwrap().to_owned();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        if reading {
            child.env("HEX_ORIGINAL_EDITOR_END_RELEASE", temporary.join("end"));
        }
        let result = child
            .args(["--exact", &name, "--ignored", "--test-threads=1"])
            .env(CHILD, "1")
            .env(
                "HEX_ORIGINAL_INSTANCE_VARIANT",
                if reading { "147" } else { "89" },
            )
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
    let mut f = if reading {
        fixture_with_authored("processing", 2, 8, &[7])
    } else {
        fixture("processing")
    };
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

    let (source, epoch) = f.control.status.live_native_parameter_source().unwrap();
    let touch = TouchPlan {
        parameter: 7,
        base: 0.25,
        reading: true,
    };
    if reading {
        assert!(
            f.control
                .arm_native_source_editor_touch_delivery_on_control(
                    source,
                    epoch + 1,
                    f.binding,
                    touch,
                )
                .is_err()
        );
    }
    let until = Instant::now() + Duration::from_secs(2);
    let target = loop {
        let armed = if reading {
            f.control
                .arm_native_source_editor_touch_delivery_on_control(source, epoch, f.binding, touch)
        } else {
            f.control
                .arm_native_source_editor_delivery_on_control(source, f.binding)
        };
        match armed {
            Ok(target) => break target,
            Err("native preparation is busy") => {}
            Err(reason) => panic!("{reason}"),
        }
        assert!(Instant::now() < until);
        std::thread::yield_now();
    };
    if reading {
        assert!(f.control.status.reserve_native_parameter_input().is_err());
    }
    std::fs::write(
        std::env::var_os("HEX_ORIGINAL_INSTANCE_RELEASE").unwrap(),
        b"editor",
    )
    .unwrap();
    let mut ordered = Vec::new();
    let mut poll = |f: &mut OrdinaryFixture, kind| {
        match f
            .control
            .poll_native_source_parameter_edits_on_control(f.binding)
        {
            Ok(Some(batch)) => {
                ordered.extend_from_slice(batch.as_slice());
                if kind == ParameterEditKind::Begin && batch.ready >= 1 {
                    return true;
                }
            }
            Ok(None) | Err("native preparation is busy" | "native edit observation is busy") => {}
            Err(reason) => panic!("{reason}"),
        }
        kind != ParameterEditKind::Begin && ordered.iter().any(|point| point.kind == kind)
    };
    while !poll(&mut f, ParameterEditKind::Begin) {
        assert!(
            Instant::now() < until,
            "signed editor input never entered the worker queue"
        );
        std::thread::yield_now();
    }
    physical_block::<5>(&mut f);
    physical_block::<3>(&mut f);
    replacement::wait_for_blocks(&root, f.binding, 3);
    while !poll(&mut f, ParameterEditKind::Perform) {
        assert!(Instant::now() < until);
        std::thread::yield_now();
    }
    assert!(
        f.control
            .try_native_source_editor_delivery_on_control()
            .is_none()
    );
    if reading {
        // A real mouse release can happen after the last Perform has already
        // been processed. The existing ordered reader may see End before PCM.
        std::fs::write(
            std::env::var_os("HEX_ORIGINAL_EDITOR_END_RELEASE").unwrap(),
            b"end",
        )
        .unwrap();
        while !poll(&mut f, ParameterEditKind::End) {
            assert!(Instant::now() < until);
            std::thread::yield_now();
        }
    }
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
    let actual_perform = ordered
        .iter()
        .find(|point| point.kind == ParameterEditKind::Perform)
        .unwrap();
    assert_eq!(
        (point.id, point.sequence, point.gesture, point.normalized),
        (
            if reading { 7 } else { 0 },
            actual_perform.sequence,
            actual_perform.gesture,
            0.75
        )
    );
    assert_eq!(actual_perform.first_frame, 16);
    assert!(confirmed.input.restorations.as_slice().is_empty());
    assert!(!confirmed.input.lost_origin);
    // Starting the authored clock at physical frame 11 splits SDK quantum 1.
    // The fixture's call-dependent gain therefore advances once more in reading mode.
    let held = if reading {
        [0.65625, -1.3125]
    } else {
        [0.5625, -1.125]
    };
    assert_eq!(sound, held);
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
    let mut retired = if reading {
        let mut retired = f
            .control
            .retire_native_source_editor_delivery_on_control()
            .unwrap();
        assert_eq!(retired.target(), target);
        assert_eq!(physical_block::<7>(&mut f), [held; 7]);
        assert!(retired.try_receipt_on_control().is_none());
        assert_eq!(physical_block::<1>(&mut f), [[0.25, -0.5]; 1]);
        let Some(Receipt::Confirmed(end)) = retired.try_receipt_on_control() else {
            panic!("the first restored Main sample must carry its exact terminal");
        };
        assert_eq!(end.target, target);
        assert_eq!(end.main_performance_frame, 51);
        assert_eq!(end.input.sequence, 3);
        assert_eq!(
            end.input.origin,
            Some(ManualParameterOrigin {
                arrangement_frame: 13,
                performance_frame: 13,
                loop_pass: 0,
            })
        );
        assert!(end.input.performs.as_slice().is_empty());
        let ended = ordered
            .iter()
            .find(|point| point.kind == ParameterEditKind::End)
            .unwrap();
        assert_eq!(
            end.input.restorations.as_slice(),
            [hex_plugin_host::protocol::EditorRestoration {
                sequence: ended.sequence,
                gesture: point.gesture,
                parameter: 7,
            }]
        );
        assert!(retired.try_receipt_on_control().is_none());
        Some(retired)
    } else {
        None
    };
    assert!(match retired.as_ref() {
        Some(reader) => reader.cancel_on_control(),
        None => f
            .control
            .cancel_native_source_editor_delivery_on_control(target),
    });
    physical_block::<4>(&mut f);
    let mut next = || match retired.as_mut() {
        Some(reader) => reader.try_receipt_on_control(),
        None => f.control.try_native_source_editor_delivery_on_control(),
    };
    assert_eq!(
        next(),
        Some(Receipt::Aborted {
            target,
            reason: AbortReason::Cancelled
        })
    );
    assert!(next().is_none());
    if reading {
        f.control.status.reserve_native_parameter_input().unwrap();
        f.control.status.native_parameter_input_end();
    }
    dispose(f);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn paired_editor_values_and_independent_restores_are_confirmed_only_at_main() {
    use hex_plugin_host::protocol::EditorTouchTarget;
    const CHILD: &str = "HEX_ORDINARY_GROUP_EDITOR_MAIN_DRIVER";
    if std::env::var_os(CHILD).is_none() {
        for (name, variant) in [("effect", "148"), ("instrument", "149")] {
            let temporary =
                std::env::temp_dir().join(format!("hex-group-main-{}-{name}", std::process::id()));
            std::fs::create_dir(&temporary).unwrap();
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    std::thread::current().name().unwrap(),
                    "--ignored",
                ])
                .env(CHILD, &temporary)
                .env("HEX_ORIGINAL_INSTANCE_VARIANT", variant)
                .env("HEX_ORIGINAL_GROUPED_EDITOR_PATTERN", "staggered")
                .env("HEX_ORIGINAL_INSTANCE_RELEASE", temporary.join("perform"))
                .env("HEX_ORIGINAL_EDITOR_END_RELEASE", temporary.join("end"))
                .env(
                    "HEX_ORIGINAL_EDITOR_SECOND_END_RELEASE",
                    temporary.join("second"),
                )
                .output()
                .unwrap();
            std::fs::remove_dir_all(temporary).unwrap();
            assert!(
                result.status.success()
                    && String::from_utf8_lossy(&result.stdout).contains("1 passed"),
                "{name}: {}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        return;
    }
    let _serial = serial();
    let gate = std::path::PathBuf::from(std::env::var_os(CHILD).unwrap());
    let instrument = std::env::var("HEX_ORIGINAL_INSTANCE_VARIANT").as_deref() == Ok("149");
    let mut f = fixture_with_authored_mode(
        "processing",
        2,
        8,
        if instrument { &[0, 11, 23] } else { &[0, 11] },
        instrument,
    );
    let device = if instrument {
        f.editor.project().tracks()[0].instrument().unwrap().id()
    } else {
        f.editor.project().main_devices()[0].id()
    };
    let ids: &[u32] = if instrument { &[0, 11, 23] } else { &[0, 11] };
    for &id in ids {
        f.editor
            .execute(Command::ReplaceDeviceAutomationTarget {
                device,
                target: hex_project::DeviceAutomationTarget::Vst3 { parameter: id },
                points: [(0, 0.25), (20, 0.5)][..if id == 23 { 2 } else { 1 }]
                    .iter()
                    .map(|&(frame, value)| hex_project::AutomationPoint {
                        beat: hex_project::BeatTime::from_ratio(frame, 24_000).unwrap(),
                        value: hex_project::AutomationValue::new(value).unwrap(),
                        outgoing: hex_project::AutomationInterpolation::Step,
                    })
                    .collect(),
            })
            .unwrap();
    }
    let overdrive = &hex_devices::overdrive::OVERDRIVE_DEFINITION;
    let dry = overdrive
        .patch_parameter(
            &overdrive.default_state().unwrap(),
            hex_devices::overdrive::parameter::DRY_WET,
            ParameterValue::Float(0.0),
        )
        .unwrap();
    for before in [(!instrument).then_some(device), None] {
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
    for block in 0..2 {
        physical_block::<5>(&mut f);
        physical_block::<3>(&mut f);
        replacement::wait_for_blocks(&root, f.binding, block + 1);
    }
    let (source, epoch) = f.control.status.live_native_parameter_source().unwrap();
    let targets = [0, 11].map(|parameter| EditorTouchTarget {
        parameter,
        base: 0.25,
        reading: true,
    });
    assert!(
        f.control
            .arm_native_source_editor_group_delivery_on_control(
                source,
                epoch + 1,
                f.binding,
                &targets,
            )
            .is_err()
    );
    let target = f
        .control
        .arm_native_source_editor_group_delivery_on_control(source, epoch, f.binding, &targets)
        .unwrap();

    let mut ordered = Vec::new();
    let mut confirmed = Vec::new();
    let mut samples = Vec::new();
    let origin = if instrument { 16 } else { 5 };
    let due = [origin + 38, origin + 46, origin + 54];
    for block in 2..if instrument { 11 } else { 8 } {
        if let Some(name) = match block {
            2 => Some("perform"),
            3 => Some("end"),
            4 => Some("second"),
            _ => None,
        } {
            std::fs::write(gate.join(name), b"original sdk group gate").unwrap();
            let until = Instant::now() + Duration::from_secs(2);
            loop {
                let ready = match f
                    .control
                    .poll_native_source_parameter_edits_on_control(f.binding)
                {
                    Ok(Some(batch)) => {
                        ordered.extend_from_slice(batch.as_slice());
                        match block {
                            2 => batch.ready >= 4,
                            3 => ordered
                                .iter()
                                .any(|edit| edit.kind == ParameterEditKind::End && edit.id == 0),
                            _ => ordered
                                .iter()
                                .any(|edit| edit.kind == ParameterEditKind::End && edit.id == 11),
                        }
                    }
                    Ok(None)
                    | Err("native preparation is busy" | "native edit observation is busy") => {
                        false
                    }
                    Err(reason) => panic!("{reason}"),
                };
                if ready {
                    break;
                }
                assert!(Instant::now() < until, "group gate {name}");
                std::thread::yield_now();
            }
        }
        samples.extend(physical_block::<5>(&mut f));
        samples.extend(physical_block::<3>(&mut f));
        replacement::wait_for_blocks(&root, f.binding, block + 1);
        while let Some(receipt) = f.control.try_native_source_editor_delivery_on_control() {
            let Receipt::Confirmed(quantum) = receipt else {
                panic!("premature group abort: {receipt:?}");
            };
            assert_eq!(quantum.target, target);
            assert_eq!(quantum.main_performance_frame / 8, block);
            confirmed.push(quantum);
        }
        assert_eq!(
            confirmed.len(),
            due.iter().filter(|&&frame| frame / 8 <= block).count()
        );
    }
    let [held, first, second] = confirmed.as_slice() else {
        panic!("one paired value and two individual restores");
    };
    assert_eq!(
        [
            held.main_performance_frame,
            first.main_performance_frame,
            second.main_performance_frame
        ],
        due
    );
    assert_eq!(
        [held.input, first.input, second.input]
            .map(|input| input.origin.unwrap().arrangement_frame),
        [origin, origin + 8, origin + 16]
    );
    let [gain, axis] = held.input.performs.as_slice() else {
        panic!("paired effective values");
    };
    assert_eq!(
        (gain.id, gain.normalized, axis.id, axis.normalized),
        (0, 0.75, 11, 0.5)
    );
    for (restored, point) in [(first, gain), (second, axis)] {
        let end = ordered
            .iter()
            .find(|edit| edit.kind == ParameterEditKind::End && edit.id == point.id)
            .unwrap();
        assert_eq!(
            restored.input.restorations.as_slice(),
            [hex_plugin_host::protocol::EditorRestoration {
                sequence: end.sequence,
                gesture: point.gesture,
                parameter: point.id,
            }]
        );
    }
    let expected: &[(usize, Frame)] = if instrument {
        &[
            (53, [1.0; 2]),
            (54, [6.0; 2]),
            (57, [6.0; 2]),
            (58, [12.0; 2]),
            (61, [12.0; 2]),
            (62, [4.0; 2]),
            (69, [4.0; 2]),
            (70, [2.0; 2]),
            (81, [2.0; 2]),
            (82, [0.0; 2]),
        ]
    } else {
        &[
            (42, [0.25, -0.5]),
            (43, [1.5, -3.0]),
            (50, [1.5, -3.0]),
            (51, [0.5, -1.0]),
            (58, [0.5, -1.0]),
            (59, [0.25, -0.5]),
        ]
    };
    for &(frame, value) in expected {
        assert_eq!(samples[frame - 16], value, "exact Main frame {frame}");
    }
    f.control.command(PlaybackCommand::Stop).unwrap();
    physical_block::<4>(&mut f);
    assert_eq!(
        f.control.try_native_source_editor_delivery_on_control(),
        Some(Receipt::Aborted {
            target,
            reason: AbortReason::SourceChanged,
        })
    );
    assert!(
        f.control
            .try_native_source_editor_delivery_on_control()
            .is_none()
    );
    dispose(f);
}
