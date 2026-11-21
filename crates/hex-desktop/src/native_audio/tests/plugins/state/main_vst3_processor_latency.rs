//! Retire an original SDK processor without retiring the current Main clock.
use super::main_vst3_pass::{WORKERLESS_DRY_PROBE_FRAMES, assert_workerless_dry_partition};
use super::*;
use hex_audio::prepared::FreshRuntimeCancellationResult;
use hex_session::service::{
    DeviceInsertOutcome, GraphEditOperation, ResidentGraphSubmission, ViewState,
};

const BLOCK: usize = 256;
const PROCESSOR_FRAMES: usize = 7;
const TOTAL_DELAY: usize = 2 * BLOCK + PROCESSOR_FRAMES;
const IMPULSE: usize = 192;
const SPACING: usize = 4 * BLOCK;
const SOURCE_FRAMES: u32 = 131_072;
const FADE: usize = 240;

fn processor_state(selection: &PluginEffectSelection) -> NativeDeviceState {
    let (executable_sha256, info_sha256) =
        Hosting::inspect_fixture_binary(&selection.module, &selection.executable, selection.cid)
            .unwrap();
    // The signed original processing fixture restores this component layout
    // before activation. Its getLatencySamples and actual sample ring then agree.
    let mut component = b"HXPS".to_vec();
    component.extend((PROCESSOR_FRAMES as u32).to_le_bytes());
    component.extend(1.0_f32.to_le_bytes());
    component.extend(48_000.0_f64.to_le_bytes());
    component.extend([2, 2, 0, 0]);
    component.extend([0; 8]);
    hex_plugin_state::encode(hex_plugin_state::State {
        identity: hex_plugin_state::Identity {
            class: hex_plugin_state::ClassId::new(selection.cid).unwrap(),
            binary: hex_plugin_state::BinaryIdentity {
                executable_sha256,
                info_sha256,
            },
        },
        component: Some(&component),
        controller: None,
    })
    .unwrap()
}

fn source_wave(files: &Files) -> PathBuf {
    let wave = files.wav(48_000, SOURCE_FRAMES);
    let mut bytes = fs::read(&wave).unwrap();
    for (at, sample) in bytes[44..].chunks_exact_mut(2).enumerate() {
        sample.copy_from_slice(&((source_sample(at) * 32_768.0) as i16).to_le_bytes());
    }
    fs::write(&wave, bytes).unwrap();
    wave
}

fn source_sample(at: usize) -> f32 {
    if at >= SOURCE_FRAMES as usize {
        0.0
    } else if at % SPACING == IMPULSE {
        0.25
    } else if at >= 128 {
        0.03125
    } else {
        0.0
    }
}

fn status(h: &ResidentHarness) -> &hex_audio::prepared::Status {
    h.audio.as_ref().unwrap().status().unwrap()
}

fn render_playing(h: &ResidentHarness) -> (usize, [hex_engine::Frame; BLOCK]) {
    let start = usize::try_from(status(h).position.load(Acquire)).unwrap();
    let pcm = h.render_plugin_block();
    edits::workers_idle(h);
    assert_eq!(status(h).position.load(Acquire), (start + BLOCK) as u64);
    assert!(status(h).playing.load(Acquire));
    (start, pcm)
}

fn assert_pcm(
    (start, pcm): (usize, [hex_engine::Frame; BLOCK]),
    expected: impl Fn(usize, usize) -> f32,
) {
    for (offset, frame) in pcm.into_iter().enumerate() {
        let at = start + offset;
        let expected = expected(at, offset);
        assert!(
            frame.into_iter().all(|lane| (lane - expected).abs() < 1e-6),
            "signed Main sample at {at}: {frame:?} vs {expected}"
        );
    }
}

fn dry(at: usize, delay: usize) -> f32 {
    at.checked_sub(delay).map_or(0.0, source_sample)
}

fn wet(at: usize) -> f32 {
    at.checked_sub(TOTAL_DELAY).map_or(0.0, |origin| {
        source_sample(origin) * (0.5 + (origin / BLOCK) as f32 * 0.125)
    })
}

fn assert_aligned_fade(block: (usize, [hex_engine::Frame; BLOCK]), to_dry: bool) {
    assert_pcm(block, |at, offset| {
        let progress = (offset + 1).min(FADE) as f32 / FADE as f32;
        let dry_gain = if to_dry { progress } else { 1.0 - progress };
        dry(at, TOTAL_DELAY) * dry_gain + wet(at) * (1.0 - dry_gain)
    });
}

