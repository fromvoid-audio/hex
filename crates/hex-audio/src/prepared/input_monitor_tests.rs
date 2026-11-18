use super::*;
use crate::input_capture::test_monitor_source;
use crate::sample_recording::{OutputClock, OutputClockIndex, OutputClockOrigin};
use hex_engine::{MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES, PausedTrackMonitor};
use hex_project::{Command, Created, Editor, Project};
use std::collections::BTreeMap;

fn paused_fixture() -> (Arc<hex_engine::PreparedRevision>, hex_project::TrackId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "input".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let revision = hex_engine::PreparedRevision::prepare(
        editor.project(),
        &BTreeMap::new(),
        Arc::new(hex_engine::SineTable::new()),
        None,
    )
    .unwrap();
    (Arc::new(revision), track)
}

fn publish_paused_fixture_graph(
    control: &mut Control,
    revision: Arc<hex_engine::PreparedRevision>,
) {
    control
        .publish_unaccounted_pcm_obligation(
            Playback::try_from_prepared_with_session(revision).unwrap(),
            PublicationPolicy::InstallFreshRuntime,
            7,
            true,
            false,
            Some(hex_engine::AudioSourceContext {
                project_generation: 7,
                editor_revision: 3,
            }),
        )
        .unwrap();
}

fn render_paused_fixture_block(callback: &mut Callback) -> [Frame; 64] {
    let mut output = [[1.0; 2]; 64];
    let mut render = || {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    };
    #[cfg(feature = "allocation-probe")]
    {
        let counts = allocation_counter::measure(&mut render);
        assert_eq!(
            (
                counts.count_total,
                counts.bytes_total,
                counts.count_current,
                counts.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }
    #[cfg(not(feature = "allocation-probe"))]
    render();
    output
}

#[test]
fn paused_keyboard_render_preserves_callback_allocation_budget() {
    use hex_engine::NativeDeviceClass;
    use hex_engine::computer_keyboard::ComputerKeyboardInput;
    use hex_lattice_contract::DeviceTypeId;

    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "keyboard".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::InsertInstrument {
            track,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap();
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: hex_engine::default_native_device_state_for_class(
                NativeDeviceClass::Effect,
                DeviceTypeId::from_static("hex.limiter"),
            )
            .unwrap(),
        })
        .unwrap();
    let revision = Arc::new(
        hex_engine::PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let latency = revision.arrangement().main_latency_frames() as usize;
    assert_ne!(latency, 0);
    let input = Arc::new(ComputerKeyboardInput::default());
    let mut playback = Playback::try_from_prepared_with_session(revision).unwrap();
    playback
        .prepare_resident_computer_keyboard(editor.project(), None, track, Arc::clone(&input))
        .unwrap();
    playback.apply(hex_engine::PlaybackCommand::Play);
    playback.render(&mut [[0.0; 2]; 4]).unwrap();
    playback.apply(hex_engine::PlaybackCommand::Pause);
    let frozen = playback.position();

    let mut render = |output: &mut [Frame]| {
        let mut render = || {
            for block in output.chunks_mut(257) {
                playback.render(block).unwrap();
            }
        };
        #[cfg(feature = "allocation-probe")]
        {
            let counts = allocation_counter::measure(&mut render);
            assert_eq!(
                (
                    counts.count_total,
                    counts.bytes_total,
                    counts.count_current,
                    counts.bytes_current,
                ),
                (0, 0, 0, 0)
            );
        }
        #[cfg(not(feature = "allocation-probe"))]
        render();
    };
    let mut pressed = vec![[0.0; 2]; latency + 257];
    input.set_key(0, Some((60, 100, 1))).unwrap();
    render(&mut pressed);
    assert!(pressed[..latency].iter().all(|frame| *frame == [0.0; 2]));
    assert!(pressed[latency..].iter().any(|frame| *frame != [0.0; 2]));
    let mut released = vec![[0.0; 2]; latency + 257];
    input.set_key(0, None).unwrap();
    render(&mut released);
    assert!(released[latency..].iter().any(|frame| *frame != [0.0; 2]));
    assert_eq!(playback.position(), frozen);
}

#[test]
fn paused_monitor_accepts_the_exact_accounted_resident_graph() {
    use super::candidate_tests::{anchored, finish, serial};
    use hex_engine::native_device_resources::{PreparedDeviceResources, ResidentDeviceAssets};
    use hex_engine::{AudioStorageLedgerReservation, NativeDeviceClass, SessionPlaybackError};
    use hex_lattice_contract::DeviceTypeId;

    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "input".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let limiter = hex_engine::default_native_device_state_for_class(
        NativeDeviceClass::Effect,
        DeviceTypeId::from_static("hex.limiter"),
    )
    .unwrap();
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: limiter,
        })
        .unwrap();
    let project = editor.project();
    let assets = ResidentDeviceAssets::default();
    let cancel = AtomicBool::new(false);
    let resources =
        PreparedDeviceResources::prepare_on_control(&assets, project, 4096, &cancel).unwrap();
    let unrelated =
        PreparedDeviceResources::prepare_on_control(&assets, project, 4096, &cancel).unwrap();
    let revision = Arc::new(
        hex_engine::PreparedRevision::prepare_with_device_resources(
            project,
            &BTreeMap::new(),
            Arc::new(hex_engine::SineTable::new()),
            None,
            &hex_engine::audio_transform::AudioTransformPreparation::default(),
            &resources,
            0,
            &cancel,
        )
        .unwrap(),
    );
    let latency = revision.arrangement().main_latency_frames() as usize;
    assert!((1..1024).contains(&latency));
    let allowance =
        PausedTrackMonitor::resident_runtime_allowance_on_control(&revision, &resources, track)
            .unwrap();
    assert!(matches!(
        PausedTrackMonitor::prepare_resident_on_control(
            Arc::clone(&revision),
            &unrelated,
            track,
            allowance,
        ),
        Err(SessionPlaybackError::InvalidCommand)
    ));
    assert!(matches!(
        PausedTrackMonitor::prepare_resident_on_control(
            Arc::clone(&revision),
            &resources,
            track,
            allowance - 1,
        ),
        Err(SessionPlaybackError::Allocation)
    ));
    let (monitor, hold) = PausedTrackMonitor::prepare_admitted_resident_on_control(
        Arc::clone(&revision),
        &resources,
        track,
        4096,
        |view| {
            use hex_engine::native_device_resources::{
                DeviceResourcePhase, inspect_device_resource_union,
            };
            let phase = DeviceResourcePhase {
                stores: &[],
                registered: &[view],
                selections: &[],
                outbound: &[],
                legacy: &[],
                checked_runtime_bytes: 0,
                construction_bytes: 0,
            };
            inspect_device_resource_union(
                &phase,
                &phase,
                AudioStorageLedgerReservation {
                    data_rows: 512,
                    handle_rows: 256,
                    root_rows: 128,
                    max_retained_bytes: 256 * 1024 * 1024,
                    max_construction_peak_bytes: 256 * 1024 * 1024,
                },
            )
            .map(|_| ())
        },
    )
    .unwrap();
    let playback = Playback::try_from_prepared_with_device_resources(revision, &resources).unwrap();
    drop(resources);

    let _serial = serial();
    let (mut control, mut data, errors, mut retirement) = anchored();
    control
        .configure_device_resource_publication_on_control(AudioStorageLedgerReservation {
            data_rows: 512,
            handle_rows: 256,
            root_rows: 128,
            max_retained_bytes: 20_000_000,
            max_construction_peak_bytes: 20_000_000,
        })
        .unwrap();
    control
        .configure_ordinary_streaming_on_control(
            crate::AudioCandidateDomainReservation {
                max_retained_data_bytes: 8 * 1024 * 1024,
                max_construction_peak_data_bytes: 8 * 1024 * 1024,
            },
            2,
            4096,
        )
        .unwrap();
    let revision = control
        .publish_with_admission_and_source_on_control(
            playback,
            PublicationPolicy::InstallFreshRuntime,
            hex_engine::AudioSourceContext {
                project_generation: 7,
                editor_revision: 3,
            },
            |_| Ok::<_, ()>(()),
        )
        .unwrap();
    data.owner_mut().unwrap().run(|callback, _| {
        callback.begin_block();
        callback.render(&mut [[0.0; 2]; 64]).unwrap();
        assert!(
            callback
                .active
                .as_ref()
                .is_some_and(ActivePublication::streaming_enabled),
            "the ordinary source workspace must be attached as on desktop",
        );
    });
    let source = control.status.recording_source().unwrap();
    assert_eq!(source.audio_revision, revision);
    assert_eq!(control.unreturned_resident_graphs_on_control(), Some(1));
    let epoch = control.status.transport_epoch.load(Acquire);
    let (mut input, stream) = test_monitor_source();
    assert_eq!(input.submit(&[0.25; 1024]), 4);
    let original = &*stream as *const _;
    assert!(
        control
            .input_monitor
            .publish_paused(stream, monitor, source, epoch)
            .is_ok()
    );

    let mut render_block = || {
        let mut block = [[0.0; 2]; 64];
        data.owner_mut().unwrap().run(|callback, _| {
            block = render_paused_fixture_block(callback);
        });
        block
    };
    let output: Vec<_> = (0..16).flat_map(|_| render_block()).collect();
    assert!(output[..latency].iter().all(|frame| *frame == [0.0; 2]));
    assert!(
        output[latency..]
            .iter()
            .all(|frame| frame[0] > 0.0 && frame[1] > 0.0)
    );
    assert_eq!(control.status.position.load(Acquire), 0);
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(control.outstanding_device_resource_leases(), 0);
    assert_eq!(input.submit(&[0.75; 512]), 2);
    control.input_monitor.release_paused();
    let tail: Vec<_> = (0..(latency.div_ceil(64) + 2))
        .flat_map(|_| render_block())
        .collect();
    assert!(tail[..latency].iter().all(|frame| *frame == output[1023]));
    assert!(tail[latency..].iter().all(|frame| *frame == [0.0; 2]));
    assert_eq!(control.status.position.load(Acquire), 0);
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(
        control.input_monitor.status(),
        input_monitor::MonitorStatus::Stopped
    );
    assert_eq!(render_block(), [[0.0; 2]; 64]);
    assert!(
        !hold.released_on_control(),
        "a callback return is not destruction"
    );
    assert_eq!(
        original,
        &*control.input_monitor.reclaim().unwrap() as *const _
    );
    assert!(hold.released_on_control());
    control.request_shutdown();
    assert_eq!(render_block(), [[0.0; 2]; 64]);
    control.reclaim();
    assert_eq!(control.unreturned_resident_graphs_on_control(), Some(0));
    drop(data);
    drop(errors);
    drop(control);
    finish(&mut retirement);
}

