from pathlib import Path
p=Path('crates/hex-ui/src/editor/resize_release_tests.rs');s=p.read_text()
start=s.index('fn review_other_track_release(');end=s.index('#[gpui::test]\nfn browser_release_review_midi_edge_admits',start)
a=s[start:end]
a=a.replace('    visual.simulate_mouse_up(release, MouseButton::Left, Modifiers::default());\n    pump(&editor, &mut visual);', '''    visual.run_until_parked();
    let (desired_audio, desired_midi) = editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        (
            v.active_trim_drag.as_ref().and_then(|d| d.preview.borrow().as_ref().map(|p| p.clip.clone())),
            v.active_midi_trim_drag.as_ref().and_then(|d| *d.preview.borrow()),
        )
    });
    assert!(desired_audio.is_some() || desired_midi.is_some());
    visual.simulate_mouse_up(release, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);''')
a=a.replace('''        assert_eq!(
            v.state.as_ref().unwrap().project.snapshot(),
            before,
            "actual other-track release must cancel; midi={midi}"
        );
        assert!(v.last_enqueued_edit.is_none());''','''        let p = &v.state.as_ref().unwrap().project;
        if let Some(expected) = &desired_audio {
            assert_eq!(p.clip(clip), Some(expected), "captured audio release midi={midi} outside={outside}");
            assert!(matches!(v.last_enqueued_edit, Some(Command::TrimClipEdge { clip: id, .. }) if id == clip));
        }
        if let Some(expected) = desired_midi {
            let actual = p.midi_clip(clip).unwrap();
            assert_eq!((actual.start(), actual.length(), actual.source_start()), (expected.start, expected.length, expected.source_start), "captured MIDI release outside={outside}");
            assert!(matches!(v.last_enqueued_edit, Some(Command::TrimMidiClipEdge { clip: id, .. }) if id == clip));
        }
        assert_eq!(p.track(other), before.tracks.iter().find(|t| t.id() == other));''')
a=a.replace('''    assert!(!visual.update(|_, cx| cx.has_active_drag()));
}''','''    assert!(!visual.update(|_, cx| cx.has_active_drag()));
    let committed = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before));
    visual.simulate_keystrokes("cmd-shift-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| assert_eq!(v.state.as_ref().unwrap().project.snapshot(), committed));
}''')
a=a.replace('browser_release_review_other_audio_track_cancels_without_edit','captured_release_over_other_audio_track_commits_without_retargeting').replace('browser_release_review_other_midi_track_cancels_without_edit','captured_release_over_other_midi_track_commits_without_retargeting')
s=s[:start]+a+s[end:]
s=s.replace('browser_release_review_generic_outside_still_cancels_audio_and_midi','captured_release_over_generic_outside_commits_audio_and_midi')
p.write_text(s)
