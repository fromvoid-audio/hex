//! Browser sample drops onto a sampler card load into that card's track.
use super::*;

/// Marks a browser preview as playing on output generation 1.
fn start_preview(view: &mut EditorView, path: &std::path::Path) {
    view.browser.playback_phase = hex_audio::sample_preview::Phase::Playing;
    view.browser.playback_output = Some((1, 1));
    view.browser.playback_path = Some(path.to_path_buf());
}

fn sample_drag(view: &EditorView, path: std::path::PathBuf) -> BrowserAudioDrag {
    let state = view.state.as_ref().expect("project state");
    let viewport = view.visible_viewport(state.timeline_frames);
    BrowserAudioDrag {
        project_generation: view.project_generation,
        revision: state.revision,
        browser_generation: Some(view.browser.scan_generation()),
        sample_rate: state.project.sample_rate(),
        path,
        label: "loop.wav".into(),
        viewport_start: viewport.start,
        viewport_length: viewport.length,
        scroll: Default::default(),
        snap_grid: view.snap_grid,
        snap_modifier: Rc::new(std::cell::Cell::new(false)),
        copy_modifier: Rc::new(std::cell::Cell::new(false)),
        metadata: Rc::new(RefCell::new(None)),
        metadata_error: Rc::new(RefCell::new(None)),
        preview: Rc::new(std::cell::Cell::new(None)),
        destination: Rc::new(std::cell::Cell::new(None)),
        insertion_line_y: Rc::new(std::cell::Cell::new(None)),
        lane_hovered: Rc::new(std::cell::Cell::new(false)),
        last_pointer: Rc::new(std::cell::Cell::new(None)),
    }
}

#[gpui::test]
fn browser_sample_drop_loads_the_target_sampler_not_the_selected_track(cx: &mut TestAppContext) {
    let (handle, editor, tracks, _directory, path) =
        browser_drag_fixture_with_tracks(cx, &["selected", "sampler"]);
    cx.update_window(handle, |_, _, cx| {
        editor.update(cx, |view, cx| {
            let state = view.state.as_ref().unwrap();
            let mut model = Editor::new((*state.project).clone(), 4).unwrap();
            model
                .execute(Command::InsertInstrument {
                    track: tracks[1],
                    state: hex_engine::default_native_device_state_for_class(
                        hex_engine::NativeDeviceClass::Instrument,
                        hex_lattice_contract::DeviceTypeId::from_static("hex.instrument.sampler"),
                    )
                    .unwrap(),
                })
                .unwrap();
            let project = Arc::new(model.project().clone());
            let sampler = project.track(tracks[1]).unwrap().instrument().unwrap().id();
            view.state = Some(Arc::new(ViewState {
                canonical_audio_source: None,
                comparison_audio: None,
                project,
                waveforms: Default::default(),
                source_overviews: Default::default(),
                audio_clip_waveforms: Default::default(),
                midi_preview: Arc::default(),
                gain_preview: state.gain_preview.clone(),
                timeline_frames: state.timeline_frames,
                can_undo: state.can_undo,
                can_redo: state.can_redo,
                revision: state.revision,
                unsaved: state.unsaved,
                document_path: state.document_path.clone(),
            }));
            view.selected_track = Some(tracks[0]);

            // A drop naming a device that is no longer the card's instrument
            // retires the payload without enqueueing a load, and like any
            // cancelled drop leaves the preview playing.
            start_preview(view, &path);
            let stale = sample_drag(view, path.clone());
            view.begin_browser_audio_drag(&stale);
            let pending = view.worker_requests_pending;
            view.drop_browser_sample_on_sampler(&stale, tracks[0], sampler, cx);
            assert_eq!(view.worker_requests_pending, pending);
            assert!(view.active_browser_audio_drag.is_none());
            assert!(!view.first_sampler_pending_matches(1));
            assert_eq!(view.browser.playback_output, Some((1, 1)));

            let drag = sample_drag(view, path.clone());
            view.begin_browser_audio_drag(&drag);
            let preview_generation = view.browser.playback_generation;
            view.drop_browser_sample_on_sampler(&drag, tracks[1], sampler, cx);
            assert!(view.active_browser_audio_drag.is_none());
            assert!(view.first_sampler_pending_matches(1));
            // The admitted load stops the preview through its ordinary cancel.
            assert!(view.browser.playback_output.is_none());
            assert_eq!(
                view.browser.playback_phase,
                hex_audio::sample_preview::Phase::Idle
            );
            assert_eq!(view.browser.playback_generation, preview_generation + 1);
            assert_eq!(view.selected_track, Some(tracks[0]), "drop does not select");

            // The pending load belongs to the dropped-on track.
            let generation = view.project_generation;
            view.first_sampler_load_complete(
                Some(hex_session::service::SamplerLoadCompletion {
                    request_id: 1,
                    project_generation: generation,
                    track: tracks[0],
                }),
                false,
            );
            assert!(view.first_sampler_pending_matches(1));
            view.first_sampler_load_complete(
                Some(hex_session::service::SamplerLoadCompletion {
                    request_id: 1,
                    project_generation: generation,
                    track: tracks[1],
                }),
                false,
            );
            assert!(!view.first_sampler_pending_matches(1));
        });
    })
    .unwrap();
}