fn begin_live_removal(
    h: &mut ResidentHarness,
    device: hex_project::DeviceId,
    original: &ViewState,
) -> ResidentGraphSubmission {
    let (submitted, resume) = h.probe_next_submitted_device_insert();
    h.service
        .submit(Request::Edit(Command::RemoveDevice(device)))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        if let Ok(exact) = submitted.try_recv() {
            resume.send(()).unwrap();
            return exact;
        }
        h.pump();
        if let Ok(event) = h.service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            assert!(!event.request_complete);
            assert_eq!(event.state.project, original.project);
            if let Some(offer) = event.device_insert_confirmation {
                assert!(offer.cancel());
                panic!("healthy signed 519-frame Main removal required a restart");
            }
        }
        assert!(
            Instant::now() < deadline,
            "Main removal submission timed out"
        );
        std::thread::yield_now();
    }
}

fn removal_terminal(h: &mut ResidentHarness) -> Event {
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        h.pump();
        if let Ok(event) = h.service.try_event() {
            assert!(event.device_insert_confirmation.is_none());
            if event.request_complete {
                return event;
            }
        }
        assert!(Instant::now() < deadline, "Main removal terminal timed out");
        std::thread::yield_now();
    }
}

fn assert_live_removal_fade(
    h: &mut ResidentHarness,
    exact: ResidentGraphSubmission,
    original: &ViewState,
) {
    let predecessor = status(h).recording_source().unwrap();
    let (start, pcm) = render_playing(h);
    if pcm
        .iter()
        .enumerate()
        .all(|(offset, frame)| *frame == [wet(start + offset); 2])
    {
        // Submission only enqueues the candidate. An unsupported callback
        // cut still renders the original wet block before Session offers consent.
        assert_eq!(status(h).recording_source(), Some(predecessor));
        let deadline = Instant::now() + Duration::from_millis(300);
        loop {
            h.pump();
            if let Ok(event) = h.service.try_event() {
                assert!(event.error.is_none(), "{:?}", event.error);
                assert!(!event.request_complete);
                assert_eq!(event.state.project, original.project);
                if let Some(offer) = event.device_insert_confirmation {
                    assert_eq!(offer.operation(), GraphEditOperation::RemoveDevice);
                    assert_eq!(offer.submission(), exact);
                    assert!(offer.cancel());
                    let cancelled = removal_terminal(h);
                    let completion = cancelled.device_insert_complete.unwrap();
                    assert_eq!(completion.submission, exact);
                    assert_eq!(completion.outcome, DeviceInsertOutcome::Cancelled);
                    assert_eq!(cancelled.state.project, original.project);
                    assert_eq!(status(h).recording_source(), Some(predecessor));
                    assert_pcm(render_playing(h), |at, _| wet(at));
                    panic!("healthy signed 519-frame Main removal required a timing restart");
                }
            }
            assert!(
                Instant::now() < deadline,
                "Main removal rendered an unchanged wet block without offering a timing restart"
            );
            std::thread::yield_now();
        }
    }
    assert_aligned_fade((start, pcm), true);
}

