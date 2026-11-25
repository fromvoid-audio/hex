//! Endpoint fault containment on real copied transports with fake workers
//! (#3977). No SDK instance is involved; workers echo their request blocks.
use super::*;
use hex_lattice::ProcessorChainOwner;
use hex_plugin_host::transport::{Control, Worker, channel};

const QUANTUM: usize = 4;
/// Both endpoints declare only the copied 2Q bridge latency.
const LATENCY_CALLBACKS: usize = 2;

crate::include_vst3_sdk_tests! {
#[test]
#[ignore = "requires explicit source-bound original SDK receipts"]
fn actual_foreign_instrument_notes_follow_accepted_callback_segments() {
    const NAME: &str = "plugin_effects::callbacks::tests::actual_foreign_instrument_notes_follow_accepted_callback_segments";
    const CHILD: &str = "HEX_FOREIGN_INSTRUMENT_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", NAME, "--ignored"])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "115")
            .output()
            .unwrap();
        assert!(
            result.status.success() && String::from_utf8_lossy(&result.stdout).contains("1 passed"),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }

    let binding = Binding {
        root: 1,
        instance: 2,
        graph: 3,
        epoch: 4,
    };
    let identity = ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, 2).unwrap();
    let source = crate::plugin_worker::native_tests::original_instance_with_inputs(binding, 0);
    let effect = NativeEffect::prepare_on_control(identity, source).unwrap();
    let facts = Facts {
        identity,
        state: effect.configuration().clone(),
        prepared: effect.prepared(),
    };
    let (mut workers, mut endpoints) =
        NativeEffectCallbacks::from_effects_on_control(vec![effect]).unwrap();
    let worker = &mut workers[0];
    let on = ForeignNote {
        frame: 1,
        channel: 2,
        pitch: 60,
        value: 127,
        kind: ForeignNoteKind::On,
        note_id: std::num::NonZeroU32::new(70_001),
        is_live: false,
    };
    let second_on = ForeignNote {
        frame: 3,
        note_id: std::num::NonZeroU32::new(70_002),
        ..on
    };
    let first_off = ForeignNote {
        value: 0,
        kind: ForeignNoteKind::Off,
        ..on
    };
    let second_off = ForeignNote {
        frame: 2,
        value: 0,
        kind: ForeignNoteKind::Off,
        ..second_on
    };
    struct Clock(u64, bool);
    impl hex_lattice::NativeInstrumentClock for Clock {
        fn beat_at_offset(&self, _: usize) -> Option<u64> {
            None
        }
        fn arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
            self.0.checked_add(offset as u64)
        }
        fn transport_is_running(&self) -> Option<bool> {
            Some(self.1)
        }
    }
    // Fill the expanded input budget without exceeding the fixture's two
    // simultaneous voices. The final two onsets must retain their original PCM.
    let mut dense = Vec::new();
    for identity in 1..64 {
        let note = ForeignNote {
            note_id: std::num::NonZeroU32::new(identity),
            ..on
        };
        dense.push(note);
        dense.push(ForeignNote {
            kind: ForeignNoteKind::Off,
            value: 0,
            ..note
        });
    }
    dense.extend([on, second_on]);
    assert_eq!(dense.len(), 128);
    let mut render = |index: usize, notes: &[ForeignNote], accept| {
        let mut output = [[1.0; 2]; 8];
        let origin = (index as u64 * 8).saturating_sub(3);
        let first_clock = Clock(origin, index != 0);
        let last_clock = Clock(if index == 0 { 0 } else { origin + 3 }, true);
        let mut callback = || {
            endpoints.begin_host_callback(8).unwrap();
            let (first, last) = output.split_at_mut(3);
            endpoints
                .process_segment_with_notes(facts.node(), 0, first, &[], &[], Some(&first_clock))
                .unwrap();
            endpoints
                .process_segment_with_notes(facts.node(), 3, last, &[], notes, Some(&last_clock))
                .unwrap();
            assert_eq!(endpoints.finish_host_callback(accept).is_ok(), accept);
        };
        #[cfg(feature = "allocation-probe")]
        assert_eq!(allocations_during(&mut callback), [0; 4]);
        #[cfg(not(feature = "allocation-probe"))]
        callback();
        output
    };
    for (index, notes) in [dense.as_slice(), &[first_off, second_off], &[], &[]]
        .into_iter()
        .enumerate()
    {
        let output = render(index, notes, true);
        let expected = match index {
            2 => [0., 0., 0., 0., 0.625, 0.625, 1.25, 1.25],
            3 => [1.5, 1.5, 1.5, 1.5, 0.75, 0., 0., 0.],
            _ => [0.; 8],
        };
        assert_eq!(output, expected.map(|sample| [sample; 2]));
        let until = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while worker.published_blocks_on_control() <= index as u64 {
            assert!(std::time::Instant::now() < until, "native PCM completion");
            std::thread::yield_now();
        }
    }
    assert_eq!(render(4, &[on], false), [[0.; 2]; 8]);
    let retirement = worker.retire_on_control().unwrap();
    assert!(retirement.failure.is_none() && retirement.native.status.success());
    assert_eq!(retirement.native.terminal.unwrap().blocks, 4);
}

#[test]
#[ignore = "requires explicit source-bound original SDK receipts"]
fn actual_unautomated_effect_delivers_in_block_and_boundary_transport_transitions_to_sdk() {
    const NAME: &str = "plugin_effects::callbacks::tests::actual_unautomated_effect_delivers_in_block_and_boundary_transport_transitions_to_sdk";
    const CHILD: &str = "HEX_UNA_EFFECT_TIMING_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", NAME, "--ignored"])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "126")
            .output()
            .unwrap();
        assert!(
            result.status.success() && String::from_utf8_lossy(&result.stdout).contains("1 passed"),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }

    #[derive(Clone, Copy)]
    struct FixtureClock {
        frame: u64,
        playing: bool,
        beat: Option<u64>,
        tempo: Option<u32>,
        meter: Option<(u8, u8)>,
        cycle: Option<(u64, u64)>,
    }
    impl hex_lattice::NativeInstrumentClock for FixtureClock {
        fn beat_at_offset(&self, _: usize) -> Option<u64> {
            None
        }
        fn arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
            self.frame.checked_add(offset as u64)
        }
        fn arrangement_beat_at_offset(&self, _: usize) -> Option<u64> {
            self.beat
        }
        fn tempo_micro_bpm_at_offset(&self, _: usize) -> Option<u32> {
            self.tempo
        }
        fn transport_is_running(&self) -> Option<bool> {
            Some(self.playing)
        }
        fn arrangement_meter_at_offset(&self, _: usize) -> Option<(u8, u8)> {
            self.meter
        }
        fn arrangement_cycle_at_offset(&self, _: usize) -> Option<(u64, u64)> {
            self.cycle
        }
    }

    // Matches variant 126's exact ProcessContext assertions for SDK blocks 1..=4.
    let block1_playing = FixtureClock {
        frame: 48_000,
        playing: true,
        beat: Some(5 << 31),
        tempo: Some(123_125_000),
        meter: None,
        cycle: None,
    };
    let block2_paused = FixtureClock {
        frame: 0,
        playing: false,
        beat: None,
        tempo: None,
        meter: None,
        cycle: None,
    };
    let block3_playing = FixtureClock {
        frame: 30_000,
        playing: true,
        beat: None,
        tempo: None,
        meter: Some((7, 8)),
        cycle: Some((5 << 30, 11 << 31)),
    };
    let block4_paused = FixtureClock {
        frame: 0,
        playing: false,
        beat: None,
        tempo: None,
        meter: Some((3, 4)),
        cycle: None,
    };
    let block5_paused = FixtureClock {
        frame: 0,
        playing: false,
        beat: None,
        tempo: None,
        meter: None,
        cycle: Some((1 << 63, (1 << 63) + 1)),
    };

    enum Quantum {
        Whole(Option<FixtureClock>),
        Split(FixtureClock, FixtureClock),
    }

    let cases = [
        (
            "running_to_paused_in_block_then_boundary_transitions",
            [
                Quantum::Whole(None),
                Quantum::Split(block1_playing, block2_paused),
                Quantum::Whole(Some(block3_playing)),
                Quantum::Whole(Some(block4_paused)),
                Quantum::Whole(Some(block5_paused)),
                Quantum::Whole(None),
            ],
            [
                [0.5; 8],
                [0.625, 0.625, 0.625, 0.625, 0.75, 0.75, 0.75, 0.75],
                [0.875; 8],
                [1.0; 8],
            ],
        ),
        (
            "boundary_then_paused_to_running_in_block_then_boundary",
            [
                Quantum::Whole(None),
                Quantum::Whole(Some(block1_playing)),
                Quantum::Split(block2_paused, block3_playing),
                Quantum::Whole(Some(block4_paused)),
                Quantum::Whole(Some(block5_paused)),
                Quantum::Whole(None),
            ],
            [
                [0.5; 8],
                [0.625; 8],
                [0.75, 0.75, 0.75, 0.75, 0.875, 0.875, 0.875, 0.875],
                [1.0; 8],
            ],
        ),
    ];

    let run_case = |index: usize, label: &str, quanta: [Quantum; 6], expected_gains: [[f32; 8]; 4]| {
        let binding = Binding {
            root: 1,
            instance: 2,
            graph: 3,
            epoch: 10 + index as u64,
        };
        let identity = ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, 2).unwrap();
        let source = crate::plugin_worker::native_tests::original_instance_for_binding(binding);
        let effect = NativeEffect::prepare_on_control(identity, source).unwrap();
        let facts = Facts {
            identity,
            state: effect.configuration().clone(),
            prepared: effect.prepared(),
        };
        assert!(!facts.prepared.is_instrument_profile());
        let (mut workers, mut endpoints) =
            NativeEffectCallbacks::from_effects_on_control(vec![effect]).unwrap();
        let worker = &mut workers[0];

        let mut received = Vec::new();
        for (quantum_index, quantum) in quanta.into_iter().enumerate() {
            let mut output = [[0.25_f32; 2]; 8];
            let mut run = || match quantum {
                Quantum::Whole(clock) => {
                    endpoints.begin_host_callback(8).unwrap();
                    endpoints
                        .process_segment(
                            facts.node(),
                            0,
                            &mut output,
                            clock.as_ref().map(|c| c as &dyn hex_lattice::NativeInstrumentClock),
                        )
                        .unwrap();
                    endpoints.finish_host_callback(true).unwrap();
                }
                Quantum::Split(first, second) => {
                    endpoints.begin_host_callback(4).unwrap();
                    endpoints
                        .process_segment(facts.node(), 0, &mut output[..4], Some(&first))
                        .unwrap();
                    endpoints.finish_host_callback(true).unwrap();
                    endpoints.begin_host_callback(4).unwrap();
                    endpoints
                        .process_segment(facts.node(), 0, &mut output[4..], Some(&second))
                        .unwrap();
                    endpoints.finish_host_callback(true).unwrap();
                }
            };
            #[cfg(feature = "allocation-probe")]
            assert_eq!(allocations_during(&mut run), [0; 4]);
            #[cfg(not(feature = "allocation-probe"))]
            run();
            received.push(output);
            let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while worker.published_blocks_on_control() <= quantum_index as u64 {
                assert!(
                    std::time::Instant::now() < until,
                    "{label}: native PCM completion at quantum {quantum_index}"
                );
                std::thread::yield_now();
            }
        }
        for (q, gains) in expected_gains.into_iter().enumerate() {
            let expected = gains.map(|gain| [gain * 0.25; 2]);
            assert_eq!(received[q + 2], expected, "{label}: quantum {q}");
        }
        let retirement = worker.retire_on_control().unwrap();
        assert!(
            retirement.failure.is_none() && retirement.native.status.success(),
            "{label}: retirement failed: {:?}",
            retirement.failure
        );
        assert_eq!(retirement.native.terminal.unwrap().blocks, 6, "{label}");
    };
    for (index, (label, quanta, expected_gains)) in cases.into_iter().enumerate() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(move || run_case(index, label, quanta, expected_gains))
            .unwrap()
            .join()
            .unwrap();
    }
}

