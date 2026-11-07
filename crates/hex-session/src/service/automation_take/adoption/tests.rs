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
    let target = DraftTarget::Device {
        device,
        target: DeviceAutomationTarget::Vst3 { parameter: 0 },
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
fn over_budget_recorded_density_retains_exact_source_and_draft_without_truncation() {
    let files = Files::new();
    let path = files.path();
    let (session, device, stamp) = fixture(AutomationInterpolation::Step);
    let before = session.project().clone();
    let events: Vec<_> = (0..=hex_project::MAX_DEVICE_AUTOMATION_POINTS_PER_LANE as u64)
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
