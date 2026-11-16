use super::super::{Event, Request, Service};
use hex_export::ExportControl;
use hex_project::{BounceAdoption, BounceTarget, ClipId};
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
            "hex-project-folder-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn cancelled_folder_save_reports_the_retained_staging_path() {
    use std::sync::{Arc, atomic::AtomicBool};
    let files = Files::new();
    let parent = files.0.join("Mixed Case");
    fs::create_dir(&parent).unwrap();
    let cancellation = super::RequestCancellation::new(
        Arc::new(AtomicBool::new(true)),
        Arc::new(AtomicU64::new(0)),
        0,
    );
    let error = super::save_as(
        parent.join("Song.hex"),
        false,
        hex_project::Project::new(48_000).unwrap(),
        None,
        &cancellation,
    )
    .err()
    .expect("cancelled save");
    let entries: Vec<_> = fs::read_dir(&parent)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    let [stage] = entries.as_slice() else {
        panic!("one retained staging folder")
    };
    let message = hex_storage::text::plain(&hex_storage::text::lowercase_outside_verbatim(&error));
    assert!(message.contains(&stage.to_string_lossy().to_string()));
    assert!(message.contains("staging path"));
    assert!(stage.join(".hex-project").is_file());
    assert!(!parent.join("Song").exists());
}

fn next(service: &Service) -> Event {
    service
        .events
        .recv_timeout(crate::test_wait::HANG_BOUND)
        .unwrap()
}

fn run(service: &Service, request: Request) -> Event {
    service.submit(request).unwrap();
    let event = next(service);
    assert!(event.error.is_none(), "{:?}", event.error);
    event
}

fn save_as(service: &Service, path: &Path, replace_existing: bool) -> Event {
    run(
        service,
        Request::SaveProjectAs {
            path: path.to_path_buf(),
            replace_existing,
        },
    )
}

fn bounce(service: &Service, clip: ClipId, revision: u64) -> Event {
    run(
        service,
        Request::BounceArrangement {
            project_generation: 0,
            revision,
            target: BounceTarget::Clips(vec![clip]),
            in_place: false,
            adoption: BounceAdoption::NewTracks,
            control: ExportControl::default(),
        },
    )
}

#[test]
fn desktop_save_collects_before_and_after_bounces_and_reopens_a_moved_folder() {
    let files = Files::new();
    let service = Service::with_recovery(
        48_000,
        16,
        2_000_000,
        files.0.join("app/recovery/test.hex-journal"),
    )
    .unwrap();
    assert!(next(&service).error.is_none());
    let external = files.0.join("Library Sample.wav");
    let wav = super::super::service_tests::wav_mono(&[8192; 64]);
    fs::write(&external, &wav).unwrap();
    let imported = run(&service, Request::Import(external.clone()));
    let clip = imported.state.project.tracks()[0].clips()[0].id();
    let bounced = bounce(&service, clip, imported.state.revision);
    let original = bounced.state.project.assets().last().unwrap().path();
    assert!(original.starts_with(files.0.join("app/media/bounces")));
    let original_bytes = fs::read(original).unwrap();

    let root = files.0.join("Song");
    let saved = root.join("Song.hex");
    let first = save_as(&service, &files.0.join("Song.hex"), false);
    assert_eq!(first.state.document_path.as_deref(), Some(saved.as_path()));
    assert!(root.join("samples").is_dir());
    assert!(!files.0.join("Song.hex").exists());
    assert_eq!(first.state.project, bounced.state.project);
    let on_disk = hex_storage::load(&saved).unwrap();
    let samples: Vec<_> = on_disk.assets().map(|asset| asset.path()).collect();
    assert_eq!(samples[0], external);
    assert!(samples[1].starts_with("samples/bounces"));
    assert_eq!(fs::read(root.join(samples[1])).unwrap(), original_bytes);

    let bounced_again = bounce(&service, clip, first.state.revision);
    let new_source = bounced_again.state.project.assets().last().unwrap().path();
    assert!(new_source.starts_with(root.join("samples/bounces")));
    let recorded = super::sample_take_root(Some(&saved)).unwrap();
    assert_eq!(recorded, root.join("samples/recorded"));
    let take = recorded.join("take.wav");
    fs::write(&take, &wav).unwrap();
    run(&service, Request::Import(take));
    let current = run(&service, Request::SaveCurrent);
    assert_eq!(
        current.state.document_path.as_deref(),
        Some(saved.as_path())
    );
    let version = root.join("Song v2.hex");
    let updated = save_as(&service, &version, false);
    assert_eq!(
        updated.state.document_path.as_deref(),
        Some(version.as_path())
    );
    assert!(!root.join("Song v2").exists());
    save_as(&service, &saved, true);
    let moved = files.0.join("Moved Song");
    drop(service);
    fs::rename(&root, &moved).unwrap();
    fs::remove_file(original).unwrap();
    let service = Service::new(48_000, 16, 2_000_000).unwrap();
    assert!(next(&service).error.is_none());
    let opened = run(&service, Request::Open(moved.join("Song v2.hex")));
    assert_eq!(opened.state.project.assets().count(), 4);
    assert_eq!(opened.state.project.tracks().len(), 4);
    for asset in opened.state.project.assets().skip(1).take(2) {
        assert!(asset.path().starts_with("samples/bounces"));
        assert!(moved.join(asset.path()).is_file());
    }
    assert_eq!(
        fs::read(moved.join("samples/recorded/take.wav")).unwrap(),
        wav
    );
    let old_set = files.0.join("Existing.hex");
    run(&service, Request::SaveNew(old_set.clone()));
    assert!(old_set.is_file());
    assert!(!files.0.join("Existing").exists());
}