#[test]
#[ignore = "requires explicit source-bound original SDK receipts"]
fn actual_unautomated_instrument_and_effect_receive_sdk_loop_wrap_origins() {
    const NAME: &str = "plugin_effects::callbacks::tests::actual_unautomated_instrument_and_effect_receive_sdk_loop_wrap_origins";
    const CHILD: &str = "HEX_LOOP_WRAP_SDK_TEST_CHILD";
    const K_PLAYING: i64 = 1 << 1;
    const K_PROJECT_TIME_MUSIC_VALID: i64 = 1 << 9;

    if std::env::var_os(CHILD).is_none() {
        let dir = std::env::temp_dir().join(format!("hex-loop-wrap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (mode, variant) in [("effect", "0"), ("instrument", "115")] {
            let trace_path = dir.join(format!("{mode}.bin"));
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", NAME, "--ignored"])
                .env(CHILD, mode)
                .env("HEX_ORIGINAL_INSTANCE_VARIANT", variant)
                .env("HEX_ORIGINAL_CONTEXT_TRACE", &trace_path)
                .output()
                .unwrap();
            assert!(
                result.status.success()
                    && String::from_utf8_lossy(&result.stdout).contains("1 passed"),
                "{mode}:\n{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
            let bytes = std::fs::read(&trace_path).unwrap();
            assert!(bytes.len() >= 8 && bytes.len().is_multiple_of(8));
            let words: Vec<i64> = bytes
                .chunks_exact(8)
                .map(|c| i64::from_le_bytes(c.try_into().unwrap()))
                .collect();
            let count = words[0] as usize;
            // Quantum 0 splits into 2 SDK slices (3 frames @ 125, 5 frames @ 64).
            // Quanta 1 (contiguous across 2 callbacks), 2 (aligned pre-wrap),
            // 3 (aligned post-wrap), 4 & 5 (drain) each execute 1 SDK slice -> 7 total.
            assert_eq!(count, 7, "{mode}: unexpected SDK slice count: {words:?}");
            let slice = |idx: usize| -> (i64, i64, i64, f64) {
                let base = 1 + idx * 4;
                (
                    words[base],
                    words[base + 1],
                    words[base + 2],
                    f64::from_bits(words[base + 3] as u64),
                )
            };
            // Slice 0: pre-wrap 3 samples at frame 125, beat 125.0, kPlaying set.
            assert_eq!(
                slice(0),
                (3, K_PLAYING | K_PROJECT_TIME_MUSIC_VALID, 125, 125.0),
                "{mode}: slice 0"
            );
            // Slice 1: post-wrap 5 samples at frame 64, beat 64.0, kPlaying set.
            assert_eq!(
                slice(1),
                (5, K_PLAYING | K_PROJECT_TIME_MUSIC_VALID, 64, 64.0),
                "{mode}: slice 1"
            );
            // Slice 2: contiguous across two callbacks (3 + 5), single 8-sample SDK call @ 69.
            assert_eq!(
                slice(2),
                (8, K_PLAYING | K_PROJECT_TIME_MUSIC_VALID, 69, 69.0),
                "{mode}: slice 2"
            );
            // Slice 3: block-aligned pre-wrap [120..128), single 8-sample SDK call @ 120.
            assert_eq!(
                slice(3),
                (8, K_PLAYING | K_PROJECT_TIME_MUSIC_VALID, 120, 120.0),
                "{mode}: slice 3"
            );
            // Slice 4: block-aligned post-wrap [64..72), single 8-sample SDK call @ 64.
            assert_eq!(
                slice(4),
                (8, K_PLAYING | K_PROJECT_TIME_MUSIC_VALID, 64, 64.0),
                "{mode}: slice 4"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }

    let mode = std::env::var(CHILD).unwrap();
    let is_instrument = mode == "instrument";
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(move || {
            struct LoopClock(u64);
            impl hex_lattice::NativeInstrumentClock for LoopClock {
                fn beat_at_offset(&self, _: usize) -> Option<u64> {
                    None
                }
                fn arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
                    Some(self.0 + offset as u64)
                }
                fn arrangement_beat_at_offset(&self, offset: usize) -> Option<u64> {
                    Some((self.0 + offset as u64) << 32)
                }
                fn transport_is_running(&self) -> Option<bool> {
                    Some(true)
                }
            }

            let binding = Binding {
                root: 1,
                instance: 2,
                graph: 3,
                epoch: if is_instrument { 21 } else { 20 },
            };
            let identity = ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, 2).unwrap();
            let source = crate::plugin_worker::native_tests::original_instance_with_inputs(
                binding,
                if is_instrument { 0 } else { 2 },
            );
            let effect = NativeEffect::prepare_on_control(identity, source).unwrap();
            let facts = Facts {
                identity,
                state: effect.configuration().clone(),
                prepared: effect.prepared(),
            };
            assert_eq!(facts.prepared.is_instrument_profile(), is_instrument);
            let (mut workers, mut endpoints) =
                NativeEffectCallbacks::from_effects_on_control(vec![effect]).unwrap();
            let worker = &mut workers[0];

            let note_on_pre = ForeignNote {
                frame: 1,
                channel: 2,
                pitch: 60,
                value: 127,
                kind: ForeignNoteKind::On,
                note_id: std::num::NonZeroU32::new(41),
                is_live: false,
            };
            let note_on_cut = ForeignNote {
                frame: 0,
                channel: 2,
                pitch: 60,
                value: 127,
                kind: ForeignNoteKind::On,
                note_id: std::num::NonZeroU32::new(42),
                is_live: false,
            };
            let note_off_pre = ForeignNote {
                frame: 2,
                value: 0,
                kind: ForeignNoteKind::Off,
                ..note_on_pre
            };
            let note_off_cut = ForeignNote {
                frame: 3,
                value: 0,
                kind: ForeignNoteKind::Off,
                ..note_on_cut
            };

            let mut received = Vec::new();
            let wait_for = |worker: &crate::plugin_worker::NativeWorker, blocks: u64| {
                let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
                while worker.published_blocks_on_control() < blocks {
                    assert!(std::time::Instant::now() < until, "native PCM completion");
                    std::thread::yield_now();
                }
            };

            // Quantum 0: interior wrap at offset 3 split across two physical callbacks (3 + 5).
            {
                let mut out = [[0.25_f32; 2]; 8];
                let mut run = || {
                    endpoints.begin_host_callback(3).unwrap();
                    if is_instrument {
                        endpoints
                            .process_segment_with_notes(
                                facts.node(),
                                0,
                                &mut out[..3],
                                &[],
                                &[note_on_pre],
                                Some(&LoopClock(125)),
                            )
                            .unwrap();
                    } else {
                        endpoints
                            .process_segment(facts.node(), 0, &mut out[..3], Some(&LoopClock(125)))
                            .unwrap();
                    }
                    endpoints.finish_host_callback(true).unwrap();

                    endpoints.begin_host_callback(5).unwrap();
                    if is_instrument {
                        endpoints
                            .process_segment_with_notes(
                                facts.node(),
                                0,
                                &mut out[3..],
                                &[],
                                &[note_on_cut, note_off_pre, note_off_cut],
                                Some(&LoopClock(64)),
                            )
                            .unwrap();
                    } else {
                        endpoints
                            .process_segment(facts.node(), 0, &mut out[3..], Some(&LoopClock(64)))
                            .unwrap();
                    }
                    endpoints.finish_host_callback(true).unwrap();
                };
                #[cfg(feature = "allocation-probe")]
                assert_eq!(allocations_during(&mut run), [0; 4]);
                #[cfg(not(feature = "allocation-probe"))]
                run();
                received.push(out);
                wait_for(worker, 1);
            }

            // Quantum 1: contiguous running audio across two physical callbacks (3 + 5).
            {
                let mut out = [[0.25_f32; 2]; 8];
                let mut run = || {
                    endpoints.begin_host_callback(3).unwrap();
                    endpoints
                        .process_segment(facts.node(), 0, &mut out[..3], Some(&LoopClock(69)))
                        .unwrap();
                    endpoints.finish_host_callback(true).unwrap();
                    endpoints.begin_host_callback(5).unwrap();
                    endpoints
                        .process_segment(facts.node(), 0, &mut out[3..], Some(&LoopClock(72)))
                        .unwrap();
                    endpoints.finish_host_callback(true).unwrap();
                };
                #[cfg(feature = "allocation-probe")]
                assert_eq!(allocations_during(&mut run), [0; 4]);
                #[cfg(not(feature = "allocation-probe"))]
                run();
                received.push(out);
                wait_for(worker, 2);
            }

            // Quanta 2..6: block-aligned wrap (120 -> 64) + 2 drain quanta (72, 80).
            for (idx, start) in [120_u64, 64, 72, 80].into_iter().enumerate() {
                let mut out = [[0.25_f32; 2]; 8];
                let mut run = || {
                    endpoints.begin_host_callback(8).unwrap();
                    endpoints
                        .process_segment(facts.node(), 0, &mut out, Some(&LoopClock(start)))
                        .unwrap();
                    endpoints.finish_host_callback(true).unwrap();
                };
                #[cfg(feature = "allocation-probe")]
                assert_eq!(allocations_during(&mut run), [0; 4]);
                #[cfg(not(feature = "allocation-probe"))]
                run();
                received.push(out);
                wait_for(worker, 3 + idx as u64);
            }

            // Quantum 0 output arrives at received[2] (2-quantum bridge latency).
            if is_instrument {
                // Slice 0 (block 0, scale 0.5): frame 0 silent, frames 1..3 have voice 41 (1.0 * 0.5 = 0.5).
                // Slice 1 (block 1, scale 0.625): frames 3..5 have voices 41+42 (2.0 * 0.625 = 1.25),
                // frame 5 has voice 42 only (1.0 * 0.625 = 0.625), frames 6..8 silent.
                let expected = [0.0, 0.5, 0.5, 1.25, 1.25, 0.625, 0.0, 0.0].map(|s| [s; 2]);
                assert_eq!(received[2], expected, "instrument loop-wrap PCM/MIDI order");
            } else {
                // Slice 0 (block 0, scale 0.5): 0.25 * 0.5 = 0.125 for frames 0..3.
                // Slice 1 (block 1, scale 0.625): 0.25 * 0.625 = 0.15625 for frames 3..8.
                let expected = [
                    0.125, 0.125, 0.125, 0.15625, 0.15625, 0.15625, 0.15625, 0.15625,
                ]
                .map(|s| [s; 2]);
                assert_eq!(received[2], expected, "effect loop-wrap PCM order");
                // Quantum 1 (block 2, single unsplit slice, scale 0.75): 0.25 * 0.75 = 0.1875.
                assert_eq!(received[3], [[0.1875; 2]; 8], "effect contiguous PCM");
                // Quantum 2 (block 3, aligned pre-wrap, scale 0.875): 0.25 * 0.875 = 0.21875.
                assert_eq!(received[4], [[0.21875; 2]; 8], "effect aligned pre-wrap PCM");
                // Quantum 3 (block 4, aligned post-wrap, scale 1.0): 0.25 * 1.0 = 0.25.
                assert_eq!(received[5], [[0.25; 2]; 8], "effect aligned post-wrap PCM");
            }

            let retirement = worker.retire_on_control().unwrap();
            assert!(
                retirement.failure.is_none() && retirement.native.status.success(),
                "{mode}: retirement failed: {:?}",
                retirement.failure
            );
            assert_eq!(retirement.native.terminal.unwrap().blocks, 6, "{mode}");
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
#[ignore = "requires explicit source-bound original SDK receipts"]
fn actual_vst3_note_events_carry_sample_accurate_ppq_position() {
    const NAME: &str = "plugin_effects::callbacks::tests::actual_vst3_note_events_carry_sample_accurate_ppq_position";
    const CHILD: &str = "HEX_NOTE_PPQ_SDK_TEST_CHILD";
    const K_NOTE_ON: i64 = 0;
    const K_NOTE_OFF: i64 = 1;

    if std::env::var_os(CHILD).is_none() {
        let trace_path =
            std::env::temp_dir().join(format!("hex-note-ppq-{}.bin", std::process::id()));
        let _ = std::fs::remove_file(&trace_path);
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", NAME, "--ignored"])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "115")
            .env("HEX_ORIGINAL_EVENT_TRACE", &trace_path)
            .output()
            .unwrap();
        assert!(
            result.status.success() && String::from_utf8_lossy(&result.stdout).contains("1 passed"),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        let bytes = std::fs::read(&trace_path).unwrap();
        let _ = std::fs::remove_file(&trace_path);
        let words: Vec<i64> = bytes
            .chunks_exact(8)
            .map(|c| i64::from_le_bytes(c.try_into().unwrap()))
            .collect();
        let count = words[0] as usize;
        assert_eq!(count, 11, "expected 11 recorded SDK note events: {words:?}");
        let ev = |idx: usize| -> (i64, i64, i64, i64, i64, f64) {
            let b = 1 + idx * 6;
            (
                words[b],
                words[b + 1],
                words[b + 2],
                words[b + 3],
                words[b + 4],
                f64::from_bits(words[b + 5] as u64),
            )
        };
        let assert_ev = |idx: usize,
                         kind: i64,
                         sample_offset: i64,
                         pitch: i64,
                         note_id: i64,
                         expected_ppq: f64| {
            let (ty, off, p, id, flags, ppq) = ev(idx);
            assert_eq!((ty, off, p, id, flags), (kind, sample_offset, pitch, note_id, 0), "event {idx}");
            assert!(
                (ppq - expected_ppq).abs() <= 1e-12,
                "event {idx}: ppq {ppq} != expected {expected_ppq}"
            );
        };
        // Quantum 0: clip at beat 8, 48 kHz / 120 BPM (note on @ 0, note off @ 6).
        assert_ev(0, K_NOTE_ON, 0, 60, 101, 8.0);
        assert_ev(1, K_NOTE_OFF, 6, 60, 101, 8.0 + (6.0 * 120.0) / (60.0 * 48000.0));
        // Quantum 1: clip moved to beat 16, 48 kHz / 120 BPM (same pitch, note_id, offsets, duration).
        assert_ev(2, K_NOTE_ON, 0, 60, 101, 16.0);
        assert_ev(3, K_NOTE_OFF, 6, 60, 101, 16.0 + (6.0 * 120.0) / (60.0 * 48000.0));
        // Quantum 2: after tempo change to 150 BPM at beat 20 (on @ 2, off @ 6).
        assert_ev(4, K_NOTE_ON, 2, 60, 102, 20.0 + (2.0 * 150.0) / (60.0 * 48000.0));
        assert_ev(5, K_NOTE_OFF, 6, 60, 102, 20.0 + (6.0 * 150.0) / (60.0 * 48000.0));
        // Quantum 3: across loop split at offset 4 (pre-wrap beat 7.75 -> post-wrap beat 4.0 @ 120 BPM).
        assert_ev(6, K_NOTE_ON, 1, 60, 103, 7.75 + (1.0 * 120.0) / (60.0 * 48000.0));
        assert_ev(7, K_NOTE_ON, 0, 60, 104, 4.0);
        assert_ev(8, K_NOTE_OFF, 2, 60, 103, 4.0 + (2.0 * 120.0) / (60.0 * 48000.0));
        // Quantum 4: no project musical clock (clock = None) -> ppqPosition == 0.0.
        assert_ev(9, K_NOTE_OFF, 1, 60, 104, 0.0);
        assert_ev(10, K_NOTE_ON, 3, 60, 105, 0.0);
        return;
    }

    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            struct MusicalClock {
                frame: u64,
                beat_raw: u64,
                tempo_micro_bpm: u32,
            }
            impl hex_lattice::NativeInstrumentClock for MusicalClock {
                fn beat_at_offset(&self, _: usize) -> Option<u64> {
                    None
                }
                fn arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
                    Some(self.frame + offset as u64)
                }
                fn arrangement_beat_at_offset(&self, offset: usize) -> Option<u64> {
                    let delta_beats = ((offset as f64) * (self.tempo_micro_bpm as f64 / 1_000_000.0))
                        / (60.0 * 48_000.0);
                    Some(self.beat_raw + (delta_beats * 4_294_967_296.0).round() as u64)
                }
                fn tempo_micro_bpm_at_offset(&self, _: usize) -> Option<u32> {
                    Some(self.tempo_micro_bpm)
                }
                fn transport_is_running(&self) -> Option<bool> {
                    Some(true)
                }
            }

            let binding = Binding {
                root: 1,
                instance: 2,
                graph: 3,
                epoch: 30,
            };
            let identity = ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, 2).unwrap();
            let source = crate::plugin_worker::native_tests::original_instance_with_inputs(binding, 0);
            let effect = NativeEffect::prepare_on_control(identity, source).unwrap();
            let facts = Facts {
                identity,
                state: effect.configuration().clone(),
                prepared: effect.prepared(),
            };
            let (mut workers, mut endpoints) =
                NativeEffectCallbacks::from_effects_on_control(vec![effect]).unwrap();
            let worker = &mut workers[0];

            let note = |frame: u16, on: bool, id: u32| ForeignNote {
                frame,
                channel: 2,
                pitch: 60,
                value: if on { 127 } else { 0 },
                kind: ForeignNoteKind::gate(on),
                note_id: std::num::NonZeroU32::new(id),
                is_live: false,
            };
            let wait_for = |worker: &crate::plugin_worker::NativeWorker, blocks: u64| {
                let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
                while worker.published_blocks_on_control() < blocks {
                    assert!(std::time::Instant::now() < until, "native PCM completion");
                    std::thread::yield_now();
                }
            };

            // Quanta 0, 1, 2: beat 8 @ 120, beat 16 @ 120, beat 20 @ 150.
            let blocks_spec: [(Option<MusicalClock>, [ForeignNote; 2]); 3] = [
                (
                    Some(MusicalClock {
                        frame: 192_000,
                        beat_raw: 8 << 32,
                        tempo_micro_bpm: 120_000_000,
                    }),
                    [note(0, true, 101), note(6, false, 101)],
                ),
                (
                    Some(MusicalClock {
                        frame: 384_000,
                        beat_raw: 16 << 32,
                        tempo_micro_bpm: 120_000_000,
                    }),
                    [note(0, true, 101), note(6, false, 101)],
                ),
                (
                    Some(MusicalClock {
                        frame: 480_000,
                        beat_raw: 20 << 32,
                        tempo_micro_bpm: 150_000_000,
                    }),
                    [note(2, true, 102), note(6, false, 102)],
                ),
            ];
            for (idx, (clock, notes)) in blocks_spec.into_iter().enumerate() {
                let mut out = [[0.0_f32; 2]; 8];
                endpoints.begin_host_callback(8).unwrap();
                endpoints
                    .process_segment_with_notes(
                        facts.node(),
                        0,
                        &mut out,
                        &[],
                        &notes,
                        clock.as_ref().map(|c| c as &dyn hex_lattice::NativeInstrumentClock),
                    )
                    .unwrap();
                endpoints.finish_host_callback(true).unwrap();
                wait_for(worker, 1 + idx as u64);
            }

            // Quantum 3: loop split at offset 4 (beat 7.75 -> beat 4.0 @ 120 BPM).
            {
                let mut out = [[0.0_f32; 2]; 8];
                let pre = MusicalClock {
                    frame: 186_000,
                    beat_raw: (7 << 32) + ((3 << 32) / 4),
                    tempo_micro_bpm: 120_000_000,
                };
                let post = MusicalClock {
                    frame: 96_000,
                    beat_raw: 4 << 32,
                    tempo_micro_bpm: 120_000_000,
                };
                endpoints.begin_host_callback(8).unwrap();
                endpoints
                    .process_segment_with_notes(
                        facts.node(),
                        0,
                        &mut out[..4],
                        &[],
                        &[note(1, true, 103)],
                        Some(&pre),
                    )
                    .unwrap();
                endpoints
                    .process_segment_with_notes(
                        facts.node(),
                        4,
                        &mut out[4..],
                        &[],
                        &[note(0, true, 104), note(2, false, 103)],
                        Some(&post),
                    )
                    .unwrap();
                endpoints.finish_host_callback(true).unwrap();
                wait_for(worker, 4);
            }

            // Quantum 4: no musical clock -> ppqPosition == 0.0.
            {
                let mut out = [[0.0_f32; 2]; 8];
                endpoints.begin_host_callback(8).unwrap();
                endpoints
                    .process_segment_with_notes(
                        facts.node(),
                        0,
                        &mut out,
                        &[],
                        &[note(1, false, 104), note(3, true, 105)],
                        None,
                    )
                    .unwrap();
                endpoints.finish_host_callback(true).unwrap();
                wait_for(worker, 5);
            }

            let retirement = worker.retire_on_control().unwrap();
            assert!(retirement.failure.is_none() && retirement.native.status.success());
            assert_eq!(retirement.native.terminal.unwrap().blocks, 5);
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
#[ignore = "requires explicit source-bound original SDK receipts"]
fn actual_vst3_live_notes_set_k_is_live_and_sequenced_notes_leave_it_clear() {
    const NAME: &str = "plugin_effects::callbacks::tests::actual_vst3_live_notes_set_k_is_live_and_sequenced_notes_leave_it_clear";
    const CHILD: &str = "HEX_NOTE_LIVE_SDK_TEST_CHILD";
    const K_NOTE_ON: i64 = 0;
    const K_NOTE_OFF: i64 = 1;
    const K_POLY_PRESSURE: i64 = 3;
    const K_IS_LIVE: i64 = 1;

    if std::env::var_os(CHILD).is_none() {
        let trace_path =
            std::env::temp_dir().join(format!("hex-note-live-{}.bin", std::process::id()));
        let _ = std::fs::remove_file(&trace_path);
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", NAME, "--ignored"])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "115")
            .env("HEX_ORIGINAL_EVENT_TRACE", &trace_path)
            .output()
            .unwrap();
        assert!(
            result.status.success() && String::from_utf8_lossy(&result.stdout).contains("1 passed"),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        let bytes = std::fs::read(&trace_path).unwrap();
        let _ = std::fs::remove_file(&trace_path);
        let words: Vec<i64> = bytes
            .chunks_exact(8)
            .map(|c| i64::from_le_bytes(c.try_into().unwrap()))
            .collect();
        let count = words[0] as usize;
        assert_eq!(count, 9, "expected 9 recorded SDK note events: {words:?}");
        let ev = |idx: usize| -> (i64, i64, i64, i64, i64) {
            let b = 1 + idx * 6;
            (words[b], words[b + 1], words[b + 2], words[b + 3], words[b + 4])
        };
        // Quantum 0: the monitored onset, pressure and release share identity and live flags.
        assert_eq!(ev(0), (K_NOTE_ON, 0, 60, 201, K_IS_LIVE));
        assert_eq!(ev(1), (K_POLY_PRESSURE, 2, 60, 201, K_IS_LIVE));
        assert_eq!(ev(2), (K_NOTE_OFF, 3, 60, 201, K_IS_LIVE));
        // Quantum 1: computer-keyboard live press + release (kIsLive).
        assert_eq!(ev(3), (K_NOTE_ON, 1, 60, 202, K_IS_LIVE));
        assert_eq!(ev(4), (K_NOTE_OFF, 5, 60, 202, K_IS_LIVE));
        // Quantum 2: overlapping sequenced clip notes sharing pitch 60 leave kIsLive clear
        // and retain matching note IDs and exact sample offsets.
        assert_eq!(ev(5), (K_NOTE_ON, 1, 60, 301, 0));
        assert_eq!(ev(6), (K_NOTE_ON, 3, 60, 302, 0));
        assert_eq!(ev(7), (K_NOTE_OFF, 5, 60, 301, 0));
        assert_eq!(ev(8), (K_NOTE_OFF, 7, 60, 302, 0));
        return;
    }

    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let binding = Binding {
                root: 1,
                instance: 2,
                graph: 3,
                epoch: 31,
            };
            let identity = ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, 2).unwrap();
            let source = crate::plugin_worker::native_tests::original_instance_with_inputs(binding, 0);
            let effect = NativeEffect::prepare_on_control(identity, source).unwrap();
            let facts = Facts {
                identity,
                state: effect.configuration().clone(),
                prepared: effect.prepared(),
            };
            let (mut workers, mut endpoints) =
                NativeEffectCallbacks::from_effects_on_control(vec![effect]).unwrap();
            let worker = &mut workers[0];

            let note = |frame: u16, on: bool, id: u32, is_live: bool| ForeignNote {
                frame,
                channel: 2,
                pitch: 60,
                value: if on { 127 } else { 0 },
                kind: ForeignNoteKind::gate(on),
                note_id: std::num::NonZeroU32::new(id),
                is_live,
            };
            let wait_for = |worker: &crate::plugin_worker::NativeWorker, blocks: u64| {
                let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
                while worker.published_blocks_on_control() < blocks {
                    assert!(std::time::Instant::now() < until, "native PCM completion");
                    std::thread::yield_now();
                }
            };

            let batches: [&[ForeignNote]; 3] = [
                &[
                    note(0, true, 201, true),
                    ForeignNote { frame: 2, kind: ForeignNoteKind::PolyPressure, value: 64, ..note(0, true, 201, true) },
                    ForeignNote::control_change(2, 2, 74, 96, std::num::NonZeroU32::new(201), true),
                    ForeignNote::pitch_bend(2, 2, 2048, std::num::NonZeroU32::new(201), true),
                    ForeignNote::channel_pressure(2, 2, 72, None, true),
                    note(3, false, 201, true),
                ],
                &[note(1, true, 202, true), note(5, false, 202, true)],
                &[
                    note(1, true, 301, false),
                    note(3, true, 302, false),
                    note(5, false, 301, false),
                    note(7, false, 302, false),
                ],
            ];
            for (idx, notes) in batches.into_iter().enumerate() {
                let mut out = [[0.0_f32; 2]; 8];
                let mut run = || {
                    endpoints.begin_host_callback(8).unwrap();
                    endpoints
                        .process_segment_with_notes(facts.node(), 0, &mut out, &[], notes, None)
                        .unwrap();
                    endpoints.finish_host_callback(true).unwrap();
                };
                #[cfg(feature = "allocation-probe")]
                assert_eq!(allocations_during(&mut run), [0; 4]);
                #[cfg(not(feature = "allocation-probe"))]
                run();
                wait_for(worker, 1 + idx as u64);
            }

            let retirement = worker.retire_on_control().unwrap();
            assert!(retirement.failure.is_none() && retirement.native.status.success());
            assert_eq!(retirement.native.terminal.unwrap().blocks, 3);
        })
        .unwrap()
        .join()
        .unwrap();
}
}

/// Copied graph facts for one endpoint, so a node can be built while the
/// callback carrier is mutably borrowed.
struct Facts {
    identity: ProcessorInstanceIdentity,
    state: NativeDeviceState,
    prepared: PreparedForeignProcessor,
}
impl Facts {
    fn node(&self) -> ForeignProcessorNode<'_> {
        ForeignProcessorNode {
            owner: self.identity.owner(),
            device_id: self.identity.device_id(),
            device_type: self.state.type_id(),
            sample_rate: 48_000,
            configuration: &self.state,
            prepared: self.prepared,
            automation: None,
            bypassed: false,
            input_gain: 1.0,
            output_gain: 1.0,
        }
    }
}

struct Endpoint {
    facts: Facts,
    _control: Control,
    worker: Worker,
}

fn endpoints() -> (NativeEffectCallbacks, Vec<Endpoint>) {
    let state = hex_engine::default_native_device_state(hex_devices::utility::UTILITY_TYPE_ID)
        .expect("utility default state");
    let mut entries = Vec::new();
    let mut endpoints = Vec::new();
    for instance in [1, 2] {
        let binding = Binding {
            root: 1,
            instance,
            graph: 1,
            epoch: 1,
        };
        let (control, callback, worker) = channel(binding, QUANTUM as u16).expect("transport");
        let identity =
            ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, instance).expect("identity");
        let callback = NativeEffectCallback::for_test(identity, binding, state.clone(), callback);
        endpoints.push(Endpoint {
            facts: Facts {
                identity,
                state: state.clone(),
                prepared: callback.configuration.prepared,
            },
            _control: control,
            worker,
        });
        entries.push(NativeEffectEntry::new(callback));
    }
    let callbacks = NativeEffectCallbacks {
        entries,
        quantum: QUANTUM,
        cycle: NativeEffectCycle::default(),
    };
    (callbacks, endpoints)
}

