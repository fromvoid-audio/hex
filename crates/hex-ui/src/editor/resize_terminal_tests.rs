//! Terminal-core controls use real Service events; guard-only cases are explicit.
use super::{PendingTrim, ResizeCommand};
use crate::editor::{EditorView, external_audio_drop_ui_test::BatchFiles, viewport::Viewport};
use gpui::{AppContext, Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext};
use hex_project::{BeatTime, ClipId, Command, TrackId, TrimEdge};
use hex_session::service::{Event, Request, Service};
use std::time::Duration;

struct Fixture {
    _files: BatchFiles,
    window: gpui::AnyWindowHandle,
    editor: Entity<EditorView>,
    track: TrackId,
    clip: ClipId,
}
fn event(service: &Service) -> Event {
    let until = crate::test_wait::hang_deadline();
    loop {
        if let Ok(event) = service.try_event() {
            return event;
        }
        assert!(
            std::time::Instant::now() < until,
            "event: hang guard expired"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn ok(service: &Service) -> Event {
    let event = event(service);
    assert!(event.error.is_none(), "{:?}", event.error);
    event
}
fn fixture(cx: &mut TestAppContext) -> Fixture {
    cx.update(crate::editor::bind_editor_keys);
    let files = BatchFiles::new();
    let wav = files.wav("terminal-core.wav", 48_000, 96_000);
    let directory = wav.parent().unwrap();
    std::fs::create_dir(directory.join("samples")).unwrap();
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
    ok(&service);
    service.submit(Request::Import(wav.clone())).unwrap();
    let state = ok(&service).state;
    let track = state.project.tracks()[0].id();
    let clip = state.project.tracks()[0].clips()[0].id();
    service
        .submit(Request::Edit(Command::SetClipLooping {
            clip,
            looping: true,
        }))
        .unwrap();
    let state = ok(&service).state;
    let window = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(window, |_, window, cx| {
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_browser_directory(window, cx, service, directory)
            });
            editor.update(cx, |v, cx| {
                assert_eq!(v.browser.directory, directory.join("samples"));
                assert_eq!(v.browser.places, vec![directory.join("samples")]);
                v.state = Some(state);
                v.focus.focus(window);
                cx.notify();
            });
            editor
        })
        .unwrap();
    Fixture {
        _files: files,
        window,
        editor,
        track,
        clip,
    }
}
const OWNED: Viewport = Viewport {
    start: 28_800,
    length: 672_000,
};
fn install(v: &mut EditorView, clip: ClipId) -> Request {
    // Controlled UI queue seam. The separate resize_pan_tests prove real
    // pointer capture produces this transient viewport and accepted release.
    let request = Request::Edit(Command::TrimClipEdge {
        clip,
        edge: TrimEdge::Right,
        frame: 104_000,
    });
    let state = v.state.as_ref().unwrap();
    let original = state.project.clip(clip).unwrap().clone();
    let asset_frames = state
        .project
        .asset(original.asset())
        .unwrap()
        .playback_frames();
    let prospective = original
        .preview_edge_trim_with_tempo(
            TrimEdge::Right,
            104_000,
            asset_frames,
            state.project.tempo_map(),
        )
        .unwrap();
    let revision = state.revision;
    let track = state.project.tracks()[0].id();
    v.viewport = Some(OWNED);
    v.pending_trim = Some(PendingTrim {
        generation: v.project_generation,
        revision,
        track,
        command: ResizeCommand::from_request(&request).unwrap(),
        remaining: None,
        scroll_viewport: Some(OWNED),
        display: super::PendingTrimDisplay::Audio {
            original,
            prospective: Box::new(prospective),
            asset_frames,
        },
    });
    request
}
fn growth(track: TrackId) -> Request {
    Request::Edit(Command::PlaceMidiClip {
        track,
        start: BeatTime::from_quarters(512).unwrap(),
        length: BeatTime::ONE,
    })
}
fn normalized(v: &EditorView) -> Viewport {
    assert!(v.pending_trim.is_none());
    let viewport = v.viewport.unwrap();
    assert_eq!(viewport.start, 0);
    assert_eq!(viewport.length, OWNED.length);
    viewport
}

#[gpui::test]
fn terminal_fifo_counts_real_ordinary_device_and_tuning_completions(cx: &mut TestAppContext) {
    for kind in 0..3 {
        let f = fixture(cx);
        let mut visual = VisualTestContext::from_window(f.window, cx);
        f.editor.update(&mut visual, |v, _| {
            let resize = install(v, f.clip);
            let earlier = match kind {
                0 => Request::Edit(Command::RenameClip {
                    clip: f.clip,
                    name: "independent name".into(),
                }),
                1 => Request::DeviceGesture(hex_session::device_gesture::GestureRequest::Cancel {
                    key: hex_session::device_gesture::GestureKey {
                        generation: v.project_generation,
                        token: 901,
                    },
                }),
                _ => Request::TuningGesture(
                    hex_session::tuning_gesture::TuningGestureRequest::Cancel {
                        key: hex_session::tuning_gesture::TuningGestureKey {
                            generation: v.project_generation,
                            token: 902,
                        },
                    },
                ),
            };
            assert!(v.enqueue(earlier));
            assert!(
                v.pending_trim.as_ref().unwrap().remaining.is_none(),
                "unrelated submission cannot stamp resize"
            );
            assert!(v.enqueue(resize));
            assert_eq!(v.pending_trim.as_ref().unwrap().remaining, Some(2));
            let earlier = event(&v.service);
            assert!(earlier.request_complete);
            assert_eq!(earlier.gesture_request, kind == 1);
            assert_eq!(earlier.tuning_gesture_request, kind == 2);
            v.receive(earlier);
            assert_eq!(v.pending_trim.as_ref().unwrap().remaining, Some(1));
            assert_eq!(
                v.viewport,
                Some(OWNED),
                "earlier completion cannot retire viewport"
            );
            let terminal = ok(&v.service);
            assert!(terminal.request_complete);
            v.receive(terminal);
            normalized(v);
            assert_eq!(v.worker_requests_pending, 0);
            let project = &v.state.as_ref().unwrap().project;
            assert_eq!(project.clip(f.clip).unwrap().duration(), 104_000);
            if kind == 0 {
                assert_eq!(project.clip_name(f.clip), Some("independent name"));
            }
            eprintln!("terminal FIFO real kind={kind}: earlier retained, own terminal normalized");
        });
    }
}

#[gpui::test]
fn terminal_deferred_match_normalizes_before_later_growth_in_one_receive_batch(
    cx: &mut TestAppContext,
) {
    let f = fixture(cx);
    let mut visual = VisualTestContext::from_window(f.window, cx);
    f.editor.update(&mut visual, |v, cx| {
        assert!(v.enqueue(Request::Edit(Command::RenameClip {
            clip: f.clip,
            name: "already in flight".into()
        })));
        let resize = install(v, f.clip);
        v.pending_requests
            .push_back(Request::Edit(Command::RenameClip {
                clip: f.clip,
                name: "queued first".into(),
            }));
        assert!(v.enqueue(resize));
        assert!(v.enqueue(growth(f.track)));
        assert_eq!(v.worker_requests_pending, 1);
        assert!(v.pending_trim.as_ref().unwrap().remaining.is_none());
        let old_ack = ok(&v.service);
        assert!(old_ack.request_complete);
        v.receive(old_ack);
        assert_eq!(v.worker_requests_pending, 0);
        assert!(
            v.pending_trim.as_ref().unwrap().remaining.is_none(),
            "an old ack cannot complete a request still in the UI queue"
        );
        assert_eq!(v.viewport, Some(OWNED));
        // Real controller drain stamps only the matching second request.
        v.tick(cx);
        assert_eq!(v.pending_trim.as_ref().unwrap().remaining, Some(2));
        assert_eq!(v.worker_requests_pending, 3);
        let events = [ok(&v.service), ok(&v.service), ok(&v.service)];
        assert!(events[1].state.timeline_frames < OWNED.end().unwrap());
        assert!(events[2].state.timeline_frames > OWNED.end().unwrap());
        // Deterministic receive-drain batch: real worker events, no intervening
        // render/tick. End-of-drain clamping would already see the grown extent.
        for event in events {
            v.receive(event);
        }
        normalized(v);
        assert_eq!(v.worker_requests_pending, 0);
        assert_eq!(
            v.state.as_ref().unwrap().project.clip_name(f.clip),
            Some("queued first")
        );
        eprintln!(
            "terminal deferred matching/real three-event receive batch: no hidden pan resurrection"
        );
    });
}

#[gpui::test]
fn terminal_preserves_explicit_fit_zoom_and_pan(cx: &mut TestAppContext) {
    for navigation in 0..3 {
        let f = fixture(cx);
        let mut visual = VisualTestContext::from_window(f.window, cx);
        f.editor.update(&mut visual, |v, _| {
            let resize = install(v, f.clip);
            assert!(v.enqueue(resize));
            match navigation {
                0 => v.fit_timeline(),
                1 => v.zoom_timeline(true),
                _ => {
                    v.zoom_timeline(true);
                    v.pan_timeline(true);
                }
            }
            let explicit = v.viewport;
            assert_ne!(explicit, Some(OWNED));
            v.receive(ok(&v.service));
            assert!(v.pending_trim.is_none());
            assert_eq!(
                v.viewport, explicit,
                "terminal cannot overwrite newer navigation"
            );
            eprintln!("terminal explicit navigation={navigation} preserved {explicit:?}");
        });
    }
}

#[gpui::test]
fn terminal_local_retirement_does_not_clobber_a_new_actual_pointer_capture(
    cx: &mut TestAppContext,
) {
    let f = fixture(cx);
    let mut visual = VisualTestContext::from_window(f.window, cx);
    visual.update(|window, _| window.activate_window());
    let retired = f.editor.update(&mut visual, |v, _| {
        let _ = install(v, f.clip);
        v.pending_trim.take().unwrap().into_viewport()
    });
    visual.run_until_parked();
    let selector = Box::leak(format!("clip-trim-right-{}", f.clip.value()).into_boxed_str());
    let down = visual.debug_bounds(selector).unwrap().center();
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        gpui::point(down.x - gpui::px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    f.editor.update(&mut visual, |v, _| {
        assert!(
            v.active_trim_drag.is_some(),
            "actual new capture, not a fabricated payload"
        );
        // Direct local-token guard control, not a claim that receive yields
        // between taking its token and installing state in ordinary runtime.
        v.normalize_completed_trim_viewport(Some(retired));
        assert_eq!(v.viewport, Some(OWNED));
        assert!(v.pending_trim.is_none());
    });
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(down, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
}

#[gpui::test]
fn terminal_project_replacement_and_generation_cannot_normalize_another_document(
    cx: &mut TestAppContext,
) {
    let f = fixture(cx);
    let mut visual = VisualTestContext::from_window(f.window, cx);
    f.editor.update(&mut visual, |v, _| {
        let _ = install(v, f.clip);
        let generation = v.project_generation;
        assert!(v.enqueue(Request::NewWithAudioMode {
            mode: hex_session::DocumentAudioMode::Resident,
            reservation: None
        }));
        let replacement = ok(&v.service);
        assert!(replacement.project_replaced && replacement.request_complete);
        v.receive(replacement);
        assert!(v.pending_trim.is_none());
        assert!(v.project_generation > generation);
        assert!(v.viewport.is_none(), "new document keeps its own fit state");
        assert!(v.state.as_ref().unwrap().project.clip(f.clip).is_none());
    });
    let f = fixture(cx);
    let mut visual = VisualTestContext::from_window(f.window, cx);
    f.editor.update(&mut visual, |v, _| {
        let _ = install(v, f.clip);
        // Explicit generation guard control; do not manufacture a replacement
        // Event or claim this is ordinary pointer input.
        v.pending_trim.as_mut().unwrap().generation = v.project_generation.saturating_add(1);
        assert!(v.retire_pending_trim());
        assert_eq!(v.viewport, Some(OWNED));
    });
}

#[gpui::test]
fn terminal_deferred_owner_survives_full_ui_queue_and_failure_retirement_is_idempotent(
    cx: &mut TestAppContext,
) {
    let f = fixture(cx);
    let mut visual = VisualTestContext::from_window(f.window, cx);
    f.editor.update(&mut visual, |v, _| {
        let resize = install(v, f.clip);
        for _ in 0..31 {
            v.pending_requests.push_back(Request::Edit(Command::Gain {
                track: f.track,
                gain: 0.75,
            }));
        }
        assert!(v.enqueue(resize));
        assert_eq!(v.pending_requests.len(), 32);
        assert!(
            !v.enqueue(growth(f.track)),
            "real bounded UI queue refuses unrelated excess"
        );
        assert!(v.pending_trim.as_ref().unwrap().remaining.is_none());
        assert_eq!(v.viewport, Some(OWNED));
        // Direct shared failure/disconnect retirement hook. No actual worker
        // crash/closed channel or impossible resize SubmitError::Busy is faked.
        assert!(v.retire_pending_trim());
        normalized(v);
        let after = v.viewport;
        assert!(!v.retire_pending_trim());
        assert_eq!(v.viewport, after);
        assert_eq!(
            v.pending_requests.len(),
            32,
            "retirement cannot erase independent queued work"
        );
    });
}

#[gpui::test]
fn terminal_uses_current_loop_and_open_gain_lane_display_extent(cx: &mut TestAppContext) {
    for gain in [false, true] {
        let f = fixture(cx);
        let mut visual = VisualTestContext::from_window(f.window, cx);
        f.editor.update(&mut visual, |v, _| {
            let extent = if gain {
                v.gain_lane.track = Some(f.track);
                Command::UpsertTrackGainPoint {
                    track: f.track,
                    point: hex_project::GainPoint {
                        incoming_gain: None,
                        beat: BeatTime::from_quarters(20).unwrap(),
                        gain: 1.0,
                        outgoing: hex_project::GainInterpolation::Step,
                    },
                }
            } else {
                Command::SetLoopRegion {
                    start: 0,
                    end: 480_000,
                    enabled: true,
                }
            };
            assert!(v.enqueue(Request::Edit(extent)));
            v.receive(ok(&v.service));
            let resize = install(v, f.clip);
            assert!(v.enqueue(resize));
            v.receive(ok(&v.service));
            assert!(v.pending_trim.is_none());
            assert_eq!(
                v.visible_viewport(v.state.as_ref().unwrap().timeline_frames),
                OWNED
            );
            assert_eq!(
                v.viewport,
                Some(OWNED),
                "legal authored display extent must not be clamped away"
            );
            eprintln!(
                "terminal preserves actual display extent gain={gain} horizon={}",
                v.state.as_ref().unwrap().timeline_frames
            );
        });
    }
}

#[gpui::test]
fn restart_offline_completion_cannot_retire_a_later_resize_ordinal(cx: &mut TestAppContext) {
    let f = fixture(cx);
    let mut visual = VisualTestContext::from_window(f.window, cx);
    f.editor.update(&mut visual, |v, _| {
        assert!(v.restart_audio_for_current_document());
        // Service::new has NO audio adapter: this exercises the actual queued
        // completion without opening an OS device or substituting a fake ack.
        let resize = install(v, f.clip);
        assert!(v.enqueue(resize));
        let submitted = v.worker_requests_pending;
        let ordinal = v.pending_trim.as_ref().unwrap().remaining;
        let restart = event(&v.service);
        assert!(restart.request_complete);
        assert_eq!(restart.error.as_deref(), Some("audio adapter unavailable"));
        v.receive(restart);
        eprintln!("restart offline earlier completion: counted={submitted} resize_ordinal={ordinal:?} owner_retained={}", v.pending_trim.is_some());
        assert!(v.pending_trim.is_some(), "actual earlier restart completion cannot be the resize terminal");
        assert_eq!(submitted, 2, "actual restart is counted exactly once alongside actual resize");
        assert_eq!(v.pending_trim.as_ref().unwrap().remaining, Some(1));
        assert_eq!(v.viewport, Some(OWNED));
        v.receive(ok(&v.service));
        normalized(v);
        assert_eq!(v.worker_requests_pending, 0);
    });
}

#[gpui::test]
fn restart_offline_pending_work_blocks_actual_resize_capture_until_completion(
    cx: &mut TestAppContext,
) {
    let f = fixture(cx);
    let mut visual = VisualTestContext::from_window(f.window, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    let restart = f.editor.update(&mut visual, |v, _| {
        assert!(v.restart_audio_for_current_document());
        // Hold the REAL worker event outside the UI receiver until after input,
        // so controller polling cannot turn this into a timing-dependent race.
        let event = event(&v.service);
        assert!(event.request_complete);
        assert_eq!(event.error.as_deref(), Some("audio adapter unavailable"));
        event
    });
    let selector = Box::leak(format!("clip-trim-right-{}", f.clip.value()).into_boxed_str());
    let down =
        crate::editor::clip_drag_ui_test::trim_grab_point(visual.debug_bounds(selector).unwrap());
    let moved = gpui::point(down.x - gpui::px(12.0), down.y);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    f.editor.read_with(&visual, |v, _| {
        assert!(
            v.active_trim_drag.is_none(),
            "unacknowledged real restart fences actual pointer admission"
        );
        assert!(v.pending_trim.is_none());
        assert_eq!(v.worker_requests_pending, 1);
    });
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    f.editor.update(&mut visual, |v, cx| {
        v.receive(restart);
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    f.editor
        .read_with(&visual, |v, _| assert!(v.active_trim_drag.is_some()));
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    f.editor.update(&mut visual, |v, _| {
        assert_eq!(v.worker_requests_pending, 0);
        v.restart_request_id = u64::MAX;
        assert!(
            !v.restart_audio_for_current_document(),
            "real exhausted-id refusal submits nothing"
        );
        assert_eq!(v.worker_requests_pending, 0, "no count on refusal");
    });
}
