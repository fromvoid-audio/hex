//! Actual title-strip paint with disposable worker/catalog startup.
use super::super::{EditorView, test_environment::Environment, viewport::Viewport};
use crate::theme;
use gpui::{AppContext, Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext};
use hex_project::{BeatTime, ClipFlavor, ClipId, Command, Created, ElementColor, TrackId};
use hex_session::service::{Event, Request, Service};
use std::{sync::Arc, time::Duration};

struct Fixture {
    _environment: Arc<Environment>,
    window: gpui::AnyWindowHandle,
    editor: Entity<EditorView>,
    track: TrackId,
    audio: ClipId,
    midi: ClipId,
}
fn next(service: &Service) -> Event {
    let until = crate::test_wait::hang_deadline();
    loop {
        if let Ok(event) = service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            return event;
        }
        assert!(
            std::time::Instant::now() < until,
            "next: hang guard expired"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn fixture(cx: &mut TestAppContext) -> Fixture {
    fixture_with_environment(cx, Environment::new().unwrap())
}
fn fixture_with_environment(cx: &mut TestAppContext, environment: Arc<Environment>) -> Fixture {
    cx.update(super::super::bind_editor_keys);
    let directory = environment.root();
    std::fs::create_dir_all(directory.join("samples")).unwrap();
    let wav = directory.join("samples/clip-title.wav");
    let count = 96_000_u32;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + count * 2).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&48_000_u32.to_le_bytes());
    bytes.extend_from_slice(&96_000_u32.to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(count * 2).to_le_bytes());
    for _ in 0..count {
        bytes.extend_from_slice(&8192_i16.to_le_bytes());
    }
    std::fs::write(&wav, bytes).unwrap();
    let service = Service::new(48_000, 16, 8_000_000).unwrap();
    next(&service);
    service.submit(Request::Import(wav.clone())).unwrap();
    let event = next(&service);
    let source = &event.state.project.tracks()[0];
    let track = source.id();
    let audio = source.clips()[0].id();
    service
        .submit(Request::Edit(Command::SetTrackColor {
            track,
            color: ElementColor::Cyan,
        }))
        .unwrap();
    next(&service);
    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track,
            start: BeatTime::from_quarters(6).unwrap(),
            length: BeatTime::ONE,
        }))
        .unwrap();
    let event = next(&service);
    let Created::Clip(midi) = event.created.unwrap() else {
        panic!("MIDI clip")
    };
    service
        .submit(Request::Edit(Command::SetClipFlavor {
            clip: audio,
            flavor: ClipFlavor::Muted,
        }))
        .unwrap();
    next(&service);
    service
        .submit(Request::Edit(Command::SetClipFlavor {
            clip: midi,
            flavor: ClipFlavor::Accent,
        }))
        .unwrap();
    let event = next(&service);
    let window = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(window, |_, window, cx| {
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_environment(
                    window,
                    cx,
                    service,
                    Arc::clone(&environment),
                )
            });
            editor.update(cx, |v, cx| {
                assert!(Arc::ptr_eq(&environment, &v.test_environment));
                assert_eq!(v.browser.directory, directory.join("samples"));
                assert_eq!(v.browser.places, vec![directory.join("samples")]);
                v.state = Some(event.state);
                v.viewport = Some(Viewport {
                    start: 0,
                    length: 192_000,
                });
                v.focus.focus(window);
                cx.notify();
            });
            editor
        })
        .unwrap();
    Fixture {
        _environment: environment,
        window,
        editor,
        track,
        audio,
        midi,
    }
}
fn selector(prefix: &str, clip: ClipId) -> &'static str {
    Box::leak(format!("{prefix}-{}", clip.value()).into_boxed_str())
}
fn assert_header_geometry(visual: &mut VisualTestContext, clip: ClipId) {
    let header = selector("clip-name", clip);
    let bounds = visual.debug_bounds(header).unwrap();
    assert_eq!(bounds.size.height, gpui::px(14.0));
    let (_, widths, _) = visual.rendered_border(header).unwrap();
    let scale = visual.update(|window, _| window.scale_factor());
    assert_eq!(f64::from(widths.bottom), f64::from(scale));
    let label = visual
        .debug_bounds(selector("clip-name-label", clip))
        .unwrap();
    assert_eq!(label.size.height, gpui::px(14.0));
    assert!((f32::from(label.center().y - bounds.center().y) + 0.5).abs() <= 0.5);
}
fn click(v: &mut VisualTestContext, clip: ClipId, modifiers: Modifiers) {
    let p = v
        .debug_bounds(selector("clip-name", clip))
        .unwrap()
        .center();
    v.simulate_mouse_down(p, MouseButton::Left, modifiers);
    v.simulate_mouse_up(p, MouseButton::Left, modifiers);
    v.run_until_parked();
}
fn pump(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    let until = crate::test_wait::hang_deadline();
    loop {
        let done = editor.update(&mut *visual, |v, cx| {
            if v.tick(cx) {
                cx.notify();
            }
            v.worker_requests_pending == 0 && v.pending_requests.is_empty()
        });
        if done {
            visual.run_until_parked();
            return;
        }
        assert!(
            std::time::Instant::now() < until,
            "pump: hang guard expired"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[gpui::test]
fn clip_titles_paint_inherited_flavor_colors_through_selection_and_recolor(
    cx: &mut TestAppContext,
) {
    let f = fixture(cx);
    let mut visual = VisualTestContext::from_window(f.window, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    let bounds =
        [f.audio, f.midi].map(|id| visual.debug_bounds(selector("clip-name", id)).unwrap());
    for clip in [f.audio, f.midi] {
        assert_header_geometry(&mut visual, clip);
    }
    let original_audio = f.editor.read_with(&visual, |v, _| {
        v.state
            .as_ref()
            .unwrap()
            .project
            .clip(f.audio)
            .unwrap()
            .clone()
    });
    let original_midi = f.editor.read_with(&visual, |v, _| {
        v.state
            .as_ref()
            .unwrap()
            .project
            .midi_clip(f.midi)
            .unwrap()
            .clone()
    });
    for color in [ElementColor::Cyan, ElementColor::Rose] {
        if color == ElementColor::Rose {
            f.editor.update(&mut visual, |v, _| {
                v.enqueue(Request::Edit(Command::SetTrackColor {
                    track: f.track,
                    color,
                }));
            });
            pump(&f.editor, &mut visual);
        }
        for select in [false, true] {
            if select {
                click(&mut visual, f.audio, Modifiers::default());
                click(
                    &mut visual,
                    f.midi,
                    Modifiers {
                        platform: true,
                        ..Modifiers::default()
                    },
                );
            } else {
                f.editor.update(&mut visual, |v, cx| {
                    v.selected_clip = None;
                    v.reconcile_clip_selection();
                    cx.notify();
                });
                visual.run_until_parked();
            }
            visual.simulate_mouse_move(
                gpui::point(gpui::px(0.0), gpui::px(0.0)),
                None,
                Modifiers::default(),
            );
            visual.run_until_parked();
            for (i, (clip, flavor, prefix)) in [
                (f.audio, ClipFlavor::Muted, "clip-visual"),
                (f.midi, ClipFlavor::Accent, "midi-clip-visual"),
            ]
            .into_iter()
            .enumerate()
            {
                let header = selector("clip-name", clip);
                let actual = visual
                    .rendered_background(header)
                    .expect("actual completed title quad");
                let expected = theme::clip_fill(color, flavor, false);
                assert_eq!(
                    actual,
                    gpui::Background::from(gpui::rgb(expected)),
                    "painted title {clip:?} {color:?} {flavor:?} selected={select}"
                );
                assert_ne!(
                    actual,
                    gpui::Background::from(gpui::rgb(theme::color(theme::PaletteRole::Raised))),
                    "title retains inherited hue"
                );
                assert_eq!(
                    visual.rendered_background(selector(prefix, clip)).unwrap(),
                    gpui::Background::from(gpui::rgb(theme::arrangement_clip_body_fill(
                        expected, select
                    ))),
                    "actual audio/MIDI body retains its selection fill"
                );
                let (divider_color, divider_widths, _) = visual.rendered_border(header).unwrap();
                assert!(divider_widths.bottom > gpui::ScaledPixels::default());
                assert!(
                    divider_widths.bottom
                        <= gpui::ScaledPixels::from(
                            visual.update(|window, _| window.scale_factor())
                        )
                );
                assert_eq!(divider_widths.top, gpui::ScaledPixels::default());
                assert_eq!(divider_widths.left, gpui::ScaledPixels::default());
                assert_eq!(divider_widths.right, gpui::ScaledPixels::default());
                let ink = theme::readable_text(expected, theme::color(theme::PaletteRole::Text));
                assert_eq!(divider_color, gpui::rgba((ink << 8) | 0x70).into());
                assert_eq!(
                    visual.debug_bounds(header).unwrap(),
                    bounds[i],
                    "colors and selection preserve header hit bounds"
                );
                let outer = visual.debug_bounds(selector(prefix, clip)).unwrap();
                assert!(
                    bounds[i].top() >= outer.top() && bounds[i].bottom() <= outer.bottom(),
                    "minimum row contains title strip"
                );
                assert_eq!(
                    visual.rendered_border(selector(prefix, clip)).unwrap().0,
                    gpui::Hsla::from(gpui::rgb(if select {
                        theme::color(theme::PaletteRole::Primary)
                    } else {
                        theme::color(theme::PaletteRole::ClipOutline)
                    })),
                    "selection outline remains independent"
                );
                eprintln!(
                    "title painted clip={} hue={color:?} flavor={flavor:?} selected={select} fill={expected:06x}",
                    clip.value()
                );
            }
        }
    }
    // Existing header selection and inline-name focus still own their inputs.
    click(&mut visual, f.audio, Modifiers::default());
    visual.simulate_keystrokes("cmd-r");
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds("clip-name-input")
            .unwrap()
            .intersects(&bounds[0])
    );
    visual.simulate_input("title draft");
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    // The compact row still has a stable 14px name strip and a separately
    // selected container surface; both members retain the same policy.
    visual.simulate_keystrokes("h");
    visual.run_until_parked();
    for selected in [true, false] {
        if selected {
            click(&mut visual, f.audio, Modifiers::default());
            click(
                &mut visual,
                f.midi,
                Modifiers {
                    platform: true,
                    ..Modifiers::default()
                },
            );
        } else {
            f.editor.update(&mut visual, |v, cx| {
                v.selected_clip = None;
                v.reconcile_clip_selection();
                cx.notify();
            });
        }
        visual.run_until_parked();
        for (clip, flavor, prefix) in [
            (f.audio, ClipFlavor::Muted, "clip-visual"),
            (f.midi, ClipFlavor::Accent, "midi-clip-visual"),
        ] {
            let header = theme::clip_fill(ElementColor::Rose, flavor, false);
            let body = theme::arrangement_clip_body_fill(header, selected);
            let outer = visual.debug_bounds(selector(prefix, clip)).unwrap();
            assert!(f32::from(outer.size.height) <= super::super::track_layout::HEADER_ONLY_HEIGHT);
            assert_eq!(
                visual.rendered_background(selector("clip-name", clip)),
                Some(gpui::rgb(header).into())
            );
            assert_eq!(
                visual.rendered_background(selector(prefix, clip)),
                Some(gpui::rgb(body).into())
            );
            if selected {
                assert_ne!(
                    body, header,
                    "compact selection visibly lifts the container"
                );
            }
        }
    }
    f.editor.read_with(&visual, |v, _| {
        assert!(v.clip_name_edit.is_none());
        let project = &v.state.as_ref().unwrap().project;
        assert_eq!(project.clip(f.audio).unwrap(), &original_audio);
        assert_eq!(project.midi_clip(f.midi).unwrap(), &original_midi);
    });
}

#[gpui::test]
fn console_audio_midi_flavors_follow_shared_fill_and_history_without_geometry_changes(
    cx: &mut TestAppContext,
) {
    let f = fixture(cx);
    let mut visual = VisualTestContext::from_window(f.window, cx);
    let mut cells = Vec::new();
    for (source, audio) in [(f.audio, true), (f.midi, false)] {
        f.editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::AddScene {
                name: "colors".into()
            })));
        });
        pump(&f.editor, &mut visual);
        let scene = f.editor.read_with(&visual, |v, _| {
            v.state
                .as_ref()
                .unwrap()
                .project
                .scenes()
                .last()
                .unwrap()
                .id()
        });
        f.editor.update(&mut visual, |v, _| {
            let command = if audio {
                Command::CopyArrangementAudioClipToSession { source, scene }
            } else {
                Command::CopyArrangementMidiToSession { source, scene }
            };
            assert!(v.enqueue(Request::Edit(command)));
        });
        pump(&f.editor, &mut visual);
        let clip = f.editor.read_with(&visual, |v, _| {
            match v
                .state
                .as_ref()
                .unwrap()
                .project
                .session_cell(f.track, scene)
                .unwrap()
            {
                hex_project::SessionCellContent::Audio(clip) => clip.id(),
                hex_project::SessionCellContent::Midi(clip) => clip.id(),
                hex_project::SessionCellContent::Stop => panic!("copied clip cell"),
            }
        });
        cells.push((scene, clip, audio));
    }
    f.editor.update(&mut visual, |v, cx| {
        v.mode = super::super::EditorMode::Session;
        cx.notify();
    });
    visual.run_until_parked();
    for (scene, clip, audio) in cells {
        let cell = Box::leak(
            format!("console-cell-{}-{}", f.track.value(), scene.value()).into_boxed_str(),
        );
        let bounds = visual.debug_bounds(cell).unwrap();
        let surface = selector("console-clip", clip);
        let painted = visual.debug_bounds(surface).unwrap();
        assert_eq!(bounds.size.height, gpui::px(31.0));
        assert_eq!(painted.size.height, gpui::px(26.0));
        assert!(painted.left() > bounds.left() && painted.right() < bounds.right());
        super::super::clip_boundary::tests::assert_open_upper_right(
            &mut visual,
            surface,
            selector("console-clip-trailing", clip),
            None,
        );
        let label = visual
            .debug_bounds(selector("clip-name-label", clip))
            .unwrap();
        let launch = Box::leak(
            format!(
                "launch-{}-{}-{}",
                if audio { "audio" } else { "midi" },
                f.track.value(),
                scene.value(),
            )
            .into_boxed_str(),
        );
        let launch = visual.debug_bounds(launch).unwrap();
        assert_eq!(launch.size, gpui::size(gpui::px(19.0), gpui::px(19.0)));
        assert!(launch.left() >= painted.left() && launch.right() < label.left());
        assert!(launch.top() >= painted.top() && launch.bottom() <= painted.bottom());
        assert_eq!(label.size.height, gpui::px(14.0));
        assert!((f32::from(label.center().y - bounds.center().y)).abs() <= 0.5);
        for hue in [ElementColor::Cyan, ElementColor::Rose] {
            f.editor.update(&mut visual, |v, _| {
                assert!(v.enqueue(Request::Edit(Command::SetTrackColor {
                    track: f.track,
                    color: hue
                })));
            });
            pump(&f.editor, &mut visual);
            for flavor in [ClipFlavor::Muted, ClipFlavor::Accent] {
                f.editor.update(&mut visual, |v, _| {
                    assert!(v.enqueue(Request::Edit(Command::SetClipFlavor { clip, flavor })));
                });
                pump(&f.editor, &mut visual);
                let before = f
                    .editor
                    .read_with(&visual, |v, _| v.state.as_ref().unwrap().project.clone());
                for selected in [false, true] {
                    f.editor.update(&mut visual, |v, cx| {
                        v.selected_cell = selected.then_some((f.track, scene));
                        cx.notify();
                    });
                    visual.run_until_parked();
                    assert_eq!(
                        visual.rendered_background(surface).unwrap(),
                        gpui::Background::from(gpui::rgb(theme::clip_fill(hue, flavor, selected)))
                    );
                    assert_eq!(visual.debug_bounds(cell).unwrap(), bounds);
                }
                f.editor.update(&mut visual, |v, _| {
                    assert!(v.enqueue(Request::Edit(Command::SetClipFlavor {
                        clip,
                        flavor: flavor.toggled()
                    })))
                });
                pump(&f.editor, &mut visual);
                f.editor
                    .update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
                pump(&f.editor, &mut visual);
                f.editor.read_with(&visual, |v, _| {
                    assert_eq!(v.state.as_ref().unwrap().project, before)
                });
                f.editor
                    .update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
                pump(&f.editor, &mut visual);
                let selected = f.editor.read_with(&visual, |v, _| {
                    assert_eq!(
                        v.state.as_ref().unwrap().project.clip_flavor(clip),
                        Some(flavor.toggled())
                    );
                    // Existing history reconciliation may retire cell selection;
                    // compare the actual painted state, not a fabricated focus.
                    v.selected_cell == Some((f.track, scene))
                });
                assert_eq!(
                    visual.rendered_background(surface).unwrap(),
                    gpui::Background::from(gpui::rgb(theme::clip_fill(
                        hue,
                        flavor.toggled(),
                        selected
                    )))
                );
            }
        }
        f.editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::SetClipActive {
                clip,
                active: false
            })));
        });
        pump(&f.editor, &mut visual);
        let inactive_fill = f.editor.read_with(&visual, |v, _| {
            let project = &v.state.as_ref().unwrap().project;
            theme::clip_activity_fill(
                project.track(f.track).unwrap().color(),
                project.clip_flavor(clip).unwrap(),
                v.selected_cell == Some((f.track, scene)),
                false,
            )
        });
        assert_eq!(
            visual.rendered_background(surface),
            Some(gpui::rgb(inactive_fill).into())
        );
        for command in [
            Command::ClearSessionCell {
                track: f.track,
                scene,
            },
            Command::SetSessionStopCell {
                track: f.track,
                scene,
            },
        ] {
            f.editor.update(&mut visual, |v, _| {
                assert!(v.enqueue(Request::Edit(command)))
            });
            pump(&f.editor, &mut visual);
            assert!(
                visual.debug_bounds(surface).is_none(),
                "empty/stop cells have no colored clip"
            );
            let role = f.editor.read_with(&visual, |v, _| {
                if v.selected_cell == Some((f.track, scene)) {
                    theme::PaletteRole::Raised
                } else {
                    theme::PaletteRole::Background
                }
            });
            assert_eq!(
                visual.rendered_background(cell),
                Some(gpui::rgb(theme::color(role)).into())
            );
            assert_eq!(visual.debug_bounds(cell).unwrap(), bounds);
        }
    }
}