#[test]
fn paused_input_uses_the_resident_callback_and_retires_at_discontinuities() {
    use input_monitor::MonitorStatus;

    for (command, hard_stop, terminal) in [
        (Some(PlaybackCommand::Stop), false, MonitorStatus::Stopped),
        (
            Some(PlaybackCommand::Seek(0)),
            false,
            MonitorStatus::Stopped,
        ),
        (Some(PlaybackCommand::Play), false, MonitorStatus::Stopped),
        (None, false, MonitorStatus::SourceChanged),
        (None, true, MonitorStatus::Stopped),
    ] {
        let (revision, track) = paused_fixture();
        let (mut control, mut callback) = channel(48_000).unwrap();
        publish_paused_fixture_graph(&mut control, Arc::clone(&revision));
        callback.begin_block();
        let source = control.status.recording_source().unwrap();
        let epoch = control.status.transport_epoch.load(Acquire);
        let monitor = PausedTrackMonitor::prepare_on_control(
            Arc::clone(&revision),
            track,
            MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
        )
        .unwrap();
        let (mut input, stream) = test_monitor_source();
        let original = &*stream as *const _;
        assert!(
            control
                .input_monitor
                .publish_paused(stream, monitor, source, epoch)
                .is_ok()
        );

        assert_eq!(render_paused_fixture_block(&mut callback), [[0.0; 2]; 64]);
        assert_eq!(control.input_monitor.status(), MonitorStatus::Priming);
        assert_eq!(input.submit(&[0.25; 512]), 2);
        let audible = render_paused_fixture_block(&mut callback);
        assert!(audible.iter().all(|frame| frame[0] > 0.0 && frame[1] > 0.0));
        assert_eq!(control.input_monitor.status(), MonitorStatus::Running);
        assert_eq!(control.status.position.load(Acquire), 0);
        assert!(!control.status.playing.load(Acquire));

        control.input_monitor.release_paused();
        if hard_stop {
            control.input_monitor.stop();
            control.input_monitor.release_paused();
        } else if let Some(command) = command {
            control.command(command).unwrap();
        } else {
            // Same project context and prepared allocation, different applied
            // publication. The cursor and prepared pointer alone cannot decide.
            publish_paused_fixture_graph(&mut control, revision);
        }
        assert_eq!(render_paused_fixture_block(&mut callback), [[0.0; 2]; 64]);
        assert_eq!(
            control.input_monitor.status(),
            terminal,
            "monitor terminal for {command:?}, hard stop {hard_stop}"
        );
        assert_eq!(
            control.status.playing.load(Acquire),
            matches!(command, Some(PlaybackCommand::Play))
        );
        assert_eq!(render_paused_fixture_block(&mut callback), [[0.0; 2]; 64]);
        assert_eq!(
            original,
            &*control.input_monitor.reclaim().unwrap() as *const _
        );
    }
}