#[test]
#[ignore = "explicit source-bound original SDK receipts required"]
fn running_main_vst3_with_sdk_processor_latency_retires_worker_and_keeps_pass_timing() {
    const CHILD: &str = "HEX_DESKTOP_MAIN_PROCESSOR_LATENCY_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                std::thread::current().name().unwrap(),
                "--ignored",
                "--test-threads=1",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "37")
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&child.stdout),
            String::from_utf8_lossy(&child.stderr)
        );
        assert!(String::from_utf8_lossy(&child.stdout).contains("1 passed"));
        return;
    }

    let _native = native_fixture();
    let files = Files::new();
    let directory = files.0.canonicalize().unwrap();
    let selection = selection();
    let state = processor_state(&selection);
    hex_ui::write_plugin_effect_test_catalog(&directory, &selection, "Fx", 0).unwrap();
    // Open must retain the imported document until the new graph applies.
    // Reserve PCM for both copies so Open keeps the replacement resident.
    let mut h = ResidentHarness::with_plugins_at_rate_and_catalog(
        host(),
        None,
        48_000,
        directory.join("plugin-catalog.json"),
        4 * u64::from(SOURCE_FRAMES),
    );
    assert!(h.next().error.is_none());
    let (empty, _) = h.applied(0);
    let imported = h.run(Request::Import(source_wave(&files)));
    h.applied(empty);
    let mut editor = hex_project::Editor::new(imported.state.project.as_ref().clone(), 8).unwrap();
    let kind = state.type_id();
    let Created::Device(device) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state,
        })
        .unwrap()
    else {
        panic!("fixture VST3");
    };
    let project = directory.join("processor-latency.hex");
    hex_storage::save_new(&project, editor.project()).unwrap();
    let old_output = status(&h).recording_source().unwrap().output_identity;
    let opened = h.run(Request::Open(project));
    super::super::super::resident_lifecycle::finish_replacement(&mut h, &opened, old_output);
    let restored = h.next();
    assert!(restored.error.is_none(), "{:?}", restored.error);
    assert_eq!(restored.state.project.as_ref(), editor.project());
    let source = restored.state.canonical_audio_source.as_ref().unwrap();
    let mut bindings = source.required_sources();
    assert!(matches!(
        bindings.next(),
        Some((_, hex_session::AudioSourceBinding::Resident(_)))
    ));
    assert!(bindings.next().is_none());
    let before = status(&h).recording_source().unwrap();
    let observation = h.submitted_observation().unwrap();
    assert_eq!(
        observation.foreign_processor(device.value(), kind),
        Some(true)
    );
    assert_eq!(
        observation
            .processor_latency(device.value(), kind)
            .unwrap()
            .stage_frames,
        TOTAL_DELAY as u32
    );
    assert_eq!(
        observation.main_latency().unwrap().arrival_frames,
        TOTAL_DELAY as u32
    );
    wait(|| h.plugin_progress.lock().unwrap().source_bindings[0].is_some());
    let (worker_device, binding, _) = h.plugin_progress.lock().unwrap().source_bindings[0].unwrap();
    assert_eq!(worker_device, device.value());
    h.enable_confirmed_device_inserts();
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for _ in 0..4 {
        assert_pcm(render_playing(&h), |at, _| wet(at));
    }

    for cancel in [true, false] {
        let exact = begin_live_removal(&mut h, device, &restored.state);
        assert_eq!(exact.output_identity, before.output_identity);
        assert_eq!(exact.context.editor_revision, restored.state.revision + 1);
        assert_eq!(status(&h).recording_source(), Some(before));
        assert_eq!(h.plugin_progress.lock().unwrap().binding, Some(binding));
        assert_live_removal_fade(&mut h, exact, &restored.state);
        assert_eq!(status(&h).recording_source(), Some(before));
        if !cancel {
            assert_pcm(render_playing(&h), |at, _| dry(at, TOTAL_DELAY));
            let applied = status(&h).recording_source().unwrap();
            assert_eq!(applied.output_identity, exact.output_identity);
            assert_eq!(applied.context, exact.context);
            assert_eq!(applied.audio_revision, exact.audio_revision);
            let installed = removal_terminal(&mut h);
            assert!(installed.error.is_none(), "{:?}", installed.error);
            let completion = installed.device_insert_complete.unwrap();
            assert_eq!(completion.submission, exact);
            assert_eq!(completion.outcome, DeviceInsertOutcome::Installed);
            assert!(installed.state.project.device(device).is_none());
            assert_eq!(installed.state.revision, restored.state.revision + 1);
            break;
        }

        wait(|| {
            match h
                .audio
                .as_ref()
                .unwrap()
                .control
                .cancel_fresh_runtime_publication(exact.audio_revision)
            {
                FreshRuntimeCancellationResult::Busy => false,
                result => {
                    assert_eq!(result, FreshRuntimeCancellationResult::Cancelled);
                    true
                }
            }
        });
        assert_aligned_fade(render_playing(&h), false);
        let cancelled = removal_terminal(&mut h);
        let completion = cancelled.device_insert_complete.unwrap();
        assert_eq!(completion.outcome, DeviceInsertOutcome::Cancelled);
        assert_eq!(completion.submission, exact);
        assert_eq!(cancelled.state.project, restored.state.project);
        assert_eq!(status(&h).recording_source(), Some(before));
        assert_eq!(h.plugin_progress.lock().unwrap().binding, Some(binding));
        assert_pcm(render_playing(&h), |at, _| wet(at));
    }

    let deadline = Instant::now() + Duration::from_secs(1);
    while h.plugin_records.load(Acquire) != 0 {
        h.pump();
        assert_pcm(render_playing(&h), |at, _| dry(at, TOTAL_DELAY));
        assert!(
            Instant::now() < deadline,
            "removed SDK worker did not retire"
        );
        // The headless callback can render far faster than the control owner
        // can retire a process. Keep the remaining impulse train measurable.
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(h.plugin_progress.lock().unwrap().binding.is_none());
    assert!(
        status(&h).position.load(Acquire) + (2 * WORKERLESS_DRY_PROBE_FRAMES) as u64
            <= u64::from(SOURCE_FRAMES) + TOTAL_DELAY as u64,
        "the retained delay must be tested on audible PCM after worker retirement"
    );
    for partition in [1, 257] {
        assert_workerless_dry_partition(&h, partition, |at| dry(at, TOTAL_DELAY));
    }

    h.audio.as_ref().unwrap().control.stop();
    h.render_plugin_block();
    assert!(!status(&h).playing.load(Acquire));
    assert_eq!(status(&h).position.load(Acquire), 0);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for _ in 0..4 {
        assert_pcm(render_playing(&h), |at, _| dry(at, 0));
    }
}
