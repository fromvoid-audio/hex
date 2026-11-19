//! Actual Shift input → metadata preview → accepted queue → worker ack/history.
use super::super::{EditorView, musical_time::SnapGrid};
use gpui::{
    AppContext, Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px,
};
use hex_project::{ClipId, Command, TrimEdge};
use hex_session::service::{Event, Request, Service};
use std::time::{Duration, Instant};

fn next(service: &Service) -> Event {
    let until = crate::test_wait::hang_deadline();
    loop {
        if let Ok(event) = service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            return event;
        }
        assert!(
            std::time::Instant::now() < until,
            "next: hang guard expired"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}
pub(super) fn pump(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    let until = crate::test_wait::hang_deadline();
    loop {
        let done = editor.update(&mut *visual, |v, cx| {
            if v.tick(cx) {
                cx.notify();
            }
            v.worker_requests_pending == 0 && v.pending_requests.is_empty()
        });
        if done {
            visual.run_until_parked();
            return;
        }
        assert!(
            Instant::now() < until,
            "worker pending at deadline: {:?}",
            editor.read_with(&*visual, |v, _| (
                v.worker_requests_pending,
                v.pending_requests.len(),
                v.state.as_ref().map(|s| s.revision),
                v.error.clone()
            ))
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}
pub(super) fn fixture(
    cx: &mut TestAppContext,
) -> (
    super::super::external_audio_drop_ui_test::BatchFiles,
    gpui::AnyWindowHandle,
    Entity<EditorView>,
    ClipId,
) {
    fixture_at_rate(cx, 48_000)
}
fn fixture_at_rate(
    cx: &mut TestAppContext,
    sample_rate: u32,
) -> (
    super::super::external_audio_drop_ui_test::BatchFiles,
    gpui::AnyWindowHandle,
    Entity<EditorView>,
    ClipId,
) {
    cx.update(super::super::bind_editor_keys);
    let files = super::super::external_audio_drop_ui_test::BatchFiles::new();
    let wav = files.wav("resize-release.wav", sample_rate, sample_rate * 2);
    let directory = wav.parent().unwrap();
    std::fs::create_dir(directory.join("samples")).unwrap();
    let service = Service::new(sample_rate, 32, 8_000_000).unwrap();
    next(&service);
    service.submit(Request::Import(wav.clone())).unwrap();
    let event = next(&service);
    let clip = event.state.project.tracks()[0].clips()[0].id();
    let track = event.state.project.tracks()[0].id();
    service
        .submit(Request::Edit(Command::MoveClip {
            clip,
            track,
            start: u64::from(sample_rate) * 2,
        }))
        .unwrap();
    let event = next(&service);
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_browser_directory(window, cx, service, directory)
            });
            editor.update(cx, |v, cx| {
                assert_eq!(v.browser.directory, directory.join("samples"));
                assert_eq!(v.browser.places, vec![directory.join("samples")]);
                v.state = Some(event.state);
                v.snap_grid = SnapGrid::default().with_platform_modifier(true);
                v.focus.focus(window);
                cx.notify();
            });
            editor
        })
        .unwrap();
    (files, handle, editor, clip)
}
#[gpui::test]
fn shift_resize_preserves_visible_extent_through_release_and_real_worker_ack(
    cx: &mut TestAppContext,
) {
    for edge in [TrimEdge::Right, TrimEdge::Left] {
        check_audio_release(cx, edge, true);
    }
}

#[gpui::test]
fn normal_resize_preserves_visible_extent_and_source_through_release_and_ack(
    cx: &mut TestAppContext,
) {
    for edge in [TrimEdge::Right, TrimEdge::Left] {
        check_audio_release(cx, edge, false);
    }
}

fn check_audio_release(cx: &mut TestAppContext, edge: TrimEdge, stretch: bool) {
    let (_files, handle, editor, clip) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let name = if edge == TrimEdge::Right {
        "right"
    } else {
        "left"
    };
    let selector = Box::leak(format!("clip-trim-{name}-{}", clip.value()).into_boxed_str());
    let down = visual.debug_bounds(selector).unwrap().center();
    let shift = Modifiers {
        shift: stretch,
        ..Modifiers::default()
    };
    let direction = if !stretch && edge == TrimEdge::Right {
        -1.0
    } else {
        1.0
    };
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    visual.simulate_mouse_down(down, MouseButton::Left, shift);
    visual.simulate_mouse_move(
        point(down.x + px(12.0 * direction), down.y),
        MouseButton::Left,
        shift,
    );
    visual.run_until_parked();
    let target = point(down.x + px(63.0 * direction), down.y);
    visual.simulate_mouse_move(target, MouseButton::Left, shift);
    visual.run_until_parked();
    let desired = editor.read_with(&visual, |v, _| {
        let drag = v.active_trim_drag.as_ref().expect("actual held Shift edge");
        assert_eq!(drag.stretch_modifier.get(), stretch);
        let p = drag.preview.borrow().as_ref().unwrap().clip.clone();
        assert_eq!(
            p.audio_edit().is_some_and(|edit| edit.domain.warp_enabled),
            stretch
        );
        assert_ne!(p.duration(), 96_000);
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        p
    });
    let held = visual
        .debug_bounds("trim-live-preview")
        .expect("actual held overlay");
    visual.simulate_mouse_up(target, MouseButton::Left, shift);
    visual.run_until_parked();
    let queued = editor.read_with(&visual, |v, _| {
        (v.worker_requests_pending, v.last_enqueued_edit.clone())
    });
    let released = visual
        .debug_bounds("trim-live-preview")
        .or_else(|| {
            visual.debug_bounds(Box::leak(
                format!("clip-visual-{}", clip.value()).into_boxed_str(),
            ))
        })
        .expect("a visible released clip");
    pump(&editor, &mut visual);
    let actual = editor.read_with(&visual, |v, _| {
        assert!(v.error.is_none(), "{:?}", v.error);
        v.state
            .as_ref()
            .unwrap()
            .project
            .clip(clip)
            .unwrap()
            .clone()
    });
    eprintln!(
        "release edge={edge:?} queued={} desired={}..{} releasedwidth={} heldwidth={} accepted={}..{} command={:?}",
        queued.0,
        desired.start(),
        desired.start() + desired.duration(),
        f32::from(released.size.width),
        f32::from(held.size.width),
        actual.start(),
        actual.start() + actual.duration(),
        queued.1
    );
    assert!(if stretch {
        matches!(queued.1,Some(Command::StretchAudioClipEdge{clip:id,edge:requested,..})if id==clip&&requested==edge)
    } else {
        matches!(queued.1,Some(Command::TrimClipEdge{clip:id,edge:requested,..})if id==clip&&requested==edge)
    });
    assert_eq!(
        actual, desired,
        "accepted Shift commit must retain exact warp/source metadata and extent"
    );
    assert!(
        (f32::from(released.left()) - f32::from(held.left())).abs() < 1.0
            && (f32::from(released.size.width) - f32::from(held.size.width)).abs() < 1.0,
        "accepted release must not expose old canonical extent while ack is pending"
    );
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
    visual.simulate_keystrokes("cmd-shift-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.clip(clip), Some(&desired))
    });
}

#[gpui::test]
fn midi_resize_keeps_released_geometry_until_actual_worker_ack_and_one_history_step(
    cx: &mut TestAppContext,
) {
    check_midi_release(cx, None);
}

#[gpui::test]
fn browser_release_commits_last_shown_midi_resize_with_one_history_step(cx: &mut TestAppContext) {
    for child in ["browser-section-samples", "browser-audition-waveform"] {
        check_midi_release(cx, Some(child));
    }
}

