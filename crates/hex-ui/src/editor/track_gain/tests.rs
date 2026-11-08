use super::super::{EditorView, bind_editor_keys, clip_drag_ui_test};
use gpui::{
    AppContext, Bounds, Entity, Modifiers, MouseButton, Pixels, TestAppContext, VisualTestContext,
    point, px, size,
};
use hex_project::{Command, Editor, TrackId};
use std::sync::Arc;

// One precreated path owner is retained by both the view and actual background jobs.
pub(in crate::editor) struct BrowserDirectory(
    std::path::PathBuf,
    Arc<super::super::test_environment::Environment>,
);
impl BrowserDirectory {
    fn new() -> Self {
        Self::with_environment(super::super::test_environment::Environment::new().unwrap())
    }
    fn with_environment(environment: Arc<super::super::test_environment::Environment>) -> Self {
        let root = environment.root().to_owned();
        std::fs::create_dir_all(root.join("samples")).unwrap();
        Self(root, environment)
    }
    fn assert_owned(&self, view: &EditorView) {
        assert!(Arc::ptr_eq(&self.1, &view.test_environment));
        assert_eq!(view.browser.directory, self.0.join("samples"));
        assert_eq!(view.browser.places, vec![self.0.join("samples")]);
    }
}

pub(in crate::editor) fn actor_fixture(
    cx: &mut TestAppContext,
) -> (
    BrowserDirectory,
    VisualTestContext,
    Entity<EditorView>,
    [TrackId; 2],
) {
    actor_fixture_with_environment(
        cx,
        super::super::test_environment::Environment::new().unwrap(),
    )
}

fn actor_fixture_with_environment(
    cx: &mut TestAppContext,
    environment: Arc<super::super::test_environment::Environment>,
) -> (
    BrowserDirectory,
    VisualTestContext,
    Entity<EditorView>,
    [TrackId; 2],
) {
    actor_fixture_in_mode(cx, environment, hex_session::DocumentAudioMode::Resident)
}

