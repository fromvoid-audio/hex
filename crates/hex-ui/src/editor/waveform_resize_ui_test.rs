//! Actual trim input must keep drawing the accepted sample's nonzero stereo peaks.
use crate::editor::{clip_drag_ui_test::editor_window, musical_time::SnapGrid};
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
use hex_project::{Command, Created, Editor, Project, TrimEdge, WarpMode, default_audio_warp};
use hex_session::{
    CachedPeak, CachedWaveform,
    service::{AudioClipWaveform, ViewState},
};
use std::{collections::BTreeMap, sync::Arc};

#[gpui::test]
fn trim_preview_requires_matching_accepted_processing_root(_cx: &mut TestAppContext) {
    let (project, _, _, clip) = crate::editor::clip_drag_ui_test::audio_project();
    let mut canonical = Editor::new(project, 8).unwrap();
    canonical
        .execute(Command::SetAudioClipPitch { clip, cents: 137 })
        .unwrap();
    let original = canonical.project().clip(clip).unwrap().clone();
    let preview = original
        .preview_edge_trim_with_tempo(
            TrimEdge::Left,
            4_103,
            96_000,
            canonical.project().tempo_map(),
        )
        .unwrap();
    let peaks = Arc::new(
        CachedWaveform::from_peaks(
            96_000,
            vec![
                [CachedPeak {
                    min: -0.5,
                    max: 0.5
                }; 2];
                64
            ],
        )
        .unwrap(),
    );
    let prepared = AudioClipWaveform {
        waveform: Arc::clone(&peaks),
        offset: 0,
        length: 96_000,
        preview_root: None,
    };
    let tempo = canonical.project().tempo_map().clone();
    let request = super::WaveformPreview {
        original: &original,
        preview: &preview,
        tempo: &tempo,
        asset: Some(&peaks),
        prepared: Some(&prepared),
        ink: crate::theme::color(crate::theme::PaletteRole::Waveform),
    };
    let check = |request| {
        super::preview_waveform_view_cached(request, 4_103, 96_000, Default::default(), None)
    };
    assert!(
        check(request).is_some(),
        "reuse accepted full-root peaks for a valid crop"
    );
    assert!(
        check(super::WaveformPreview {
            prepared: None,
            ..request
        })
        .is_none(),
        "missing prepared peaks cannot fall back to unrelated raw peaks"
    );
    let stale = AudioClipWaveform {
        offset: 1,
        ..prepared.clone()
    };
    assert!(
        check(super::WaveformPreview {
            prepared: Some(&stale),
            ..request
        })
        .is_none()
    );
    let short_root = AudioClipWaveform {
        waveform: Arc::new(
            CachedWaveform::from_peaks(
                95_999,
                vec![
                    [CachedPeak {
                        min: -0.5,
                        max: 0.5
                    }; 2];
                    64
                ],
            )
            .unwrap(),
        ),
        ..prepared.clone()
    };
    assert!(
        check(super::WaveformPreview {
            prepared: Some(&short_root),
            ..request
        })
        .is_none()
    );
    canonical
        .execute(Command::SetAudioClipPitch { clip, cents: 200 })
        .unwrap();
    let changed = canonical.project().clip(clip).unwrap();
    assert!(
        check(super::WaveformPreview {
            preview: changed,
            ..request
        })
        .is_none(),
        "another processing root cannot reuse accepted crop peaks"
    );
}

#[gpui::test]
fn trim_keeps_sample_varied_waveforms_visible_with_warp_on_and_off(cx: &mut TestAppContext) {
    resize_waveforms(cx, false);
}

#[gpui::test]
fn shift_stretch_keeps_live_sample_varied_waveforms_visible(cx: &mut TestAppContext) {
    resize_waveforms(cx, true);
}

