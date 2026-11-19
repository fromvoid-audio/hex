//! Real durable draft overflow and custody at the Session/Storage boundary.
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, Command, Created,
    DeviceAutomationTarget, Editor, Project,
};
use hex_session::{Session, service::automation_take::prepare_device_take_adoption_on_control};
use hex_storage::automation_draft::{
    AppliedEvent, CaptureClock, CompletedAutomationTake, DraftClock, DraftProvenance,
    DraftReadLimits, DraftTarget, DraftTerminal, DraftWriter, MAX_APPEND_EVENTS,
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "hex-device-take-density-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn over_budget_recorded_density_retains_exact_source_and_draft_without_truncation() {
    let files = Files::new();
    let path = files.0.join("capture.hex-automation-draft");
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
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state,
        })
        .unwrap()
    else {
        panic!("device")
    };
    let target = DeviceAutomationTarget::Vst3 { parameter: 0 };
    editor
        .execute(Command::ReplaceDeviceAutomationTarget {
            device,
            target,
            points: vec![
                AutomationPoint {
                    beat: BeatTime::ZERO,
                    value: AutomationValue::new(0.1).unwrap(),
                    outgoing: AutomationInterpolation::Step,
                },
                AutomationPoint {
                    beat: BeatTime::ONE,
                    value: AutomationValue::new(0.9).unwrap(),
                    outgoing: AutomationInterpolation::Step,
                },
            ],
        })
        .unwrap();
    let session = Session::from_loaded(editor.project().clone(), 16, 4096, vec![]).unwrap();
    let before = session.project().clone();
    let stamp = DraftProvenance {
        sample_rate: 48_000,
        output_identity: 9,
        project_generation: 0,
        editor_revision: 2,
        audio_revision: 7,
        transport_epoch: 1,
        gesture_serial: 1,
    };
    let events: Vec<_> = (0..=hex_project::MAX_DEVICE_AUTOMATION_TOUCH_STEPS_PER_LANE as u64)
        .map(|frame| AppliedEvent {
            sequence: frame + 1,
            performance_frame: frame,
            content_frame: frame,
            loop_pass: 0,
            value: AutomationValue::new((frame % 2) as f32).unwrap(),
        })
        .collect();
    let mut writer = DraftWriter::create_with_clock(
        &path,
        session.project(),
        DraftTarget::Device { device, target },
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
    let end = events.len() as u64 + 1;
    writer
        .finish(DraftTerminal::Finished(DraftClock {
            performance_frame: end,
            content_frame: end,
            loop_pass: 0,
        }))
        .unwrap();
    drop(writer);
    let mut take = CompletedAutomationTake::open_on_control(
        &path,
        DraftReadLimits {
            max_source_bytes: hex_storage::MAX_PROJECT_DOCUMENT_BYTES as u64,
        },
    )
    .unwrap();
    let original = fs::read(&path).unwrap();
    let error =
        prepare_device_take_adoption_on_control(&session, None, &mut take, stamp).unwrap_err();
    assert!(error.contains("touch point budget"), "{error}");
    assert_eq!(session.project(), &before);
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(take.event_count(), events.len() as u64);
}