fn actor_fixture_in_mode(
    cx: &mut TestAppContext,
    environment: Arc<super::super::test_environment::Environment>,
    mode: hex_session::DocumentAudioMode,
) -> (
    BrowserDirectory,
    VisualTestContext,
    Entity<EditorView>,
    [TrackId; 2],
) {
    use hex_project::Created;
    use hex_session::service::{Request, Service};
    fn receive(service: &Service) -> hex_session::service::Event {
        let deadline = crate::test_wait::hang_deadline();
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
    if mode == hex_session::DocumentAudioMode::VerifiedStreaming {
        service
            .submit(Request::NewWithAudioMode {
                mode,
                reservation: Some(hex_session::SourcePlanReservation {
                    reader: hex_media::VerifiedWavReaderLimits::new(
                        hex_media::WavReaderLimits::new(64 * 1024 * 1024, 8_000_000, 32, 4096),
                        1024 * 1024,
                    ),
                    max_retained_sources: 1,
                    max_path_bytes: 1024 * 1024,
                    max_manifest_bytes: 1024 * 1024,
                }),
            })
            .unwrap();
        assert!(receive(&service).error.is_none());
    }
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
    let browser = BrowserDirectory::with_environment(environment);
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_environment(
                    window,
                    cx,
                    service,
                    Arc::clone(&browser.1),
                )
            });
            editor.update(cx, |view, cx| {
                browser.assert_owned(view);
                view.project_generation = state
                    .as_ref()
                    .unwrap()
                    .canonical_audio_source
                    .as_ref()
                    .unwrap()
                    .context()
                    .project_generation;
                view.document_audio_mode = mode;
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

pub(in crate::editor) fn pump(
    editor: &Entity<EditorView>,
    cx: &mut TestAppContext,
    done: impl Fn(&EditorView) -> bool,
) {
    let deadline = crate::test_wait::hang_deadline();
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
    let end = point(start.x, start.y - px(24.0));
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
fn verified_track_volume_drag_uses_actor_and_keeps_return_gain_explicit(cx: &mut TestAppContext) {
    let (_browser, mut visual, editor, [track, other]) = actor_fixture_in_mode(
        cx,
        super::super::test_environment::Environment::new().unwrap(),
        hex_session::DocumentAudioMode::VerifiedStreaming,
    );
    let rail = visual
        .debug_bounds(Box::leak(
            format!("track-volume-{}-slider", track.value()).into_boxed_str(),
        ))
        .expect("verified track volume mounts the shared numeric slider");
    let revision = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().revision);
    let start = point(rail.left() + px(10.0), rail.center().y);
    let end = point(start.x, start.y - px(24.0));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    pump(&editor, cx, |view| {
        view.device_gesture.snapshot().is_some() && view.worker_requests_pending == 0
    });
    let held = editor.read_with(cx, |view, _| {
        assert_eq!(view.selected_track, Some(other));
        let state = view.state.as_ref().unwrap();
        assert_eq!(state.revision, revision);
        assert_eq!(state.project.track(track).unwrap().gain(), 0.5);
        assert_eq!(state.project.track(other).unwrap().gain(), 1.5);
        let hex_session::device_gesture::ParameterValue::Float(value) =
            view.device_gesture.snapshot().unwrap().value
        else {
            panic!("gain");
        };
        assert!(value > 0.5);
        value
    });
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    for _ in 0..3 {
        editor.update(cx, |_, cx| cx.notify());
        visual.run_until_parked();
        editor.read_with(cx, |view, cx| {
            assert!(
                (view
                    .track_gain_controls
                    .bindings
                    .get(&track)
                    .unwrap()
                    .entity
                    .read(cx)
                    .value()
                    - f64::from(held))
                .abs()
                    < 0.000001
            );
        });
    }
    pump(&editor, cx, |view| {
        view.device_gesture.snapshot().is_none() && view.worker_requests_pending == 0
    });
    editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().unwrap();
        assert_eq!(state.revision, revision + 1);
        assert_eq!(state.project.track(track).unwrap().gain(), held);
        assert_eq!(state.project.track(other).unwrap().gain(), 1.5);
    });
    editor.update(cx, |view, _| {
        assert!(view.enqueue(hex_session::service::Request::Undo));
    });
    pump(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .gain()
                == 0.5
    });
    editor.update(cx, |view, _| {
        view.enqueue(hex_session::service::Request::Edit(
            Command::AddReturnTrack {
                name: "streamed bus".into(),
            },
        ))
    });
    pump(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && !view
                .state
                .as_ref()
                .unwrap()
                .project
                .return_tracks()
                .is_empty()
    });
    editor.update(cx, |view, cx| {
        view.document_audio_mode = hex_session::DocumentAudioMode::VerifiedStreaming;
        cx.notify();
    });
    visual.run_until_parked();
    let id = editor.read_with(cx, |view, _| {
        view.state.as_ref().unwrap().project.return_tracks()[0].id()
    });
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("return-gain-up-{}", id.value()).into_boxed_str()
            ))
            .is_none()
    );
    let field = visual
        .debug_bounds(Box::leak(
            format!("return-volume-{}-value", id.value()).into_boxed_str(),
        ))
        .unwrap()
        .center();
    visual.simulate_click(field, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("-6 db");
    visual.simulate_keystrokes("enter");
    let expected = 10_f32.powf(-6.0 / 20.0);
    pump(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && (view.state.as_ref().unwrap().project.return_tracks()[0].gain() - expected).abs()
                < 1e-5
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
                    incoming_gain: None,
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
                EditorView::new_for_test_with_environment(
                    window,
                    cx,
                    service,
                    Arc::clone(&browser.1),
                )
            });
            editor.update(cx, |view, cx| {
                browser.assert_owned(view);
                view.state = Some(Arc::new(hex_session::service::ViewState {
                    canonical_audio_source: None,
                    comparison_audio: None,
                    project: Arc::new(model.project().clone()),
                    waveforms: Default::default(),
                    source_overviews: Default::default(),
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
                assert!(rail.size.width >= px(58.0));
                assert_eq!(
                    rail, field,
                    "one filled field replaces the old separate rail/readout"
                );
                for prefix in [
                    "gain-lane-show",
                    "track-mute",
                    "track-solo",
                    "track-return-sends",
                ] {
                    let control = bounds(&mut visual, prefix, track);
                    assert!(
                        control.size.width >= px(14.0),
                        "{prefix} keeps a usable target"
                    );
                    assert!(control.left() >= header.left() && control.right() <= header.right());
                    assert!(control.bottom() <= height_handle.top());
                    assert!(
                        control.right() <= slider.left()
                            || control.left() >= slider.right()
                            || control.bottom() <= slider.top()
                            || control.top() >= slider.bottom(),
                        "midi={midi} expanded={expanded} {prefix} {control:?} overlaps volume {slider:?} in {header:?}"
                    );
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

fn automation_readout_owns_gain(
    cx: &mut TestAppContext,
    recorded: bool,
    mode: hex_session::DocumentAudioMode,
) {
    use hex_project::{
        BeatTime, GainInterpolation, GainPoint, GainTouchContext, GainTouchSpan, GainTouchStep,
    };
    let (_browser, mut visual, editor, [track, _]) = actor_fixture_in_mode(
        cx,
        super::super::test_environment::Environment::new().unwrap(),
        mode,
    );
    let command = if recorded {
        Command::SpliceGainTouch {
            track,
            expected_epoch: 0,
            span: GainTouchSpan {
                source_start: BeatTime::ZERO,
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
                incoming_gain: None,
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
    automation_readout_owns_gain(cx, false, hex_session::DocumentAudioMode::Resident);
}

#[gpui::test]
fn header_volume_recorded_touch_lane_shows_auto_without_edit(cx: &mut TestAppContext) {
    automation_readout_owns_gain(cx, true, hex_session::DocumentAudioMode::Resident);
}

#[gpui::test]
fn verified_header_volume_automation_envelope_refuses_drag(cx: &mut TestAppContext) {
    automation_readout_owns_gain(cx, false, hex_session::DocumentAudioMode::VerifiedStreaming);
}

#[gpui::test]
fn verified_header_volume_recorded_touch_lane_refuses_drag(cx: &mut TestAppContext) {
    automation_readout_owns_gain(cx, true, hex_session::DocumentAudioMode::VerifiedStreaming);
}

#[gpui::test]
fn header_volume_is_one_filled_numeric_field_not_a_separate_rail(cx: &mut TestAppContext) {
    // This fixture verifies control bounds and painted fields, not glyph metrics.
    // Keep OS font initialization in the native typography acceptance boundary.
    let (_browser, mut visual, _editor, [track, _]) = actor_fixture(cx);
    let bounds = |visual: &mut VisualTestContext, suffix: &str| {
        visual
            .debug_bounds(Box::leak(
                format!("track-volume-{}{suffix}", track.value()).into_boxed_str(),
            ))
            .unwrap()
    };
    let whole = bounds(&mut visual, "");
    let field = bounds(&mut visual, "-value");
    assert!(
        field.size.width >= whole.size.width - px(4.0),
        "reference requires a centered value across the filled bar, not a separate narrow value field: {whole:?} {field:?}"
    );
    assert_eq!(field.center().x, whole.center().x);
    assert_eq!(field.size.height, px(14.0));
    let header = self::bounds(&mut visual, "arrangement-track-header", track);
    let pan = self::bounds(&mut visual, "track-pan", track);
    let activator = self::bounds(&mut visual, "track-mute", track);
    let solo = self::bounds(&mut visual, "track-solo", track);
    let arm = self::bounds(&mut visual, "track-arm-unavailable", track);
    assert_eq!(activator.size.height, px(14.0));
    assert_eq!(solo.size.width, px(16.0));
    assert_eq!(solo.size.height, px(14.0));
    assert_eq!(arm.size.width, px(16.0));
    assert_eq!(arm.size.height, px(14.0));
    assert_eq!(activator.top(), solo.top());
    assert_eq!(solo.top(), arm.top());
    assert_eq!(whole.top(), pan.top());
    assert!((activator.left() - whole.left()).abs() <= px(1.0));
    assert!(activator.bottom() <= whole.top());
    assert_eq!(arm.right(), pan.right());
    let meter = self::bounds(&mut visual, "track-output-meter", track);
    assert!(
        pan.right() <= meter.left(),
        "pan {pan:?} overlaps meter {meter:?}"
    );
    for rect in [whole, pan, activator, solo, arm] {
        assert!(rect.left() >= header.left() && rect.right() <= header.right());
        assert!(rect.top() >= header.top() && rect.bottom() <= header.bottom());
    }
    let mut scene = Vec::new();
    for suffix in [
        "track-mute",
        "track-solo",
        "track-arm-unavailable",
        "track-volume",
        "track-pan",
    ] {
        let id = Box::leak(format!("{suffix}-{}", track.value()).into_boxed_str());
        let rect = visual.debug_bounds(id).unwrap();
        scene.push(serde_json::json!({"id": id, "bounds": [f32::from(rect.left()), f32::from(rect.top()), f32::from(rect.size.width), f32::from(rect.size.height)], "background": visual.rendered_background(id)}));
    }
    for prefix in ["track-volume", "track-pan"] {
        for suffix in ["slider", "bar-fill", "value"] {
            let id = Box::leak(format!("{prefix}-{}-{suffix}", track.value()).into_boxed_str());
            let rect = visual.debug_bounds(id).unwrap();
            let background = visual.rendered_background(id);
            if suffix == "slider" || (prefix == "track-volume" && suffix == "bar-fill") {
                assert!(background.is_some());
            }
            scene.push(serde_json::json!({"id":id, "bounds":[f32::from(rect.left()), f32::from(rect.top()),f32::from(rect.size.width),f32::from(rect.size.height)], "background":background}));
        }
    }
    let activator_id = Box::leak(format!("track-mute-{}", track.value()).into_boxed_str());
    let solo_id = Box::leak(format!("track-solo-{}", track.value()).into_boxed_str());
    assert_eq!(
        visual.rendered_background(activator_id),
        Some(gpui::rgb(crate::theme::color(crate::theme::PaletteRole::Primary)).into())
    );
    assert_ne!(
        visual.rendered_background(activator_id),
        visual.rendered_background(solo_id)
    );
    println!(
        "HEX439_RENDERED_SCENE {}",
        serde_json::to_string(&scene).unwrap()
    );
}

#[gpui::test]
fn header_mixer_pan_entry_drag_cancel_and_activator_solo_are_real_commands(
    cx: &mut TestAppContext,
) {
    let (_browser, mut visual, editor, [track, other]) = actor_fixture(cx);
    let pan = bounds(&mut visual, "track-pan", track);
    let field = visual
        .debug_bounds(Box::leak(
            format!("track-pan-{}-value", track.value()).into_boxed_str(),
        ))
        .unwrap();
    assert!(field.left() >= pan.left() && field.right() <= pan.right());
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("track-pan-{}-bar-center", track.value()).into_boxed_str()
            ))
            .is_none(),
        "the pan value must not have a center line behind it"
    );
    let start = field.center();
    let end = point(start.x, start.y - px(20.0));
    drag(&mut visual, start, end);
    pump(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.device_gesture.snapshot().is_none()
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .balance()
                > 0.0
    });
    let before = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    pump(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.device_gesture.snapshot().is_none()
    });
    assert_eq!(
        editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone()),
        before
    );
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(start, MouseButton::Left, Modifiers::default());
    visual.simulate_input("25 l");
    visual.simulate_keystrokes("enter");
    pump(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.device_gesture.snapshot().is_none()
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .balance()
                == -0.25
    });
    editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(project.track(other).unwrap().balance(), 0.0);
        assert!(!project.track(track).unwrap().soloed());
        assert!(!view.track_resize_active() && view.active_clip_drag.is_none());
    });
    for prefix in ["track-mute", "track-solo"] {
        let hit = bounds(&mut visual, prefix, track).center();
        visual.simulate_mouse_down(hit, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(hit, MouseButton::Left, Modifiers::default());
        pump(&editor, cx, |view| view.worker_requests_pending == 0);
    }
    editor.read_with(cx, |view, _| {
        let track = view.state.as_ref().unwrap().project.track(track).unwrap();
        assert!(track.muted() && track.soloed());
    });
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("track-arm-unavailable-{}", track.value()).into_boxed_str()
            ))
            .is_some()
    );
    let name = bounds(&mut visual, "track-name", track).center();
    visual.simulate_mouse_down(name, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(name, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| assert_eq!(view.selected_track, Some(track)));
}

