//! Literal render preservation across a global edit, including a tempo-seam event.
use super::*;
use hex_project::{AudioEventEdit, ClipFade};

#[test]
fn global_tempo_preserves_frozen_event_and_curved_fade_pcm_through_move_split_copy_and_history() {
    let (mut editor, clip, media) = fixture(true);
    let settings = default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        WarpMode::RePitch,
    )
    .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(settings),
        })
        .unwrap();
    let half = BeatTime::from_raw(BeatTime::ONE.raw() / 2);
    editor
        .execute(Command::SetTempoPoint {
            beat: half,
            tempo: Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: None,
        })
        .unwrap();
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Split { at: half.raw() },
        })
        .unwrap();
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Move {
                event: 1,
                start: half.raw(),
            },
        })
        .unwrap();
    editor
        .execute(Command::SetClipFadeEnvelope {
            clip,
            fade: ClipFade::new(12_000, 0, 500, 1000)
                .unwrap()
                .with_shape(100, 200, 600, -500)
                .unwrap(),
        })
        .unwrap();
    let context = AudioTransformPreparation::default();
    let before = render(&prepare(&editor, &media, &context), 0, 12_000, 63);
    // The moved event spans the slower side of the seam. Even destination
    // frames advance only one source frame; endpoint-only conversion fails this.
    let source = media.values().next().unwrap().frames();
    for i in 0..16 {
        assert_eq!(before[4000 + 2 * i], source[137 + i]);
    }
    editor
        .execute(Command::SetProjectTempo {
            tempo: Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    assert_eq!(editor.project().clip(clip).unwrap().duration(), 12_000);
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 12_000, 257),
        before
    );
    let track = editor.project().tracks()[0].id();
    editor
        .execute(Command::MoveClip {
            clip,
            track,
            start: 32_000,
        })
        .unwrap();
    let previous = editor.project().clone();
    editor
        .execute(Command::SetProjectTempo {
            tempo: Tempo::new(90_000_000).unwrap(),
        })
        .unwrap();
    let start = editor.project().clip(clip).unwrap().start();
    assert_eq!(start, 21_333);
    assert_eq!(
        render(&prepare(&editor, &media, &context), start, 12_000, 1),
        before
    );
    assert_eq!(
        render(
            &prepare(&editor, &media, &AudioTransformPreparation::default()),
            start,
            12_000,
            127
        ),
        before
    );
    let changed = editor.project().clone();
    assert!(editor.undo());
    assert_eq!(editor.project(), &previous);
    assert!(editor.redo());
    assert_eq!(editor.project(), &changed);
    let Created::Clip(right) = editor
        .execute(Command::SplitClip {
            clip,
            frame: start + 2001,
        })
        .unwrap()
    else {
        panic!("split")
    };
    assert_eq!(
        render(&prepare(&editor, &media, &context), start, 12_000, 64),
        before
    );
    let Created::Clip(copied) = editor
        .execute(Command::CopyClipTo {
            clip: right,
            track,
            start: 50_000,
        })
        .unwrap()
    else {
        panic!("copy")
    };
    let length = editor.project().clip(copied).unwrap().duration() as usize;
    assert_eq!(length, 9999);
    assert_eq!(
        render(&prepare(&editor, &media, &context), 50_000, length, 63),
        before[2001..]
    );
}

#[test]
fn global_tempo_warped_region_and_midi_note_edges_follow_new_clock() {
    let (mut editor, clip, media) = fixture(false);
    let fade = editor.project().clip(clip).unwrap().fade();
    let settings = default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        WarpMode::RePitch,
    )
    .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(settings),
        })
        .unwrap();
    let track = editor.project().tracks()[0].id();
    editor
        .execute(Command::MoveClip {
            clip,
            track,
            start: 16_000,
        })
        .unwrap();
    let Created::Track(midi_track) = editor
        .execute(Command::AddMidiTrack {
            name: "midi".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(midi) = editor
        .execute(Command::PlaceMidiClip {
            track: midi_track,
            start: BeatTime::from_quarters(2).unwrap(),
            length: BeatTime::from_quarters(2).unwrap(),
        })
        .unwrap()
    else {
        panic!("midi")
    };
    editor
        .execute(Command::AddMidiNote {
            clip: midi,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    assert_eq!(
        crate::compile::midi_events(editor.project())
            .unwrap()
            .iter()
            .map(|e| e.frame)
            .collect::<Vec<_>>(),
        [16_000, 24_000]
    );
    editor
        .execute(Command::SetProjectTempo {
            tempo: Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    assert_eq!(
        crate::compile::midi_events(editor.project())
            .unwrap()
            .iter()
            .map(|e| e.frame)
            .collect::<Vec<_>>(),
        [32_000, 48_000]
    );
    let current = editor.project().clip(clip).unwrap();
    assert_eq!((current.start(), current.duration()), (32_000, 16_000));
    let revision = prepare(&editor, &media, &AudioTransformPreparation::default());
    let actual = render(&revision, 32_000, 16_000, 63);
    let original = media.values().next().unwrap().frames();
    for (index, source) in original.iter().enumerate() {
        let gain = fade.gain_at(index as u64);
        assert_eq!(
            actual[index * 2],
            [source[0] * gain, source[1] * gain],
            "warped source frame {index}"
        );
    }
    assert_eq!(render(&revision, 31_999, 1, 1), [[0.0; 2]]);
}
