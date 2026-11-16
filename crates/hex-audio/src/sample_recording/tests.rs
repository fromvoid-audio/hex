use super::*;
use crate::{device::finish_host_chunk, prepared};
use hex_engine::{Audio, PlaybackCommand, PreparedRevision, SineTable, playback::Playback};
use hex_project::{Command, Created, Editor, Project};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    time::{Duration, Instant},
};

fn two_tracks() -> (Playback, [OutputCaptureId; 2]) {
    tracks_with_timing(false)
}

fn tracks_with_timing(timing: bool) -> (Playback, [OutputCaptureId; 2]) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let mut tracks = Vec::new();
    let mut media = BTreeMap::new();
    for (name, value) in [("left", [0.25, 0.0]), ("right", [0.0, -0.5])] {
        let Created::Imported { track, asset, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: name.into(),
                path: format!("{name}.wav").into(),
                sample_rate: 48_000,
                frames: 512,
            })
            .unwrap()
        else {
            panic!("expected imported track");
        };
        editor.execute(Command::Gain { track, gain: 1.0 }).unwrap();
        tracks.push(Track(track));
        media.insert(asset, Audio::new(48_000, vec![value; 512]).unwrap());
    }
    if timing {
        editor
            .execute(Command::SetProjectTempo {
                tempo: hex_project::Tempo::new(120_000_000).unwrap(),
            })
            .unwrap();
        editor
            .execute(Command::SetTempoPoint {
                beat: hex_project::BeatTime::from_raw(1 << 24), // frame 94
                tempo: hex_project::Tempo::new(60_000_000).unwrap(),
            })
            .unwrap();
        editor
            .execute(Command::SetLoopRegion {
                start: 32,
                end: 160,
                enabled: true,
            })
            .unwrap();
    }
    let prepared =
        PreparedRevision::prepare(editor.project(), &media, Arc::new(SineTable::new()), None)
            .unwrap();
    (
        Playback::try_from_prepared_with_session(Arc::new(prepared)).unwrap(),
        tracks.try_into().unwrap(),
    )
}

fn deliver(callback: &mut prepared::Callback, channels: usize) -> bool {
    deliver_with_output_clock(callback, channels, None)
}

fn deliver_with_output_clock(
    callback: &mut prepared::Callback,
    channels: usize,
    clock: Option<OutputClock>,
) -> bool {
    let mut pcm = [[0.0; 2]; 64];
    let mut device = [0.0; 128];
    let mut render = || {
        callback.begin_block();
        let rendered = callback.render(&mut pcm);
        assert!(rendered.is_ok(), "{rendered:?}");
        callback.set_sample_recording_output_clock(clock);
        finish_host_chunk(
            callback,
            &mut device,
            &mut pcm,
            channels,
            &rendered,
            0,
            Instant::now(),
        )
    };
    #[cfg(feature = "allocation-probe")]
    {
        let mut result = false;
        let measured = allocation_counter::measure(|| result = render());
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        result
    }
    #[cfg(not(feature = "allocation-probe"))]
    render()
}

#[test]
fn confirmed_output_clock_follows_timing_and_is_cleared_when_packets_recycle() {
    let (graph, outputs) = tracks_with_timing(true);
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    control
        .publish_with_policy_and_context_generation(
            graph,
            prepared::PublicationPolicy::InstallFreshRuntime,
            7,
        )
        .unwrap();
    let mut reader = control.sample_recording.start(7, &outputs, false).unwrap();
    assert!(reader.ready());
    control.command(PlaybackCommand::Play).unwrap();
    let playback = cpal::StreamInstant::new(10, 123);
    for offset in 0..PACKETS {
        assert!(deliver_with_output_clock(
            &mut callback,
            2,
            Some(OutputClock {
                playback,
                offset_frames: (offset * 64) as u32,
                graph_latency_frames: Some(0),
            })
        ));
    }
    let mut seen = 0;
    reader.drain_with_output_clock(|frames, _, _, timing, clock| {
        assert_eq!(frames, 64);
        assert_eq!(
            clock,
            Some(OutputClock {
                playback,
                offset_frames: (seen * 64) as u32,
                graph_latency_frames: Some(0),
            })
        );
        assert!(!timing.is_empty());
        seen += 1;
    });
    assert_eq!(seen, PACKETS);

    // The next capture reuses a packet that previously held a timestamp.
    assert!(deliver(&mut callback, 2));
    reader.drain_with_output_clock(|_, _, _, _, clock| {
        assert_eq!(clock, None);
    });

    // A timestamp staged for a refused output must never be published.
    assert!(!deliver_with_output_clock(
        &mut callback,
        0,
        Some(OutputClock {
            playback,
            offset_frames: 0,
            graph_latency_frames: Some(0),
        })
    ));
    let mut rejected = 0;
    let end = reader.drain_with_output_clock(|_, _, _, _, _| rejected += 1);
    assert_eq!(rejected, 0);
    assert_eq!(end, Some(End::OutputFailed));
}