fn check_midi_release(cx: &mut TestAppContext, browser_child: Option<&'static str>) {
    use hex_project::BeatTime;
    for edge in [TrimEdge::Right, TrimEdge::Left] {
        let (_files, handle, editor, _) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::AddTrack {
                name: "midi release".into()
            })));
        });
        pump(&editor, &mut visual);
        let track = editor.read_with(&visual, |v, _| {
            v.state.as_ref().unwrap().project.tracks()[1].id()
        });
        // Keep the real worker's created ID rather than fabricating a UI-only clip.
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::PlaceMidiClip {
                track,
                start: BeatTime::ONE,
                length: BeatTime::from_quarters(4).unwrap(),
            })));
        });
        pump(&editor, &mut visual);
        let clip = editor.read_with(&visual, |v, _| {
            v.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .midi_clips()[0]
                .id()
        });
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::AddMidiNote {
                clip,
                start: BeatTime::ONE,
                duration: BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 100,
            })));
        });
        pump(&editor, &mut visual);
        let (before, original) = editor.read_with(&visual, |v, _| {
            let p = &v.state.as_ref().unwrap().project;
            (p.snapshot(), p.midi_clip(clip).unwrap().clone())
        });
        let name = if edge == TrimEdge::Right {
            "right"
        } else {
            "left"
        };
        let selector =
            Box::leak(format!("midi-clip-trim-{name}-{}", clip.value()).into_boxed_str());
        let down = visual.debug_bounds(selector).unwrap().center();
        let direction = if edge == TrimEdge::Right { -1.0 } else { 1.0 };
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(down.x + px(12.0 * direction), down.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.run_until_parked();
        let target = point(down.x + px(55.0 * direction), down.y);
        visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let desired = editor.read_with(&visual, |v, _| {
            v.active_midi_trim_drag
                .as_ref()
                .unwrap()
                .preview
                .borrow()
                .unwrap()
        });
        let held = visual.debug_bounds("midi-trim-live-preview").unwrap();
        let release = if let Some(child) = browser_child {
            let position = visual
                .debug_bounds(child)
                .expect("actual browser child")
                .center();
            visual.simulate_mouse_move(position, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            editor.read_with(&visual, |v, _| {
                assert_eq!(
                    *v.active_midi_trim_drag.as_ref().unwrap().preview.borrow(),
                    Some(desired)
                );
            });
            position
        } else {
            target
        };
        visual.simulate_mouse_up(release, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let released = visual
            .debug_bounds("midi-trim-live-preview")
            .expect("accepted released MIDI preview");
        assert_eq!(released, held);
        editor.read_with(&visual, |v, _| {
            assert!(v.active_midi_trim_drag.is_none());
            assert!(v.pending_trim.is_some());
            assert_eq!(v.worker_requests_pending, 1);
            assert!(v.pending_requests.is_empty());
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        });
        pump(&editor, &mut visual);
        let actual = editor.read_with(&visual, |v, _| {
            assert!(v.pending_trim.is_none());
            let c = v.state.as_ref().unwrap().project.midi_clip(clip).unwrap();
            assert_eq!(
                (c.start(), c.length(), c.source_start()),
                (desired.start(), desired.length(), desired.source_start())
            );
            assert_eq!(c.source().notes(), original.source().notes());
            assert_eq!(
                c.source().source_length(),
                original.source().source_length()
            );
            assert_eq!(
                (c.source().loop_start(), c.source().loop_end()),
                (original.source().loop_start(), original.source().loop_end())
            );
            c.clone()
        });
        if let Some(child) = browser_child {
            eprintln!(
                "browser midi release edge={edge:?} child={child} accepted={:?}..{:?} source={:?}",
                actual.start(),
                actual.length(),
                actual.source_start()
            );
        }
        visual.simulate_keystrokes("cmd-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
        });
        visual.simulate_keystrokes("cmd-shift-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.state.as_ref().unwrap().project.midi_clip(clip),
                Some(&actual)
            )
        });
    }
}

#[gpui::test]
fn resize_escape_stale_generation_pending_refusal_and_no_motion_do_not_retain_a_commit(
    cx: &mut TestAppContext,
) {
    for case in 0..4 {
        let (_files, handle, editor, clip) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let selector = Box::leak(format!("clip-trim-right-{}", clip.value()).into_boxed_str());
        let down = visual.debug_bounds(selector).unwrap().center();
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        visual.simulate_mouse_down(down, MouseButton::Left, shift);
        let target = if case == 3 {
            down
        } else {
            point(down.x + px(43.0), down.y)
        };
        if case != 3 {
            visual.simulate_mouse_move(point(down.x + px(12.0), down.y), MouseButton::Left, shift);
            visual.run_until_parked();
            visual.simulate_mouse_move(target, MouseButton::Left, shift);
            visual.run_until_parked();
        }
        match case {
            0 => visual.simulate_keystrokes("escape"),
            1 => editor.update(&mut visual, |v, _| v.project_generation += 1),
            2 => editor.update(&mut visual, |v, _| v.close_prompt_open = true),
            _ => {}
        }
        visual.simulate_mouse_up(target, MouseButton::Left, shift);
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert!(v.pending_trim.is_none(), "case{case}");
            assert!(v.active_trim_drag.is_none(), "case{case}");
            assert_eq!(v.worker_requests_pending, 0);
            assert!(v.pending_requests.is_empty());
            assert!(v.last_enqueued_edit.is_none());
            eprintln!("browser release cancellation case={case}: no command or pending owner");
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        });
    }
}

fn install_pending_audio(v: &mut EditorView, clip: ClipId) -> Request {
    let state = v.state.as_ref().unwrap();
    let original = state.project.clip(clip).unwrap().clone();
    let track = state
        .project
        .tracks()
        .iter()
        .find(|t| t.clips().iter().any(|c| c.id() == clip))
        .unwrap()
        .id();
    let frame = original.start() + original.duration() + 24_000;
    let asset_frames = state
        .project
        .asset(original.asset())
        .unwrap()
        .playback_frames();
    let prospective = hex_project::preview_audio_clip_stretch(
        &original,
        TrimEdge::Right,
        frame,
        state.project.tempo_map(),
        asset_frames,
    )
    .unwrap();
    let request = Request::Edit(Command::StretchAudioClipEdge {
        clip,
        edge: TrimEdge::Right,
        frame,
    });
    v.pending_trim = Some(super::PendingTrim {
        generation: v.project_generation,
        revision: state.revision,
        track,
        command: super::ResizeCommand::from_request(&request).unwrap(),
        remaining: None,
        scroll_viewport: None,
        display: super::PendingTrimDisplay::Audio {
            original,
            prospective: Box::new(prospective),
            asset_frames,
        },
    });
    request
}

