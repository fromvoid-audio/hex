use hex_project::{
    BeatTime, ClipEdit, ClipId, Command, Created, Editor, Invalid, Project, SessionCellContent,
    SessionSceneTarget, TrackId,
};

fn beats(n: u64) -> BeatTime {
    BeatTime::from_quarters(n).unwrap()
}
fn track(editor: &mut Editor, name: &str) -> TrackId {
    let Created::Track(id) = editor
        .execute(Command::AddTrack { name: name.into() })
        .unwrap()
    else {
        panic!("track")
    };
    id
}
fn scene(editor: &mut Editor) -> hex_project::SceneId {
    let Created::Scene(id) = editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    id
}
fn audio(editor: &mut Editor, owner: TrackId) -> ClipId {
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "source.wav".into(),
            sample_rate: 48_000,
            frames: 64,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceClip {
            track: owner,
            asset,
            start: 100,
            duration: 20,
            source_offset: 7,
            source_length: 20,
            looping: false,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    clip
}

#[test]
fn audio_column_and_first_scene_are_one_prepared_history_edge_with_no_default_device() {
    let mut e = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let source_track = track(&mut e, "source");
    let clip = audio(&mut e, source_track);
    e.execute(Command::EditClips {
        clips: vec![clip],
        edit: ClipEdit::Gain { millidb: -6000 },
    })
    .unwrap();
    e.execute(Command::SetClipActive {
        clip,
        active: false,
    })
    .unwrap();
    let original = e.project().clip(clip).unwrap().clone();
    let Created::Track(destination) = e
        .execute(Command::CopyArrangementAudioClipToNewSessionTrack {
            initial_gain: Some(0.25),
            source: clip,
            before_track: Some(source_track),
            scene: SessionSceneTarget::Insert { before: None },
        })
        .unwrap()
    else {
        panic!("destination")
    };
    assert_eq!(e.project().tracks()[0].id(), destination);
    assert_eq!(e.project().track(destination).unwrap().gain(), 0.25);
    let row = e.project().scenes()[0].id();
    let Some(SessionCellContent::Audio(copied)) = e.project().session_cell(destination, row) else {
        panic!("audio")
    };
    assert_eq!(
        (
            copied.asset(),
            copied.source_start(),
            copied.source_end(),
            copied.gain(),
            copied.active(),
            copied.flavor()
        ),
        (
            original.asset(),
            7,
            27,
            original.gain(),
            false,
            original.flavor()
        )
    );
    assert_ne!(copied.id(), clip);
    assert_eq!(e.project().clip(clip), Some(&original));
    assert!(
        e.project()
            .track(destination)
            .unwrap()
            .instrument()
            .is_none()
    );
    assert!(e.project().track(destination).unwrap().devices().is_empty());
    let committed = e.project().clone();
    assert!(e.undo());
    assert_eq!(e.project().tracks().len(), 1);
    assert!(e.project().scenes().is_empty());
    assert!(e.project().session_cells().is_empty());
    assert!(e.redo());
    assert_eq!(e.project(), &committed);
}

#[test]
fn midi_column_preserves_source_loop_volume_and_existing_source_cell_with_fresh_note_ids() {
    let mut e = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let owner = track(&mut e, "notes");
    let row = scene(&mut e);
    let Created::Clip(clip) = e
        .execute(Command::PlaceMidiClip {
            track: owner,
            start: beats(8),
            length: beats(4),
        })
        .unwrap()
    else {
        panic!("midi")
    };
    e.execute(Command::AddMidiNote {
        clip,
        start: beats(1),
        duration: beats(1),
        pitch: 64,
        channel: 2,
        velocity: 93,
    })
    .unwrap();
    e.execute(Command::SetMidiClipSource {
        clip,
        source_length: beats(8),
        source_start: beats(1),
        loop_start: beats(2),
        loop_end: beats(6),
        looping: true,
        length: beats(12),
    })
    .unwrap();
    e.execute(Command::SetSessionStopCell {
        track: owner,
        scene: row,
    })
    .unwrap();
    let original = e.project().midi_clip(clip).unwrap().clone();
    let Created::Track(destination) = e
        .execute(Command::CopyArrangementMidiToNewSessionTrack {
            initial_gain: Some(0.25),
            source: clip,
            before_track: None,
            scene: SessionSceneTarget::Existing(row),
        })
        .unwrap()
    else {
        panic!("destination")
    };
    assert_eq!(e.project().tracks().last().unwrap().id(), destination);
    assert_eq!(e.project().track(destination).unwrap().gain(), 0.25);
    assert!(matches!(
        e.project().session_cell(owner, row),
        Some(SessionCellContent::Stop)
    ));
    let Some(SessionCellContent::Midi(copy)) = e.project().session_cell(destination, row) else {
        panic!("copy")
    };
    assert_eq!(
        (
            copy.source().source_length(),
            copy.source().source_start(),
            copy.source().loop_start(),
            copy.source().loop_end(),
            copy.source().looping(),
            copy.playback_offset()
        ),
        (
            original.source_length(),
            original.source_start(),
            original.loop_start(),
            original.loop_end(),
            true,
            original.playback_offset()
        )
    );
    assert_eq!(copy.volume(), original.volume());
    assert_eq!(copy.source().notes().len(), original.notes().len());
    for (new, old) in copy.source().notes().iter().zip(original.notes()) {
        assert_ne!(new.id(), old.id());
        assert_eq!(
            (
                new.start(),
                new.duration(),
                new.pitch(),
                new.channel(),
                new.velocity()
            ),
            (
                old.start(),
                old.duration(),
                old.pitch(),
                old.channel(),
                old.velocity()
            )
        );
    }
    assert_eq!(e.project().midi_clip(clip), Some(&original));
    assert!(
        e.project()
            .track(destination)
            .unwrap()
            .instrument()
            .is_none()
    );
}

#[test]
fn new_column_inserts_a_scene_at_its_stable_anchor_without_replacing_existing_cells() {
    let mut e = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let owner = track(&mut e, "audio");
    let clip = audio(&mut e, owner);
    let first = scene(&mut e);
    let last = scene(&mut e);
    e.execute(Command::SetSessionStopCell {
        track: owner,
        scene: last,
    })
    .unwrap();
    let Created::Track(destination) = e
        .execute(Command::CopyArrangementAudioClipToNewSessionTrack {
            initial_gain: None,
            source: clip,
            before_track: None,
            scene: SessionSceneTarget::Insert { before: Some(last) },
        })
        .unwrap()
    else {
        panic!("track")
    };
    assert_eq!(e.project().scenes()[0].id(), first);
    assert_eq!(e.project().scenes()[2].id(), last);
    assert!(matches!(
        e.project()
            .session_cell(destination, e.project().scenes()[1].id()),
        Some(SessionCellContent::Audio(_))
    ));
    assert!(matches!(
        e.project().session_cell(owner, last),
        Some(SessionCellContent::Stop)
    ));
}

#[test]
fn failed_preparation_preserves_all_ids_project_and_redo_after_private_track_and_scene_creation() {
    let mut e = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let owner = track(&mut e, "source");
    let clip = audio(&mut e, owner);
    track(&mut e, "temporary");
    assert!(e.undo());
    assert!(e.can_redo());
    let before = e.project().clone();
    let result = e.execute_prepared(
        Command::CopyArrangementAudioClipToNewSessionTrack {
            initial_gain: Some(0.25),
            source: clip,
            before_track: None,
            scene: SessionSceneTarget::Insert { before: None },
        },
        |candidate| {
            assert_eq!(candidate.tracks().len(), before.tracks().len() + 1);
            assert_eq!(candidate.scenes().len(), 1);
            assert_eq!(candidate.tracks().last().unwrap().gain(), 0.25);
            Err::<(), _>(Invalid("admission refused"))
        },
    );
    assert!(result.is_err());
    assert_eq!(e.project(), &before);
    assert!(e.can_redo());
    assert!(e.redo());
}

#[test]
fn invalid_column_gain_does_not_create_a_track_scene_or_history() {
    let mut e = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let owner = track(&mut e, "source");
    let audio = audio(&mut e, owner);
    let Created::Clip(midi) = e
        .execute(Command::PlaceMidiClip {
            track: owner,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("midi")
    };
    let before = e.project().clone();
    for gain in [f32::NAN, f32::INFINITY, -0.1, 2.1] {
        for midi_source in [false, true] {
            let scene = SessionSceneTarget::Insert { before: None };
            let command = if midi_source {
                Command::CopyArrangementMidiToNewSessionTrack {
                    source: midi,
                    before_track: None,
                    scene,
                    initial_gain: Some(gain),
                }
            } else {
                Command::CopyArrangementAudioClipToNewSessionTrack {
                    source: audio,
                    before_track: None,
                    scene,
                    initial_gain: Some(gain),
                }
            };
            assert!(e.execute(command).is_err());
            assert_eq!(e.project(), &before);
        }
    }
}

#[test]
fn stale_anchors_wrong_source_kind_and_transformed_audio_refuse_without_partial_births() {
    let mut e = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let owner = track(&mut e, "source");
    let clip = audio(&mut e, owner);
    let stale_track = track(&mut e, "removed");
    e.execute(Command::RemoveTrack(stale_track)).unwrap();
    let stale_scene = scene(&mut e);
    e.execute(Command::RemoveScene(stale_scene)).unwrap();
    let targets = [
        (
            Some(stale_track),
            SessionSceneTarget::Insert { before: None },
        ),
        (None, SessionSceneTarget::Existing(stale_scene)),
        (
            None,
            SessionSceneTarget::Insert {
                before: Some(stale_scene),
            },
        ),
    ];
    for (before_track, scene) in targets {
        let before = e.project().clone();
        assert!(
            e.execute(Command::CopyArrangementAudioClipToNewSessionTrack {
                initial_gain: None,
                source: clip,
                before_track,
                scene
            })
            .is_err()
        );
        assert_eq!(e.project(), &before);
    }
    let before = e.project().clone();
    assert!(
        e.execute(Command::CopyArrangementMidiToNewSessionTrack {
            initial_gain: None,
            source: clip,
            before_track: None,
            scene: SessionSceneTarget::Insert { before: None }
        })
        .is_err()
    );
    assert_eq!(e.project(), &before);
    e.execute(Command::EditClips {
        clips: vec![clip],
        edit: ClipEdit::WarpEnabled { enabled: true },
    })
    .unwrap();
    let before = e.project().clone();
    assert!(
        e.execute(Command::CopyArrangementAudioClipToNewSessionTrack {
            initial_gain: None,
            source: clip,
            before_track: None,
            scene: SessionSceneTarget::Insert { before: None }
        })
        .is_err()
    );
    assert_eq!(e.project(), &before);
}