fn input(callback: usize) -> [[f32; 2]; QUANTUM] {
    std::array::from_fn(|frame| {
        let value = (callback * QUANTUM + frame + 1) as f32;
        [value, -value]
    })
}

/// One complete physical callback. Each endpoint receives the same source and
/// writes its own output, so their alignment can be compared directly.
fn render(
    callbacks: &mut NativeEffectCallbacks,
    endpoints: &[Endpoint],
    source: &[[f32; 2]; QUANTUM],
    outputs: &mut [[[f32; 2]; QUANTUM]; 2],
) -> [Result<(), DeviceContractError>; 3] {
    let begun = callbacks
        .begin_host_callback(QUANTUM)
        .map_err(|_| DeviceContractError::ForeignProcessingFailed);
    let mut processed = [Ok(()), Ok(())];
    for ((output, endpoint), result) in outputs.iter_mut().zip(endpoints).zip(&mut processed) {
        *output = *source;
        *result = callbacks.process_segment(endpoint.facts.node(), 0, output, None);
    }
    let finished = callbacks
        .finish_host_callback(true)
        .map_err(|_| DeviceContractError::ForeignProcessingFailed);
    let [first, second] = processed;
    [begun.and(finished), first, second]
}

/// Allocation total/net counts and bytes on this thread while `run` renders.
#[cfg(feature = "allocation-probe")]
fn allocations_during(run: impl FnOnce()) -> [u64; 4] {
    let measured = allocation_counter::measure(run);
    [
        measured.count_total,
        measured.count_current.unsigned_abs(),
        measured.bytes_total,
        measured.bytes_current.unsigned_abs(),
    ]
}
#[cfg(not(feature = "allocation-probe"))]
fn allocations_during(run: impl FnOnce()) -> [u64; 4] {
    run();
    [0; 4]
}