#[gpui::test]
fn return_header_filled_gain_commits_and_keyboard_invalid_draft_blocks_owner_transition(
    cx: &mut TestAppContext,
) {
    let (_browser, mut visual, editor, _) = actor_fixture(cx);
    editor.update(cx, |view, _| {
        view.enqueue(hex_session::service::Request::Edit(
            Command::AddReturnTrack { name: "bus".into() },
        ))
    });
    pump(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && !view
                .state
                .as_ref()
                .unwrap()
                .project
                .return_tracks()
                .is_empty()
    });
    visual.run_until_parked();
    let id = editor.read_with(cx, |view, _| {
        view.state.as_ref().unwrap().project.return_tracks()[0].id()
    });
    let selector = Box::leak(format!("return-volume-{}-value", id.value()).into_boxed_str());
    let field = visual.debug_bounds(selector).unwrap().center();
    visual.simulate_mouse_down(field, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(field, MouseButton::Left, Modifiers::default());
    visual.simulate_input("-12 db");
    visual.simulate_keystrokes("enter");
    let expected = 10.0_f32.powf(-12.0 / 20.0);
    pump(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.device_gesture.snapshot().is_none()
            && (view.state.as_ref().unwrap().project.return_tracks()[0].gain() - expected).abs()
                < 1e-6
    });
    visual.simulate_mouse_down(field, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(field, MouseButton::Left, Modifiers::default());
    visual.simulate_input("+100 db");
    editor.update(cx, |view, cx| {
        assert!(!super::device_panel::finalize_controls(view, cx))
    });
    editor.read_with(cx, |view, _| {
        assert!(
            (view.state.as_ref().unwrap().project.return_tracks()[0].gain() - expected).abs()
                < 1e-6
        )
    });
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    let mute_id = Box::leak(format!("return-mute-{}", id.value()).into_boxed_str());
    let on = visual.rendered_background(mute_id).unwrap();
    assert_eq!(
        on,
        gpui::rgb(crate::theme::color(crate::theme::PaletteRole::Primary)).into()
    );
    let hit = visual.debug_bounds(mute_id).unwrap().center();
    visual.simulate_mouse_down(hit, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(hit, MouseButton::Left, Modifiers::default());
    pump(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.state.as_ref().unwrap().project.return_tracks()[0].muted()
    });
    visual.run_until_parked();
    assert_ne!(visual.rendered_background(mute_id).unwrap(), on);
}

#[gpui::test]
fn header_fixture_real_catalog_retains_one_root_after_view_and_caller_drop(
    cx: &mut TestAppContext,
) {
    use std::sync::atomic::Ordering;
    let environment = super::super::test_environment::Environment::new().unwrap();
    let root = environment.root().to_owned();
    std::fs::create_dir(root.join("samples")).unwrap();
    let wav = root.join("samples/header.wav");
    let bytes = [
        b"RIFF".as_slice(),
        &44_u32.to_le_bytes(),
        b"WAVEfmt ",
        &16_u32.to_le_bytes(),
        &1_u16.to_le_bytes(),
        &1_u16.to_le_bytes(),
        &48_000_u32.to_le_bytes(),
        &96_000_u32.to_le_bytes(),
        &2_u16.to_le_bytes(),
        &16_u16.to_le_bytes(),
        b"data",
        &8_u32.to_le_bytes(),
        &[1, 0, 1, 0, 1, 0, 1, 0],
    ]
    .concat();
    std::fs::write(&wav, &bytes).unwrap();
    let weak = Arc::downgrade(&environment);
    let gate = environment.gate_next_catalog_save();
    let (owner, mut visual, editor, _) = actor_fixture_with_environment(cx, environment);
    assert!(gate.reached.load(Ordering::Acquire));
    assert!(!gate.finished.load(Ordering::Acquire));
    let paths = editor.read_with(&visual, |view, _| {
        owner.assert_owned(view);
        [
            view.browser.test_preferences_path().to_owned(),
            view.recent_projects.test_path().to_owned(),
            view.theme_settings.test_path().to_owned(),
            view.project_directory.test_path().to_owned(),
        ]
    });
    for (path, name) in paths.iter().zip([
        "browser.json",
        "recent-projects.json",
        "theme.json",
        "project-directory.json",
    ]) {
        assert_eq!(*path, root.join("config").join(name));
    }
    let database = paths[0].with_extension("samples-v1.db");
    assert_eq!(database, root.join("config/browser.samples-v1.db"));
    assert!(!database.exists());
    let editor_weak = editor.downgrade();
    visual.update(|window, cx| {
        let _ = window.replace_root(cx, |_, _| gpui::Empty);
    });
    drop(editor);
    drop(owner);
    visual.update(|_, _| {});
    visual.run_until_parked();
    assert!(editor_weak.upgrade().is_none());
    assert_eq!(
        weak.strong_count(),
        1,
        "actual held catalog job retains its path owner"
    );
    assert_eq!(std::fs::read(&wav).unwrap(), bytes);
    for path in &paths {
        assert!(path.parent().unwrap().is_dir());
    }
    gate.release();
    visual.run_until_parked();
    assert!(gate.finished.load(Ordering::Acquire));
    assert!(gate.save_succeeded.load(Ordering::Relaxed));
    assert!(gate.root_present_during_save.load(Ordering::Relaxed));
    visual.update(|_, _| {});
    assert!(weak.upgrade().is_none());
    let deadline = crate::test_wait::hang_deadline();
    while root.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "owned header root cleanup"
        );
        std::thread::yield_now();
    }
    visual.run_until_parked();
    assert!(
        !root.exists(),
        "retired catalog job cannot recreate header root"
    );
    eprintln!(
        "header fixture SAME Environment/WAV/fourprefs/database, actual held save, view+caller retired, cleanup verified"
    );
}

