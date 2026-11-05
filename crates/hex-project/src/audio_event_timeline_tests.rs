use crate::{AudioEventEdit, ClipId, Command, Created, Editor, Project, TrimEdge};

fn imported(frames: u64) -> (Editor, ClipId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "test".into(),
            path: "test.wav".into(),
            sample_rate: 48_000,
            frames,
        })
        .unwrap()
    else {
        panic!("fixture")
    };
    (editor, clip)
}

fn place(
    editor: &mut Editor,
    clip: ClipId,
    event: u64,
    start: i64,
) -> Result<Created, crate::Invalid> {
    editor.execute(Command::EditAudioEvents {
        clip,
        operation: AudioEventEdit::Place { event, start },
    })
}

// (project start, project end, source frame at start) of every active event,
// in song time. Free placement must keep this stable outside the moved event.
fn song_events(editor: &Editor, clip: ClipId) -> Vec<(u64, u64, u64)> {
    let clip = editor.project().clip(clip).unwrap();
    let view = clip.audio_edit().map_or(0, |edit| edit.view_start);
    clip.audio_events()
        .events
        .iter()
        .map(|e| {
            (
                clip.start() + e.start - view,
                clip.start() + e.end - view,
                e.source_start,
            )
        })
        .collect()
}

#[test]
fn events_placed_past_either_edge_are_kept_and_revealed_by_widening() {
    let (mut editor, clip) = imported(100);
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Split { at: 40 },
        })
        .unwrap();
    let window = |editor: &Editor| {
        let clip = editor.project().clip(clip).unwrap();
        (clip.start(), clip.duration())
    };
    let before = window(&editor);
    // Past the end: the timeline grows, the clip window stays.
    place(&mut editor, clip, 2, 130).unwrap();
    assert_eq!(window(&editor), before);
    assert_eq!(song_events(&editor, clip), [(0, 40, 0), (130, 190, 40)]);
    assert!(
        editor
            .project()
            .clip(clip)
            .unwrap()
            .has_retained_audio_roots()
    );
    // Before the origin: everything shifts by the lead, song time is unchanged.
    place(&mut editor, clip, 1, -25).unwrap();
    assert_eq!(window(&editor), before);
    let placed = editor.project().clip(clip).unwrap();
    let edit = placed.audio_edit().unwrap();
    assert_eq!(
        (edit.view_start, edit.view_end, edit.domain.clock.length()),
        (25, 125, 215)
    );
    // Song time is relative to the unchanged clip start, so the leading event
    // sits 25 frames before it (shown here relative to a clip start of 0).
    let events = placed.audio_events();
    let shown: Vec<_> = events
        .events
        .iter()
        .map(|e| (e.start, e.end, e.source_start))
        .collect();
    assert_eq!(shown, [(0, 40, 0), (155, 215, 40)]);
    // Widening the window reveals the far event at its placed song time.
    editor
        .execute(Command::TrimClipEdge {
            clip,
            edge: TrimEdge::Right,
            frame: 190,
        })
        .unwrap();
    assert_eq!(window(&editor), (0, 190));
    assert!(editor.undo());
    assert!(editor.undo());
    assert_eq!(window(&editor), before);
    assert_eq!(song_events(&editor, clip), [(0, 40, 0), (130, 190, 40)]);
    assert!(editor.redo());
    assert!(editor.redo());
    assert_eq!(window(&editor), (0, 190));
}

#[test]
fn placement_refuses_unsupported_roots_and_unbounded_timelines() {
    let (mut editor, clip) = imported(100);
    let before = editor.project().clone();
    for start in [i64::MIN, i64::MAX, 16_777_216] {
        assert!(place(&mut editor, clip, 1, start).is_err());
    }
    assert!(place(&mut editor, clip, 9, 10).is_err());
    editor
        .execute(Command::SetAudioClipMusical {
            clip,
            enabled: true,
        })
        .unwrap();
    let musical = editor.project().clone();
    assert!(place(&mut editor, clip, 1, 120).is_err());
    assert_eq!(editor.project(), &musical);
    assert!(editor.undo());
    assert_eq!(editor.project(), &before);
}
