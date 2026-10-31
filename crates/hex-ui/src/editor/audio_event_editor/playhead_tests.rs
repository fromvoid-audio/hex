//! Real worker publication and callback position, then completed-frame paint.
use super::*;
use crate::editor::{
    PlaybackCommand,
    clip_drag_ui_test::{pump_move, upper_rail_owned_fixture},
    device_panel::tests::eq_ui_tests::{graph_layout_tests, held_pointer_tests},
};
use gpui::{Entity, TestAppContext, VisualTestContext, px, size};
use hex_project::Command;
use std::{sync::atomic::Ordering::Acquire, time::Instant};

fn marker(
    visual: &mut VisualTestContext,
    selector: &'static str,
    body: &'static str,
    fraction: Option<f32>,
) {
    let line = visual.debug_bounds(selector);
    if let Some(fraction) = fraction {
        let line = line.expect("accepted audio playback indicator");
        let body = visual.debug_bounds(body).unwrap();
        assert!((line.left() - body.left() - body.size.width * fraction).abs() < px(1.0));
        assert_eq!(line.size.width, px(1.0));
        let border = if selector == "audio-source-playback-position" {
            px(2.0)
        } else {
            px(0.0)
        };
        assert_eq!(line.size.height, body.size.height - border);
        assert!(
            !visual
                .rendered_quad_paint_layers(selector, false)
                .is_empty()
        );
    } else {
        assert!(line.is_none(), "no fabricated playback position");
    }
}
fn pump_until(
    editor: &Entity<EditorView>,
    visual: &mut VisualTestContext,
    cx: &mut TestAppContext,
    output: &mut hex_audio::WorkerPreparedOutput,
    backend: &mut hex_audio::worker_fixture::HeadlessBackend,
    ready: impl Fn(&EditorView) -> bool,
) {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        held_pointer_tests::pump_output(editor, cx, output, backend);
        visual.run_until_parked();
        if editor.read_with(cx, |v, _| ready(v)) {
            editor.read_with(cx, |v, _| assert!(v.error.is_none(), "{:?}", v.error));
            return;
        }
        assert!(
            Instant::now() < deadline,
            "playback acknowledgement: {:?}",
            editor.read_with(cx, |v, _| v.error.clone())
        );
    }
}
#[gpui::test]
fn audio_playhead_tracks_accepted_arrangement_position_through_trim_repeat_and_viewport(
    cx: &mut TestAppContext,
) {
    for warped in [false, true] {
        audio_playhead_case(cx, warped);
    }
}
fn audio_playhead_case(cx: &mut TestAppContext, warped: bool) {
    let _guard = graph_layout_tests::status_fixture_guard();
    let (_files, handle, editor) = upper_rail_owned_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1280.0), px(800.0)));
    let clip = editor.update(&mut visual, |v, cx| {
        let track = &v.state.as_ref().unwrap().project.tracks()[0];
        let clip = track.clips()[0].id();
        v.selected_track = Some(track.id());
        v.selected_clip = Some(clip);
        v.clip_view_open = true;
        v.device_panel_active = false;
        cx.notify();
        clip
    });
    for command in [
        Command::TrimClip {
            clip,
            start: 96_000,
            duration: 60_000,
            source_offset: 12_000,
            source_length: 60_000,
        },
        Command::ResizeAudioClipView {
            clip,
            operation: hex_project::AudioViewResizeOperation::CycleRange {
                start: 12_000,
                end: 36_000,
                looping: true,
            },
        },
        Command::TrimClipEdge {
            clip,
            edge: hex_project::TrimEdge::Left,
            frame: 102_000,
        },
    ] {
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(command)))
        });
        pump_move(&editor, &mut visual);
    }
    if warped {
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::SetAudioClipWarp {
                clip,
                settings: Some(hex_project::WarpSettings {
                    mode: hex_project::WarpMode::RePitch,
                    markers: vec![
                        hex_project::WarpMarker {
                            source_frame: 12_000,
                            beat: hex_project::BeatTime::ZERO
                        },
                        hex_project::WarpMarker {
                            source_frame: 30_000,
                            beat: hex_project::BeatTime::from_ratio(1, 2).unwrap(),
                        },
                        hex_project::WarpMarker {
                            source_frame: 72_000,
                            beat: hex_project::BeatTime::from_ratio(5, 2).unwrap()
                        },
                    ],
                }),
            })))
        });
        pump_move(&editor, &mut visual);
    }
    visual.run_until_parked();
    marker(
        &mut visual,
        "audio-playback-position",
        "audio-events-waveform",
        None,
    );
    let (mut output, mut backend) =
        held_pointer_tests::attach_actual_output(&editor, cx, &mut visual);
    editor.update(cx, |v, _| {
        v.queue_audio_command(PlaybackCommand::PlayFrom(108_000))
    });
    // Intent alone cannot relocate the accepted line.
    marker(
        &mut visual,
        "audio-playback-position",
        "audio-events-waveform",
        None,
    );
    pump_until(&editor, &mut visual, cx, &mut output, &mut backend, |v| {
        v.audio_status()
            .is_some_and(|s| s.playing.load(Acquire) && s.position.load(Acquire) >= 108_000)
    });
    for _ in 0..3 {
        held_pointer_tests::pump_output(&editor, cx, &mut output, &mut backend);
        visual.run_until_parked();
        let frame = editor.read_with(cx, |v, _| v.audio_status().unwrap().position.load(Acquire));
        let root = 12_000 + (frame - 96_000) % 24_000;
        marker(
            &mut visual,
            "audio-playback-position",
            "audio-events-waveform",
            Some(root as f32 / 60_000.0),
        );
    }
    editor.update(cx, |v, _| v.queue_audio_command(PlaybackCommand::Pause));
    pump_until(&editor, &mut visual, cx, &mut output, &mut backend, |v| {
        !v.audio_status().unwrap().playing.load(Acquire)
    });
    editor.update(cx, |v, _| v.request_seek(126_000));
    pump_until(&editor, &mut visual, cx, &mut output, &mut backend, |v| {
        v.audio_status().unwrap().position.load(Acquire) == 126_000
            && v.pending_seek_ticket.is_none()
    });
    let before = editor.read_with(cx, |v, _| {
        (
            v.state.as_ref().unwrap().project.clone(),
            v.audio_event_editor.cursor,
            v.audio_event_editor.range,
        )
    });
    for (viewport, fraction) in [
        (None, Some(0.3)),
        (
            Some(Viewport {
                start: 12_000,
                length: 24_000,
            }),
            Some(0.25),
        ),
        (
            Some(Viewport {
                start: 36_000,
                length: 24_000,
            }),
            None,
        ),
        (None, Some(0.3)),
    ] {
        editor.update(cx, |v, cx| {
            v.audio_event_editor.viewport = viewport;
            cx.notify();
        });
        visual.run_until_parked();
        marker(
            &mut visual,
            "audio-playback-position",
            "audio-events-waveform",
            fraction,
        );
        editor.read_with(cx, |v, _| {
            assert_eq!(*v.state.as_ref().unwrap().project, *before.0);
            assert_eq!(
                (v.audio_event_editor.cursor, v.audio_event_editor.range),
                (before.1, before.2)
            );
        });
    }
    let track = editor.update(cx, |v, cx| {
        let track = v.selected_track;
        v.selected_clip = None;
        v.selected_track = None;
        cx.notify();
        track
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("audio-events-waveform").is_none());
    editor.update(cx, |v, cx| {
        v.selected_clip = Some(clip);
        v.selected_track = track;
        cx.notify();
    });
    // The source tab is in original-media coordinates, including its 12k offset.
    editor.update(cx, |v, cx| {
        v.audio_event_editor.source_view = true;
        cx.notify();
    });
    visual.run_until_parked();
    marker(
        &mut visual,
        "audio-source-playback-position",
        "arrangement-audio-waveform",
        Some(if warped {
            35_250.0 / 96_000.0
        } else {
            30_000.0 / 96_000.0
        }),
    );
    editor.update(cx, |v, cx| {
        v.audio_event_editor.source_view = false;
        cx.notify();
    });
    for fault in [0, 1, 2, 3, 4] {
        editor.update(cx, |v, cx| {
            let s = v.audio_status().unwrap();
            match fault {
                0 => s
                    .device_fault
                    .store(true, std::sync::atomic::Ordering::Release),
                1 => s
                    .render_fault
                    .store(true, std::sync::atomic::Ordering::Release),
                2 => s
                    .shutdown_complete
                    .store(true, std::sync::atomic::Ordering::Release),
                3 => {
                    v.project_generation += 1;
                }
                _ => {
                    v.audible_playback.as_mut().unwrap().audio_revision += 1;
                }
            }
            cx.notify();
        });
        visual.run_until_parked();
        marker(
            &mut visual,
            "audio-playback-position",
            "audio-events-waveform",
            None,
        );
        editor.update(cx, |v, cx| {
            let s = v.audio_status().unwrap();
            match fault {
                0 => s
                    .device_fault
                    .store(false, std::sync::atomic::Ordering::Release),
                1 => s
                    .render_fault
                    .store(false, std::sync::atomic::Ordering::Release),
                2 => s
                    .shutdown_complete
                    .store(false, std::sync::atomic::Ordering::Release),
                3 => {
                    v.project_generation -= 1;
                }
                _ => {
                    v.audible_playback.as_mut().unwrap().audio_revision -= 1;
                }
            }
            cx.notify();
        });
        visual.run_until_parked();
        marker(
            &mut visual,
            "audio-playback-position",
            "audio-events-waveform",
            Some(0.3),
        );
    }
    for frame in [90_000, 156_000] {
        editor.update(cx, |v, _| v.request_seek(frame));
        pump_until(&editor, &mut visual, cx, &mut output, &mut backend, |v| {
            v.audio_status().unwrap().position.load(Acquire) == frame
                && v.pending_seek_ticket.is_none()
        });
        marker(
            &mut visual,
            "audio-playback-position",
            "audio-events-waveform",
            None,
        );
    }
    editor.update(cx, |v, _| v.request_seek(126_000));
    pump_until(&editor, &mut visual, cx, &mut output, &mut backend, |v| {
        v.audio_status().unwrap().position.load(Acquire) == 126_000
            && v.pending_seek_ticket.is_none()
    });
    marker(
        &mut visual,
        "audio-playback-position",
        "audio-events-waveform",
        Some(0.3),
    );
    editor.update(cx, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::SetClipActive {
            clip,
            active: false
        })))
    });
    pump_move(&editor, &mut visual);
    marker(
        &mut visual,
        "audio-playback-position",
        "audio-events-waveform",
        None,
    );
    pump_until(&editor, &mut visual, cx, &mut output, &mut backend, |v| {
        v.applied_project_revision() == Some(v.state.as_ref().unwrap().revision)
    });
    marker(
        &mut visual,
        "audio-playback-position",
        "audio-events-waveform",
        None,
    );
    editor.update(cx, |v, _| assert!(v.enqueue(Request::Undo)));
    pump_until(&editor, &mut visual, cx, &mut output, &mut backend, |v| {
        v.worker_requests_pending == 0
            && v.pending_requests.is_empty()
            && v.applied_project_revision() == Some(v.state.as_ref().unwrap().revision)
    });
    marker(
        &mut visual,
        "audio-playback-position",
        "audio-events-waveform",
        Some(0.3),
    );
    editor.update(cx, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddScene {
            name: "playhead route".into()
        })))
    });
    pump_until(&editor, &mut visual, cx, &mut output, &mut backend, |v| {
        v.worker_requests_pending == 0
            && v.pending_requests.is_empty()
            && v.applied_project_revision() == Some(v.state.as_ref().unwrap().revision)
    });
    let scene = editor.read_with(cx, |v, _| {
        v.state.as_ref().unwrap().project.scenes()[0].id()
    });
    editor.update(cx, |v, _| {
        assert!(
            v.enqueue(Request::Edit(Command::CreateSessionAudioClip {
                track: track.unwrap(),
                scene,
                name: "route source".into(),
                asset: v
                    .state
                    .as_ref()
                    .unwrap()
                    .project
                    .clip(clip)
                    .unwrap()
                    .asset(),
                source_start: 12_000,
                source_end: 72_000,
            }))
        )
    });
    pump_until(&editor, &mut visual, cx, &mut output, &mut backend, |v| {
        v.worker_requests_pending == 0
            && v.pending_requests.is_empty()
            && v.applied_project_revision() == Some(v.state.as_ref().unwrap().revision)
    });
    let (track, session_clip) = editor.update(cx, |v, cx| {
        let track = track.unwrap();
        let hex_project::SessionCellContent::Audio(cell) = v
            .state
            .as_ref()
            .unwrap()
            .project
            .session_cell(track, scene)
            .unwrap()
        else {
            panic!("audio cell")
        };
        let id = cell.id();
        v.selected_clip = Some(clip);
        v.selected_track = Some(track);
        v.mode = crate::editor::EditorMode::Arrangement;
        cx.notify();
        (track, id)
    });
    visual.run_until_parked();
    marker(
        &mut visual,
        "audio-playback-position",
        "audio-events-waveform",
        Some(0.3),
    );
    editor.update(cx, |v, _| {
        v.queue_session_command(
            hex_engine::SessionPlaybackCommand::LaunchClip {
                clip: session_clip,
                quantization: hex_engine::LaunchQuantization::Immediate,
            },
            v.state.as_ref().unwrap().revision,
        );
    });
    pump_until(&editor, &mut visual, cx, &mut output, &mut backend, |v| {
        let mut entries = [hex_audio::prepared::SessionRouteEntry::default();
            hex_audio::prepared::SESSION_ROUTE_CAPACITY];
        v.audio_status()
            .unwrap()
            .session_routes(&mut entries)
            .is_some_and(|snapshot| {
                entries[..snapshot.count].iter().any(|e| {
                    e.track_id == track.value()
                        && e.owner
                            == hex_audio::prepared::SessionRouteOwner::Session {
                                clip_id: session_clip.value(),
                            }
                })
            })
    });
    marker(
        &mut visual,
        "audio-playback-position",
        "audio-events-waveform",
        None,
    );
    // The Console cell editor follows that accepted launch on the performance
    // clock: the route publishes its launch frame with the owner it accepted.
    let (launch, performance) = editor.update(cx, |v, cx| {
        v.mode = crate::editor::EditorMode::Session;
        v.selected_cell = Some((track, scene));
        cx.notify();
        let status = v.audio_status().unwrap();
        let mut entries = [hex_audio::prepared::SessionRouteEntry::default();
            hex_audio::prepared::SESSION_ROUTE_CAPACITY];
        let snapshot = status.session_routes(&mut entries).unwrap();
        let entry = entries[..snapshot.count]
            .iter()
            .find(|e| e.track_id == track.value())
            .unwrap();
        (
            entry
                .audio_launch_frame
                .expect("published Session audio launch"),
            status.performance_frame.load(Acquire),
        )
    });
    visual.run_until_parked();
    let elapsed = performance.checked_sub(launch).unwrap();
    assert!(elapsed < 60_000, "one-shot cell still audible");
    marker(
        &mut visual,
        "session-audio-playback-position",
        "session-audio-waveform",
        Some(elapsed as f32 / 60_000.0),
    );
    editor.update(cx, |v, _| {
        v.audio.take();
    });
    backend.close();
    drop(output);
}
