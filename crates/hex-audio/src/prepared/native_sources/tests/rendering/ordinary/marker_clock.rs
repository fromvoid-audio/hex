//! Actual installed native producer and one confirmed Main reader. Only the
//! output timestamp is injected; no physical-route prediction is established.
use super::*;
use crate::sample_recording::{End, OutputClock, OutputClockOrigin, Source};
use std::path::PathBuf;

const QUANTUM: usize = 8;
const CALLBACK_NS: u128 = 2_000_000_000_000_000_000;
const EXPECTED: Binding = Binding {
    root: 1,
    instance: 2,
    graph: 12,
    epoch: 1,
};

struct Directory(PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn frame_ns(frames: u64) -> u128 {
    u128::from(frames) * 1_000_000_000 / 48_000
}
fn instant(nanos: u128) -> cpal::StreamInstant {
    cpal::StreamInstant::new(
        i64::try_from(nanos / 1_000_000_000).unwrap(),
        u32::try_from(nanos % 1_000_000_000).unwrap(),
    )
}
fn as_ns(instant: cpal::StreamInstant) -> u128 {
    instant
        .duration_since(&cpal::StreamInstant::new(0, 0))
        .unwrap()
        .as_nanos()
}
fn marker_time(clock: OutputClock, local_frame: usize) -> Option<u128> {
    (clock.origin == OutputClockOrigin::CoreAudioHostTime).then(|| {
        as_ns(clock.playback) + frame_ns(u64::from(clock.offset_frames) + local_frame as u64)
    })
}

fn recorded_block(
    f: &mut OrdinaryFixture,
    number: u64,
    origin: OutputClockOrigin,
) -> [Frame; QUANTUM] {
    let mut output = [[0.0; 2]; QUANTUM];
    let mut host = [0.0_f32; QUANTUM * 2];
    // Mirror CPAL's supported fixed-rate prediction: callback time plus one
    // device buffer. This is injected data, not a real Core Audio timestamp.
    let info = cpal::OutputCallbackInfo::new(cpal::OutputStreamTimestamp {
        callback: instant(CALLBACK_NS + frame_ns(number * QUANTUM as u64)),
        playback: instant(CALLBACK_NS + frame_ns((number + 1) * QUANTUM as u64)),
    });
    allocation_free(|| {
        f.data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            callback.begin_native_effect_callback(QUANTUM).unwrap();
            let rendered = callback.render(&mut output);
            assert!(rendered.is_ok());
            callback.set_sample_recording_output_clock(Some(OutputClock {
                playback: info.timestamp().playback,
                offset_frames: 0,
                graph_latency_frames: None,
                origin,
            }));
            assert!(crate::device::finish_host_chunk(
                callback,
                &mut host,
                &mut output,
                2,
                &rendered,
                number * QUANTUM as u64,
                Instant::now()
            ));
        })
    });
    replacement::wait_for_blocks(&f.control.native_candidate_root(), f.binding, number + 1);
    output
}

