use super::super::{EditorView, bind_editor_keys, clip_drag_ui_test};
use gpui::{
    AppContext, Bounds, Entity, Modifiers, MouseButton, Pixels, TestAppContext, VisualTestContext,
    point, px, size,
};
use hex_project::{Command, Editor, TrackId};
use std::sync::Arc;

// Every path is installed before browser workers start and retained through the test.
struct BrowserDirectory(std::path::PathBuf);
impl BrowserDirectory {
    fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "hex-header-volume-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(root.join("samples")).unwrap();
        Self(root)
    }
    fn assert_owned(&self, view: &EditorView) {
        assert_eq!(view.browser.directory, self.0.join("samples"));
        assert_eq!(view.browser.places, vec![self.0.join("samples")]);
    }
}
impl Drop for BrowserDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn actor_fixture(
    cx: &mut TestAppContext,
) -> (
    BrowserDirectory,
    VisualTestContext,
    Entity<EditorView>,
    [TrackId; 2],
) {
    use hex_project::Created;
    use hex_session::service::{Request, Service};
    fn receive(service: &Service) -> hex_session::service::Event {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Ok(event) = service.try_event() {
                return event;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "actor response missing"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    cx.update(bind_editor_keys);
    let service = Service::new(48_000, 32, 0).unwrap();
    receive(&service);
    let mut tracks = Vec::new();
    let mut state = None;
    for gain in [0.5, 1.5] {
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "volume".into(),
            }))
            .unwrap();
        let Created::Track(track) = receive(&service).created.unwrap() else {
            panic!("track")
        };
        service
            .submit(Request::Edit(Command::Gain { track, gain }))
            .unwrap();
        state = Some(receive(&service).state);
        tracks.push(track);
    }
    let browser = BrowserDirectory::new();
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_browser_directory(window, cx, service, &browser.0)
            });
            editor.update(cx, |view, cx| {
                browser.assert_owned(view);
                view.state = state;
                view.selected_track = Some(tracks[1]);
                cx.notify();
            });
            editor
        })
        .unwrap();
    let visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1280.0), px(800.0)));
    visual.run_until_parked();
    (browser, visual, editor, tracks.try_into().unwrap())
}

fn pump(editor: &Entity<EditorView>, cx: &mut TestAppContext, done: impl Fn(&EditorView) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        editor.update(cx, |view, cx| {
            view.tick(cx);
            cx.notify();
        });
        if editor.read_with(cx, |view, _| done(view)) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "gesture did not settle: {}",
            editor.read_with(cx, |view, _| format!(
                "{:?}, {}",
                view.error, view.worker_requests_pending
            ))
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[gpui::test]
fn volume_drag_targets_its_track_without_selection_resize_or_clip_drag_and_cancels(
    cx: &mut TestAppContext,
) {
    let (_browser, mut visual, editor, [track, other]) = actor_fixture(cx);
    let selector = Box::leak(format!("track-volume-{}-slider", track.value()).into_boxed_str());
    let rail = visual.debug_bounds(selector).unwrap();
    let start = point(rail.left() + px(10.0), rail.center().y);
    let end = point(rail.right() - px(2.0), rail.center().y);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    pump(&editor, cx, |view| {
        view.device_gesture.snapshot().is_some() && view.worker_requests_pending == 0
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.selected_track,
            Some(other),
            "fader must not select its background track"
        );
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .gain(),
            0.5,
            "held gesture is not canonical history"
        );
        assert!(!view.track_resize_active());
        assert!(view.active_clip_drag.is_none());
    });
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    pump(&editor, cx, |view| {
        view.device_gesture.snapshot().is_none() && view.worker_requests_pending == 0
    });
    let committed = editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(project.track(other).unwrap().gain(), 1.5);
        let gain = project.track(track).unwrap().gain();
        assert!(gain > 0.5);
        gain
    });
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(rail.left(), rail.center().y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(start, MouseButton::Left, Modifiers::default());
    pump(&editor, cx, |view| {
        view.device_gesture.snapshot().is_none() && view.worker_requests_pending == 0
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .gain(),
            committed
        )
    });
}

#[gpui::test]
fn verified_streaming_keeps_explicit_functional_gain_controls(cx: &mut TestAppContext) {
    let (_browser, mut visual, editor, [track, other]) = actor_fixture(cx);
    editor.update(cx, |view, cx| {
        view.document_audio_mode = hex_session::DocumentAudioMode::VerifiedStreaming;
        cx.notify();
    });
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("track-volume-{}", track.value()).into_boxed_str()
            ))
            .is_none()
    );
    let up = bounds(&mut visual, "track-gain-up", track).center();
    visual.simulate_mouse_down(up, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
    pump(&editor, cx, |view| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .gain()
            > 0.5
            && view.worker_requests_pending == 0
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(other)
                .unwrap()
                .gain(),
            1.5
        );
        assert!(view.device_gesture.snapshot().is_none());
    });
}

