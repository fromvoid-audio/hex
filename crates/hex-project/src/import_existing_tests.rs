use crate::{
    AssetId, AudioImportWarp, Command, Created, Editor, ExistingAudioPlacement, Invalid,
    OriginalSourceFacts, Project, SceneId, SessionCellContent, TrackId,
};
use std::path::PathBuf;

fn new_import(track: TrackId, start: u64, path: &str, frames: u64, overwrite: bool) -> Command {
    let path = PathBuf::from(path);
    if overwrite {
        Command::ImportPreparedAudioClipWithOverwrite {
            track,
            start,
            path,
            source_sample_rate: 48_000,
            source_frames: frames,
            playback_frames: frames,
            conversion_version: 0,
        }
    } else {
        Command::ImportPreparedAudioClip {
            track,
            start,
            path,
            source_sample_rate: 48_000,
            source_frames: frames,
            playback_frames: frames,
            conversion_version: 0,
        }
    }
}

fn reuse(asset: AssetId, placement: ExistingAudioPlacement) -> Command {
    Command::ImportExistingAudio { asset, placement }
}

/// One track holding one imported 100-frame clip of `existing.wav`.
fn fixture() -> (Editor, TrackId, AssetId) {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 16).expect("editor");
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "audio".into(),
        })
        .expect("track")
    else {
        panic!("expected a track");
    };
    let Created::Imported { asset, .. } = editor
        .execute(new_import(track, 0, "existing.wav", 100, false))
        .expect("first import")
    else {
        panic!("expected an import");
    };
    (editor, track, asset)
}

fn imports(
    editor: &mut Editor,
    commands: Vec<Command>,
    warp: AudioImportWarp,
) -> Result<Vec<Created>, Invalid> {
    editor
        .execute_audio_imports_with_warp_prepared(commands, warp, |_, _| Ok::<_, Invalid>(()))
        .map(|(created, ())| created)
}

/// Undo keeps the identity allocator's high-water mark by design; compare
/// everything else exactly.
fn content(project: &Project) -> Project {
    let mut project = project.clone();
    project.next_id = 0;
    project
}

fn imported(created: &Created) -> (TrackId, AssetId, crate::ClipId) {
    match created {
        Created::Imported { track, asset, clip } => (*track, *asset, *clip),
        other => panic!("expected an import, got {other:?}"),
    }
}

#[test]
fn mixed_batch_reuses_existing_asset_as_one_undo_step_with_exact_undo_redo() {
    let (mut editor, track, asset) = fixture();
    let before = editor.project().clone();
    let created = imports(
        &mut editor,
        vec![
            reuse(
                asset,
                ExistingAudioPlacement::Clip {
                    track,
                    start: 200,
                    overwrite: false,
                },
            ),
            new_import(track, 300, "new.wav", 50, false),
            reuse(
                asset,
                ExistingAudioPlacement::NewTrack {
                    name: "reused".into(),
                    before_track: None,
                    start: 10,
                },
            ),
        ],
        AudioImportWarp::Off,
    )
    .expect("mixed batch");
    let after = editor.project().clone();
    assert_eq!(created.len(), 3);
    let (first_track, first_asset, first_clip) = imported(&created[0]);
    let (_, new_asset, _) = imported(&created[1]);
    let (new_track, third_asset, third_clip) = imported(&created[2]);
    assert_eq!((first_track, first_asset), (track, asset));
    assert_eq!(third_asset, asset);
    assert_ne!(new_asset, asset);
    // Only the new member added an asset.
    assert_eq!(after.assets().count(), before.assets().count() + 1);
    let clip = after.clip(first_clip).expect("reused clip");
    assert_eq!(
        (
            clip.asset(),
            clip.start(),
            clip.duration(),
            clip.source_length()
        ),
        (asset, 200, 100, 100)
    );
    let clip = after.clip(third_clip).expect("reused new-track clip");
    assert_eq!((clip.asset(), clip.start()), (asset, 10));
    assert_eq!(after.tracks().last().map(|t| t.id()), Some(new_track));

    assert!(editor.undo());
    assert_eq!(content(editor.project()), content(&before));
    assert!(editor.redo());
    assert_eq!(editor.project(), &after);
    // Snapshot reopen preserves the reused references without a new format.
    let reopened = Project::from_snapshot(after.snapshot()).expect("reopen");
    assert_eq!(reopened, after);
}