#[gpui::test]
fn released_resize_survives_earlier_completion_until_project_replacement(cx: &mut TestAppContext) {
    let (_files, handle, editor, clip) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let clip_selector = Box::leak(format!("clip-visual-{}", clip.value()).into_boxed_str());
    let original = visual.debug_bounds(clip_selector).unwrap();

    // Hold the real later events outside the service queue so the repaint cannot
    // consume them before the intermediate frame has completed.
    let (replacement, terminal) = editor.update(&mut visual, |v, cx| {
        let resize = install_pending_audio(v, clip);
        assert!(v.enqueue(Request::Edit(Command::SetLoopRegion {
            start: 12_000,
            end: 36_000,
            enabled: true,
        })));
        assert!(v.enqueue(Request::NewWithAudioMode {
            mode: hex_session::DocumentAudioMode::Resident,
            reservation: None,
        }));
        assert!(v.enqueue(resize));
        assert_eq!(v.pending_trim.as_ref().unwrap().remaining, Some(3));
        let earlier = next(&v.service);
        let replacement = next(&v.service);
        let until = crate::test_wait::hang_deadline();
        let terminal = loop {
            if let Ok(event) = v.service.try_event() {
                break event;
            }
            assert!(std::time::Instant::now() < until, "released_resize_survives_earlier_completion_until_project_replacement: hang guard expired");
            std::thread::sleep(Duration::from_millis(2));
        };
        assert!(earlier.request_complete && !earlier.project_replaced);
        assert!(replacement.request_complete && replacement.project_replaced);
        assert!(terminal.request_complete && terminal.error.is_some());
        v.receive(earlier);
        assert_eq!(v.pending_trim.as_ref().unwrap().remaining, Some(2));
        cx.notify();
        (replacement, terminal)
    });
    visual.run_until_parked();
    let preview = visual.debug_bounds("trim-live-preview").unwrap();
    assert!(preview.size.width > original.size.width);
    assert_eq!(
        visual.debug_bounds(clip_selector),
        Some(preview),
        "the hit surface follows the painted preview"
    );
    let track = editor.read_with(&visual, |v, _| {
        v.state.as_ref().unwrap().project.tracks()[0].id()
    });
    editor.read_with(&visual, |v, _| {
        let (_, displayed) = v.audio_trim_display(track, Some(clip)).unwrap();
        assert_eq!(displayed.duration(), 120_000);
    });
    let extended = point(
        original.right() + (preview.right() - original.right()) / 2.0,
        preview.origin.y + px(20.0),
    );
    visual.simulate_mouse_down(extended, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(extended, MouseButton::Right, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        let menu = v
            .context_menu
            .as_ref()
            .expect("preview extension hits the clip");
        assert_eq!(menu.items[0].label, "use accent flavor");
        assert!(v.pending_trim.is_some());
    });
    visual.simulate_keystrokes("escape");

    editor.update(&mut visual, |v, cx| {
        v.receive(replacement);
        assert!(v.pending_trim.is_none());
        assert!(v.state.as_ref().unwrap().project.clip(clip).is_none());
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("trim-live-preview").is_none());
    assert!(visual.debug_bounds(clip_selector).is_none());
    editor.update(&mut visual, |v, cx| {
        v.receive(terminal);
        assert!(v.pending_trim.is_none());
        assert!(v.state.as_ref().unwrap().project.clip(clip).is_none());
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("trim-live-preview").is_none());
    assert!(visual.debug_bounds(clip_selector).is_none());
}

#[gpui::test]
fn pending_resize_correlates_real_fifo_acceptance_failure_and_deferred_submission(
    cx: &mut TestAppContext,
) {
    // Exercise the UI queue seam with real requests/events in a controlled order;
    // the rendered tests above cover actual pointer release into this same seam.
    for fail_resize in [false, true] {
        let (_files, handle, editor, clip) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        let track = editor.read_with(&visual, |v, _| {
            v.state.as_ref().unwrap().project.tracks()[0].id()
        });
        editor.update(&mut visual, |v, _| {
            let request = install_pending_audio(v, clip);
            let earlier = if fail_resize {
                Command::RemoveClip(clip)
            } else {
                Command::SetLoopRegion {
                    start: 12_000,
                    end: 36_000,
                    enabled: true,
                }
            };
            assert!(v.enqueue(Request::Edit(earlier)));
            assert!(
                v.pending_trim.as_ref().unwrap().remaining.is_none(),
                "an unrelated submission cannot claim this preview"
            );
            assert!(v.enqueue(request));
            assert_eq!(v.pending_trim.as_ref().unwrap().remaining, Some(2));
            // Read only one actual worker completion at a time, rather than tick
            // draining both and hiding early retirement by the unrelated ack.
            let first = next(&v.service);
            assert!(first.request_complete);
            v.receive(first);
            assert_eq!(v.pending_trim.as_ref().unwrap().remaining, Some(1));
            assert_eq!(
                v.audio_trim_display(track, None).is_some(),
                !fail_resize,
                "independent edits preserve preview; deletion invalidates its stable clip owner"
            );
        });
        let until = crate::test_wait::hang_deadline();
        loop {
            let completed = editor.update(&mut visual, |v, _| {
                if let Ok(event) = v.service.try_event() {
                    assert!(event.request_complete);
                    assert_eq!(event.error.is_some(), fail_resize);
                    v.receive(event);
                    assert!(
                        v.pending_trim.is_none(),
                        "the exact own terminal retires only this preview"
                    );
                    assert_eq!(v.worker_requests_pending, 0);
                    let p = &v.state.as_ref().unwrap().project;
                    if fail_resize {
                        assert!(p.clip(clip).is_none());
                    } else {
                        assert_eq!(p.clip(clip).unwrap().duration(), 120_000);
                        assert_eq!(p.loop_region().unwrap().start(), 12_000);
                    }
                    true
                } else {
                    false
                }
            });
            if completed {
                break;
            }
            assert!(
                std::time::Instant::now() < until,
                "pending_resize_correlates_real_fifo_acceptance_failure_and_deferred_submission: hang guard expired"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    // A request accepted by the bounded UI queue is not yet a submitted worker
    // request. Preserve that distinction, then mark it only in the real drain.
    let (_files, handle, editor, clip) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |v, _| {
        let request = install_pending_audio(v, clip);
        v.pending_requests.push_back(request);
        assert!(v.pending_trim.as_ref().unwrap().remaining.is_none());
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(v.pending_trim.is_none());
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .clip(clip)
                .unwrap()
                .duration(),
            120_000
        );
    });
}

#[gpui::test]
fn browser_release_commits_last_shown_audio_resize_after_actual_child_routing(
    cx: &mut TestAppContext,
) {
    for (edge, stretch, child, moved) in [
        (TrimEdge::Right, false, "browser-section-samples", true),
        (TrimEdge::Left, false, "browser-audition-waveform", true),
        (TrimEdge::Right, true, "browser-audition-waveform", false),
        (TrimEdge::Left, true, "browser-section-samples", true),
    ] {
        let (_files, handle, editor, clip) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        let name = if edge == TrimEdge::Right {
            "right"
        } else {
            "left"
        };
        let selector = Box::leak(format!("clip-trim-{name}-{}", clip.value()).into_boxed_str());
        let down = visual.debug_bounds(selector).unwrap().center();
        let modifiers = Modifiers {
            shift: stretch,
            ..Modifiers::default()
        };
        let direction = if edge == TrimEdge::Right && !stretch {
            -1.0
        } else {
            1.0
        };
        visual.simulate_mouse_down(down, MouseButton::Left, modifiers);
        visual.simulate_mouse_move(
            point(down.x + px(12.0 * direction), down.y),
            MouseButton::Left,
            modifiers,
        );
        visual.run_until_parked();
        visual.simulate_mouse_move(
            point(down.x + px(50.0 * direction), down.y),
            MouseButton::Left,
            modifiers,
        );
        visual.run_until_parked();
        let desired = editor.read_with(&visual, |v, _| {
            v.active_trim_drag
                .as_ref()
                .unwrap()
                .preview
                .borrow()
                .as_ref()
                .unwrap()
                .clip
                .clone()
        });
        assert_ne!(desired.duration(), 96_000);
        let browser = visual
            .debug_bounds(child)
            .expect("real rendered browser child")
            .center();
        if moved {
            visual.simulate_mouse_move(browser, MouseButton::Left, modifiers);
            visual.run_until_parked();
        }
        if moved {
            let viewport = editor.read_with(&visual, |v, _| v.viewport);
            let now = Instant::now();
            for elapsed in [0, 50, 100] {
                editor.update(&mut visual, |v, cx| {
                    v.tick_clip_gesture_autoscroll(now + Duration::from_millis(elapsed), cx);
                });
            }
            editor.read_with(&visual, |v, _| {
                assert_eq!(v.viewport, viewport, "browser hover must stop edge-pan")
            });
        }
        let entry_preview = editor.read_with(&visual, |v, _| {
            v.active_trim_drag
                .as_ref()
                .and_then(|d| d.preview.borrow().as_ref().map(|p| p.clip.clone()))
        });
        assert_eq!(
            entry_preview.as_ref(),
            Some(&desired),
            "browser entry must retain the shown valid preview"
        );
        visual.simulate_mouse_up(browser, MouseButton::Left, modifiers);
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.worker_requests_pending, 1);
            assert!(v.pending_requests.is_empty());
        });
        pump(&editor, &mut visual);
        editor.read_with(&visual,|v,_|{
            let state=v.state.as_ref().unwrap();
            eprintln!("browser release edge={edge:?} stretch={stretch} child={child} moved={moved} held={}..{} entry={:?} actual={:?} command={:?}",desired.start(),desired.start()+desired.duration(),entry_preview.as_ref().map(|p|(p.start(),p.duration())),state.project.clip(clip).map(|c|(c.start(),c.duration())),v.last_enqueued_edit);
            assert_eq!(state.project.clip(clip),Some(&desired),"release over real browser must retain last timeline preview");
            assert!(v.active_trim_drag.is_none()&&v.active_midi_trim_drag.is_none()&&v.pending_trim.is_none());
            assert!(v.active_browser_audio_drag.is_none()&&v.active_native_device_drag.is_none());
            assert_eq!(v.worker_requests_pending,0);
            assert!(v.pending_requests.is_empty());
            assert!(v.error.is_none(),"{:?}",v.error);
        });
        visual.simulate_keystrokes("cmd-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
        });
        visual.simulate_keystrokes("cmd-shift-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.clip(clip), Some(&desired))
        });
    }
}

