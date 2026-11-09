use super::*;
use crate::Session;
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, Command, Created,
    DeviceAutomationTarget, DeviceId,
};
use hex_storage::automation_draft::{
    AppliedEvent, CaptureClock, CompletedAutomationTake, DraftClock, DraftProvenance,
    DraftReadLimits, DraftTarget, DraftTerminal, DraftWriter, MAX_APPEND_EVENTS,
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "hex-device-take-adoption-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> PathBuf {
        self.0.join("capture.hex-automation-draft")
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn fixture(outgoing: AutomationInterpolation) -> (Session, DeviceId, DraftProvenance) {
    let state = hex_plugin_state::encode(hex_plugin_state::State {
        identity: hex_plugin_state::Identity {
            class: hex_plugin_state::ClassId::new([9; 16]).unwrap(),
            binary: hex_plugin_state::BinaryIdentity {
                executable_sha256: [6; 32],
                info_sha256: [7; 32],
            },
        },
        component: Some(&[42; 12]),
        controller: Some(&[]),
    })
    .unwrap();
    let mut session = Session::new(48_000, 16, 4096).unwrap();
    // Canonical conversion fixture only: no SDK runtime or publication authority.
    // Runtime PCM/admission is owned by the later capture consumer.
    let Created::Device(device) = session
        .editor
        .execute(Command::InsertMainDevice {
            before: None,
            state,
        })
        .unwrap()
    else {
        panic!("device")
    };
    session
        .editor
        .execute(Command::ReplaceDeviceAutomationTarget {
            device,
            target: DeviceAutomationTarget::Vst3 { parameter: 0 },
            points: vec![
                AutomationPoint {
                    beat: BeatTime::ZERO,
                    value: AutomationValue::new(0.1).unwrap(),
                    outgoing,
                },
                AutomationPoint {
                    beat: BeatTime::ONE,
                    value: AutomationValue::new(0.9).unwrap(),
                    outgoing: AutomationInterpolation::Step,
                },
            ],
        })
        .unwrap();
    (
        session,
        device,
        DraftProvenance {
            sample_rate: 48_000,
            output_identity: 9,
            project_generation: 0,
            editor_revision: 2,
            audio_revision: 7,
            transport_epoch: 1,
            gesture_serial: 1,
        },
    )
}

fn write_take(
    path: &Path,
    session: &Session,
    device: DeviceId,
    stamp: DraftProvenance,
    events: &[AppliedEvent],
    end: u64,
) -> CompletedAutomationTake {
    write_target_take(
        path,
        session,
        device,
        DeviceAutomationTarget::Vst3 { parameter: 0 },
        stamp,
        events,
        end,
    )
}
fn write_target_take(
    path: &Path,
    session: &Session,
    device: DeviceId,
    kind: DeviceAutomationTarget,
    stamp: DraftProvenance,
    events: &[AppliedEvent],
    end: u64,
) -> CompletedAutomationTake {
    let target = DraftTarget::Device {
        device,
        target: kind,
    };
    let mut writer = DraftWriter::create_with_clock(
        path,
        session.project(),
        target,
        stamp,
        CaptureClock {
            sample_rate: 48_000,
            loop_region: None,
        },
    )
    .unwrap();
    for batch in events.chunks(MAX_APPEND_EVENTS) {
        writer.append(batch).unwrap();
    }
    writer
        .finish(DraftTerminal::Finished(DraftClock {
            performance_frame: end,
            content_frame: end,
            loop_pass: 0,
        }))
        .unwrap();
    drop(writer);
    CompletedAutomationTake::open_on_control(
        path,
        DraftReadLimits {
            max_source_bytes: hex_storage::MAX_PROJECT_DOCUMENT_BYTES as u64,
        },
    )
    .unwrap()
}
fn event(frame: u64, value: f32) -> AppliedEvent {
    AppliedEvent {
        sequence: frame + 1,
        performance_frame: frame,
        content_frame: frame,
        loop_pass: 0,
        value: AutomationValue::new(value).unwrap(),
    }
}

#[test]
fn sealed_vst3_take_prepares_one_class_stamped_splice_with_unchanged_step_and_linear_base() {
    for outgoing in [
        AutomationInterpolation::Step,
        AutomationInterpolation::Linear,
    ] {
        let files = Files::new();
        let path = files.path();
        let (mut session, device, stamp) = fixture(outgoing);
        let before = session.project().clone();
        let mut take = write_take(
            &path,
            &session,
            device,
            stamp,
            &[event(8, 0.75), event(10, 0.25)],
            12,
        );
        let original = fs::read(&path).unwrap();
        let command =
            prepare_device_take_adoption_on_control(&session, None, &mut take, stamp).unwrap();
        assert!(
            matches!(&command,Command::SpliceDeviceAutomationTouch {vst3_class:Some(class),target:DeviceAutomationTarget::Vst3 {parameter:0},..} if *class==[9;16])
        );
        assert_eq!(session.project(), &before, "preparation is read-only");
        session.editor.execute(command).unwrap();
        let target = DeviceAutomationTarget::Vst3 { parameter: 0 };
        let lane = session
            .project()
            .device(device)
            .unwrap()
            .automation_target(target)
            .unwrap();
        let base = before
            .device(device)
            .unwrap()
            .automation_target(target)
            .unwrap();
        assert_eq!(lane.points, base.points);
        let at = |frame| before.tempo_map().frame_to_beat(frame).unwrap();
        for frame in [0, 7, 12, 24_000] {
            assert_eq!(lane.value_at(at(frame)), base.value_at(at(frame)));
        }
        assert_eq!(lane.value_at(at(8)).unwrap().get(), 0.75);
        assert_eq!(lane.value_at(at(10)).unwrap().get(), 0.25);
        let recorded = session.project().clone();
        assert_eq!(
            hex_storage::schema_version_for(&recorded),
            hex_storage::VST3_AUTOMATION_TOUCH_SCHEMA_VERSION
        );
        assert_eq!(
            hex_storage::decode(&hex_storage::encode(&recorded).unwrap()).unwrap(),
            recorded
        );
        assert!(session.editor.undo());
        assert_eq!(session.project(), &before);
        assert!(session.editor.redo());
        assert_eq!(session.project(), &recorded);
        assert_eq!(
            fs::read(&path).unwrap(),
            original,
            "adoption never deletes the source"
        );
    }
}

#[test]
fn mismatched_provenance_and_current_source_refuse_without_editing_or_discarding_the_take() {
    let files = Files::new();
    let path = files.path();
    let (mut session, device, stamp) = fixture(AutomationInterpolation::Linear);
    let mut take = write_take(&path, &session, device, stamp, &[event(8, 0.75)], 12);
    let original = fs::read(&path).unwrap();
    let before = session.project().clone();
    let wrong = DraftProvenance {
        gesture_serial: stamp.gesture_serial + 1,
        ..stamp
    };
    assert!(prepare_device_take_adoption_on_control(&session, None, &mut take, wrong).is_err());
    assert_eq!(session.project(), &before);
    session
        .editor
        .execute(Command::RemoveDevice(device))
        .unwrap();
    let changed = session.project().clone();
    assert!(prepare_device_take_adoption_on_control(&session, None, &mut take, stamp).is_err());
    assert_eq!(session.project(), &changed);
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn multi_minute_native_and_raw_zero_vst3_takes_keep_every_distinct_step() {
    let files = Files::new();
    let (mut session, vst3, stamp) = fixture(AutomationInterpolation::Linear);
    let Created::Device(native) = session
        .editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: hex_engine::default_native_device_state(hex_engine::DeviceTypeId::from_static(
                "hex.utility",
            ))
            .unwrap(),
        })
        .unwrap()
    else {
        panic!("native effect")
    };
    let count = 1_101;
    let stride = 9_600; // 5 delivered points/second, over 3 minutes.
    let events: Vec<_> = (1..=count)
        .map(|index| event(index * stride, if index % 2 == 0 { 0.25 } else { 0.75 }))
        .collect();
    let terminal = (count + 1) * stride;
    for (device, target, label) in [
        (vst3, DeviceAutomationTarget::Vst3 { parameter: 0 }, "vst3"),
        (native, DeviceAutomationTarget::HostOutputGain, "native"),
    ] {
        let before = session.project().clone();
        let path = files.0.join(format!("{label}.hex-automation-draft"));
        let mut take = write_target_take(&path, &session, device, target, stamp, &events, terminal);
        let bytes = fs::read(&path).unwrap();
        let command =
            prepare_device_take_adoption_on_control(&session, None, &mut take, stamp).unwrap();
        session.editor.execute(command).unwrap();
        let recorded = session.project().clone();
        let lane = recorded
            .device(device)
            .unwrap()
            .automation_target(target)
            .unwrap();
        let [span] = lane.touch.as_slice() else {
            panic!("one continuous take")
        };
        assert_eq!(span.steps.len(), events.len());
        let timing = recorded.tempo_map();
        for (step, event) in span.steps.iter().zip(&events) {
            assert_eq!(
                timing.beat_to_frame(step.beat).unwrap(),
                event.content_frame
            );
            assert_eq!(step.value, event.value);
        }
        assert_eq!(timing.beat_to_frame(span.end).unwrap(), terminal);
        assert_eq!(
            lane.points.as_slice(),
            before
                .device(device)
                .unwrap()
                .automation_target(target)
                .map_or(&[][..], |lane| lane.points.as_slice())
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert!(session.editor.undo());
        assert_eq!(session.project(), &before);
        assert!(session.editor.redo());
        assert_eq!(session.project(), &recorded);
        let saved = files.0.join(format!("{label}.hex"));
        hex_storage::save_new(&saved, &recorded).unwrap();
        assert_eq!(hex_storage::load(&saved).unwrap(), recorded);
    }
}

#[test]
fn over_budget_recorded_density_retains_exact_source_and_draft_without_truncation() {
    let files = Files::new();
    let path = files.path();
    let (session, device, stamp) = fixture(AutomationInterpolation::Step);
    let before = session.project().clone();
    let events: Vec<_> = (0..=hex_project::MAX_DEVICE_AUTOMATION_TOUCH_STEPS_PER_LANE as u64)
        .map(|frame| event(frame, (frame % 2) as f32))
        .collect();
    let mut take = write_take(
        &path,
        &session,
        device,
        stamp,
        &events,
        events.len() as u64 + 1,
    );
    let original = fs::read(&path).unwrap();
    let error =
        prepare_device_take_adoption_on_control(&session, None, &mut take, stamp).unwrap_err();
    assert!(error.contains("touch point budget"), "{error}");
    assert_eq!(session.project(), &before);
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(take.event_count(), events.len() as u64);
}

#[test]
fn looped_vst3_take_keeps_last_pass_overlap_and_older_uncovered_tail_in_one_history_edge() {
    let files = Files::new();
    let path = files.path();
    let (mut session, device, stamp) = fixture(AutomationInterpolation::Linear);
    let before = session.project().clone();
    let target = DeviceAutomationTarget::Vst3 { parameter: 0 };
    let capture = CaptureClock {
        sample_rate: 48_000,
        loop_region: Some(hex_project::LoopRegion::new(8, 16, true).unwrap()),
    };
    let mut writer = DraftWriter::create_with_clock(
        &path,
        &before,
        DraftTarget::Device { device, target },
        stamp,
        capture,
    )
    .unwrap();
    // First pass writes12..16. The partial newer pass overwrites8..14;
    // the first pass's14..16 survives rather than extending the final value.
    writer
        .append(&[
            AppliedEvent {
                content_frame: 12,
                ..event(0, 0.25)
            },
            AppliedEvent {
                content_frame: 8,
                loop_pass: 1,
                ..event(4, 0.75)
            },
            AppliedEvent {
                content_frame: 10,
                loop_pass: 1,
                ..event(6, 0.5)
            },
        ])
        .unwrap();
    writer
        .finish(DraftTerminal::Finished(DraftClock {
            performance_frame: 10,
            content_frame: 14,
            loop_pass: 1,
        }))
        .unwrap();
    drop(writer);
    let original = fs::read(&path).unwrap();
    let mut take = CompletedAutomationTake::open_on_control(
        &path,
        DraftReadLimits {
            max_source_bytes: hex_storage::MAX_PROJECT_DOCUMENT_BYTES as u64,
        },
    )
    .unwrap();
    let command =
        prepare_device_take_adoption_on_control(&session, None, &mut take, stamp).unwrap();
    session.editor.execute(command).unwrap();
    let recorded = session.project().clone();
    let lane = recorded
        .device(device)
        .unwrap()
        .automation_target(target)
        .unwrap();
    let base = before
        .device(device)
        .unwrap()
        .automation_target(target)
        .unwrap();
    assert_eq!(lane.vst3_class, Some([9; 16]));
    assert_eq!(lane.points, base.points);
    for frame in 0..24 {
        let beat = before.tempo_map().frame_to_beat(frame).unwrap();
        let expected = match frame {
            8..10 => Some(AutomationValue::new(0.75).unwrap()),
            10..14 => Some(AutomationValue::new(0.5).unwrap()),
            14..16 => Some(AutomationValue::new(0.25).unwrap()),
            _ => base.value_at(beat),
        };
        assert_eq!(lane.value_at(beat), expected, "frame{frame}");
    }
    assert_eq!(
        hex_storage::decode(&hex_storage::encode(&recorded).unwrap()).unwrap(),
        recorded
    );
    assert!(session.editor.undo());
    assert_eq!(session.project(), &before);
    assert!(session.editor.redo());
    assert_eq!(session.project(), &recorded);
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn complete_group_splices_independent_looped_lanes_and_undoes_together() {
    use hex_storage::automation_draft::{
        CompletedGroupedDraft, GroupApplication, GroupDraftEvent, Vst3GroupTarget,
    };
    let files = Files::new();
    let path = files.path();
    let (mut session, device, stamp) = fixture(AutomationInterpolation::Linear);
    for (parameter, value) in [(11, 0.33), (23, 0.625)] {
        session
            .editor
            .execute(Command::ReplaceDeviceAutomationTarget {
                device,
                target: DeviceAutomationTarget::Vst3 { parameter },
                points: vec![AutomationPoint {
                    beat: BeatTime::ZERO,
                    value: AutomationValue::new(value).unwrap(),
                    outgoing: AutomationInterpolation::Step,
                }],
            })
            .unwrap();
    }
    let before = session.project().clone();
    let target = Vst3GroupTarget::new(device, [9; 16], &[0, 11]).unwrap();
    let capture = CaptureClock {
        sample_rate: 48_000,
        loop_region: Some(hex_project::LoopRegion::new(8, 16, true).unwrap()),
    };
    let rows = [
        (0, 10, 20, 0, 12, GroupApplication::Value(0.25)),
        (11, 11, 21, 2, 14, GroupApplication::Value(0.75)),
        (0, 10, 22, 4, 8, GroupApplication::Value(0.5)),
        (0, 10, 23, 6, 10, GroupApplication::Restored),
        (11, 11, 24, 7, 11, GroupApplication::Value(0.125)),
        (11, 11, 25, 9, 13, GroupApplication::Restored),
    ]
    .into_iter()
    .enumerate()
    .map(
        |(
            index,
            (parameter, sdk_gesture, sdk_sequence, performance_frame, content_frame, application),
        )| {
            GroupDraftEvent {
                ordinal: index as u64 + 1,
                parameter,
                sdk_sequence,
                sdk_gesture,
                clock: DraftClock {
                    performance_frame,
                    content_frame,
                    loop_pass: u64::from(performance_frame >= 4),
                },
                application,
            }
        },
    )
    .collect::<Vec<_>>();
    let mut writer = DraftWriter::create_with_clock(
        &path,
        &before,
        DraftTarget::Vst3Group(target),
        stamp,
        capture,
    )
    .unwrap();
    writer.append_group(&rows).unwrap();
    writer
        .finish(DraftTerminal::Finished(rows[5].clock))
        .unwrap();
    drop(writer);
    let original = fs::read(&path).unwrap();
    let mut take = CompletedGroupedDraft::open(
        &path,
        DraftReadLimits {
            max_source_bytes: hex_storage::MAX_PROJECT_DOCUMENT_BYTES as u64,
        },
        16,
    )
    .unwrap();
    let wrong = DraftProvenance {
        transport_epoch: stamp.transport_epoch + 1,
        ..stamp
    };
    assert!(prepare_group_take_adoption_on_control(&session, None, &mut take, wrong).is_err());
    let command = prepare_group_take_adoption_on_control(&session, None, &mut take, stamp).unwrap();
    assert!(
        matches!(&command, Command::SpliceVst3AutomationTouchGroup { class, lanes, .. }
        if *class == [9;16] && lanes.iter().map(|(parameter, _)| *parameter).eq([0, 11]))
    );
    assert_eq!(session.project(), &before);
    let refused = session.editor.execute_prepared(command.clone(), |_| {
        Err::<(), _>(hex_project::Invalid("graph refused"))
    });
    assert!(refused.is_err());
    assert_eq!(session.project(), &before);
    session.editor.execute(command).unwrap();
    let recorded = session.project().clone();
    let original_device = before.device(device).unwrap();
    let edited_device = recorded.device(device).unwrap();
    let unrelated = DeviceAutomationTarget::Vst3 { parameter: 23 };
    assert_eq!(
        edited_device.automation_target(unrelated),
        original_device.automation_target(unrelated)
    );
    for parameter in [0, 11] {
        let kind = DeviceAutomationTarget::Vst3 { parameter };
        let base = original_device.automation_target(kind).unwrap();
        let lane = edited_device.automation_target(kind).unwrap();
        assert_eq!(lane.vst3_class, Some([9; 16]));
        assert_eq!(lane.points, base.points);
        for frame in 0..18 {
            let beat = before.tempo_map().frame_to_beat(frame).unwrap();
            let step = match (parameter, frame) {
                (0, 8..=9) => Some(0.5),
                (0, 12..=15) => Some(0.25),
                (11, 8..=10 | 14..=15) => Some(0.75),
                (11, 11..=12) => Some(0.125),
                _ => None,
            };
            let expected = step
                .map(|value| AutomationValue::new(value).unwrap())
                .or_else(|| base.value_at(beat));
            assert_eq!(
                lane.value_at(beat),
                expected,
                "raw {parameter}, frame {frame}"
            );
        }
    }
    assert!(prepare_group_take_adoption_on_control(&session, None, &mut take, stamp).is_err());
    let saved = files.0.join("group.hex");
    hex_storage::save_new(&saved, &recorded).unwrap();
    assert_eq!(hex_storage::load(&saved).unwrap(), recorded);
    assert!(session.editor.undo());
    assert_eq!(session.project(), &before);
    assert!(session.editor.redo());
    assert_eq!(session.project(), &recorded);
    assert_eq!(fs::read(&path).unwrap(), original);
}