/// Echo every pending request as its reply; the healthy endpoint is identity.
fn serve(worker: &mut Worker) {
    while let Some(block) = worker.receive().expect("healthy worker") {
        worker.complete(block).expect("reply published");
    }
}

#[test]
fn worker_quanta_use_their_first_aligned_clock_even_across_split_callbacks() {
    use hex_plugin_host::transport::ProcessTiming;

    struct Clock(u64, bool);
    impl hex_lattice::NativeInstrumentClock for Clock {
        fn beat_at_offset(&self, _: usize) -> Option<u64> {
            None
        }
        fn arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
            Some(self.0 + offset as u64)
        }
        fn arrangement_beat_at_offset(&self, offset: usize) -> Option<u64> {
            Some((self.0 + offset as u64) << 32)
        }
        fn tempo_micro_bpm_at_offset(&self, _: usize) -> Option<u32> {
            Some(123_125_000)
        }
        fn transport_is_running(&self) -> Option<bool> {
            Some(self.1)
        }
        fn arrangement_meter_at_offset(&self, offset: usize) -> Option<(u8, u8)> {
            Some(if offset == 0 { (7, 8) } else { (3, 4) })
        }
        fn arrangement_cycle_at_offset(&self, offset: usize) -> Option<(u64, u64)> {
            (offset == 0).then_some((0, 4 << 32))
        }
    }
    let (mut callbacks, mut endpoints) = endpoints();
    let endpoint = &mut endpoints[0];
    let node = endpoint.facts.node();
    let mut callback = |frames, clock: Option<&dyn hex_lattice::NativeInstrumentClock>| {
        let mut output = [[0.5; 2]; QUANTUM];
        assert_eq!(
            allocations_during(|| {
                callbacks.begin_host_callback(frames).unwrap();
                callbacks
                    .process_segment(node, 0, &mut output[..frames], clock)
                    .unwrap();
                callbacks.finish_host_callback(true).unwrap();
            }),
            [0; 4]
        );
    };
    callback(2, Some(&Clock(100, true)));
    assert!(endpoint.worker.receive().unwrap().is_none());
    callback(4, Some(&Clock(500, false)));
    let mut first = endpoint.worker.receive().unwrap().unwrap();
    assert_eq!(
        first.timing,
        Some(ProcessTiming {
            arrangement_frame: 100,
            playing: true,
            arrangement_beat_raw: Some(100 << 32),
            tempo_micro_bpm: std::num::NonZeroU32::new(123_125_000),
            meter: Some((7, 8)),
            cycle_beats_raw: Some((0, 4 << 32)),
        })
    );
    assert_eq!(
        first.timing_changes.as_slice(),
        &[hex_plugin_host::transport::TimingChange {
            frame: 2,
            timing: ProcessTiming {
                arrangement_frame: 500,
                playing: false,
                arrangement_beat_raw: Some(500 << 32),
                tempo_micro_bpm: std::num::NonZeroU32::new(123_125_000),
                meter: Some((7, 8)),
                cycle_beats_raw: Some((0, 4 << 32)),
            },
        }]
    );
    first.timing_changes = Default::default();
    endpoint.worker.complete(first).unwrap();
    callback(2, None);
    let second = endpoint.worker.receive().unwrap().unwrap();
    assert_eq!(
        second.timing,
        Some(ProcessTiming {
            arrangement_frame: 502,
            playing: false,
            arrangement_beat_raw: Some(502 << 32),
            tempo_micro_bpm: std::num::NonZeroU32::new(123_125_000),
            meter: Some((3, 4)),
            cycle_beats_raw: None,
        })
    );
}