#[gpui::test]
fn browser_resize_release_still_cancels_escape_stale_generation_and_foreign_focus(
    cx: &mut TestAppContext,
) {
    for case in 0..4 {
        let (_files, handle, editor, clip) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        let selector = Box::leak(format!("clip-trim-right-{}", clip.value()).into_boxed_str());
        let down = visual.debug_bounds(selector).unwrap().center();
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(down.x - px(45.0), down.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.run_until_parked();
        assert!(editor.read_with(&visual, |v, _| v.active_trim_drag.is_some()));
        let browser = visual
            .debug_bounds("browser-section-samples")
            .unwrap()
            .center();
        visual.simulate_mouse_move(browser, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        match case {
            0 => visual.simulate_keystrokes("escape"),
            1 => editor.update(&mut visual, |v, _| {
                v.project_generation += 1;
            }),
            2 => visual.update(|window, cx| {
                editor.update(cx, |v, _| {
                    v.audio_event_focus.focus(window);
                });
            }),
            _ => editor.update(&mut visual, |v, cx| {
                v.set_mode(super::super::EditorMode::Session);
                cx.notify();
            }),
        }
        visual.simulate_mouse_up(browser, MouseButton::Left, Modifiers::default());
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.state.as_ref().unwrap().project.snapshot(),
                before,
                "guard case{case}"
            );
            assert!(v.pending_trim.is_none() && v.active_trim_drag.is_none());
            assert!(v.pending_requests.is_empty() && v.worker_requests_pending == 0);
            assert!(v.last_enqueued_edit.is_none());
            eprintln!("browser release cancellation case={case}: no command or pending owner");
        });
    }
}

#[gpui::test]
fn browser_resize_stationary_command_preview_and_release_use_the_last_timeline_point(
    cx: &mut TestAppContext,
) {
    for stretch in [false, true] {
        for command_on_release in [false, true] {
            let (_files, handle, editor, clip) = fixture(cx);
            let mut visual = VisualTestContext::from_window(handle, cx);
            editor.update(&mut visual, |v, cx| {
                v.snap_grid = SnapGrid::default();
                cx.notify();
            });
            visual.run_until_parked();
            let before =
                editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
            let selector = Box::leak(format!("clip-trim-right-{}", clip.value()).into_boxed_str());
            let down = visual.debug_bounds(selector).unwrap().center();
            let ordinary = Modifiers {
                shift: stretch,
                ..Modifiers::default()
            };
            let command = Modifiers {
                platform: true,
                ..ordinary
            };
            visual.simulate_mouse_down(down, MouseButton::Left, ordinary);
            visual.simulate_mouse_move(
                point(down.x - px(12.0), down.y),
                MouseButton::Left,
                ordinary,
            );
            visual.run_until_parked();
            visual.simulate_mouse_move(
                point(down.x - px(43.7), down.y),
                MouseButton::Left,
                ordinary,
            );
            visual.run_until_parked();
            let browser = visual
                .debug_bounds("browser-audition-waveform")
                .unwrap()
                .center();
            visual.simulate_mouse_move(browser, MouseButton::Left, ordinary);
            visual.run_until_parked();
            let read_preview = |v: &EditorView| {
                v.active_trim_drag
                    .as_ref()
                    .unwrap()
                    .preview
                    .borrow()
                    .as_ref()
                    .unwrap()
                    .clip
                    .clone()
            };
            let snapped = editor.read_with(&visual, |v, _| read_preview(v));
            visual.simulate_modifiers_change(command);
            visual.run_until_parked();
            let free = editor.read_with(&visual, |v, _| read_preview(v));
            assert_ne!(
                free, snapped,
                "stationary command must refresh actual retained timeline preview"
            );
            visual.simulate_modifiers_change(ordinary);
            visual.run_until_parked();
            editor.read_with(&visual, |v, _| assert_eq!(read_preview(v), snapped));
            let (release, desired) = if command_on_release {
                visual.simulate_modifiers_change(command);
                visual.run_until_parked();
                (command, free)
            } else {
                (ordinary, snapped)
            };
            visual.simulate_mouse_up(browser, MouseButton::Left, release);
            pump(&editor, &mut visual);
            editor.read_with(&visual, |v, _| {
                assert_eq!(v.state.as_ref().unwrap().project.clip(clip), Some(&desired))
            });
            eprintln!(
                "browser stationary modifier stretch={stretch} command_on_release={command_on_release} accepted_duration={}",
                desired.duration()
            );
            visual.simulate_keystrokes("cmd-z");
            pump(&editor, &mut visual);
            editor.read_with(&visual, |v, _| {
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
            });
        }
    }
}

fn review_midi_clip(editor: &Entity<EditorView>, visual: &mut VisualTestContext) -> ClipId {
    use hex_project::BeatTime;
    editor.update(&mut *visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddTrack {
            name: "review midi source".into()
        })))
    });
    pump(editor, visual);
    let track = editor.read_with(&*visual, |v, _| {
        v.state
            .as_ref()
            .unwrap()
            .project
            .tracks()
            .last()
            .unwrap()
            .id()
    });
    editor.update(&mut *visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::PlaceMidiClip {
            track,
            start: BeatTime::ONE,
            length: BeatTime::from_quarters(4).unwrap(),
        })))
    });
    pump(editor, visual);
    let clip = editor.read_with(&*visual, |v, _| {
        v.state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .midi_clips()[0]
            .id()
    });
    editor.update(&mut *visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::ONE,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        })))
    });
    pump(editor, visual);
    clip
}

fn review_other_track_release(cx: &mut TestAppContext, midi: bool, outside: bool) {
    for edge in [TrimEdge::Left, TrimEdge::Right] {
        captured_release_case(cx, midi, edge, false, false, if outside { 1 } else { 0 });
    }
}

fn captured_release_case(
    cx: &mut TestAppContext,
    midi: bool,
    edge: TrimEdge,
    stretch: bool,
    enabled_warp: bool,
    destination: u8,
) {
    captured_release_cases(cx, midi, stretch, enabled_warp, [(edge, destination)]);
}