#[test]
fn paused_input_refuses_the_wrong_prepared_graph_even_with_matching_source_id() {
    let (installed, installed_track) = paused_fixture();
    let (other, track) = paused_fixture();
    let (mut control, mut callback) = channel(48_000).unwrap();
    publish_paused_fixture_graph(&mut control, Arc::clone(&installed));
    callback.begin_block();
    let source = control.status.recording_source().unwrap();
    let epoch = control.status.transport_epoch.load(Acquire);
    let monitor =
        PausedTrackMonitor::prepare_on_control(other, track, MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES)
            .unwrap();
    let (mut input, stream) = test_monitor_source();
    assert_eq!(input.submit(&[0.25; 512]), 2);
    assert!(
        control
            .input_monitor
            .publish_paused(stream, monitor, source, epoch)
            .is_ok()
    );
    let reserved = PausedTrackMonitor::prepare_on_control(
        installed,
        installed_track,
        MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
    )
    .unwrap();
    let (mut replacement_input, replacement_stream) = test_monitor_source();
    let original = &*replacement_stream as *const _;
    let Err(refusal) =
        control
            .input_monitor
            .publish_paused(replacement_stream, reserved, source, epoch)
    else {
        panic!("paused monitor slot must remain occupied")
    };
    assert_eq!(original, &*refusal.stream as *const _);

    assert_eq!(render_paused_fixture_block(&mut callback), [[0.0; 2]; 64]);
    assert_eq!(
        control.input_monitor.status(),
        input_monitor::MonitorStatus::Unsupported
    );
    assert!(!control.status.render_fault.load(Acquire));
    assert_eq!(render_paused_fixture_block(&mut callback), [[0.0; 2]; 64]);
    assert!(control.input_monitor.reclaim().is_some());
    assert_eq!(replacement_input.submit(&[0.25; 512]), 2);
    assert!(
        control
            .input_monitor
            .publish_paused(refusal.stream, refusal.monitor, source, epoch)
            .is_ok()
    );
    let audible = render_paused_fixture_block(&mut callback);
    assert!(audible.iter().all(|frame| frame[0] > 0.0 && frame[1] > 0.0));
}