#[test]
fn unautomated_effect_records_in_block_and_boundary_transport_transitions() {
    use hex_plugin_host::transport::{ProcessTiming, TimingChange};

    struct Clock(u64, bool);
    impl hex_lattice::NativeInstrumentClock for Clock {
        fn beat_at_offset(&self, _: usize) -> Option<u64> {
            None
        }
        fn arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
            Some(self.0 + offset as u64)
        }
        fn transport_is_running(&self) -> Option<bool> {
            Some(self.1)
        }
    }

    let (mut callbacks, mut endpoints) = endpoints();
    let endpoint = &mut endpoints[0];
    let node = endpoint.facts.node();
    assert!(node.automation.is_none());
    assert!(!node.prepared.is_instrument_profile());

    let mut callback = |frames: usize, clock: &Clock| {
        let mut output = [[0.25; 2]; QUANTUM];
        assert_eq!(
            allocations_during(|| {
                callbacks.begin_host_callback(frames).unwrap();
                callbacks
                    .process_segment(node, 0, &mut output[..frames], Some(clock))
                    .unwrap();
                callbacks.finish_host_callback(true).unwrap();
            }),
            [0; 4]
        );
    };

    // running -> paused inside a 4-frame quantum at sample offset 2.
    callback(2, &Clock(100, true));
    assert!(endpoint.worker.receive().unwrap().is_none());
    callback(2, &Clock(102, false));
    let mut running_to_paused = endpoint.worker.receive().unwrap().unwrap();
    assert_eq!(
        running_to_paused.timing,
        Some(ProcessTiming {
            arrangement_frame: 100,
            playing: true,
            ..Default::default()
        })
    );
    assert_eq!(
        running_to_paused.timing_changes.as_slice(),
        &[TimingChange {
            frame: 2,
            timing: ProcessTiming {
                arrangement_frame: 102,
                playing: false,
                ..Default::default()
            },
        }]
    );
    running_to_paused.timing_changes = Default::default();
    endpoint.worker.complete(running_to_paused).unwrap();

    // paused -> running inside a 4-frame quantum at sample offset 2.
    callback(2, &Clock(104, false));
    assert!(endpoint.worker.receive().unwrap().is_none());
    callback(2, &Clock(106, true));
    let mut paused_to_running = endpoint.worker.receive().unwrap().unwrap();
    assert_eq!(
        paused_to_running.timing,
        Some(ProcessTiming {
            arrangement_frame: 104,
            playing: false,
            ..Default::default()
        })
    );
    assert_eq!(
        paused_to_running.timing_changes.as_slice(),
        &[TimingChange {
            frame: 2,
            timing: ProcessTiming {
                arrangement_frame: 106,
                playing: true,
                ..Default::default()
            },
        }]
    );
    paused_to_running.timing_changes = Default::default();
    endpoint.worker.complete(paused_to_running).unwrap();

    // Exact block-boundary transitions (running -> paused, then paused -> running)
    // update the initial timing without emitting unnecessary timing_changes splits.
    callback(4, &Clock(108, false));
    let boundary_pause = endpoint.worker.receive().unwrap().unwrap();
    assert_eq!(
        boundary_pause.timing,
        Some(ProcessTiming {
            arrangement_frame: 108,
            playing: false,
            ..Default::default()
        })
    );
    assert!(boundary_pause.timing_changes.as_slice().is_empty());
    endpoint.worker.complete(boundary_pause).unwrap();

    callback(4, &Clock(112, true));
    let boundary_play = endpoint.worker.receive().unwrap().unwrap();
    assert_eq!(
        boundary_play.timing,
        Some(ProcessTiming {
            arrangement_frame: 112,
            playing: true,
            ..Default::default()
        })
    );
    assert!(boundary_play.timing_changes.as_slice().is_empty());
    endpoint.worker.complete(boundary_play).unwrap();
}