fn captured_release_cases(
    cx: &mut TestAppContext,
    midi: bool,
    stretch: bool,
    enabled_warp: bool,
    cases: impl IntoIterator<Item = (TrimEdge, u8)>,
) {
    let (_files, handle, editor, audio) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    let clip = if midi {
        review_midi_clip(&editor, &mut visual)
    } else {
        audio
    };
    if enabled_warp {
        let settings = editor.read_with(&visual, |v, _| {
            let p = &v.state.as_ref().unwrap().project;
            hex_project::default_audio_warp(
                p.clip(clip).unwrap(),
                p.tempo_map(),
                hex_project::WarpMode::Beats {
                    preserve: hex_project::BeatsPreserve::Transients,
                    tail: hex_project::BeatsTail::Forward,
                    decay: 100,
                },
            )
            .unwrap()
        });
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::SetAudioClipWarp {
                clip,
                settings: Some(settings)
            })))
        });
        pump(&editor, &mut visual);
    }
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddTrack {
            name: "other release target".into()
        })))
    });
    pump(&editor, &mut visual);
    let (before, other, other_before) = editor.read_with(&visual, |v, _| {
        let p = &v.state.as_ref().unwrap().project;
        (
            p.snapshot(),
            p.tracks().last().unwrap().id(),
            p.tracks().last().unwrap().clone(),
        )
    });
    let (viewport, selected_track, selected_clip) = editor.read_with(&visual, |v, _| {
        (v.viewport, v.selected_track, v.selected_clip)
    });
    eprintln!("CAPTURED_RELEASE_FIXTURE midi={midi} shift={stretch} warp={enabled_warp}");
    for (edge, destination) in cases {
        let outside = destination != 0;
        editor.update(&mut visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            assert!(v.error.is_none() && v.pending_trim.is_none());
            assert_eq!(v.worker_requests_pending, 0);
            assert!(v.pending_requests.is_empty());
            assert!(v.active_trim_drag.is_none() && v.active_midi_trim_drag.is_none());
            v.viewport = viewport;
            v.selected_track = selected_track;
            v.selected_clip = selected_clip;
            v.last_enqueued_edit = None;
        });
        visual.run_until_parked();
        let prefix = if midi { "midi-clip" } else { "clip" };
        let down = visual
            .debug_bounds(Box::leak(
                format!(
                    "{prefix}-trim-{}-{}",
                    if edge == TrimEdge::Left {
                        "left"
                    } else {
                        "right"
                    },
                    clip.value()
                )
                .into_boxed_str(),
            ))
            .unwrap()
            .center();
        let modifiers = Modifiers {
            shift: stretch,
            ..Modifiers::default()
        };
        let direction = if edge == TrimEdge::Left { 1.0 } else { -1.0 };
        visual.simulate_mouse_down(down, MouseButton::Left, modifiers);
        visual.simulate_mouse_move(
            point(down.x + px(12.0 * direction), down.y),
            MouseButton::Left,
            modifiers,
        );
        visual.run_until_parked();
        visual.simulate_mouse_move(
            point(down.x + px(40.0 * direction), down.y),
            MouseButton::Left,
            modifiers,
        );
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert!(if midi {
                v.active_midi_trim_drag
                    .as_ref()
                    .unwrap()
                    .preview
                    .borrow()
                    .is_some()
            } else {
                v.active_trim_drag
                    .as_ref()
                    .unwrap()
                    .preview
                    .borrow()
                    .is_some()
            });
        });
        let lane = visual
            .debug_bounds(Box::leak(
                format!("arrangement-lane-{}", other.value()).into_boxed_str(),
            ))
            .unwrap();
        let release = if destination >= 2 {
            let selector = match destination {
                2 => Box::leak(
                    format!("arrangement-track-header-{}", other.value()).into_boxed_str(),
                ),
                3 => "arrangement-loop-rail-row",
                4 => "editor-status-bar",
                5 => "browser-audition-waveform",
                _ => unreachable!("private fixture destination"),
            };
            let bounds = visual
                .debug_bounds(selector)
                .expect("actual rendered receiver");
            if matches!(destination, 3 | 4) {
                point(down.x + px(40.0 * direction), bounds.center().y)
            } else {
                bounds.center()
            }
        } else if outside {
            let source = editor.read_with(&visual, |v, _| {
                if midi {
                    v.active_midi_trim_drag.as_ref().unwrap().source_track
                } else {
                    v.active_trim_drag.as_ref().unwrap().source_track
                }
            });
            let _source_lane = visual
                .debug_bounds(Box::leak(
                    format!("arrangement-lane-{}", source.value()).into_boxed_str(),
                ))
                .unwrap();
            let body = editor.read_with(&visual, |v, _| {
                assert_eq!(v.arrangement_bounds_epoch, v.drag_layout_generation);
                v.arrangement_bounds.unwrap()
            });
            let position = point(down.x + px(40.0 * direction), px(body.top) - px(20.0));
            let browser = visual.debug_bounds("resize-browser-hit-region").unwrap();
            let bounds = gpui::Bounds::new(
                point(px(body.left), px(body.top)),
                gpui::size(px(body.width), px(body.height)),
            );
            assert!(!browser.contains(&position));
            assert!(!bounds.contains(&position));
            position
        } else {
            point(down.x + px(40.0 * direction), lane.center().y)
        };
        editor.read_with(&visual, |v, _| {
            let target = v
                .hovered_drag_target(f32::from(release.x), f32::from(release.y))
                .map(|(track, _)| track);
            if destination <= 1 {
                assert_eq!(target, if outside { None } else { Some(other) });
            }
        });
        eprintln!(
            "resize review release midi={midi} outside={outside} point={release:?} browser={:?}",
            visual.debug_bounds("resize-browser-hit-region")
        );
        visual.simulate_mouse_move(release, MouseButton::Left, modifiers);
        visual.run_until_parked();
        let (desired_audio, desired_midi, selection) = editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            (
                v.active_trim_drag.as_ref().and_then(|d| {
                    d.preview
                        .borrow()
                        .as_ref()
                        .map(|p| (p.clip.clone(), p.command_frame_for_test()))
                }),
                v.active_midi_trim_drag
                    .as_ref()
                    .and_then(|d| *d.preview.borrow()),
                (v.selected_track, v.selected_clip),
            )
        });
        assert!(desired_audio.is_some() || desired_midi.is_some());
        visual.simulate_mouse_up(release, MouseButton::Left, modifiers);
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
        assert!(v.error.is_none(), "real worker error: {:?}", v.error);
        assert_eq!((v.selected_track, v.selected_clip), selection);
        let p = &v.state.as_ref().unwrap().project;
        if let Some((expected, command_frame)) = &desired_audio {
            assert_eq!(p.clip(clip), Some(expected), "captured audio release midi={midi} outside={outside}");
            assert!(if stretch {
                matches!(v.last_enqueued_edit, Some(Command::StretchAudioClipEdge { clip: id, edge: actual, frame }) if id == clip && actual == edge && frame == *command_frame)
            } else {
                matches!(v.last_enqueued_edit, Some(Command::TrimClipEdge { clip: id, edge: actual, frame }) if id == clip && actual == edge && frame == *command_frame)
            });
        }
        if let Some(expected) = desired_midi {
            let actual = p.midi_clip(clip).unwrap();
            assert_eq!((actual.start(), actual.length(), actual.source_start()), (expected.start(), expected.length(), expected.source_start()), "captured MIDI release outside={outside}");
            assert!(matches!(v.last_enqueued_edit, Some(Command::TrimMidiClipEdge { clip: id, .. }) if id == clip));
        }
        assert_eq!(p.track(other), Some(&other_before));
        assert!(
            v.active_trim_drag.is_none()
                && v.active_midi_trim_drag.is_none()
                && v.pending_trim.is_none()
        );
        assert_eq!(v.worker_requests_pending, 0);
        assert!(v.pending_requests.is_empty());
    });
        assert!(!visual.update(|_, cx| cx.has_active_drag()));
        let committed =
            editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        visual.simulate_keystrokes("cmd-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
        });
        visual.simulate_keystrokes("cmd-shift-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), committed)
        });
        // Preserve every case's Undo->Redo proof, then actually Undo again
        // and wait for the worker before reusing this mode's fixture.
        visual.simulate_keystrokes("cmd-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            assert!(v.error.is_none() && v.pending_trim.is_none());
            assert_eq!(v.worker_requests_pending, 0);
            assert!(v.pending_requests.is_empty());
            assert!(v.active_trim_drag.is_none() && v.active_midi_trim_drag.is_none());
        });
        assert!(!visual.update(|_, cx| cx.has_active_drag()));
    }
}

#[gpui::test]
fn captured_release_over_other_audio_track_commits_without_retargeting(cx: &mut TestAppContext) {
    review_other_track_release(cx, false, false);
}
#[gpui::test]
fn captured_release_over_other_midi_track_commits_without_retargeting(cx: &mut TestAppContext) {
    review_other_track_release(cx, true, false);
}
#[gpui::test]
fn browser_release_review_midi_edge_admits_existing_detail_or_browser_focus(
    cx: &mut TestAppContext,
) {
    for browser_focus in [false, true] {
        let (_files, handle, editor, _) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.update(|window, _| window.activate_window());
        let clip = review_midi_clip(&editor, &mut visual);
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        visual.update(|window, cx| {
            editor.read_with(cx, |v, cx| {
                if browser_focus {
                    v.browser.search.read(cx).focus(window);
                } else {
                    v.midi_focus.focus(window);
                }
                assert!(!v.focus.is_focused(window));
            })
        });
        let down = visual
            .debug_bounds(Box::leak(
                format!("midi-clip-trim-right-{}", clip.value()).into_boxed_str(),
            ))
            .unwrap()
            .center();
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(down.x - px(12.0), down.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.run_until_parked();
        let target = point(down.x - px(40.0), down.y);
        visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let desired = editor.read_with(&visual, |v, _| {
            v.active_midi_trim_drag
                .as_ref()
                .unwrap()
                .preview
                .borrow()
                .unwrap()
        });
        visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            let c = v.state.as_ref().unwrap().project.midi_clip(clip).unwrap();
            assert_eq!(
                (c.start(), c.length(), c.source_start()),
                (desired.start(), desired.length(), desired.source_start()),
                "actual edge admission from browser_focus={browser_focus} must survive release"
            );
            assert!(v.error.is_none() && v.pending_trim.is_none());
        });
        visual.simulate_keystrokes("cmd-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
        });
    }
}

#[gpui::test]
fn captured_release_over_generic_outside_commits_audio_and_midi(cx: &mut TestAppContext) {
    for midi in [false, true] {
        review_other_track_release(cx, midi, true);
    }
}

