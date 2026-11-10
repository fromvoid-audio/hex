//! Rendered retained-fade control and actual command regressions.
use super::*;
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext};
use hex_project::{BeatTime, Created, Editor, Tempo};
use std::sync::Arc;
#[gpui::test]
fn transformed_cropped_musical_fades_keep_readouts_reset_adjust_and_projected_curve(
    cx: &mut TestAppContext,
) {
    let (project, track, _, clip) = crate::editor::clip_drag_ui_test::audio_project();
    let mut canonical = Editor::new(project, 32).unwrap();
    canonical
        .execute(Command::SetClipFades {
            clip,
            fade_in: 4_800,
            fade_out: 9_600,
        })
        .unwrap();
    canonical
        .execute(Command::SetAudioClipPitch { clip, cents: 137 })
        .unwrap();
    canonical
        .execute(Command::SetAudioClipMusical {
            clip,
            enabled: true,
        })
        .unwrap();
    let Created::Clip(tail) = canonical
        .execute(Command::SplitClip {
            clip,
            frame: 31_337,
        })
        .unwrap()
    else {
        panic!("split")
    };
    canonical
        .execute(Command::SetTempoPoint {
            beat: BeatTime::ONE,
            tempo: Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    let project = canonical.project().clone();
    let tail_clip = project.clip(tail).unwrap();
    assert_eq!(tail_clip.fade().fade_in(), 0);
    assert_eq!(tail_clip.retained_fade().fade_in(), 4_800);
    let samples = crate::editor::waveform::projected_fade_envelope_samples(
        tail_clip,
        project.tempo_map(),
        tail_clip.start(),
        tail_clip.start() + tail_clip.duration(),
    )
    .unwrap();
    assert!(!samples.is_empty());
    assert_eq!(samples.first().unwrap().1, 1.0);
    assert_eq!(samples.last().unwrap().1, 0.0);
    // Frozen frame91200 maps from beat3.8 to158400 at the changed tempo.
    let edit = tail_clip.audio_edit().unwrap();
    assert!(
        (edit
            .projected_fade(project.tempo_map())
            .unwrap()
            .gain_at(158_400)
            .unwrap()
            - edit.domain.fade.gain_at(91_200))
        .abs()
            < 1e-6
    );
    for reset in [false, true] {
        let (handle, editor) =
            crate::editor::clip_drag_ui_test::editor_window(cx, project.clone(), Arc::default());
        editor.update(cx, |view, cx| {
            view.selected_track = Some(track);
            view.selected_clip = Some(tail);
            cx.notify();
        });
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        assert!(visual.debug_bounds("retained-fade-disclosure").is_some());
        let target = visual
            .debug_bounds(if reset { "fade-reset" } else { "fade-in-up" })
            .expect("real fade control")
            .center();
        visual.simulate_mouse_down(target, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        let command = editor.read_with(cx, |view, _| {
            view.last_enqueued_edit.clone().expect("fade edit")
        });
        let expected = if reset { (0, 0) } else { (5_040, 9_600) };
        let mut history = Editor::new(project.clone(), 4).unwrap();
        history.execute(command).unwrap();
        assert_eq!(
            (
                history
                    .project()
                    .clip(tail)
                    .unwrap()
                    .retained_fade()
                    .fade_in(),
                history
                    .project()
                    .clip(tail)
                    .unwrap()
                    .retained_fade()
                    .fade_out()
            ),
            expected
        );
        assert!(history.undo());
        assert_eq!(history.project(), &project);
    }
}

#[gpui::test]
fn detail_fade_length_control_creates_an_s_curve(cx: &mut TestAppContext) {
    let (project, track, _, clip) = crate::editor::clip_drag_ui_test::audio_project();
    let (handle, editor) =
        crate::editor::clip_drag_ui_test::editor_window(cx, project.clone(), Arc::default());
    editor.update(cx, |view, cx| {
        view.selected_track = Some(track);
        view.selected_clip = Some(clip);
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let target = visual
        .debug_bounds("fade-in-up")
        .expect("fade length control")
        .center();
    visual.simulate_mouse_down(target, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    let command = editor.read_with(cx, |view, _| {
        view.last_enqueued_edit.clone().expect("fade edit")
    });
    let mut history = Editor::new(project, 4).unwrap();
    history.execute(command).unwrap();
    let fade = history.project().clip(clip).unwrap().retained_fade();
    assert!(fade.fade_in() > 0);
    assert_eq!(fade.curve_laws()[0], ClipFade::DEFAULT_S_CURVE);
}