#[gpui::test]
fn header_pan_vertical_drag_direction_fine_bounds_and_atomic_history(cx: &mut TestAppContext) {
    use hex_session::service::Request;
    let (_browser, mut visual, editor, [track, other]) = actor_fixture(cx);
    let pan_bounds = bounds(&mut visual, "track-pan", track);
    let start = pan_bounds.center();
    for (dx, dy, fine, expected) in [
        (0.0, -24.0, false, 0.4),
        (0.0, 24.0, false, -0.4),
        (24.0, 0.0, false, 0.0),
        (0.0, -24.0, true, 0.04),
        (0.0, -1000.0, false, 1.0),
        (0.0, 1000.0, false, -1.0),
    ] {
        let before = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
        assert_eq!(before.track(track).unwrap().balance(), 0.0);
        let end = point(start.x + px(dx), start.y + px(dy));
        let modifiers = Modifiers {
            shift: fine,
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(start, MouseButton::Left, modifiers);
        visual.simulate_mouse_move(end, Some(MouseButton::Left), modifiers);
        visual.simulate_mouse_up(end, MouseButton::Left, modifiers);
        pump(&editor, cx, |view| {
            view.worker_requests_pending == 0 && view.device_gesture.snapshot().is_none()
        });
        let after = editor.read_with(cx, |view, _| {
            let project = &view.state.as_ref().unwrap().project;
            assert!(
                (project.track(track).unwrap().balance() - expected).abs() < 0.0001,
                "actual pan dx={dx} dy={dy} fine={fine}: {} expected{expected}",
                project.track(track).unwrap().balance()
            );
            assert_eq!(project.track(other).unwrap().balance(), 0.0);
            assert_eq!(
                project.track(track).unwrap().gain(),
                before.track(track).unwrap().gain()
            );
            assert_eq!(view.selected_track, Some(other));
            assert!(!view.track_resize_active() && view.active_clip_drag.is_none());
            project.clone()
        });
        visual.run_until_parked();
        assert_eq!(bounds(&mut visual, "track-pan", track), pan_bounds);
        if expected == 0.0 {
            assert_eq!(
                after, before,
                "horizontal motion must leave canonical content unchanged"
            );
        } else {
            editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
            pump(&editor, cx, |view| {
                view.worker_requests_pending == 0
                    && *view.state.as_ref().unwrap().project == *before
            });
            editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
            pump(&editor, cx, |view| {
                view.worker_requests_pending == 0 && *view.state.as_ref().unwrap().project == *after
            });
            editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
            pump(&editor, cx, |view| {
                view.worker_requests_pending == 0
                    && *view.state.as_ref().unwrap().project == *before
            });
        }
    }
}

#[gpui::test]
fn unused_lower_header_space_selects_tracks_with_modifiers_in_both_audio_modes(
    cx: &mut TestAppContext,
) {
    for mode in [
        hex_session::DocumentAudioMode::Resident,
        hex_session::DocumentAudioMode::VerifiedStreaming,
    ] {
        let (_browser, mut visual, editor, [first, second]) = actor_fixture_in_mode(
            cx,
            super::super::test_environment::Environment::new().unwrap(),
            mode,
        );
        visual.run_until_parked();
        let lower_gap = |visual: &mut VisualTestContext, track: TrackId| {
            let a = Box::leak(format!("gain-lane-show-{}", track.value()).into_boxed_str());
            let next = if mode == hex_session::DocumentAudioMode::Resident {
                format!("track-return-sends-{}", track.value())
            } else {
                format!("track-volume-{}", track.value())
            };
            let b = Box::leak(next.into_boxed_str());
            let left = visual.debug_bounds(a).unwrap();
            let right = visual.debug_bounds(b).unwrap();
            assert!(
                left.right() < right.left(),
                "use actual unused space between controls"
            );
            point((left.right() + right.left()) / 2.0, left.center().y)
        };
        let first_gap = lower_gap(&mut visual, first);
        let second_gap = lower_gap(&mut visual, second);
        visual.simulate_click(first_gap, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(
                view.selected_track_ids(),
                [first],
                "lower background in {mode:?}"
            );
            assert_eq!(view.selected_track, Some(first));
        });
        visual.update(|window, cx| {
            editor.read_with(cx, |view, _| assert!(view.track_header_focused(window)))
        });
        visual.simulate_click(
            second_gap,
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        );
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.selected_track_ids(), [first, second])
        });
        visual.simulate_click(
            first_gap,
            Modifiers {
                platform: true,
                ..Modifiers::default()
            },
        );
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.selected_track_ids(), [second]);
            assert_eq!(
                view.worker_requests_pending, 0,
                "background selection is view state only"
            );
            assert!(!view.track_resize_active() && view.active_clip_drag.is_none());
        });
        if mode == hex_session::DocumentAudioMode::Resident {
            let mute = bounds(&mut visual, "track-mute", first);
            let sample_arm = bounds(&mut visual, "sample-arm-track", first);
            visual.simulate_click(sample_arm.center(), Modifiers::default());
            visual.simulate_mouse_move(point(px(0.0), px(0.0)), None, Modifiers::default());
            visual.run_until_parked();
            assert_eq!(
                visual.rendered_background(Box::leak(
                    format!("sample-arm-track-{}", first.value()).into_boxed_str()
                )),
                Some(gpui::rgb(crate::theme::color(crate::theme::PaletteRole::Recording)).into()),
                "armed recording stays red when interaction highlights are blue"
            );
            editor.read_with(&visual, |view, _| {
                assert_eq!(
                    view.selected_track_ids(),
                    [second],
                    "sample arm owns its click"
                );
            });
            assert!(mute.right() < sample_arm.left());
            let gap = point((mute.right() + sample_arm.left()) / 2.0, mute.center().y);
            visual.simulate_click(gap, Modifiers::default());
            visual.run_until_parked();
            editor.read_with(&visual, |view, _| {
                assert_eq!(
                    view.selected_track_ids(),
                    [first],
                    "mixer gutter selects too"
                )
            });
        }
    }
}

#[gpui::test]
fn lower_header_controls_keep_their_actions_without_selecting_background_track(
    cx: &mut TestAppContext,
) {
    for mode in [
        hex_session::DocumentAudioMode::Resident,
        hex_session::DocumentAudioMode::VerifiedStreaming,
    ] {
        let (_browser, mut visual, editor, [track, other]) = actor_fixture_in_mode(
            cx,
            super::super::test_environment::Environment::new().unwrap(),
            mode,
        );
        visual.run_until_parked();
        let name = bounds(&mut visual, "track-name", other).center();
        visual.simulate_click(name, Modifiers::default());
        visual.run_until_parked();
        for prefix in ["track-mute", "track-solo"] {
            let hit = bounds(&mut visual, prefix, track).center();
            visual.simulate_click(hit, Modifiers::default());
            pump(&editor, cx, |view| view.worker_requests_pending == 0);
            editor.read_with(&visual, |view, _| {
                assert_eq!(view.selected_track_ids(), [other])
            });
        }
        editor.read_with(&visual, |view, _| {
            let track = view.state.as_ref().unwrap().project.track(track).unwrap();
            assert!(track.muted() && track.soloed());
        });
        let sends = bounds(&mut visual, "track-return-sends", track).center();
        visual.simulate_click(sends, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.send_menu_track, Some(track));
            assert_eq!(view.selected_track_ids(), [other]);
        });
        visual.simulate_click(sends, Modifiers::default());
        let rail = visual
            .debug_bounds(Box::leak(
                format!("track-volume-{}-slider", track.value()).into_boxed_str(),
            ))
            .unwrap();
        let start = point(rail.left() + px(10.0), rail.center().y);
        drag(&mut visual, start, point(start.x, start.y - px(24.0)));
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
                    > 0.5
        });
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.selected_track_ids(), [other]);
        });
        // Automation deliberately selects its owner; retain that existing action.
        let automation = bounds(&mut visual, "gain-lane-show", track).center();
        visual.simulate_click(automation, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.gain_lane.track, Some(track));
            assert_eq!(view.selected_track, Some(track));
        });
    }
}

