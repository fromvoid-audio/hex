//! Rendered Drum Rack pad drop-to-canonical-command, delayed-ACK repaint,
//! refusal, and project-replacement retirement UI tests (#3476).
use super::*;
use crate::editor::browser::BrowserSection;
use hex_library::{EntryKind, LibraryEntry};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

struct TestDir(PathBuf);

impl TestDir {
    fn new(prefix: &str) -> Self {
        static SERIAL: AtomicU64 = AtomicU64::new(1);
        let id = SERIAL.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("hex-ui-{prefix}-{}-{id}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create test temp dir");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn mono_wav_bytes(sample_rate: u32, frames: usize) -> Vec<u8> {
    let data_bytes = u32::try_from(frames * 2).unwrap();
    let mut out = Vec::with_capacity(44 + data_bytes as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16_u32.to_le_bytes());
    out.extend_from_slice(&1_u16.to_le_bytes());
    out.extend_from_slice(&1_u16.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    out.extend_from_slice(&2_u16.to_le_bytes());
    out.extend_from_slice(&16_u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_bytes.to_le_bytes());
    for i in 0..frames {
        let phase = 2.0 * std::f32::consts::PI * 220.0 * (i as f32) / (sample_rate as f32);
        let sample = (phase.sin() * 0.5 * i16::MAX as f32).round() as i16;
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

fn leak_selector(selector: String) -> &'static str {
    Box::leak(selector.into_boxed_str())
}

fn open_shared_drum_rack_fixture(
    cx: &mut TestAppContext,
) -> (
    WorkerUiFixture,
    hex_project::SourceId,
    TrackId,
    TrackId,
    DeviceId,
) {
    let rack_state = hex_engine::default_native_device_state_for_class(
        NativeDeviceClass::Instrument,
        hex_engine::DRUM_RACK_TYPE_ID,
    )
    .expect("default drum rack state");
    let fixture = worker_ui_fixture::open(
        cx,
        vec![
            Command::CreateNativeSource {
                name: "shared drums".into(),
                devices: vec![rack_state],
                instrument_index: 0,
            },
            Command::AddMidiTrack {
                name: "drums a".into(),
            },
            Command::AddMidiTrack {
                name: "drums b".into(),
            },
        ],
    );
    let (source_id, track_a, track_b, rack_device) = fixture.editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        let source = project.native_sources()[0].id();
        let device = project.native_sources()[0].instrument().id();
        let a = project.tracks()[0].id();
        let b = project.tracks()[1].id();
        (source, a, b, device)
    });

    // Bind both MIDI tracks to the same retained Drum Rack source.
    let mut fixture = fixture;
    for track in [track_a, track_b] {
        let target_rev = fixture
            .editor
            .read_with(cx, |view, _| view.state.as_ref().unwrap().revision + 1);
        fixture.editor.update(cx, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::BindTrackSource {
                track,
                source: Some(source_id),
            })));
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
            let done = fixture.editor.update(cx, |view, cx| {
                view.tick(cx);
                assert!(view.error.is_none(), "{:?}", view.error);
                view.state
                    .as_ref()
                    .is_some_and(|state| state.revision >= target_rev)
                    && view.worker_requests_pending == 0
            });
            if done {
                break;
            }
            assert!(Instant::now() < deadline, "bind track source timed out");
            std::thread::yield_now();
        }
    }
    (fixture, source_id, track_a, track_b, rack_device)
}

fn wait_for_drum_rack_drop_settlement(
    fixture: &mut WorkerUiFixture,
    visual: &mut VisualTestContext,
    expected_revision: u64,
) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let done = fixture.editor.update(visual, |view, cx| {
            view.tick(cx);
            assert!(view.error.is_none(), "{:?}", view.error);
            view.state
                .as_ref()
                .is_some_and(|state| state.revision >= expected_revision)
                && !view.device_panel.drum_rack_drop.has_pending()
                && view.worker_requests_pending == 0
        });
        if done {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "drum rack pad drop did not settle at revision {expected_revision}"
        );
        std::thread::yield_now();
    }
    fixture.editor.update(visual, |_, cx| cx.notify());
    visual.refresh().unwrap();
    visual.run_until_parked();
}

