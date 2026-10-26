//! Opt-in diagnostic behavior and explicit optimized overhead probe.
use super::*;
#[test]
fn audio_diagnostics_preserve_pcm_commands_and_callback_bounds_when_full() {
    fn run(flags: AudioDiagnosticsFlags) -> Vec<[f32; 2]> {
        let (mut control, mut callback) = channel(8_000).unwrap();
        control.set_audio_diagnostics(flags);
        let revision = control.publish(super::tests::metronome_graph()).unwrap();
        let ticket = control
            .action_with_ticket(PlaybackAction::SetMetronome(true))
            .unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        let mut pcm = vec![[0.0; 2]; 512];
        let measured = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut pcm).unwrap();
            // Deliberately stop draining: trace pressure must never delay
            // music/Stop or allocate/free, even beyond the ring capacity.
            for _ in 0..audio_diagnostics::TRACE_CAPACITY + 10 {
                callback.trace_callback_timing(100, 1_000, 512);
            }
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(control.status.applied_revision.load(Acquire), revision);
        assert!(control.status.consumed_command.load(Acquire) >= ticket);
        if flags.enabled() {
            assert!(control.status.audio_diagnostics_dropped() > 0);
            let events: Vec<_> = std::iter::from_fn(|| control.take_audio_diagnostic()).collect();
            assert_eq!(events.len(), audio_diagnostics::TRACE_CAPACITY);
            assert!(
                events
                    .windows(2)
                    .all(|pair| pair[0].sequence + 1 == pair[1].sequence)
            );
            assert!(events.iter().any(
                |e| e.revision == revision && e.kind == AudioDiagnosticKind::PublicationApplied
            ));
            assert!(events.iter().any(|e| matches!(e.kind, AudioDiagnosticKind::CommandDequeued { ticket: t, .. } if t == ticket)));
            assert!(
                events
                    .iter()
                    .all(|e| e.output == control.output_identity().unwrap())
            );
        } else {
            assert_eq!(control.status.audio_diagnostics_dropped(), 0);
            assert!(control.take_audio_diagnostic().is_none());
        }
        control.set_audio_diagnostics(AudioDiagnosticsFlags::TRANSPORT);
        callback.trace_callback_timing(100, 1_000, 512);
        assert!(control.take_audio_diagnostic().is_none());
        control.stop();
        let measured = allocation_counter::measure(|| callback.begin_block());
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert!(!control.status.playing.load(Acquire));
        assert!(
            std::iter::from_fn(|| control.take_audio_diagnostic())
                .any(|event| matches!(event.kind, AudioDiagnosticKind::PriorityStop { .. }))
        );

        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
        pcm
    }
    let plain = run(AudioDiagnosticsFlags::OFF);
    assert!(plain.iter().any(|frame| frame[0] != 0.0));
    assert_eq!(plain, run(AudioDiagnosticsFlags::ALL));
}

/// Isolated headless observation, never a native deadline or full DAW benchmark.
#[test]
#[ignore = "explicit optimized diagnostic overhead measurement"]
fn audio_diagnostics_optimized_callback_overhead() {
    const BLOCKS: usize = 4096;
    const WARMUP: usize = 256;
    const FRAMES: usize = 128;
    fn run(flags: AudioDiagnosticsFlags, saturated: bool) -> [u128; 4] {
        let audio = hex_engine::Audio::new(48_000, vec![[0.25, -0.125]]).unwrap();
        let clip =
            hex_engine::Clip::new(audio, 0, ((BLOCKS + WARMUP) * FRAMES) as u64, true).unwrap();
        let track = hex_engine::Track::new(vec![clip], 1.0, false).unwrap();
        let project = Arc::new(hex_engine::Project::new(48_000, vec![track]).unwrap());
        let (mut control, mut callback) = channel(48_000).unwrap();
        control.set_audio_diagnostics(flags);
        control.publish(Playback::from_shared(project)).unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        let mut pcm = [[0.0; 2]; FRAMES];
        let mut times = Vec::with_capacity(BLOCKS);
        for block in 0..BLOCKS + WARMUP {
            let start = std::time::Instant::now();
            callback.begin_block();
            callback.render(&mut pcm).unwrap();
            callback.trace_callback_timing(start.elapsed().as_nanos() as u64, 2_666_666, FRAMES);
            let elapsed = start.elapsed().as_nanos();
            if block >= WARMUP {
                times.push(elapsed);
            }
            // Check every audible sample; draining and oracles stay outside timing.
            assert!(pcm.iter().all(|frame| *frame == [0.25, -0.125]));
            if !saturated {
                while control.take_audio_diagnostic().is_some() {}
            }
        }
        assert_eq!(
            control.status.audio_diagnostics_dropped() > 0,
            saturated && flags.enabled()
        );
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
        times.sort_unstable();
        [
            times[BLOCKS / 2],
            times[BLOCKS * 95 / 100],
            times[BLOCKS * 99 / 100],
            times[BLOCKS - 1],
        ]
    }
    for round in 0..5 {
        // Alternate order to expose warm-up/order noise rather than hide it.
        let cases = if round % 2 == 0 {
            [
                (AudioDiagnosticsFlags::OFF, false),
                (AudioDiagnosticsFlags::ALL, false),
                (AudioDiagnosticsFlags::ALL, true),
            ]
        } else {
            [
                (AudioDiagnosticsFlags::ALL, true),
                (AudioDiagnosticsFlags::ALL, false),
                (AudioDiagnosticsFlags::OFF, false),
            ]
        };
        for (flags, saturated) in cases {
            eprintln!(
                "audio_diag_overhead round={round} flags={flags:?} saturated={saturated} rate=48000 frames={FRAMES} warmup={WARMUP} blocks={BLOCKS} ns_p50_p95_p99_max={:?}",
                run(flags, saturated)
            );
        }
    }
}
