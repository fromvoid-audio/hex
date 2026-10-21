//! Actual browser source replacement and cancellation, with canonical PCM oracles.
use super::envelope::render;
use super::*;
use gpui::{Modifiers, MouseButton, VisualTestContext};
use hex_engine::SamplerParameter::{
    self, Gain, LoopEnabled, LoopStartFrame, StartFrame, Transpose,
};
pub(super) fn exercise(
    cx: &gpui::TestAppContext,
    visual: &mut VisualTestContext,
    h: &mut Harness,
    window: gpui::AnyWindowHandle,
    track: hex_project::TrackId,
    saved: &std::path::Path,
) {
    let end = Instant::now() + Duration::from_secs(35);
    let initial = hex_storage::load(saved).unwrap();
    let device = initial.track(track).unwrap().instrument().unwrap().id();
    let other = initial.tracks().iter().find(|t| t.id() != track).unwrap();
    let other_id = other.id();
    let utility = other.devices().first().unwrap().id();
    let target_selector = Box::leak(format!("track-name-{}", track.value()).into_boxed_str());
    let other_selector = Box::leak(format!("track-name-{}", other_id.value()).into_boxed_str());
    let other_frame = Box::leak(format!("effect-device-{}", utility.value()).into_boxed_str());
    let selector = |parameter: SamplerParameter| -> &'static str {
        Box::leak(format!("sampler-{}-{}-value", parameter.key(), device.value()).into_boxed_str())
    };
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
            .and_then(|s| s.applied_audio_source())
    };
    let view = || {
        cx.update(|cx| hex_ui::sampler_source_test_view(window, track, cx))
            .unwrap()
            .unwrap()
    };
    let wait_next = |visual: &mut VisualTestContext,
                     old: hex_audio::prepared::AppliedAudioSource| {
        loop {
            pulse(visual);
            if applied().is_some_and(|a| {
                a.context.project_generation == old.context.project_generation
                    && a.context.editor_revision == old.context.editor_revision + 1
                    && a.output_identity != old.output_identity
            }) {
                break;
            }
            assert!(
                Instant::now() < end,
                "replacement/current: {:?}",
                cx.update(|cx| hex_ui::sampler_keyboard_test_status(window, cx))
            );
        }
        for _ in 0..8 {
            pulse(visual);
        }
    };
    let center = visual.debug_bounds(target_selector).unwrap().center();
    visual.simulate_click(center, Modifiers::default());
    visual.run_until_parked();
    for (parameter, value) in [
        (Gain, 0.5),
        (Transpose, -12.0),
        (StartFrame, 1.0),
        (LoopStartFrame, 1.0),
        (LoopEnabled, 1.0),
    ] {
        let old = applied().unwrap();
        let center = visual.debug_bounds(selector(parameter)).unwrap().center();
        visual.simulate_mouse_down(center, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(center, MouseButton::Left, Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input(&value.to_string());
        visual.simulate_keystrokes("enter");
        wait_next(visual, old);
    }
    let dispatch = |visual: &mut VisualTestContext| {
        let row = visual.debug_bounds("browser-entry-1").unwrap().center();
        visual.simulate_mouse_down(row, MouseButton::Right, Modifiers::default());
        visual.simulate_mouse_up(row, MouseButton::Right, Modifiers::default());
        visual.run_until_parked();
        let item = visual.debug_bounds("context-menu-item").unwrap().center();
        visual.simulate_click(item, Modifiers::default());
        visual.run_until_parked();
    };
    let old = applied().unwrap();
    let before = view();
    let cells = h.observation.lock().unwrap().gain_cells.len();
    let cancels = h.observation.lock().unwrap().sampler_cancel_count;
    // No callback pulse between dispatch and the second context action: the
    // real stopped fence cannot finish before this actual cancel menu action.
    let begins = h.observation.lock().unwrap().sampler_begin_count;
    dispatch(visual);
    // Observe the real begin before cancelling, without rendering its callback.
    while h.observation.lock().unwrap().sampler_begin_count == begins {
        cx.background_executor
            .advance_clock(Duration::from_millis(16));
        visual.run_until_parked();
        std::thread::sleep(Duration::from_millis(1));
        assert!(Instant::now() < end);
    }
    dispatch(visual);
    loop {
        pulse(visual);
        if h.observation.lock().unwrap().sampler_cancel_count > cancels {
            break;
        }
        assert!(
            Instant::now() < end,
            "source cancel did not reach native sink"
        );
    }
    for _ in 0..12 {
        pulse(visual);
    }
    assert_eq!(applied(), Some(old));
    assert_eq!(view().asset, before.asset);
    assert!(view().overviews.shares_storage_with(&before.overviews));
    assert_eq!(h.observation.lock().unwrap().gain_cells.len(), cells);
    dispatch(visual);
    let other_center = visual.debug_bounds(other_selector).unwrap().center();
    visual.simulate_click(other_center, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds(other_frame).is_some());
    let focus = visual.update(|w, cx| w.focused(cx));
    wait_next(visual, old);
    assert!(visual.debug_bounds(other_frame).is_some());
    assert_eq!(visual.update(|w, cx| w.focused(cx)), focus);
    let changed = view();
    assert_eq!(changed.device, device);
    assert_ne!(changed.asset, before.asset);
    assert_eq!((changed.source_frames, changed.playback_frames), (6, 8));
    assert!(changed.original_source.is_none());
    assert_eq!(changed.overviews.len(), 2);
    assert!(changed.overviews.get(before.asset).is_some());
    assert!(changed.overviews.get(changed.asset).is_some());
    assert!(
        changed
            .overviews
            .matches_context(applied().unwrap().context)
    );
    // Same device ID, larger source: actual retained endpoint widget must be
    // rebuilt for the new bounds, not keep the previous four-frame range.
    let center = visual.debug_bounds(target_selector).unwrap().center();
    visual.simulate_click(center, Modifiers::default());
    visual.run_until_parked();
    for value in [5.0_f64, 6.0] {
        let old = applied().unwrap();
        let center = visual
            .debug_bounds(selector(SamplerParameter::EndFrame))
            .unwrap()
            .center();
        visual.simulate_mouse_down(center, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(center, MouseButton::Left, Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input(&value.to_string());
        visual.simulate_keystrokes("enter");
        let bound_deadline = Instant::now() + Duration::from_secs(10);
        loop {
            pulse(visual);
            if applied().is_some_and(|a| {
                a.context.editor_revision == old.context.editor_revision + 1
                    && a.output_identity != old.output_identity
            }) {
                break;
            }
            assert!(
                Instant::now() < bound_deadline,
                "new source endpoint {value} did not fit refreshed widget bounds"
            );
        }
        for _ in 0..8 {
            pulse(visual);
        }
    }
    // A880/project16k/original12k/transpose-12 => .75 original frames per output.
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
        &output[..4],
        &[
            [0.125, 0.0625],
            [-0.15625, 0.109375],
            [-0.09375, 0.0],
            [0.046875, -0.03125]
        ]
    );
    let stop = visual.debug_bounds("stop · escape").unwrap().center();
    visual.simulate_click(stop, Modifiers::default());
    for _ in 0..8 {
        pulse(visual);
    }
    visual.simulate_keystrokes("cmd-s");
    loop {
        pulse(visual);
        let stored = hex_storage::load(saved).unwrap();
        if stored.assets().count() == 2 {
            let state = stored.device(device).unwrap().state();
            assert_eq!(
                state.asset_references()[0].asset.value(),
                changed.asset.value()
            );
            assert!(stored.assets().all(|a| a.original_source().is_some()));
            for (p, value) in [
                (Gain, 0.5),
                (Transpose, -12.0),
                (StartFrame, 0.0),
                (SamplerParameter::EndFrame, 6.0),
                (LoopEnabled, 0.0),
                (LoopStartFrame, 0.0),
                (SamplerParameter::LoopEndFrame, 6.0),
            ] {
                assert_eq!(
                    hex_engine::sampler_parameter_value_on_control(p, state).unwrap(),
                    value
                );
            }
            break;
        }
        assert!(
            Instant::now() < end,
            "actual replacement Save did not finish"
        );
    }
    let before_open = applied().unwrap();
    visual.simulate_keystrokes("cmd-n");
    visual.run_until_parked();
    visual.simulate_keystrokes("cmd-n");
    while visual.debug_bounds(target_selector).is_some() {
        pulse(visual);
        assert!(Instant::now() < end);
    }
    visual.simulate_keystrokes("cmd-o");
    while !cx.did_prompt_for_paths() {
        pulse(visual);
        assert!(Instant::now() < end);
    }
    cx.simulate_paths_selection(Some(vec![saved.to_path_buf()]));
    loop {
        pulse(visual);
        if applied().is_some_and(|a| {
            a.context.project_generation == before_open.context.project_generation + 2
        }) {
            break;
        }
        assert!(
            Instant::now() < end,
            "replacement reopen did not reach CURRENT"
        );
    }
    for _ in 0..8 {
        pulse(visual);
    }
    let reopened = view();
    assert_eq!(reopened.asset, changed.asset);
    assert_eq!(reopened.overviews.len(), 2);
    assert!(reopened.original_source.is_some());
    let play = visual
        .debug_bounds("play or pause · space")
        .unwrap()
        .center();
    visual.simulate_click(play, Modifiers::default());
    visual.run_until_parked();
    {
        let mut slot = h.backend.lock().unwrap();
        render(slot.as_mut().unwrap(), &mut output);
    }
    assert_eq!(
        &output[..4],
        &[
            [0.125, 0.0625],
            [-0.15625, 0.109375],
            [-0.09375, 0.0],
            [0.046875, -0.03125]
        ]
    );
    let stop = visual.debug_bounds("stop · escape").unwrap().center();
    visual.simulate_click(stop, Modifiers::default());
    for _ in 0..8 {
        pulse(visual);
    }
}