#[gpui::test]
fn rendered_drum_rack_pad_drops_for_sample_and_instrument_hold_delayed_ack_preview_across_repaints(
    cx: &mut TestAppContext,
) {
    let dir = TestDir::new("drum-rack-drop");
    let wav_path = dir.path().join("kick_drop.wav");
    std::fs::write(&wav_path, mono_wav_bytes(48_000, 512)).expect("write wav");

    let (mut fixture, source_id, track_a, track_b, rack_device) = open_shared_drum_rack_fixture(cx);
    let editor = fixture.editor.clone();
    let mut visual = VisualTestContext::from_window(fixture.handle, cx);
    visual.simulate_resize(size(px(1600.0), px(850.0)));

    editor.update(&mut visual, |view, cx| {
        view.selected_track = Some(track_a);
        view.device_panel.owner = Some(DevicePanelOwner::Track(track_a));
        view.device_panel.active = true;
        view.audio_detail.clip_view_open = true;
        view.browser.set_test_sample_listing(
            dir.path().to_path_buf(),
            vec![LibraryEntry {
                name: "kick_drop.wav".into(),
                path: wav_path.clone(),
                kind: EntryKind::WaveAudio,
            }],
        );
        cx.notify();
    });
    visual.run_until_parked();

    let pad1_sel = leak_selector(format!("drum-rack-pad-{}-1", rack_device.value()));
    let pad1_pending_sel =
        leak_selector(format!("drum-rack-pad-pending-{}-1", rack_device.value()));
    let pad2_sel = leak_selector(format!("drum-rack-pad-{}-2", rack_device.value()));
    let pad2_pending_sel =
        leak_selector(format!("drum-rack-pad-pending-{}-2", rack_device.value()));
    assert!(visual.debug_bounds(pad1_sel).is_some(), "pad 1 rendered");
    assert!(visual.debug_bounds(pad2_sel).is_some(), "pad 2 rendered");

    // 1. Rendered sample drag from browser onto Drum Rack pad 1.
    let before_rev = editor.read_with(&visual, |view, _| view.state.as_ref().unwrap().revision);
    let sample_src = visual
        .debug_bounds("browser-entry-0")
        .expect("browser sample row")
        .center();
    let pad1_center = visual
        .debug_bounds(pad1_sel)
        .expect("pad 1 bounds")
        .center();
    visual.simulate_mouse_down(sample_src, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(sample_src.x + px(12.0), sample_src.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    visual.simulate_mouse_move(pad1_center, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_mouse_up(pad1_center, MouseButton::Left, Modifiers::default());

    // Delayed-ACK repaint verification: complete two repaints BEFORE draining the worker.
    // Ending pointer capture must NOT expose stale empty canonical pad state while in flight.
    for _ in 0..2 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.refresh().unwrap();
        visual.run_until_parked();
        assert!(
            visual.debug_bounds(pad1_pending_sel).is_some(),
            "pending pad 1 preview must remain painted across delayed-ACK repaints"
        );
    }
    editor.read_with(&visual, |view, _| {
        assert!(view.device_panel.drum_rack_drop.has_pending());
        assert!(view.browser.active_audio_drag.is_none());
    });

    // Now settle the worker event + audio publication and verify canonical pad 1 + retired preview.
    wait_for_drum_rack_drop_settlement(&mut fixture, &mut visual, before_rev + 1);
    assert!(
        visual.debug_bounds(pad1_pending_sel).is_none(),
        "pending pad 1 preview must retire in the same receive as canonical install"
    );
    editor.read_with(&visual, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        let views = hex_engine::drum_rack_pad_views_on_control(
            project
                .native_source(source_id)
                .unwrap()
                .instrument()
                .state(),
        )
        .unwrap();
        assert!(views[0].occupied);
        assert_eq!(views[0].name, "kick_drop");
        assert!(views[0].asset_key.is_some());
        assert_eq!(
            project
                .track(track_a)
                .unwrap()
                .instrument()
                .unwrap()
                .state(),
            project
                .track(track_b)
                .unwrap()
                .instrument()
                .unwrap()
                .state(),
            "shared drum rack source must stay identical across contributor tracks"
        );
    });

    // 2. Rendered native leaf instrument drag (FM4) from browser onto Drum Rack pad 2.
    editor.update(&mut visual, |view, cx| {
        view.browser.section = BrowserSection::Instruments;
        cx.notify();
    });
    visual.run_until_parked();

    let before_inst_rev =
        editor.read_with(&visual, |view, _| view.state.as_ref().unwrap().revision);
    let inst_src = visual
        .debug_bounds("browser-device-instrument-hex.instrument.fm4")
        .expect("browser fm4 row")
        .center();
    let pad2_center = visual
        .debug_bounds(pad2_sel)
        .expect("pad 2 bounds")
        .center();
    visual.simulate_mouse_down(inst_src, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(inst_src.x + px(12.0), inst_src.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    visual.simulate_mouse_move(pad2_center, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_mouse_up(pad2_center, MouseButton::Left, Modifiers::default());

    // Delayed-ACK repaint verification for instrument pad drop.
    for _ in 0..2 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.refresh().unwrap();
        visual.run_until_parked();
        assert!(
            visual.debug_bounds(pad2_pending_sel).is_some(),
            "pending pad 2 preview must remain painted across delayed-ACK repaints"
        );
    }

    wait_for_drum_rack_drop_settlement(&mut fixture, &mut visual, before_inst_rev + 1);
    assert!(visual.debug_bounds(pad2_pending_sel).is_none());
    editor.read_with(&visual, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        let views = hex_engine::drum_rack_pad_views_on_control(
            project
                .native_source(source_id)
                .unwrap()
                .instrument()
                .state(),
        )
        .unwrap();
        assert!(views[0].occupied, "pad 1 sample must remain intact");
        assert!(views[1].occupied, "pad 2 must now hold fm4");
        assert_eq!(
            views[1].child_type_id,
            Some(hex_lattice_contract::DeviceTypeId::from_static(
                "hex.instrument.fm4"
            ))
        );
    });
}

#[gpui::test]
fn drum_rack_pad_drop_refuses_nested_rack_and_retires_pending_on_project_replacement(
    cx: &mut TestAppContext,
) {
    let dir = TestDir::new("drum-rack-drop");
    let wav_path = dir.path().join("hat.wav");
    std::fs::write(&wav_path, mono_wav_bytes(48_000, 512)).expect("write wav");

    let (mut fixture, source_id, track_a, _track_b, rack_device) =
        open_shared_drum_rack_fixture(cx);
    let editor = fixture.editor.clone();
    let mut visual = VisualTestContext::from_window(fixture.handle, cx);
    visual.simulate_resize(size(px(1600.0), px(850.0)));

    editor.update(&mut visual, |view, cx| {
        view.selected_track = Some(track_a);
        view.device_panel.owner = Some(DevicePanelOwner::Track(track_a));
        view.device_panel.active = true;
        view.audio_detail.clip_view_open = true;
        view.browser.set_test_instruments_section();
        cx.notify();
    });
    visual.run_until_parked();

    let pad3_sel = leak_selector(format!("drum-rack-pad-{}-3", rack_device.value()));
    let pad3_center = visual
        .debug_bounds(pad3_sel)
        .expect("pad 3 bounds")
        .center();

    // 1. Attempting to drop a nested Drum Rack onto a pad is refused before enqueueing.
    let before_pending = editor.read_with(&visual, |view, _| view.worker_requests_pending);
    let nested_src = visual
        .debug_bounds("browser-device-instrument-hex.rack.drum")
        .expect("browser drum rack row")
        .center();
    visual.simulate_mouse_down(nested_src, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(nested_src.x + px(12.0), nested_src.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    visual.simulate_mouse_move(pad3_center, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_mouse_up(pad3_center, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();

    editor.read_with(&visual, |view, _| {
        assert_eq!(view.worker_requests_pending, before_pending);
        assert!(!view.device_panel.drum_rack_drop.has_pending());
        assert!(view.error.is_some());
        let views = hex_engine::drum_rack_pad_views_on_control(
            view.state
                .as_ref()
                .unwrap()
                .project
                .native_source(source_id)
                .unwrap()
                .instrument()
                .state(),
        )
        .unwrap();
        assert!(views.iter().all(|v| !v.occupied));
    });

    // 2. Project replacement retires an in-flight pad drop preview cleanly.
    editor.update(&mut visual, |view, cx| {
        view.error = None;
        view.browser.set_test_sample_listing(
            dir.path().to_path_buf(),
            vec![LibraryEntry {
                name: "hat.wav".into(),
                path: wav_path.clone(),
                kind: EntryKind::WaveAudio,
            }],
        );
        cx.notify();
    });
    visual.run_until_parked();

    let pad1_sel = leak_selector(format!("drum-rack-pad-{}-1", rack_device.value()));
    let pad1_pending_sel =
        leak_selector(format!("drum-rack-pad-pending-{}-1", rack_device.value()));
    let sample_src = visual
        .debug_bounds("browser-entry-0")
        .expect("browser sample row")
        .center();
    let pad1_center = visual
        .debug_bounds(pad1_sel)
        .expect("pad 1 bounds")
        .center();
    visual.simulate_mouse_down(sample_src, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(sample_src.x + px(12.0), sample_src.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    visual.simulate_mouse_move(pad1_center, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_mouse_up(pad1_center, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds(pad1_pending_sel).is_some());

    // Deliver a project replacement event while the drop is still in flight.
    // The pending preview must retire in the exact same receive and repaint cleanly.
    let gen_before = editor.read_with(&visual, |view, _| view.project_generation);
    let replacement_project = Arc::new(Project::new(48_000).expect("replacement project"));
    let replacement_event = Event {
        state: Arc::new(ViewState {
            canonical_audio_source: None,
            comparison_audio: None,
            project: replacement_project,
            waveforms: Default::default(),
            source_overviews: Default::default(),
            audio_clip_waveforms: Default::default(),
            midi_preview: Arc::new(Default::default()),
            gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
            timeline_frames: 384_000,
            can_undo: false,
            can_redo: false,
            revision: 1,
            unsaved: false,
            document_path: None,
        }),
        playback: None,
        status: Some("new project".into()),
        error: None,
        resident_edit_source_unavailable: None,
        recovery_failure: None,
        request_complete: true,
        sampler_load_complete: None,
        sampler_edit_complete: None,
        plugin_effect_complete: Vec::new(),
        device_insert_proposal_complete: None,
        device_insert_confirmation: None,
        device_insert_complete: None,
        plugin_parameter_observation: false,
        plugin_parameters: None,
        plugin_parameter_conversion: None,
        computer_keyboard_complete: None,
        midi_take_complete: None,
        device_automation_take_complete: None,
        tuning_audition_complete: None,
        live_expression_complete: None,
        bounce_complete: None,
        audio_batch_complete: None,
        audio_batch_failure: None,
        import_truncation: None,
        save_complete: false,
        save_correlation: None,
        document_format_upgrade_required: false,
        save_confirmation_required: false,
        open_complete: true,
        project_replaced: true,
        stopped_playback_restore_complete: false,
        created: None,
        duplicate_complete: false,
        restore_complete: None,
        device_preview: None,
        tuning_preview: None,
        tuning_gesture_ack: None,
        tuning_gesture_request: false,
        active_tuning_gesture: None,
        tuning_library: None,
        gesture_ack: None,
        gesture_protocol_ack: false,
        gesture_request: false,
        active_device_gesture: None,
    };
    editor.update(&mut visual, |view, cx| {
        let _ = view.receive(replacement_event);
        assert!(view.project_generation > gen_before);
        assert!(!view.device_panel.drum_rack_drop.has_pending());
        cx.notify();
    });
    visual.refresh().unwrap();
    visual.run_until_parked();
    assert!(visual.debug_bounds(pad1_pending_sel).is_none());
    editor.read_with(&visual, |view, _| {
        assert!(!view.device_panel.drum_rack_drop.has_pending());
        assert_eq!(view.state.as_ref().unwrap().project.tracks().len(), 0);
    });
    let _ = fixture.backend.render(&mut [[0.0; 2]; 64]);
}