#[gpui::test]
fn browser_release_review_original_labeled_outside_point_commits_audio_and_midi(
    cx: &mut TestAppContext,
) {
    for midi in [false, true] {
        let (_files, handle, editor, audio) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        let clip = if midi {
            review_midi_clip(&editor, &mut visual)
        } else {
            audio
        };
        visual.run_until_parked();
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        let prefix = if midi { "midi-clip" } else { "clip" };
        let down = visual
            .debug_bounds(Box::leak(
                format!("{prefix}-trim-left-{}", clip.value()).into_boxed_str(),
            ))
            .unwrap()
            .center();
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(down.x + px(12.0), down.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.run_until_parked();
        visual.simulate_mouse_move(
            point(down.x + px(40.0), down.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.run_until_parked();
        let (audio_preview, midi_preview) = editor.read_with(&visual, |v, _| {
            (
                v.active_trim_drag
                    .as_ref()
                    .and_then(|d| d.preview.borrow().as_ref().map(|p| p.clip.clone())),
                v.active_midi_trim_drag
                    .as_ref()
                    .and_then(|d| *d.preview.borrow()),
            )
        });
        assert!(if midi {
            midi_preview.is_some()
        } else {
            audio_preview.is_some()
        });
        let original_point = point(px(464.0), px(214.0));
        let browser = visual.debug_bounds("resize-browser-hit-region").unwrap();
        assert!(
            browser.contains(&original_point),
            "historical point must really belong to the browser"
        );
        editor.read_with(&visual, |v, _| {
            assert!(v.hovered_drag_target(464.0, 214.0).is_none())
        });
        visual.simulate_mouse_move(original_point, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        visual.simulate_mouse_up(original_point, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.worker_requests_pending, 1);
            assert!(v.pending_requests.is_empty());
            assert!(v.active_trim_drag.is_none() && v.active_midi_trim_drag.is_none());
        });
        pump(&editor, &mut visual);
        let after = editor.read_with(&visual, |v, _| {
            let p = &v.state.as_ref().unwrap().project;
            if let Some(expected) = &audio_preview { assert_eq!(p.clip(clip), Some(expected)); }
            if let Some(expected) = midi_preview {
                let c = p.midi_clip(clip).unwrap();
                assert_eq!((c.start(),c.length(),c.source_start()),(expected.start(),expected.length(),expected.source_start()));
            }
            assert!(v.pending_trim.is_none() && v.error.is_none());
            assert!(v.active_browser_audio_drag.is_none() && v.active_native_device_drag.is_none());
            eprintln!("original browser point={original_point:?} actual region={browser:?} midi={midi}: one accepted resize");
            p.snapshot()
        });
        assert_ne!(after, before);
        visual.simulate_keystrokes("cmd-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
        });
        visual.simulate_keystrokes("cmd-shift-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), after)
        });
    }
}

#[gpui::test]
fn release_audit_fast_audio_mouseup_uses_final_position_and_reverse(cx: &mut TestAppContext) {
    audit_fast_mouseup(cx, false, false, false);
}
#[gpui::test]
fn release_audit_fast_shift_mouseup_uses_final_position_and_reverse(cx: &mut TestAppContext) {
    audit_fast_mouseup(cx, true, false, false);
}
#[gpui::test]
fn release_audit_fast_midi_mouseup_uses_final_position_and_reverse(cx: &mut TestAppContext) {
    audit_fast_mouseup(cx, false, true, false);
}
#[gpui::test]
fn release_audit_fast_enabled_warp_trim_uses_final_position_and_reverse(cx: &mut TestAppContext) {
    audit_fast_mouseup(cx, false, false, true);
}
fn audit_fast_mouseup(cx: &mut TestAppContext, stretch: bool, midi: bool, enabled_warp: bool) {
    for edge in [TrimEdge::Left, TrimEdge::Right] {
        let (_files, handle, editor, audio) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let clip = if midi {
            review_midi_clip(&editor, &mut visual)
        } else {
            audio
        };
        if enabled_warp {
            let settings = editor.read_with(&visual, |v, _| {
                let p = &v.state.as_ref().unwrap().project;
                hex_project::default_audio_warp(
                    p.clip(clip).unwrap(),
                    p.tempo_map(),
                    hex_project::WarpMode::Beats {
                        preserve: hex_project::BeatsPreserve::Transients,
                        tail: hex_project::BeatsTail::Forward,
                        decay: 100,
                    },
                )
                .unwrap()
            });
            editor.update(&mut visual, |v, _| {
                assert!(v.enqueue(Request::Edit(Command::SetAudioClipWarp {
                    clip,
                    settings: Some(settings),
                })))
            });
            pump(&editor, &mut visual);
        }
        visual.run_until_parked();
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        let selector = format!(
            "{}clip-trim-{}-{}",
            if midi { "midi-" } else { "" },
            if edge == TrimEdge::Left {
                "left"
            } else {
                "right"
            },
            clip.value()
        );
        let down = visual
            .debug_bounds(Box::leak(selector.into_boxed_str()))
            .unwrap()
            .center();
        let modifiers = Modifiers {
            shift: stretch,
            ..Modifiers::default()
        };
        let direction = if edge == TrimEdge::Left { 1.0 } else { -1.0 };
        visual.simulate_mouse_down(down, MouseButton::Left, modifiers);
        visual.simulate_mouse_move(
            point(down.x + px(12.0 * direction), down.y),
            MouseButton::Left,
            modifiers,
        );
        visual.run_until_parked();
        let moved = point(down.x + px(55.0 * direction), down.y);
        visual.simulate_mouse_move(moved, MouseButton::Left, modifiers);
        visual.run_until_parked();
        // Reverse at release WITHOUT another mouse-move or modifiers event.
        // Projection oracle uses the existing model; event routing must deliver
        // this new coordinate instead of committing the cached move coordinate.
        let released = point(down.x + px(35.0 * direction), down.y);
        let (expected_audio, expected_midi) = editor.read_with(&visual, |v, _| {
            let p = &v.state.as_ref().unwrap().project;
            let viewport = v.visible_viewport(v.state.as_ref().unwrap().timeline_frames);
            if midi {
                let drag = v.active_midi_trim_drag.as_ref().expect("real MIDI capture");
                assert_eq!(drag.last_pointer_x.get(), Some(f32::from(moved.x)));
                let lane = v.drag_lanes[&drag.source_track];
                let beat =
                    super::snapped_midi_edge_beat(drag, p, f32::from(released.x), lane, viewport)
                        .unwrap();
                let expected = drag.clip.preview_edge_trim(drag.edge, beat).unwrap();
                assert_ne!(Some(expected.into()), *drag.preview.borrow());
                (None, Some(expected))
            } else {
                let drag = v.active_trim_drag.as_ref().expect("real audio capture");
                assert_eq!(drag.last_pointer_x.get(), Some(f32::from(moved.x)));
                let lane = v.drag_lanes[&drag.source_track];
                let frame =
                    super::snapped_edge_frame(drag, p, f32::from(released.x), lane, viewport)
                        .unwrap();
                let expected = super::preview_clip(drag, p, frame).unwrap();
                assert_ne!(expected, drag.preview.borrow().as_ref().unwrap().clip);
                (Some(expected), None)
            }
        });
        visual.simulate_mouse_up(released, MouseButton::Left, modifiers);
        visual.run_until_parked();
        pump(&editor, &mut visual);
        let after = editor.read_with(&visual, |v, _| {
            assert!(v.error.is_none(), "{:?}", v.error);
            assert!(v.active_trim_drag.is_none() && v.active_midi_trim_drag.is_none());
            assert!(v.pending_trim.is_none());
            let p = &v.state.as_ref().unwrap().project;
            if let Some(expected) = &expected_audio {
                assert_eq!(p.clip(clip), Some(expected));
            }
            if let Some(expected) = expected_midi {
                let actual = p.midi_clip(clip).unwrap();
                assert_eq!(actual.start(), expected.start());
                assert_eq!(actual.length(), expected.length());
                assert_eq!(actual.source_start(), expected.source_start());
            }
            p.snapshot()
        });
        assert_ne!(before, after);
        visual.simulate_keystrokes("cmd-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
        });
        visual.simulate_keystrokes("cmd-shift-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), after)
        });
    }
}

#[gpui::test]
fn captured_release_audio_over_header_ruler_status_and_browser(cx: &mut TestAppContext) {
    let cases = [TrimEdge::Left, TrimEdge::Right]
        .into_iter()
        .flat_map(|edge| (2..=5).map(move |destination| (edge, destination)));
    captured_release_cases(cx, false, false, false, cases);
}

#[gpui::test]
fn captured_release_midi_over_header_ruler_status_and_browser(cx: &mut TestAppContext) {
    let cases = [TrimEdge::Left, TrimEdge::Right]
        .into_iter()
        .flat_map(|edge| (2..=5).map(move |destination| (edge, destination)));
    captured_release_cases(cx, true, false, false, cases);
}