#[test]
fn unautomated_effect_delivers_interior_loop_wraps_without_redundant_splits() {
    use hex_plugin_host::transport::{ProcessTiming, TimingChange};

    struct WindowClock([u64; QUANTUM]);
    impl hex_lattice::NativeInstrumentClock for WindowClock {
        fn beat_at_offset(&self, _: usize) -> Option<u64> {
            None
        }
        fn arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
            self.0.get(offset).copied()
        }
        fn transport_is_running(&self) -> Option<bool> {
            Some(true)
        }
    }

    let (mut callbacks, mut endpoints) = endpoints();
    let endpoint = &mut endpoints[0];
    let node = endpoint.facts.node();

    // Single-segment delayed OriginWindow-style interior wrap: [126, 127, 64, 65].
    {
        let mut output = [[0.25; 2]; QUANTUM];
        let clock = WindowClock([126, 127, 64, 65]);
        assert_eq!(
            allocations_during(|| {
                callbacks.begin_host_callback(QUANTUM).unwrap();
                callbacks
                    .process_segment(node, 0, &mut output, Some(&clock))
                    .unwrap();
                callbacks.finish_host_callback(true).unwrap();
            }),
            [0; 4]
        );
        let mut req = endpoint.worker.receive().unwrap().unwrap();
        assert_eq!(
            req.timing,
            Some(ProcessTiming {
                arrangement_frame: 126,
                playing: true,
                ..Default::default()
            })
        );
        assert_eq!(
            req.timing_changes.as_slice(),
            &[TimingChange {
                frame: 2,
                timing: ProcessTiming {
                    arrangement_frame: 64,
                    playing: true,
                    ..Default::default()
                },
            }]
        );
        req.timing_changes = Default::default();
        endpoint.worker.complete(req).unwrap();
    }

    // Two-callback interior wrap: [127] then [64, 65, 66].
    {
        let mut out_a = [[0.25; 2]; 1];
        let mut out_b = [[0.25; 2]; 3];
        let clock_a = WindowClock([127, 0, 0, 0]);
        let clock_b = WindowClock([64, 65, 66, 0]);
        assert_eq!(
            allocations_during(|| {
                callbacks.begin_host_callback(1).unwrap();
                callbacks
                    .process_segment(node, 0, &mut out_a, Some(&clock_a))
                    .unwrap();
                callbacks.finish_host_callback(true).unwrap();
                callbacks.begin_host_callback(3).unwrap();
                callbacks
                    .process_segment(node, 0, &mut out_b, Some(&clock_b))
                    .unwrap();
                callbacks.finish_host_callback(true).unwrap();
            }),
            [0; 4]
        );
        let mut req = endpoint.worker.receive().unwrap().unwrap();
        assert_eq!(
            req.timing,
            Some(ProcessTiming {
                arrangement_frame: 127,
                playing: true,
                ..Default::default()
            })
        );
        assert_eq!(
            req.timing_changes.as_slice(),
            &[TimingChange {
                frame: 1,
                timing: ProcessTiming {
                    arrangement_frame: 64,
                    playing: true,
                    ..Default::default()
                },
            }]
        );
        req.timing_changes = Default::default();
        endpoint.worker.complete(req).unwrap();
    }

    // Contiguous two-callback audio [67, 68] + [69, 70] emits zero timing changes.
    {
        let mut out_a = [[0.25; 2]; 2];
        let mut out_b = [[0.25; 2]; 2];
        let clock_a = WindowClock([67, 68, 0, 0]);
        let clock_b = WindowClock([69, 70, 0, 0]);
        assert_eq!(
            allocations_during(|| {
                callbacks.begin_host_callback(2).unwrap();
                callbacks
                    .process_segment(node, 0, &mut out_a, Some(&clock_a))
                    .unwrap();
                callbacks.finish_host_callback(true).unwrap();
                callbacks.begin_host_callback(2).unwrap();
                callbacks
                    .process_segment(node, 0, &mut out_b, Some(&clock_b))
                    .unwrap();
                callbacks.finish_host_callback(true).unwrap();
            }),
            [0; 4]
        );
        let req = endpoint.worker.receive().unwrap().unwrap();
        assert_eq!(req.timing.unwrap().arrangement_frame, 67);
        assert!(req.timing_changes.as_slice().is_empty());
        endpoint.worker.complete(req).unwrap();
    }

    // Wrap aligned to next block boundary ([124..128) then [64..68)) emits zero splits.
    for frames in [[124, 125, 126, 127], [64, 65, 66, 67]] {
        let mut output = [[0.25; 2]; QUANTUM];
        let clock = WindowClock(frames);
        assert_eq!(
            allocations_during(|| {
                callbacks.begin_host_callback(QUANTUM).unwrap();
                callbacks
                    .process_segment(node, 0, &mut output, Some(&clock))
                    .unwrap();
                callbacks.finish_host_callback(true).unwrap();
            }),
            [0; 4]
        );
        let req = endpoint.worker.receive().unwrap().unwrap();
        assert_eq!(req.timing.unwrap().arrangement_frame, frames[0] as i64);
        assert!(req.timing_changes.as_slice().is_empty());
        endpoint.worker.complete(req).unwrap();
    }
}

#[test]
fn live_bypass_crossfades_aligned_plugin_output_and_survives_rapid_reversal() {
    for gain in [0.001_f32, 0.5, 1.0, 2.0, 15.0] {
        let (mut callbacks, mut endpoints) = endpoints();
        let mut run = |bypassed: bool, frames: usize, split: bool| {
            let mut node = endpoints[0].facts.node();
            node.bypassed = bypassed;
            node.output_gain = gain;
            let mut output = [[0.25, -0.25]; QUANTUM];
            let measured = allocations_during(|| {
                callbacks.begin_host_callback(frames).unwrap();
                if split && frames > 1 {
                    callbacks
                        .process_segment(node, 0, &mut output[..1], None)
                        .unwrap();
                    callbacks
                        .process_segment(node, 1, &mut output[1..frames], None)
                        .unwrap();
                } else {
                    callbacks
                        .process_segment(node, 0, &mut output[..frames], None)
                        .unwrap();
                }
                callbacks.finish_host_callback(true).unwrap();
            });
            assert_eq!(measured, [0; 4]);
            while let Some(mut block) = endpoints[0].worker.receive().unwrap() {
                for frame in &mut block.pcm[..QUANTUM] {
                    frame[0] *= 2.0;
                    frame[1] *= 2.0;
                }
                endpoints[0].worker.complete(block).unwrap();
            }
            output
        };
        // The physical 2Q transport and dry delay start in silence.
        for _ in 0..4 {
            run(false, QUANTUM, false);
        }
        let start = run(false, QUANTUM, true);
        let wet = 0.5 * gain;
        assert!((start[3][0] - wet).abs() < 1e-6);
        let mut elapsed = 0;
        for frames in [1, 3, 4, 4, 4, 4] {
            let output = run(true, frames, true);
            for frame in &output[..frames] {
                elapsed += 1;
                let expected = wet + (0.25 - wet) * elapsed as f32 / 240.0;
                assert!(
                    (frame[0] - expected).abs() < 1e-5,
                    "bypass frame {elapsed}: {frame:?} expected {expected}"
                );
            }
        }
        let from = wet + (0.25 - wet) * elapsed as f32 / 240.0;
        let reversed = run(false, QUANTUM, true);
        for (i, frame) in reversed.iter().enumerate() {
            let expected = from + (wet - from) * (i + 1) as f32 / 240.0;
            assert!(
                (frame[0] - expected).abs() < 1e-5,
                "reverse frame {i}: {frame:?} expected {expected}"
            );
        }
    }
}