#[test]
fn monitor_renders_during_bound_recording_and_returns_owner_on_stop() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "input".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let revision = Arc::new(
        hex_engine::PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let playback = Playback::try_from_prepared_with_session(revision).unwrap();
    let (mut control, mut callback) = channel(48_000).unwrap();
    control
        .publish_unaccounted_pcm_obligation(
            playback,
            PublicationPolicy::InstallFreshRuntime,
            7,
            true,
            false,
            Some(hex_engine::AudioSourceContext {
                project_generation: 7,
                editor_revision: 3,
            }),
        )
        .unwrap();
    callback.begin_block();
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    let source = callback.recording_source().unwrap();
    let mut reader = control.sample_recording.start_bound(source).unwrap();
    let mut index = OutputClockIndex::new(&reader);
    assert!(reader.ready());
    let (mut input, stream) = test_monitor_source();
    assert_eq!(input.submit(&[0.25; 512]), 2);
    let original = &*stream as *const _;
    assert!(
        control
            .input_monitor
            .publish(stream, source, hex_engine::OutputCaptureId::Track(track))
            .is_ok()
    );
    let mut output = [[0.0; 2]; 64];
    #[cfg(feature = "allocation-probe")]
    let counts = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
        callback.set_sample_recording_output_clock(Some(OutputClock {
            playback: cpal::StreamInstant::new(10, 0),
            offset_frames: 0,
            graph_latency_frames: None,
            origin: OutputClockOrigin::Unknown,
        }));
        callback.finish_sample_recording(true);
    });
    #[cfg(not(feature = "allocation-probe"))]
    {
        callback.begin_block();
        callback.render(&mut output).unwrap();
        callback.set_sample_recording_output_clock(Some(OutputClock {
            playback: cpal::StreamInstant::new(10, 0),
            offset_frames: 0,
            graph_latency_frames: None,
            origin: OutputClockOrigin::Unknown,
        }));
        callback.finish_sample_recording(true);
    }
    #[cfg(feature = "allocation-probe")]
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert!(output.iter().any(|frame| *frame != [0.0; 2]));
    assert_eq!(
        control.input_monitor.status(),
        input_monitor::MonitorStatus::Running
    );
    assert_eq!(index.poll(&mut reader), Ok(None));
    assert_eq!(index.len(), 1);
    assert!(reader.sources().is_empty());
    control.input_monitor.stop();
    #[cfg(feature = "allocation-probe")]
    {
        let counts = allocation_counter::measure(|| callback.begin_block());
        assert_eq!(
            (
                counts.count_total,
                counts.bytes_total,
                counts.count_current,
                counts.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }
    #[cfg(not(feature = "allocation-probe"))]
    callback.begin_block();
    assert_eq!(
        control.input_monitor.status(),
        input_monitor::MonitorStatus::Stopped
    );
    let returned = control.input_monitor.reclaim().unwrap();
    assert_eq!(original, &*returned as *const _);
    output.fill([1.0; 2]);
    callback.render(&mut output).unwrap();
    assert!(output.iter().all(|frame| *frame == [0.0; 2]));
    assert_eq!(input.submit(&[0.25; 64]), 1);
    callback.set_sample_recording_output_clock(Some(OutputClock {
        playback: cpal::StreamInstant::new(10, 0),
        offset_frames: 64,
        graph_latency_frames: None,
        origin: OutputClockOrigin::Unknown,
    }));
    callback.finish_sample_recording(true);
    assert_eq!(index.poll(&mut reader), Ok(None));
    assert_eq!(index.len(), 2);
}

#[test]
fn pcm_capture_refuses_monitor_without_interrupting_the_take() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "input".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let revision = Arc::new(
        hex_engine::PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let (mut control, mut callback) = channel(48_000).unwrap();
    control
        .publish_unaccounted_pcm_obligation(
            Playback::try_from_prepared_with_session(revision).unwrap(),
            PublicationPolicy::InstallFreshRuntime,
            7,
            true,
            false,
            Some(hex_engine::AudioSourceContext {
                project_generation: 7,
                editor_revision: 3,
            }),
        )
        .unwrap();
    callback.begin_block();
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    let source = callback.recording_source().unwrap();
    let mut reader = control.sample_recording.start(7, &[], true).unwrap();
    assert!(reader.ready());
    let (mut input, stream) = test_monitor_source();
    input.submit(&[0.25; 512]);
    assert!(
        control
            .input_monitor
            .publish(stream, source, hex_engine::OutputCaptureId::Track(track))
            .is_ok()
    );
    callback.begin_block();
    let mut output = [[1.0; 2]; 64];
    callback.render(&mut output).unwrap();
    callback.finish_sample_recording(true);
    assert!(output.iter().all(|frame| *frame == [0.0; 2]));
    assert_eq!(
        control.input_monitor.status(),
        input_monitor::MonitorStatus::Unsupported
    );
    callback.begin_block();
    assert!(control.input_monitor.reclaim().is_some());
    assert_eq!(input.submit(&[0.25; 64]), 1);
    let mut frames = 0;
    assert!(reader.drain(|count, _, _| frames += count).is_none());
    assert_eq!(frames, 64);
}

#[test]
fn priming_and_refusal_preserve_existing_audible_playback() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "audio".into(),
            path: "audio.wav".into(),
            sample_rate: 48_000,
            frames: 1024,
        })
        .unwrap()
    else {
        panic!("imported audio")
    };
    let audio = hex_engine::Audio::new(48_000, vec![[0.5, -0.25]; 1024]).unwrap();
    let revision = Arc::new(
        hex_engine::PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::from([(asset, audio)]),
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut pair = Vec::new();
    for _ in 0..2 {
        let (mut control, mut callback) = channel(48_000).unwrap();
        control
            .publish_unaccounted_pcm_obligation(
                Playback::try_from_prepared_with_session(Arc::clone(&revision)).unwrap(),
                PublicationPolicy::InstallFreshRuntime,
                7,
                true,
                false,
                Some(hex_engine::AudioSourceContext {
                    project_generation: 7,
                    editor_revision: 3,
                }),
            )
            .unwrap();
        callback.begin_block();
        control.command(PlaybackCommand::Play).unwrap();
        callback.begin_block();
        pair.push((control, callback));
    }
    let [
        (reference_control, reference),
        (monitored_control, monitored),
    ] = &mut pair[..]
    else {
        panic!("pair")
    };
    let _ = reference_control;
    let source = monitored.recording_source().unwrap();
    let (mut input, stream) = test_monitor_source();
    assert!(
        monitored_control
            .input_monitor
            .publish(stream, source, hex_engine::OutputCaptureId::Track(track))
            .is_ok()
    );
    monitored.begin_block();
    let mut expected = [[0.0; 2]; 64];
    let mut actual = [[0.0; 2]; 64];
    reference.render(&mut expected).unwrap();
    monitored.render(&mut actual).unwrap();
    assert!(expected.iter().any(|frame| *frame != [0.0; 2]));
    assert_eq!(actual, expected);
    assert_eq!(
        monitored_control.input_monitor.status(),
        input_monitor::MonitorStatus::Priming
    );
    input.submit(&[0.25; 512]);
    let reader = monitored_control
        .sample_recording
        .start(7, &[], true)
        .unwrap();
    assert!(reader.ready());
    reference.begin_block();
    monitored.begin_block();
    reference.render(&mut expected).unwrap();
    monitored.render(&mut actual).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(
        monitored_control.input_monitor.status(),
        input_monitor::MonitorStatus::Unsupported
    );
}
