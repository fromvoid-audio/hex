//! Focused local cursors participate in Arrangement's rendered play-start intent.
use crate::{editor, editor::worker_ui_fixture};
use gpui::{Modifiers, TestAppContext, VisualTestContext, point, px};
use hex_project::{BeatTime, Command, Tempo};
use hex_session::service::Request;
use std::sync::atomic::Ordering::Acquire;

#[gpui::test]
fn focused_audio_clip_cursor_replaces_the_arrangement_play_start_marker(cx: &mut TestAppContext) {
    let (handle, editor, _, _) = editor::clip_view_ui_test::audio_detail_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let initial = visual.debug_bounds("time-marker-top-start").unwrap();
    let ruler = visual.debug_bounds("audio-events-time-ruler").unwrap();
    visual.simulate_click(ruler.center(), Modifiers::default());
    visual.run_until_parked();
    visual.update(|window, cx| {
        assert!(editor.read(cx).audio_event_focus.is_focused(window));
    });
    let projected = visual.debug_bounds("clip-editor-play-start-top");
    assert!(
        projected.is_some_and(|marker| marker.center().x > initial.center().x),
        "the focused audio cursor should supply the visible play start",
    );

    visual.update(|window, cx| editor.update(cx, |view, _| view.focus.focus(window)));
    visual.run_until_parked();
    assert!(visual.debug_bounds("clip-editor-play-start-top").is_none());
    assert_eq!(visual.debug_bounds("time-marker-top-start"), Some(initial));

    visual.simulate_click(ruler.center(), Modifiers::default());
    visual.run_until_parked();
    let marker = visual.debug_bounds("clip-editor-play-start-top").unwrap();
    visual.simulate_click(marker.center(), Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        visual
            .debug_bounds("locked-play-start-top")
            .unwrap()
            .center()
            .x,
        marker.center().x,
        "locking the visible marker pins the local cursor's projected frame",
    );
}

#[gpui::test]
fn focused_piano_cursor_starts_playback_until_arrangement_focus_or_lock_wins(
    cx: &mut TestAppContext,
) {
    cx.update(editor::bind_editor_keys);
    let mut fixture = worker_ui_fixture::open_with_section_support(
        cx,
        vec![Command::AddMidiTrack {
            name: "midi".into(),
        }],
        false,
        true,
    );
    let editor = fixture.editor.clone();
    let track = editor.read_with(cx, |view, _| {
        view.state.as_ref().unwrap().project.tracks()[0].id()
    });
    let mut visual = VisualTestContext::from_window(fixture.handle, cx);
    let pass_audio = |fixture: &mut worker_ui_fixture::WorkerUiFixture,
                      visual: &mut VisualTestContext| {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        editor.update(visual, |view, cx| {
            if view.tick(cx) {
                cx.notify();
            }
        });
        visual.run_until_parked();
    };
    let beat = |n| BeatTime::from_quarters(n).unwrap();
    for command in [
        Command::PlaceMidiClip {
            track,
            start: beat(4),
            length: beat(4),
        },
        Command::SetTempoPoint {
            beat: beat(5),
            tempo: Tempo::new(60_000_000).unwrap(),
        },
    ] {
        let revision = editor.update(&mut visual, |view, _| {
            let revision = view.state.as_ref().unwrap().revision;
            assert!(view.enqueue(Request::Edit(command)));
            revision + 1
        });
        let deadline = crate::test_wait::hang_deadline();
        loop {
            pass_audio(&mut fixture, &mut visual);
            if editor.read_with(&visual, |view, _| {
                assert!(view.error.is_none(), "{:?}", view.error);
                view.applied_project_revision() == Some(revision)
            }) {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            let clip = view
                .state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .midi_clips()[0]
                .id();
            view.select_arrangement_clip(track, clip);
            view.focus_clip_details(window, cx);
            view.clip_view_open = true;
            cx.notify();
        })
    });
    visual.run_until_parked();
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    visual.simulate_click(
        point(roll.center().x, roll.top() + px(36.0)),
        Modifiers::default(),
    );
    visual.run_until_parked();
    visual.update(|window, cx| {
        let view = editor.read(cx);
        assert!(view.midi_focus.is_focused(window));
        assert_eq!(view.midi_marker, Some(beat(2).raw()));
    });

    let play = |fixture: &mut worker_ui_fixture::WorkerUiFixture,
                visual: &mut VisualTestContext,
                expected| {
        visual.simulate_keystrokes("space");
        pass_audio(fixture, visual);
        editor.read_with(visual, |view, _| {
            let status = view.audio_status().unwrap();
            assert!(status.playing.load(Acquire), "{:?}", view.error);
            assert_eq!(
                status.position.load(Acquire),
                expected + 64,
                "play should use the current cursor owner",
            );
        });
    };
    let stop = |fixture: &mut worker_ui_fixture::WorkerUiFixture,
                visual: &mut VisualTestContext| {
        let stop = visual.debug_bounds("transport-stop-icon").unwrap().center();
        visual.simulate_click(stop, Modifiers::default());
        pass_audio(fixture, visual);
    };

    // Beat 5 starts at 120000, then each beat is 48000 frames.
    play(&mut fixture, &mut visual, 168_000);
    stop(&mut fixture, &mut visual);

    visual.update(|window, cx| editor.update(cx, |view, _| view.focus.focus(window)));
    visual.run_until_parked();
    play(&mut fixture, &mut visual, 96_000);
    stop(&mut fixture, &mut visual);

    let lock = visual
        .debug_bounds("time-marker-top-start")
        .unwrap()
        .center();
    visual.simulate_click(lock, Modifiers::default());
    visual.simulate_click(
        point(roll.center().x, roll.top() + px(36.0)),
        Modifiers::default(),
    );
    visual.run_until_parked();
    play(&mut fixture, &mut visual, 96_000);
    fixture.finish(cx);
}
