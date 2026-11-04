use crate::{
    AudioDomainClock, AudioEventEdit, ClipFade, ClipFlavor, ClipId, Command, Created, Editor,
    Project, TrimEdge,
};

fn fixture() -> (Editor, ClipId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "prefix".into(),
            path: "prefix.wav".into(),
            sample_rate: 48_000,
            frames: 128,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    let track = editor.project().tracks()[0].id();
    editor
        .execute(Command::MoveClip {
            clip,
            track,
            start: 4000,
        })
        .unwrap();
    editor
        .execute(Command::SetClipFadeEnvelope {
            clip,
            fade: ClipFade::new(128, 0, 13, 17).unwrap(),
        })
        .unwrap();
    editor
        .execute(Command::SetClipFlavor {
            clip,
            flavor: ClipFlavor::Accent,
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipGain {
            clip,
            millidb: -6000,
        })
        .unwrap();
    (editor, clip)
}
fn trim(editor: &mut Editor, clip: ClipId, frame: u64) -> Result<Created, crate::Invalid> {
    editor.execute(Command::TrimClipEdge {
        clip,
        edge: TrimEdge::Left,
        frame,
    })
}

#[test]
fn silent_prefix_keeps_media_fade_and_metadata_with_repeatable_history() {
    let (mut editor, clip) = fixture();
    editor
        .execute(Command::SetClipActive {
            clip,
            active: false,
        })
        .unwrap();
    let before = editor.project().snapshot();
    let original = editor.project().clip(clip).unwrap().clone();
    let preview = original
        .preview_edge_trim_with_tempo(TrimEdge::Left, 3500, 128, editor.project().tempo_map())
        .unwrap();
    assert_eq!((preview.start(), preview.duration()), (3500, 628));
    assert_eq!(editor.project().snapshot(), before, "preview is read-only");
    trim(&mut editor, clip, 3500).unwrap();
    assert_eq!(editor.project().clip(clip).unwrap(), &preview);
    let first = editor.project().snapshot();
    for start in [3500, 3000] {
        if start == 3000 {
            trim(&mut editor, clip, start).unwrap();
        }
        let placed = editor.project().clip(clip).unwrap();
        let edit = placed.audio_edit().unwrap();
        let lead = 4000 - start;
        assert_eq!((placed.start(), placed.duration()), (start, 4128 - start));
        assert_eq!(
            (
                placed.asset(),
                placed.source_offset(),
                placed.source_length()
            ),
            (original.asset(), 0, 128)
        );
        assert_eq!(
            (
                placed.name(),
                placed.flavor(),
                placed.active(),
                placed.gain()
            ),
            (
                original.name(),
                original.flavor(),
                original.active(),
                original.gain()
            )
        );
        assert_eq!(edit.domain.clock, AudioDomainClock::Frames(128 + lead));
        assert_eq!(edit.domain.fade, original.retained_fade());
        assert_eq!(edit.domain.fade_start, lead);
        let events = placed.audio_events();
        assert!(events.sources.is_empty(), "no retained-root nesting");
        assert_eq!(events.next_id, 2);
        assert_eq!(events.events.len(), 1);
        let event = events.events[0];
        assert_eq!(
            (
                event.id,
                event.source,
                event.start,
                event.end,
                event.source_start
            ),
            (1, 0, lead, lead + 128, 0)
        );
    }
    let second = editor.project().snapshot();
    assert_eq!(second.next_id, before.next_id);
    assert_eq!(
        Project::from_snapshot(second.clone()).unwrap().snapshot(),
        second
    );
    assert!(editor.undo());
    assert_eq!(editor.project().snapshot(), first);
    assert!(editor.undo());
    assert_eq!(editor.project().snapshot(), before);
    assert!(editor.redo());
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), second);
}

#[test]
fn cropped_frame_events_keep_source_reads_and_identity_when_the_root_gains_silence() {
    let (mut editor, clip) = fixture();
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Split { at: 64 },
        })
        .unwrap();
    trim(&mut editor, clip, 4020).unwrap();
    let old = editor.project().clip(clip).unwrap().clone();
    let events = old.audio_events();
    trim(&mut editor, clip, 3500).unwrap();
    let placed = editor.project().clip(clip).unwrap();
    assert_eq!((placed.start(), placed.duration()), (3500, 628));
    assert_eq!(placed.audio_edit().unwrap().domain.fade_start, 500);
    for (a, b) in placed.audio_events().events.iter().zip(&events.events) {
        assert_eq!(
            (a.id, a.source_start, a.fade, a.active, a.gain),
            (b.id, b.source_start, b.fade, b.active, b.gain)
        );
        assert_eq!((a.start, a.end), (b.start + 500, b.end + 500));
    }
}

#[test]
fn oversized_prefix_refuses_atomically_and_plain_loop_rewind_keeps_its_law() {
    let (mut editor, clip) = fixture();
    let track = editor.project().tracks()[0].id();
    editor
        .execute(Command::MoveClip {
            clip,
            track,
            start: u64::MAX - 128,
        })
        .unwrap();
    let before = editor.project().snapshot();
    assert!(trim(&mut editor, clip, 0).is_err());
    assert_eq!(editor.project().snapshot(), before);
    assert!(editor.undo());
    editor
        .execute(Command::SetClipLooping {
            clip,
            looping: true,
        })
        .unwrap();
    let phase = editor.project().clip(clip).unwrap().source_phase();
    trim(&mut editor, clip, 3500).unwrap();
    let placed = editor.project().clip(clip).unwrap();
    assert_eq!(placed.start(), 3500);
    assert_eq!(placed.source_phase(), (phase + 128 - 500 % 128) % 128);
    assert!(placed.audio_edit().is_none());
}