#[gpui::test]
fn captured_release_shift_over_other_track_and_foreign_panels(cx: &mut TestAppContext) {
    let cases = [TrimEdge::Left, TrimEdge::Right]
        .into_iter()
        .flat_map(|edge| (0..=5).map(move |destination| (edge, destination)));
    captured_release_cases(cx, false, true, false, cases);
}

#[gpui::test]
fn captured_release_enabled_warp_over_other_track_and_foreign_panels(cx: &mut TestAppContext) {
    let cases = [TrimEdge::Left, TrimEdge::Right]
        .into_iter()
        .flat_map(|edge| (0..=5).map(move |destination| (edge, destination)));
    captured_release_cases(cx, false, false, true, cases);
}

#[gpui::test]
fn resize_paint_normal_audio_source_track_selected(cx: &mut TestAppContext) {
    check_resize_lane_paint(cx, false, false, None, false);
}
#[gpui::test]
fn resize_paint_normal_audio_other_track_selected(cx: &mut TestAppContext) {
    check_resize_lane_paint(cx, false, false, None, true);
}
#[gpui::test]
fn resize_paint_shift_audio_source_track_selected(cx: &mut TestAppContext) {
    check_resize_lane_paint(cx, false, true, None, false);
}
#[gpui::test]
fn resize_paint_shift_audio_other_track_selected(cx: &mut TestAppContext) {
    check_resize_lane_paint(cx, false, true, None, true);
}
#[gpui::test]
fn resize_paint_midi_source_track_selected(cx: &mut TestAppContext) {
    check_resize_lane_paint(cx, true, false, None, false);
}
#[gpui::test]
fn resize_paint_midi_other_track_selected(cx: &mut TestAppContext) {
    check_resize_lane_paint(cx, true, false, None, true);
}
#[gpui::test]
fn resize_paint_cancel_escape_source_track_selected(cx: &mut TestAppContext) {
    check_resize_lane_paint(cx, false, false, Some(0), false);
}
#[gpui::test]
fn resize_paint_cancel_escape_other_track_selected(cx: &mut TestAppContext) {
    check_resize_lane_paint(cx, false, false, Some(0), true);
}
#[gpui::test]
fn resize_paint_cancel_stale_source_track_selected(cx: &mut TestAppContext) {
    check_resize_lane_paint(cx, false, true, Some(1), false);
}
#[gpui::test]
fn resize_paint_cancel_stale_other_track_selected(cx: &mut TestAppContext) {
    check_resize_lane_paint(cx, false, true, Some(1), true);
}
#[gpui::test]
fn resize_paint_cancel_foreign_focus_source_track_selected(cx: &mut TestAppContext) {
    check_resize_lane_paint(cx, true, false, Some(2), false);
}
#[gpui::test]
fn resize_paint_cancel_foreign_focus_other_track_selected(cx: &mut TestAppContext) {
    check_resize_lane_paint(cx, true, false, Some(2), true);
}
fn check_resize_lane_paint(
    cx: &mut TestAppContext,
    midi: bool,
    stretch: bool,
    cancel: Option<u8>,
    other_selected: bool,
) {
    for edge in [TrimEdge::Left, TrimEdge::Right] {
        // Lane paint, selection and receipt custody use identical seconds
        // and pixel geometry at 8kHz; keep real preparation off the long
        // 48kHz DSP workload covered by the other release tests.
        let (_files, handle, editor, audio) = fixture_at_rate(cx, 8_000);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.update(|window, _| window.activate_window());
        let clip = if midi {
            review_midi_clip(&editor, &mut visual)
        } else {
            audio
        };
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::AddTrack {
                name: "paint other track".into()
            })))
        });
        pump(&editor, &mut visual);
        visual.run_until_parked();
        let (track, before) = editor.read_with(&visual, |v, _| {
            let p = &v.state.as_ref().unwrap().project;
            let track = p
                .tracks()
                .iter()
                .find(|t| {
                    t.clips().iter().any(|c| c.id() == clip)
                        || t.midi_clips().iter().any(|c| c.id() == clip)
                })
                .unwrap()
                .id();
            (track, p.snapshot())
        });
        let clicked_track = editor.read_with(&visual, |v, _| {
            if other_selected {
                v.state
                    .as_ref()
                    .unwrap()
                    .project
                    .tracks()
                    .last()
                    .unwrap()
                    .id()
            } else {
                track
            }
        });
        let header = visual
            .debug_bounds(Box::leak(
                format!("track-name-{}", clicked_track.value()).into_boxed_str(),
            ))
            .unwrap()
            .center();
        visual.simulate_mouse_down(header, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(header, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let (selection, clip_selected) = editor.read_with(&visual, |v, _| {
            assert_eq!(v.selected_track, Some(clicked_track));
            (
                (v.selected_track, v.selected_clip),
                v.clip_is_selected(clip),
            )
        });
        editor.update(&mut visual, |v, _| v.last_enqueued_edit = None);
        let lane: &'static str =
            Box::leak(format!("arrangement-lane-{}", track.value()).into_boxed_str());
        let divider: &'static str =
            Box::leak(format!("lane-right-divider-{}", track.value()).into_boxed_str());
        let lane_paint = |visual: &mut VisualTestContext| {
            assert!(
                visual.rendered_border(lane).is_none(),
                "lane must not regain a layout-consuming or resize-feedback border"
            );
            let bounds = visual.debug_bounds(divider).expect("paint-only divider");
            let lane_bounds = visual.debug_bounds(lane).unwrap();
            assert_eq!(bounds.size.width, px(1.0));
            assert_eq!(bounds.right(), lane_bounds.right());
            assert_eq!(bounds.size.height, lane_bounds.size.height);
            (
                visual
                    .rendered_background(divider)
                    .expect("actual divider paint"),
                bounds,
            )
        };
        let baseline = lane_paint(&mut visual);
        let down = visual
            .debug_bounds(Box::leak(
                format!(
                    "{}clip-trim-{}-{}",
                    if midi { "midi-" } else { "" },
                    if edge == TrimEdge::Left {
                        "left"
                    } else {
                        "right"
                    },
                    clip.value()
                )
                .into_boxed_str(),
            ))
            .unwrap()
            .center();
        let modifiers = Modifiers {
            shift: stretch,
            ..Modifiers::default()
        };
        let direction = if edge == TrimEdge::Left { 1.0 } else { -1.0 };
        visual.simulate_mouse_down(down, MouseButton::Left, modifiers);
        visual.simulate_mouse_move(
            point(down.x + px(12.0 * direction), down.y),
            MouseButton::Left,
            modifiers,
        );
        visual.run_until_parked();
        let target = point(down.x + px(42.0 * direction), down.y);
        visual.simulate_mouse_move(target, MouseButton::Left, modifiers);
        visual.run_until_parked();
        let outline = Box::leak(
            format!("track-selection-outline-{}", clicked_track.value()).into_boxed_str(),
        );
        assert!(
            visual.debug_bounds(outline).is_none(),
            "held resizing keeps selection styling local to clips"
        );
        let painted = lane_paint(&mut visual);
        eprintln!(
            "RESIZE_LANE_SCENE midi={midi} shift={stretch} edge={edge:?} before={baseline:?} held={painted:?}"
        );
        assert_eq!(
            painted.0, baseline.0,
            "resize must not replace the divider color"
        );
        assert_eq!(
            painted.1, baseline.1,
            "resize must not change divider geometry"
        );
        let preview = if midi {
            "midi-trim-live-preview"
        } else {
            "trim-live-preview"
        };
        assert_eq!(
            visual
                .rendered_border(preview)
                .expect("actual clip-local feedback")
                .0,
            gpui::Hsla::from(gpui::rgb(if clip_selected {
                crate::theme::color(crate::theme::PaletteRole::Primary)
            } else {
                crate::theme::color(crate::theme::PaletteRole::ClipOutline)
            }))
        );
        // A captured source never adopts whole-lane drop styling while
        // crossing a foreign panel or the neighboring track.
        let other = editor.read_with(&visual, |v, _| {
            v.state
                .as_ref()
                .unwrap()
                .project
                .tracks()
                .last()
                .unwrap()
                .id()
        });
        let foreign_lane = visual
            .debug_bounds(Box::leak(
                format!("arrangement-lane-{}", other.value()).into_boxed_str(),
            ))
            .unwrap();
        let browser = visual
            .debug_bounds("browser-section-samples")
            .unwrap()
            .center();
        for outside in [browser, point(target.x, foreign_lane.center().y), target] {
            visual.simulate_mouse_move(outside, MouseButton::Left, modifiers);
            visual.run_until_parked();
            let source_lane = lane_paint(&mut visual);
            assert_eq!(source_lane.0, baseline.0);
            assert_eq!(source_lane.1, baseline.1);
            assert!(
                visual.rendered_border(preview).is_some(),
                "held clip-local feedback survives crossing"
            );
        }
        let desired = editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            assert_eq!((v.selected_track, v.selected_clip), selection);
            (
                v.active_trim_drag
                    .as_ref()
                    .and_then(|d| d.preview.borrow().as_ref().map(|p| p.clip.clone())),
                v.active_midi_trim_drag
                    .as_ref()
                    .and_then(|d| *d.preview.borrow()),
            )
        });
        if let Some(case) = cancel {
            match case {
                0 => visual.simulate_keystrokes("escape"),
                1 => editor.update(&mut visual, |v, cx| {
                    v.project_generation += 1;
                    cx.notify();
                }),
                _ => visual
                    .update(|window, cx| editor.read(cx).browser.search.read(cx).focus(window)),
            }
            visual.simulate_mouse_up(target, MouseButton::Left, modifiers);
            pump(&editor, &mut visual);
            editor.read_with(&visual, |v, _| {
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
                assert!(
                    v.active_trim_drag.is_none()
                        && v.active_midi_trim_drag.is_none()
                        && v.pending_trim.is_none()
                );
                assert_eq!(v.worker_requests_pending, 0);
                assert!(v.last_enqueued_edit.is_none());
                assert_eq!((v.selected_track, v.selected_clip), selection);
            });
            let terminal_lane = lane_paint(&mut visual);
            assert_eq!(terminal_lane.0, baseline.0);
            assert_eq!(terminal_lane.1, baseline.1);
            assert!(!visual.update(|_, cx| cx.has_active_drag()));
            // Pulse2871 retains the independent inherited preview-disappearance
            // failure and exact completed-scene probe; this case owns lane paint
            // and logical cancellation only, without claiming that bug fixed.
            continue;
        }
        visual.simulate_mouse_up(target, MouseButton::Left, modifiers);
        pump(&editor, &mut visual);
        let after = editor.read_with(&visual, |v, _| {
            assert!(v.error.is_none(), "{:?}", v.error);
            // Paint does not change selection while held. Completion follows
            // the existing pointer/canonical selection policy, not a guessed
            // Created::Clip result (trim commands return Created::None).
            assert!(
                v.active_trim_drag.is_none()
                    && v.active_midi_trim_drag.is_none()
                    && v.pending_trim.is_none()
            );
            let p = &v.state.as_ref().unwrap().project;
            if let Some(expected) = &desired.0 {
                assert_eq!(p.clip(clip), Some(expected));
            }
            if let Some(expected) = desired.1 {
                let c = p.midi_clip(clip).unwrap();
                assert_eq!(
                    (c.start(), c.length(), c.source_start()),
                    (expected.start(), expected.length(), expected.source_start())
                );
            }
            p.snapshot()
        });
        assert_eq!(lane_paint(&mut visual), baseline);
        assert!(visual.debug_bounds(preview).is_none());
        visual.simulate_keystrokes("cmd-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
        });
        visual.simulate_keystrokes("cmd-shift-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), after)
        });
    }
}

