//! Rendered keys through the real worker, canonical edits, and history.
use super::{
    loop_selection_ui_test::{click, fixture, pump},
    time_selection::TimeSelection,
};
use gpui::{TestAppContext, VisualTestContext};

#[gpui::test]
fn activation_c_and_zero_toggle_mixed_clips_atomically_with_history(cx: &mut TestAppContext) {
    let (handle, editor, audio, midi, _files) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    click(&mut visual, audio, false);
    click(&mut visual, midi, true);
    // Modified keys and auto-repeat must not toggle selected clips.
    visual.simulate_keystrokes("alt-c");
    visual.simulate_event(gpui::KeyDownEvent {
        keystroke: gpui::Keystroke::parse("c").unwrap(),
        is_held: true,
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state.as_ref().unwrap().project.clip_active(audio),
            Some(true)
        );
    });
    for (key, expected) in [("c", false), ("0", true)] {
        let headers = [audio, midi].map(|clip| {
            let selector: &'static str =
                Box::leak(format!("clip-name-{}", clip.value()).into_boxed_str());
            (selector, visual.rendered_background(selector).unwrap())
        });
        visual.simulate_keystrokes(key);
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            let p = &v.state.as_ref().unwrap().project;
            assert_eq!(p.clip_active(audio), Some(!expected));
            assert_eq!(p.clip_active(midi), Some(!expected));
        });
        let pending = headers.map(|(selector, before)| {
            let after = visual.rendered_background(selector).unwrap();
            assert_ne!(after, before);
            (selector, after)
        });
        pump(&editor, &mut visual);
        for (selector, background) in pending {
            assert_eq!(visual.rendered_background(selector), Some(background));
        }
        editor.read_with(&visual, |v, _| {
            assert!(!v.automation_view.enabled);
            let p = &v.state.as_ref().unwrap().project;
            assert_eq!(p.clip_active(audio), Some(expected));
            assert_eq!(p.clip_active(midi), Some(expected));
        });
    }
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        let p = &v.state.as_ref().unwrap().project;
        assert_eq!(p.clip_active(audio), Some(false));
        assert_eq!(p.clip_active(midi), Some(false));
    });
    visual.simulate_keystrokes("cmd-shift-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        let p = &v.state.as_ref().unwrap().project;
        assert_eq!(p.clip_active(audio), Some(true));
        assert_eq!(p.clip_active(midi), Some(true));
    });
}

#[gpui::test]
fn activation_time_span_splits_only_inside_and_header_focus_targets_track(cx: &mut TestAppContext) {
    let (handle, editor, audio, _, _files) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    click(&mut visual, audio, false);
    let track = editor.update(&mut visual, |v, _| {
        let track = v.state.as_ref().unwrap().project.tracks()[0].id();
        v.time_selection = Some(TimeSelection {
            anchor_track: track,
            end_track: track,
            anchor_frame: 12_000,
            end_frame: 36_000,
        });
        track
    });
    visual.simulate_keystrokes("c");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        let clips = v
            .state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .clips();
        assert_eq!(clips.len(), 3);
        assert!(clips[0].active());
        assert!(!clips[1].active());
        assert!(clips[2].active());
        assert_eq!(clips[0].id(), audio);
    });
    visual.update(|window, cx| {
        editor.update(cx, |v, cx| v.track_header_focus(track, cx).focus(window));
    });
    visual.simulate_keystrokes("c");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        let track = v.state.as_ref().unwrap().project.track(track).unwrap();
        assert!(track.muted());
        assert_eq!(track.clips().len(), 3);
        assert!(!track.clips()[1].active());
    });
}

