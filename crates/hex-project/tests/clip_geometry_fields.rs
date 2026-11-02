//! Geometry commands change placement and source windows only. Activation,
//! name, flavor (clip color) and clip gain must survive every such edit,
//! including the canonical trim preview the UI draws during a drag.
use hex_project::{
    BeatTime, ClipFlavor, ClipGain, ClipId, Command, Created, Editor, Project, TrackId, TrimEdge,
};
use std::{path::PathBuf, sync::Arc};

const NAME: &str = "kept";
const GAIN: i32 = -6_000;
const ASSET_FRAMES: u64 = 400;

type Fields = (bool, String, ClipFlavor, ClipGain);
type GeometryEdit = dyn Fn(&mut Editor, TrackId, ClipId);

fn expected() -> Fields {
    (
        false,
        NAME.into(),
        ClipFlavor::Accent,
        ClipGain::new(GAIN).unwrap(),
    )
}

fn track(editor: &mut Editor) -> TrackId {
    match editor
        .execute(Command::AddTrack {
            name: "hybrid".into(),
        })
        .unwrap()
    {
        Created::Track(id) => id,
        other => panic!("unexpected result: {other:?}"),
    }
}

fn decorate(editor: &mut Editor, clip: ClipId) {
    for command in [
        Command::RenameClip {
            clip,
            name: NAME.into(),
        },
        Command::SetClipFlavor {
            clip,
            flavor: ClipFlavor::Accent,
        },
        Command::SetClipActive {
            clip,
            active: false,
        },
    ] {
        editor.execute(command).unwrap();
    }
}

/// One deactivated, renamed, accent, -6 dB audio clip at frames 100..200.
fn audio_fixture() -> (Editor, TrackId, ClipId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let track = track(&mut editor);
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: PathBuf::from("audio.wav"),
            sample_rate: 48_000,
            frames: ASSET_FRAMES,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 100,
            duration: 100,
            source_offset: 50,
            source_length: 100,
            looping: false,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::SetAudioClipGain {
            clip,
            millidb: GAIN,
        })
        .unwrap();
    decorate(&mut editor, clip);
    (editor, track, clip)
}

fn audio_fields(editor: &Editor, track: TrackId) -> Vec<Fields> {
    editor
        .project()
        .track(track)
        .unwrap()
        .clips()
        .iter()
        .map(|c| (c.active(), c.name().into(), c.flavor(), c.gain()))
        .collect()
}

fn beat(quarters: u64) -> BeatTime {
    BeatTime::from_quarters(quarters).unwrap()
}

/// One deactivated, renamed, accent, -6 dB MIDI clip at beats 4..12 with a note.
fn midi_fixture() -> (Editor, TrackId, ClipId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let track = track(&mut editor);
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: beat(4),
            length: beat(8),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: beat(1),
            duration: beat(1),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    editor
        .execute(Command::SetMidiClipGain {
            clip,
            gain: ClipGain::new(GAIN).unwrap(),
        })
        .unwrap();
    decorate(&mut editor, clip);
    (editor, track, clip)
}

fn midi_fields(editor: &Editor, track: TrackId) -> Vec<Fields> {
    editor
        .project()
        .track(track)
        .unwrap()
        .midi_clips()
        .iter()
        .map(|c| {
            (
                c.active(),
                c.name().into(),
                c.flavor(),
                c.volume().clip_gain(),
            )
        })
        .collect()
}

#[test]
fn audio_edge_trim_preview_and_commit_keep_a_deactivated_clip_inactive() {
    for (edge, frame) in [
        (TrimEdge::Left, 80),
        (TrimEdge::Left, 120),
        (TrimEdge::Right, 180),
        (TrimEdge::Right, 220),
    ] {
        let (mut editor, track, clip) = audio_fixture();
        let preview = editor
            .project()
            .clip(clip)
            .unwrap()
            .preview_edge_trim(edge, frame, ASSET_FRAMES)
            .unwrap();
        assert_ne!(
            (preview.start(), preview.duration()),
            (100, 100),
            "{edge:?} {frame}"
        );
        assert_eq!(
            (
                preview.active(),
                preview.name().into(),
                preview.flavor(),
                preview.gain()
            ),
            expected(),
            "preview {edge:?} {frame}"
        );
        editor
            .execute(Command::TrimClipEdge { clip, edge, frame })
            .unwrap();
        assert_eq!(
            audio_fields(&editor, track),
            vec![expected()],
            "{edge:?} {frame}"
        );
    }
}