#[gpui::test]
fn focus_cancel_removes_midi_resize_completed_paint_without_another_input(cx: &mut TestAppContext) {
    check_focus_cancel_paint(cx, true, false, false);
}

#[gpui::test]
fn focus_cancel_removes_audio_normal_shift_warp_and_midi_shift_paint(cx: &mut TestAppContext) {
    for (midi, stretch, warp) in [
        (false, false, false),
        (false, true, false),
        (false, false, true),
        (false, true, true),
        (true, true, false),
    ] {
        check_focus_cancel_paint(cx, midi, stretch, warp);
    }
}

fn check_focus_cancel_paint(cx: &mut TestAppContext, midi: bool, stretch: bool, warp: bool) {
    let (_files, handle, editor, audio) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    let clip = if midi {
        review_midi_clip(&editor, &mut visual)
    } else {
        audio
    };
    if warp {
        let settings = editor.read_with(&visual, |v, _| {
            let p = &v.state.as_ref().unwrap().project;
            hex_project::default_audio_warp(
                p.clip(clip).unwrap(),
                p.tempo_map(),
                hex_project::WarpMode::Beats {
                    preserve: hex_project::BeatsPreserve::Transients,
                    tail: hex_project::BeatsTail::Forward,
                    decay: 100,
                },
            )
            .unwrap()
        });
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::SetAudioClipWarp {
                clip,
                settings: Some(settings)
            })))
        });
        pump(&editor, &mut visual);
    }
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddTrack {
            name: "focus probe other".into()
        })))
    });
    pump(&editor, &mut visual);
    let track = editor.read_with(&visual, |v, _| {
        v.state
            .as_ref()
            .unwrap()
            .project
            .tracks()
            .iter()
            .find(|t| {
                t.midi_clips().iter().any(|c| c.id() == clip)
                    || t.clips().iter().any(|c| c.id() == clip)
            })
            .unwrap()
            .id()
    });
    let header = visual
        .debug_bounds(Box::leak(
            format!("track-name-{}", track.value()).into_boxed_str(),
        ))
        .unwrap()
        .center();
    visual.simulate_mouse_down(header, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(header, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.update(&mut visual, |v, _| v.last_enqueued_edit = None);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let down = visual
        .debug_bounds(Box::leak(
            format!(
                "{}clip-trim-left-{}",
                if midi { "midi-" } else { "" },
                clip.value()
            )
            .into_boxed_str(),
        ))
        .unwrap()
        .center();
    let modifiers = Modifiers {
        shift: stretch,
        ..Default::default()
    };
    visual.simulate_mouse_down(down, MouseButton::Left, modifiers);
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        modifiers,
    );
    visual.run_until_parked();
    let target = point(down.x + px(42.0), down.y);
    visual.simulate_mouse_move(target, MouseButton::Left, modifiers);
    visual.run_until_parked();
    let preview = if midi {
        "midi-trim-live-preview"
    } else {
        "trim-live-preview"
    };
    let observe = |stage: &str, visual: &mut VisualTestContext| {
        let state = visual.update(|window, cx| {
            editor.read_with(cx, |v, _| {
                (
                    v.active_midi_trim_drag.is_some() || v.active_trim_drag.is_some(),
                    v.pending_trim.is_some(),
                    v.focus.is_focused(window),
                    window.is_window_active(),
                    cx.has_active_drag(),
                )
            })
        });
        let border = visual.rendered_border(preview);
        let fill = visual.rendered_background(preview);
        eprintln!(
            "FOCUS_CANCEL_PHASE {stage} logical_pending_rootfocus_windowactive_gpuidrag={state:?} bounds={:?} border={border:?} fill={fill:?}",
            visual.debug_bounds(preview)
        );
        (state, border, fill)
    };
    let held = observe("held", &mut visual);
    assert!(held.0.0 && held.0.2 && held.0.3 && held.0.4);
    assert!(held.1.is_some() && held.2.is_some());
    visual.update(|window, cx| editor.read(cx).browser.search.read(cx).focus(window));
    visual.run_until_parked();
    let after_focus = observe("after-mounted-search-focus-before-release", &mut visual);
    assert!(
        !after_focus.0.0
            && !after_focus.0.1
            && !after_focus.0.2
            && after_focus.0.3
            && !after_focus.0.4
    );
    assert!(
        after_focus.1.is_none() && after_focus.2.is_none(),
        "focus terminal repaints before any release input"
    );
    visual.simulate_mouse_up(target, MouseButton::Left, modifiers);
    pump(&editor, &mut visual);
    let after_release = observe("after-actual-release", &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        assert!(
            v.active_trim_drag.is_none()
                && v.active_midi_trim_drag.is_none()
                && v.pending_trim.is_none()
        );
        assert_eq!(v.worker_requests_pending, 0);
        assert!(v.pending_requests.is_empty() && v.last_enqueued_edit.is_none());
    });
    assert!(!after_release.0.4);
    assert!(
        after_release.1.is_none() && after_release.2.is_none(),
        "actual completed preview paint must retire after focus cancellation, without another input or forced refresh"
    );
}