#[test]
fn output_clock_index_keeps_confirmed_packets_gaps_and_terminal() {
    let (graph, outputs) = tracks_with_timing(true);
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    control
        .publish_with_policy_and_context_generation(
            graph,
            prepared::PublicationPolicy::InstallFreshRuntime,
            7,
        )
        .unwrap();
    let mut reader = control.sample_recording.start(7, &outputs, false).unwrap();
    let mut index = OutputClockIndex::new(&reader);
    assert!(reader.ready());
    let playback = cpal::StreamInstant::new(10, 0);
    assert!(deliver_with_output_clock(
        &mut callback,
        2,
        Some(OutputClock {
            playback,
            offset_frames: 0,
            graph_latency_frames: Some(0),
        }),
    )); // stopped: the packet is an explicit timeline gap
    control.command(PlaybackCommand::Play).unwrap();
    for number in 1..=PACKETS {
        assert!(deliver_with_output_clock(
            &mut callback,
            2,
            Some(OutputClock {
                playback,
                offset_frames: (number * 64) as u32,
                graph_latency_frames: Some(0),
            }),
        ));
        assert_eq!(index.poll(&mut reader).unwrap(), None);
    }
    assert_eq!(index.sample_rate(), 48_000);
    assert_eq!(index.len(), PACKETS + 1);
    assert!(index.packet(0).unwrap().timing.is_empty());
    assert!(!index.packet(1).unwrap().timing.is_empty());
    let confirmed = index.packet(1).unwrap();
    let span = confirmed.timing[0];
    let window = crate::input_capture::OutputWindow::from_recorded(
        confirmed.clock,
        confirmed.frames as usize,
        span,
    )
    .unwrap();
    let mapped = crate::input_capture::timeline_overlap(
        crate::input_capture::SharedClock,
        48_000,
        index.sample_rate(),
        crate::input_capture::InputTiming {
            take_start: 0,
            frames: 64,
            captured_at: playback,
            offset_frames: 64,
        },
        window,
    )
    .unwrap()
    .unwrap();
    assert_eq!(mapped.take_start, span.take_start);
    assert_eq!(mapped.timeline_start, span.timeline_start);
    assert_eq!(u64::from(mapped.frames), span.take_end - span.take_start);
    assert_eq!(
        index.packet(PACKETS).unwrap().clock.offset_frames,
        (PACKETS * 64) as u32
    );
    assert!(index.packet(PACKETS + 1).is_none());
    control.sample_recording.stop();
    assert!(deliver(&mut callback, 2));
    assert_eq!(index.poll(&mut reader).unwrap(), Some(End::Stopped));
    assert_eq!(index.poll(&mut reader).unwrap(), Some(End::Stopped));
    let take = crate::input_capture::InputTake {
        path: PathBuf::new(),
        input_device: format!("{}:test-input", cpal::default_host().id())
            .parse()
            .unwrap(),
        input_config: crate::input_capture::InputConfig {
            sample_rate: 48_000,
            channels: 1,
        },
        sample_rate: 48_000,
        frames: 128,
        timing: vec![crate::input_capture::InputTiming {
            take_start: 0,
            frames: 128,
            captured_at: playback,
            offset_frames: 64,
        }],
        end: crate::input_capture::InputTakeEnd::Stopped,
        open_error: None,
    };
    assert_eq!(
        crate::input_capture::align_complete_take(crate::input_capture::SharedClock, &take, &index),
        Ok(crate::input_capture::CandidatePlacement {
            timeline_start: span.timeline_start,
            frames: 128
        })
    );
}