#[gpui::test]
fn compact_resize_refuses_to_hide_an_unfinished_header_gain_draft(cx: &mut TestAppContext) {
    let (_browser, mut visual, editor, [track, _]) = actor_fixture(cx);
    let original = editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.clone()
    });
    let selector = Box::leak(format!("track-volume-{}-value", track.value()).into_boxed_str());
    let field = visual.debug_bounds(selector).unwrap().center();
    visual.simulate_click(field, Modifiers::default());
    visual.simulate_input("+100 db");
    let handle = bounds(&mut visual, "track-height-resize", track).center();
    let original_height = bounds(&mut visual, "arrangement-track-header", track)
        .size
        .height;
    drag(&mut visual, handle, point(handle.x, px(0.0)));
    assert_eq!(
        bounds(&mut visual, "arrangement-track-header", track)
            .size
            .height,
        original_height
    );
    assert!(
        visual.debug_bounds(selector).is_some(),
        "draft remains visible and recoverable"
    );
    editor.read_with(&visual, |view, cx| {
        assert!(view.track_gain_controls.has_pending_draft(cx));
        assert_eq!(view.state.as_ref().unwrap().project, original);
        assert_eq!(view.worker_requests_pending, 0);
    });
    visual.simulate_keystrokes("escape");
    let handle = bounds(&mut visual, "track-height-resize", track).center();
    drag(&mut visual, handle, point(handle.x, px(0.0)));
    assert_eq!(
        bounds(&mut visual, "arrangement-track-header", track)
            .size
            .height,
        px(23.0)
    );
    assert!(visual.debug_bounds(selector).is_none());
}

// Compare completed scene fragments rather than only the final model. These
// regions are unrelated to the edited fader and must survive every repaint.
fn stable_region(
    visual: &mut VisualTestContext,
    selector: &'static str,
) -> (
    Bounds<Pixels>,
    Vec<(Bounds<gpui::ScaledPixels>, gpui::Background)>,
) {
    (
        visual.debug_bounds(selector).expect(selector),
        visual.rendered_filled_quads(selector),
    )
}

#[gpui::test]
fn unrelated_gain_does_not_dim_audio_detail_controls(cx: &mut TestAppContext) {
    let (handle, editor, audio, _, _files) =
        super::super::loop_selection_ui_test::fixture_with_bounce_media(cx, true);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1280.0), px(800.0)));
    let (source, other) = editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        (project.tracks()[0].id(), project.tracks()[1].id())
    });
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            assert!(view.set_selected_clip(source, audio, window, cx));
            view.clip_view_open = true;
            view.device_panel_active = false;
            cx.notify();
        })
    });
    visual.run_until_parked();
    let before = stable_region(&mut visual, "clip-view");
    let header = Box::leak(format!("track-controls-{}", source.value()).into_boxed_str());
    let before_header = stable_region(&mut visual, header);
    let selector = Box::leak(format!("track-volume-{}-slider", other.value()).into_boxed_str());
    let rail = visual.debug_bounds(selector).unwrap();
    let down = point(rail.left() + px(10.0), rail.center().y);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x, down.y - px(24.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    pump(&editor, cx, |view| {
        view.device_gesture.snapshot().is_some() && view.worker_requests_pending == 0
    });
    for _ in 0..3 {
        visual.update(|window, cx| {
            editor.update(cx, |_, cx| cx.notify());
            window.draw(cx).clear();
        });
        assert_eq!(
            stable_region(&mut visual, "clip-view"),
            before,
            "unrelated gain flashes audio controls"
        );
        assert_eq!(stable_region(&mut visual, header), before_header);
    }
    visual.simulate_mouse_up(
        point(down.x, down.y - px(24.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    pump(&editor, cx, |view| {
        !view.device_gesture_pending() && view.worker_requests_pending == 0
    });
    visual.run_until_parked();
    assert_eq!(stable_region(&mut visual, "clip-view"), before);
    let controls = [
        "audio-clip-gain",
        "audio-clip-warp-toggle",
        "audio-sample-bpm",
    ];
    let before_controls = controls.map(|selector| stable_region(&mut visual, selector));
    visual.update(|window, cx| editor.read_with(cx, |view, _| view.focus.focus(window)));
    visual.simulate_keystrokes("cmd-d");
    editor.read_with(cx, |view, _| {
        assert!(
            view.worker_requests_pending != 0 || !view.pending_requests.is_empty(),
            "observe the actual pre-ACK duplication frame"
        )
    });
    for _ in 0..3 {
        visual.update(|window, cx| {
            editor.update(cx, |_, cx| cx.notify());
            window.draw(cx).clear();
        });
        assert_eq!(
            controls.map(|selector| stable_region(&mut visual, selector)),
            before_controls,
            "duplication pending flashes audio controls"
        );
    }
    pump(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.pending_requests.is_empty()
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(source)
                .unwrap()
                .clips()
                .len(),
            2
        );
    });
    for _ in 0..3 {
        visual.update(|window, cx| {
            editor.update(cx, |_, cx| cx.notify());
            window.draw(cx).clear();
        });
        assert_eq!(
            controls.map(|selector| stable_region(&mut visual, selector)),
            before_controls,
            "duplication terminal flashes unchanged audio controls"
        );
    }
}

#[gpui::test]
fn console_selected_group_uses_shared_gain_gesture_and_meter(cx: &mut TestAppContext) {
    use hex_project::TrackSelectionIds;
    use hex_session::service::Request;
    let (_browser, mut visual, editor, [track, other]) = actor_fixture(cx);
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::GroupTracks {
            tracks: TrackSelectionIds::new(&[track]).unwrap(),
            name: "group".into(),
        })))
    });
    pump(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.state.as_ref().unwrap().project.has_group_tracks()
    });
    let group = editor.read_with(cx, |view, _| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .tracks()
            .iter()
            .find(|track| track.is_group())
            .unwrap()
            .id()
    });
    editor.update(cx, |view, cx| {
        view.set_mode(super::super::EditorMode::Session);
        cx.notify();
    });
    visual.run_until_parked();
    let header = visual
        .debug_bounds(Box::leak(
            format!("session-track-{}", group.value()).into_boxed_str(),
        ))
        .unwrap()
        .center();
    visual.simulate_mouse_down(header, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(header, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("console-track-meter-{}", group.value()).into_boxed_str()
            ))
            .is_some()
    );
    editor.update(cx, |view, cx| {
        view.track_meters.set_test_level(group, [0.8, 0.2]);
        cx.notify();
    });
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("track-output-meter-{}-left-fill", group.value()).into_boxed_str()
            ))
            .unwrap()
            .size
            .height
            > px(1.0)
    );
    let rail = visual
        .debug_bounds(Box::leak(
            format!("track-volume-{}-slider", group.value()).into_boxed_str(),
        ))
        .expect("selected Console group gain");
    let revision = editor.read_with(cx, |v, _| v.state.as_ref().unwrap().revision);
    let start = point(rail.left() + px(10.0), rail.center().y);
    let end = point(start.x, start.y - px(24.0));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    pump(&editor, cx, |view| {
        view.device_gesture.snapshot().is_some() && view.worker_requests_pending == 0
    });
    let held = editor.read_with(cx, |view, _| {
        let hex_session::device_gesture::ParameterValue::Float(value) =
            view.device_gesture.snapshot().unwrap().value
        else {
            panic!("gain")
        };
        assert!(value > 1.0);
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(group)
                .unwrap()
                .gain(),
            1.0
        );
        value
    });
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    let other_header = visual
        .debug_bounds(Box::leak(
            format!("session-track-{}", other.value()).into_boxed_str(),
        ))
        .unwrap()
        .center();
    visual.simulate_mouse_down(other_header, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(other_header, MouseButton::Left, Modifiers::default());
    for _ in 0..3 {
        editor.update(cx, |_, cx| cx.notify());
        visual.run_until_parked();
        assert!(
            visual.debug_bounds("console-track-gain-owner").is_none(),
            "the draft remains in its original visible column"
        );
        editor.read_with(cx, |view, cx| {
            assert_eq!(view.selected_track, Some(other));
            assert!(
                (view
                    .track_gain_controls
                    .bindings
                    .get(&group)
                    .unwrap()
                    .entity
                    .read(cx)
                    .value()
                    - f64::from(held))
                .abs()
                    < 0.000001
            )
        });
    }
    pump(&editor, cx, |view| {
        view.device_gesture.snapshot().is_none() && view.worker_requests_pending == 0
    });
    editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().unwrap();
        assert_eq!(state.revision, revision + 1);
        assert_eq!(state.project.track(group).unwrap().gain(), held);
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .track(group)
                .unwrap()
                .gain()
                == 1.0
    });
    let header = visual
        .debug_bounds(Box::leak(
            format!("session-track-{}", other.value()).into_boxed_str(),
        ))
        .unwrap()
        .center();
    visual.simulate_mouse_down(header, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(header, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("console-track-meter-{}", group.value()).into_boxed_str()
            ))
            .is_some()
    );
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("console-track-meter-{}", other.value()).into_boxed_str()
            ))
            .is_some()
    );
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("track-volume-{}-slider", other.value()).into_boxed_str()
            ))
            .is_some()
    );
    editor.update(cx, |view, _| {
        assert!(
            view.enqueue(Request::Edit(Command::ReplaceTrackGainEnvelope {
                track: other,
                points: vec![hex_project::GainPoint {
                    incoming_gain: None,
                    beat: hex_project::BeatTime::ZERO,
                    gain: 0.8,
                    outgoing: hex_project::GainInterpolation::Step
                }],
            }))
        )
    });
    pump(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && !view
                .state
                .as_ref()
                .unwrap()
                .project
                .track(other)
                .unwrap()
                .gain_envelope()
                .is_empty()
    });
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("track-volume-auto-{}", other.value()).into_boxed_str()
            ))
            .is_some()
    );
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("track-volume-{}-slider", other.value()).into_boxed_str()
            ))
            .is_none()
    );
}