#[gpui::test]
fn activation_session_cells_and_typing_focus_do_not_fall_back_to_track(cx: &mut TestAppContext) {
    let (handle, editor, audio, midi, _files) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(hex_session::service::Request::Edit(
            hex_project::Command::AddScene {
                name: "scene".into(),
            }
        )));
    });
    pump(&editor, &mut visual);
    let scene = editor.read_with(&visual, |v, _| {
        v.state.as_ref().unwrap().project.scenes()[0].id()
    });
    for command in [
        hex_project::Command::CopyArrangementAudioClipToSession {
            source: audio,
            scene,
        },
        hex_project::Command::CopyArrangementMidiToSession {
            source: midi,
            scene,
        },
    ] {
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(hex_session::service::Request::Edit(command)))
        });
        pump(&editor, &mut visual);
    }
    for index in 0..2 {
        let clip = editor.update(&mut visual, |v, cx| {
            let p = &v.state.as_ref().unwrap().project;
            let track = p.tracks()[index].id();
            let clip = match p.session_cell(track, scene).unwrap() {
                hex_project::SessionCellContent::Audio(clip) => clip.id(),
                hex_project::SessionCellContent::Midi(clip) => clip.id(),
                _ => panic!("copied clip"),
            };
            v.mode = super::EditorMode::Session;
            v.selected_track = Some(track);
            v.selected_cell = Some((track, scene));
            cx.notify();
            clip
        });
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        visual.simulate_keystrokes("a");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            assert!(
                !v.automation_view.enabled,
                "console focus does not toggle arranger mode"
            );
        });
        visual.simulate_keystrokes("c");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            let p = &v.state.as_ref().unwrap().project;
            assert_eq!(p.clip_active(clip), Some(false));
            assert!(p.tracks().iter().all(|track| !track.muted()));
            assert_eq!(p.clip_active(audio), Some(true));
            assert_eq!(p.clip_active(midi), Some(true));
        });
    }
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    visual.update(|window, cx| {
        editor.update(cx, |v, cx| {
            v.browser.open = true;
            v.browser.search.read(cx).focus(window);
            cx.notify();
        });
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("a c 0");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
}

#[gpui::test]
fn activation_header_mute_applies_selection_atomically_and_rejects_deselected_focus(
    cx: &mut TestAppContext,
) {
    use gpui::Modifiers;
    use hex_project::Command;
    use hex_session::service::Request;
    let (handle, editor, _, _, _files) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddTrack {
            name: "empty".into()
        })));
    });
    pump(&editor, &mut visual);
    let tracks = editor.read_with(&visual, |v, _| {
        v.state
            .as_ref()
            .unwrap()
            .project
            .tracks()
            .iter()
            .map(|t| t.id())
            .collect::<Vec<_>>()
    });
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::Mute {
            track: tracks[2],
            muted: true
        })));
    });
    pump(&editor, &mut visual);
    let names = [tracks[0], tracks[2]]
        .map(|id| Box::leak(format!("track-name-{}", id.value()).into_boxed_str()) as &'static str);
    let first = visual.debug_bounds(names[0]).unwrap().center();
    let last = visual.debug_bounds(names[1]).unwrap().center();
    visual.simulate_click(first, Modifiers::default());
    visual.simulate_click(
        last,
        Modifiers {
            platform: true,
            ..Modifiers::default()
        },
    );
    visual.run_until_parked();
    let before = editor.read_with(&visual, |v, _| {
        assert_eq!(v.selected_track_ids(), [tracks[0], tracks[2]]);
        v.state.as_ref().unwrap().project.snapshot()
    });
    visual.simulate_keystrokes("c");
    pump(&editor, &mut visual);
    let muted = editor.read_with(&visual, |v, _| {
        let p = &v.state.as_ref().unwrap().project;
        assert!(p.track(tracks[0]).unwrap().muted());
        assert!(!p.track(tracks[1]).unwrap().muted());
        assert!(p.track(tracks[2]).unwrap().muted());
        p.snapshot()
    });
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
    visual.simulate_keystrokes("cmd-shift-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), muted)
    });
    visual.simulate_keystrokes("0");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .tracks()
                .iter()
                .all(|t| !t.muted())
        )
    });
    // Command-click leaves keyboard focus on a header excluded from membership.
    visual.simulate_click(
        last,
        Modifiers {
            platform: true,
            ..Modifiers::default()
        },
    );
    visual.run_until_parked();
    let before = editor.read_with(&visual, |v, _| {
        assert_eq!(v.selected_track_ids(), [tracks[0]]);
        v.state.as_ref().unwrap().project.snapshot()
    });
    visual.simulate_keystrokes("c");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
}
