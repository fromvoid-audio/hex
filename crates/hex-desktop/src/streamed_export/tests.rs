//! Unsupported scalar offline capture refuses before resolving any file.
use super::*;
use hex_project::{Command, Created, Editor, Project};

#[test]
fn scalar_offline_capture_refuses_before_source_resolution() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "parameter".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: hex_engine::default_native_device_state(hex_engine::DeviceTypeId::from_static(
                "hex.utility",
            ))
            .unwrap(),
        })
        .unwrap()
    else {
        panic!("device")
    };
    let source = hex_engine::ScalarSourceMetadata::new(
        hex_engine::ScalarSourceKey::new(1, 1, 1).unwrap(),
        hex_engine::ScalarSourceTarget::Device {
            device,
            target: hex_project::DeviceAutomationTarget::HostInputGain,
        },
        48_000,
        0,
        4,
        hex_engine::ScalarProjectionIdentity {
            version: hex_project::AUTOMATION_PROJECTION_VERSION,
            fingerprint: hex_storage::automation_draft::automation_projection_fingerprint(
                editor.project().tempo_map(),
            ),
        },
        hex_engine::ScalarFileIdentity {
            format_version: 2,
            file_bytes: 100,
            terminal_chain: [1; 32],
        },
    )
    .unwrap();
    let prepared = hex_engine::PreparedRevision::prepare_with_scalar_automation(
        editor.project(),
        &std::collections::BTreeMap::new(),
        Arc::new(hex_engine::SineTable::new()),
        None,
        hex_engine::ScalarAutomationBinding {
            device,
            target: hex_project::DeviceAutomationTarget::HostInputGain,
            source,
        },
    )
    .unwrap();
    let reads = std::cell::Cell::new(0);
    let result = VerifiedPages::capture_files(
        &prepared,
        |_| {
            reads.set(reads.get() + 1);
            Err(ExportError::Invalid("unexpected source resolution"))
        },
        None,
        StreamConfig::with_pool_pages(3),
        &AtomicBool::new(false),
    );
    assert_eq!(reads.get(), 0);
    match result {
        Err(ExportError::Invalid(reason)) => {
            assert_eq!(reason, "offline scalar source capture is not prepared")
        }
        _ => panic!("scalar capture must refuse explicitly"),
    }
}
