//! Signed stopped Save refusal and retry against the original worker.
use super::*;

#[test]
#[ignore = "explicit original SDK helper/source receipts required"]
fn stopped_save_capture_overflow_preserves_existing_file_and_retries_original() {
    const CHILD: &str = "HEX_DESKTOP_STOPPED_SAVE_OVERFLOW_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                std::thread::current().name().unwrap(),
                "--ignored",
                "--nocapture",
            ])
            .env(CHILD, "1")
            // Valid startup state, an oversized first fresh capture, then
            // normal capture on the same component.
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "95")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "{stdout}\n{stderr}"
        );
        return;
    }

    let _native = native_fixture();
    let (mut h, files, before) = fixture(Start::Unsaved);
    let original = h.plugin_progress.lock().unwrap().binding.unwrap();
    let destination = files.0.join("existing.hex");
    let existing = hex_project::Project::new(48_000).unwrap();
    hex_storage::save_new(&destination, &existing).unwrap();
    let bytes = fs::read(&destination).unwrap();

    h.service
        .submit(Request::SaveProjectAs {
            path: destination.clone(),
            replace_existing: true,
        })
        .unwrap();
    let refused = h.next();
    assert!(refused.save_complete);
    assert!(
        refused.error.as_deref().is_some_and(
            |error| error.starts_with("vst3 state capture refused: plugin state exceeds")
        ),
        "{:?}",
        refused.error,
    );
    assert_eq!(fs::read(&destination).unwrap(), bytes);
    assert_eq!(hex_storage::load(&destination).unwrap(), existing);
    assert_eq!(refused.state.project, before.project);
    assert_eq!(refused.state.revision, before.revision);
    assert!(refused.state.unsaved);
    assert_eq!(h.plugin_progress.lock().unwrap().binding, Some(original));

    let recovered_path = files.0.join("recovered.hex");
    let saved = h.run(Request::SaveNew(recovered_path.clone()));
    assert!(
        saved.save_complete && saved.error.is_none(),
        "{:?}",
        saved.error
    );
    assert_eq!(saved.state.revision, before.revision);
    assert_eq!(h.plugin_progress.lock().unwrap().binding, Some(original));
    assert_eq!(h.plugin_completed.load(Acquire), 0);
    assert_eq!(
        hex_storage::load(&recovered_path).unwrap().main_devices(),
        before.project.main_devices(),
    );
    assert_eq!(fs::read(destination).unwrap(), bytes);
}

#[test]
#[ignore = "explicit original SDK helper/source receipts required"]
fn stopped_save_discards_only_the_group_rejected_at_the_editor_capacity() {
    save_with_group_pressure(true);
}

#[test]
#[ignore = "explicit original SDK helper/source receipts required"]
fn stopped_save_refuses_an_unsettled_editor_group_then_retries_its_terminal() {
    save_with_group_pressure(false);
}

fn save_with_group_pressure(capacity_exceeded: bool) {
    use hex_audio::plugin_worker::ParameterEditKind;

    const CHILD: &str = "HEX_DESKTOP_STOPPED_SAVE_EDIT_PRESSURE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let files = Files::new();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                std::thread::current().name().unwrap(),
                "--ignored",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "148")
            .env(
                "HEX_ORIGINAL_GROUPED_EDITOR_PATTERN",
                if capacity_exceeded {
                    "overflow"
                } else {
                    "held"
                },
            )
            .env("HEX_ORIGINAL_INSTANCE_RELEASE", files.0.join("edit"))
            .env("HEX_ORIGINAL_INSTANCE_TRACE", files.0.join("trace"))
            .env("HEX_ORIGINAL_EDITOR_END_RELEASE", files.0.join("end"))
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "{stdout}\n{stderr}"
        );
        return;
    }

    let _native = native_fixture();
    let (mut h, files, before) =
        fixture_with_host(Start::Unsaved, 2048, host().with_grouped_editor_callbacks());
    let original = h.plugin_progress.lock().unwrap().binding.unwrap();
    let path = files.0.join("pressure.hex");
    let (ready, resume) = h.probe_next_save_capture();
    h.service.submit(Request::SaveNew(path.clone())).unwrap();
    ready.recv_timeout(Duration::from_secs(2)).unwrap();
    fs::write(
        std::env::var_os("HEX_ORIGINAL_INSTANCE_RELEASE").unwrap(),
        b"fill editor group",
    )
    .unwrap();
    let trace = PathBuf::from(std::env::var_os("HEX_ORIGINAL_INSTANCE_TRACE").unwrap());
    wait(|| fs::read(&trace).is_ok_and(|data| data.starts_with(&25_i32.to_le_bytes())));
    resume.send(()).unwrap();
    let mut saved = h.next();
    if !capacity_exceeded {
        assert!(saved.save_complete);
        assert_eq!(
            saved.error.as_deref(),
            Some("vst3 editor group is still open · finish editing, then try save again"),
        );
        assert!(!path.exists());
        assert_eq!(saved.state.project, before.project);
        assert_eq!(h.plugin_progress.lock().unwrap().binding, Some(original));
        fs::write(
            std::env::var_os("HEX_ORIGINAL_EDITOR_END_RELEASE").unwrap(),
            b"complete the accepted group",
        )
        .unwrap();
        wait(|| {
            h.pump();
            h.plugin_progress
                .lock()
                .unwrap()
                .editor_receipts
                .iter()
                .flatten()
                .any(|event| event.kind == ParameterEditKind::GroupEnd)
        });
        saved = h.run(Request::SaveNew(path.clone()));
    }
    assert!(
        saved.save_complete && saved.error.is_none(),
        "{:?}",
        saved.error
    );
    assert_eq!(saved.state.revision, before.revision);
    let observation = *h.plugin_progress.lock().unwrap();
    assert_eq!(observation.binding, Some(original));
    let kinds = observation
        .editor_receipts
        .iter()
        .flatten()
        .map(|event| event.kind)
        .collect::<Vec<_>>();
    let stored = hex_storage::load(path).unwrap();
    if capacity_exceeded {
        assert_eq!(
            kinds,
            [
                ParameterEditKind::GroupBegin,
                ParameterEditKind::Begin,
                ParameterEditKind::Begin,
                ParameterEditKind::End,
                ParameterEditKind::End,
                ParameterEditKind::GroupAborted,
            ],
        );
        assert_eq!(stored.main_devices(), before.project.main_devices());
    } else {
        assert!(kinds.contains(&ParameterEditKind::GroupEnd), "{kinds:?}");
        let captured = hex_plugin_state::decode(stored.main_devices()[0].state()).unwrap();
        assert_eq!(&captured.component.unwrap()[8..12], &3.0_f32.to_le_bytes());
    }
    assert_eq!(h.plugin_completed.load(Acquire), 0);
}
