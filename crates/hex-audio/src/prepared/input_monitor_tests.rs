use super::*;
use crate::input_capture::test_monitor_source;
use crate::sample_recording::{OutputClock, OutputClockIndex, OutputClockOrigin};
use hex_project::{Command, Created, Editor, Project};
use std::collections::BTreeMap;

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
