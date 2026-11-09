use super::completed_tests::changed_metadata;
use super::*;

pub(super) fn vst3_fixture() -> (Project, DraftTarget, DraftProvenance) {
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
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state,
        })
        .unwrap()
    else {
        panic!("device")
    };
    let (_, _, provenance) = fixture();
    (
        editor.project().clone(),
        DraftTarget::Device {
            device,
            target: DeviceAutomationTarget::Vst3 { parameter: 0 },
        },
        provenance,
    )
}

#[test]
fn clocked_vst3_zero_target_preserves_class_values_clocks_and_completed_take() {
    let (source, target, provenance) = vst3_fixture();
    assert_eq!(target.vst3_class_in_source(&source).unwrap(), Some([9; 16]));
    for loop_region in [
        None,
        Some(hex_project::LoopRegion::new(64, 128, true).unwrap()),
    ] {
        let dir = TestDir::new();
        let path = dir.path();
        let capture = CaptureClock {
            sample_rate: 48_000,
            loop_region,
        };
        let (first, second, end_content, pass) = if loop_region.is_some() {
            (126, 64, 66, 1)
        } else {
            (0, 2, 4, 0)
        };
        let events = [
            AppliedEvent {
                sequence: 1,
                performance_frame: 0,
                content_frame: first,
                loop_pass: 0,
                value: AutomationValue::new(1.0 / 3.0).unwrap(),
            },
            AppliedEvent {
                sequence: 2,
                performance_frame: 2,
                content_frame: second,
                loop_pass: pass,
                value: AutomationValue::new(0.75).unwrap(),
            },
        ];
        let end = DraftClock {
            performance_frame: 4,
            content_frame: end_content,
            loop_pass: pass,
        };
        let mut writer =
            DraftWriter::create_with_clock(&path, &source, target, provenance, capture).unwrap();
        writer.append(&events).unwrap();
        writer.finish(DraftTerminal::Finished(end)).unwrap();
        drop(writer);
        let original = fs::read(&path).unwrap();
        assert_eq!(u32_at(&original, 8), VST3_CLOCK_VERSION);
        let mut completed = CompletedDraft::open(&path, limits()).unwrap();
        assert_eq!(completed.target(), target);
        assert_eq!(completed.source(), &source);
        assert_eq!(completed.provenance(), provenance);
        assert_eq!(completed.capture_clock(), Some(capture));
        assert_eq!(completed.end_clock(), end);
        let mut page = [None; 3];
        assert_eq!(completed.read_events(0, &mut page).unwrap(), 2);
        assert_eq!(page, [Some(events[0]), Some(events[1]), None]);
        let take = CompletedAutomationTake::from_completed_on_control(completed).unwrap();
        assert_eq!(take.source(), &source);
        assert_eq!(take.target(), target);
        assert_eq!(
            take.target().vst3_class_in_source(take.source()).unwrap(),
            Some([9; 16])
        );
        assert_eq!(take.event_count(), 2);
        assert_eq!(take.end_clock(), end);
        drop(take);
        assert_eq!(fs::read(&path).unwrap(), original);
    }
}

#[test]
fn vst3_requires_known_clock_valid_source_and_raw_identity_before_publication() {
    let (source, target, provenance) = vst3_fixture();
    let dir = TestDir::new();
    let path = dir.path();
    let capture = CaptureClock {
        sample_rate: 48_000,
        loop_region: None,
    };
    assert!(DraftWriter::create_new(&path, &source, target, provenance).is_err());
    assert!(!path.exists());
    assert!(
        DraftWriter::create_with_clock(
            &path,
            &Project::new(48_000).unwrap(),
            target,
            provenance,
            capture
        )
        .is_err()
    );
    assert!(!path.exists());
    let DraftTarget::Device { device, .. } = target else {
        panic!("device")
    };
    let invalid = DraftTarget::Device {
        device,
        target: DeviceAutomationTarget::Vst3 {
            parameter: u32::MAX,
        },
    };
    assert!(DraftWriter::create_with_clock(&path, &source, invalid, provenance, capture).is_err());
    assert!(!path.exists());
    let mut editor = Editor::new(source, 4).unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap();
    assert!(target.vst3_class_in_source(editor.project()).is_err());
    assert!(
        DraftWriter::create_with_clock(&path, editor.project(), target, provenance, capture)
            .is_err()
    );
    assert!(!path.exists());
}

#[test]
fn legacy_headers_never_reinterpret_vst3_and_malformed_headers_preserve_original_bytes() {
    let (source, target, provenance) = vst3_fixture();
    let dir = TestDir::new();
    let path = dir.path();
    let capture = CaptureClock {
        sample_rate: 48_000,
        loop_region: None,
    };
    drop(DraftWriter::create_with_clock(&path, &source, target, provenance, capture).unwrap());
    let original = fs::read(&path).unwrap();
    let meta = metadata(&source, target, provenance).unwrap();
    for version in [VERSION, CLOCK_VERSION] {
        assert!(resolve_target(&source, &meta, version).is_err());
    }
    let changes: &[fn(&mut serde_json::Value)] = &[
        |v| v["metadata"]["parameter"] = serde_json::json!(u32::MAX),
        |v| v["metadata"]["owner"] = serde_json::json!(u64::MAX),
        |v| v["metadata"]["target_kind"] = serde_json::json!(1),
        |v| v["metadata"]["unknown"] = true.into(),
        |v| v["capture_clock"]["sample_rate"] = 0.into(),
    ];
    for change in changes {
        let malformed = changed_metadata(&original, VST3_CLOCK_VERSION, change);
        fs::write(&path, &malformed).unwrap();
        assert!(DraftReader::open(&path, limits()).is_err());
        assert_eq!(fs::read(&path).unwrap(), malformed);
    }
    for version in [CLOCK_VERSION, VST3_GROUP_VERSION + 1] {
        let refused = changed_metadata(&original, version, |_| {});
        fs::write(&path, &refused).unwrap();
        assert!(DraftReader::open(&path, limits()).is_err());
        assert_eq!(fs::read(&path).unwrap(), refused);
    }
    fs::write(&path, &original).unwrap();
    let reader = DraftReader::open(&path, limits()).unwrap();
    assert_eq!(reader.outcome(), DraftOutcome::Interrupted);
    assert_eq!(reader.target(), target);
}
