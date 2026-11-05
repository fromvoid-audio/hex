//! Actual existing MIDI editor actions after a source-bearing sampler is CURRENT.
use super::*;
use gpui::{Modifiers, VisualTestContext};

pub(super) fn exercise(
    cx: &gpui::TestAppContext,
    visual: &mut VisualTestContext,
    h: &mut Harness,
    window: gpui::AnyWindowHandle,
    track: hex_project::TrackId,
    saved: &std::path::Path,
) {
    visual.simulate_resize(gpui::size(gpui::px(1600.0), gpui::px(900.0)));
    visual.run_until_parked();
    let old = h
        .backend
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .rendered_current(h)
        .unwrap();
    let selector = Box::leak(format!("track-name-{}", track.value()).into_boxed_str());
    let point = visual.debug_bounds(selector).unwrap().center();
    visual.simulate_click(point, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_keystrokes("cmd-shift-m");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        cx.background_executor
            .advance_clock(Duration::from_millis(16));
        visual.run_until_parked();
        {
            let mut slot = h.backend.lock().unwrap();
            if let Some(backend) = slot.as_mut() {
                if backend.shutdown_requested() {
                    slot.take();
                } else {
                    super::envelope::render(backend, &mut [[0.0; 2]; 64]);
                }
            }
        }
        let applied = h
            .backend
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|b| b.status())
            .and_then(|s| s.rendered_current(h));
        if applied.is_some_and(|a| {
            a.context.project_generation == old.context.project_generation
                && a.context.editor_revision == old.context.editor_revision + 1
                && a.output_identity == old.output_identity
                && a.audio_revision > old.audio_revision
        }) && ready(cx, window)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "loaded sampler MIDI create did not reach fresh CURRENT: {:?}",
            cx.update(|cx| hex_ui::sampler_keyboard_test_status(window, cx))
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    for _ in 0..8 {
        cx.background_executor
            .advance_clock(Duration::from_millis(16));
        visual.run_until_parked();
    }
    let tab = visual.debug_bounds("clip-detail-tab").unwrap().center();
    visual.simulate_click(tab, Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("midi-note-roll").is_some(),
        "created clip must open existing piano roll"
    );
    // Real draw stroke commits EditMidiNotes on mouse-up through the same route.
    let bounds = visual.debug_bounds("midi-note-roll").unwrap();
    let down = gpui::point(
        bounds.origin.x + gpui::px(10.0),
        bounds.origin.y + bounds.size.height * (11.5 / 24.0),
    );
    transact(cx, visual, h, window, "draw", |visual| {
        visual.simulate_click(bounds.center(), Modifiers::default());
        visual.simulate_keystrokes("b");
        cx.background_executor
            .advance_clock(Duration::from_millis(500));
        visual.run_until_parked();
        visual.simulate_mouse_down(down, gpui::MouseButton::Left, Modifiers::default());

        visual.simulate_mouse_up(down, gpui::MouseButton::Left, Modifiers::default());
    });
    // Literal A880 / authored A440, original8k/output16k, velocity100.
    pcm(
        cx,
        visual,
        h,
        &[
            [0.098425195, -0.19685039],
            [0.39370078, 0.0],
            [-0.098425195, 0.19685039],
            [0.0, 0.0],
        ],
    );
    transact(cx, visual, h, window, "change velocity", |v| {
        click(v, "selected note velocity up 8")
    });
    // The existing delete control and global Undo/Redo dispatch real commands.
    transact(cx, visual, h, window, "delete arrange note", |v| {
        click(v, "delete selected note · backspace")
    });
    transact(cx, visual, h, window, "history/delete", |v| {
        v.simulate_keystrokes("cmd-z")
    });
    transact(cx, visual, h, window, "history/delete", |v| {
        v.simulate_keystrokes("cmd-shift-z")
    });
    transact(cx, visual, h, window, "history/delete", |v| {
        v.simulate_keystrokes("cmd-z")
    });
    click(visual, "show console view · tab");
    cx.background_executor
        .advance_clock(Duration::from_millis(16));
    visual.run_until_parked();
    transact(cx, visual, h, window, "append scene", |v| {
        click(v, "add scene")
    });
    visual.simulate_keystrokes("cmd-s");
    let end = Instant::now() + Duration::from_secs(5);
    let scene = loop {
        cx.background_executor
            .advance_clock(Duration::from_millis(16));
        visual.run_until_parked();
        if let Ok(p) = hex_storage::load(saved)
            && let Some(scene) = p.scenes().first()
            && ready(cx, window)
        {
            break scene.id();
        }
        assert!(Instant::now() < end, "save new scene");
        std::thread::sleep(Duration::from_millis(1));
    };
    let selector =
        Box::leak(format!("console-cell-{}-{}", track.value(), scene.value()).into_boxed_str());
    click(visual, selector);
    cx.background_executor
        .advance_clock(Duration::from_millis(16));
    visual.run_until_parked();
    transact(cx, visual, h, window, "create console midi", |v| {
        click(v, "create midi cell")
    });
    click(visual, "clip-detail-tab");
    let bounds = visual.debug_bounds("midi-note-roll").unwrap();
    let down = gpui::point(
        bounds.origin.x + gpui::px(10.0),
        bounds.origin.y + bounds.size.height * (11.5 / 24.0),
    );
    transact(cx, visual, h, window, "draw console note", |v| {
        cx.background_executor
            .advance_clock(Duration::from_millis(500));
        v.run_until_parked();
        v.simulate_mouse_down(down, gpui::MouseButton::Left, Modifiers::default());
        v.simulate_mouse_up(down, gpui::MouseButton::Left, Modifiers::default());
    });
    transact(cx, visual, h, window, "change console velocity", |v| {
        click(v, "selected note velocity up 8")
    });
    transact(cx, visual, h, window, "delete console note", |v| {
        click(v, "delete selected note · backspace")
    });
}