fn resize_waveforms(cx: &mut TestAppContext, stretch: bool) {
    cx.update(crate::editor::bind_editor_keys);
    let modifiers = Modifiers {
        shift: stretch,
        ..Modifiers::default()
    };
    let mut failures = Vec::new();
    for (rate, frames, pattern) in [(48_000, 96_000, 0), (44_100, 88_200, 1)] {
        for (processing, looping) in [
            ("plain", false),
            ("pitch with warp off", false),
            ("beats warp", false),
            ("beats warp", true),
        ] {
            for edge in [TrimEdge::Left, TrimEdge::Right] {
                let mut canonical = Editor::new(Project::new(rate).unwrap(), 16).unwrap();
                let Created::Imported { track, asset, clip } = canonical
                    .execute(Command::ImportAudioTrack {
                        name: "sample".into(),
                        path: "waveform-regression.wav".into(),
                        sample_rate: rate,
                        frames,
                    })
                    .unwrap()
                else {
                    panic!("sample import");
                };
                if looping {
                    canonical
                        .execute(Command::SetClipLooping {
                            clip,
                            looping: true,
                        })
                        .unwrap();
                    canonical
                        .execute(Command::SetClipSourcePhase { clip, phase: 701 })
                        .unwrap();
                }
                if processing == "pitch with warp off" {
                    canonical
                        .execute(Command::SetAudioClipPitch { clip, cents: 137 })
                        .unwrap();
                } else if processing == "beats warp" {
                    let settings = default_audio_warp(
                        canonical.project().clip(clip).unwrap(),
                        canonical.project().tempo_map(),
                        WarpMode::Beats {
                            preserve: hex_project::BeatsPreserve::Transients,
                            tail: hex_project::BeatsTail::Forward,
                            decay: 100,
                        },
                    )
                    .unwrap();
                    canonical
                        .execute(Command::SetAudioClipWarp {
                            clip,
                            settings: Some(settings),
                        })
                        .unwrap();
                }
                // A partial source/crop, not just an edge-aligned complete asset.
                canonical
                    .execute(Command::TrimClipEdge {
                        clip,
                        edge: TrimEdge::Left,
                        frame: 4_103,
                    })
                    .unwrap();
                let original = canonical.project().clone();
                // Distinct bipolar and sparse stereo sample summaries, with silence
                // between impulses. These are accepted cache fixtures, not generated
                // preview peaks; the renderer must keep reading the same Arc.
                let peaks = Arc::new(
                    CachedWaveform::from_peaks(
                        frames as usize,
                        (0..127)
                            .map(|bin| {
                                let a = if pattern == 0 {
                                    0.2 + (bin % 11) as f32 / 20.0
                                } else if bin % 5 == 0 {
                                    0.8
                                } else {
                                    0.0
                                };
                                [
                                    CachedPeak {
                                        min: -a,
                                        max: a * 0.7,
                                    },
                                    CachedPeak {
                                        min: -a * 0.6,
                                        max: a * 0.9,
                                    },
                                ]
                            })
                            .collect(),
                    )
                    .unwrap(),
                );
                let prepared_peaks = Arc::new(
                    CachedWaveform::from_peaks(
                        frames as usize,
                        (0..127)
                            .map(|bin| {
                                let a = 0.1 + (bin % 7) as f32 / 30.0;
                                [
                                    CachedPeak {
                                        min: -a * 0.7,
                                        max: a,
                                    },
                                    CachedPeak {
                                        min: -a,
                                        max: a * 0.8,
                                    },
                                ]
                            })
                            .collect(),
                    )
                    .unwrap(),
                );
                let expected_source = if processing == "plain" {
                    &peaks
                } else {
                    &prepared_peaks
                };
                let mut prepared = BTreeMap::new();
                if let Some(edit) = original.clip(clip).unwrap().audio_edit() {
                    let (_, _, offset, _) = edit.projected_domain(original.tempo_map()).unwrap();
                    prepared.insert(
                        clip,
                        AudioClipWaveform {
                            waveform: Arc::clone(&prepared_peaks),
                            offset,
                            length: original.clip(clip).unwrap().duration(),
                            preview_root: None,
                        },
                    );
                }
                let (handle, editor) = editor_window(cx, original.clone(), Arc::default());
                editor.update(cx, |view, cx| {
                    let old = view.state.as_ref().unwrap();
                    view.state = Some(Arc::new(ViewState {
                        canonical_audio_source: None,
                        comparison_audio: None,
                        project: Arc::clone(&old.project),
                        waveforms: BTreeMap::from([(asset, Arc::clone(&peaks))]),
                        source_overviews: Default::default(),
                        audio_clip_waveforms: prepared,
                        midi_preview: Arc::clone(&old.midi_preview),
                        gain_preview: old.gain_preview.clone(),
                        timeline_frames: old.timeline_frames,
                        can_undo: old.can_undo,
                        can_redo: old.can_redo,
                        revision: old.revision,
                        unsaved: old.unsaved,
                        document_path: old.document_path.clone(),
                    }));
                    view.snap_grid = SnapGrid::Off;
                    cx.notify();
                });
                let mut visual = VisualTestContext::from_window(handle, cx);
                visual.run_until_parked();
                let selector: &'static str = Box::leak(
                    format!(
                        "clip-trim-{}-{}",
                        if edge == TrimEdge::Left {
                            "left"
                        } else {
                            "right"
                        },
                        clip.value()
                    )
                    .into_boxed_str(),
                );
                let down = visual.debug_bounds(selector).unwrap().center();
                let direction = if edge == TrimEdge::Left { 1.0 } else { -1.0 };
                visual.simulate_mouse_down(down, MouseButton::Left, modifiers);
                visual.simulate_mouse_move(
                    point(down.x + px(12.0 * direction), down.y),
                    MouseButton::Left,
                    modifiers,
                );
                visual.run_until_parked();
                for distance in [18.0, 24.0, 20.0] {
                    visual.simulate_mouse_move(
                        point(down.x + px(distance * direction), down.y),
                        MouseButton::Left,
                        modifiers,
                    );
                    visual.run_until_parked();
                    assert!(visual.debug_bounds("trim-live-preview").is_some());
                    editor.update(&mut visual, |view, cx| {
                        view.waveform_geometry_cache.borrow_mut().clear();
                        cx.notify();
                    });
                    visual.run_until_parked();
                    editor.read_with(&visual, |view, _| {
                        let drag = view.active_trim_drag.as_ref().expect("actual active trim");
                        assert_eq!(drag.source_track, track);
                        let rendered = view.waveform_geometry_cache.borrow().rendered_waveforms();
                        assert_eq!(rendered.len(), 1, "only the actual preview waveform is painted");
                        let preview = drag.preview.borrow();
                        let preview = &preview.as_ref().unwrap().clip;
                        // A repeating clip's right edge drags its outer repeat
                        // (Pulse #3778); repeats paint through the occurrence
                        // path, not a single prepared crop mapping.
                        let outer_repeat = looping && edge == TrimEdge::Right && !stretch;
                        let expected_mapping = preview
                            .audio_edit()
                            .filter(|_| !stretch && !outer_repeat)
                            .map(|edit| {
                                let (_, _, offset, _) =
                                    edit.projected_domain(original.tempo_map()).unwrap();
                                (preview.start(), offset)
                            });
                        let root_matches = Arc::ptr_eq(&rendered[0].source, expected_source);
                        if !rendered[0].spans().iter().all(|span| *span > 1.0)
                            || !root_matches || rendered[0].prepared != expected_mapping
                        {
                            failures.push(format!("{rate}/{pattern}/{processing}/loop={looping}/{edge:?}/{distance}: spans={:?}, accepted_root={root_matches}, mapping={:?}, expected={expected_mapping:?}", rendered[0].spans(), rendered[0].prepared));
                        }
                        assert!(view.last_enqueued_edit.is_none());
                        assert_eq!(view.state.as_ref().unwrap().project.as_ref(), &original);
                    });
                }
                visual.simulate_keystrokes("escape");
                visual.simulate_mouse_up(
                    point(down.x + px(20.0 * direction), down.y),
                    MouseButton::Left,
                    modifiers,
                );
                visual.run_until_parked();
                editor.update(&mut visual, |view, cx| {
                    // Rebuild the ordinary retained clip scene as well: its
                    // previous painted paths legitimately bypass geometry-cache
                    // preparation, which this probe observes.
                    view.clip_content_cache = Default::default();
                    view.waveform_geometry_cache.borrow_mut().clear();
                    cx.notify();
                });
                visual.run_until_parked();
                editor.read_with(&visual, |view, _| {
                    assert!(view.last_enqueued_edit.is_none());
                    assert!(view.active_trim_drag.is_none());
                    let rendered = view.waveform_geometry_cache.borrow().rendered_waveforms();
                    assert_eq!(rendered.len(), 1);
                    assert!(
                        rendered[0].spans().iter().all(|span| *span > 1.0),
                        "waveform must remain visible after the drag ends"
                    );
                    assert!(Arc::ptr_eq(&rendered[0].source, expected_source));
                });
            }
        }
    }
    assert!(
        failures.is_empty(),
        "waveform disappeared or used the wrong root/crop:\n{}",
        failures.join("\n")
    );
}