#[test]
fn plugin_bypass_keeps_host_output_trim_on_the_wet_side_only() {
    use hex_lattice::{
        NativeRuntimeChain, PreparedProcessorFactory, PreparedRuntimeInstance, StereoMatrix,
    };
    use std::sync::Arc;
    let (mut callbacks, mut endpoints) = endpoints();
    let state = endpoints[0]
        .facts
        .state
        .clone()
        .with_output_gain_db(6.0)
        .unwrap();
    let gain = state.output_gain_linear();
    let prepared = endpoints[0].facts.prepared;
    let identity = endpoints[0].facts.identity;
    let make = |bypassed| {
        NativeRuntimeChain::new_prepared(vec![PreparedRuntimeInstance {
            modulation: None,
            automation: Default::default(),
            meter: Default::default(),
            owner: identity.owner(),
            device_id: identity.device_id(),
            device_type: state.type_id(),
            matrix: StereoMatrix::identity(),
            bypassed,
            sample_rate: 48_000,
            configuration: Some(Arc::new(state.clone())),
            definition: Some(PreparedProcessorFactory::Foreign(prepared)),
        }])
        .unwrap()
    };
    let mut old = make(false);
    let mut bypass = make(true);
    assert!(bypass.configuration_matches_range(&old, 0..1));
    let mut run = |chain: &mut NativeRuntimeChain| {
        let mut output = [[0.25, -0.25]; QUANTUM];
        let measured = allocations_during(|| {
            callbacks.begin_host_callback(QUANTUM).unwrap();
            chain.begin_block();
            chain
                .process_range_with_foreign(0..1, &mut output, 0, None, None, &mut callbacks)
                .unwrap();
            chain.commit_block();
            callbacks.finish_host_callback(true).unwrap();
        });
        assert_eq!(measured, [0; 4]);
        while let Some(mut block) = endpoints[0].worker.receive().unwrap() {
            for frame in &mut block.pcm[..QUANTUM] {
                frame[0] *= 2.0;
                frame[1] *= 2.0;
            }
            endpoints[0].worker.complete(block).unwrap();
        }
        output
    };
    for _ in 0..4 {
        run(&mut old);
    }
    let before = run(&mut old);
    let wet = 0.5 * gain;
    assert!((before[3][0] - wet).abs() < 1e-6);
    bypass.transfer_history_from(&mut old);
    let first = run(&mut bypass);
    for (index, frame) in first.iter().enumerate() {
        let expected = wet + (0.25 - wet) * (index + 1) as f32 / 240.0;
        assert!(
            (frame[0] - expected).abs() < 1e-5,
            "{index}: {frame:?} expected {expected}"
        );
    }
    for _ in 0..59 {
        run(&mut bypass);
    }
    let dry = run(&mut bypass);
    assert_eq!(dry, [[0.25, -0.25]; QUANTUM]);
}

#[test]
fn invalid_output_trim_rejects_the_foreign_epoch() {
    let (mut callbacks, endpoints) = endpoints();
    let mut node = endpoints[0].facts.node();
    node.output_gain = -1.0;
    let mut frames = [[0.25; 2]; QUANTUM];
    callbacks.begin_host_callback(QUANTUM).unwrap();
    assert_eq!(
        callbacks.process_segment(node, 0, &mut frames, None),
        Err(DeviceContractError::ForeignProcessingUnavailable)
    );
    assert!(callbacks.cycle.revoked());
}

#[test]
fn endpoint_returns_post_trim_without_amplifying_extreme_finite_samples() {
    let (mut callbacks, mut endpoints) = endpoints();
    let source = f32::MAX / 4.0;
    for block in 0..5 {
        let mut node = endpoints[0].facts.node();
        node.output_gain = 0.001;
        let mut frames = [[source, -source]; QUANTUM];
        let measured = allocations_during(|| {
            callbacks.begin_host_callback(QUANTUM).unwrap();
            callbacks
                .process_segment(node, 0, &mut frames, None)
                .unwrap();
            callbacks.finish_host_callback(true).unwrap();
        });
        assert_eq!(measured, [0; 4]);
        if block >= LATENCY_CALLBACKS {
            assert!(frames.iter().flatten().all(|sample| sample.is_finite()));
            assert_eq!(frames[0][0], source * 0.001);
        }
        serve(&mut endpoints[0].worker);
    }
    let mut node = endpoints[0].facts.node();
    node.bypassed = true;
    node.output_gain = 0.001;
    let mut dry = [[source, -source]; QUANTUM];
    callbacks.begin_host_callback(QUANTUM).unwrap();
    callbacks.process_segment(node, 0, &mut dry, None).unwrap();
    callbacks.finish_host_callback(true).unwrap();
    for (i, frame) in dry.iter().enumerate() {
        let expected = f64::from(source) * (0.001 + 0.999 * (i + 1) as f64 / 240.0);
        assert!(frame[0].is_finite());
        assert!((f64::from(frame[0]) / expected - 1.0).abs() < 1e-5);
    }
    serve(&mut endpoints[0].worker);
    let mut node = endpoints[0].facts.node();
    node.output_gain = 0.001;
    node.bypassed = false;
    let mut reversed = [[source, -source]; QUANTUM];
    callbacks.begin_host_callback(QUANTUM).unwrap();
    callbacks
        .process_segment(node, 0, &mut reversed, None)
        .unwrap();
    callbacks.finish_host_callback(true).unwrap();
    assert!(reversed.iter().flatten().all(|sample| sample.is_finite()));
    assert!(reversed[0][0] > source * 0.001);
}

#[test]
fn actual_chain_receives_post_trim_once_and_untrimmed_aligned_bypass() {
    use hex_lattice::{
        NativeRuntimeChain, PreparedProcessorFactory, PreparedRuntimeInstance, StereoMatrix,
    };
    use std::sync::Arc;
    let (mut callbacks, mut endpoints) = endpoints();
    let state = endpoints[0]
        .facts
        .state
        .clone()
        .with_output_gain_db(-60.0)
        .unwrap();
    let gain = state.output_gain_linear();
    let prepared = endpoints[0].facts.prepared;
    let identity = endpoints[0].facts.identity;
    let make = |bypassed| {
        NativeRuntimeChain::new_prepared(vec![PreparedRuntimeInstance {
            modulation: None,
            automation: Default::default(),
            meter: Default::default(),
            owner: identity.owner(),
            device_id: identity.device_id(),
            device_type: state.type_id(),
            matrix: StereoMatrix::identity(),
            bypassed,
            sample_rate: 48_000,
            configuration: Some(Arc::new(state.clone())),
            definition: Some(PreparedProcessorFactory::Foreign(prepared)),
        }])
        .unwrap()
    };
    let mut active = make(false);
    let mut bypass = make(true);
    let source = f32::MAX / 4.0;
    let mut run = |chain: &mut NativeRuntimeChain| {
        let mut output = [[source, -source]; QUANTUM];
        let measured = allocations_during(|| {
            callbacks.begin_host_callback(QUANTUM).unwrap();
            chain.begin_block();
            chain
                .process_range_with_foreign(0..1, &mut output, 0, None, None, &mut callbacks)
                .unwrap();
            chain.commit_block();
            callbacks.finish_host_callback(true).unwrap();
        });
        assert_eq!(measured, [0; 4]);
        serve(&mut endpoints[0].worker);
        output
    };
    for _ in 0..4 {
        run(&mut active);
    }
    assert_eq!(run(&mut active)[0], [source * gain, -source * gain]);
    bypass.transfer_history_from(&mut active);
    let first = run(&mut bypass)[0];
    let expected = f64::from(source) * (f64::from(gain) + (1.0 - f64::from(gain)) / 240.0);
    assert!(first[0].is_finite());
    assert!((f64::from(first[0]) / expected - 1.0).abs() < 1e-5);
}

#[test]
fn revoked_endpoint_passes_aligned_dry_with_current_enabled_trim() {
    let (mut callbacks, mut endpoints) = endpoints();
    let source = [[0.25, -0.25]; QUANTUM];
    for block in 0..7 {
        let mut node = endpoints[0].facts.node();
        node.output_gain = 0.5;
        let mut output = source;
        callbacks.begin_host_callback(QUANTUM).unwrap();
        callbacks
            .process_segment(node, 0, &mut output, None)
            .unwrap();
        callbacks.finish_host_callback(true).unwrap();
        if block >= LATENCY_CALLBACKS {
            assert_eq!(output, [[0.125, -0.125]; QUANTUM]);
        }
        if block == 0 {
            serve(&mut endpoints[0].worker);
        }
    }
    assert!(!callbacks.entries[0].transport_is_live());
    let mut node = endpoints[0].facts.node();
    node.bypassed = true;
    node.output_gain = 0.5;
    let mut output = source;
    callbacks.begin_host_callback(QUANTUM).unwrap();
    callbacks
        .process_segment(node, 0, &mut output, None)
        .unwrap();
    callbacks.finish_host_callback(true).unwrap();
    assert_eq!(output, source);
}