#[gpui::test]
fn console_gain_draft_stays_visible_when_selection_changes(cx: &mut TestAppContext) {
    let (_browser, mut visual, editor, [track, other]) = actor_fixture(cx);
    editor.update(cx, |view, cx| {
        view.set_mode(super::super::EditorMode::Session);
        view.selected_track = Some(track);
        cx.notify();
    });
    visual.run_until_parked();
    let field_id = Box::leak(format!("track-volume-{}-value", track.value()).into_boxed_str());
    let field = visual.debug_bounds(field_id).unwrap().center();
    visual.simulate_mouse_down(field, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(field, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("+100 db");
    let other_header = visual
        .debug_bounds(Box::leak(
            format!("session-track-{}", other.value()).into_boxed_str(),
        ))
        .unwrap()
        .center();
    visual.simulate_mouse_down(other_header, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(other_header, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        assert_eq!(view.selected_track, Some(other));
        assert!(
            view.track_gain_controls
                .bindings
                .get(&track)
                .unwrap()
                .entity
                .read(cx)
                .is_editing()
        );
    });
    assert!(
        visual.debug_bounds(field_id).is_some(),
        "the old track's draft must stay visible"
    );
    assert!(
        visual.debug_bounds("console-track-gain-owner").is_none(),
        "the original column retains the draft without a duplicate footer"
    );
    let field = visual.debug_bounds(field_id).unwrap().center();
    visual.simulate_mouse_down(field, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(field, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        assert!(
            !view
                .track_gain_controls
                .bindings
                .get(&track)
                .unwrap()
                .entity
                .read(cx)
                .is_editing(),
            "escape cancels the draft while retaining the column's control"
        );
    });
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("track-volume-{}-slider", other.value()).into_boxed_str()
            ))
            .is_some()
    );
}

#[gpui::test]
fn console_gain_draft_retires_when_source_tracks_are_removed(cx: &mut TestAppContext) {
    use hex_session::service::Request;
    let (_browser, mut visual, editor, [track, other]) = actor_fixture(cx);
    editor.update(cx, |view, cx| {
        view.set_mode(super::super::EditorMode::Session);
        view.selected_track = Some(track);
        cx.notify();
    });
    visual.run_until_parked();
    let field = visual
        .debug_bounds(Box::leak(
            format!("track-volume-{}-value", track.value()).into_boxed_str(),
        ))
        .unwrap()
        .center();
    visual.simulate_mouse_down(field, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(field, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("+100 db");
    editor.update(cx, |view, _| {
        view.service
            .submit(Request::Edit(Command::RemoveTracks(vec![track, other])))
            .unwrap()
    });
    pump(&editor, cx, |view| {
        view.state.as_ref().unwrap().project.tracks().is_empty()
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            !view.track_gain_controls.bindings.contains_key(&track),
            "removed track must not retain a numeric draft"
        )
    });
    assert!(visual.debug_bounds("console-track-gain-owner").is_none());
}

#[gpui::test]
fn console_track_activator_and_solo_use_canonical_commands(cx: &mut TestAppContext) {
    use hex_project::TrackSelectionIds;
    use hex_session::service::Request;
    let (_browser, mut visual, editor, [track, other]) = actor_fixture(cx);
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::GroupTracks {
            tracks: TrackSelectionIds::new(&[track]).unwrap(),
            name: "group".into(),
        })))
    });
    pump(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.state.as_ref().unwrap().project.has_group_tracks()
    });
    let group = editor.read_with(cx, |view, _| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .tracks()
            .iter()
            .find(|t| t.is_group())
            .unwrap()
            .id()
    });
    editor.update(cx, |view, cx| {
        view.set_mode(super::super::EditorMode::Session);
        view.selected_track = Some(group);
        cx.notify();
    });
    visual.run_until_parked();
    let mute = visual
        .debug_bounds(Box::leak(
            format!("console-track-mute-{}", group.value()).into_boxed_str(),
        ))
        .expect("console activator")
        .center();
    visual.simulate_mouse_down(mute, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(mute, MouseButton::Left, Modifiers::default());
    pump(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .track(group)
                .unwrap()
                .muted()
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && !view
                .state
                .as_ref()
                .unwrap()
                .project
                .track(group)
                .unwrap()
                .muted()
    });
    let solo = visual
        .debug_bounds(Box::leak(
            format!("console-track-solo-{}", group.value()).into_boxed_str(),
        ))
        .expect("console solo")
        .center();
    visual.simulate_mouse_down(solo, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(solo, MouseButton::Left, Modifiers::default());
    pump(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .track(group)
                .unwrap()
                .soloed()
    });
    editor.update(cx, |view, cx| {
        view.selected_track = Some(other);
        cx.notify();
    });
    visual.run_until_parked();
    let solo = visual
        .debug_bounds(Box::leak(
            format!("console-track-solo-{}", other.value()).into_boxed_str(),
        ))
        .unwrap()
        .center();
    let modifiers = Modifiers {
        platform: true,
        ..Modifiers::default()
    };
    visual.simulate_mouse_down(solo, MouseButton::Left, modifiers);
    visual.simulate_mouse_up(solo, MouseButton::Left, modifiers);
    pump(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .track(other)
                .unwrap()
                .soloed()
    });
    editor.read_with(cx, |view, _| {
        assert!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(group)
                .unwrap()
                .soloed()
        )
    });
}

#[gpui::test]
fn selected_track_gain_drag_previews_and_commits_relative_values_together(cx: &mut TestAppContext) {
    selected_relative_case(cx, hex_session::DocumentAudioMode::Resident);
}

#[gpui::test]
fn verified_selected_track_gain_drag_preserves_group_preview_and_one_undo(cx: &mut TestAppContext) {
    selected_relative_case(cx, hex_session::DocumentAudioMode::VerifiedStreaming);
}