#[gpui::test]
fn silent_left_prefix_keeps_waveform_and_geometry_through_real_worker_ack(cx: &mut TestAppContext) {
    check_silent_left_prefix_through_ack(cx, &[(false, false), (true, false), (true, true)], false);
}

#[gpui::test]
fn musical_left_prefix_keeps_waveform_and_geometry_through_real_worker_ack(
    cx: &mut TestAppContext,
) {
    check_silent_left_prefix_through_ack(cx, &[(true, false)], true);
}

fn check_silent_left_prefix_through_ack(
    cx: &mut TestAppContext,
    cases: &[(bool, bool)],
    musical: bool,
) {
    use crate::editor::clip_drag_ui_test::{pump_move, trim_grab_point, upper_rail_owned_fixture};
    use hex_session::service::Request;
    for &(processed, once) in cases {
        let (_files, handle, editor) = upper_rail_owned_fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        let (track, clip) = editor.read_with(&visual, |v, _| {
            let track = &v.state.as_ref().unwrap().project.tracks()[0];
            (track.id(), track.clips()[0].id())
        });
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::MoveClip {
                clip,
                track,
                start: 48_000
            })))
        });
        pump_move(&editor, &mut visual);
        if processed {
            editor.update(&mut visual, |v, _| {
                assert!(v.enqueue(Request::Edit(Command::SetAudioClipSourceGain {
                    clip,
                    points: vec![hex_project::SourceGainPoint {
                        frame: 0,
                        gain_q16: hex_project::SOURCE_GAIN_UNITY
                    }]
                })))
            });
            pump_move(&editor, &mut visual);
        }
        if processed {
            editor.update(&mut visual, |v, _| {
                assert!(v.enqueue(Request::Edit(Command::SetAudioClipPitch {
                    clip,
                    cents: 137
                })))
            });
            pump_move(&editor, &mut visual);
        }
        if musical {
            editor.update(&mut visual, |v, _| {
                assert!(v.enqueue(Request::Edit(Command::SetAudioClipMusical {
                    clip,
                    enabled: true
                })));
            });
            pump_move(&editor, &mut visual);
        }
        if once {
            editor.update(&mut visual, |v, _| {
                assert!(v.enqueue(Request::Edit(Command::ResizeAudioClipView {
                    clip,
                    operation: hex_project::AudioViewResizeOperation::CycleRange {
                        start: 0,
                        end: 72_000,
                        looping: false,
                    },
                })))
            });
            pump_move(&editor, &mut visual);
        }
        let (before, source) = editor.update(&mut visual, |v, cx| {
            v.viewport = Some(crate::editor::viewport::Viewport {
                start: 0,
                length: 192_000,
            });
            v.snap_grid = SnapGrid::Off;
            let state = v.state.as_ref().unwrap();
            let source = if once {
                Arc::clone(
                    &state.audio_clip_waveforms[&clip]
                        .preview_root
                        .as_ref()
                        .unwrap()
                        .waveform,
                )
            } else if processed {
                Arc::clone(&state.audio_clip_waveforms[&clip].waveform)
            } else {
                Arc::clone(&state.waveforms[&state.project.clip(clip).unwrap().asset()])
            };
            cx.notify();
            (state.project.snapshot(), source)
        });
        visual.run_until_parked();
        let selector: &'static str =
            Box::leak(format!("clip-trim-left-{}", clip.value()).into_boxed_str());
        let down = trim_grab_point(visual.debug_bounds(selector).unwrap());
        let end = down - point(px(40.0), px(0.0));
        // Escape after moving into the new prefix cannot submit a partial edit.
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        visual.simulate_keystrokes("escape");
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
        });
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let preview = editor.read_with(&visual, |v, _| {
            v.active_trim_drag
                .as_ref()
                .unwrap()
                .preview
                .borrow()
                .as_ref()
                .unwrap()
                .clip
                .clone()
        });
        assert!(preview.start() < 48_000);
        assert_eq!(preview.start() + preview.duration(), 144_000);
        let held = visual.debug_bounds("trim-live-preview").unwrap();
        for released in [false, true] {
            if released {
                visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
            }
            for _ in 0..3 {
                editor.update(&mut visual, |v, cx| {
                    v.clip_content_cache = Default::default();
                    v.waveform_geometry_cache.borrow_mut().clear();
                    cx.notify();
                });
                visual.run_until_parked();
                assert_eq!(visual.debug_bounds("trim-live-preview"), Some(held));
                editor.read_with(&visual, |v, _| {
                    assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
                    let waveforms = v.waveform_geometry_cache.borrow().rendered_waveforms();
                    assert_eq!(
                        waveforms.len(),
                        1,
                        "prefix preview must retain the accepted waveform"
                    );
                    assert!(Arc::ptr_eq(&waveforms[0].source, &source));
                    assert!(
                        waveforms[0].spans().iter().all(|s| *s > 1.0),
                        "nonzero audio must stay visible after adding leading silence"
                    );
                    if processed && !once && !musical {
                        assert_eq!(waveforms[0].prepared, Some((48_000, 0)));
                    }
                });
            }
        }
        pump_move(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert!(v.pending_trim.is_none());
            assert_eq!(v.state.as_ref().unwrap().project.clip(clip), Some(&preview));
        });
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
        pump_move(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
        });
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
        pump_move(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.clip(clip), Some(&preview))
        });
    }
}

