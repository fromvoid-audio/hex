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
    assert_eq!(
        (
            tail_clip.retained_fade().offset(),
            tail_clip.retained_fade().fade_in(),
            tail_clip.retained_fade().fade_out()
        ),
        (0, 0, 9_600)
    );
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
    let outer = tail_clip
        .projected_outer_fade(project.tempo_map())
        .unwrap()
        .unwrap();
    assert_eq!(outer.gain_at(tail_clip.start()).unwrap(), 1.0);
    assert_eq!(
        outer
            .gain_at(tail_clip.start() + tail_clip.duration() - 1)
            .unwrap(),
        0.0
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
        let target = visual
            .debug_bounds(if reset { "fade-reset" } else { "fade-in-up" })
            .expect("real fade control")
            .center();
        visual.simulate_mouse_down(target, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        let command = editor.read_with(cx, |view, _| {
            view.test_probes
                .last_enqueued_edit
                .clone()
                .expect("fade edit")
        });
        let expected = if reset { (0, 0) } else { (240, 9_600) };
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
        view.test_probes
            .last_enqueued_edit
            .clone()
            .expect("fade edit")
    });
    let mut history = Editor::new(project, 4).unwrap();
    history.execute(command).unwrap();
    let fade = history.project().clip(clip).unwrap().retained_fade();
    assert!(fade.fade_in() > 0);
    assert_eq!(fade.curve_laws()[0], ClipFade::DEFAULT_S_CURVE);
}

#[gpui::test]
fn musical_repeat_shade_keeps_the_held_mesh_through_a_delayed_worker_ack(cx: &mut TestAppContext) {
    musical_repeat_fade_workflow(cx, false);
}

#[gpui::test]
fn legacy_musical_repeat_adopts_outer_fade_without_a_waveform_flash(cx: &mut TestAppContext) {
    musical_repeat_fade_workflow(cx, true);
}

