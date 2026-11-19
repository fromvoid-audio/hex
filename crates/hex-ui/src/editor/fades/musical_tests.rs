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
        None,
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

#[gpui::test]
fn musical_repeat_shade_keeps_the_held_mesh_through_a_delayed_worker_ack(cx: &mut TestAppContext) {
    use crate::editor::{
        clip_drag_ui_test::editor_window, drag::group_drag_ui_test::pump,
        external_audio_drop_ui_test::BatchFiles, viewport::Viewport,
    };
    use gpui::{point, rgba};
    use hex_project::{AudioViewResizeOperation, Project};

    let files = BatchFiles::new();
    let path = files.wav("musical-repeat.wav", 48_000, 19_174);
    let (window, editor) = editor_window(cx, Project::new(48_000).unwrap(), Arc::default());
    let mut visual = VisualTestContext::from_window(window, cx);
    pump(&editor, &mut visual);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Import(path)))
    });
    pump(&editor, &mut visual);
    let clip = editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.tracks()[0].clips()[0].id()
    });
    for command in [
        Command::SetTempoPoint {
            beat: BeatTime::ONE,
            tempo: Tempo::new(72_000_000).unwrap(),
        },
        Command::SetAudioClipMusical {
            clip,
            enabled: true,
        },
        Command::ResizeAudioClipView {
            clip,
            operation: AudioViewResizeOperation::RepeatOuterEnd {
                project_frame: 82_000,
            },
        },
    ] {
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::Edit(command)))
        });
        pump(&editor, &mut visual);
    }
    let fade = editor.read_with(&visual, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        let clip = project.clip(clip).unwrap();
        ClipFade::new(clip.duration(), 0, 13_184, 23_254)
            .unwrap()
            .with_curve_laws([ClipFade::DEFAULT_S_CURVE; 2])
    });
    editor.update(&mut visual, |view, cx| {
        view.viewport = Some(Viewport {
            start: 0,
            length: 85_000,
        });
        view.fade_key_held = true;
        assert!(view.enqueue(Request::Edit(Command::SetClipFadeEnvelope { clip, fade })));
        cx.notify();
    });
    pump(&editor, &mut visual);

    let selector: &'static str =
        Box::leak(format!("clip-fade-shade-{}", clip.value()).into_boxed_str());
    let shade: gpui::Background = rgba(theme::CLIP_FADE_SHADE).into();
    let mesh = |visual: &mut VisualTestContext| {
        visual
            .rendered_path_triangles(selector)
            .into_iter()
            .filter(|path| path.color == shade)
            .map(|path| path.vertices)
            .collect::<Vec<_>>()
    };
    let original = mesh(&mut visual);
    assert!(
        !original.is_empty(),
        "musical Repeat lost its committed fade shade"
    );
    let selector: &'static str =
        Box::leak(format!("fade-in-curve-{}", clip.value()).into_boxed_str());
    let from = visual.debug_bounds(selector).unwrap().center();
    let to = point(from.x + px(9.0), from.y + px(8.0));
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let held = mesh(&mut visual);
    assert_ne!(held, original);
    assert!(!held.is_empty());
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    let deadline = crate::test_wait::hang_deadline();
    let terminal = loop {
        if let Ok(event) = editor.read_with(&visual, |view, _| view.service.try_event()) {
            if event.request_complete {
                break event;
            }
            editor.update(&mut visual, |view, _| view.receive(event));
        }
        assert!(
            std::time::Instant::now() < deadline,
            "musical fade terminal"
        );
        std::thread::yield_now();
    };
    assert!(terminal.error.is_none(), "{:?}", terminal.error);
    for _ in 0..2 {
        editor.update(&mut visual, |view, cx| {
            view.fade_key_held = false;
            assert!(view.pending_fade.is_some());
            cx.notify();
        });
        visual.run_until_parked();
        assert_eq!(mesh(&mut visual), held, "pre-ack repaint");
    }
    editor.update(&mut visual, |view, cx| {
        view.receive(terminal);
        assert!(view.pending_fade.is_none());
        cx.notify();
    });
    visual.run_until_parked();
    assert_eq!(mesh(&mut visual), held, "installed canonical repaint");
}