#[test]
fn audio_geometry_commands_preserve_clip_fields() {
    let cases: Vec<(&str, Box<GeometryEdit>)> = vec![
        (
            "trim clip",
            Box::new(|e, _, clip| {
                e.execute(Command::TrimClip {
                    clip,
                    start: 110,
                    duration: 50,
                    source_offset: 60,
                    source_length: 50,
                })
                .unwrap();
            }),
        ),
        (
            "trim end by",
            Box::new(|e, _, clip| {
                e.execute(Command::TrimClipEndBy { clip, frames: -10 })
                    .unwrap();
            }),
        ),
        (
            "split",
            Box::new(|e, _, clip| {
                e.execute(Command::SplitClip { clip, frame: 150 }).unwrap();
            }),
        ),
        (
            "move",
            Box::new(|e, track, clip| {
                e.execute(Command::MoveClip {
                    clip,
                    track,
                    start: 300,
                })
                .unwrap();
            }),
        ),
        (
            "nudge",
            Box::new(|e, _, clip| {
                e.execute(Command::NudgeClip { clip, frames: 7 }).unwrap();
            }),
        ),
        (
            "loop",
            Box::new(|e, _, clip| {
                e.execute(Command::SetClipLooping {
                    clip,
                    looping: true,
                })
                .unwrap();
            }),
        ),
        (
            "duplicate",
            Box::new(|e, _, clip| {
                e.execute(Command::DuplicateClip(clip)).unwrap();
            }),
        ),
        (
            "paste",
            Box::new(|e, track, clip| {
                let clipboard = Arc::new(e.project().capture_clips(&[clip]).unwrap());
                e.execute(Command::PasteClips {
                    clipboard,
                    track,
                    frame: 1_000,
                    insert: false,
                })
                .unwrap();
            }),
        ),
    ];
    for (label, edit) in cases {
        let (mut editor, track, clip) = audio_fixture();
        let before = editor.project().clone();
        edit(&mut editor, track, clip);
        assert_ne!(editor.project(), &before, "{label} changed nothing");
        let fields = audio_fields(&editor, track);
        assert!(!fields.is_empty(), "{label}");
        assert!(
            fields.iter().all(|f| *f == expected()),
            "{label}: {fields:?}"
        );
    }
}

#[test]
fn midi_geometry_commands_preserve_clip_fields() {
    let cases: Vec<(&str, Box<GeometryEdit>)> = vec![
        (
            "trim left",
            Box::new(|e, _, clip| {
                e.execute(Command::TrimMidiClipEdge {
                    clip,
                    edge: TrimEdge::Left,
                    beat: beat(5),
                })
                .unwrap();
            }),
        ),
        (
            "trim right",
            Box::new(|e, _, clip| {
                e.execute(Command::TrimMidiClipEdge {
                    clip,
                    edge: TrimEdge::Right,
                    beat: beat(14),
                })
                .unwrap();
            }),
        ),
        (
            "split",
            Box::new(|e, _, clip| {
                e.execute(Command::SplitMidiClip {
                    clip,
                    beat: beat(8),
                })
                .unwrap();
            }),
        ),
        (
            "move",
            Box::new(|e, track, clip| {
                e.execute(Command::MoveMidiClip {
                    clip,
                    track,
                    start: beat(20),
                })
                .unwrap();
            }),
        ),
        (
            "loop",
            Box::new(|e, _, clip| {
                e.execute(Command::SetMidiClipSource {
                    clip,
                    source_length: beat(8),
                    source_start: BeatTime::ZERO,
                    loop_start: BeatTime::ZERO,
                    loop_end: beat(4),
                    looping: true,
                    length: beat(8),
                })
                .unwrap();
            }),
        ),
        (
            "duplicate",
            Box::new(|e, _, clip| {
                e.execute(Command::DuplicateMidiClip(clip)).unwrap();
            }),
        ),
        (
            "paste",
            Box::new(|e, track, clip| {
                let clipboard = Arc::new(e.project().capture_clips(&[clip]).unwrap());
                e.execute(Command::PasteClips {
                    clipboard,
                    track,
                    frame: 480_000,
                    insert: false,
                })
                .unwrap();
            }),
        ),
    ];
    for (label, edit) in cases {
        let (mut editor, track, clip) = midi_fixture();
        let before = editor.project().clone();
        edit(&mut editor, track, clip);
        assert_ne!(editor.project(), &before, "{label} changed nothing");
        let fields = midi_fields(&editor, track);
        assert!(!fields.is_empty(), "{label}");
        assert!(
            fields.iter().all(|f| *f == expected()),
            "{label}: {fields:?}"
        );
    }
}

/// Documented consolidate policy: the container is active and keeps the
/// earliest clip's name and flavor, while deactivated sources stay silent
/// through inactive material.
#[test]
fn consolidating_deactivated_midi_clips_keeps_their_material_inactive() {
    let (mut editor, track, first) = midi_fixture();
    let Created::Clip(second) = editor.execute(Command::DuplicateMidiClip(first)).unwrap() else {
        panic!("duplicate")
    };
    let Created::Clip(grouped) = editor
        .execute(Command::GroupClips(vec![first, second]))
        .unwrap()
    else {
        panic!("group")
    };
    let clip = editor.project().midi_clip(grouped).unwrap();
    assert!(clip.active());
    assert_eq!((clip.name(), clip.flavor()), (NAME, ClipFlavor::Accent));
    assert_eq!(clip.notes().len(), 2);
    assert!(clip.notes().iter().all(|note| !note.active()));
    assert_eq!(midi_fields(&editor, track).len(), 1);
}