fn transact(
    cx: &gpui::TestAppContext,
    visual: &mut VisualTestContext,
    h: &mut Harness,
    window: gpui::AnyWindowHandle,
    label: &str,
    action: impl FnOnce(&mut VisualTestContext),
) {
    let old = h
        .backend
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .rendered_current(h)
        .unwrap();
    action(visual);
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        cx.background_executor
            .advance_clock(Duration::from_millis(16));
        visual.run_until_parked();
        {
            let mut slot = h.backend.lock().unwrap();
            if let Some(backend) = slot.as_mut() {
                if backend.shutdown_requested() {
                    slot.take();
                } else {
                    super::envelope::render(backend, &mut [[0.0; 2]; 64]);
                }
            }
        }
        let applied = h
            .backend
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|b| b.status())
            .and_then(|s| s.rendered_current(h));
        if applied.is_some_and(|a| {
            a.context.project_generation == old.context.project_generation
                && a.context.editor_revision == old.context.editor_revision + 1
                && a.output_identity == old.output_identity
                && a.audio_revision > old.audio_revision
        }) && ready(cx, window)
        {
            break;
        }
        assert!(
            Instant::now() < end,
            "{label} did not reach CURRENT (old revision {}): {:?}",
            old.context.editor_revision,
            cx.update(|cx| hex_ui::sampler_keyboard_test_status(window, cx))
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    for _ in 0..8 {
        cx.background_executor
            .advance_clock(Duration::from_millis(16));
        visual.run_until_parked();
    }
}
fn click(visual: &mut VisualTestContext, selector: &'static str) {
    let point = visual
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector}"))
        .center();
    visual.simulate_click(point, Modifiers::default());
    visual.run_until_parked();
}

fn pcm(
    cx: &gpui::TestAppContext,
    visual: &mut VisualTestContext,
    h: &mut Harness,
    expected: &[[f32; 2]],
) {
    click(visual, "stop · escape");
    cx.background_executor
        .advance_clock(Duration::from_millis(16));
    visual.run_until_parked();
    {
        let mut slot = h.backend.lock().unwrap();
        super::envelope::render(slot.as_mut().unwrap(), &mut [[0.0; 2]; 64]);
    }
    cx.background_executor
        .advance_clock(Duration::from_millis(16));
    visual.run_until_parked();
    click(visual, "play or pause · space");
    cx.background_executor
        .advance_clock(Duration::from_millis(16));
    visual.run_until_parked();
    let mut output = [[0.0; 2]; 64];
    {
        let mut slot = h.backend.lock().unwrap();
        super::envelope::render(slot.as_mut().unwrap(), &mut output);
    }
    for (actual, expected) in output.iter().zip(expected) {
        for c in 0..2 {
            assert!(
                (actual[c] - expected[c]).abs() < 1e-7,
                "pcm {:?} expected {expected:?}",
                &output[..4]
            );
        }
    }
    click(visual, "stop · escape");
    cx.background_executor
        .advance_clock(Duration::from_millis(16));
    visual.run_until_parked();
    {
        let mut slot = h.backend.lock().unwrap();
        super::envelope::render(slot.as_mut().unwrap(), &mut output);
    }
    assert_eq!(output, [[0.0; 2]; 64]);
}

fn ready(cx: &gpui::TestAppContext, window: gpui::AnyWindowHandle) -> bool {
    cx.update(|cx| hex_ui::sampler_midi_test_ready(window, cx)) == Ok(true)
}
