//! Actual rendered ruler input, existing worker output, callback ACK and later paint.
use super::super::*;
use crate::editor::{
    device_panel::tests::eq_ui_tests::{graph_layout_tests, held_pointer_tests},
    midi_canvas::creation_tests::{fixture, pump},
};
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point, rgb};
use hex_project::{BeatTime, Command};
use std::{sync::atomic::Ordering::Acquire, time::Instant};

fn assert_playhead(visual: &mut VisualTestContext, fraction: f32) {
    let bounds = visual.debug_bounds("midi-note-roll").unwrap();
    let scale = gpui::ScaledPixels::from(visual.update(|window, _| window.scale_factor()));
    let color: gpui::Background = rgb(theme::color(theme::PaletteRole::Secondary)).into();
    let x = f32::from(bounds.origin.x) + 1.0 + (f32::from(bounds.size.width) - 2.0) * fraction;
    let all = visual.rendered_filled_quads("midi-note-roll");
    let secondary: Vec<_> = all
        .iter()
        .filter(|(_, c)| c == &color)
        .map(|(b, _)| *b)
        .collect();
    let lines: Vec<_> = all
        .into_iter()
        .filter(|(b, c)| {
            c == &color
                && (b.size.width / scale - 1.0).abs() < 0.05
                && (b.size.height / scale - (f32::from(bounds.size.height) - 2.0)).abs() < 0.05
        })
        .collect();
    assert_eq!(
        lines.len(),
        1,
        "one accepted playback cursor, separate from insertion marker; bounds={bounds:?}, secondary={secondary:?}"
    );
    assert!((lines[0].0.origin.x / scale - x).abs() < 0.1);
    assert!((lines[0].0.origin.y / scale - f32::from(bounds.origin.y) - 1.0).abs() < 0.1);
}

#[gpui::test]
fn rendered_ruler_cue_waits_for_callback_and_retains_source_start_and_selection(
    cx: &mut TestAppContext,
) {
    for stateful in [false, true] {
        rendered_ruler_case(cx, stateful);
    }
}
fn rendered_ruler_case(cx: &mut TestAppContext, stateful: bool) {
    let _guard = graph_layout_tests::status_fixture_guard();
    let (mut visual, editor, _environment, clip) = fixture(cx);
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::MoveMidiClip {
            clip,
            track: view.selected_track.unwrap(),
            start: BeatTime::from_quarters(8).unwrap(),
        })));
    });
    pump(&editor, &mut visual);
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::ONE,
            duration: BeatTime::from_raw(BeatTime::ONE.raw() / 4),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })));
    });
    pump(&editor, &mut visual);
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::InsertInstrument {
            track: view.selected_track.unwrap(),
            state: hex_project::default_instrument_state().unwrap(),
        })));
    });
    pump(&editor, &mut visual);
    if stateful {
        editor.update(cx, |view, _| {
            assert!(view.enqueue(Request::AddNativeDevice {
                track: view.selected_track.unwrap(),
                class: hex_engine::NativeDeviceClass::Effect,
                type_id: Box::new(hex_engine::eq::EQ_TYPE_ID),
                before: None,
            }));
        });
        pump(&editor, &mut visual);
    }
    let (mut output, mut backend) =
        held_pointer_tests::attach_actual_output(&editor, cx, &mut visual);
    editor.read_with(cx, |view, _| {
        assert_eq!(view.current_seek_requires_preroll(), Some(stateful))
    });

    let before = editor.read_with(cx, |view, _| {
        (
            view.state.as_ref().unwrap().revision,
            view.midi_marker,
            view.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .clone(),
            view.midi_selection.ids().to_vec(),
        )
    });
    assert_eq!(
        before.3.len(),
        1,
        "retain a real selected note across playback cueing"
    );
    let ruler = visual.debug_bounds("midi-source-ruler").unwrap();
    let at = point(
        ruler.origin.x + ruler.size.width * 0.25,
        ruler.origin.y + px(13.0),
    );
    visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert_eq!(view.audio_status().unwrap().position.load(Acquire), 0);
        assert_eq!(
            view.midi_playhead_source_beat(),
            None,
            "admission is not callback acceptance"
        );
    });
    let deadline = crate::test_wait::hang_deadline();
    loop {
        held_pointer_tests::pump_output(&editor, cx, &mut output, &mut backend);
        visual.run_until_parked();
        if editor.read_with(cx, |view, _| {
            view.midi_playhead_source_beat() == Some(BeatTime::ONE.raw())
        }) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "cue never acknowledged: {:?}",
            editor.read_with(cx, |v, _| v.error.clone())
        );
    }
    for _ in 0..3 {
        held_pointer_tests::pump_output(&editor, cx, &mut output, &mut backend);
        visual.run_until_parked();
        assert_playhead(&mut visual, 0.25);
    }
    editor.read_with(cx, |view, _| {
        assert_eq!(view.audio_status().unwrap().position.load(Acquire), 216_000);
        assert!(!view.audio_status().unwrap().playing.load(Acquire));
        assert_eq!(view.state.as_ref().unwrap().revision, before.0);
        assert_eq!(view.midi_marker, before.1);
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap(),
            &before.2
        );
        assert_eq!(view.midi_selection.ids(), before.3.as_slice());
    });
    editor.update(cx, |view, cx| {
        view.selected_clip = None;
        view.selected_track = None;
        assert!(view.detail_clip_id().is_none());
        assert!(view.midi_playhead_source_beat().is_none());
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("midi-note-roll").is_none());
    editor.update(cx, |view, _| {
        view.audio.take();
    });
    backend.close();
    drop(output);
}