#[gpui::test]
fn prefix_waveform_accepts_only_the_canonical_source_and_matching_prepared_extent(
    _cx: &mut TestAppContext,
) {
    let (project, track, _, clip) = crate::editor::clip_drag_ui_test::audio_project();
    let mut canonical = Editor::new(project, 16).unwrap();
    canonical
        .execute(Command::MoveClip {
            clip,
            track,
            start: 48_000,
        })
        .unwrap();
    canonical
        .execute(Command::SetAudioClipSourceGain {
            clip,
            points: vec![hex_project::SourceGainPoint {
                frame: 0,
                gain_q16: 2 * hex_project::SOURCE_GAIN_UNITY,
            }],
        })
        .unwrap();
    let original = canonical.project().clip(clip).unwrap().clone();
    let tempo = canonical.project().tempo_map();
    let preview = original
        .preview_edge_trim_with_tempo(TrimEdge::Left, 24_000, 96_000, tempo)
        .unwrap();
    let peaks = Arc::new(
        CachedWaveform::from_peaks(
            96_000,
            vec![
                [CachedPeak {
                    min: -0.5,
                    max: 0.5
                }; 2];
                64
            ],
        )
        .unwrap(),
    );
    let accepted = AudioClipWaveform {
        waveform: Arc::clone(&peaks),
        offset: 0,
        length: 96_000,
        preview_root: None,
    };
    let request = super::WaveformPreview {
        original: &original,
        preview: &preview,
        tempo,
        asset: None,
        prepared: Some(&accepted),
        ink: crate::theme::color(crate::theme::PaletteRole::Waveform),
    };
    let check =
        |request| super::prefix_waveform_view(request, 24_000, 144_000, Default::default(), None);
    assert!(check(request).is_some());
    assert!(
        check(super::WaveformPreview {
            prepared: None,
            ..request
        })
        .is_none()
    );
    let stale = AudioClipWaveform {
        offset: 1,
        ..accepted.clone()
    };
    assert!(
        check(super::WaveformPreview {
            prepared: Some(&stale),
            ..request
        })
        .is_none()
    );
    let stale = AudioClipWaveform {
        length: 95_999,
        ..accepted.clone()
    };
    assert!(
        check(super::WaveformPreview {
            prepared: Some(&stale),
            ..request
        })
        .is_none()
    );
    let foreign = AudioClipWaveform {
        preview_root: Some(hex_session::service::AudioPreviewWaveform {
            waveform: Arc::clone(&peaks),
            origin: 0,
        }),
        ..accepted.clone()
    };
    assert!(
        check(super::WaveformPreview {
            prepared: Some(&foreign),
            ..request
        })
        .is_none()
    );
    let mut snapshot = preview.retained_snapshot();
    Arc::make_mut(&mut snapshot.edit.as_mut().unwrap().domain).source_gain[0].gain_q16 =
        3 * hex_project::SOURCE_GAIN_UNITY;
    let wrong = canonical
        .project()
        .restore_retained_audio_root(1, snapshot, Arc::new(tempo.clone()))
        .unwrap()
        .clip;
    assert!(
        check(super::WaveformPreview {
            preview: &wrong,
            ..request
        })
        .is_none(),
        "an arbitrary changed root cannot borrow accepted peaks"
    );
}

