use hex_project::{Command, Created, Editor, Project};
use std::path::PathBuf;

fn imported() -> (Project, hex_project::AssetId, hex_project::AssetId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let mut assets = Vec::new();
    for name in ["kick", "snare"] {
        let Created::Imported { asset, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: name.into(),
                path: PathBuf::from(format!("/gone/{name}.wav")),
                sample_rate: 48_000,
                frames: 64,
            })
            .unwrap()
        else {
            panic!("import");
        };
        assets.push(asset);
    }
    (editor.project().clone(), assets[0], assets[1])
}

#[test]
fn offline_assets_are_runtime_only_kept_through_edits_and_history() {
    let (project, kick, snare) = imported();
    let saved = project.snapshot();
    let offline = project.clone().with_offline_assets([kick]).unwrap();
    assert!(offline.asset_offline(kick) && !offline.asset_offline(snare));
    assert_eq!(
        offline.asset(kick).unwrap().path(),
        PathBuf::from("/gone/kick.wav")
    );
    // Never saved: the snapshot is unchanged and a rebuilt project is online.
    assert_eq!(offline.snapshot(), saved);
    assert!(
        !Project::from_snapshot(offline.snapshot())
            .unwrap()
            .asset_offline(kick)
    );

    let mut editor = Editor::new(offline, 8).unwrap();
    editor
        .execute(Command::AddTrack {
            name: "extra".into(),
        })
        .unwrap();
    assert!(editor.project().asset_offline(kick));
    assert!(editor.undo());
    assert!(editor.project().asset_offline(kick));
}

#[test]
fn offline_marking_refuses_unknown_assets_without_partial_changes() {
    let (project, kick, _) = imported();
    let mut later = Editor::new(project.clone(), 1).unwrap();
    let Created::Asset(foreign) = later
        .execute(Command::AddAsset {
            path: "/gone/later.wav".into(),
            sample_rate: 48_000,
            frames: 1,
        })
        .unwrap()
    else {
        panic!("asset");
    };
    assert!(project.asset(foreign).is_none());
    assert!(
        project
            .clone()
            .with_offline_assets([kick, foreign])
            .is_err()
    );
    let mut empty = Editor::new(Project::new(48_000).unwrap(), 1).unwrap();
    let Created::Asset(unused) = empty
        .execute(Command::AddAsset {
            path: "/gone/unused.wav".into(),
            sample_rate: 48_000,
            frames: 1,
        })
        .unwrap()
    else {
        panic!("asset");
    };
    // Unused records may be offline too; removing the asset clears its state.
    let offline = empty
        .project()
        .clone()
        .with_offline_assets([unused])
        .unwrap();
    let mut editor = Editor::new(offline, 4).unwrap();
    editor.execute(Command::RemoveAsset(unused)).unwrap();
    assert_eq!(editor.project().offline_assets().count(), 0);
}

#[test]
fn relinking_offline_media_is_one_undoable_step_and_refuses_bad_targets() {
    let (project, kick, snare) = imported();
    let offline = project.with_offline_assets([kick]).unwrap();
    let mut editor = Editor::new(offline.clone(), 8).unwrap();
    let found = PathBuf::from("/found/kick.wav");
    for bad in [
        vec![],
        vec![(snare, PathBuf::from("/found/snare.wav"))],
        vec![(kick, PathBuf::from("relative/kick.wav"))],
        vec![(kick, found.clone()), (kick, found.clone())],
    ] {
        assert!(editor.execute(Command::RelinkAssets(bad)).is_err());
        assert_eq!(
            editor.project(),
            &offline,
            "refusal leaves the project intact"
        );
    }
    editor
        .execute(Command::RelinkAssets(vec![(kick, found.clone())]))
        .unwrap();
    assert!(!editor.project().asset_offline(kick));
    assert_eq!(editor.project().asset(kick).unwrap().path(), found);
    assert!(editor.undo());
    assert_eq!(editor.project(), &offline);
    assert!(editor.project().asset_offline(kick));
    assert!(editor.redo());
    assert!(!editor.project().asset_offline(kick));
    assert_eq!(editor.project().asset(kick).unwrap().path(), found);
    // Relinked paths are saved like any other asset path.
    assert_eq!(
        editor
            .project()
            .snapshot()
            .assets
            .iter()
            .find(|a| a.id == kick.value())
            .unwrap()
            .path,
        found
    );
}