fn musical_repeat_fade_workflow(cx: &mut TestAppContext, legacy: bool) {
    use crate::editor::{
        clip_drag_ui_test::editor_window, drag::group_drag_ui_test::pump,
        external_audio_drop_ui_test::BatchFiles, viewport::Viewport,
        waveform_geometry::painted_clip_waveform_outline,
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
        view.interaction.fade_key_held = true;
        view.timeline_caches
            .waveform_geometry_cache
            .borrow_mut()
            .clear_paint_samples();
        cx.notify();
    });
    visual.run_until_parked();
    let neutral_waveform = painted_clip_waveform_outline(&editor, &mut visual, clip);
    let basis = editor.read_with(&visual, |view, _| {
        let state = view.state.as_ref().unwrap();
        let neutral = state.audio_clip_waveforms[&clip]
            .neutral_musical
            .as_ref()
            .unwrap();
        assert!(neutral.waveform.bounded_samples(0..1, 1).is_none());
        Arc::clone(&neutral.waveform)
    });
    let assert_shared_basis = |visual: &VisualTestContext| {
        editor.read_with(visual, |view, _| {
            let state = view.state.as_ref().unwrap();
            let neutral = state.audio_clip_waveforms[&clip]
                .neutral_musical
                .as_ref()
                .unwrap();
            assert!(Arc::ptr_eq(&neutral.waveform, &basis));
        });
    };
    let initial = if legacy {
        Command::SetClipFades {
            clip,
            fade_in: 13_184,
            fade_out: 23_254,
        }
    } else {
        Command::SetClipFadeEnvelope { clip, fade }
    };
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(initial)));
    });
    pump(&editor, &mut visual);
    let before = editor.read_with(&visual, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error);
        let project = &view.state.as_ref().unwrap().project;
        let clip = project.clip(clip).unwrap();
        if legacy {
            assert_eq!(
                clip.audio_edit().unwrap().domain.fade_clock,
                hex_project::AudioClipFadeClock::LegacyRoot
            );
            assert!(clip.retained_fade().fade_in() > 0);
            let tempo = project.tempo_map();
            let projected = hex_project::AudioViewProjection::new(clip, tempo).unwrap();
            assert!(projected.occurrence_count() > 1);
            for index in 0..projected.occurrence_count() {
                let occurrence = projected.occurrence(index).unwrap();
                let samples = crate::editor::waveform::projected_fade_envelope_samples(
                    clip,
                    tempo,
                    occurrence.destination_start,
                    (occurrence.destination_start + 2).min(occurrence.destination_end),
                    None,
                )
                .unwrap();
                assert_eq!(samples.first(), Some(&(0.0, 0.0)));
            }
        } else {
            assert_eq!(clip.retained_fade(), fade);
        }
        (
            clip.audio_edit().unwrap().domain.fade_clock,
            clip.retained_fade(),
        )
    });
    if !legacy {
        assert_shared_basis(&visual);
    }

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
    let original_waveform = painted_clip_waveform_outline(&editor, &mut visual, clip);
    assert_ne!(original_waveform, neutral_waveform);
    if legacy {
        assert!(visual.debug_bounds(selector).is_some());
    } else {
        assert!(
            !original.is_empty(),
            "musical Repeat lost its committed fade shade"
        );
    }
    let selector: &'static str =
        Box::leak(format!("fade-in-curve-{}", clip.value()).into_boxed_str());
    let from = visual.debug_bounds(selector).unwrap().center();
    let to = point(from.x + px(9.0), from.y + px(8.0));
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |view, _| {
        view.timeline_caches
            .waveform_geometry_cache
            .borrow_mut()
            .clear_paint_samples();
    });
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let held = mesh(&mut visual);
    let held_waveform = painted_clip_waveform_outline(&editor, &mut visual, clip);
    assert_ne!(
        held_waveform, original_waveform,
        "held fade reshapes the musical waveform"
    );
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
            view.interaction.fade_key_held = false;
            assert!(view.interaction.pending_fade.is_some());
            view.timeline_caches
                .waveform_geometry_cache
                .borrow_mut()
                .clear_paint_samples();
            cx.notify();
        });
        visual.run_until_parked();
        assert_eq!(mesh(&mut visual), held, "pre-ack repaint");
        assert_eq!(
            painted_clip_waveform_outline(&editor, &mut visual, clip),
            held_waveform
        );
    }
    editor.update(&mut visual, |view, cx| {
        view.receive(terminal);
        assert!(view.interaction.pending_fade.is_none());
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(
            project
                .clip(clip)
                .unwrap()
                .audio_edit()
                .unwrap()
                .domain
                .fade_clock,
            hex_project::AudioClipFadeClock::Outer,
        );
        view.timeline_caches
            .waveform_geometry_cache
            .borrow_mut()
            .clear_paint_samples();
        cx.notify();
    });
    visual.run_until_parked();
    if !legacy {
        assert_shared_basis(&visual);
    }
    assert_eq!(mesh(&mut visual), held, "installed canonical repaint");
    assert_eq!(
        painted_clip_waveform_outline(&editor, &mut visual, clip),
        held_waveform
    );

    editor.update(&mut visual, |view, _| {
        view.timeline_caches
            .waveform_geometry_cache
            .borrow_mut()
            .clear_paint_samples();
        assert!(view.enqueue(Request::Undo));
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        let clip = project.clip(clip).unwrap();
        assert_eq!(
            (
                clip.audio_edit().unwrap().domain.fade_clock,
                clip.retained_fade()
            ),
            before,
        );
    });
    assert_eq!(
        painted_clip_waveform_outline(&editor, &mut visual, clip),
        original_waveform
    );
    editor.update(&mut visual, |view, _| {
        view.timeline_caches
            .waveform_geometry_cache
            .borrow_mut()
            .clear_paint_samples();
        assert!(view.enqueue(Request::Edit(Command::SetClipFadeEnvelope {
            clip,
            fade: ClipFade::neutral(fade.span()).unwrap(),
        })));
    });
    pump(&editor, &mut visual);
    if !legacy {
        assert_shared_basis(&visual);
    }
    assert_eq!(
        painted_clip_waveform_outline(&editor, &mut visual, clip),
        neutral_waveform
    );
}