#[gpui::test]
fn plain_lower_fade_prefix_uses_the_same_raw_source_mapping(_cx: &mut TestAppContext) {
    let (project, track, _, clip) = crate::editor::clip_drag_ui_test::audio_project();
    let mut canonical = Editor::new(project, 8).unwrap();
    canonical
        .execute(Command::MoveClip {
            clip,
            track,
            start: 48_000,
        })
        .unwrap();
    canonical
        .execute(Command::SetClipFades {
            clip,
            fade_in: 137,
            fade_out: 211,
        })
        .unwrap();
    let original = canonical.project().clip(clip).unwrap();
    let tempo = canonical.project().tempo_map();
    let preview = original
        .preview_fade_edge_trim(TrimEdge::Left, 24_000, 96_000, tempo)
        .unwrap();
    let peaks = Arc::new(
        CachedWaveform::from_peaks(
            96_000,
            vec![
                [CachedPeak {
                    min: -0.5,
                    max: 0.5
                }; 2];
                64
            ],
        )
        .unwrap(),
    );
    assert!(
        super::prefix_waveform_view(
            super::WaveformPreview {
                original,
                preview: &preview,
                tempo,
                asset: Some(&peaks),
                prepared: None,
                ink: crate::theme::color(crate::theme::PaletteRole::Waveform),
            },
            24_000,
            144_000,
            Default::default(),
            None
        )
        .is_some()
    );
}