struct Directory(PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn confirmed_pcm_writes_each_armed_source_as_a_separate_take() {
    let path = Directory(std::env::temp_dir().join(
        format!("hex-sample-takes-{}-{}", std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()),
    ));
    let (graph, outputs) = two_tracks();
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    control
        .publish_with_policy_and_context_generation(
            graph,
            prepared::PublicationPolicy::InstallFreshRuntime,
            7,
        )
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let reader = control.sample_recording.start(7, &outputs, false).unwrap();
    let audition = control.sample_preview.request().unwrap();
    control
        .sample_preview
        .publish_loop(audition, Audio::new(48_000, vec![[0.125; 2]; 3]).unwrap())
        .unwrap();
    let state = reader.state.clone();
    let worker_path = path.0.clone();
    let worker = std::thread::spawn(move || write_takes(reader, &worker_path).unwrap());
    let deadline = Instant::now() + Duration::from_millis(500);
    while state.load(Acquire) == PENDING {
        assert!(Instant::now() < deadline, "take writer did not prepare");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(deliver(&mut callback, 2));
    assert_eq!(
        control.sample_preview.snapshot().phase,
        crate::sample_preview::Phase::Playing
    );
    assert!(deliver(&mut callback, 2));
    assert!(!deliver(&mut callback, 0));
    let batch = worker.join().unwrap();
    assert_eq!(batch.end, End::OutputFailed);
    assert!(batch.write_error.is_none());
    assert_eq!(batch.takes.len(), 2);
    for (take, value) in batch.takes.iter().zip([[0.25, 0.0], [0.0, -0.5]]) {
        assert_eq!(take.frames, 128);
        let wav =
            hex_media::decode_wav_file(&take.path, hex_media::Limits::new(8192, 512)).unwrap();
        assert_eq!(wav.sample_rate(), 48_000);
        assert_eq!(wav.audio().frames(), &[value; 128]);
    }
}

#[test]
fn overflow_preserves_a_full_prefix_and_stop_stays_available_before_first_chunk() {
    let (mut control, mut callback) = channel(48_000);
    let mut reader = control.start(1, &[], true).unwrap();
    assert!(reader.ready());
    assert_eq!(control.phase(), Some(Phase::Preparing));
    for _ in 0..PACKETS {
        callback.begin(1);
        let (layout, stage) = callback.observation().unwrap();
        stage.observe(layout, ProjectSignal::Master, 0, &[[0.5; 2]]);
        callback.finish(true);
    }
    assert_eq!(control.phase(), Some(Phase::Capturing));
    callback.begin(1);
    assert_eq!(control.phase(), Some(Phase::Finishing));
    let mut confirmed = 0;
    assert_eq!(
        reader.drain(|frames, _, pcm| {
            confirmed += frames;
            assert_eq!(Reader::source_frames(pcm, frames, 0), Some(&[[0.5; 2]][..]));
        }),
        Some(End::Backpressure)
    );
    assert_eq!(confirmed, PACKETS);
    let mut next = control.start(1, &[], true).unwrap();
    control.stop();
    assert!(!next.ready());
    callback.begin(1);
    assert_eq!(
        next.drain(|_, _, _| panic!("stopped take contains PCM")),
        Some(End::Stopped)
    );
}

#[test]
fn source_replacement_ends_the_take_before_replacement_audio() {
    let (graph, outputs) = two_tracks();
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    control
        .publish_with_policy_and_context_generation(
            graph,
            prepared::PublicationPolicy::InstallFreshRuntime,
            7,
        )
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut reader = control.sample_recording.start(7, &outputs, true).unwrap();
    assert!(reader.ready());
    assert!(deliver(&mut callback, 2));
    let (replacement, _) = two_tracks();
    control
        .publish_with_policy_and_context_generation(
            replacement,
            prepared::PublicationPolicy::InstallFreshRuntime,
            8,
        )
        .unwrap();
    assert!(deliver(&mut callback, 2));
    let mut confirmed = 0;
    assert_eq!(
        reader.drain(|frames, _, _| confirmed += frames),
        Some(End::SourceChanged)
    );
    assert_eq!(confirmed, 64);
}

#[test]
fn confirmed_timing_splits_at_tempo_changes_loop_wraps_and_stops() {
    let (graph, outputs) = tracks_with_timing(true);
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    control
        .publish_with_policy_and_context_generation(
            graph,
            prepared::PublicationPolicy::InstallFreshRuntime,
            7,
        )
        .unwrap();
    let mut reader = control.sample_recording.start(7, &outputs, false).unwrap();
    assert!(reader.ready());
    assert!(deliver(&mut callback, 2)); // stopped PCM, no beat
    control.command(PlaybackCommand::Play).unwrap();
    for _ in 0..3 {
        assert!(deliver(&mut callback, 2));
    }
    control.command(PlaybackCommand::Stop).unwrap();
    // The transport still runs for the existing 128-frame declick, then
    // delivers a stopped callback without a clock.
    for _ in 0..3 {
        assert!(deliver(&mut callback, 2));
    }
    let mut cursor = 0;
    let mut timing = Vec::new();
    reader.drain_with_timing(|frames, _, _, packet| {
        for span in packet {
            super::timing::append(
                &mut timing,
                TimingSpan {
                    take_start: cursor + span.take_start,
                    take_end: cursor + span.take_end,
                    ..*span
                },
                super::timing::MAX_SPANS,
            );
        }
        cursor += frames as u64;
    });
    assert_eq!(cursor, 448);
    assert_eq!(
        timing
            .iter()
            .map(|span| (span.take_start, span.take_end, span.timeline_start))
            .collect::<Vec<_>>(),
        [
            (64, 158, 0),
            (158, 224, 94),
            (224, 286, 32),
            (286, 352, 94),
            (352, 384, 32)
        ]
    );
    assert_eq!(timing[0].frame_at_beat(0), Some(64));
    assert_eq!(timing[1].frame_at_beat(1 << 24), Some(158));
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn full_disk_mid_packet_preserves_the_common_recorded_prefix() {
    let parent = Directory(std::env::temp_dir().join(format!(
        "hex-sample-full-disk-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
    )));
    std::fs::create_dir(&parent.0).unwrap();
    let project_path = parent.0.join("project.hex");
    let media_path = parent.0.join("original.wav");
    std::fs::write(&project_path, b"original project").unwrap();
    std::fs::write(&media_path, b"original media").unwrap();
    let (_, outputs) = two_tracks();
    let (mut control, mut callback) = channel(48_000);
    let reader = control.start(1, &outputs, false).unwrap();
    let state = reader.state.clone();
    let destination = parent.0.join("new-takes");
    let (result_tx, result_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        takes::write_fault::arm();
        let _ = result_tx.send(write_takes(reader, &destination));
    });
    let deadline = Instant::now() + Duration::from_millis(500);
    while state.load(Acquire) == PENDING {
        assert!(Instant::now() < deadline, "take writer did not prepare");
        std::thread::yield_now();
    }
    for _ in 0..2 {
        callback.begin(64);
        let (layout, stage) = callback.observation().unwrap();
        for (output, value) in outputs.iter().zip([[0.25, 0.0], [0.0, -0.5]]) {
            let OutputCaptureId::Track(id) = output else {
                panic!("expected track")
            };
            stage.observe(layout, ProjectSignal::Track(id.value()), 0, &[value; 64]);
        }
        stage.observe(layout, ProjectSignal::Master, 0, &[[0.0; 2]; 64]);
        callback.finish(true);
    }
    let result = match result_rx.recv_timeout(Duration::from_millis(500)) {
        Ok(result) => result,
        Err(error) => {
            control.stop();
            callback.begin(0);
            worker.join().unwrap();
            panic!("recording worker did not report the write failure: {error}");
        }
    };
    worker.join().unwrap();
    let batch = result.unwrap();
    assert_eq!(batch.end, End::StorageFailed);
    assert_eq!(
        batch.write_error.unwrap().raw_os_error(),
        Some(rustix::io::Errno::NOSPC.raw_os_error())
    );
    assert_eq!(batch.takes.len(), 2);
    for (take, value) in batch.takes.iter().zip([[0.25, 0.0], [0.0, -0.5]]) {
        assert_eq!(take.frames, 64);
        let wav =
            hex_media::decode_wav_file(&take.path, hex_media::Limits::new(8192, 512)).unwrap();
        assert_eq!(wav.audio().frames(), &[value; 64]);
        assert_eq!(std::fs::metadata(&take.path).unwrap().len(), 44 + 64 * 8);
    }
    assert_eq!(std::fs::read(project_path).unwrap(), b"original project");
    assert_eq!(std::fs::read(media_path).unwrap(), b"original media");
}