#[gpui::test]
fn header_volume_text_commit_uses_canonical_gain_and_automation_refuses_drag(
    cx: &mut TestAppContext,
) {
    let (_browser, mut visual, editor, [track, other]) = actor_fixture(cx);
    let field = visual
        .debug_bounds(Box::leak(
            format!("track-volume-{}-value", track.value()).into_boxed_str(),
        ))
        .unwrap()
        .center();
    visual.simulate_mouse_down(field, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(field, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("0 db");
    visual.simulate_keystrokes("enter");
    pump(&editor, cx, |view| {
        view.device_gesture.snapshot().is_none()
            && view.worker_requests_pending == 0
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .gain()
                == 1.0
    });
    editor.update(cx, |view, _| {
        view.enqueue(hex_session::service::Request::Edit(
            Command::ReplaceTrackGainEnvelope {
                track,
                points: vec![hex_project::GainPoint {
                    beat: hex_project::BeatTime::ZERO,
                    gain: 0.8,
                    outgoing: hex_project::GainInterpolation::Step,
                }],
            },
        ))
    });
    pump(&editor, cx, |view| {
        !view
            .state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .gain_envelope()
            .is_empty()
    });
    visual.run_until_parked();
    let rail = visual
        .debug_bounds(Box::leak(
            format!("track-volume-auto-{}", track.value()).into_boxed_str(),
        ))
        .unwrap();
    drag(
        &mut visual,
        rail.center(),
        point(rail.right(), rail.center().y),
    );
    editor.read_with(cx, |view, _| {
        assert!(view.device_gesture.snapshot().is_none());
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(project.track(track).unwrap().gain(), 1.0);
        assert_eq!(project.track(other).unwrap().gain(), 1.5);
    });
}

fn bounds(visual: &mut VisualTestContext, prefix: &str, track: TrackId) -> Bounds<Pixels> {
    visual
        .debug_bounds(Box::leak(
            format!("{prefix}-{}", track.value()).into_boxed_str(),
        ))
        .unwrap_or_else(|| panic!("missing {prefix}"))
}

fn fixture(
    cx: &mut TestAppContext,
    midi: bool,
) -> (
    BrowserDirectory,
    VisualTestContext,
    Entity<EditorView>,
    [TrackId; 2],
) {
    cx.update(bind_editor_keys);
    let (project, first, second, _) = if midi {
        clip_drag_ui_test::midi_project()
    } else {
        clip_drag_ui_test::audio_project()
    };
    let mut model = Editor::new(project, 8).unwrap();
    model
        .execute(Command::Gain {
            track: first,
            gain: 0.5,
        })
        .unwrap();
    model
        .execute(Command::Gain {
            track: second,
            gain: 1.5,
        })
        .unwrap();
    let browser = BrowserDirectory::new();
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service = hex_session::service::Service::new(48_000, 32, 8_000_000).unwrap();
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_browser_directory(window, cx, service, &browser.0)
            });
            editor.update(cx, |view, cx| {
                browser.assert_owned(view);
                view.state = Some(Arc::new(hex_session::service::ViewState {
                    canonical_audio_source: None,
                    comparison_audio: None,
                    project: Arc::new(model.project().clone()),
                    waveforms: Default::default(),
                    audio_clip_waveforms: Default::default(),
                    midi_preview: Arc::default(),
                    gain_preview: Ok(Arc::default()),
                    timeline_frames: 384_000,
                    can_undo: false,
                    can_redo: false,
                    revision: 1,
                    unsaved: false,
                    document_path: None,
                }));
                cx.notify();
            });
            editor
        })
        .unwrap();
    let visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1280.0), px(800.0)));
    visual.run_until_parked();
    (browser, visual, editor, [first, second])
}

fn drag(visual: &mut VisualTestContext, start: gpui::Point<Pixels>, end: gpui::Point<Pixels>) {
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
}