#[gpui::test]
fn rendered_console_cue_requires_active_clip_and_moves_only_accepted_source_cursor(
    cx: &mut TestAppContext,
) {
    let _guard = graph_layout_tests::status_fixture_guard();
    let (mut visual, editor, _environment, _) = fixture(cx);
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::AddScene { name: "cue".into() })));
    });
    pump(&editor, &mut visual);
    let (track, scene) = editor.read_with(cx, |view, _| {
        (
            view.selected_track.unwrap(),
            view.state.as_ref().unwrap().project.scenes()[0].id(),
        )
    });
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "console cue".into(),
            source_length: BeatTime::from_quarters(4).unwrap(),
        })));
    });
    pump(&editor, &mut visual);
    let clip = editor.update(cx, |view, cx| {
        let hex_project::SessionCellContent::Midi(source) = view
            .state
            .as_ref()
            .unwrap()
            .project
            .session_cell(track, scene)
            .unwrap()
        else {
            panic!("midi cell")
        };
        let clip = source.id();
        view.select_session_midi(track, scene, clip);
        view.mode = EditorMode::Session;
        cx.notify();
        clip
    });
    visual.run_until_parked();
    let (mut output, mut backend) =
        held_pointer_tests::attach_actual_output(&editor, cx, &mut visual);
    let click = |visual: &mut VisualTestContext| {
        let ruler = visual.debug_bounds("midi-source-ruler").unwrap();
        let at = point(
            ruler.origin.x + ruler.size.width * 0.5,
            ruler.origin.y + px(13.0),
        );
        visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    };
    click(&mut visual);
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.error.as_deref(),
            Some("launch this clip before cueing it")
        );
        assert_eq!(view.midi_playhead_source_beat(), None);
        assert_eq!(view.audio_status().unwrap().position.load(Acquire), 0);
    });
    let revision = editor.update(cx, |view, _| {
        view.error = None;
        let revision = view.state.as_ref().unwrap().revision;
        view.queue_session_command(
            hex_engine::SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: hex_engine::LaunchQuantization::Immediate,
            },
            revision,
        );
        revision
    });
    let deadline = crate::test_wait::hang_deadline();
    loop {
        held_pointer_tests::pump_output(&editor, cx, &mut output, &mut backend);
        visual.run_until_parked();
        if editor.read_with(cx, |view, _| view.midi_playhead_source_beat() == Some(0)) {
            break;
        }
        assert!(Instant::now() < deadline, "launch never accepted");
    }
    click(&mut visual);
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.midi_playhead_source_beat(),
            Some(0),
            "request cannot move accepted cursor"
        )
    });
    let deadline = crate::test_wait::hang_deadline();
    loop {
        held_pointer_tests::pump_output(&editor, cx, &mut output, &mut backend);
        visual.run_until_parked();
        if editor.read_with(cx, |view, _| {
            view.midi_playhead_source_beat() == Some(BeatTime::from_quarters(2).unwrap().raw())
        }) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "cue never accepted: {:?}",
            editor.read_with(cx, |v, _| v.error.clone())
        );
    }
    for _ in 0..3 {
        held_pointer_tests::pump_output(&editor, cx, &mut output, &mut backend);
        visual.run_until_parked();
        assert_playhead(&mut visual, 0.5);
    }
    editor.read_with(cx, |view, _| {
        let status = view.audio_status().unwrap();
        assert_eq!(status.position.load(Acquire), 0);
        assert_eq!(status.performance_frame.load(Acquire), 0);
        assert!(!status.playing.load(Acquire));
        assert_eq!(view.state.as_ref().unwrap().revision, revision);
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .source_start(),
            BeatTime::ZERO
        );
    });
    editor.update(cx, |view, _| {
        view.audio.take();
    });
    backend.close();
    drop(output);
}