#[test]
#[ignore = "explicit signed slice-clock observation profile and original SDK receipts required"]
fn installed_sdk_marker_joins_bound_main_pcm_with_an_injected_output_clock() {
    const CHILD: &str = "HEX_INSTALLED_SDK_MARKER_CLOCK_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let directory = Directory(std::env::temp_dir().join(
            format!("hex-installed-sdk-marker-{}-{}",
            std::process::id(), std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()),
        ));
        std::fs::create_dir(&directory.0).unwrap();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                std::thread::current().name().unwrap(),
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD, "1")
            .env("RUST_MIN_STACK", "16777216")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "127")
            .env("HEX_ORIGINAL_SLICE_CLOCK_TARGET", "1,2,12,1,0,0")
            .env("HEX_ORIGINAL_SLICE_CLOCK_PATH", directory.0.join("clock"))
            .env_remove("HEX_ORIGINAL_SDK_TIMING_GATE")
            .env_remove("HEX_ORIGINAL_INSTRUMENT_LIVE_PRESET")
            .output()
            .unwrap();
        assert!(
            result.status.success() && String::from_utf8_lossy(&result.stdout).contains("1 passed"),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        print!("{}", String::from_utf8_lossy(&result.stdout));
        return;
    }
    let _serial = serial();
    let mut f = fixture_with_authored_mode("processing", 0, QUANTUM as u16, &[], true);
    // This isolated child has exactly one Root and one native source. Assert
    // the fully armed tuple; never infer identity from a matching waveform.
    assert_eq!(f.binding, EXPECTED);
    assert!(
        f.media.is_empty(),
        "the marker must come from the installed SDK, not resident replay"
    );
    let clip = &f.editor.project().tracks()[0].midi_clips()[0];
    let (clip_id, note) = (clip.id(), clip.notes()[0].id());
    f.editor
        .execute(Command::UpdateMidiNote {
            note,
            start: hex_project::BeatTime::from_ratio(1, 24_000).unwrap(),
            duration: hex_project::BeatTime::from_ratio(2, 24_000).unwrap(),
            pitch: 60,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    f.editor
        .execute(Command::AddMidiNote {
            clip: clip_id,
            start: hex_project::BeatTime::from_ratio(33, 24_000).unwrap(),
            duration: hex_project::BeatTime::from_ratio(2, 24_000).unwrap(),
            pitch: 60,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    let playback = compile(&f, true);
    assert_eq!(
        playback
            .prepared_revision()
            .unwrap()
            .arrangement()
            .main_latency_frames(),
        16
    );
    let revision = publish(
        &mut f.control,
        playback,
        PublicationPolicy::InstallFreshRuntime,
    );
    let source = AppliedAudioSource {
        context: context(),
        audio_revision: revision,
        output_identity: f.binding.root,
    };
    f.data.owner_mut().unwrap().run(|callback, _| {
        callback.begin_block();
        // Ordinary publication retains a stream envelope even with no pages.
        // Drive its render-side installation without advancing the SDK request.
        callback.render(&mut []).unwrap();
        assert_eq!(callback.recording_source(), Some(source));
    });
    assert_eq!(f.control.status.recording_source(), Some(source));
    assert_eq!(
        f.control.native_source_endpoint_installed(f.binding),
        Some(true)
    );
    let mut reader = f
        .control
        .sample_recording
        .start_bound_main_for_test(source)
        .unwrap();
    assert_eq!(reader.source_binding(), Some(source));
    assert!(reader.ready());
    f.control.command(PlaybackCommand::Play).unwrap();
    let mut rendered = Vec::new();
    for number in 0..8 {
        rendered.extend(recorded_block(
            &mut f,
            number,
            if number == 6 {
                OutputClockOrigin::Unknown
            } else {
                OutputClockOrigin::CoreAudioHostTime
            },
        ));
        assert_eq!(
            f.control.status.position.load(Acquire),
            (number + 1) * QUANTUM as u64
        );
    }
    let mut expected = [[0.0; 2]; 64];
    for frame in [17, 18, 49, 50] {
        expected[frame] = [1.0; 2];
    }
    assert_eq!(rendered.as_slice(), expected);
    assert_eq!(f.control.status.recording_source(), Some(source));
    reader.stop();
    recorded_block(&mut f, 8, OutputClockOrigin::CoreAudioHostTime);
    let mut packets = 0;
    let mut marker_predictions = Vec::new();
    assert_eq!(
        reader.drain_with_output_clock(|frames, sources, pcm, _, clock| {
            assert_eq!(sources, &[Source::Main]);
            assert_eq!(frames, QUANTUM);
            assert_eq!(
                &pcm[..frames],
                &expected[packets * QUANTUM..(packets + 1) * QUANTUM]
            );
            let clock = clock.unwrap();
            assert_eq!(clock.graph_latency_frames, Some(16));
            for local in 0..frames {
                if pcm[local] != [1.0; 2] {
                    continue;
                }
                // This join uses the source-bound output packet and its sample
                // offset. The playing cursor was checked for every block.
                marker_predictions.push((packets * QUANTUM + local, marker_time(clock, local)));
            }
            packets += 1;
        }),
        Some(End::Stopped)
    );
    assert_eq!(packets, 8);
    assert_eq!(marker_predictions.len(), 4);
    assert_eq!(
        marker_predictions[0],
        (17, Some(CALLBACK_NS + frame_ns(24) + frame_ns(1)))
    );
    assert_eq!(
        marker_predictions[2..],
        [(49, None), (50, None)],
        "unknown clock has no join"
    );
    // PCM already traversed the graph and CPAL prediction already contains its
    // buffer. Neither the graph's 16 nor another device buffer belongs here.
    assert_ne!(
        marker_predictions[0].1.unwrap(),
        CALLBACK_NS + frame_ns(24) + frame_ns(1) + frame_ns(8)
    );
    f.control.sample_recording.reclaim();
    let stale = AppliedAudioSource {
        audio_revision: source.audio_revision + 1,
        ..source
    };
    let mut refused = f
        .control
        .sample_recording
        .start_bound_main_for_test(stale)
        .unwrap();
    assert!(refused.ready());
    recorded_block(&mut f, 9, OutputClockOrigin::CoreAudioHostTime);
    let mut refused_packets = 0;
    assert_eq!(
        refused.drain_with_output_clock(|_, _, _, _, _| refused_packets += 1),
        Some(End::SourceChanged)
    );
    assert_eq!(
        refused_packets, 0,
        "mismatched source cannot publish PCM or timestamps"
    );
    let OrdinaryFixture {
        mut control,
        data,
        errors,
        retirement,
        ..
    } = f;
    let mut retirement = close_backend(data, errors, retirement);
    let returned = recover(&mut control, EXPECTED);
    assert!(returned.error.is_none() && returned.cleanup_error.is_none());
    assert!(
        matches!(returned.disposal, native::PreparationDisposal::Reaped(value) if value.status.success())
    );
    drop(control);
    finish(&mut retirement);
    let bytes = std::fs::read(std::env::var_os("HEX_ORIGINAL_SLICE_CLOCK_PATH").unwrap()).unwrap();
    assert_eq!(bytes.len(), 144);
    assert_eq!(&bytes[..8], b"HXMC0001");
    let mut fields = [0_u64; 17];
    for (field, bytes) in fields.iter_mut().zip(bytes[8..].chunks_exact(8)) {
        *field = u64::from_le_bytes(bytes.try_into().unwrap());
    }
    assert_eq!(
        &fields[..9],
        &[
            EXPECTED.root,
            EXPECTED.instance,
            EXPECTED.graph,
            EXPECTED.epoch,
            0,
            0,
            0,
            0,
            8
        ]
    );
    assert!(fields[9] > 0 && fields[10] >= fields[9]);
    assert!(fields[11] > 0 && fields[12] > 0);
    assert_eq!(&fields[14..], &[1, 1, 1]);
    let nanos = |ticks| u128::from(ticks) * u128::from(fields[11]) / u128::from(fields[12]);
    let predicted = marker_predictions[0].1.unwrap();
    let bracket = (
        predicted.checked_sub(nanos(fields[10])).unwrap(),
        predicted.checked_sub(nanos(fields[9])).unwrap(),
    );
    assert!(bracket.0 <= bracket.1);
    println!(
        "installed_sdk_marker,binding={EXPECTED:?},source={source:?},marker_frame=17,fields={fields:?},injected_bracket_ns={bracket:?},physical_route=false"
    );
}