#[test]
fn reused_overwrite_and_warp_match_new_asset_imports() {
    for warp in [AudioImportWarp::Off, AudioImportWarp::Beats] {
        let (mut reused, track, asset) = fixture();
        let (mut fresh, _, _) = fixture();
        let placement = ExistingAudioPlacement::Clip {
            track,
            start: 40,
            overwrite: true,
        };
        imports(&mut reused, vec![reuse(asset, placement)], warp).expect("reused overwrite");
        imports(
            &mut fresh,
            vec![new_import(track, 40, "existing.wav", 100, true)],
            warp,
        )
        .expect("fresh overwrite");
        let shape = |project: &Project| {
            project.tracks()[0]
                .clips()
                .iter()
                .map(|clip| {
                    (
                        clip.start(),
                        clip.duration(),
                        clip.source_offset(),
                        clip.audio_edit().is_some_and(|e| e.domain.warp_enabled),
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(shape(reused.project()), shape(fresh.project()));
        assert_eq!(reused.project().assets().count(), 1);
        assert_eq!(fresh.project().assets().count(), 2);
    }
}

#[test]
fn console_member_places_existing_asset_without_a_new_asset() {
    let (mut editor, track, asset) = fixture();
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .expect("scene")
    else {
        panic!("expected a scene");
    };
    let before = editor.project().clone();
    let session = |scene: SceneId| {
        reuse(
            asset,
            ExistingAudioPlacement::Session {
                track,
                scene,
                name: "cell".into(),
                source_start: 0,
                source_end: 100,
            },
        )
    };
    assert!(
        imports(&mut editor, vec![session(scene)], AudioImportWarp::Beats).is_err(),
        "console imports stay unwarped"
    );
    assert_eq!(editor.project(), &before);
    imports(&mut editor, vec![session(scene)], AudioImportWarp::Off).expect("console import");
    assert_eq!(editor.project().assets().count(), 1);
    assert!(matches!(
        editor.project().session_cell(track, scene),
        Some(SessionCellContent::Audio(clip)) if clip.asset() == asset
    ));
    assert!(editor.undo());
    assert_eq!(content(editor.project()), content(&before));
}

#[test]
fn single_command_reuse_matches_batch_placement() {
    let (mut editor, track, asset) = fixture();
    let created = editor
        .execute(reuse(
            asset,
            ExistingAudioPlacement::Clip {
                track,
                start: 500,
                overwrite: false,
            },
        ))
        .expect("single reuse");
    let (_, reused, clip) = imported(&created);
    assert_eq!(reused, asset);
    assert_eq!(editor.project().assets().count(), 1);
    assert_eq!(editor.project().clip(clip).map(|c| c.start()), Some(500));
}

#[test]
fn missing_offline_and_device_assets_are_refused_atomically() {
    let (editor, track, asset) = fixture();
    let placement = ExistingAudioPlacement::Clip {
        track,
        start: 200,
        overwrite: false,
    };
    let batch = |asset: AssetId| {
        vec![
            new_import(track, 400, "new.wav", 10, false),
            reuse(asset, placement.clone()),
        ]
    };
    let refuse = |project: Project, asset: AssetId| {
        assert!(project.existing_import_frames(asset).is_err());
        let mut editor = Editor::new(project, 8).expect("editor");
        let before = editor.project().clone();
        assert!(imports(&mut editor, batch(asset), AudioImportWarp::Off).is_err());
        assert!(editor.execute(reuse(asset, placement.clone())).is_err());
        assert_eq!(editor.project(), &before);
        assert!(!editor.can_undo());
    };

    assert_eq!(
        editor.project().existing_import_frames(asset).ok(),
        Some(100)
    );
    refuse(editor.project().clone(), AssetId(u64::MAX));

    let offline = editor
        .project()
        .clone()
        .with_offline_assets([asset])
        .expect("offline");
    refuse(offline, asset);

    // An otherwise unused device-original source isolates the import refusal
    // from the clip-derivative validation that runs on every candidate.
    let mut device = editor.project().clone();
    let source = device
        .add_prepared_asset(PathBuf::from("device.wav"), 48_000, 64, 64, 0)
        .expect("device asset");
    device
        .assets
        .get_mut(&source)
        .expect("device asset")
        .original_source =
        Some(OriginalSourceFacts::new(OriginalSourceFacts::LAW_VERSION, [7; 32]).expect("facts"));
    refuse(device, source);
}
