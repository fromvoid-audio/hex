//! Observe actual waveform paint callbacks/masks, never a mock header property.
use crate::editor::{EditorView, external_audio_drop_ui_test::BatchFiles, musical_time::SnapGrid};
use gpui::{AppContext, Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext};
use hex_project::{ClipId, Command, TrackId};
use hex_session::service::{Event, Request, Service};
use std::time::{Duration, Instant};

fn next(service: &Service) -> Event {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(e) = service.try_event() {
            assert!(e.error.is_none(), "{:?}", e.error);
            return e;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn fixture(
    cx: &mut TestAppContext,
    looping: bool,
) -> (
    BatchFiles,
    gpui::AnyWindowHandle,
    Entity<EditorView>,
    TrackId,
    ClipId,
) {
    cx.update(crate::editor::bind_editor_keys);
    let files = BatchFiles::new();
    let path = files.wav("body.wav", 48_000, 96_000);
    let folder = path.parent().unwrap();
    std::fs::create_dir(folder.join("samples")).unwrap();
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
    next(&service);
    service.submit(Request::Import(path.clone())).unwrap();
    let e = next(&service);
    let track = e.state.project.tracks()[0].id();
    let clip = e.state.project.tracks()[0].clips()[0].id();
    service
        .submit(Request::Edit(Command::MoveClip {
            clip,
            track,
            start: 96_000,
        }))
        .unwrap();
    let mut e = next(&service);
    if looping {
        service
            .submit(Request::Edit(Command::SetClipLooping {
                clip,
                looping: true,
            }))
            .unwrap();
        e = next(&service);
    }
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_browser_directory(window, cx, service, folder)
            });
            editor.update(cx, |v, cx| {
                assert_eq!(v.browser.directory, folder.join("samples"));
                assert_eq!(v.browser.places, vec![folder.join("samples")]);
                v.state = Some(e.state);
                v.snap_grid = SnapGrid::default().with_platform_modifier(true);
                v.focus.focus(window);
                cx.notify();
            });
            editor
        })
        .unwrap();
    (files, handle, editor, track, clip)
}
fn assert_body(visual: &mut VisualTestContext, editor: &Entity<EditorView>, clip: ClipId) {
    let selector = Box::leak(format!("clip-name-{}", clip.value()).into_boxed_str());
    let header = visual
        .debug_bounds(selector)
        .expect("actual retained clip title");
    editor.update(&mut *visual, |v, cx| {
        v.clip_content_cache = Default::default();
        v.waveform_geometry_cache.borrow_mut().clear();
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(&*visual, |v, _| {
        let cache = v.waveform_geometry_cache.borrow();
        let paints = cache.painted_waveforms();
        assert_eq!(
            paints.len(),
            1,
            "actual Arrangement waveform painter must run"
        );
        let p = paints[0];
        eprintln!(
            "waveform header bottom={:?}, actual paint={:?}, mask={:?}, paths={:?}",
            header.bottom(),
            p.bounds,
            p.mask,
            p.ordinates
        );
        assert!(
            p.bounds.top() >= header.bottom(),
            "waveform geometry must begin below actual title"
        );
        assert!(
            p.mask.top() >= header.bottom(),
            "actual fill/stroke content mask must exclude title"
        );
        for (lo, hi) in p.ordinates {
            assert!(lo >= f32::from(header.bottom()) && hi >= lo);
        }
    });
}
#[gpui::test]
fn waveform_body_painter_stays_below_ordinary_selected_and_looped_headers(cx: &mut TestAppContext) {
    for looping in [false, true] {
        let (_files, handle, editor, _track, clip) = fixture(cx, looping);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        assert_body(&mut visual, &editor, clip);
        let header = visual
            .debug_bounds(Box::leak(
                format!("clip-name-{}", clip.value()).into_boxed_str(),
            ))
            .unwrap();
        visual.simulate_mouse_down(header.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(header.center(), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert_body(&mut visual, &editor, clip);
    }
}
