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
fn trim_keeps_sample_varied_waveforms_visible_with_warp_on_and_off(cx: &mut TestAppContext) {
    for (rate, frames, pattern) in [(48_000, 96_000, 0), (44_100, 88_200, 1)] {
        for processing in ["plain", "pitch with warp off", "beats warp"] {
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
                let mut prepared = BTreeMap::new();
                if let Some(edit) = original.clip(clip).unwrap().audio_edit() {
                    let (_, _, offset, _) = edit.projected_domain(original.tempo_map()).unwrap();
                    prepared.insert(
                        clip,
                        AudioClipWaveform {
                            waveform: Arc::clone(&peaks),
                            offset,
                            length: original.clip(clip).unwrap().duration(),
                        },
                    );
                }
                let (handle, editor) = editor_window(cx, original.clone(), Arc::default());
                editor.update(cx, |view, cx| {
                    let old = view.state.as_ref().unwrap();
                    view.state = Some(Arc::new(ViewState {
                        canonical_audio_source: None,
                        project: Arc::clone(&old.project),
                        waveforms: BTreeMap::from([(asset, Arc::clone(&peaks))]),
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
                visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
                visual.simulate_mouse_move(
                    point(down.x + px(12.0 * direction), down.y),
                    MouseButton::Left,
                    Modifiers::default(),
                );
                visual.run_until_parked();
                for distance in [18.0, 24.0, 20.0] {
                    visual.simulate_mouse_move(
                        point(down.x + px(distance * direction), down.y),
                        MouseButton::Left,
                        Modifiers::default(),
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
                        let rendered = view.waveform_geometry_cache.borrow().rendered_signal_spans();
                        assert_eq!(rendered.len(), 1, "only the actual preview waveform is painted");
                        assert!(rendered[0].iter().all(|span| *span > 1.0), "{rate}/{pattern}/{processing}/{edge:?}: accepted nonzero stereo waveform disappeared: {rendered:?}");
                        assert!(view.last_enqueued_edit.is_none());
                        assert_eq!(view.state.as_ref().unwrap().project.as_ref(), &original);
                    });
                }
                visual.simulate_keystrokes("escape");
                visual.simulate_mouse_up(
                    point(down.x + px(20.0 * direction), down.y),
                    MouseButton::Left,
                    Modifiers::default(),
                );
                visual.run_until_parked();
                editor.read_with(&visual, |view, _| {
                    assert!(view.last_enqueued_edit.is_none())
                });
            }
        }
    }
}