#[gpui::test]
fn volume_headers_fit_audio_midi_minimum_and_resized_rows_without_losing_controls(
    cx: &mut TestAppContext,
) {
    for midi in [false, true] {
        let (_browser, mut visual, editor, tracks) = fixture(cx, midi);
        for expanded in [false, true] {
            if expanded {
                let width = bounds(&mut visual, "track-width-resize", tracks[0]).center();
                drag(&mut visual, width, point(width.x + px(100.0), width.y));
                let height = bounds(&mut visual, "track-height-resize", tracks[0]).center();
                drag(&mut visual, height, point(height.x, height.y + px(65.0)));
            }
            for track in tracks {
                let header = bounds(&mut visual, "arrangement-track-header", track);
                let lane = bounds(&mut visual, "arrangement-lane", track);
                let slider = bounds(&mut visual, "track-volume", track);
                let rail = visual
                    .debug_bounds(Box::leak(
                        format!("track-volume-{}-slider", track.value()).into_boxed_str(),
                    ))
                    .unwrap();
                let field = visual
                    .debug_bounds(Box::leak(
                        format!("track-volume-{}-value", track.value()).into_boxed_str(),
                    ))
                    .unwrap();
                let height_handle = bounds(&mut visual, "track-height-resize", track);
                let width_handle = bounds(&mut visual, "track-width-resize", track);
                assert_eq!(header.origin.y, lane.origin.y);
                assert_eq!(header.size.height, lane.size.height);
                assert!(slider.left() >= width_handle.right());
                assert!(slider.right() <= header.right());
                assert!(slider.bottom() <= height_handle.top());
                assert!(rail.size.width >= px(36.0) && rail.right() <= field.left());
                for prefix in [
                    "gain-lane-show",
                    "track-mute",
                    "track-solo",
                    "track-return-sends",
                ] {
                    let control = bounds(&mut visual, prefix, track);
                    assert!(control.left() >= header.left() && control.right() <= header.right());
                    assert!(control.bottom() <= height_handle.top());
                    assert!(control.right() <= slider.left() || control.left() >= slider.right());
                }
            }
            editor.read_with(&visual, |view, _| {
                assert!(!view.track_resize_active());
                assert!(view.device_gesture.snapshot().is_none());
                assert!(view.active_clip_drag.is_none());
                assert!(
                    view.last_enqueued_edit.is_none(),
                    "resizing must not change canonical gain"
                );
            });
        }
    }
}

fn automation_readout_owns_gain(cx: &mut TestAppContext, recorded: bool) {
    use hex_project::{
        BeatTime, GainInterpolation, GainPoint, GainTouchContext, GainTouchSpan, GainTouchStep,
    };
    let (_browser, mut visual, editor, [track, _]) = actor_fixture(cx);
    let command = if recorded {
        Command::SpliceGainTouch {
            track,
            expected_epoch: 0,
            span: GainTouchSpan {
                start: BeatTime::ZERO,
                end: BeatTime::ONE,
                context: GainTouchContext {
                    recording_epoch: 1,
                    project_generation: 0,
                    canonical_revision: 1,
                    audio_revision: 1,
                    transport_epoch: 1,
                    gesture_epoch: 1,
                    gesture_serial: 1,
                    sample_rate: 48_000,
                    loop_pass: 0,
                },
                steps: vec![GainTouchStep {
                    beat: BeatTime::ZERO,
                    gain: 0.8,
                    origin_beat: BeatTime::ZERO,
                    source_sequence: 1,
                }],
            },
        }
    } else {
        Command::ReplaceTrackGainEnvelope {
            track,
            points: vec![GainPoint {
                beat: BeatTime::ZERO,
                gain: 0.8,
                outgoing: GainInterpolation::Step,
            }],
        }
    };
    editor.update(cx, |view, _| {
        assert!(view.enqueue(hex_session::service::Request::Edit(command)))
    });
    pump(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.state.as_ref().is_some_and(|s| {
                let owner = s.project.track(track).unwrap();
                if recorded {
                    !owner.gain_lane().spans().is_empty()
                } else {
                    !owner.gain_envelope().is_empty()
                }
            })
    });
    visual.run_until_parked();
    let indication = bounds(&mut visual, "track-volume-auto", track);
    let before = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().clone());
    drag(
        &mut visual,
        indication.center(),
        point(indication.right(), indication.center().y),
    );
    editor.read_with(cx, |view, _| {
        let after = view.state.as_ref().unwrap();
        assert_eq!(after.project, before.project);
        assert_eq!(after.revision, before.revision);
        assert_eq!(
            (after.can_undo, after.can_redo),
            (before.can_undo, before.can_redo)
        );
        assert!(view.device_gesture.snapshot().is_none());
        assert_eq!(view.worker_requests_pending, 0);
        assert!(view.pending_requests.is_empty());
    });
}

#[gpui::test]
fn header_volume_automation_envelope_shows_auto_without_edit(cx: &mut TestAppContext) {
    automation_readout_owns_gain(cx, false);
}

#[gpui::test]
fn header_volume_recorded_touch_lane_shows_auto_without_edit(cx: &mut TestAppContext) {
    automation_readout_owns_gain(cx, true);
}

#[gpui::test]
fn header_volume_is_one_filled_numeric_field_not_a_separate_rail(cx: &mut TestAppContext) {
    let (_browser, mut visual, _editor, [track, _]) = actor_fixture(cx);
    let bounds = |visual: &mut VisualTestContext, suffix: &str| {
        visual.debug_bounds(Box::leak(format!("track-volume-{}{suffix}", track.value()).into_boxed_str())).unwrap()
    };
    let whole = bounds(&mut visual, "");
    let field = bounds(&mut visual, "-value");
    assert!(field.size.width >= whole.size.width - px(4.0), "reference requires a centered value across the filled bar, not a separate narrow value field: {whole:?} {field:?}");
    assert_eq!(field.center().x, whole.center().x);
    assert!(field.size.height >= px(16.0));
}
