//! Opt-in optimized baseline using the actual resident callback and original
//! VST3 fixture. Setup, output validation and pacing stay outside render timing.
use super::super::resident_lifecycle::finish_replacement;
use super::*;
use hex_engine::{DeviceTypeId, default_native_device_state};
use hex_project::{Editor, Project};

const RATE: u32 = 48_000;
const TRACKS: usize = 48;
const SOURCE_FRAMES: u32 = 8192;
const WARMUP: u32 = 128;
const SAMPLES: u32 = 512;
const MIXED_PLUGINS: usize = 4;
// The shared headless output reserves the native host's 256-frame maximum.
const PLUGIN_QUANTUM: usize = 256;

#[test]
#[ignore = "opt-in optimized playback measurement with a fixed callback cadence"]
fn dense_native_serial_baseline() {
    profile(0);
}

#[test]
#[ignore = "opt-in optimized playback measurement with controlled native VST3 receipts"]
fn dense_mixed_serial_baseline() {
    profile(MIXED_PLUGINS);
}

fn fixture(files: &Files) -> PathBuf {
    let wav = files.wav(RATE, SOURCE_FRAMES);
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 1).unwrap();
    let Created::Imported { asset, track, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "profile audio".into(),
            path: wav.file_name().unwrap().into(),
            source_sample_rate: RATE,
            source_frames: u64::from(SOURCE_FRAMES),
            playback_frames: u64::from(SOURCE_FRAMES),
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("profile import");
    };
    for index in 0..TRACKS {
        let track = if index == 0 {
            track
        } else {
            let Created::Track(id) = editor
                .execute(Command::AddTrack {
                    name: format!("profile {index}"),
                })
                .unwrap()
            else {
                panic!("profile track");
            };
            id
        };
        if index == 0 {
            let clip = editor.project().track(track).unwrap().clips()[0].id();
            editor.execute(Command::RemoveClip(clip)).unwrap();
        }
        editor
            .execute(Command::PlaceClip {
                track,
                asset,
                start: 0,
                duration: u64::from(SOURCE_FRAMES) * 64,
                source_offset: 0,
                source_length: u64::from(SOURCE_FRAMES),
                looping: true,
            })
            .unwrap();
        editor
            .execute(Command::Gain { track, gain: 0.025 })
            .unwrap();
        for kind in [
            "hex.utility",
            if index % 4 == 0 {
                "hex.reverb"
            } else {
                "hex.saturator"
            },
        ] {
            editor
                .execute(Command::InsertDevice {
                    track,
                    before: None,
                    state: default_native_device_state(DeviceTypeId::from_static(kind)).unwrap(),
                })
                .unwrap();
        }
    }
    let path = files.0.join("dense-playback-profile.hex");
    hex_storage::save_new(&path, editor.project()).unwrap();
    path
}

