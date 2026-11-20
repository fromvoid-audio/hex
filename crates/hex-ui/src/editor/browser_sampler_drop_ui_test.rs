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
        project_asset: None,
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

            let mut stale_browser = sample_drag(view, path.clone());
            stale_browser.browser_generation = Some(view.browser.scan_generation() - 1);
            view.begin_browser_audio_drag(&stale_browser);
            view.drop_browser_sample_on_track(
                &stale_browser,
                Some(tracks[0]),
                hex_project::InstrumentLoadPlacement::First,
                cx,
            );
            assert_eq!(view.worker_requests_pending, pending);
            assert!(view.active_browser_audio_drag.is_none());
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

            let blocked = sample_drag(view, path.clone());
            view.begin_browser_audio_drag(&blocked);
            let pending = view.worker_requests_pending;
            view.drop_browser_sample_on_track(
                &blocked,
                Some(tracks[0]),
                hex_project::InstrumentLoadPlacement::First,
                cx,
            );
            assert_eq!(view.worker_requests_pending, pending);
            assert_eq!(
                view.error.as_deref(),
                Some("sampler load is already pending")
            );

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

#[gpui::test]
fn browser_sample_real_drop_targets_load_one_sampler_and_preserve_other_tracks(
    cx: &mut TestAppContext,
) {
    use crate::editor::{EditorMode, device_panel::DevicePanelOwner, worker_ui_fixture};
    use gpui::size;
    use hex_session::service::Request;
    use std::time::{Duration, Instant};

    for route in [
        "panel",
        "arrangement-header",
        "console-header",
        "sampler-card",
    ] {
        let directory = PreferencesTestDirectory::new();
        let path = directory.0.join("loop.WAV");
        std::fs::write(&path, mono_pcm_wav(480)).unwrap();
        let existing = route == "sampler-card";
        let type_id = if existing {
            "hex.instrument.sampler"
        } else {
            "hex.utility"
        };
        let class = if existing {
            hex_engine::NativeDeviceClass::Instrument
        } else {
            hex_engine::NativeDeviceClass::Effect
        };
        let mut fixture = worker_ui_fixture::open(
            cx,
            vec![
                Command::AddMidiTrack {
                    name: "other".into(),
                },
                Command::InsertTrackWithDevice {
                    name: "target".into(),
                    before_track: None,
                    slot: if existing {
                        hex_project::TrackDeviceSlot::Instrument
                    } else {
                        hex_project::TrackDeviceSlot::Effect
                    },
                    state: hex_engine::default_native_device_state_for_class(
                        class,
                        hex_lattice_contract::DeviceTypeId::from_static(type_id),
                    )
                    .unwrap(),
                },
            ],
        );
        let editor = fixture.editor.clone();
        let before = editor.read_with(cx, |view, _| Arc::clone(view.state.as_ref().unwrap()));
        let other = before.project.tracks()[0].id();
        let track = before.project.tracks()[1].id();
        let old_sampler = before
            .project
            .track(track)
            .unwrap()
            .instrument()
            .map(hex_project::Device::id);
        let mut visual = VisualTestContext::from_window(fixture.handle, cx);
        visual.simulate_resize(size(px(1500.0), px(850.0)));
        editor.update(&mut visual, |view, cx| {
            view.mode = if route == "console-header" {
                EditorMode::Session
            } else {
                EditorMode::Arrangement
            };
            view.selected_track = Some(if route.contains("header") {
                other
            } else {
                track
            });
            view.device_panel.owner = Some(DevicePanelOwner::Track(track));
            view.device_panel_active = true;
            view.clip_view_open = true;
            view.browser.open = true;
            view.browser.auto_audition = false;
            view.browser.scans.generation += 1;
            view.browser.places = vec![directory.0.clone()];
            view.browser.directory = directory.0.clone();
            view.browser.section = BrowserSection::Place(directory.0.clone());
            view.browser.loading = false;
            view.browser.error = None;
            view.browser.listing = Some(DirectoryListing {
                directory: directory.0.clone(),
                entries: vec![LibraryEntry {
                    name: "loop.WAV".into(),
                    path: path.clone(),
                    kind: EntryKind::WaveAudio,
                }],
                enumerated_entries: 1,
                truncated: false,
            });
            cx.notify();
        });
        visual.run_until_parked();
        let source = visual.debug_bounds("browser-entry-0").unwrap().center();
        visual.simulate_mouse_down(source, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(source.x + px(12.0), source.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.run_until_parked();
        let selector = match route {
            "panel" => format!("device-drop-end-{}", track.value()),
            "arrangement-header" => format!("arrangement-track-header-{}", track.value()),
            "console-header" => format!("session-track-label-{}", track.value()),
            _ => format!("device-drop-card-{}", old_sampler.unwrap().value()),
        };
        let destination = visual
            .debug_bounds(Box::leak(selector.clone().into_boxed_str()))
            .unwrap_or_else(|| panic!("{route} receiver {selector}"))
            .center();
        visual.simulate_mouse_move(destination, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        // A painted panel owns its target even if selection changes before release.
        editor.update(&mut visual, |view, _| view.selected_track = Some(other));
        visual.simulate_mouse_up(destination, MouseButton::Left, Modifiers::default());
        let wait_for_revision = |fixture: &mut worker_ui_fixture::WorkerUiFixture,
                                 visual: &mut VisualTestContext,
                                 revision: u64| {
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
                let complete = editor.update(visual, |view, cx| {
                    view.tick(cx);
                    assert!(view.error.is_none(), "{route}: {:?}", view.error);
                    view.state
                        .as_ref()
                        .is_some_and(|state| state.revision == revision)
                        && !view.first_sampler_pending_matches(1)
                        && view.applied_project_revision() == Some(revision)
                });
                if complete {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "{route} revision {revision} did not finish"
                );
                std::thread::yield_now();
            }
        };
        wait_for_revision(&mut fixture, &mut visual, before.revision + 1);
        editor.read_with(&visual, |view, _| {
            let state = view.state.as_ref().unwrap();
            assert_eq!(view.selected_track, Some(other), "{route}");
            assert_eq!(state.project.track(other), before.project.track(other));
            let target = state.project.track(track).unwrap();
            assert_eq!(
                target.devices(),
                before.project.track(track).unwrap().devices()
            );
            assert_eq!(
                target.instrument_position(),
                Some(usize::from(route == "panel"))
            );
            let sampler = target.instrument().unwrap();
            assert_eq!(
                sampler.state().type_id().as_str(),
                Ok("hex.instrument.sampler")
            );
            assert_eq!(sampler.state().asset_references().len(), 1);
            assert_eq!(state.project.assets().count(), 1);
            if let Some(id) = old_sampler {
                assert_eq!(sampler.id(), id);
            }
            assert!(view.active_browser_audio_drag.is_none());
        });
        if route == "panel" {
            let loaded = editor.read_with(&visual, |view, _| {
                Arc::clone(&view.state.as_ref().unwrap().project)
            });
            editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
            wait_for_revision(&mut fixture, &mut visual, before.revision + 2);
            editor.read_with(&visual, |view, _| {
                let restored = &view.state.as_ref().unwrap().project;
                // Undo preserves the allocation high-water mark.
                assert_eq!(restored.tracks(), before.project.tracks());
                assert_eq!(restored.native_sources(), before.project.native_sources());
                assert_eq!(restored.assets().count(), before.project.assets().count());
            });
            editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Redo)));
            wait_for_revision(&mut fixture, &mut visual, before.revision + 3);
            editor.read_with(&visual, |view, _| {
                assert_eq!(view.state.as_ref().unwrap().project, loaded)
            });
        }
        fixture.finish(cx);
    }
}

