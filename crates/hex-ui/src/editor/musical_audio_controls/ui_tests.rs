use super::*;
use gpui::{Modifiers, MouseButton, TestVisualTestContext};
use hex_session::service::ViewState;
use std::sync::Arc;
const CHOICES: [&str; 6] = [
    "audio-clip-warp-mode-0",
    "audio-clip-warp-mode-1",
    "audio-clip-warp-mode-2",
    "audio-clip-warp-mode-3",
    "audio-clip-warp-mode-4",
    "audio-clip-warp-mode-5",
];

#[gpui::test]
fn selected_clip_warp_toggle_is_visible_and_dispatches_transient_beats(cx: &mut TestAppContext) {
    let (handle, editor, _, clip) = super::super::clip_view_ui_test::audio_detail_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let toggle = visual
        .debug_bounds("audio-clip-warp-toggle")
        .expect("visible warp toggle");
    let sidebar = visual.debug_bounds("arrangement-audio-controls").unwrap();
    assert!(toggle.origin.y >= sidebar.origin.y && toggle.bottom() <= sidebar.bottom());
    visual.simulate_mouse_down(toggle.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(toggle.center(), MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(matches!(&view.last_enqueued_edit, Some(Command::SetAudioClipWarp { clip: id, settings: Some(settings) })
            if *id == clip && settings.mode == modes()[0]));
    });
}
#[gpui::test]
fn algorithm_picker_dispatches_direct_choice_without_resetting_current_settings(
    cx: &mut TestAppContext,
) {
    assert_mode_choice(cx, 0);
    assert_mode_choice(cx, 1);
}
fn assert_mode_choice(cx: &mut Testrequested: usize) {
    let (handle, editor, _, clip) = super::super::clip_view_ui_test::audio_detail_fixture(cx);
    editor.update(cx, |view, cx| {
        let old = view.state.as_ref().unwrap();
        let mut model = hex_project::Editor::new((*old.project).clone(), 8).unwrap();
        model
            .execute(command(model.project(), clip, Action::Warp).unwrap())
            .unwrap();
        model
            .execute(command(model.project(), clip, Action::Decay(-40)).unwrap())
            .unwrap();
        view.state = Some(Arc::new(ViewState {
            canonical_audio_source: None,
            project: Arc::new(model.project().clone()),
            waveforms: old.waveforms.clone(),
            audio_clip_waveforms: old.audio_clip_waveforms.clone(),
            midi_preview: old.midi_preview.clone(),
            gain_preview: old.gain_preview.clone(),
            timeline_frames: old.timeline_frames,
            can_undo: true,
            can_redo: false,
            revision: old.revision + 1,
            unsaved: true,
            document_path: None,
        }));
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let sidebar = visual.debug_bounds("arrangement-audio-controls").unwrap();
    for selector in CHOICES {
        let bounds = visual
            .debug_bounds(selector)
            .expect("implemented algorithm choice");
        assert!(bounds.origin.y >= sidebar.origin.y && bounds.bottom() <= sidebar.bottom());
    }
    // Re-selecting Beats preserves the user's decay instead of resetting defaults.
    let bounds = visual.debug_bounds(CHOICES[requested]).unwrap();
    visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(matches!(&view.last_enqueued_edit,
        Some(Command::SetAudioClipWarp{clip:id,settings:Some(settings)})
        if *id == clip && if requested == 0 { matches!(settings.mode,WarpMode::Beats{decay:60,..}) }
            else { settings.mode == modes()[requested] }))
    });
}
