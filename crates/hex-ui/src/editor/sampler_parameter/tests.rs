use super::*;
use gpui::{TestAppContext, VisualTestContext};
use hex_project::{Command, Created, Editor, Project};

pub(in crate::editor) fn sampler_text(view: &EditorView, cx: &gpui::App) -> String {
    view.sampler_parameter
        .controls
        .get(&SamplerParameter::AttackMs)
        .unwrap()
        .entity
        .read(cx)
        .display_text()
        .to_owned()
}

fn numeric_sampler_fixture(
    cx: &mut TestAppContext,
) -> (
    VisualTestContext,
    gpui::Entity<EditorView>,
    TrackId,
    hex_project::DeviceId,
) {
    cx.update(super::super::bind_editor_keys);
    let mut authored = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(track) = authored
        .execute(Command::AddMidiTrack {
            name: "sampler".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Asset(asset) = authored
        .execute(Command::AddAsset {
            path: "original.wav".into(),
            sample_rate: 48_000,
            frames: 4,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let state =
        hex_engine::native_device_resources::current_sampler_fixture::one_shot_state(4, asset)
            .unwrap();
    let Created::Device(device) = authored
        .execute(Command::InsertInstrument { track, state })
        .unwrap()
    else {
        panic!("device")
    };
    let Created::Clip(clip) = authored
        .execute(Command::PlaceMidiClip {
            track,
            start: hex_project::BeatTime::ZERO,
            length: hex_project::BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    let (window, editor) = super::super::clip_drag_ui_test::editor_window(
        cx,
        authored.project().clone(),
        Arc::default(),
    );
    let mut visual = VisualTestContext::from_window(window, cx);
    editor.update(&mut visual, |view, cx| {
        view.selected_track = Some(track);
        view.selected_clip = Some(clip);
        view.device_panel.owner = Some(device_panel::DevicePanelOwner::Track(track));
        view.device_panel_active = true;
        view.clip_view_open = true;
        cx.notify();
    });
    visual.run_until_parked();
    (visual, editor, track, device)
}

#[gpui::test]
fn unrelated_edit_preserves_sampler_numeric_text_and_valid_commit(cx: &mut TestAppContext) {
    let (mut visual, editor, track, device) = numeric_sampler_fixture(cx);
    let selector = Box::leak(format!("sampler-attack-{}-value", device.value()).into_boxed_str());
    let position = visual.debug_bounds(selector).unwrap().center();
    visual.simulate_click(position, gpui::Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("invalid");
    editor.update(&mut visual, |view, cx| {
        let control = view
            .sampler_parameter
            .controls
            .get(&SamplerParameter::AttackMs)
            .unwrap();
        assert!(control.edit_state.is_editing());
        assert_eq!(control.entity.read(cx).display_text(), "invalid");
        assert!(!view.enqueue(Request::Edit(Command::Mute { track, muted: true })));
        let generation = view.project_generation;
        let revision = view.state.as_ref().unwrap().revision;
        assert!(!view.enqueue(Request::GainLane {
            project_generation: generation,
            revision,
            track,
            edit: hex_session::service::GainLaneEdit::Clear,
        }));
        assert!(!view.enqueue(Request::ImportToTrack {
            path: "other.wav".into(),
            track,
            start: 0,
        }));
        let asset = view
            .state
            .as_ref()
            .unwrap()
            .project
            .assets()
            .next()
            .unwrap()
            .id();
        assert!(!view.enqueue(Request::RelinkMedia([(asset, "other.wav".into())].into(),)));
        assert!(!view.enqueue(Request::EditArrangementSection {
            project_generation: generation,
            revision,
            command: Command::AddArrangementSection {
                name: "section".into(),
                color: hex_project::ElementColor::Cyan,
                start: hex_project::BeatTime::ZERO,
                end: hex_project::BeatTime::ONE,
            },
        }));
        view.duplicate_selected();
        assert!(!view.duplicate_intent.is_pending());
        let control = view
            .sampler_parameter
            .controls
            .get(&SamplerParameter::AttackMs)
            .unwrap();
        assert_eq!(control.entity.read(cx).display_text(), "invalid");
        assert_eq!(view.worker_requests_pending, 0);
    });
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("100");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    editor.update(&mut visual, |view, _| {
        assert!(view.sampler_parameter.pending.is_some());
        assert_eq!(view.worker_requests_pending, 1);
    });
}

#[gpui::test]
fn clipboard_edits_refuse_while_copy_and_sampler_text_remain_available(cx: &mut TestAppContext) {
    let (mut visual, editor, track, device) = numeric_sampler_fixture(cx);
    let selector = Box::leak(format!("sampler-attack-{}-value", device.value()).into_boxed_str());
    let position = visual.debug_bounds(selector).unwrap().center();
    visual.simulate_click(position, gpui::Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("invalid");
    editor.update(&mut visual, |view, cx| {
        let generation = view.project_generation;
        let revision = view.state.as_ref().unwrap().revision;
        let range = hex_project::TimeRange::new(vec![track], 0, 1_024).unwrap();
        for request in [
            Request::CutTimeRange(range.clone()),
            Request::TrackClipboard {
                project_generation: generation,
                revision,
                action: hex_session::service::TrackClipboardAction::Cut(
                    hex_project::TrackSelectionIds::new(&[track]).unwrap(),
                ),
            },
            Request::DeviceClipboard {
                project_generation: generation,
                revision,
                action: hex_session::service::DeviceClipboardAction::Cut {
                    owner: hex_project::DeviceChainOwner::Track(track),
                    selected: hex_project::DeviceSelectionIds::new(&[device]).unwrap(),
                },
            },
        ] {
            assert!(!view.enqueue(request));
        }
        assert_eq!(view.worker_requests_pending, 0);
        assert!(view.enqueue(Request::CopyTimeRange(range)));
        assert_eq!(view.worker_requests_pending, 1);
        assert_eq!(sampler_text(view, cx), "invalid");
    });
}

#[gpui::test]
fn unrelated_edit_refuses_during_sampler_drag_and_release_commits(cx: &mut TestAppContext) {
    let (mut visual, editor, track, device) = numeric_sampler_fixture(cx);
    let selector = Box::leak(format!("sampler-attack-{}-value", device.value()).into_boxed_str());
    let position = visual.debug_bounds(selector).unwrap().center();
    visual.simulate_mouse_down(
        position,
        gpui::MouseButton::Left,
        gpui::Modifiers::default(),
    );
    let moved = position - gpui::point(gpui::px(0.0), gpui::px(12.0));
    visual.simulate_mouse_move(
        moved,
        Some(gpui::MouseButton::Left),
        gpui::Modifiers::default(),
    );
    editor.update(&mut visual, |view, _| {
        assert!(
            view.sampler_parameter
                .controls
                .get(&SamplerParameter::AttackMs)
                .unwrap()
                .gesture
                .is_some()
        );
        assert!(!view.enqueue(Request::Edit(Command::Mute { track, muted: true })));
        assert_eq!(view.worker_requests_pending, 0);
    });
    visual.simulate_mouse_up(moved, gpui::MouseButton::Left, gpui::Modifiers::default());
    visual.run_until_parked();
    editor.update(&mut visual, |view, _| {
        assert!(view.sampler_parameter.pending.is_some());
        assert_eq!(view.worker_requests_pending, 1);
    });
}

#[gpui::test]
fn source_hints_and_paint_require_same_view_source_facts_and_context(cx: &mut TestAppContext) {
    let mut authored = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(track) = authored
        .execute(Command::AddMidiTrack {
            name: "sampler".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Asset(asset) = authored
        .execute(Command::AddAsset {
            path: "original.wav".into(),
            sample_rate: 48_000,
            frames: 4,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let Created::Asset(other_asset) = authored
        .execute(Command::AddAsset {
            path: "other.wav".into(),
            sample_rate: 48_000,
            frames: 4,
        })
        .unwrap()
    else {
        panic!("other asset")
    };
    let state =
        hex_engine::native_device_resources::current_sampler_fixture::one_shot_state(4, asset)
            .unwrap();
    let Created::Device(device) = authored
        .execute(Command::InsertInstrument { track, state })
        .unwrap()
    else {
        panic!("device")
    };
    let Created::Device(neighbor) = authored
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: hex_engine::default_native_device_state(
                hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
            )
            .unwrap(),
        })
        .unwrap()
    else {
        panic!("neighbor")
    };
    let original = authored.project().clone();
    let facts = hex_project::OriginalSourceFacts::new(1, [7; 32]).unwrap();
    let wrong = hex_project::OriginalSourceFacts::new(1, [8; 32]).unwrap();
    let (window, editor) =
        super::super::clip_drag_ui_test::editor_window(cx, original.clone(), Arc::default());
    let mut visual = VisualTestContext::from_window(window, cx);
    let selector = Box::leak(format!("sampler-start-{}-value", device.value()).into_boxed_str());
    for case in 0..9 {
        let valid = case <= 1;
        let mut snapshot = original.snapshot();
        if case != 0 {
            snapshot.assets[0].original_source = Some(if case == 2 { wrong } else { facts });
        }
        let mut project = Project::from_snapshot(snapshot).unwrap();
        if case == 0 {
            let mut state =
                hex_engine::native_device_resources::current_sampler_fixture::classic_loop_state(
                    4, asset, 0.001,
                )
                .unwrap();
            for (parameter, value) in [
                (SamplerParameter::LoopStartFrame, 1.0),
                (SamplerParameter::LoopEndFrame, 3.0),
                (SamplerParameter::StartFrame, 1.0),
                (SamplerParameter::EndFrame, 3.0),
            ] {
                state = hex_engine::patch_sampler_parameter_on_control(parameter, &state, value)
                    .unwrap();
            }
            let mut editor = Editor::new(project, 16).unwrap();
            editor
                .execute(Command::ReplaceDeviceState { device, state })
                .unwrap();
            project = editor.project().clone();
        }
        let pcm: &[[f32; 2]] = if case == 4 {
            &[[0.5, -0.25]; 3]
        } else {
            &[[0.5, -0.25]; 4]
        };
        let fixture = hex_session::source_overview_test_support::Fixture::new(
            if case == 5 { other_asset } else { asset },
            pcm,
            if case == 3 { 44_100 } else { 48_000 },
            facts,
            None,
        )
        .unwrap();
        let context = hex_engine::AudioSourceContext {
            project_generation: if case == 6 { 1 } else { 2 },
            editor_revision: if case == 7 { 6 } else { 7 },
        };
        let cache = if case == 8 {
            hex_session::OriginalSourceOverviews::default()
        } else {
            fixture.view(context)
        };
        editor.update(cx, |view, cx| {
            view.state = Some(Arc::new(hex_session::service::ViewState {
                canonical_audio_source: None,
                comparison_audio: None,
                project: Arc::new(project),
                // A matching ordinary derivative waveform is never authority
                // for a native original when its qualified overview is absent.
                waveforms: std::collections::BTreeMap::from([(
                    asset,
                    Arc::new(
                        hex_media::waveform::Waveform::from_peaks(
                            4,
                            vec![
                                [hex_media::waveform::Peak {
                                    min: -0.25,
                                    max: 0.5
                                }; 2];
                                4
                            ],
                        )
                        .unwrap(),
                    ),
                )]),
                source_overviews: cache,
                audio_clip_waveforms: Default::default(),
                midi_preview: Arc::default(),
                gain_preview: Ok(Arc::default()),
                timeline_frames: 384_000,
                can_undo: false,
                can_redo: false,
                revision: 7,
                unsaved: true,
                document_path: None,
            }));
            view.project_generation = 2;
            view.selected_track = Some(track);
            view.device_panel.owner = Some(device_panel::DevicePanelOwner::Track(track));
            view.device_panel_active = true;
            view.clip_view_open = true;
            let metadata = view.state.as_ref().unwrap().project.asset(asset).unwrap();
            assert_eq!(
                view.sampler_source_overview(metadata).is_some(),
                valid,
                "case{case}"
            );
            let target = Target {
                generation: 2,
                revision: 7,
                owner: hex_session::service::SamplerParameterOwner::Track(track),
                device,
                parameter: SamplerParameter::StartFrame,
            };
            assert_eq!(
                target.current(view),
                valid,
                "a hidden source-bound control cannot dispatch using an old target: case{case}"
            );
            cx.notify();
        });
        visual.run_until_parked();
        if case == 0 {
            for prefix in [
                "device-drag-title",
                "device-bypass",
                "instrument-replace",
                "instrument-remove",
            ] {
                let selector = Box::leak(format!("{prefix}-{}", device.value()).into_boxed_str());
                assert!(
                    visual.debug_bounds(selector).is_some(),
                    "loaded sampler retains the ordinary {prefix} action without an audio output"
                );
            }
            for width in [1600.0, 900.0] {
                visual.simulate_resize(gpui::size(gpui::px(width), gpui::px(700.0)));
                visual.run_until_parked();
                let waveform = visual.debug_bounds("sampler-source-waveform").unwrap();
                // TestWindow currently renders at 2x. The 1x shape oracle is in
                // overview_waveform; physical 1x display acceptance is separate.
                let scale = visual.update(|window, _| window.scale_factor());
                assert_eq!(scale, 2.0);
                let canvas_bounds = gpui::Bounds::new(
                    waveform.origin,
                    gpui::size(waveform.size.width, waveform.size.height - gpui::px(14.0)),
                );
                editor.read_with(&visual, |view, _| {
                    let source = super::super::overview_waveform::Source::Sampler {
                        table: view.state.as_ref().unwrap().source_overviews.clone(),
                        asset,
                    };
                    let expected = super::super::waveform::waveform_column_count(
                        f32::from(canvas_bounds.size.width), scale,
                    );
                    assert!(expected > 128, "paint uses detailed device-pixel columns");
                    assert_eq!(
                        view.overview_waveform_cache.borrow().prepared_columns(&source, canvas_bounds, scale),
                        Some(expected),
                        "actual sampler canvas must prepare geometry at its rendered bounds after resize",
                    );
                });
                let controls = visual.debug_bounds("sampler-controls").unwrap();
                let panel = visual.debug_bounds("device-panel").unwrap();
                assert!(
                    waveform.size.width >= gpui::px(900.0),
                    "source must dominate the wide sampler, not remain a thumbnail: {waveform:?}"
                );
                assert!(
                    waveform.size.height >= gpui::px(96.0),
                    "waveform must retain useful vertical detail: {waveform:?}"
                );
                assert!(
                    waveform.bottom() <= controls.top(),
                    "controls belong below the sample"
                );
                assert!(
                    controls.size.height <= gpui::px(30.0),
                    "compact controls must preserve sample space: {controls:?}"
                );
                assert_eq!(
                    panel.size.height,
                    gpui::px(super::super::device_panel::DEVICE_PANEL_HEIGHT),
                    "device panel remains fixed height"
                );
                let region = visual.debug_bounds("sampler-source-active-region").unwrap();
                let start = visual.debug_bounds("sampler-source-start").unwrap();
                let end = visual.debug_bounds("sampler-source-end").unwrap();
                assert!(
                    (f32::from(region.left() - waveform.left()) / f32::from(waveform.size.width)
                        - 0.25)
                        .abs()
                        < 0.002
                );
                assert!(
                    (f32::from(region.size.width) / f32::from(waveform.size.width) - 0.5).abs()
                        < 0.002
                );
                assert!((f32::from(start.left() - region.left())).abs() <= 1.0);
                assert!((f32::from(end.left() - region.right())).abs() <= 1.0);
            }
            let rail = visual.debug_bounds("device-chain-scroll").unwrap();
            visual.simulate_event(gpui::ScrollWheelEvent {
                position: rail.center(),
                delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(-10_000.0), gpui::px(0.0))),
                ..Default::default()
            });
            visual.run_until_parked();
            let neighbor_selector =
                Box::leak(format!("effect-device-{}", neighbor.value()).into_boxed_str());
            let neighbor = visual.debug_bounds(neighbor_selector).unwrap();
            assert!(
                neighbor.left() >= rail.left() && neighbor.right() <= rail.right(),
                "neighbor remains reachable by horizontal scrolling: {neighbor:?} within {rail:?}"
            );
        }
        assert_eq!(
            visual.debug_bounds("sampler-source-waveform").is_some(),
            valid,
            "case{case}"
        );
        assert_eq!(visual.debug_bounds(selector).is_some(), valid, "case{case}");
    }
    editor.update(cx, |view, _| {
        view.computer_keyboard.enabled = true;
        view.computer_keyboard.suspended = false;
        assert!(view.tick_computer_keyboard());
        assert!(
            view.computer_keyboard.input.is_some(),
            "sampler keyboard preparation uses the ordinary input without a private output epoch"
        );
        assert_eq!(view.computer_keyboard.target, Some((2, 7, track)));
        view.cancel_computer_keyboard();
        view.computer_keyboard.enabled = false;
    });
}

/// Snapshot the actual adapter receipt after its ordinary Service submission.
pub(in crate::editor) fn pending_request(
    view: &EditorView,
) -> Option<hex_session::service::SamplerParameterEdit> {
    let pending = view.sampler_parameter.pending.as_ref()?;
    let PendingTarget::Parameter(target) = pending.target else {
        return None;
    };
    Some(hex_session::service::SamplerParameterEdit {
        request_id: pending.id,
        project_generation: target.generation,
        revision: target.revision,
        owner: target.owner,
        device: target.device,
        parameter: target.parameter,
        value: pending.value?,
        cancel: Arc::clone(&pending.cancel),
    })
}

pub(in crate::editor) fn rendered_control_value(
    view: &EditorView,
    parameter: SamplerParameter,
    cx: &gpui::App,
) -> Option<f64> {
    view.sampler_parameter
        .controls
        .get(&parameter)
        .map(|control| control.entity.read(cx).value())
}

pub(in crate::editor) fn control_entity(
    view: &EditorView,
    parameter: SamplerParameter,
) -> Option<Entity<NumericControl>> {
    view.sampler_parameter
        .controls
        .get(&parameter)
        .map(|control| control.entity.clone())
}

#[gpui::test]
fn scalar_sampler_preview_survives_repaints_until_matching_terminal(cx: &mut TestAppContext) {
    let (mut visual, editor, track, device) = numeric_sampler_fixture(cx);
    let selector = Box::leak(format!("sampler-attack-{}-value", device.value()).into_boxed_str());
    let position = visual.debug_bounds(selector).unwrap().center();
    visual.simulate_click(position, gpui::Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("100");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    for _ in 0..3 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        editor.read_with(&visual, |view, cx| {
            let pending = pending_request(view).expect("pending sampler edit");
            assert_eq!(
                pending.owner,
                hex_session::service::SamplerParameterOwner::Track(track)
            );
            assert_eq!(pending.value, 100.0);
            assert_eq!(
                rendered_control_value(view, SamplerParameter::AttackMs, cx),
                Some(100.0)
            );
        });
    }
    editor.update(&mut visual, |view, _| {
        let pending = pending_request(view).unwrap();
        view.finish_sampler_parameter(Some(pending.request_id + 1));
        assert!(view.sampler_parameter_pending());
        let old = view.state.as_ref().unwrap();
        let mut event = super::super::arrangement_main_track_ui_test::completed_event(old);
        let installed = Arc::get_mut(&mut event.state).unwrap();
        let current = installed.project.device(device).unwrap().state();
        let patched = hex_engine::patch_sampler_parameter_on_control(
            SamplerParameter::AttackMs,
            current,
            100.0,
        )
        .unwrap();
        let mut model = Editor::new((*installed.project).clone(), 16).unwrap();
        model
            .execute(Command::ReplaceDeviceState {
                device,
                state: patched,
            })
            .unwrap();
        installed.project = Arc::new(model.project().clone());
        event.sampler_edit_complete = Some(pending.request_id);
        view.receive(event);
        assert!(!view.sampler_parameter_pending());
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        assert_eq!(
            rendered_control_value(view, SamplerParameter::AttackMs, cx),
            Some(100.0)
        );
        assert_eq!(
            hex_engine::sampler_parameter_value_on_control(
                SamplerParameter::AttackMs,
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .device(device)
                    .unwrap()
                    .state()
            )
            .unwrap(),
            100.0
        );
    });
}
