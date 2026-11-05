//! Real files in an isolated temp folder; the user's media is never touched.
use super::*;
use crate::service::{Event, Request, Service};
use std::{fs, sync::atomic::AtomicU64};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Files(PathBuf);

impl Files {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hex-missing-media-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }

    fn wav(&self, relative: &str, rate: u32, frames: u32) -> PathBuf {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut bytes = Vec::from(*b"RIFF");
        bytes.extend_from_slice(&(36 + frames * 2).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(frames * 2).to_le_bytes());
        for frame in 0..frames {
            bytes.extend_from_slice(&((frame % 64) as i16 * 64).to_le_bytes());
        }
        fs::write(&path, bytes).unwrap();
        path
    }
}

impl Drop for Files {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Distinct real asset IDs; the type has no public constructor.
fn asset_id(index: u64) -> AssetId {
    let mut editor = hex_project::Editor::new(Project::new(48_000).unwrap(), 1).unwrap();
    let mut last = None;
    for _ in 0..index {
        let hex_project::Created::Asset(id) = editor
            .execute(hex_project::Command::AddAsset {
                path: "x.wav".into(),
                sample_rate: 48_000,
                frames: 1,
            })
            .unwrap()
        else {
            panic!("asset")
        };
        last = Some(id);
    }
    last.unwrap()
}

fn item(id: u64, path: PathBuf, frames: u64) -> MissingMedia {
    MissingMedia {
        asset: asset_id(id),
        expected_path: path,
        source_frames: frames,
        source_sample_rate: 48_000,
        device_original: false,
        usages: Vec::new(),
        usage_count: 0,
    }
}

fn search(missing: &[MissingMedia], roots: &[PathBuf], limits: SearchLimits) -> SearchOutcome {
    search_folders(
        missing,
        roots,
        limits,
        &AtomicBool::new(false),
        &AtomicUsize::new(0),
    )
}

#[test]
fn candidate_fit_flags_rate_and_length_mismatches() {
    let files = Files::new();
    let expected = item(1, files.0.join("old/kick.wav"), 100);
    let good = files.wav("new/kick.wav", 48_000, 100);
    let short = files.wav("short/kick.wav", 48_000, 99);
    let rate = files.wav("rate/kick.wav", 44_100, 100);
    assert_eq!(check_candidate(&expected, &good), CandidateFit::Matches);
    assert!(
        matches!(check_candidate(&expected, &short), CandidateFit::Mismatch(m) if m.contains("length"))
    );
    assert!(
        matches!(check_candidate(&expected, &rate), CandidateFit::Mismatch(m) if m.contains("rate"))
    );
    assert!(!check_candidate(&expected, &files.0.join("none.wav")).usable());
    fs::write(files.0.join("text.wav"), b"not audio").unwrap();
    let unverified = check_candidate(&expected, &files.0.join("text.wav"));
    assert!(matches!(unverified, CandidateFit::Unverified(_)));
}

#[test]
fn relative_resolve_maps_siblings_under_a_moved_root() {
    let files = Files::new();
    let old = files.0.join("old-library");
    let missing = vec![
        item(1, old.join("drums/kick.wav"), 10),
        item(2, old.join("drums/snare.wav"), 20),
        item(3, old.join("keys/pad.wav"), 30),
        item(4, old.join("keys/wrong.wav"), 40),
        item(5, files.0.join("elsewhere/hat.wav"), 50),
    ];
    let picked = files.wav("library/drums/kick.wav", 48_000, 10);
    files.wav("library/drums/snare.wav", 48_000, 20);
    files.wav("library/keys/pad.wav", 48_000, 30);
    files.wav("library/keys/wrong.wav", 48_000, 41);
    files.wav("library/hat.wav", 48_000, 50);
    let mapped = resolve_relative(&missing, &BTreeMap::new(), &missing[0], &picked);
    assert_eq!(
        mapped,
        vec![
            (missing[1].asset, files.0.join("library/drums/snare.wav")),
            (missing[2].asset, files.0.join("library/keys/pad.wav")),
        ]
    );
    let already = BTreeMap::from([(missing[1].asset, PathBuf::from("/chosen"))]);
    let renamed = files.wav("renamed/kick-v2.wav", 48_000, 10);
    files.wav("renamed/snare.wav", 48_000, 20);
    assert!(resolve_relative(&missing, &already, &missing[0], &renamed).is_empty());
}

#[test]
fn folder_search_reports_unique_ambiguous_and_mismatched_candidates() {
    let files = Files::new();
    let missing = vec![
        item(1, PathBuf::from("/gone/Kick.wav"), 10),
        item(2, PathBuf::from("/gone/snare.wav"), 20),
        item(3, PathBuf::from("/gone/pad.wav"), 30),
        item(4, PathBuf::from("/gone/absent.wav"), 40),
    ];
    let kick = files.wav("a/deep/KICK.WAV", 48_000, 10);
    files.wav("a/snare.wav", 48_000, 20);
    files.wav("b/snare.wav", 48_000, 20);
    files.wav("b/pad.wav", 48_000, 31);
    let outcome = search(
        &missing,
        std::slice::from_ref(&files.0),
        SearchLimits::default(),
    );
    assert_eq!(outcome.stopped, None);
    assert_eq!(outcome.unique_match(missing[0].asset), Some(kick.as_path()));
    assert_eq!(outcome.candidates[&missing[1].asset].len(), 2);
    assert_eq!(outcome.unique_match(missing[1].asset), None);
    assert!(matches!(
        outcome.candidates[&missing[2].asset][0].1,
        CandidateFit::Mismatch(_)
    ));
    assert_eq!(outcome.unique_match(missing[2].asset), None);
    assert!(!outcome.candidates.contains_key(&missing[3].asset));
}

#[test]
fn proposed_candidate_lists_resolve_exact_ambiguous_mismatched_and_none() {
    let files = Files::new();
    let missing = vec![
        item(1, PathBuf::from("/gone/Kick.wav"), 10),
        item(2, PathBuf::from("/gone/snare.wav"), 20),
        item(3, PathBuf::from("/gone/pad.wav"), 30),
        item(4, PathBuf::from("/gone/absent.wav"), 40),
    ];
    let kick = files.wav("a/KICK.WAV", 48_000, 10);
    let snares = [
        files.wav("a/snare.wav", 48_000, 20),
        files.wav("b/snare.wav", 48_000, 20),
    ];
    let pad = files.wav("b/pad.wav", 44_100, 30);
    let renamed = files.wav("b/other.wav", 48_000, 10);
    let lists = BTreeMap::from([
        // A differently named catalog proposal is not this matcher's business.
        (missing[0].asset, vec![renamed, kick.clone()]),
        (missing[1].asset, snares.to_vec()),
        (missing[2].asset, vec![pad]),
        (missing[3].asset, vec![files.0.join("gone/absent.wav")]),
    ]);
    let cancel = AtomicBool::new(false);
    let outcome = check_candidate_lists(&missing, &lists, 8, &cancel);
    assert_eq!(outcome.stopped, None);
    assert_eq!(outcome.scanned, 5);
    assert_eq!(outcome.unique_match(missing[0].asset), Some(kick.as_path()));
    assert_eq!(outcome.candidates[&missing[0].asset].len(), 1);
    assert_eq!(outcome.unique_match(missing[1].asset), None);
    assert_eq!(outcome.candidates[&missing[1].asset].len(), 2);
    assert!(matches!(
        outcome.candidates[&missing[2].asset][0].1,
        CandidateFit::Mismatch(_)
    ));
    assert!(matches!(
        outcome.candidates[&missing[3].asset][0].1,
        CandidateFit::Unusable(_)
    ));
    assert_eq!(outcome.unreadable, 1);

    let capped = check_candidate_lists(&missing[1..2], &lists, 1, &cancel);
    assert_eq!(capped.candidates[&missing[1].asset].len(), 1);
    cancel.store(true, Ordering::Relaxed);
    let stopped = check_candidate_lists(&missing, &lists, 8, &cancel);
    assert_eq!(stopped.stopped, Some(SearchStop::Cancelled));
    assert!(stopped.candidates.is_empty());
}

#[test]
fn folder_search_is_bounded_and_cancellable() {
    let files = Files::new();
    let missing = vec![item(1, PathBuf::from("/gone/kick.wav"), 10)];
    files.wav("1/2/3/kick.wav", 48_000, 10);
    for i in 0..5 {
        files.wav(&format!("flat/{i}.wav"), 48_000, 1);
    }
    let shallow = search(
        &missing,
        std::slice::from_ref(&files.0),
        SearchLimits {
            max_depth: 2,
            ..SearchLimits::default()
        },
    );
    assert!(shallow.candidates.is_empty());
    assert_eq!(shallow.depth_skipped, 1);
    let capped = search(
        &missing,
        &[files.0.join("flat")],
        SearchLimits {
            max_entries: 3,
            ..SearchLimits::default()
        },
    );
    assert_eq!(
        (capped.scanned, capped.stopped),
        (3, Some(SearchStop::EntryLimit))
    );
    let progress = AtomicUsize::new(0);
    let cancelled = search_folders(
        &missing,
        std::slice::from_ref(&files.0),
        SearchLimits::default(),
        &AtomicBool::new(true),
        &progress,
    );
    assert_eq!(cancelled.stopped, Some(SearchStop::Cancelled));
    assert_eq!(progress.load(Ordering::Relaxed), 0);
    let unreadable = search(
        &missing,
        &[files.0.join("no-such-root")],
        SearchLimits::default(),
    );
    assert_eq!((unreadable.unreadable, unreadable.stopped), (1, None));
}

#[cfg(unix)]
#[test]
fn folder_search_survives_symlink_cycles() {
    let files = Files::new();
    let missing = vec![item(1, PathBuf::from("/gone/kick.wav"), 10)];
    files.wav("lib/kick.wav", 48_000, 10);
    std::os::unix::fs::symlink(files.0.join("lib"), files.0.join("lib/loop")).unwrap();
    std::os::unix::fs::symlink(files.0.join("lib"), files.0.join("alias")).unwrap();
    let outcome = search(
        &missing,
        std::slice::from_ref(&files.0),
        SearchLimits::default(),
    );
    assert_eq!(outcome.stopped, None);
    // "alias" sorts first, so the shared directory is visited once through it.
    let found = outcome.unique_match(missing[0].asset).unwrap();
    assert_eq!(found, files.0.join("alias/kick.wav"));
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

#[test]
fn moved_media_is_reported_found_and_relinked_on_open_then_saved() {
    let files = Files::new();
    let original = files.wav("old/drums/kick.wav", 48_000, 256);
    let service = Service::new(48_000, 16, 1 << 20).unwrap();
    let _ = next(&service);
    let imported = run(&service, Request::Import(original.clone()));
    let asset = imported.state.project.assets().next().unwrap().id();
    let saved = files.0.join("song.hex");
    run(&service, Request::SaveNew(saved.clone()));
    let saved_bytes = fs::read(&saved).unwrap();

    let moved = files.0.join("new/drums/kick.wav");
    fs::create_dir_all(moved.parent().unwrap()).unwrap();
    fs::rename(&original, &moved).unwrap();
    let media_bytes = fs::read(&moved).unwrap();

    let source = ProjectSource::Saved(saved.clone());
    let report = missing_media_report(&source).unwrap();
    assert_eq!(report.len(), 1);
    assert_eq!(
        (report[0].asset, &report[0].expected_path),
        (asset, &original)
    );
    assert_eq!(report[0].usage_count, 1);
    assert!(matches!(
        &report[0].usages[0],
        MediaUsage::ArrangementClip { .. }
    ));

    // Opening without a relink keeps the asset offline instead of failing.
    let offline = run(&service, Request::Open(saved.clone()));
    assert!(offline.project_replaced && offline.error.is_none());
    assert!(offline.state.project.asset_offline(asset));
    assert_eq!(offline.state.project.asset(asset).unwrap().path(), original);

    let outcome = search(&report, &[files.0.join("new")], SearchLimits::default());
    let found = outcome.unique_match(asset).unwrap().to_path_buf();
    assert_eq!(found, moved);
    let opened = run(
        &service,
        Request::OpenWith {
            source,
            relinks: BTreeMap::from([(asset, found)]),
        },
    );
    assert!(opened.project_replaced && opened.state.unsaved);
    assert_eq!(opened.state.project.asset(asset).unwrap().path(), moved);
    // Opening and relinking stay in memory until a deliberate save.
    assert_eq!(fs::read(&saved).unwrap(), saved_bytes);

    run(&service, Request::SaveCurrent);
    assert!(
        missing_media_report(&ProjectSource::Saved(saved.clone()))
            .unwrap()
            .is_empty()
    );
    let reopened = run(&service, Request::Open(saved));
    assert_eq!(reopened.state.project.asset(asset).unwrap().path(), moved);
    assert_eq!(fs::read(&moved).unwrap(), media_bytes);
}

#[test]
fn missing_media_opens_offline_keeps_clips_and_saves_original_paths() {
    let files = Files::new();
    let kick = files.wav("old/kick.wav", 48_000, 256);
    let snare = files.wav("old/snare.wav", 48_000, 128);
    let service = Service::new(48_000, 16, 1 << 20).unwrap();
    let _ = next(&service);
    run(&service, Request::Import(kick.clone()));
    let imported = run(&service, Request::Import(snare.clone()));
    let id_of = |project: &Project, path: &Path| {
        project
            .assets()
            .find(|asset| asset.path() == path)
            .unwrap()
            .id()
    };
    let kick_id = id_of(&imported.state.project, &kick);
    let snare_id = id_of(&imported.state.project, &snare);
    let clips = |project: &Project| {
        project
            .tracks()
            .iter()
            .flat_map(|track| track.clips())
            .count()
    };
    let clip_count = clips(&imported.state.project);
    assert_eq!(clip_count, 2);
    let saved = files.0.join("song.hex");
    run(&service, Request::SaveNew(saved.clone()));
    fs::remove_file(&kick).unwrap();

    let opened = run(&service, Request::Open(saved.clone()));
    assert!(opened.error.is_none(), "{:?}", opened.error);
    assert!(opened.project_replaced);
    let project = &opened.state.project;
    assert_eq!(project.offline_assets().collect::<Vec<_>>(), [kick_id]);
    assert!(!project.asset_offline(snare_id));
    assert_eq!(clips(project), clip_count, "offline clips are kept");
    assert_eq!(project.asset(kick_id).unwrap().path(), kick);

    // Saving keeps the original path; reopening still finds it missing.
    run(&service, Request::SaveCurrent);
    let report = missing_media_report(&ProjectSource::Saved(saved.clone())).unwrap();
    assert_eq!(report.len(), 1);
    assert_eq!(report[0].expected_path, kick);
    let reopened = run(&service, Request::Open(saved));
    assert!(reopened.state.project.asset_offline(kick_id));
    assert_eq!(clips(&reopened.state.project), clip_count);
    assert!(!kick.exists(), "opening never recreates missing media");
}

#[test]
fn offline_media_relinks_in_place_as_one_undo_step_and_saves_new_paths() {
    let files = Files::new();
    let kick = files.wav("old/kick.wav", 48_000, 256);
    let service = Service::new(48_000, 16, 1 << 20).unwrap();
    let _ = next(&service);
    let imported = run(&service, Request::Import(kick.clone()));
    let asset = imported.state.project.assets().next().unwrap().id();
    let saved = files.0.join("song.hex");
    run(&service, Request::SaveNew(saved.clone()));
    let moved = files.0.join("new/kick.wav");
    fs::create_dir_all(moved.parent().unwrap()).unwrap();
    fs::rename(&kick, &moved).unwrap();
    let wrong = files.wav("new/short.wav", 48_000, 100);

    let opened = run(&service, Request::Open(saved.clone()));
    assert!(opened.state.project.asset_offline(asset));
    let offline_project = opened.state.project.clone();

    // A file whose length differs is refused and changes nothing.
    service
        .submit(Request::RelinkMedia(BTreeMap::from([(asset, wrong)])))
        .unwrap();
    let refused = next(&service);
    assert!(
        refused
            .error
            .as_deref()
            .is_some_and(|e| e.contains("rate or frame count")),
        "{:?}",
        refused.error
    );
    assert_eq!(refused.state.project, offline_project);

    let relinked = run(
        &service,
        Request::RelinkMedia(BTreeMap::from([(asset, moved.clone())])),
    );
    assert!(relinked.state.unsaved);
    assert!(!relinked.state.project.asset_offline(asset));
    assert_eq!(relinked.state.project.asset(asset).unwrap().path(), moved);

    let undone = run(&service, Request::Undo);
    assert!(undone.state.project.asset_offline(asset));
    assert_eq!(undone.state.project.asset(asset).unwrap().path(), kick);
    let redone = run(&service, Request::Redo);
    assert!(!redone.state.project.asset_offline(asset));
    assert_eq!(redone.state.project.asset(asset).unwrap().path(), moved);

    run(&service, Request::SaveCurrent);
    assert!(
        missing_media_report(&ProjectSource::Saved(saved.clone()))
            .unwrap()
            .is_empty()
    );
    let reopened = run(&service, Request::Open(saved));
    assert!(!reopened.state.project.asset_offline(asset));
    assert_eq!(reopened.state.project.asset(asset).unwrap().path(), moved);
}
