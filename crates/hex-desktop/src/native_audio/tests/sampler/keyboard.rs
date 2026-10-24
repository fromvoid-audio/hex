//! Actual rendered keyboard lifecycle, isolated from source/save fixture setup.
//! Each asynchronous operation owns the unchanged Harness wait from its trigger;
//! setup completion is diagnostic context, never a deadline for later input.
use super::*;
pub(super) fn exercise_blocked_sampler_keyboard(
    cx: &gpui::TestAppContext,
    visual: &mut gpui::VisualTestContext,
    h: &mut Harness,
    window: gpui::AnyWindowHandle,
    setup_finished: Instant,
    stereo: bool,
) {
    let key = |visual: &mut gpui::VisualTestContext, name: &str, down: bool| {
        let keystroke = gpui::Keystroke::parse(name).unwrap();
        if down {
            visual.simulate_event(gpui::KeyDownEvent {
                keystroke,
                is_held: false,
            });
        } else {
            visual.simulate_event(gpui::KeyUpEvent { keystroke });
        }
        visual.run_until_parked();
    };
    let render = |h: &mut Harness, stopped: bool| {
        let mut output = [[0.0; 2]; 64];
        let mut slot = h.backend.lock().unwrap();
        let backend = slot.as_mut().unwrap();
        let measured = allocation_counter::measure(|| backend.render(&mut output).unwrap());
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        let status = backend.status().unwrap();
        if stopped {
            assert_eq!(
                status.position.load(std::sync::atomic::Ordering::Acquire),
                0
            );
            assert_eq!(
                status
                    .performance_frame
                    .load(std::sync::atomic::Ordering::Acquire),
                0
            );
        }
        output
    };
    // Capture the actual first live block and preserve its phase. One block is
    // a multiple of both the8-frame root and4-frame octave loop periods.
    let first = render(h, true);
    assert!(
        first.iter().any(|f| *f != [0.0; 2]),
        "{:?}",
        cx.update(|cx| hex_ui::sampler_keyboard_test_status(window, cx))
    );
    key(visual, "k", true);
    let both = render(h, true);
    let (root, octave) = if stereo {
        (
            [
                [0.125, -0.25],
                [0.3125, -0.125],
                [0.5, 0.0],
                [0.1875, 0.125],
                [-0.125, 0.25],
                [-0.0625, 0.125],
                [0.0, 0.0],
                [0.0625, -0.125],
            ],
            [[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25], [0.0, 0.0]],
        )
    } else {
        (
            [
                [0.125, 0.125],
                [0.5, 0.5],
                [-0.125, -0.125],
                [0.0, 0.0],
                [0.125, 0.125],
                [0.5, 0.5],
                [-0.125, -0.125],
                [0.0, 0.0],
            ],
            [
                [0.125, 0.125],
                [-0.125, -0.125],
                [0.125, 0.125],
                [-0.125, -0.125],
            ],
        )
    };
    for i in 0..64 {
        for c in 0..2 {
            assert!(
                (both[i][c] - root[i % 8][c] - octave[i % 4][c]).abs() < 1e-6,
                "both {i}: {:?}",
                both[i]
            );
        }
    }
    let (entered_tx, entered) = mpsc::sync_channel(1);
    let (release, wait) = mpsc::sync_channel(1);
    h.observation.lock().unwrap().pause_poll = Some((entered_tx, wait));
    entered.recv_timeout(Duration::from_secs(2)).unwrap();
    key(visual, "a", false);
    let released = render(h, true);
    for i in 0..64 {
        for c in 0..2 {
            let tail = if i == 0 {
                root[0][c]
            } else if i == 1 {
                root[1][c] * 0.5
            } else {
                0.0
            };
            assert!(
                (released[i][c] - octave[i % 4][c] - tail).abs() < 1e-6,
                "release {i}: {:?}",
                released[i]
            );
        }
    }
    key(visual, "a", true);
    assert_eq!(
        render(h, true),
        both,
        "a fresh physical press retriggers alongside the held octave while Service is blocked"
    );
    visual.update(|w, _| w.blur());
    cx.background_executor
        .advance_clock(Duration::from_millis(16));
    visual.run_until_parked();
    assert!(
        render(h, true).iter().all(|f| *f == [0.0; 2]),
        "focus cancellation must bypass the blocked worker: {:?}",
        cx.update(|cx| hex_ui::sampler_keyboard_test_status(window, cx))
    );
    release.send(()).unwrap();
    // Force the more precise race: worker passed UI cancellation checks but has
    // not yet published activation. The UI already knows this reserved epoch.
    let (entered_tx, entered) = mpsc::sync_channel(1);
    let (release, wait) = mpsc::sync_channel(1);
    h.observation.lock().unwrap().pause_resident_publication = Some((entered_tx, wait));
    let end = Instant::now() + h.wait;
    let toggle = visual.debug_bounds("computer midi keyboard").unwrap();
    visual.simulate_click(toggle.center(), gpui::Modifiers::default());
    // Focus-loss leaves the toggle enabled; first turn it off, then on.
    visual.simulate_click(toggle.center(), gpui::Modifiers::default());
    loop {
        cx.background_executor
            .advance_clock(Duration::from_millis(16));
        visual.run_until_parked();
        if entered.try_recv().is_ok() {
            break;
        }
        assert!(
            Instant::now() < end,
            "late activation race did not reach actual worker gate"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    key(visual, "a", true);
    visual.simulate_click(toggle.center(), gpui::Modifiers::default());
    assert!(render(h, true).iter().all(|f| *f == [0.0; 2]));
    release.send(()).unwrap();
    for _ in 0..16 {
        cx.background_executor
            .advance_clock(Duration::from_millis(16));
        visual.run_until_parked();
        assert!(
            render(h, true).iter().all(|f| *f == [0.0; 2]),
            "a late cancelled activation resurrected keys"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    // Playing uses the same route and sums keyboard+authored voices before
    // host output trim/effects/mix, rather than replacing the active graph.
    key(visual, "a", false);
    key(visual, "k", false);
    let end = Instant::now() + h.wait;
    visual.simulate_keystrokes("space");
    loop {
        cx.background_executor
            .advance_clock(Duration::from_millis(16));
        visual.run_until_parked();
        let output = render(h, false);
        if h.backend
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .playing
            .load(std::sync::atomic::Ordering::Acquire)
        {
            for i in 0..64 {
                assert_eq!(output[i], root[i % 8]);
            }
            break;
        }
        assert!(Instant::now() < end, "actual Arrange Play did not start");
    }
    visual.simulate_click(toggle.center(), gpui::Modifiers::default());
    for _ in 0..16 {
        cx.background_executor
            .advance_clock(Duration::from_millis(16));
        visual.run_until_parked();
        render(h, false);
        std::thread::sleep(Duration::from_millis(1));
    }
    key(visual, "a", true);
    let combined = render(h, false);
    for i in 0..64 {
        for c in 0..2 {
            assert!(
                (combined[i][c] - 2.0 * root[i % 8][c]).abs() < 1e-6,
                "playing sum {i}: {:?}",
                combined[i]
            );
        }
    }
    visual.simulate_keystrokes("escape");
    assert!(
        render(h, true).iter().all(|f| *f == [0.0; 2]),
        "Stop must end authored and keyboard voices"
    );
    cx.background_executor
        .advance_clock(Duration::from_millis(16));
    visual.run_until_parked();
    visual.simulate_event(gpui::KeyDownEvent {
        keystroke: gpui::Keystroke::parse("a").unwrap(),
        is_held: true,
    });
    assert!(
        render(h, true).iter().all(|f| *f == [0.0; 2]),
        "old held autorepeat restarted a stopped voice"
    );
    // Start a genuine new ordinary activation while leaving its graph queued. Stop must
    // revoke its old physical epoch without discarding the already admitted input root.
    let end = Instant::now() + h.wait;
    visual.simulate_click(toggle.center(), gpui::Modifiers::default());
    visual.simulate_click(toggle.center(), gpui::Modifiers::default());
    key(visual, "a", false);
    key(visual, "a", true);
    loop {
        cx.background_executor
            .advance_clock(Duration::from_millis(16));
        visual.run_until_parked();
        let applied = h
            .backend
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .rendered_current(h)
            .unwrap();
        let submitted = h
            .observation
            .lock()
            .unwrap()
            .resident_submitted
            .last()
            .copied()
            .unwrap();
        if submitted.output_identity == applied.output_identity
            && submitted.context == applied.context
            && submitted.audio_revision > applied.audio_revision
        {
            break;
        }
        assert!(
            Instant::now() < end,
            "actual ordinary keyboard activation was not queued"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    visual.simulate_keystrokes("escape");
    // The previous activation graph is still queued: held autorepeat after this Stop must
    // leave its revoked root available for the next fresh physical epoch, without a callback.
    visual.simulate_event(gpui::KeyDownEvent {
        keystroke: gpui::Keystroke::parse("a").unwrap(),
        is_held: true,
    });
    visual.run_until_parked();
    key(visual, "a", false);
    key(visual, "a", true);
    for _ in 0..16 {
        cx.background_executor
            .advance_clock(Duration::from_millis(16));
        visual.run_until_parked();
        std::thread::sleep(Duration::from_millis(1));
    }
    let fresh_after_stop = render(h, true);
    for (i, actual) in fresh_after_stop.iter().enumerate() {
        assert_eq!(
            *actual,
            root[i % 8],
            "fresh post-Stop activation was revoked: {:?}",
            cx.update(|cx| hex_ui::sampler_keyboard_test_status(window, cx))
        );
    }
    visual.simulate_keystrokes("escape");
    assert!(render(h, true).iter().all(|f| *f == [0.0; 2]));
    cx.background_executor
        .advance_clock(Duration::from_millis(16));
    visual.run_until_parked();
    let capacity_started = Instant::now();
    let end = capacity_started + h.wait;
    // Seventeen simultaneously held keys exceed the authored16-voice pool.
    // Refusal is visible without disturbing the CURRENT source or allocating.
    for name in [
        "a", "w", "s", "e", "d", "f", "t", "g", "y", "h", "u", "j", "k", "o", "l", "p", ";",
    ] {
        key(visual, name, false);
    }
    for name in [
        "a", "w", "s", "e", "d", "f", "t", "g", "y", "h", "u", "j", "k", "o", "l", "p", ";",
    ] {
        key(visual, name, true);
    }
    let mut polls = 0;
    loop {
        polls += 1;
        cx.background_executor
            .advance_clock(Duration::from_millis(16));
        visual.run_until_parked();
        let output = render(h, true);
        let state = cx
            .update(|cx| hex_ui::sampler_keyboard_test_status(window, cx))
            .unwrap();
        let rejected = state.contains("computer keyboard note refused");
        if rejected {
            assert!(output.iter().all(|f| *f == [0.0; 2]));
            break;
        }
        assert!(
            Instant::now() < end,
            "capacity refusal was not reported by actual callback: polls={polls} capacity_elapsed={:?} since_setup_handoff={:?} budget={:?} current={:?} state={state}",
            capacity_started.elapsed(),
            setup_finished.elapsed(),
            h.wait,
            h.backend.lock().ok().and_then(|slot| slot
                .as_ref()
                .and_then(|backend| backend.status())
                .and_then(|status| status.rendered_current(h))
                .map(|s| (s.output_identity, s.context, s.audio_revision)))
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    cx.background_executor
        .advance_clock(Duration::from_millis(16));
    visual.run_until_parked();
    assert!(
        cx.update(|cx| hex_ui::sampler_keyboard_test_status(window, cx))
            .unwrap()
            .contains("computer keyboard note refused")
    );
}
