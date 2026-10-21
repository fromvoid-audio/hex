use super::*;
use gpui::{TestAppContext, VisualTestContext};
use hex_project::{Command, Created, Editor, Project};

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
        let project = Project::from_snapshot(snapshot).unwrap();
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
                track,
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
        assert_eq!(
            visual.debug_bounds("sampler-source-waveform").is_some(),
            valid,
            "case{case}"
        );
        assert_eq!(visual.debug_bounds(selector).is_some(), valid, "case{case}");
    }
}