#[test]
fn settled_main_pass_copy_uses_committed_519_frame_ring_without_allocating() {
    const LATENCY: usize = 519;
    let binding = Binding {
        root: 1,
        instance: 2,
        graph: 3,
        epoch: 4,
    };
    let identity = ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, 2).unwrap();
    let state =
        hex_engine::default_native_device_state(hex_devices::utility::UTILITY_TYPE_ID).unwrap();
    let (control, transport, _worker) = channel(binding, 256).unwrap();
    let mut callback = NativeEffectCallback::for_test(identity, binding, state, transport);
    callback.configuration.prepared = hex_lattice::PreparedForeignProcessor::new(256, 7, 512)
        .unwrap()
        .with_binding(binding)
        .unwrap();
    callback.dry.resize(LATENCY, [0.0; 2]);
    let input = |index: usize| [index as f32 / 1024.0, -(index as f32) / 1024.0];
    let prefix: [_; LATENCY + 13] = std::array::from_fn(input);
    for block in prefix.chunks(256) {
        callback.advance_dry(block);
    }
    let mut entry = NativeEffectEntry::new(callback);
    assert!(entry.settled_main_pass_delay(519).unwrap().is_none());
    for bypassed in [false, true] {
        entry.callback.bypass.process(
            &mut [[0.0; 2]; 256],
            &[[0.0; 2]; 256],
            bypassed,
            48_000,
            1.0,
        );
    }
    assert!(entry.settled_main_pass_delay(512).is_err());
    entry.staged = true;
    assert!(entry.settled_main_pass_delay(519).unwrap().is_none());
    entry.staged = false;
    for partition in [1, 257] {
        let mut output: [_; LATENCY + 17] =
            std::array::from_fn(|index| input(prefix.len() + index));
        assert_eq!(
            allocations_during(|| {
                let mut delay = entry.settled_main_pass_delay(519).unwrap().unwrap();
                for block in output.chunks_mut(partition) {
                    delay.process(block);
                }
            }),
            [0; 4]
        );
        for (index, frame) in output.into_iter().enumerate() {
            assert_eq!(frame, input(13 + index));
        }
    }
    control.revoke();
    assert!(entry.settled_main_pass_delay(519).is_err());
}

#[test]
fn settled_return_pass_requires_the_exact_accepted_frame_and_owner() {
    let binding = Binding {
        root: 1,
        instance: 2,
        graph: 3,
        epoch: 4,
    };
    let identity = ProcessorInstanceIdentity::new(ProcessorChainOwner::Return(5), 2).unwrap();
    let state =
        hex_engine::default_native_device_state(hex_devices::utility::UTILITY_TYPE_ID).unwrap();
    let (control, transport, _worker) = channel(binding, 256).unwrap();
    let callback = NativeEffectCallback::for_test(identity, binding, state, transport);
    let mut entry = NativeEffectEntry::new(callback);
    assert_eq!(entry.settled_return_pass(identity, 768), Ok(false));
    // Model the exact committed endpoint facts at the public callback seam.
    // The signed pass test checks their real worker and render progression.
    entry
        .callback
        .bypass
        .process(&mut [[0.0; 2]; 256], &[[0.0; 2]; 256], true, 48_000, 1.0);
    entry.callback.return_bypass_frame = std::num::NonZeroU64::new(768);
    let other = ProcessorInstanceIdentity::new(ProcessorChainOwner::Return(6), 2).unwrap();
    assert_eq!(
        allocations_during(|| {
            assert_eq!(entry.settled_return_pass(identity, 768), Ok(true));
            assert!(entry.settled_return_pass(identity, 769).is_err());
            assert!(entry.settled_return_pass(other, 768).is_err());
        }),
        [0; 4]
    );
    entry.staged = true;
    assert!(entry.settled_return_pass(identity, 768).is_err());
    entry.staged = false;
    control.revoke();
    assert!(entry.settled_return_pass(identity, 768).is_err());
}

#[test]
fn late_native_effect_replies_use_aligned_dry_and_resume_wet() {
    let binding = Binding {
        root: 1,
        instance: 2,
        graph: 1,
        epoch: 1,
    };
    let identity = ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, 2).unwrap();
    let state =
        hex_engine::default_native_device_state(hex_devices::utility::UTILITY_TYPE_ID).unwrap();
    let (control, transport, mut worker) =
        hex_plugin_host::transport::native_pcm_channel_from_completed_prefix(binding, 4, 0)
            .unwrap();
    let mut endpoint = NativeEffectCallback::for_test(identity, binding, state.clone(), transport);
    let facts = Facts {
        identity,
        state,
        prepared: endpoint.configuration.prepared,
    };
    let mut position = 0;
    let mut render = |expected: &[f32], concealed| {
        let mut samples = [[0.0; 2]; QUANTUM];
        let frames = &mut samples[..expected.len()];
        for (index, frame) in frames.iter_mut().enumerate() {
            let value = (position + index + 1) as f32;
            *frame = [value, -value];
        }
        assert_eq!(
            allocations_during(|| {
                endpoint.begin_host_callback(frames.len()).unwrap();
                endpoint
                    .process_segment(facts.node(), 0, frames, None)
                    .unwrap();
                endpoint.finish_host_callback(true).unwrap();
            }),
            [0; 4]
        );
        for (frame, &value) in frames.iter().zip(expected) {
            assert_eq!(*frame, [value, -value]);
        }
        assert_eq!(endpoint.concealed_output, concealed);
        position += frames.len();
    };
    let respond = |worker: &mut Worker, sequence| {
        let mut block = worker.receive().unwrap().unwrap();
        assert_eq!(block.sequence, sequence);
        for sample in block.pcm.iter_mut().flatten() {
            *sample *= 2.0;
        }
        worker.complete(block).unwrap();
    };

    render(&[0.0; 4], false);
    render(&[0.0; 2], false);
    render(&[0.0, 0.0, 1.0, 2.0], true);
    // A second missing quantum starts in the same partial callback.
    render(&[3.0, 4.0, 5.0, 6.0], true);
    respond(&mut worker, 0); // Late reply cannot replace the dry quantum.
    respond(&mut worker, 1);
    respond(&mut worker, 2);
    render(&[7.0, 8.0, 18.0, 20.0], true);
    respond(&mut worker, 3);
    render(&[22.0, 24.0, 26.0, 28.0], false);
    assert!(!control.is_revoked());
    assert_eq!(control.take_callback_fault(), None);
}

#[test]
fn one_endpoint_missing_its_reply_degrades_alone_while_the_other_keeps_processing() {
    let (mut callbacks, mut endpoints) = endpoints();
    let mut outputs = [[[0.0; 2]; QUANTUM]; 2];
    // The faulty worker answers its first request, then stops replying, so
    // its third callback misses the reply its fixed pipeline deadline needs.
    const STALLED_AFTER: usize = 1;
    const CALLBACKS: usize = 8;
    let mut allocations = [0; 4];
    for callback in 0..CALLBACKS {
        let source = input(callback);
        let mut results = [Ok(()), Ok(()), Ok(())];
        let measured = allocations_during(|| {
            results = render(&mut callbacks, &endpoints, &source, &mut outputs);
        });
        for (total, measured) in allocations.iter_mut().zip(measured) {
            *total += measured;
        }
        assert_eq!(results, [Ok(()), Ok(()), Ok(())], "callback {callback}");
        let expected = callback
            .checked_sub(LATENCY_CALLBACKS)
            .map_or([[0.0; 2]; QUANTUM], input);
        assert_eq!(
            outputs[0], expected,
            "healthy endpoint at callback {callback}"
        );
        // The degraded endpoint is replaced by the same aligned dry signal
        // its healthy output would have produced, never by silence.
        assert_eq!(
            outputs[1], expected,
            "degraded endpoint at callback {callback}"
        );
        serve(&mut endpoints[0].worker);
        if callback < STALLED_AFTER {
            serve(&mut endpoints[1].worker);
        }
    }
    // Total and signed-net counts and bytes, so deallocation is caught too.
    assert_eq!(allocations, [0; 4], "contained fault path allocated");
    assert!(!callbacks.cycle.revoked(), "the output stayed live");
    let healthy = &callbacks.entries[0];
    let degraded = &callbacks.entries[1];
    assert!(healthy.transport_is_live());
    assert_eq!(healthy.callback.transport.fault(), None);
    assert!(!degraded.transport_is_live());
    assert_eq!(
        degraded.callback.transport.fault(),
        Some(Fault::MissingOutput)
    );
    // Control sees the degraded entry through its own worker lease only.
    assert_eq!(endpoints[1].worker.receive().err(), Some(Fault::Revoked));
    assert_eq!(endpoints[0]._control.take_callback_fault(), None);
    assert_eq!(
        endpoints[1]._control.take_callback_fault(),
        Some(Fault::MissingOutput)
    );
    assert_eq!(endpoints[1]._control.take_callback_fault(), None);
    assert_eq!(
        endpoints[0].worker.completed_blocks(),
        CALLBACKS as u64,
        "the healthy worker processed every callback"
    );
}

#[test]
fn a_mismatched_graph_node_still_fails_the_whole_callback() {
    let (mut callbacks, endpoints) = endpoints();
    let mut frames = input(0);
    callbacks.begin_host_callback(QUANTUM).expect("begin");
    let mut node = endpoints[0].facts.node();
    node.sample_rate = 44_100;
    assert_eq!(
        callbacks.process_segment(node, 0, &mut frames, None),
        Err(DeviceContractError::ForeignProcessingUnavailable)
    );
    assert_eq!(frames, [[0.0; 2]; QUANTUM]);
    assert!(callbacks.cycle.revoked());
    assert!(
        callbacks
            .entries
            .iter()
            .all(|entry| !entry.transport_is_live())
    );
}
