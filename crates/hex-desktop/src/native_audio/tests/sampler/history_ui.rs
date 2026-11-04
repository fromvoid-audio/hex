//! Retained numeric fields and global keyboard history through the real worker.
use super::envelope::render;
use super::*;
use gpui::{Modifiers, MouseButton, VisualTestContext};
use hex_engine::SamplerParameter::{self, AttackMs, Gain, Transpose};
pub(super) fn exercise(
    cx: &gpui::TestAppContext,
    visual: &mut VisualTestContext,
    h: &mut Harness,
    window: gpui::AnyWindowHandle,
    track: hex_project::TrackId,
    saved: &std::path::Path,
) {
    let started = Instant::now();
    let end = started + Duration::from_secs(30);
    let initial = hex_storage::load(saved).unwrap();
    let device = initial.track(track).unwrap().instrument().unwrap().id();
    let other = initial
        .tracks()
        .iter()
        .find(|t| t.id() != track)
        .unwrap()
        .id();
    let selector = |p: SamplerParameter| -> &'static str {
        Box::leak(format!("sampler-{}-{}-value", p.key(), device.value()).into_boxed_str())
    };
    let target_selector = Box::leak(format!("track-name-{}", track.value()).into_boxed_str());
    let other_device = initial.track(other).unwrap().devices().first().unwrap();
    assert_eq!(other_device.state().type_id().as_str(), Ok("hex.utility"));
    let other_device_selector =
        Box::leak(format!("effect-device-{}", other_device.id().value()).into_boxed_str());
    let other_selector = Box::leak(format!("track-name-{}", other.value()).into_boxed_str());
    let other_selected =
        Box::leak(format!("selected-track-lane-{}", other.value()).into_boxed_str());
    let pulse = |visual: &mut VisualTestContext| {
        cx.background_executor
            .advance_clock(Duration::from_millis(16));
        visual.run_until_parked();
        let mut slot = h.backend.lock().unwrap();
        if let Some(backend) = slot.as_mut() {
            if backend.shutdown_requested() {
                slot.take();
            } else {
                render(backend, &mut [[0.0; 2]; 64]);
            }
        }
        drop(slot);
        std::thread::sleep(Duration::from_millis(1));
    };
    let applied = || {
        h.backend
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|b| b.status())
            .and_then(|s| s.rendered_current(h))
    };
    let wait_next = |visual: &mut VisualTestContext, old: RenderedCurrent| {
        loop {
            pulse(visual);
            if applied().is_some_and(|a| {
                a.context.project_generation == old.context.project_generation
                    && a.context.editor_revision == old.context.editor_revision + 1
                    && a.output_identity == old.output_identity
                    && a.audio_revision > old.audio_revision
            }) {
                break;
            }
            assert!(
                Instant::now() < end,
                "history/control CURRENT: {:?}",
                cx.update(|cx| hex_ui::sampler_keyboard_test_status(window, cx))
            );
        }
        for _ in 0..8 {
            pulse(visual);
        }
    };
    let read_view = || {
        cx.update(|cx| hex_ui::sampler_source_test_view(window, track, cx))
            .unwrap()
            .unwrap()
    };
    let original_overview = read_view();
    let target_center = visual.debug_bounds(target_selector).unwrap().center();
    visual.simulate_click(target_center, Modifiers::default());
    visual.run_until_parked();
    let mut states = vec![initial.device(device).unwrap().state().clone()];
    for (parameter, value) in [(Gain, 0.5), (Transpose, -12.0), (AttackMs, 0.125)] {
        let old = applied().unwrap();
        let center = visual.debug_bounds(selector(parameter)).unwrap().center();
        visual.simulate_mouse_down(center, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(center, MouseButton::Left, Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input(&value.to_string());
        visual.simulate_keystrokes("enter");
        wait_next(visual, old);
        states.push(
            hex_engine::patch_sampler_parameter_on_control(
                parameter,
                states.last().unwrap(),
                value,
            )
            .unwrap(),
        );
    }
    // First Undo cancels an uncommitted local draft and consumes the shortcut.
    let old = applied().unwrap();
    let center = visual.debug_bounds(selector(Gain)).unwrap().center();
    visual.simulate_mouse_down(center, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(center, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("0.9");
    visual.simulate_keystrokes("cmd-z");
    for _ in 0..12 {
        pulse(visual);
    }
    assert_eq!(
        applied(),
        Some(old),
        "draft cancellation must consume global Undo"
    );
    // Dispatch Undo on the sampler, then select another device before completion.
    visual.simulate_keystrokes("cmd-z");
    let other_center = visual.debug_bounds(other_selector).unwrap().center();
    visual.simulate_click(other_center, Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual.debug_bounds(other_device_selector).is_some(),
        "actual other device panel must be selected"
    );
    let selected_focus = visual.update(|w, cx| w.focused(cx));
    wait_next(visual, old);
    assert_eq!(visual.update(|w, cx| w.focused(cx)), selected_focus);
    assert!(
        visual.debug_bounds(other_selected).is_some(),
        "history must preserve newer selection"
    );
    // The next global Undo still applies to the sampler while another device is selected.
    for (keys, index) in [
        ("cmd-z", 1),
        ("cmd-z", 0),
        ("cmd-shift-z", 1),
        ("cmd-shift-z", 2),
        ("cmd-shift-z", 3),
    ] {
        let old = applied().unwrap();
        visual.simulate_keystrokes(keys);
        wait_next(visual, old);
        assert!(visual.debug_bounds(other_selected).is_some());
        assert!(visual.debug_bounds(other_device_selector).is_some());
        let source = read_view();
        assert!(
            source
                .overviews
                .shares_storage_with(&original_overview.overviews)
        );
        assert_eq!(source.revision, applied().unwrap().context.editor_revision);
        visual.simulate_keystrokes("cmd-s");
        loop {
            pulse(visual);
            if hex_storage::load(saved)
                .unwrap()
                .device(device)
                .unwrap()
                .state()
                == &states[index]
                && cx
                    .update(|cx| hex_ui::sampler_midi_test_ready(window, cx))
                    .unwrap()
            {
                break;
            }
            assert!(Instant::now() < end, "history save state{index}");
        }
        // Literal original8k/output16k at projectA880: base step1, pitch-12 step.5.
        let play = visual
            .debug_bounds("play or pause · space")
            .unwrap()
            .center();
        visual.simulate_click(play, Modifiers::default());
        visual.run_until_parked();
        let mut output = [[0.0; 2]; 64];
        {
            let mut slot = h.backend.lock().unwrap();
            render(slot.as_mut().unwrap(), &mut output);
        }
        let expected = match index {
            0 => [[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25]],
            1 => [[0.0625, -0.125], [0.25, 0.0], [-0.0625, 0.125]],
            2 => [[0.0625, -0.125], [0.15625, -0.0625], [0.25, 0.0]],
            _ => [[0.0, 0.0], [0.078125, -0.03125], [0.25, 0.0]],
        };
        assert_eq!(&output[..3], &expected, "actual history PCM state{index}");
        let stop = visual.debug_bounds("stop · escape").unwrap().center();
        visual.simulate_click(stop, Modifiers::default());
        for _ in 0..8 {
            pulse(visual);
        }
    }
    let before_open = applied().unwrap();
    visual.simulate_keystrokes("cmd-n");
    visual.run_until_parked();
    visual.simulate_keystrokes("cmd-n");
    while visual.debug_bounds(target_selector).is_some() {
        pulse(visual);
        assert!(
            Instant::now() < end,
            "history New did not finish: {:?}",
            cx.update(|cx| hex_ui::sampler_keyboard_test_status(window, cx))
        );
    }
    visual.simulate_keystrokes("cmd-o");
    while !cx.did_prompt_for_paths() {
        pulse(visual);
        assert!(Instant::now() < end);
    }
    cx.simulate_paths_selection(Some(vec![saved.to_path_buf()]));
    let reopen_started = Instant::now();
    loop {
        pulse(visual);
        if applied().is_some_and(|a| {
            a.context.project_generation == before_open.context.project_generation + 2
        }) {
            break;
        }
        assert!(
            Instant::now() < end,
            "history saved state did not reopen into native CURRENT: total={:?}, reopen={:?}, before={before_open:?}, current={:?}, ui={:?}, prompt={:?}",
            started.elapsed(),
            reopen_started.elapsed(),
            applied(),
            cx.update(|cx| hex_ui::sampler_keyboard_test_status(window, cx)),
            cx.pending_prompt()
        );
    }
    for _ in 0..8 {
        pulse(visual);
    }
    let reopened = read_view();
    assert_eq!(reopened.asset, original_overview.asset);
    assert_eq!(reopened.source_frames, 4);
    assert!(
        reopened
            .overviews
            .matches_context(applied().unwrap().context)
    );
    let play = visual
        .debug_bounds("play or pause · space")
        .unwrap()
        .center();
    visual.simulate_click(play, Modifiers::default());
    visual.run_until_parked();
    let mut output = [[0.0; 2]; 64];
    {
        let mut slot = h.backend.lock().unwrap();
        render(slot.as_mut().unwrap(), &mut output);
    }
    assert_eq!(
        &output[..3],
        &[[0.0, 0.0], [0.078125, -0.03125], [0.25, 0.0]]
    );
    let stop = visual.debug_bounds("stop · escape").unwrap().center();
    visual.simulate_click(stop, Modifiers::default());
    for _ in 0..8 {
        pulse(visual);
    }
}
