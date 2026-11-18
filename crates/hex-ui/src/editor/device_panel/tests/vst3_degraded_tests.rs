//! Degraded vst3 marker (#4569): the current output's notice reaches the
//! device panel through the tick's audio-event step.
use super::*;
use crate::{EditorAudio, EditorAudioEvent};

fn selector(status: &str, device: DeviceId) -> &'static str {
    Box::leak(format!("device-vst3-{status}-{}", device.value()).into_boxed_str())
}

#[gpui::test]
fn only_the_current_documents_degraded_vst3_devices_show_the_marker(cx: &mut TestAppContext) {
    let _output_serial = hex_audio::worker_fixture::serial_output();
    let (mut output, backend) =
        hex_audio::worker_fixture::open_after_retirement(48_000, false).unwrap();
    let client = output.take_client().unwrap();
    let identity = client.output_identity().unwrap();
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "plugins".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    for _ in 0..2 {
        editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: NativeDeviceState::new(
                    hex_lattice_contract::DeviceTypeId::from_static("hex.vst3"),
                    1,
                    vec![0],
                )
                .unwrap(),
            })
            .unwrap();
    }
    let devices: Vec<DeviceId> = editor
        .project()
        .track(track)
        .unwrap()
        .devices()
        .iter()
        .map(|device| device.id())
        .collect();
    let (healthy, faulted) = (devices[0], devices[1]);
    let (handle, view) = super::super::super::clip_drag_ui_test::editor_window(
        cx,
        editor.project().clone(),
        Arc::default(),
    );
    let (events, receiver) = std::sync::mpsc::sync_channel(4);
    let generation = 1;
    view.update(cx, |view, cx| {
        view.project_generation = generation;
        view.audio_events = receiver;
        view.audio = Some(EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "degraded fixture".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        view.selected_track = Some(track);
        view.device_panel_active = true;
        view.clip_view_open = true;
        view.mode = super::super::super::EditorMode::Session;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    let deliver = |visual: &mut VisualTestContext,
                   project_generation,
                   output_identity,
                   devices,
                   current: bool| {
        events
            .send(EditorAudioEvent::DegradedPlugins {
                project_generation,
                output_identity,
                devices,
            })
            .unwrap();
        view.update(visual, |view, cx| {
            // The tick's audio-event step, without the unrelated service sync.
            assert_eq!(view.poll_editor_audio_events(cx), current);
            cx.notify();
        });
        visual.run_until_parked();
    };
    visual.run_until_parked();
    for device in [healthy, faulted] {
        assert!(visual.debug_bounds(selector("pending", device)).is_some());
    }
    let button = selector("editor", healthy);
    let button_color = |open| {
        let role = if open {
            theme::DeviceActionRole::Enabled
        } else {
            theme::DeviceActionRole::Normal
        };
        gpui::Background::from(gpui::rgb(
            theme::device_header_action_style(false, role).background,
        ))
    };
    assert_eq!(
        visual.rendered_background(button),
        Some(button_color(false))
    );
    for (generation, output, devices, changed, open) in [
        (0, identity, vec![healthy.value()], false, false),
        (
            generation,
            identity + 1,
            vec![healthy.value()],
            false,
            false,
        ),
        (generation, identity, vec![healthy.value()], true, true),
        (generation, identity, Vec::new(), true, false),
    ] {
        events
            .send(EditorAudioEvent::PluginEditorWindows {
                project_generation: generation,
                output_identity: output,
                devices,
            })
            .unwrap();
        view.update(&mut visual, |view, cx| {
            assert_eq!(view.poll_editor_audio_events(cx), changed);
            cx.notify();
        });
        visual.run_until_parked();
        assert_eq!(visual.rendered_background(button), Some(button_color(open)));
    }
    // A notice from an output that is no longer current is ignored.
    deliver(
        &mut visual,
        generation,
        identity + 1,
        vec![faulted.value()],
        false,
    );
    // The same output may still carry a queued notice from its old document.
    deliver(&mut visual, 0, identity, vec![faulted.value()], false);
    assert!(visual.debug_bounds(selector("degraded", faulted)).is_none());
    deliver(
        &mut visual,
        generation,
        identity,
        vec![faulted.value()],
        true,
    );
    assert!(visual.debug_bounds(selector("degraded", faulted)).is_some());
    assert!(visual.debug_bounds(selector("pending", faulted)).is_none());
    assert!(visual.debug_bounds(selector("degraded", healthy)).is_none());
    assert!(visual.debug_bounds(selector("pending", healthy)).is_some());
    // An empty set clears the marker.
    deliver(&mut visual, generation, identity, Vec::new(), true);
    assert!(visual.debug_bounds(selector("degraded", faulted)).is_none());
    assert!(visual.debug_bounds(selector("pending", faulted)).is_some());
    drop(backend);
    output.publication.close();
}