fn profile(plugins: usize) {
    let _native = native_fixture();
    assert!(!cfg!(debug_assertions), "run this profile with --release");
    let block: usize = std::env::var("HEX_PLAYBACK_PROFILE_BLOCK")
        .unwrap_or_else(|_| "256".into())
        .parse()
        .unwrap();
    assert!(
        [64, 128, 256].contains(&block),
        "supported profile block sizes: 64, 128, 256"
    );
    let files = Files::new();
    let path = fixture(&files);
    let mut harness = if plugins == 0 {
        ResidentHarness::new()
    } else {
        ResidentHarness::with_plugins(
            host(),
            Some(EffectPresentation {
                input_frames: 0,
                output_frames: (PLUGIN_QUANTUM * 2) as u32,
            }),
        )
    };
    assert!(harness.next().error.is_none());
    harness.applied(0);
    let output = harness
        .audio
        .as_ref()
        .unwrap()
        .control
        .output_identity()
        .unwrap();
    let opened = harness.run(Request::Open(path));
    let mut revision = finish_replacement(&mut harness, &opened, output);
    let mut state = opened.state;
    assert_eq!(state.project.tracks().len(), TRACKS);

    for index in 0..plugins {
        let track = state.project.tracks()[index * (TRACKS / plugins)].id();
        let insertion = request(&state, PluginEffectDestination::Track(track));
        harness
            .service
            .submit(Request::InsertPluginEffect(insertion))
            .unwrap();
        let pending = harness.next();
        assert!(pending.error.is_none());
        let accepted = terminal(&mut harness);
        assert!(accepted.error.is_none(), "{:?}", accepted.error);
        assert_eq!(accepted.plugin_effect_complete.len(), 1);
        assert!(accepted.plugin_effect_complete[0].accepted);
        state = accepted.state;
        (revision, _) = harness.applied(revision);
    }
    assert_eq!(
        state
            .project
            .tracks()
            .iter()
            .map(|track| track.devices().len())
            .sum::<usize>(),
        TRACKS * 2 + plugins,
    );
    assert_eq!(harness.plugin_records.load(Acquire), plugins);
    let control = &mut harness.audio.as_mut().unwrap().control;
    control.command(PlaybackCommand::Play).unwrap();

    let budget = Duration::from_nanos(block as u64 * 1_000_000_000 / u64::from(RATE));
    let mut times = Vec::with_capacity(SAMPLES as usize);
    let mut frames = vec![[0.0; 2]; block];
    let mut host = vec![0.0; block * 2];
    let mut fingerprint = 0xcbf29ce484222325_u64;
    let mut first_signal = None;
    let mut slot = harness.callback.lock().unwrap();
    let callback = slot.as_mut().unwrap();
    let start = Instant::now();
    for index in 0..WARMUP + SAMPLES {
        if let Some(until) = (start + budget * index).checked_duration_since(Instant::now()) {
            std::thread::sleep(until);
        }
        let begin = Instant::now();
        let result = callback.render_native_direct(&mut frames, &mut host);
        let elapsed = begin.elapsed();
        assert!(
            result.is_ok(),
            "callback {index} failed after {elapsed:?}: {result:?}"
        );
        if index >= WARMUP {
            times.push(elapsed);
        }
        assert!(
            host.chunks_exact(2)
                .zip(&frames)
                .all(|(host, pcm)| host == pcm)
        );
        for (offset, frame) in frames.iter().enumerate() {
            if *frame != [0.0; 2] {
                first_signal.get_or_insert(index as usize * block + offset);
            }
            for sample in frame {
                assert!(sample.is_finite());
                fingerprint =
                    (fingerprint ^ u64::from(sample.to_bits())).wrapping_mul(0x100000001b3);
            }
        }
    }
    let status = callback.status().unwrap();
    assert!(status.playing.load(Acquire));
    assert_eq!(status.applied_revision.load(Acquire), revision);
    let position = status.position.load(Acquire);
    assert_eq!(position, u64::from(WARMUP + SAMPLES) * block as u64);
    assert!(!status.device_fault.load(Acquire) && !status.render_fault.load(Acquire));
    let audible_lag = position
        .checked_sub(status.audible_position.load(Acquire))
        .unwrap();
    let first_signal = first_signal.expect("profile must deliver nonzero audio");
    if plugins != 0 {
        assert!(
            first_signal >= 2 * PLUGIN_QUANTUM,
            "mixed output appeared before the native transport delay"
        );
        let expected_worker_blocks = position / PLUGIN_QUANTUM as u64;
        wait(|| harness.plugin_completed.load(Acquire) >= expected_worker_blocks);
    }
    let worker_blocks = harness.plugin_completed.load(Acquire);
    times.sort_unstable();
    let percentile = |percent: usize| times[(times.len() - 1) * percent / 100].as_nanos();
    let mean = times.iter().map(Duration::as_nanos).sum::<u128>() / u128::from(SAMPLES);
    let misses = times.iter().filter(|&&time| time > budget).count();
    let plugin_quantum = if plugins == 0 { 0 } else { PLUGIN_QUANTUM };
    println!(
        "dense_playback rate={RATE} block={block} tracks={TRACKS} utility=48 reverb=12 saturator=36 vst3={plugins} vst3_quantum={plugin_quantum} warmup={WARMUP} samples={SAMPLES} budget_ns={} mean_ns={mean} p50_ns={} p95_ns={} p99_ns={} max_ns={} misses={misses} first_signal={first_signal} audible_lag={audible_lag} worker_blocks={worker_blocks} pcm={fingerprint:016x}",
        budget.as_nanos(),
        percentile(50),
        percentile(95),
        percentile(99),
        percentile(100),
    );
    // The callback and native workers retire through the harness after the
    // profiling borrow ends. No worker is waited on during timed rendering.
    drop(slot);
}