fn selected_relative_case(cx: &mut TestAppContext, mode: hex_session::DocumentAudioMode) {
    let (_browser, mut visual, editor, [track, other]) = actor_fixture_in_mode(
        cx,
        super::super::test_environment::Environment::new().unwrap(),
        mode,
    );
    editor.update(cx, |view, cx| {
        view.track_selection.replace(&[track, other]);
        view.selected_track = Some(track);
        cx.notify();
    });
    visual.run_until_parked();
    let rail = visual
        .debug_bounds(Box::leak(
            format!("track-volume-{}-slider", track.value()).into_boxed_str(),
        ))
        .unwrap();
    let start = point(rail.left() + px(10.0), rail.center().y);
    let end = point(start.x, start.y - px(24.0));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    pump(&editor, cx, |view| {
        view.device_gesture.snapshot().is_some() && view.worker_requests_pending == 0
    });
    visual.run_until_parked();
    let (held, expected_other) = editor.read_with(cx, |view, cx| {
        let hex_session::device_gesture::ParameterValue::Float(held) =
            view.device_gesture.snapshot().unwrap().value
        else {
            panic!("gain")
        };
        assert!(held > 0.5);
        let expected_other = (1.5 * held / 0.5).min(2.0);
        assert!(
            (view.track_gain_controls.bindings[&other]
                .entity
                .read(cx)
                .value()
                - f64::from(expected_other))
            .abs()
                < 1e-6,
            "all selected faders must preview the same relative dB change"
        );
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(other)
                .unwrap()
                .gain(),
            1.5,
            "preview is not canonical history"
        );
        (held, expected_other)
    });
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    for _ in 0..3 {
        editor.update(cx, |_, cx| cx.notify());
        visual.run_until_parked();
        editor.read_with(cx, |view, cx| {
            for (id, gain) in [(track, held), (other, expected_other)] {
                assert!(
                    (view.track_gain_controls.bindings[&id]
                        .entity
                        .read(cx)
                        .value()
                        - f64::from(gain))
                    .abs()
                        < 1e-6,
                    "released group must stay visible until its exact ACK"
                );
            }
        });
    }
    pump(&editor, cx, |view| {
        view.device_gesture.snapshot().is_none() && view.worker_requests_pending == 0
    });
    editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(project.track(track).unwrap().gain(), held);
        assert_eq!(project.track(other).unwrap().gain(), expected_other);
    });
    editor.update(cx, |view, _| {
        assert!(view.enqueue(hex_session::service::Request::Undo))
    });
    pump(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .gain()
                == 0.5
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
            1.5,
            "one Undo restores the whole group"
        )
    });
}

#[gpui::test]
fn console_group_gain_freezes_membership_and_escape_restores_every_member(cx: &mut TestAppContext) {
    let (_browser, mut visual, editor, [track, other]) = actor_fixture(cx);
    editor.update(cx, |view, cx| {
        view.track_selection.replace(&[track, other]);
        view.selected_track = Some(track);
        view.set_mode(super::super::EditorMode::Session);
        cx.notify();
    });
    visual.run_until_parked();
    let selector = Box::leak(format!("track-volume-{}-slider", track.value()).into_boxed_str());
    let rail = visual.debug_bounds(selector).unwrap();
    let start = point(rail.left() + px(10.0), rail.center().y);
    let end = point(start.x, start.y - px(24.0));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    pump(&editor, cx, |view| {
        view.device_gesture.snapshot().is_some() && view.worker_requests_pending == 0
    });
    let held = editor.read_with(cx, |view, _| {
        let hex_session::device_gesture::ParameterValue::Float(gain) =
            view.device_gesture.snapshot().unwrap().value
        else {
            panic!("gain")
        };
        gain
    });
    editor.update(cx, |view, cx| {
        view.track_selection.replace(&[other]);
        view.selected_track = Some(other);
        cx.notify();
    });
    visual.run_until_parked();
    assert!(
        visual.debug_bounds(selector).is_some(),
        "Console must retain its actual gesture owner"
    );
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_gesture
                .preview_track_gain(view.project_generation, other),
            Some((held * 3.0).min(2.0)),
            "membership stays frozen after selection changes"
        )
    });
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    for _ in 0..3 {
        editor.update(cx, |_, cx| cx.notify());
        visual.run_until_parked();
    }
    pump(&editor, cx, |view| {
        view.device_gesture.snapshot().is_none() && view.worker_requests_pending == 0
    });
    editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(project.track(track).unwrap().gain(), 0.5);
        assert_eq!(project.track(other).unwrap().gain(), 1.5);
    });
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("track-volume-{}-slider", other.value()).into_boxed_str()
            ))
            .is_some()
    );
}

#[gpui::test]
fn stale_selected_gain_begin_refuses_the_whole_group_without_an_extra_undo(
    cx: &mut TestAppContext,
) {
    use hex_session::service::Request;
    let (_browser, mut visual, editor, [track, other]) = actor_fixture(cx);
    editor.update(cx, |view, cx| {
        view.track_selection.replace(&[track, other]);
        view.selected_track = Some(track);
        cx.notify();
    });
    visual.run_until_parked();
    let rail = visual
        .debug_bounds(Box::leak(
            format!("track-volume-{}-slider", track.value()).into_boxed_str(),
        ))
        .unwrap();
    // Another actor edit precedes Begin while the rendered projection is stale.
    editor.update(cx, |view, _| {
        view.service
            .submit(Request::Edit(Command::Gain {
                track: other,
                gain: 0.75,
            }))
            .unwrap();
        view.worker_submitted();
    });
    let start = point(rail.left() + px(10.0), rail.center().y);
    let end = point(start.x, start.y - px(24.0));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    pump(&editor, cx, |view| {
        view.device_gesture.snapshot().is_none()
            && view.worker_requests_pending == 0
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .track(other)
                .unwrap()
                .gain()
                == 0.75
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .gain(),
            0.5
        );
        for (id, gain) in [(track, 0.5), (other, 0.75)] {
            assert_eq!(
                view.track_gain_controls.bindings[&id]
                    .entity
                    .read(cx)
                    .value(),
                gain
            );
        }
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .track(other)
                .unwrap()
                .gain()
                == 1.5
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
            0.5
        )
    });
}

#[gpui::test]
fn dragging_a_silent_selected_anchor_preserves_other_selected_gains(cx: &mut TestAppContext) {
    use hex_session::{device_gesture::GestureTarget, service::Request};
    let (_browser, mut visual, editor, [track, other]) = actor_fixture(cx);
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::Gain { track, gain: 0.0 })))
    });
    pump(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .gain()
                == 0.0
    });
    editor.update(cx, |view, cx| {
        view.track_selection.replace(&[track, other]);
        view.selected_track = Some(track);
        cx.notify();
    });
    visual.run_until_parked();
    let rail = visual
        .debug_bounds(Box::leak(
            format!("track-volume-{}-slider", track.value()).into_boxed_str(),
        ))
        .unwrap();
    let start = point(rail.left() + px(10.0), rail.center().y);
    let end = point(start.x, start.y - px(24.0));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    pump(&editor, cx, |view| {
        view.device_gesture.snapshot().is_some() && view.worker_requests_pending == 0
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_gesture.snapshot().unwrap().target,
            GestureTarget::TrackGain { track }
        )
    });
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    pump(&editor, cx, |view| {
        view.device_gesture.snapshot().is_none() && view.worker_requests_pending == 0
    });
    editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert!(project.track(track).unwrap().gain() > 0.0);
        assert_eq!(project.track(other).unwrap().gain(), 1.5);
    });
}