#[gpui::test]
fn unassigned_sampler_real_browser_drop_loads_and_replaces_without_assignment(
    cx: &mut TestAppContext,
) {
    use crate::editor::device_panel::DevicePanelOwner;
    use gpui::size;
    use hex_library::{DirectoryListing, EntryKind, LibraryEntry};
    use std::time::{Duration, Instant};
    cx.update(crate::editor::bind_editor_keys);
    struct FixtureDir(std::path::PathBuf);
    impl Drop for FixtureDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let directory =
        FixtureDir(std::env::temp_dir().join(format!("hex-source-drop-{}", std::process::id())));
    std::fs::create_dir_all(&directory.0).unwrap();
    let path = directory.0.join("first.wav");
    let samples = [1024_i16, -4096, 8192, 0];
    let mut wav = b"RIFF".to_vec();
    wav.extend_from_slice(&(36_u32 + (samples.len() as u32 * 2)).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&48_000_u32.to_le_bytes());
    wav.extend_from_slice(&96_000_u32.to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&((samples.len() as u32) * 2).to_le_bytes());
    for sample in samples {
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(&path, wav).unwrap();
    let empty = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        hex_lattice_contract::DeviceTypeId::from_static("hex.instrument.sampler"),
    )
    .unwrap();
    let mut fixture = crate::editor::worker_ui_fixture::open(
        cx,
        vec![
            Command::AddMidiTrack {
                name: "other".into(),
            },
            Command::CreateNativeSource {
                name: "drop sampler".into(),
                devices: vec![empty],
                instrument_index: 0,
            },
        ],
    );
    let editor = fixture.editor.clone();
    let before = editor.read_with(cx, |view, _| Arc::clone(view.state.as_ref().unwrap()));
    let track = before.project.tracks()[0].id();
    let source = before
        .project
        .native_sources()
        .iter()
        .find(|source| source.name() == "drop sampler")
        .unwrap()
        .id();
    let device = before
        .project
        .native_source(source)
        .unwrap()
        .instrument()
        .id();
    let mut visual = VisualTestContext::from_window(fixture.handle, cx);
    visual.simulate_resize(size(px(1500.0), px(850.0)));
    editor.update(&mut visual, |view, cx| {
        view.selected_track = Some(track);
        view.device_panel_active = true;
        view.clip_view_open = true;
        cx.notify();
    });
    visual.run_until_parked();
    crate::editor::device_panel::tests::source_picker_ui_tests::open_unassigned_source(
        &mut visual,
        &editor,
        "drop sampler · 0 midi",
    );
    editor.update(&mut visual, |view, cx| {
        view.browser.open = true;
        view.browser.auto_audition = false;
        view.browser.scans.generation += 1;
        view.browser.places = vec![directory.0.clone()];
        view.browser.directory = directory.0.clone();
        view.browser.section = crate::editor::browser::BrowserSection::Place(directory.0.clone());
        view.browser.loading = false;
        view.browser.error = None;
        view.browser.listing = Some(DirectoryListing {
            directory: directory.0.clone(),
            entries: vec![LibraryEntry {
                name: "first.wav".into(),
                path: path.clone(),
                kind: EntryKind::WaveAudio,
            }],
            enumerated_entries: 1,
            truncated: false,
        });
        cx.notify();
    });
    visual.run_until_parked();
    for n in 1..=2 {
        let origin = visual.debug_bounds("browser-entry-0").unwrap().center();
        visual.simulate_mouse_down(origin, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(origin.x + px(12.0), origin.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.run_until_parked();
        let destination = visual.debug_bounds("source-device-panel").unwrap().center();
        visual.simulate_mouse_move(destination, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        visual.simulate_mouse_up(destination, MouseButton::Left, Modifiers::default());
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
            let complete = editor.update(&mut visual, |view, cx| {
                view.tick(cx);
                assert!(view.error.is_none(), "drop {n}: {:?}", view.error);
                view.state
                    .as_ref()
                    .is_some_and(|state| state.revision == before.revision + n)
                    && !view.sampler_parameter_pending()
            });
            if complete {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "source drop {n} did not complete"
            );
            std::thread::yield_now();
        }
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            let state = view.state.as_ref().unwrap();
            assert_eq!(
                view.device_panel.owner,
                Some(DevicePanelOwner::Source(source))
            );
            assert_eq!(state.project.track(track), before.project.track(track));
            assert_eq!(state.project.source_contributors(source).count(), 0);
            assert_eq!(
                state
                    .project
                    .native_source(source)
                    .unwrap()
                    .instrument()
                    .id(),
                device
            );
            assert_eq!(state.project.assets().count(), n as usize);
        });
    }
    fixture.finish(cx);
}