#[gpui::test]
fn disabled_audio_and_midi_clips_paint_neutral_bodies_and_titles_and_restore_color(
    cx: &mut TestAppContext,
) {
    let f = fixture(cx);
    let mut visual = VisualTestContext::from_window(f.window, cx);
    f.editor.update(&mut visual, |view, cx| {
        view.selected_clip = None;
        view.reconcile_clip_selection();
        cx.notify();
    });
    visual.run_until_parked();
    for active in [false, true] {
        for clip in [f.audio, f.midi] {
            f.editor.update(&mut visual, |view, _| {
                assert!(view.enqueue(Request::Edit(Command::SetClipActive { clip, active })));
            });
            pump(&f.editor, &mut visual);
        }
        for (clip, flavor, prefix) in [
            (f.audio, ClipFlavor::Muted, "clip-visual"),
            (f.midi, ClipFlavor::Accent, "midi-clip-visual"),
        ] {
            let expected: gpui::Background = gpui::rgb(theme::clip_activity_fill(
                ElementColor::Cyan,
                flavor,
                false,
                active,
            ))
            .into();
            assert_eq!(
                visual.rendered_background(selector(prefix, clip)),
                Some(expected)
            );
            assert_eq!(
                visual.rendered_background(selector("clip-name", clip)),
                Some(expected)
            );
        }
    }
}