#[gpui::test]
fn main_slider_edits_and_cancels_one_canonical_owner_in_both_views(cx: &mut TestAppContext) {
    use hex_session::{device_gesture::GestureTarget, service::Request};
    let (_browser, mut visual, editor, tracks) = actor_fixture(cx);
    for (mode, db) in [
        (super::super::EditorMode::Arrangement, -6.0_f64),
        (super::super::EditorMode::Session, -12.0_f64),
    ] {
        editor.update(cx, |view, cx| {
            view.set_mode(mode);
            cx.notify();
        });
        visual.run_until_parked();
        let (previous, track_gains) = editor.read_with(cx, |view, _| {
            let project = &view.state.as_ref().unwrap().project;
            (
                project.master_gain(),
                tracks.map(|id| project.track(id).unwrap().gain()),
            )
        });
        assert!(visual.debug_bounds("main-gain-up").is_none());
        let field = visual.debug_bounds("main-volume-value").unwrap().center();
        visual.simulate_mouse_down(field, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(field, MouseButton::Left, Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input(&format!("{db} db"));
        visual.simulate_keystrokes("enter");
        let expected = 10.0_f32.powf(db as f32 / 20.0);
        pump(&editor, cx, |view| {
            view.device_gesture.snapshot().is_none()
                && view.worker_requests_pending == 0
                && (view.state.as_ref().unwrap().project.master_gain() - expected).abs() < 1e-5
        });
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert_eq!(
                tracks.map(|id| view
                    .state
                    .as_ref()
                    .unwrap()
                    .project
                    .track(id)
                    .unwrap()
                    .gain()),
                track_gains
            )
        });
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && (view.state.as_ref().unwrap().project.master_gain() - previous).abs() < 1e-5
        });
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
        pump(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && (view.state.as_ref().unwrap().project.master_gain() - expected).abs() < 1e-5
        });
        visual.run_until_parked();
        let rail = visual.debug_bounds("main-volume-slider").unwrap();
        let start = point(rail.left() + px(10.0), rail.center().y);
        let end = start - point(px(0.0), px(18.0));
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        pump(&editor, cx, |view| {
            view.device_gesture
                .snapshot()
                .is_some_and(|s| s.target == GestureTarget::MainGain)
                && view.worker_requests_pending == 0
        });
        editor.read_with(cx, |view, _| {
            assert!(
                (view.state.as_ref().unwrap().project.master_gain() - expected).abs() < 1e-5,
                "held preview does not commit canonical gain"
            )
        });
        visual.simulate_keystrokes("escape");
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        pump(&editor, cx, |view| {
            view.device_gesture.snapshot().is_none() && view.worker_requests_pending == 0
        });
        editor.read_with(cx, |view, _| {
            assert!((view.state.as_ref().unwrap().project.master_gain() - expected).abs() < 1e-5)
        });
    }
}

#[gpui::test]
fn verified_main_slider_keeps_explicit_entry_and_finalization_without_held_preview(
    cx: &mut TestAppContext,
) {
    use hex_session::{DocumentAudioMode, service::Request};
    let environment = super::super::test_environment::Environment::new().unwrap();
    let (_browser, mut visual, editor, tracks) =
        actor_fixture_in_mode(cx, environment, DocumentAudioMode::VerifiedStreaming);
    let field = visual.debug_bounds("main-volume-value").unwrap().center();
    visual.simulate_mouse_down(field, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(field, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("-6 db");
    editor.update(cx, |view, cx| {
        assert!(super::super::device_panel::finalize_controls(view, cx))
    });
    let expected = 10.0_f32.powf(-6.0 / 20.0);
    pump(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && (view.state.as_ref().unwrap().project.master_gain() - expected).abs() < 1e-5
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.device_gesture.snapshot().is_none());
        assert!(
            view.audible_playback.is_none(),
            "no resident preview fallback"
        );
    });
    let rail = visual.debug_bounds("main-volume-slider").unwrap();
    let start = point(rail.left() + px(10.0), rail.center().y);
    let end = start - point(px(0.0), px(18.0));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            view.error
                .as_deref()
                .is_some_and(|s| s.contains("resident"))
        );
        assert!(view.device_gesture.snapshot().is_none());
        assert!((view.state.as_ref().unwrap().project.master_gain() - expected).abs() < 1e-5);
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.state.as_ref().unwrap().project.master_gain() == 1.0
    });
    visual.run_until_parked();
    let field = visual.debug_bounds("main-volume-value").unwrap().center();
    visual.simulate_mouse_down(field, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(field, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("-12 db");
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::Gain {
            track: tracks[0],
            gain: 0.2
        })))
    });
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        assert_eq!(view.state.as_ref().unwrap().project.master_gain(), 1.0);
        assert_eq!(
            view.track_gain_controls.mixer[&super::MixerKey::MainGain]
                .entity
                .read(cx)
                .value(),
            1.0,
            "rejected Main entry must not remain painted during another pending edit"
        );
        assert!(
            view.error.as_deref().is_some_and(
                |message| message.contains("current edit") && !message.contains("resident")
            ),
            "atomic entry is supported; only the pending edit blocks it"
        );
    });
}

#[gpui::test]
fn verified_return_slider_keeps_atomic_history_and_reverts_refused_edits(cx: &mut TestAppContext) {
    use hex_session::{DocumentAudioMode, service::Request};
    let (_browser, mut visual, editor, tracks) = actor_fixture_in_mode(
        cx,
        super::super::test_environment::Environment::new().unwrap(),
        DocumentAudioMode::VerifiedStreaming,
    );
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::AddReturnTrack {
            name: "bus".into()
        })));
    });
    pump(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && !view
                .state
                .as_ref()
                .unwrap()
                .project
                .return_tracks()
                .is_empty()
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.document_audio_mode,
            DocumentAudioMode::VerifiedStreaming
        )
    });
    let id = editor.read_with(cx, |view, _| {
        view.state.as_ref().unwrap().project.return_tracks()[0].id()
    });
    let field_id = Box::leak(format!("return-volume-{}-value", id.value()).into_boxed_str());
    let rail_id = Box::leak(format!("return-volume-{}-slider", id.value()).into_boxed_str());
    let field = visual.debug_bounds(field_id).unwrap().center();
    visual.simulate_click(field, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("-6 db");
    editor.update(cx, |view, cx| {
        assert!(super::super::device_panel::finalize_controls(view, cx))
    });
    let expected = 10_f32.powf(-6.0 / 20.0);
    pump(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && (view.state.as_ref().unwrap().project.return_tracks()[0].gain() - expected).abs()
                < 1e-5
    });
    editor.read_with(cx, |view, _| {
        assert!(view.device_gesture.snapshot().is_none());
        assert!(view.audible_playback.is_none(), "no resident fallback");
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(project.master_gain(), 1.0);
        assert_eq!(project.track(tracks[0]).unwrap().gain(), 0.5);
        assert_eq!(project.track(tracks[1]).unwrap().gain(), 1.5);
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.state.as_ref().unwrap().project.return_tracks()[0].gain() == 1.0
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
    pump(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && (view.state.as_ref().unwrap().project.return_tracks()[0].gain() - expected).abs()
                < 1e-5
    });
    visual.run_until_parked();
    let rail = visual.debug_bounds(rail_id).unwrap();
    let start = point(rail.left() + px(10.0), rail.center().y);
    let end = start - point(px(0.0), px(18.0));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            view.error
                .as_deref()
                .is_some_and(|s| s.contains("resident"))
        );
        assert!(view.device_gesture.snapshot().is_none());
        assert!(
            (view.state.as_ref().unwrap().project.return_tracks()[0].gain() - expected).abs()
                < 1e-5
        );
    });
    let field = visual.debug_bounds(field_id).unwrap().center();
    visual.simulate_click(field, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("-12 db");
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::Gain {
            track: tracks[0],
            gain: 0.2
        })))
    });
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        assert!(
            (view.state.as_ref().unwrap().project.return_tracks()[0].gain() - expected).abs()
                < 1e-5
        );
        assert!(
            (view.track_gain_controls.mixer[&super::MixerKey::ReturnGain(id)]
                .entity
                .read(cx)
                .value()
                - f64::from(expected))
            .abs()
                < 1e-5
        );
        assert!(
            view.error
                .as_deref()
                .is_some_and(|s| s.contains("current edit") && !s.contains("resident"))
        );
    });
}
