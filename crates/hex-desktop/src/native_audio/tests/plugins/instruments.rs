//! Native instrument insertion through the real Session and desktop worker.
use super::*;
use hex_engine::{LaunchQuantization::Immediate, SessionPlaybackCommand};
use hex_project::BeatTime;

mod automation_start;
mod section_edit;
mod section_recovery;
mod track_duplicate;

#[test]
#[ignore = "requires explicit source-bound SDK receipts"]
fn slow_native_instrument_exchange_reaches_the_session_log() {
    const CHILD: &str = "HEX_SLOW_EXCHANGE_LOG_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let gate = Files::new();
        let test = std::thread::current().name().unwrap().to_owned();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &test, "--ignored", "--nocapture"])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "127")
            .env("HEX_ORIGINAL_SDK_TIMING_GATE", "1")
            .env("HEX_ORIGINAL_INSTANCE_TRACE", gate.0.join("trace"))
            .env("HEX_ORIGINAL_INSTANCE_RELEASE", gate.0.join("release"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let _native = native_fixture();
    let files = Files::new();
    let trace = PathBuf::from(std::env::var_os("HEX_ORIGINAL_INSTANCE_TRACE").unwrap());
    let release = PathBuf::from(std::env::var_os("HEX_ORIGINAL_INSTANCE_RELEASE").unwrap());
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
    let gate = std::thread::spawn(move || {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            if fs::read(&trace).ok().and_then(|bytes| {
                bytes
                    .get(..4)
                    .and_then(|b| b.try_into().ok())
                    .map(i32::from_le_bytes)
            }) == Some(9)
            {
                break;
            }
            assert!(Instant::now() < until, "SDK process gate did not open");
            std::thread::sleep(Duration::from_millis(1));
        }
        ready_tx.send(()).unwrap();
        std::thread::sleep(Duration::from_millis(15));
        release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        fs::write(release, b"release").unwrap();
    });
    let selected = selection();
    hex_ui::write_plugin_effect_test_catalog(&files.0, &selected, "Instrument|Synth", 0).unwrap();
    let catalog = files.0.canonicalize().unwrap().join("plugin-catalog.json");
    let mut log = hex_diagnostics::SessionLog::start(files.0.join("logs")).unwrap();
    let log_path = log.path();
    let mut harness = ResidentHarness::with_plugins_catalog_and_diagnostics(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
        catalog,
        log.handle(),
    );
    assert!(harness.next().error.is_none());
    let (applied, _) = harness.applied(0);
    let added = harness.run(Request::Edit(Command::AddMidiTrack {
        name: "synth".into(),
    }));
    let Some(Created::Track(track)) = added.created else {
        panic!("midi track")
    };
    let (applied, _) = harness.applied(applied);
    let insertion = request(&added.state, PluginEffectDestination::Instrument(track));
    harness
        .service
        .submit(Request::InsertPluginEffect(insertion))
        .unwrap();
    let accepted = terminal(&mut harness);
    assert!(accepted.error.is_none(), "{:?}", accepted.error);
    let _ = harness.applied(applied);
    let binding = harness.plugin_progress.lock().unwrap().binding.unwrap();
    harness
        .audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    harness.render_plugin_block();
    ready_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    harness.render_plugin_block();
    harness.render_plugin_block();
    release_tx.send(()).unwrap();
    gate.join().unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    let (line, gap) = loop {
        harness.pump();
        let text = fs::read_to_string(&log_path).unwrap_or_default();
        let slow = text
            .lines()
            .find(|line| line.contains("stage=slow_exchange"));
        let gap = text
            .lines()
            .find(|line| line.contains("stage=concealed_output"));
        if let (Some(slow), Some(gap)) = (slow, gap) {
            break (slow.to_owned(), gap.to_owned());
        }
        assert!(
            Instant::now() < until,
            "no real exchange sample in session log: {text}"
        );
        std::thread::sleep(Duration::from_millis(1));
    };
    assert!(line.contains(&format!("binding={binding:?}")), "{line}");
    for field in [
        "sequence=",
        "exchange_ns=",
        "sdk_process_ns=",
        "quantum_ns=",
        "dropped=",
        "queue_ns=unavailable",
    ] {
        assert!(line.contains(field), "{line}");
    }
    let number = |name: &str| -> u64 {
        line.split_whitespace()
            .rev()
            .find_map(|field| field.strip_prefix(name))
            .unwrap()
            .parse()
            .unwrap()
    };
    let exchange = number("exchange_ns=");
    let sdk = number("sdk_process_ns=");
    let pickup = line
        .trim_end_matches('"')
        .split_whitespace()
        .find_map(|field| field.strip_prefix("empty_poll_to_pickup_ns="))
        .unwrap();
    assert!(
        pickup == "unavailable" || pickup.parse::<u64>().is_ok(),
        "{line}"
    );
    assert_eq!(number("sequence="), 0, "{line}");
    assert!(sdk >= 15_000_000 && exchange >= sdk, "{line}");
    assert_eq!(number("quantum_ns="), 5_333_333);
    assert!(gap.contains(&format!("binding={binding:?}")), "{gap}");
    assert!(
        gap.contains("reply_sequence=0") && gap.contains("worker_phase=Processing"),
        "{gap}"
    );
    assert!(gap.contains("overflowed="), "{gap}");
    drop(harness);
    log.shutdown().unwrap();
}

#[test]
#[ignore = "requires explicit source-bound SDK receipts and optional installed catalog"]
fn native_vst3_instrument_bounces_to_new_track_and_in_place() {
    const CHILD: &str = "HEX_DESKTOP_TEST_INSTRUMENT_BOUNCE_CHILD";
    let installed = std::env::var("HEX_TEST_VST3_INSTRUMENT_BUNDLE").ok();
    if std::env::var_os(CHILD).is_none() {
        let test = std::thread::current().name().unwrap().to_owned();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .args(["--exact", &test, "--ignored", "--nocapture"])
            .env(CHILD, "1");
        if installed.is_none() {
            child
                .env("HEX_ORIGINAL_INSTANCE_VARIANT", "127")
                .env("HEX_ORIGINAL_INSTRUMENT_LIVE_PRESET", "1");
        }
        let result = child.output().unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }
    let _native = native_fixture();
    let files = Files::new();
    let (catalog, selected) = if let Some(bundle) = installed.as_ref() {
        let catalog = PathBuf::from(std::env::var_os("HEX_TEST_VST3_INSTRUMENT_CATALOG").unwrap());
        let selected =
            installed_selection(&catalog, bundle, hex_plugin_catalog::PluginRole::Instrument);
        (catalog, selected)
    } else {
        let mut selected = selection();
        let module = files.0.canonicalize().unwrap().join("original.vst3");
        super::ui::copy_original_module(&selected.module, &module);
        selected.module = module;
        hex_ui::write_plugin_effect_test_catalog(&files.0, &selected, "Instrument|Synth", 0)
            .unwrap();
        (
            files.0.canonicalize().unwrap().join("plugin-catalog.json"),
            selected,
        )
    };
    let rate = if installed.is_some() { 44_100 } else { 48_000 };
    let recovery = files.0.join("recovery");
    std::fs::create_dir(&recovery).unwrap();
    let mut harness = ResidentHarness::with_plugins_at_rate_and_catalog_recovery(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
        rate,
        2_000_000,
        catalog,
        recovery.join("bounce.hex-journal"),
    );
    assert!(harness.next().error.is_none());
    let (applied, _) = harness.applied(0);
    let added = harness.run(Request::Edit(Command::AddMidiTrack {
        name: "synth".into(),
    }));
    let Some(Created::Track(track)) = added.created else {
        panic!("midi track")
    };
    let (applied, _) = harness.applied(applied);
    let clip = harness.run(Request::Edit(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::from_quarters(1).unwrap(),
    }));
    let Some(Created::Clip(clip)) = clip.created else {
        panic!("midi clip")
    };
    let (applied, _) = harness.applied(applied);
    let note = harness.run(Request::Edit(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::from_ratio(1, 2).unwrap(),
        pitch: 60,
        channel: 0,
        velocity: 100,
    }));
    let (applied, _) = harness.applied(applied);
    let mut insertion = request(&note.state, PluginEffectDestination::Instrument(track));
    insertion.selection = selected;
    harness
        .service
        .submit(Request::InsertPluginEffect(insertion))
        .unwrap();
    let accepted = terminal(&mut harness);
    assert!(accepted.error.is_none(), "{:?}", accepted.error);
    let (applied, _) = harness.applied(applied);
    let binding = harness.plugin_progress.lock().unwrap().binding.unwrap();
    let state = accepted
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .state()
        .clone();
    if installed.is_none() {
        let original = hex_plugin_state::decode(&state).unwrap().component.unwrap();
        assert_eq!(f32::from_le_bytes(original[8..12].try_into().unwrap()), 1.0);
        harness
            .audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
        // Send a live note so the fixture selects a preset internally. The
        // canonical project still contains the insertion-time default.
        let live_peak = (0..6)
            .flat_map(|_| render_completed_block(&harness))
            .flatten()
            .map(f32::abs)
            .fold(0.0, f32::max);
        assert!(live_peak > 0.001, "the instrument must sound before bounce");
        harness
            .audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Pause)
            .unwrap();
        render_completed_block(&harness);
    }
    let control = hex_export::ExportControl::default();
    harness
        .service
        .submit(Request::BounceArrangement {
            project_generation: 0,
            revision: accepted.state.revision,
            target: hex_project::BounceTarget::Clips(vec![clip]),
            in_place: false,
            adoption: hex_project::BounceAdoption::NewTracks,
            control: control.clone(),
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(14);
    let bounced = loop {
        harness.pump();
        harness.render();
        if let Ok(event) = harness.service.try_event() {
            break event;
        }
        assert!(
            Instant::now() < deadline,
            "bounce timed out at {:?}",
            control.progress()
        );
        std::thread::sleep(Duration::from_millis(1));
    };
    assert!(bounced.error.is_none(), "{:?}", bounced.error);
    let project = &bounced.state.project;
    assert_eq!(
        project.track(track).unwrap().instrument().unwrap().state(),
        &state
    );
    let audio_track = project
        .tracks()
        .iter()
        .find(|candidate| candidate.id() != track)
        .unwrap();
    let asset = project.asset(audio_track.clips()[0].asset()).unwrap();
    let wav = std::fs::read(asset.path()).unwrap();
    let audible = wav[hex_export::FLOAT32_WAV_HEADER_BYTES as usize..]
        .chunks_exact(4)
        .any(|sample| f32::from_le_bytes(sample.try_into().unwrap()).abs() > 0.001);
    assert!(
        audible,
        "offline instrument bounce must contain audible PCM"
    );
    if installed.is_none() {
        let first = hex_export::FLOAT32_WAV_HEADER_BYTES as usize;
        let sample = f32::from_le_bytes(wav[first..first + 4].try_into().unwrap());
        let expected = 100.0 / 127.0 * 0.5;
        assert!(
            (sample - expected).abs() < 1e-6,
            "the compensated bounce must start with the live preset: {sample} vs {expected}"
        );
    }
    let (applied, _) = harness.applied(applied);
    assert_eq!(
        harness.plugin_progress.lock().unwrap().binding,
        Some(binding)
    );

    let before = Arc::clone(&bounced.state.project);
    let baked = super::super::resident_sections::run_callback(
        &mut harness,
        Request::BounceArrangement {
            project_generation: 0,
            revision: bounced.state.revision,
            target: hex_project::BounceTarget::Track(track),
            in_place: true,
            adoption: hex_project::BounceAdoption::NewTracks,
            control: hex_export::ExportControl::default(),
        },
    );
    assert!(baked.error.is_none(), "{:?}", baked.error);
    let (applied, _) = harness.applied(applied);
    assert_eq!(baked.state.revision, bounced.state.revision + 1);
    let project = &baked.state.project;
    let baked_track = project.track(track).unwrap();
    assert!(baked_track.instrument().is_none());
    assert!(baked_track.midi_clips().is_empty());
    assert_eq!(baked_track.clips().len(), 1);
    let asset = project.asset(baked_track.clips()[0].asset()).unwrap();
    let actual = std::fs::read(asset.path()).unwrap();
    let header = hex_export::FLOAT32_WAV_HEADER_BYTES as usize;
    if installed.is_none() {
        assert_eq!(
            &actual[header..header + 256 * 8],
            &wav[header..header + 256 * 8],
            "both bounce commands preserve the same compensated onset"
        );
    } else {
        assert!(
            actual[header..]
                .chunks_exact(4)
                .any(|sample| { f32::from_le_bytes(sample.try_into().unwrap()).abs() > 0.001 })
        );
    }
    let undo = super::super::resident_sections::run_callback(&mut harness, Request::Undo);
    assert!(undo.error.is_none(), "{:?}", undo.error);
    let (applied, _) = harness.applied(applied);
    super::offline_bounce::assert_undone(&undo.state.project, &before);
    let redo = super::super::resident_sections::run_callback(&mut harness, Request::Redo);
    assert!(redo.error.is_none(), "{:?}", redo.error);
    harness.applied(applied);
    assert_eq!(redo.state.project, baked.state.project);
}

pub(super) fn replay_reopened_instrument(
    h: &mut ResidentHarness,
    path: &std::path::Path,
    device: hex_project::DeviceId,
    blocks: usize,
) -> Vec<[f32; 2]> {
    let old_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    h.service.submit(Request::Open(path.to_owned())).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let opened = loop {
        h.pump();
        h.render_worker_step();
        if let Ok(event) = h.service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            if event.project_replaced {
                break event;
            }
        }
        assert!(Instant::now() < deadline, "replay open did not complete");
    };
    let first_applied =
        super::super::resident_lifecycle::finish_replacement(h, &opened, old_output);
    let type_id = opened
        .state
        .project
        .device(device)
        .unwrap()
        .state()
        .type_id();
    let foreign_ready = |h: &ResidentHarness| {
        h.submitted_observation()
            .unwrap()
            .foreign_processor(device.value(), type_id)
            == Some(true)
    };
    if !foreign_ready(h) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !foreign_ready(h) {
            h.pump();
            h.render_worker_step();
            if let Ok(event) = h.service.try_event() {
                assert!(event.error.is_none(), "{:?}", event.error);
            }
            assert!(
                Instant::now() < deadline,
                "restored SDK source unavailable: {:?}; {:?}",
                h.unavailable,
                h.plugin_progress.lock().unwrap()
            );
        }
        h.applied(first_applied);
    }
    assert_eq!(
        opened.state.project.as_ref(),
        &hex_storage::load(path).unwrap()
    );
    assert_eq!(
        h.submitted_observation()
            .unwrap()
            .foreign_processor(device.value(), type_id),
        Some(true)
    );
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let mut pcm = Vec::with_capacity(blocks * 256);
    for _ in 0..blocks {
        pcm.extend(render_completed_block(h));
    }
    pcm
}

pub(super) fn render_completed_block(harness: &ResidentHarness) -> [hex_engine::Frame; 256] {
    let frames = harness.render_plugin_block();
    let deadline = Instant::now() + Duration::from_millis(800);
    // Session's optional diagnostic is sampled on a different control cadence.
    // An increase can describe an older request, leaving this request in flight.
    // Pace the headless host by the exact installed endpoints' published prefix.
    while harness
        .callback
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .native_workers_idle_on_control()
        != Some(true)
    {
        let status = harness.audio.as_ref().unwrap().status().unwrap();
        assert!(
            Instant::now() < deadline,
            "worker did not publish the committed prefix; playing={} source={:?} fault={:?}",
            status.playing.load(Acquire),
            status.native_effect_source(),
            status.render_fault_snapshot(),
        );
        std::thread::yield_now();
    }
    // Give the other asynchronous owner a real turn after the published DSP
    // prefix. Commit cleanup can enqueue Release only after observing that
    // prefix; accelerated headless quanta must not starve this control lane.
    let polled = harness.control_polls.load(Acquire);
    while harness.control_polls.load(Acquire) == polled {
        assert!(Instant::now() < deadline, "native control did not poll");
        std::thread::yield_now();
    }
    frames
}

fn verify_saved_instrument_automation(
    harness: &mut ResidentHarness,
    project: &hex_project::Project,
    device: hex_project::DeviceId,
    parameter_name: &str,
) {
    use hex_project::{AutomationInterpolation, AutomationPoint, AutomationValue, Editor};
    use hex_session::service::PluginParameterQuery;

    let context = harness.submitted.unwrap().1.context;
    let output = harness
        .audio
        .as_ref()
        .unwrap()
        .control
        .output_identity()
        .unwrap();
    harness.run(Request::QueryPluginParameters(PluginParameterQuery {
        context,
        device,
        output_identity: output,
        request_id: 1,
        page_first: None,
    }));
    let rows = harness.next().plugin_parameters.unwrap().rows.unwrap();
    let parameter = rows
        .iter()
        .find(|row| row.editable && row.name.eq_ignore_ascii_case(parameter_name))
        .unwrap_or_else(|| {
            panic!(
                "editable parameter {parameter_name:?} missing. Volume and output candidates: {:?}",
                rows.iter()
                    .filter(|row| {
                        let name = row.name.to_ascii_lowercase();
                        name.contains("vol") || name.contains("out")
                    })
                    .map(|row| (&row.name, row.id, row.editable))
                    .collect::<Vec<_>>()
            )
        });
    let target = hex_project::DeviceAutomationTarget::Vst3 {
        parameter: parameter.id,
    };
    let points = [
        (BeatTime::ZERO, 0.0),
        (BeatTime::from_ratio(1, 2).unwrap(), 0.8),
        (BeatTime::from_ratio(5, 4).unwrap(), 0.0),
    ]
    .map(|(beat, normalized)| AutomationPoint {
        beat,
        value: AutomationValue::new(normalized).unwrap(),
        outgoing: AutomationInterpolation::Step,
    })
    .to_vec();
    let mut editor = Editor::new(project.clone(), 8).unwrap();
    editor
        .execute(Command::ReplaceDeviceAutomationTarget {
            device,
            target,
            points: points.clone(),
        })
        .unwrap();
    let files = Files::new();
    let path = files.0.join("instrument-automation.hex");
    hex_storage::save_new(&path, editor.project()).unwrap();
    let opened = harness.run(Request::Open(path.clone()));
    assert!(
        opened.error.is_none() && opened.project_replaced,
        "{:?}",
        opened.error
    );
    assert_eq!(
        opened
            .state
            .project
            .device(device)
            .unwrap()
            .automation_target(target)
            .unwrap()
            .points,
        points
    );
    let revision = super::super::resident_lifecycle::finish_replacement(harness, &opened, output);
    let restored = harness.next();
    assert!(restored.error.is_none(), "{:?}", restored.error);
    let transport = |harness: &mut ResidentHarness, command| {
        harness
            .audio
            .as_mut()
            .unwrap()
            .control
            .command(command)
            .unwrap();
    };
    let render_peak = |harness: &ResidentHarness| {
        render_completed_block(harness)
            .into_iter()
            .flatten()
            .map(f32::abs)
            .fold(0.0, f32::max)
    };
    transport(harness, PlaybackCommand::Play);
    let mut peaks = Vec::new();
    for block in 0..180 {
        if block == 90 {
            transport(harness, PlaybackCommand::Pause);
            render_peak(harness);
            assert!(
                !harness
                    .audio
                    .as_ref()
                    .unwrap()
                    .status()
                    .unwrap()
                    .playing
                    .load(Acquire)
            );
            transport(harness, PlaybackCommand::Play);
        }
        peaks.push(render_peak(harness));
    }
    let peak = |blocks: std::ops::Range<usize>| peaks[blocks].iter().copied().fold(0.0, f32::max);
    // The real instrument smooths output changes. Measure away from each
    // transition while an Arrangement note is still held.
    let (before, during, resumed, after) =
        (peak(25..40), peak(84..90), peak(100..108), peak(170..180));
    println!(
        "instrument automation parameter={} id={} before={before} during={during} resumed={resumed} after={after}",
        parameter.name, parameter.id
    );
    assert!(
        during > 0.01 && resumed > 0.01 && before < during * 0.05 && after < during * 0.05,
        "saved output automation must mute, open, and mute again across playback: {peaks:?}"
    );
    harness
        .service
        .submit(Request::Edit(Command::SetLoopRegion {
            start: 0,
            end: 98_304,
            enabled: true,
        }))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        harness.pump();
        harness.render_worker_step();
        if let Ok(event) = harness.service.try_event()
            && event.request_complete
        {
            assert!(event.error.is_none(), "{:?}", event.error);
            break;
        }
        assert!(Instant::now() < deadline, "live loop edit did not complete");
        std::thread::sleep(Duration::from_millis(1));
    }
    harness.applied(revision);
    let status = harness.audio.as_ref().unwrap().status().unwrap();
    let mut position = status.position.load(Acquire);
    let mut wrapped = false;
    assert!(
        (0..512).any(|_| {
            let peak = render_peak(harness);
            let current = status.position.load(Acquire);
            wrapped |= current < position;
            position = current;
            if wrapped && peak > 0.01 {
                println!("instrument automation live loop peak={peak}");
                true
            } else {
                false
            }
        }),
        "the edited loop must return to the audible step"
    );
    let saved = harness.run(Request::SaveCurrent);
    assert!(
        saved.error.is_none() && saved.save_complete,
        "{:?}",
        saved.error
    );
    let reopened = hex_storage::load(&path).unwrap();
    assert_eq!(
        reopened
            .device(device)
            .unwrap()
            .automation_target(target)
            .unwrap()
            .points,
        points
    );
    assert_instrument_takeover_begin(
        harness,
        device,
        parameter.id,
        &saved.state.project,
        saved.state.revision,
    );
}

#[test]
#[ignore = "requires explicit source-bound SDK receipts and optional installed catalog"]
fn second_vst3_instrument_sounds_during_the_same_playback() {
    const CHILD: &str = "HEX_DESKTOP_TEST_SECOND_LIVE_INSTRUMENT";
    let installed = std::env::var("HEX_TEST_VST3_INSTRUMENT_BUNDLE").ok();
    if std::env::var_os(CHILD).is_none() {
        let test = std::thread::current().name().unwrap().to_owned();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .args(["--exact", &test, "--ignored", "--nocapture"])
            .env(CHILD, "1");
        if installed.is_none() {
            child.env("HEX_ORIGINAL_INSTANCE_VARIANT", "127");
        }
        let result = child.output().unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }
    let _native = native_fixture();
    let rate = if installed.is_some() { 44_100 } else { 48_000 };
    let mut harness = ResidentHarness::with_plugins_at_rate(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
        rate,
    );
    assert!(harness.next().error.is_none());
    let (mut applied, _) = harness.applied(0);
    let mut expected_output = None;
    let mut instruments = Vec::new();
    harness.run(Request::Edit(Command::SetLoopRegion {
        start: 0,
        end: u64::from(rate) * 4,
        enabled: true,
    }));
    (applied, _) = harness.applied(applied);
    for (name, balance) in [("left", -1.0), ("right", 1.0)] {
        let added = harness.run(Request::Edit(Command::AddMidiTrack { name: name.into() }));
        let Some(Created::Track(track)) = added.created else {
            panic!("midi track")
        };
        (applied, _) = harness.applied(applied);
        harness.run(Request::Edit(Command::Balance { track, balance }));
        (applied, _) = harness.applied(applied);
        let length = BeatTime::from_quarters(8).unwrap();
        let placed = harness.run(Request::Edit(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length,
        }));
        let Some(Created::Clip(clip)) = placed.created else {
            panic!("midi clip")
        };
        (applied, _) = harness.applied(applied);
        let note = harness.run(Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: length,
            pitch: 60,
            channel: 0,
            velocity: 100,
        }));
        (applied, _) = harness.applied(applied);
        let mut insertion = request(&note.state, PluginEffectDestination::Instrument(track));
        if let Some(bundle) = installed.as_ref() {
            let catalog =
                PathBuf::from(std::env::var_os("HEX_TEST_VST3_INSTRUMENT_CATALOG").unwrap());
            insertion.selection =
                installed_selection(&catalog, bundle, hex_plugin_catalog::PluginRole::Instrument);
        }
        harness
            .service
            .submit(Request::InsertPluginEffect(insertion))
            .unwrap();
        let end = Instant::now() + Duration::from_secs(5);
        let accepted = loop {
            harness.pump();
            harness.render_worker_step();
            if let Ok(event) = harness.service.try_event()
                && !event.plugin_effect_complete.is_empty()
            {
                break event;
            }
            assert!(Instant::now() < end, "matching live insertion terminal");
            std::thread::sleep(Duration::from_millis(1));
        };
        assert!(
            accepted.error.is_none() && accepted.plugin_effect_complete[0].accepted,
            "{:?}",
            accepted.error
        );
        instruments.push((
            track,
            accepted
                .state
                .project
                .track(track)
                .unwrap()
                .instrument()
                .unwrap()
                .id(),
        ));
        let (next, metadata) = harness.applied(applied);
        assert_eq!(metadata.context.editor_revision, accepted.state.revision);
        applied = next;
        let audio = harness.audio.as_mut().unwrap();
        let output = audio.control.output_identity().unwrap();
        if let Some(original) = expected_output {
            assert_eq!(output, original);
            assert!(audio.status().unwrap().playing.load(Acquire));
        } else {
            expected_output = Some(output);
            audio.control.command(PlaybackCommand::Play).unwrap();
        }
        let mut peak = [0.0_f32; 2];
        for _ in 0..12 {
            for frame in render_completed_block(&harness) {
                for channel in 0..2 {
                    peak[channel] = peak[channel].max(frame[channel].abs());
                }
            }
        }
        assert!(
            peak[0] > 0.001,
            "original instrument must keep sounding: {peak:?}"
        );
        if name == "left" {
            assert!(
                peak[1] < 0.001,
                "first instrument is isolated left: {peak:?}"
            );
        } else {
            assert!(
                peak[1] > 0.001,
                "second instrument must sound on the same playback: {peak:?}"
            );
        }
    }

    // Follow the editor's Stop protocol even though the running revision was
    // already current. The next edit needs fresh endpoints for the other track.
    harness.audio.as_ref().unwrap().control.stop();
    assert_eq!(harness.render_plugin_block(), [[0.0; 2]; 256]);
    let mut restored = harness.run(Request::RestoreStoppedPlayback);
    if !restored.stopped_playback_restore_complete {
        restored = restored_terminal(&mut harness);
    }
    assert!(restored.error.is_none(), "{:?}", restored.error);
    let (renewed, metadata) = harness.applied(applied);
    assert!(metadata.stopped_restore);
    let [(first_track, first), (second_track, second)] = instruments.as_slice() else {
        panic!("two installed instruments");
    };
    let added = harness.run(Request::AddNativeDevice {
        track: *first_track,
        class: hex_engine::NativeDeviceClass::Effect,
        type_id: Box::new(hex_engine::DeviceTypeId::from_static("hex.rattle")),
        before: None,
    });
    assert!(added.error.is_none(), "{:?}", added.error);
    let (effect_applied, metadata) = harness.applied(renewed);
    assert_eq!(metadata.context.editor_revision, added.state.revision);

    let replacement = hex_engine::DeviceTypeId::from_static("hex.instrument.sine");
    let edited = harness.run(Request::ReplaceNativeDevice {
        device: *first,
        class: hex_engine::NativeDeviceClass::Instrument,
        type_id: Box::new(replacement),
    });
    assert!(edited.error.is_none(), "{:?}", edited.error);
    let first = edited
        .state
        .project
        .track(*first_track)
        .unwrap()
        .instrument()
        .unwrap();
    assert_eq!(first.state().type_id(), replacement);
    let retained = edited
        .state
        .project
        .track(*second_track)
        .unwrap()
        .instrument()
        .unwrap();
    assert_eq!(retained.id(), *second);
    assert_eq!(retained.state().type_id().as_str(), Ok("hex.vst3"));
    let (_, metadata) = harness.applied(effect_applied);
    assert_eq!(metadata.context.editor_revision, edited.state.revision);
    let audio = harness.audio.as_ref().unwrap();
    assert_eq!(audio.control.output_identity(), expected_output);
    assert!(!audio.status().unwrap().playing.load(Acquire));
}

#[test]
#[ignore = "requires explicit source-bound SDK receipts and optional installed catalog"]
fn instrument_insertion_plays_authored_midi_in_the_desktop_audio_graph() {
    instrument_entrypoint(InstrumentWorkflow::Playback);
}

#[test]
#[ignore = "requires explicit source-bound SDK receipts"]
fn session_play_from_releases_old_sdk_instrument_gate() {
    instrument_entrypoint(InstrumentWorkflow::SessionRestart);
}

#[test]
#[ignore = "requires explicit source-bound SDK receipts"]
fn session_loop_repeats_sdk_instrument_gates() {
    instrument_entrypoint(InstrumentWorkflow::SessionLoop);
}

#[test]
#[ignore = "requires explicit source-bound SDK receipts"]
fn session_seek_releases_old_sdk_instrument_gate() {
    instrument_entrypoint(InstrumentWorkflow::SessionSeek);
}

#[test]
#[ignore = "requires explicit source-bound SDK receipts"]
fn session_priority_stop_releases_gate_on_original_sdk_endpoint() {
    instrument_entrypoint(InstrumentWorkflow::SessionStopTrace);
}

#[test]
#[ignore = "requires explicit source-bound SDK receipts"]
fn session_pause_releases_sdk_gate_without_rechasing_on_resume() {
    instrument_entrypoint(InstrumentWorkflow::SessionPause);
}

/// An opt-in wall-clock driver for the actual native worker, not a physical
/// audio deadline or a reproduction of any particular user project.
#[test]
#[ignore = "requires an installed VST3 instrument, its catalog and a source-bound helper"]
fn paced_live_instrument_automation_preserves_the_worker() {
    instrument_entrypoint(InstrumentWorkflow::PacedAuthoring);
}

/// Includes the live parameter reads present in the reported editing workflow.
#[test]
#[ignore = "requires an installed VST3 instrument, its catalog and a source-bound helper"]
fn paced_automation_and_parameter_polling_preserve_the_worker() {
    instrument_entrypoint(InstrumentWorkflow::PacedMonitoring);
}

/// Five independently hosted instruments share one paced graph callback.
#[test]
#[ignore = "requires an installed VST3 instrument, its catalog and a source-bound helper"]
fn paced_five_instruments_with_automation_and_polling() {
    instrument_entrypoint(InstrumentWorkflow::PacedFive);
}

#[test]
#[ignore = "requires an installed VST3 instrument, its catalog and a source-bound helper"]
fn paced_five_instruments_with_automation_undo_and_redo() {
    instrument_entrypoint(InstrumentWorkflow::PacedFiveHistory);
}

#[test]
#[ignore = "requires explicit source-bound SDK receipts and optional installed catalog"]
fn shared_instrument_lane_authoring_undo_read_controls_and_pcm() {
    instrument_entrypoint(InstrumentWorkflow::Authoring);
}

#[test]
#[ignore = "requires explicit source-bound SDK receipts"]
fn instrument_takeover_preserves_other_base_and_held_midi() {
    const CASE: &str = "HEX_DESKTOP_INSTRUMENT_TAKEOVER_CASE";
    if std::env::var_os(CASE).is_none() {
        let name = std::thread::current().name().unwrap().to_owned();
        for case in [
            "reading",
            "unread",
            "empty",
            "renew",
            "author",
            "author_empty",
            "idle",
            "idle_empty",
            "refused",
        ] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", &name, "--ignored", "--nocapture"])
                .env(CASE, case)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{case}: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            print!("{}", String::from_utf8_lossy(&output.stdout));
        }
        return;
    }
    instrument_entrypoint(InstrumentWorkflow::Takeover);
}

#[test]
#[ignore = "explicit source-bound signed SDK receipts required"]
fn instrument_gesture_points_are_confirmed_at_final_output() {
    const CASE: &str = "HEX_DESKTOP_INSTRUMENT_TAKEOVER_CASE";
    if std::env::var_os(CASE).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                std::thread::current().name().unwrap(),
                "--ignored",
                "--nocapture",
            ])
            .env(CASE, "delivery")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        return;
    }
    instrument_entrypoint(InstrumentWorkflow::Takeover);
    if std::env::var_os("HEX_DESKTOP_TEST_INSTRUMENT_CHILD").is_some() {
        assert!(
            crate::native_audio::tests::resident_preview::CONFIRMED_TEST_POINTS.load(Acquire) > 0
        );
    }
}

#[test]
#[ignore = "explicit source-bound signed SDK receipts required"]
fn confirmed_point_survives_output_close_while_sdk_ack_is_pending() {
    const CASE: &str = "HEX_DESKTOP_INSTRUMENT_TAKEOVER_CASE";
    if std::env::var_os(CASE).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                std::thread::current().name().unwrap(),
                "--ignored",
                "--nocapture",
            ])
            .env(CASE, "delivery_retire")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        return;
    }
    instrument_entrypoint(InstrumentWorkflow::Takeover);
    if std::env::var_os("HEX_DESKTOP_TEST_INSTRUMENT_CHILD").is_some() {
        assert!(
            crate::native_audio::tests::resident_preview::CONFIRMED_TEST_POINTS.load(Acquire) > 0
        );
    }
}

#[test]
#[ignore = "explicit source-bound signed SDK receipts required"]
fn recorded_touch_admission_and_exclusive_restoration_use_actual_output() {
    const CASE: &str = "HEX_DESKTOP_INSTRUMENT_TAKEOVER_CASE";
    if std::env::var_os(CASE).is_none() {
        for case in [
            "touch_close_begin",
            "touch_close_point",
            "touch_close_pending",
            "touch_close_queued",
            "touch_close_restored",
            "touch",
            "touch_empty",
            "touch_commit",
            "touch_control_lag",
        ] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    std::thread::current().name().unwrap(),
                    "--ignored",
                    "--nocapture",
                ])
                .env(CASE, case)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{case}: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        }
        return;
    }
    instrument_entrypoint(InstrumentWorkflow::Takeover);
    if std::env::var_os("HEX_DESKTOP_TEST_INSTRUMENT_CHILD").is_some() {
        use crate::native_audio::tests::resident_preview::{
            CONFIRMED_TEST_ENDS, CONFIRMED_TEST_INTERRUPTS, CONFIRMED_TEST_NOOPS,
            CONFIRMED_TEST_POINTS, CONFIRMED_TEST_READY,
        };
        assert!(CONFIRMED_TEST_READY.load(Acquire) > 0);
        if std::env::var(CASE).unwrap() == "touch_stress" {
            assert_eq!(CONFIRMED_TEST_READY.load(Acquire), 100);
            assert_eq!(CONFIRMED_TEST_POINTS.load(Acquire), 100);
            assert_eq!(CONFIRMED_TEST_ENDS.load(Acquire), 100);
            assert_eq!(CONFIRMED_TEST_NOOPS.load(Acquire), 0);
        } else if std::env::var(CASE).unwrap() == "touch_close_restored" {
            assert_eq!(CONFIRMED_TEST_READY.load(Acquire), 1);
            assert_eq!(CONFIRMED_TEST_POINTS.load(Acquire), 1);
            assert_eq!(CONFIRMED_TEST_ENDS.load(Acquire), 1);
            assert_eq!(CONFIRMED_TEST_INTERRUPTS.load(Acquire), 0);
        } else if std::env::var(CASE).unwrap().starts_with("touch_close_") {
            assert_eq!(CONFIRMED_TEST_READY.load(Acquire), 1);
            assert_eq!(CONFIRMED_TEST_INTERRUPTS.load(Acquire), 1);
            assert_eq!(CONFIRMED_TEST_ENDS.load(Acquire), 0);
        } else if std::env::var(CASE).unwrap() == "touch_empty" {
            assert_eq!(CONFIRMED_TEST_POINTS.load(Acquire), 0);
            assert!(CONFIRMED_TEST_NOOPS.load(Acquire) > 0);
        } else {
            assert!(CONFIRMED_TEST_POINTS.load(Acquire) > 0);
            assert_eq!(
                CONFIRMED_TEST_POINTS.load(Acquire),
                CONFIRMED_TEST_ENDS.load(Acquire)
            );
        }
    }
}

#[test]
#[ignore = "explicit source-bound signed SDK receipts required"]
fn recorded_session_gesture_commits_one_take_and_reopens() {
    std::env::var("HEX_DESKTOP_INSTRUMENT_TAKEOVER_CASE")
        .map(|case| {
            assert!(matches!(
                case.as_str(),
                "session_record"
                    | "session_record_changed"
                    | "session_first_lane"
                    | "session_first_lane_with_other"
                    | "session_first_lane_stop"
                    | "session_barriers"
                    | "session_save"
                    | "session_barrier_undo"
                    | "session_barrier_export"
                    | "session_barrier_refused"
                    | "session_barrier_stop"
                    | "session_stop"
                    | "session_refused"
                    | "session_recover"
                    | "session_writer_failure"
                    | "session_writer_adopt"
                    | "session_writer_keep_apply"
                    | "session_writer_refusal"
                    | "session_writer_source_lost"
                    | "session_cancel"
                    | "session_empty"
            ))
        })
        .expect("run explicitly with HEX_DESKTOP_INSTRUMENT_TAKEOVER_CASE=session_record");
    instrument_entrypoint(InstrumentWorkflow::Takeover);
}

#[derive(Clone, Copy)]
enum InstrumentWorkflow {
    Playback,
    SessionRestart,
    SessionLoop,
    SessionPause,
    SessionSeek,
    SessionStopTrace,
    Authoring,
    AuthoringAcrossPlay,
    PacedAuthoring,
    PacedMonitoring,
    PacedFive,
    PacedFiveHistory,
    Takeover,
}

fn instrument_entrypoint(workflow: InstrumentWorkflow) {
    let shared_authoring = !matches!(
        workflow,
        InstrumentWorkflow::Playback
            | InstrumentWorkflow::SessionRestart
            | InstrumentWorkflow::SessionLoop
            | InstrumentWorkflow::SessionPause
            | InstrumentWorkflow::SessionSeek
            | InstrumentWorkflow::SessionStopTrace
    );
    let takeover = matches!(workflow, InstrumentWorkflow::Takeover);
    let paced = matches!(
        workflow,
        InstrumentWorkflow::PacedAuthoring
            | InstrumentWorkflow::PacedMonitoring
            | InstrumentWorkflow::PacedFive
            | InstrumentWorkflow::PacedFiveHistory
    );
    let monitor = matches!(
        workflow,
        InstrumentWorkflow::PacedMonitoring
            | InstrumentWorkflow::PacedFive
            | InstrumentWorkflow::PacedFiveHistory
    );
    let name = std::thread::current().name().unwrap().to_owned();
    const CHILD: &str = "HEX_DESKTOP_TEST_INSTRUMENT_CHILD";
    let installed = std::env::var("HEX_TEST_VST3_INSTRUMENT_BUNDLE").ok();
    if std::env::var_os(CHILD).is_none() {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .args(["--exact", &name, "--ignored", "--nocapture"])
            .env(CHILD, "1");
        let trace_directory =
            matches!(workflow, InstrumentWorkflow::SessionStopTrace).then(Files::new);
        if let Some(directory) = &trace_directory {
            child.env("HEX_ORIGINAL_NOTE_TRACE_DIR", &directory.0);
        }
        if installed.is_none() {
            child.env(
                "HEX_ORIGINAL_INSTANCE_VARIANT",
                if takeover {
                    "146"
                } else if shared_authoring {
                    "145"
                } else {
                    "127"
                },
            );
        }
        let result = child.output().unwrap();
        let output = String::from_utf8_lossy(&result.stdout);
        assert!(
            result.status.success() && output.contains("1 passed"),
            "{output}\n{}",
            String::from_utf8_lossy(&result.stderr)
        );
        print!("{output}");
        return;
    }
    let _native = native_fixture();
    let controlled_fixture = installed.is_none();
    let channel = if controlled_fixture && shared_authoring {
        2
    } else {
        0
    };
    // Installed drum instruments may map their default sounds below middle C.
    let installed_pitch = installed.as_ref().and_then(|_| {
        std::env::var("HEX_TEST_VST3_INSTRUMENT_PITCH")
            .ok()
            .map(|pitch| pitch.parse::<u8>().expect("installed instrument pitch"))
    });
    let pitch = installed_pitch.unwrap_or(60);
    let rate = if installed.is_some() { 44_100 } else { 48_000 };
    let presentation = Some(EffectPresentation {
        input_frames: 0,
        output_frames: 512,
    });
    let automation_parameter = std::env::var("HEX_TEST_VST3_INSTRUMENT_AUTOMATION_PARAMETER").ok();
    let recording_catalog = std::env::var("HEX_DESKTOP_INSTRUMENT_TAKEOVER_CASE")
        .ok()
        .filter(|case| case.starts_with("session_"))
        .map(|_| {
            let files = Files::new();
            let mut selected = selection();
            let module = files.0.canonicalize().unwrap().join("original.vst3");
            super::ui::copy_original_module(&selected.module, &module);
            selected.module = module;
            hex_ui::write_plugin_effect_test_catalog(&files.0, &selected, "Instrument|Synth", 0)
                .unwrap();
            files
        });
    let mut harness = if let Some(files) = recording_catalog.as_ref() {
        ResidentHarness::with_plugins_at_rate_and_catalog_recovery(
            host(),
            presentation,
            rate,
            8192,
            files.0.canonicalize().unwrap().join("plugin-catalog.json"),
            files
                .0
                .canonicalize()
                .unwrap()
                .join("recording.hex-journal"),
        )
    } else if automation_parameter.is_some() || (shared_authoring && installed.is_some()) {
        let catalog = PathBuf::from(std::env::var_os("HEX_TEST_VST3_INSTRUMENT_CATALOG").unwrap());
        ResidentHarness::with_plugins_at_rate_and_catalog(host(), presentation, rate, catalog, 8192)
    } else {
        ResidentHarness::with_plugins_at_rate(host(), presentation, rate)
    };
    assert!(harness.next().error.is_none());
    let (mut applied, _) = harness.applied(0);
    let add = harness.run(Request::Edit(Command::AddMidiTrack {
        name: "synth".into(),
    }));
    let Some(Created::Track(track)) = add.created else {
        panic!("midi track");
    };
    (applied, _) = harness.applied(applied);
    let (clip_length, note_duration) = if paced {
        // Keep the instrument sounding through the optional long stress run.
        (
            BeatTime::from_quarters(1024).unwrap(),
            BeatTime::from_quarters(1024).unwrap(),
        )
    } else if takeover {
        // The headless control driver can render far faster than wall time
        // while waiting for SDK work. Keep its held note alive throughout
        // these operations without adding processing to the test.
        (
            BeatTime::from_quarters(256).unwrap(),
            BeatTime::from_quarters(256).unwrap(),
        )
    } else if automation_parameter.is_some() {
        (
            BeatTime::from_quarters(4).unwrap(),
            BeatTime::from_ratio(17, 16).unwrap(),
        )
    } else {
        (
            BeatTime::from_quarters(1).unwrap(),
            BeatTime::from_ratio(1, 2).unwrap(),
        )
    };
    let clip = harness.run(Request::Edit(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: clip_length,
    }));
    let Some(Created::Clip(clip)) = clip.created else {
        panic!("midi clip");
    };
    (applied, _) = harness.applied(applied);
    let mut note = harness.run(Request::Edit(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: note_duration,
        pitch,
        channel,
        velocity: 112,
    }));
    (applied, _) = harness.applied(applied);
    if automation_parameter.is_some() {
        // Pause releases held gates. A new onset proves the saved parameter
        // is still effective after resume.
        note = harness.run(Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::from_ratio(9, 8).unwrap(),
            duration: BeatTime::from_quarters(2).unwrap(),
            pitch,
            channel,
            velocity: 112,
        }));
        (applied, _) = harness.applied(applied);
    }
    let console = if shared_authoring {
        None
    } else {
        let scene = harness.run(Request::Edit(Command::AddScene {
            name: "console".into(),
        }));
        let Some(Created::Scene(scene)) = scene.created else {
            panic!("scene")
        };
        (applied, _) = harness.applied(applied);
        let console = harness.run(Request::Edit(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "console notes".into(),
            source_length: if matches!(workflow, InstrumentWorkflow::SessionLoop) {
                BeatTime::from_ratio(1, 32).unwrap()
            } else {
                BeatTime::from_quarters(1).unwrap()
            },
        }));
        let Some(Created::SessionClip(console)) = console.created else {
            panic!("console clip")
        };
        (applied, _) = harness.applied(applied);
        if matches!(workflow, InstrumentWorkflow::SessionLoop) {
            harness.run(Request::Edit(Command::SetSessionMidiClipSource {
                clip: console,
                source_length: BeatTime::from_ratio(1, 32).unwrap(),
                source_start: BeatTime::ZERO,
                loop_start: BeatTime::ZERO,
                loop_end: BeatTime::from_ratio(1, 32).unwrap(),
                looping: true,
            }));
            (applied, _) = harness.applied(applied);
        }
        note = harness.run(Request::Edit(Command::AddMidiNote {
            clip: console,
            start: BeatTime::ZERO,
            duration: if matches!(workflow, InstrumentWorkflow::SessionLoop) {
                BeatTime::from_ratio(1, 64).unwrap()
            } else {
                BeatTime::from_quarters(1).unwrap()
            },
            pitch: installed_pitch.unwrap_or(if controlled_fixture { 60 } else { 64 }),
            channel,
            velocity: 88,
        }));
        (applied, _) = harness.applied(applied);
        Some(console)
    };
    let insertion = if paced {
        let bundle = installed
            .as_ref()
            .expect("paced test requires an installed instrument");
        PluginEffectInsertion {
            context: note
                .state
                .canonical_audio_source
                .as_ref()
                .unwrap()
                .context(),
            selection: installed_selection(
                &PathBuf::from(std::env::var_os("HEX_TEST_VST3_INSTRUMENT_CATALOG").unwrap()),
                bundle,
                hex_plugin_catalog::PluginRole::Instrument,
            ),
            destination: PluginEffectDestination::Instrument(track),
            before: None,
            track_device_name: None,
            control: Arc::new(PluginEffectControl::default()),
        }
    } else {
        let mut insertion = request(&note.state, PluginEffectDestination::Instrument(track));
        if let Some(bundle) = installed.as_ref() {
            insertion.selection = installed_selection(
                &PathBuf::from(std::env::var_os("HEX_TEST_VST3_INSTRUMENT_CATALOG").unwrap()),
                bundle,
                hex_plugin_catalog::PluginRole::Instrument,
            );
        }
        insertion
    };
    harness
        .service
        .submit(Request::InsertPluginEffect(insertion))
        .unwrap();
    let accepted = terminal(&mut harness);
    assert!(accepted.error.is_none(), "{:?}", accepted.error);
    assert!(accepted.plugin_effect_complete[0].accepted);
    let project_track = accepted.state.project.track(track).unwrap();
    assert!(project_track.devices().is_empty());
    assert!(project_track.instrument().is_some());
    assert_eq!(accepted.state.project.sample_rate(), rate);
    (applied, _) = harness.applied(applied);
    let instrument = project_track.instrument().unwrap().id();
    if matches!(
        workflow,
        InstrumentWorkflow::PacedFive | InstrumentWorkflow::PacedFiveHistory
    ) {
        for index in 1..5 {
            let added = harness.run(Request::Edit(Command::AddMidiTrack {
                name: format!("synth {index}"),
            }));
            let Some(Created::Track(track)) = added.created else {
                panic!("midi track");
            };
            (applied, _) = harness.applied(applied);
            let placed = harness.run(Request::Edit(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: clip_length,
            }));
            let Some(Created::Clip(clip)) = placed.created else {
                panic!("midi clip");
            };
            (applied, _) = harness.applied(applied);
            let note = harness.run(Request::Edit(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: note_duration,
                pitch,
                channel,
                velocity: 112,
            }));
            (applied, _) = harness.applied(applied);
            let mut insertion = request(&note.state, PluginEffectDestination::Instrument(track));
            insertion.selection = installed_selection(
                &PathBuf::from(std::env::var_os("HEX_TEST_VST3_INSTRUMENT_CATALOG").unwrap()),
                installed.as_ref().unwrap(),
                hex_plugin_catalog::PluginRole::Instrument,
            );
            harness
                .service
                .submit(Request::InsertPluginEffect(insertion))
                .unwrap();
            let event = terminal(&mut harness);
            assert!(event.error.is_none(), "{:?}", event.error);
            assert!(event.plugin_effect_complete[0].accepted);
            assert_eq!(
                event
                    .state
                    .project
                    .tracks()
                    .iter()
                    .filter(|track| track.instrument().is_some())
                    .count(),
                index + 1
            );
            (applied, _) = harness.applied(applied);
        }
    }
    if matches!(workflow, InstrumentWorkflow::AuthoringAcrossPlay) {
        automation_start::verify(
            &mut harness,
            project_track.instrument().unwrap().id(),
            applied,
        );
        return;
    }
    if takeover {
        verify_instrument_takeover(
            &mut harness,
            &accepted.state.project,
            project_track.instrument().unwrap().id(),
            applied,
            recording_catalog.as_ref().map(|files| files.0.as_path()),
        );
        return;
    }
    if paced {
        verify_paced_instrument_authoring(
            &mut harness,
            instrument,
            applied,
            monitor,
            matches!(workflow, InstrumentWorkflow::PacedFiveHistory),
        );
        return;
    }
    if shared_authoring {
        verify_shared_instrument_authoring(
            &mut harness,
            &accepted.state.project,
            project_track.instrument().unwrap().id(),
            if controlled_fixture { "gain" } else { "Output" },
            applied,
        );
        return;
    }
    if let Some(parameter) = automation_parameter {
        verify_saved_instrument_automation(
            &mut harness,
            &accepted.state.project,
            project_track.instrument().unwrap().id(),
            &parameter,
        );
        return;
    }
    let console = console.unwrap();
    if matches!(workflow, InstrumentWorkflow::SessionSeek) {
        assert!(
            controlled_fixture,
            "this oracle requires the controlled SDK fixture"
        );
        let send = |harness: &mut ResidentHarness, command| {
            harness
                .audio
                .as_mut()
                .unwrap()
                .control
                .session_command_with_ticket(command)
                .unwrap();
        };
        let settle = |harness: &ResidentHarness| {
            for _ in 0..4 {
                render_completed_block(harness);
            }
        };
        let sounding = |harness: &ResidentHarness, stage: &str| {
            for frame in render_completed_block(harness) {
                assert!(
                    (frame[0] - 88.0 / 127.0).abs() < 1e-5,
                    "expected one SDK gate at {stage}: {frame:?}"
                );
                assert!((frame[0] - frame[1]).abs() < 1e-6);
            }
        };
        send(&mut harness, SessionPlaybackCommand::Play);
        settle(&harness);
        send(
            &mut harness,
            SessionPlaybackCommand::LaunchClip {
                clip: console,
                quantization: Immediate,
            },
        );
        settle(&harness);
        sounding(&harness, "before seek");

        send(&mut harness, SessionPlaybackCommand::Seek(12_000));
        settle(&harness);
        sounding(&harness, "after seek");
        send(
            &mut harness,
            SessionPlaybackCommand::StopTrack {
                track,
                quantization: Immediate,
            },
        );
        settle(&harness);
        assert_eq!(
            render_completed_block(&harness),
            [[0.0; 2]; 256],
            "seek left a prior gate"
        );

        let status = harness.audio.as_ref().unwrap().status().unwrap();
        assert!(status.playing.load(Acquire));
        assert!(status.position.load(Acquire) >= 12_000);
        assert!(status.rejected_action().is_none());
        assert!(status.render_fault_snapshot().is_none());
        return;
    }
    if matches!(workflow, InstrumentWorkflow::SessionStopTrace) {
        assert!(
            controlled_fixture,
            "this oracle requires the controlled SDK fixture"
        );
        let send = |harness: &mut ResidentHarness, command| {
            harness
                .audio
                .as_mut()
                .unwrap()
                .control
                .session_command_with_ticket(command)
                .unwrap();
        };
        send(&mut harness, SessionPlaybackCommand::Play);
        for _ in 0..4 {
            render_completed_block(&harness);
        }
        send(
            &mut harness,
            SessionPlaybackCommand::LaunchClip {
                clip: console,
                quantization: Immediate,
            },
        );
        for _ in 0..4 {
            render_completed_block(&harness);
        }
        let sounding = render_completed_block(&harness);
        assert!(
            sounding
                .iter()
                .all(|frame| (frame[0] - 88.0 / 127.0).abs() < 1e-5)
        );
        let directory = PathBuf::from(std::env::var_os("HEX_ORIGINAL_NOTE_TRACE_DIR").unwrap());
        let paths: Vec<_> = std::fs::read_dir(&directory)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "bin"))
            .collect();
        assert_eq!(
            paths.len(),
            1,
            "expected the one sounding SDK helper: {paths:?}"
        );
        let original = &paths[0];
        use std::os::unix::fs::MetadataExt as _;
        let before_inode = std::fs::metadata(original).unwrap().ino();
        harness.audio.as_ref().unwrap().control.stop();
        assert_eq!(harness.render_plugin_block(), [[0.0; 2]; 256]);
        let mut restored = harness.run(Request::RestoreStoppedPlayback);
        if !restored.stopped_playback_restore_complete {
            restored = restored_terminal(&mut harness);
        }
        assert!(restored.error.is_none(), "{:?}", restored.error);
        let (_, metadata) = harness.applied(applied);
        assert!(metadata.stopped_restore);
        let deadline = Instant::now() + Duration::from_secs(5);
        let notes = loop {
            harness.pump();
            let updated =
                std::fs::metadata(original).is_ok_and(|metadata| metadata.ino() != before_inode);
            let words = updated
                .then(|| std::fs::read(original).ok())
                .flatten()
                .filter(|bytes| bytes.len() == 20)
                .map(|bytes| {
                    bytes
                        .chunks_exact(4)
                        .map(|chunk| i32::from_ne_bytes(chunk.try_into().unwrap()))
                        .collect::<Vec<_>>()
                });
            if let Some(words) = words.filter(|words| words[1] >= 2) {
                break words;
            }
            assert!(
                Instant::now() < deadline,
                "original SDK helper trace did not update"
            );
            std::thread::yield_now();
        };
        assert_eq!(
            notes[1], notes[2],
            "original SDK endpoint retained a note: {notes:?}"
        );
        assert!(
            notes[3] > 0,
            "the held gate needs a real occurrence identity: {notes:?}"
        );
        assert_eq!(
            notes[3], notes[4],
            "the released identity differs: {notes:?}"
        );
        println!("original SDK helper {original:?} note trace={notes:?}");
        return;
    }
    if matches!(workflow, InstrumentWorkflow::SessionPause) {
        assert!(
            controlled_fixture,
            "this oracle requires the controlled SDK fixture"
        );
        let send = |harness: &mut ResidentHarness, command| {
            harness
                .audio
                .as_mut()
                .unwrap()
                .control
                .session_command_with_ticket(command)
                .unwrap();
        };
        send(&mut harness, SessionPlaybackCommand::Play);
        for _ in 0..4 {
            render_completed_block(&harness);
        }
        send(
            &mut harness,
            SessionPlaybackCommand::LaunchClip {
                clip: console,
                quantization: Immediate,
            },
        );
        for _ in 0..4 {
            render_completed_block(&harness);
        }
        let sounding = render_completed_block(&harness);
        for frame in sounding {
            assert!(
                (frame[0] - 88.0 / 127.0).abs() < 1e-5,
                "Session gate was not sounding: {frame:?}"
            );
            assert!((frame[0] - frame[1]).abs() < 1e-6);
        }
        send(&mut harness, SessionPlaybackCommand::Pause);
        for _ in 0..4 {
            render_completed_block(&harness);
        }
        assert_eq!(render_completed_block(&harness), [[0.0; 2]; 256]);
        let status = harness.audio.as_ref().unwrap().status().unwrap();
        assert!(!status.playing.load(Acquire));
        let paused_at = status.position.load(Acquire);
        assert!(
            paused_at < 24_000,
            "held note must not have reached its authored end"
        );
        send(&mut harness, SessionPlaybackCommand::Play);
        for _ in 0..4 {
            render_completed_block(&harness);
        }
        assert_eq!(render_completed_block(&harness), [[0.0; 2]; 256]);
        let status = harness.audio.as_ref().unwrap().status().unwrap();
        assert!(status.playing.load(Acquire));
        assert!(status.position.load(Acquire) > paused_at);
        assert!(status.position.load(Acquire) < 24_000);
        assert!(status.rejected_action().is_none());
        assert!(status.render_fault_snapshot().is_none());
        return;
    }
    if matches!(workflow, InstrumentWorkflow::SessionLoop) {
        assert!(
            controlled_fixture,
            "this oracle requires the controlled SDK fixture"
        );
        let send = |harness: &mut ResidentHarness, command| {
            harness
                .audio
                .as_mut()
                .unwrap()
                .control
                .session_command_with_ticket(command)
                .unwrap();
        };
        send(&mut harness, SessionPlaybackCommand::Play);
        for _ in 0..4 {
            render_completed_block(&harness);
        }
        send(
            &mut harness,
            SessionPlaybackCommand::LaunchClip {
                clip: console,
                quantization: Immediate,
            },
        );
        for _ in 0..3 {
            render_completed_block(&harness);
        }
        let pcm: Vec<_> = (0..12)
            .flat_map(|_| render_completed_block(&harness))
            .collect();
        let expected = 88.0 / 127.0;
        assert!(
            (pcm.last().unwrap()[0] - expected).abs() < 1e-5,
            "the final SDK gate must be sounding before StopTrack"
        );
        let mut runs = Vec::new();
        for frame in pcm {
            assert!((frame[0] - frame[1]).abs() < 1e-6);
            let on = if frame[0].abs() < 1e-6 {
                false
            } else {
                assert!(
                    (frame[0] - expected).abs() < 1e-5,
                    "unexpected sample {frame:?}"
                );
                true
            };
            if let Some((last, count)) = runs.last_mut()
                && *last == on
            {
                *count += 1;
            } else {
                runs.push((on, 1_usize));
            }
        }
        assert!(runs.len() >= 8, "expected several loop cycles: {runs:?}");
        for &(on, count) in &runs[1..runs.len() - 1] {
            assert_eq!(count, 375, "unexpected {on} gate length: {runs:?}");
        }
        send(
            &mut harness,
            SessionPlaybackCommand::StopTrack {
                track,
                quantization: Immediate,
            },
        );
        for _ in 0..4 {
            render_completed_block(&harness);
        }
        assert_eq!(render_completed_block(&harness), [[0.0; 2]; 256]);
        return;
    }
    if matches!(workflow, InstrumentWorkflow::SessionRestart) {
        assert!(
            controlled_fixture,
            "this oracle requires the controlled SDK fixture"
        );
        let send = |harness: &mut ResidentHarness, command| {
            harness
                .audio
                .as_mut()
                .unwrap()
                .control
                .session_command_with_ticket(command)
                .unwrap();
        };
        send(&mut harness, SessionPlaybackCommand::Play);
        for _ in 0..4 {
            render_completed_block(&harness);
        }
        send(
            &mut harness,
            SessionPlaybackCommand::LaunchClip {
                clip: console,
                quantization: Immediate,
            },
        );
        let peak = |frames: [hex_engine::Frame; 256]| {
            frames
                .into_iter()
                .flatten()
                .map(f32::abs)
                .fold(0.0, f32::max)
        };
        for _ in 0..4 {
            render_completed_block(&harness);
        }
        let before = peak(render_completed_block(&harness));
        assert!(
            (before - 88.0 / 127.0).abs() < 0.0001,
            "Session note must sound before restart, got {before}"
        );
        harness
            .audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::PlayFrom(12_000))
            .unwrap();
        for _ in 0..4 {
            render_completed_block(&harness);
        }
        let after = peak(render_completed_block(&harness));
        assert!(
            (before - after).abs() < 0.0001,
            "Session restart must leave exactly one SDK gate: before={before}, after={after}"
        );
        let status = harness.audio.as_ref().unwrap().status().unwrap();
        assert!(status.position.load(Acquire) >= 12_000);
        assert!(status.rejected_action().is_none());
        assert!(status.render_fault_snapshot().is_none());
        send(
            &mut harness,
            SessionPlaybackCommand::StopTrack {
                track,
                quantization: Immediate,
            },
        );
        for _ in 0..3 {
            render_completed_block(&harness);
        }
        assert_eq!(render_completed_block(&harness), [[0.0; 2]; 256]);
        return;
    }
    // Exercise the resident keyboard request on a newly inserted worker
    // before the authored transport has ever started.
    let keys = Arc::new(
        harness
            .audio
            .as_ref()
            .unwrap()
            .control
            .new_computer_keyboard_input_on_control()
            .unwrap()
            .with_live_notes()
            .unwrap(),
    );
    let context = harness.submitted.unwrap().1.context;
    harness.run(Request::PrepareComputerKeyboard {
        project_generation: context.project_generation,
        revision: context.editor_revision,
        request_id: 1,
        track,
        input: Arc::clone(&keys),
    });
    let (keyboard_revision, keyboard) = harness.applied(applied);
    applied = keyboard_revision;
    assert_eq!(keyboard.computer_keyboard_request, Some(1));
    let pcm_peak = |frames: [hex_engine::Frame; 256]| {
        frames
            .into_iter()
            .flatten()
            .map(f32::abs)
            .fold(0.0, f32::max)
    };
    keys.set_key(0, Some((pitch, 100, 1))).unwrap();
    let keyboard_peak = (0..6)
        .map(|_| pcm_peak(render_completed_block(&harness)))
        .fold(0.0, f32::max);
    assert!(keyboard_peak > 0.001, "expected stopped keyboard pcm");
    assert!(
        !harness
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .playing
            .load(Acquire)
    );
    keys.set_key(0, None).unwrap();
    for _ in 0..4 {
        render_completed_block(&harness);
    }
    if controlled_fixture {
        assert_eq!(render_completed_block(&harness), [[0.0; 2]; 256]);
    } else {
        let mut release_peak = f32::INFINITY;
        let mut quiet_blocks = 0;
        assert!(
            (0..1000).any(|_| {
                release_peak = pcm_peak(render_completed_block(&harness));
                quiet_blocks = if release_peak < 0.001 {
                    quiet_blocks + 1
                } else {
                    0
                };
                quiet_blocks == 4
            }),
            "keyboard release must settle, held peak={keyboard_peak}, release peak={release_peak}"
        );
    }
    println!("instrument keyboard rate={rate} peak={keyboard_peak}");
    if controlled_fixture {
        use hex_engine::computer_keyboard::LiveNoteEvent;
        let activation = keys.begin_live_notes_activation_on_control().unwrap();
        let queue = keys.live_notes().unwrap();
        queue
            .publish(
                activation,
                LiveNoteEvent::NoteOn {
                    channel: 0,
                    pitch,
                    velocity: 100,
                },
            )
            .unwrap();
        queue
            .publish(
                activation,
                LiveNoteEvent::NoteOff {
                    channel: 0,
                    pitch,
                    velocity: 41,
                },
            )
            .unwrap();
        let audible_frames = (0..6)
            .flat_map(|_| render_completed_block(&harness))
            .filter(|frame| frame.iter().any(|sample| sample.abs() > 0.001))
            .count();
        assert_eq!(
            audible_frames, 1,
            "ordered hardware tap must last one frame"
        );
        assert_eq!(render_completed_block(&harness), [[0.0; 2]; 256]);
        println!("instrument hardware tap rate={rate} frames={audible_frames}");
        queue
            .publish(
                activation,
                LiveNoteEvent::NoteOn {
                    channel: 0,
                    pitch,
                    velocity: 93,
                },
            )
            .unwrap();
        let held_peak = (0..6)
            .map(|_| pcm_peak(render_completed_block(&harness)))
            .fold(0.0, f32::max);
        assert!(held_peak > 0.001);
        keys.trigger_live_note_audition_on_control(&[(pitch, 37)], 256)
            .unwrap();
        let combined_peak = (0..6)
            .map(|_| pcm_peak(render_completed_block(&harness)))
            .fold(0.0, f32::max);
        assert!(
            combined_peak > held_peak * 1.25,
            "preview and controller sound together"
        );
        let after_preview = pcm_peak(render_completed_block(&harness));
        assert!(
            (after_preview - held_peak).abs() < 0.0001,
            "only the preview expires"
        );
        queue
            .publish(
                activation,
                LiveNoteEvent::NoteOff {
                    channel: 0,
                    pitch,
                    velocity: 41,
                },
            )
            .unwrap();
        for _ in 0..4 {
            render_completed_block(&harness);
        }
        assert_eq!(render_completed_block(&harness), [[0.0; 2]; 256]);
        assert!(queue.same_current_activation(activation));
    }
    // Stop advancing at frame 64 of the copied quantum. Play must carry its
    // new context inside that same worker block.
    let worker_phase = || {
        harness
            .callback
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .native_worker_phase_for_test()
    };
    let wait_for_short_callback = |harness: &ResidentHarness| {
        harness.render();
        wait(|| {
            harness
                .callback
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .native_workers_idle_on_control()
                == Some(true)
        });
    };
    for _ in 0..4 {
        if worker_phase() == Some(64) {
            break;
        }
        wait_for_short_callback(&harness);
    }
    assert_eq!(worker_phase(), Some(64));
    harness
        .audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let mut peak = 0.0_f32;
    let mut peak_after_edit = 0.0_f32;
    for block in 0..20 {
        let frames = render_completed_block(&harness);
        if block < 2 {
            let early_peak = pcm_peak(frames);
            assert!(early_peak < 0.001, "early block {block}: {early_peak}");
        }
        for sample in frames.iter().flatten() {
            peak = peak.max(sample.abs());
            if block > 3 {
                peak_after_edit = peak_after_edit.max(sample.abs());
            }
        }
        if block == 3 {
            let before = harness
                .audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .native_effect_source()
                .unwrap();
            harness.run(Request::Edit(Command::Balance {
                track,
                balance: -0.25,
            }));
            let (revision, _) = harness.applied(applied);
            let status = harness.audio.as_ref().unwrap().status().unwrap();
            let after = status.native_effect_source().unwrap();
            assert_eq!(after.audio_revision, revision);
            assert_eq!(after.output_identity, before.output_identity);
            assert!(status.playing.load(Acquire));
        }
    }
    assert!(peak > 0.001, "expected instrument pcm, peak={peak}");
    assert!(
        peak_after_edit > 0.001,
        "held instrument note must still sound, peak={peak_after_edit}"
    );
    println!("instrument arrangement rate={rate} peak={peak}");

    let send = |harness: &mut ResidentHarness, command| {
        harness
            .audio
            .as_mut()
            .unwrap()
            .control
            .session_command_with_ticket(command)
            .unwrap();
    };
    send(
        &mut harness,
        SessionPlaybackCommand::LaunchClip {
            clip: console,
            quantization: Immediate,
        },
    );
    for _ in 0..3 {
        render_completed_block(&harness);
    }
    let console_peak = render_completed_block(&harness)
        .iter()
        .flatten()
        .map(|sample| sample.abs())
        .fold(0.0_f32, f32::max);
    assert!(console_peak > 0.001, "expected console instrument pcm");
    send(
        &mut harness,
        SessionPlaybackCommand::StopTrack {
            track,
            quantization: Immediate,
        },
    );
    for _ in 0..3 {
        render_completed_block(&harness);
    }
    let after_stop = render_completed_block(&harness);
    if controlled_fixture {
        assert!(
            after_stop.iter().all(|frame| *frame == [0.0; 2]),
            "stopped console must release the last gate"
        );
    }
    println!("instrument console rate={rate} peak={console_peak}");

    if controlled_fixture {
        // Hold both sources in the retained SDK component through the
        // desktop's priority Stop and stopped-restore handshake.
        send(
            &mut harness,
            SessionPlaybackCommand::LaunchClip {
                clip: console,
                quantization: Immediate,
            },
        );
        for _ in 0..3 {
            render_completed_block(&harness);
        }
        let authored_peak = pcm_peak(render_completed_block(&harness));
        assert!(authored_peak > 0.001);
        keys.set_key(0, Some((60, 100, 2))).unwrap();
        let combined_peak = (0..5)
            .map(|_| pcm_peak(render_completed_block(&harness)))
            .fold(0.0, f32::max);
        assert!(
            combined_peak > 1.5 * authored_peak,
            "expected both held gates"
        );
        let before_stop = harness
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(Acquire);
        harness.audio.as_ref().unwrap().control.stop();
        assert_eq!(harness.render_plugin_block(), [[0.0; 2]; 256]);
        harness
            .service
            .submit(Request::RestoreStoppedPlayback)
            .unwrap();
        let pending = harness.next();
        assert!(pending.error.is_none(), "{:?}", pending.error);
        assert!(!pending.stopped_playback_restore_complete);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            harness.pump();
            harness.render_plugin_block();
            if let Ok(event) = harness.service.try_event() {
                assert!(event.error.is_none(), "{:?}", event.error);
                if event.stopped_playback_restore_complete {
                    break;
                }
            }
            assert!(
                Instant::now() < deadline,
                "stopped restore terminal timed out"
            );
            std::thread::yield_now();
        }
        let (_, restored) = harness.applied(before_stop);
        assert!(restored.stopped_restore);
        wait(|| harness.plugin_completed.load(Acquire) == 0);
        for _ in 0..4 {
            assert_eq!(render_completed_block(&harness), [[0.0; 2]; 256]);
        }
        keys.ensure_output_activation_on_control().unwrap();
        keys.set_key(0, Some((60, 100, 3))).unwrap();
        let restored_peak = (0..6)
            .map(|_| pcm_peak(render_completed_block(&harness)))
            .fold(0.0, f32::max);
        assert!(restored_peak > 0.001, "expected fresh stopped keyboard pcm");
        assert!(
            !harness
                .audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .playing
                .load(Acquire)
        );
        keys.set_key(0, None).unwrap();
        for _ in 0..4 {
            render_completed_block(&harness);
        }
        assert_eq!(render_completed_block(&harness), [[0.0; 2]; 256]);
        println!("instrument keyboard after priority stop rate={rate} peak={restored_peak}");
        // The restored Arrangement contains a note at zero. Route to silence
        // before starting the callback to isolate any retained old SDK gate.
        wait(|| {
            harness.pump();
            harness
                .audio
                .as_mut()
                .unwrap()
                .control
                .session_command_with_ticket(SessionPlaybackCommand::StopTrack {
                    track,
                    quantization: Immediate,
                })
                .is_ok()
        });
        send(&mut harness, SessionPlaybackCommand::Play);
        for _ in 0..4 {
            assert_eq!(render_completed_block(&harness), [[0.0; 2]; 256]);
        }
        send(
            &mut harness,
            SessionPlaybackCommand::LaunchClip {
                clip: console,
                quantization: Immediate,
            },
        );
        for _ in 0..3 {
            render_completed_block(&harness);
        }
        assert!(
            render_completed_block(&harness)
                .iter()
                .flatten()
                .any(|sample| sample.abs() > 0.001)
        );
        send(
            &mut harness,
            SessionPlaybackCommand::StopTrack {
                track,
                quantization: Immediate,
            },
        );
        for _ in 0..3 {
            render_completed_block(&harness);
        }
        assert_eq!(render_completed_block(&harness), [[0.0; 2]; 256]);

        // Return to the authored Arrangement, then seek out of a held note.
        // The worker's copied output must release it after its bridge latency.
        send(
            &mut harness,
            SessionPlaybackCommand::BackToArrangement {
                quantization: Immediate,
            },
        );
        render_completed_block(&harness);
        harness
            .audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::PlayFrom(0))
            .unwrap();
        let held_peak = (0..8)
            .map(|_| pcm_peak(render_completed_block(&harness)))
            .fold(0.0, f32::max);
        assert!(
            held_peak > 0.001,
            "the authored note must be held before seeking"
        );
        harness
            .audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Seek(48_000))
            .unwrap();
        for _ in 0..6 {
            render_completed_block(&harness);
        }
        for _ in 0..2 {
            assert_eq!(
                render_completed_block(&harness),
                [[0.0; 2]; 256],
                "seek must release the held SDK gate"
            );
        }
        let status = harness.audio.as_ref().unwrap().status().unwrap();
        assert!(status.position.load(Acquire) >= 48_000);
        assert!(status.rejected_action().is_none());
        assert!(status.render_fault_snapshot().is_none());
        harness
            .audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::PlayFrom(0))
            .unwrap();
        let fresh_peak = (0..8)
            .map(|_| pcm_peak(render_completed_block(&harness)))
            .fold(0.0, f32::max);
        assert!(
            fresh_peak > 0.001,
            "a fresh authored note must sound after seek"
        );
        let status = harness.audio.as_ref().unwrap().status().unwrap();
        assert!(status.rejected_action().is_none());
        assert!(status.render_fault_snapshot().is_none());
    }
}

fn verify_paced_instrument_authoring(
    harness: &mut ResidentHarness,
    device: hex_project::DeviceId,
    mut applied: u64,
    monitor: bool,
    history: bool,
) {
    use hex_project::{
        AutomationInterpolation, AutomationPoint, AutomationValue, DeviceAutomationTarget,
    };
    use hex_session::service::PluginParameterQuery;
    let context = harness.submitted.unwrap().1.context;
    let output_identity = harness
        .audio
        .as_ref()
        .unwrap()
        .control
        .output_identity()
        .unwrap();
    let page_first: u16 = std::env::var("HEX_TEST_VST3_INSTRUMENT_AUTOMATION_PAGE")
        .unwrap_or_else(|_| "148".into())
        .parse()
        .expect("parameter page offset");
    harness.run(Request::QueryPluginParameters(PluginParameterQuery {
        context,
        device,
        output_identity,
        request_id: 10,
        page_first: Some(page_first),
    }));
    let rows = harness.next().plugin_parameters.unwrap().rows.unwrap();
    let parameter: u32 = std::env::var("HEX_TEST_VST3_INSTRUMENT_AUTOMATION_ID")
        .unwrap_or_else(|_| "148".into())
        .parse()
        .expect("parameter id");
    let row = rows
        .iter()
        .find(|row| row.id == parameter)
        .expect("requested parameter in inventory");
    let target = DeviceAutomationTarget::Vst3 { parameter: row.id };
    let class = row.automation_class.expect("automatable parameter");
    println!("paced_vst3 parameter={} id={}", row.name, row.id);
    harness
        .audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let period = Duration::from_nanos(
        256 * 1_000_000_000 / u64::from(harness.audio.as_ref().unwrap().sample_rate),
    );
    let start = Instant::now();
    let mut pending = false;
    let mut sent = 0;
    let mut completed = 0;
    let mut query_pending = None;
    let mut query_sent = 0_u64;
    let mut query_completed = 0_u64;
    let mut query_changed = 0_u64;
    let mut query_yielded = 0_u64;
    let mut revision = context.editor_revision;
    let mut max_lateness = Duration::ZERO;
    let mut previous_callback = None;
    let mut recent_callbacks = [(0_u32, 0_u128, 0_u128, 0_u128); 8];
    let mut peak = 0.0_f32;
    let mut tail_peak = 0.0_f32;
    // The short default remains useful for routine feedback. The original
    // report failed after tens of thousands of blocks, so permit an explicit
    // longer stress run without lengthening ordinary checks.
    let blocks = std::env::var("HEX_TEST_VST3_PACED_BLOCKS")
        .map(|value| value.parse::<u32>().expect("paced block count"))
        .unwrap_or(600);
    assert!(
        (600..=100_000).contains(&blocks),
        "paced block count out of range"
    );
    let burst_delay = std::env::var("HEX_TEST_VST3_BURST_MILLIS")
        .map(|value| value.parse::<u64>().expect("burst delay"))
        .unwrap_or(0);
    assert!(burst_delay <= 25, "burst delay out of range");
    for block in 0..blocks {
        let deadline = start + period * block;
        if let Some(delay) = deadline.checked_duration_since(Instant::now()) {
            std::thread::sleep(delay);
        }
        if burst_delay != 0 && block != 0 && block % 100 == 0 {
            std::thread::sleep(Duration::from_millis(burst_delay));
        }
        harness.pump();
        let callback_start = Instant::now();
        let late = callback_start.saturating_duration_since(deadline);
        let gap = previous_callback.map_or(Duration::ZERO, |previous| {
            callback_start.duration_since(previous)
        });
        previous_callback = Some(callback_start);
        max_lateness = max_lateness.max(late);
        let frames = harness.render_plugin_block();
        recent_callbacks[block as usize % 8] = (
            block,
            late.as_micros(),
            gap.as_micros(),
            callback_start.elapsed().as_micros(),
        );
        let block_peak = frames
            .into_iter()
            .flatten()
            .fold(0.0_f32, |value, sample| value.max(sample.abs()));
        peak = peak.max(block_peak);
        if block >= blocks - 50 {
            tail_peak = tail_peak.max(block_peak);
        }
        if monitor && query_pending.is_none() && (30..500).contains(&block) && block % 25 == 0 {
            let query_context = hex_engine::AudioSourceContext {
                editor_revision: revision,
                ..context
            };
            let request_id = 100 + query_sent;
            harness
                .service
                .submit(Request::QueryPluginParameters(PluginParameterQuery {
                    context: query_context,
                    device,
                    output_identity,
                    request_id,
                    page_first: Some(page_first),
                }))
                .unwrap();
            query_sent += 1;
            query_pending = Some(request_id);
        }
        let edit_count = if history { 5 } else { 3 };
        let spacing = if history { 100 } else { 125 };
        if !pending && sent < edit_count && block >= 50 + sent * spacing {
            let points = [
                (BeatTime::ZERO, 0.4),
                (BeatTime::from_ratio(1 + u64::from(sent), 8).unwrap(), 0.7),
            ]
            .map(|(beat, value)| AutomationPoint {
                beat,
                value: AutomationValue::new(value).unwrap(),
                outgoing: AutomationInterpolation::Step,
            })
            .to_vec();
            let request = if sent == 3 {
                Request::Undo
            } else if sent == 4 {
                Request::Redo
            } else {
                Request::DeviceAutomation {
                    project_generation: context.project_generation,
                    revision,
                    device,
                    target,
                    vst3_class: Some(class),
                    points,
                }
            };
            harness.service.submit(request).unwrap();
            sent += 1;
            pending = true;
        }
        while let Ok(event) = harness.service.try_event() {
            if let Some(error) = &event.error {
                harness.pump();
                let progress = *harness.plugin_progress.lock().unwrap();
                let status = harness.audio.as_ref().unwrap().status().unwrap();
                let fault = status.render_fault_snapshot();
                let source = status.native_effect_source();
                let blocks = harness.plugin_completed.load(Acquire);
                let worker_failure = harness.plugin_failure.lock().unwrap().clone();
                panic!(
                    "live automation failed at block {block}, edits={sent}/{completed}, query={query_pending:?}, observation={}, progress={progress:?}, source={source:?}, blocks={blocks}, degraded={:?}, max_late={max_lateness:?}, render_fault={fault:?}, worker_failure={worker_failure:?}: {error}",
                    event.plugin_parameter_observation, harness.degraded_devices
                );
            }
            if let Some(result) = event.plugin_parameters {
                assert_eq!(query_pending, Some(result.query.request_id));
                match result.rows {
                    Ok(_) => query_completed += 1,
                    Err(error) if error == "vst3 parameter refresh yielded to control" => {
                        query_yielded += 1;
                    }
                    Err(error) => {
                        assert!(
                            error.contains("changed") || error.contains("cancel"),
                            "unexpected live query error: {error}"
                        );
                        query_changed += 1;
                    }
                }
                query_pending = None;
            } else if event.state.revision > revision {
                assert!(pending, "unexpected project revision during polling");
                if history && completed >= 3 {
                    let points = &event
                        .state
                        .project
                        .device(device)
                        .unwrap()
                        .automation_target(target)
                        .unwrap()
                        .points;
                    let expected_beat = if completed == 3 { 2 } else { 3 };
                    assert_eq!(points.len(), 2);
                    assert_eq!(
                        points[1].beat,
                        BeatTime::from_ratio(expected_beat, 8).unwrap()
                    );
                }
                revision = event.state.revision;
                pending = false;
                completed += 1;
            } else {
                assert!(
                    monitor && query_pending.is_some(),
                    "unexpected event during authoring"
                );
            }
        }
        assert!(
            harness.degraded_devices.is_empty(),
            "vst endpoint degraded after block {block}, maximum lateness {:?}: {:?}; recent (block, late_us, gap_us, render_us)={recent_callbacks:?}; worker failure={:?}",
            max_lateness,
            harness.degraded_devices,
            *harness.plugin_failure.lock().unwrap()
        );
        let status = harness.audio.as_ref().unwrap().status().unwrap();
        if block >= 50 {
            assert!(
                status.playing.load(Acquire),
                "the edit requires active playback"
            );
        }
        assert!(!status.device_fault.load(Acquire) && !status.render_fault.load(Acquire));
        applied = applied.max(status.applied_revision.load(Acquire));
    }
    harness.pump();
    assert!(
        harness.degraded_devices.is_empty(),
        "vst endpoint degraded: {:?}",
        harness.degraded_devices
    );
    let expected_edits = if history { 5 } else { 3 };
    assert_eq!(
        (sent, completed, pending),
        (expected_edits, expected_edits, false)
    );
    if monitor {
        assert!(
            query_completed >= 5,
            "too few parameter queries completed: sent={query_sent} completed={query_completed} changed={query_changed} yielded={query_yielded}"
        );
        assert!(
            query_yielded > 0,
            "the test must exercise a read yielding to an automation edit"
        );
        assert!(
            query_pending.is_none(),
            "the last parameter query must complete"
        );
        assert_eq!(query_sent, query_completed + query_changed + query_yielded);
        println!(
            "paced_vst3 queries_sent={query_sent} completed={query_completed} invalidated={query_changed} yielded={query_yielded} pending={}",
            query_pending.is_some()
        );
    }
    let source = harness
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .native_effect_source()
        .unwrap();
    assert_eq!(
        source.context.editor_revision, revision,
        "last edit must be applied"
    );
    assert!(
        peak > 0.001 && tail_peak > 0.001,
        "instrument must remain audible after editing"
    );
    println!(
        "paced_vst3 rate={} blocks={blocks} edits={completed} applied={applied} max_late_us={} peak={peak} tail_peak={tail_peak}",
        harness.audio.as_ref().unwrap().sample_rate,
        max_lateness.as_micros()
    );
}

fn verify_shared_instrument_authoring(
    harness: &mut ResidentHarness,
    original: &hex_project::Project,
    device: hex_project::DeviceId,
    parameter_name: &str,
    mut applied: u64,
) {
    use hex_project::{
        AutomationInterpolation, AutomationPoint, AutomationReadSelection as Read, AutomationValue,
    };
    use hex_session::service::{DeviceAutomationControlEdit as Edit, PluginParameterQuery};
    let context = harness.submitted.unwrap().1.context;
    let output = harness
        .audio
        .as_ref()
        .unwrap()
        .control
        .output_identity()
        .unwrap();
    harness.run(Request::QueryPluginParameters(PluginParameterQuery {
        context,
        device,
        output_identity: output,
        request_id: 10,
        page_first: None,
    }));
    let rows = harness.next().plugin_parameters.unwrap().rows.unwrap();
    let row = rows
        .iter()
        .find(|row| row.name.eq_ignore_ascii_case(parameter_name))
        .unwrap();
    let class = row.automation_class.unwrap();
    let target = hex_project::DeviceAutomationTarget::Vst3 { parameter: row.id };
    let points = [
        (BeatTime::ZERO, 0.0),
        (BeatTime::from_ratio(1, 8).unwrap(), 0.8),
    ]
    .map(|(beat, value)| AutomationPoint {
        beat,
        value: AutomationValue::new(value).unwrap(),
        outgoing: AutomationInterpolation::Step,
    })
    .to_vec();
    let authored = super::automation::edit_with_running_callback(
        harness,
        Request::DeviceAutomation {
            project_generation: context.project_generation,
            revision: context.editor_revision,
            device,
            target,
            vst3_class: Some(class),
            points: points.clone(),
        },
    );
    assert!(authored.error.is_none(), "{:?}", authored.error);
    assert_eq!(
        authored
            .state
            .project
            .device(device)
            .unwrap()
            .automation_target(target)
            .unwrap()
            .points,
        points
    );
    applied = harness.applied(applied).0;
    let undone = super::automation::edit_with_running_callback(harness, Request::Undo);
    assert!(undone.error.is_none(), "{:?}", undone.error);
    assert_eq!(undone.state.project.as_ref(), original);
    applied = harness.applied(applied).0;
    let redone = super::automation::edit_with_running_callback(harness, Request::Redo);
    assert!(redone.error.is_none(), "{:?}", redone.error);
    assert_eq!(redone.state.project, authored.state.project);
    applied = harness.applied(applied).0;
    let base = super::automation::edit_with_running_callback(
        harness,
        Request::DeviceAutomationControl {
            project_generation: context.project_generation,
            revision: redone.state.revision,
            edit: Edit::ReadSelection {
                device,
                target,
                vst3_class: Some(class),
                selection: Read::Base,
            },
        },
    );
    assert!(base.error.is_none(), "{:?}", base.error);
    assert_eq!(
        base.state
            .project
            .device(device)
            .unwrap()
            .automation_target(target)
            .unwrap()
            .read_selection,
        Read::Base
    );
    applied = harness.applied(applied).0;
    let enabled = super::automation::edit_with_running_callback(
        harness,
        Request::DeviceAutomationControl {
            project_generation: context.project_generation,
            revision: base.state.revision,
            edit: Edit::Reenable,
        },
    );
    assert!(enabled.error.is_none(), "{:?}", enabled.error);
    assert_eq!(
        enabled
            .state
            .project
            .device(device)
            .unwrap()
            .automation_target(target)
            .unwrap()
            .read_selection,
        Read::Authored
    );
    harness.applied(applied);
    let files = Files::new();
    let path = files.0.join("shared-instrument-lane.hex");
    let saved = harness.run(Request::SaveNew(path.clone()));
    assert!(saved.save_complete);
    assert_eq!(
        hex_storage::load(path)
            .unwrap()
            .device(device)
            .unwrap()
            .automation_target(target)
            .unwrap(),
        saved
            .state
            .project
            .device(device)
            .unwrap()
            .automation_target(target)
            .unwrap()
    );
    assert_instrument_takeover_begin(
        harness,
        device,
        row.id,
        &saved.state.project,
        saved.state.revision,
    );
    harness
        .audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let peaks = (0..32)
        .map(|_| {
            render_completed_block(harness)
                .into_iter()
                .flatten()
                .map(f32::abs)
                .fold(0.0, f32::max)
        })
        .collect::<Vec<_>>();
    let before = peaks[6..8].iter().copied().fold(0.0, f32::max);
    let during = peaks[20..32].iter().copied().fold(0.0, f32::max);
    println!(
        "shared instrument parameter={} id={} before={before} during={during}",
        row.name, row.id
    );
    assert!(
        during > 0.001 && before < during * 0.05,
        "authored lane must reach the actual instrument: {peaks:?}"
    );
}

fn assert_instrument_takeover_begin(
    harness: &mut ResidentHarness,
    device: hex_project::DeviceId,
    parameter: u32,
    expected: &hex_project::Project,
    revision: u64,
) {
    let context = harness.submitted.unwrap().1.context;
    let output = harness
        .audio
        .as_ref()
        .unwrap()
        .control
        .output_identity()
        .unwrap();
    harness.run(Request::QueryPluginParameters(
        hex_session::service::PluginParameterQuery {
            context,
            device,
            output_identity: output,
            request_id: 2,
            page_first: None,
        },
    ));
    let rows = harness.next().plugin_parameters.unwrap().rows.unwrap();
    let current = rows.iter().find(|row| row.id == parameter).unwrap();
    let mut client = hex_session::device_gesture::GestureClient::default();
    client
        .begin_plugin(hex_session::device_gesture::PluginParameterOrigin {
            context,
            device,
            parameter: current.id,
            page_first: current.page_first,
            metadata_revision: current.metadata_revision,
            initial: hex_session::device_gesture::PluginParameterValue::new(current.normalized)
                .unwrap(),
        })
        .unwrap();
    let begin = client.next_request().unwrap();
    client.mark_submitted();
    let refused = harness.run(Request::DeviceGesture(begin));
    let ack = refused.gesture_ack.unwrap();
    assert!(ack.accepted && !ack.source_lost, "{ack:?}");
    assert!(client.acknowledge(&ack));
    assert_eq!(refused.state.project.as_ref(), expected);
    assert_eq!(refused.state.revision, revision);
    client.cancel().unwrap();
    let cancel = client.next_request().unwrap();
    client.mark_submitted();
    let cancelled = harness.run(Request::DeviceGesture(cancel));
    let ack = cancelled.gesture_ack.unwrap();
    assert!(ack.accepted, "{ack:?}");
    assert!(client.acknowledge(&ack));
    assert_eq!(cancelled.state.project.as_ref(), expected);
}

/// Deliberately vary callback sizes while the worker owns complete SDK quanta.
fn render_takeover_partial(
    h: &ResidentHarness,
    frames: usize,
    held: bool,
    label: &str,
    request: Option<&hex_session::device_gesture::GestureRequest>,
) {
    let status = h.audio.as_ref().unwrap().status().unwrap();
    let start = status.position.load(Acquire);
    let performance_start = status.performance_frame.load(Acquire);
    let binding = h.plugin_progress.lock().unwrap().binding;
    let mut pcm = [[0.0; 2]; 256];
    let mut host = [0.0; 512];
    let mut slot = h.callback.lock().unwrap();
    let callback = slot.as_mut().unwrap();
    let measured = allocation_counter::measure(|| {
        callback
            .render_native_direct(&mut pcm[..frames], &mut host[..frames * 2])
            .unwrap();
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
    if held {
        assert!(
            pcm[..frames]
                .iter()
                .flatten()
                .all(|sample| sample.abs() > 1.0e-6),
            "held gate dropped: case={label} request={request:?} frames={frames} position={start}->{} performance={performance_start}->{} playing={} source={:?} fault={:?} phase={:?} source_fault={:?} head={:?}",
            status.position.load(Acquire),
            status.performance_frame.load(Acquire),
            status.playing.load(Acquire),
            status.recording_source(),
            status.render_fault_snapshot(),
            callback.native_worker_phase_for_test(),
            binding.and_then(|binding| callback.native_source_fault_on_control(binding)),
            &pcm[..frames.min(5)],
        );
    }
}

fn verify_instrument_takeover(
    h: &mut ResidentHarness,
    original: &hex_project::Project,
    device: hex_project::DeviceId,
    mut applied: u64,
    recovery_directory: Option<&std::path::Path>,
) {
    use hex_project::{
        AutomationInterpolation, AutomationPoint, AutomationValue, DeviceAutomationTarget,
    };
    use hex_session::device_gesture::{GestureClient, PluginParameterOrigin, PluginParameterValue};
    use hex_session::service::{DeviceAutomationControlEdit, PluginParameterQuery};
    let case = std::env::var("HEX_DESKTOP_INSTRUMENT_TAKEOVER_CASE").unwrap();
    let author = matches!(case.as_str(), "author" | "author_empty");
    let idle_unlaned = case == "idle_empty";
    let first_lane = matches!(
        case.as_str(),
        "session_first_lane" | "session_first_lane_with_other" | "session_first_lane_stop"
    );
    let stop_recording = matches!(case.as_str(), "session_stop" | "session_first_lane_stop");
    let reading =
        !first_lane && !matches!(case.as_str(), "unread" | "empty" | "renew" | "idle_empty");
    let playing = !matches!(case.as_str(), "idle" | "idle_empty");
    let context = h.submitted.unwrap().1.context;
    let output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    let mut original_binding = None;
    let class = if case == "author_empty" {
        hex_plugin_state::decode(original.device(device).unwrap().state())
            .unwrap()
            .identity
            .class
            .bytes()
    } else {
        h.run(Request::QueryPluginParameters(PluginParameterQuery {
            context,
            device,
            output_identity: output,
            request_id: 11,
            page_first: None,
        }));
        let result = h.next().plugin_parameters.unwrap();
        original_binding = result.source;
        let rows = result.rows.unwrap();
        assert_eq!(rows.len(), 2);
        rows.iter()
            .find(|r| r.id == 7)
            .unwrap()
            .automation_class
            .unwrap()
    };
    let mut before = original.clone();
    let mut revision = context.editor_revision;
    for (id, value) in [(7, 0.25), (11, 0.5)] {
        if matches!(
            case.as_str(),
            "empty"
                | "renew"
                | "author_empty"
                | "idle_empty"
                | "session_first_lane"
                | "session_first_lane_stop"
        ) || id == 7 && (!reading || author)
        {
            continue;
        }
        let event = super::automation::edit_with_running_callback(
            h,
            Request::DeviceAutomation {
                project_generation: context.project_generation,
                revision,
                device,
                target: DeviceAutomationTarget::Vst3 { parameter: id },
                vst3_class: Some(class),
                points: vec![AutomationPoint {
                    beat: BeatTime::ZERO,
                    value: AutomationValue::new(value).unwrap(),
                    outgoing: AutomationInterpolation::Step,
                }],
            },
        );
        assert!(event.error.is_none(), "{:?}", event.error);
        revision = event.state.revision;
        before = (*event.state.project).clone();
        applied = h.applied(applied).0;
    }
    let peak = |h: &ResidentHarness| {
        (0..4)
            .map(|_| {
                render_completed_block(h)
                    .into_iter()
                    .flatten()
                    .map(f32::abs)
                    .fold(0.0, f32::max)
            })
            .last()
            .unwrap()
    };
    if playing {
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
    }
    let baseline = if playing {
        let p = peak(h);
        assert!(p > 0.001);
        p
    } else {
        0.0
    };
    if author {
        let binding = h.plugin_progress.lock().unwrap().binding.unwrap();
        for value in [0.5, 0.25] {
            h.service
                .submit(Request::DeviceAutomation {
                    project_generation: context.project_generation,
                    revision,
                    device,
                    target: DeviceAutomationTarget::Vst3 { parameter: 7 },
                    vst3_class: Some(class),
                    points: vec![AutomationPoint {
                        beat: BeatTime::ZERO,
                        value: AutomationValue::new(value).unwrap(),
                        outgoing: AutomationInterpolation::Step,
                    }],
                })
                .unwrap();
            let until = Instant::now() + Duration::from_secs(3);
            let mut fragment = 0;
            let mut step = |h: &mut ResidentHarness| {
                render_takeover_partial(
                    h,
                    [3, 5, 117, 1, 130][fragment % 5],
                    true,
                    "live authoring",
                    None,
                );
                fragment += 1;
                while h
                    .callback
                    .lock()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .native_workers_idle_on_control()
                    != Some(true)
                {
                    assert!(Instant::now() < until);
                    std::thread::yield_now();
                }
                assert!(Instant::now() < until, "live lane authoring timed out");
            };
            let event = loop {
                h.pump();
                if let Ok(event) = h.service.try_event() {
                    break event;
                }
                step(h);
            };
            assert!(event.error.is_none(), "{:?}", event.error);
            while h
                .audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .applied_revision
                .load(Acquire)
                <= applied
            {
                h.pump();
                step(h);
            }
            applied = h.applied(applied).0;
            before = (*event.state.project).clone();
            revision = event.state.revision;
            assert_eq!(
                h.plugin_progress.lock().unwrap().binding,
                Some(binding),
                "new lane must retain the original SDK instrument"
            );
            assert!(
                (peak(h) - baseline * value / 0.25).abs() < 0.0001,
                "live authoring must deliver the new value on the held gate"
            );
        }
    }
    let query = |h: &mut ResidentHarness, request_id| {
        let context = h.submitted.unwrap().1.context;
        h.run(Request::QueryPluginParameters(PluginParameterQuery {
            context,
            device,
            output_identity: output,
            request_id,
            page_first: None,
        }));
        let row = h
            .next()
            .plugin_parameters
            .unwrap()
            .rows
            .unwrap()
            .into_iter()
            .find(|row| row.id == 7)
            .unwrap();
        PluginParameterOrigin {
            context,
            device,
            parameter: 7,
            page_first: row.page_first,
            metadata_revision: row.metadata_revision,
            initial: PluginParameterValue::new(row.normalized).unwrap(),
        }
    };
    let send = |h: &mut ResidentHarness, client: &mut GestureClient, accepted: bool| {
        let request = client.next_request().unwrap();
        client.mark_submitted();
        h.service.submit(Request::DeviceGesture(request)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut fragment = 0usize;
        loop {
            h.pump();
            if let Ok(event) = h.service.try_event()
                && let Some(ack) = event.gesture_ack.as_ref()
                && ack.key == request.key()
                && ack.phase == request.phase()
            {
                let status = h.audio.as_ref().unwrap().status().unwrap();
                assert_eq!(
                    ack.accepted,
                    accepted,
                    "{case}: {ack:?} {:?}; playing={} source={:?} fault={:?}",
                    event.error,
                    status.playing.load(Acquire),
                    status.native_parameter_source(playing),
                    status.render_fault_snapshot()
                );
                assert!(!ack.source_lost || stop_recording, "{case}: {ack:?}");
                assert!(client.acknowledge_with_snapshot(ack, event.active_device_gesture));
                return event;
            }
            assert!(
                Instant::now() < deadline,
                "instrument gesture timed out: {request:?}"
            );
            render_takeover_partial(
                h,
                [3, 5, 117, 1, 130][fragment % 5],
                playing
                    && case != "delivery_retire"
                    && !stop_recording
                    && !case.starts_with("touch_close_"),
                &case,
                Some(&request),
            );
            fragment += 1;
            // The native worker is asynchronous; wait only on this test/control
            // thread before offering the next irregular physical fragment.
            while h
                .callback
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .native_workers_idle_on_control()
                != Some(true)
            {
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
            std::thread::yield_now();
        }
    };
    let mut client = GestureClient::default();
    if case.starts_with("session_") {
        let origin = query(h, 12);
        let binding = h.plugin_progress.lock().unwrap().binding;
        client.begin_plugin_recording(origin).unwrap();
        assert_eq!(send(h, &mut client, true).state.project.as_ref(), &before);
        #[cfg(feature = "recording-fault-test-support")]
        if matches!(
            case.as_str(),
            "session_writer_failure"
                | "session_writer_adopt"
                | "session_writer_keep_apply"
                | "session_writer_refusal"
                | "session_writer_source_lost"
        ) {
            use hex_session::service::automation_draft::{DraftProvenance, Vst3CaptureTarget};
            use hex_session::service::{
                DeviceAutomationTakeAction as Action, DeviceAutomationTakeProgress as Progress,
                DeviceAutomationTakeScope, DeviceCaptureTarget,
            };
            let status = h.audio.as_ref().unwrap().status().unwrap();
            let applied_source = status.recording_source().unwrap();
            let scope = DeviceAutomationTakeScope {
                target: DeviceCaptureTarget::Vst3(Vst3CaptureTarget {
                    device,
                    parameter: 7,
                    class,
                    binding: binding.unwrap(),
                }),
                stamp: DraftProvenance {
                    sample_rate: 48_000,
                    output_identity: applied_source.output_identity,
                    project_generation: origin.context.project_generation,
                    editor_revision: origin.context.editor_revision,
                    audio_revision: applied_source.audio_revision,
                    transport_epoch: status.transport_epoch.load(Acquire),
                    gesture_serial: client.snapshot().unwrap().key.token,
                },
            };
            let armed = h.run(Request::DeviceAutomationTake {
                request_id: 491,
                scope,
                action: Action::InjectWriterFailure,
            });
            assert!(armed.error.is_none(), "{:?}", armed.error);
            assert_eq!(
                armed.device_automation_take_complete.unwrap().progress,
                Progress::Appended
            );
            client
                .change_plugin(PluginParameterValue::new(0.75).unwrap())
                .unwrap();
            send(h, &mut client, true);
            client.finish_plugin(origin.initial).unwrap();
            let failed = send(h, &mut client, false);
            assert_eq!(failed.state.project.as_ref(), &before);
            let directory = recovery_directory.unwrap().join("automation-drafts");
            let paths = std::fs::read_dir(&directory)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect::<Vec<_>>();
            assert_eq!(paths.len(), 1);
            let original = std::fs::read(&paths[0]).unwrap();
            let copy = paths[0].with_extension("recovery-copy");
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                let started = super::automation::edit_with_running_callback(
                    h,
                    Request::DeviceAutomationTake {
                        request_id: 492,
                        scope,
                        action: Action::CopyFailedPlugin {
                            destination: copy.clone(),
                            max_events: 16,
                        },
                    },
                );
                if let Some(reason) = started.error {
                    assert!(
                        reason.contains("no failed device automation custody")
                            || reason.contains("vst3 recovery custody is incomplete")
                            || reason.contains("vst3 recovery requires a quiescent gesture"),
                        "{reason}"
                    );
                    assert!(
                        Instant::now() < deadline,
                        "writer custody was not handed off: {reason}"
                    );
                    continue;
                }
                assert_eq!(
                    started.device_automation_take_complete.unwrap().progress,
                    Progress::Pending
                );
                break;
            }
            let receipt = loop {
                let event = super::automation::edit_with_running_callback(
                    h,
                    Request::DeviceAutomationTake {
                        request_id: 493,
                        scope,
                        action: Action::PollFailedCopy,
                    },
                );
                assert!(event.error.is_none(), "{:?}", event.error);
                match event.device_automation_take_complete.unwrap().progress {
                    Progress::Pending => assert!(Instant::now() < deadline),
                    Progress::VerifiedCopy { path, receipt } => {
                        assert_eq!(path, copy);
                        use hex_session::service::PluginParameterObservation as Observation;
                        use hex_storage::automation_draft::DraftTerminal;
                        let originals = crate::native_audio::tests::resident_preview::SESSION_RECORDING_OBSERVATIONS.lock().unwrap().clone();
                        let points = originals
                            .iter()
                            .filter(|event| matches!(event, Observation::Delivered(_)))
                            .count();
                        assert!(points > 0);
                        assert_eq!(receipt.events(), points as u64);
                        let restored = originals.iter().find_map(|event| match event {
                            Observation::Restored(delivery) => Some(delivery.application.origin),
                            _ => None,
                        });
                        assert_eq!(
                            receipt.terminal(),
                            restored.map_or(DraftTerminal::Interrupted, DraftTerminal::Finished)
                        );
                        break receipt;
                    }
                    other => panic!("unexpected recovery copy: {other:?}"),
                }
            };
            assert_eq!(std::fs::read(&paths[0]).unwrap(), original);
            assert_eq!(h.service.try_event().ok().and_then(|e| e.error), None);
            let mut discovered = super::automation::edit_with_running_callback(
                h,
                Request::QueryFailedPluginAutomation { request_id: 493 },
            );
            assert!(discovered.error.is_none(), "{:?}", discovered.error);
            let discovery = discovered.device_automation_take_complete.take().unwrap();
            assert_eq!(discovery.request_id, 493);
            assert_eq!(
                discovery.progress,
                Progress::FailedPluginCustody {
                    scope: Box::new(scope),
                    path: std::fs::canonicalize(&paths[0]).unwrap(),
                }
            );
            if case != "session_writer_failure" {
                use hex_storage::automation_draft::DraftTerminal;
                assert!(
                    matches!(receipt.terminal(), DraftTerminal::Finished(_)),
                    "actual restoration is required for adoption: {:?}",
                    receipt.terminal()
                );
                let copy_bytes = std::fs::read(&copy).unwrap();
                let keep_then_apply = case == "session_writer_keep_apply";
                if keep_then_apply {
                    let kept = super::automation::edit_with_running_callback(
                        h,
                        Request::DeviceAutomationTake {
                            request_id: 494,
                            scope,
                            action: Action::KeepFailedPluginCopy {
                                path: copy.clone(),
                                receipt,
                                max_events: 16,
                            },
                        },
                    );
                    assert!(kept.error.is_none(), "{:?}", kept.error);
                    assert_eq!(
                        kept.device_automation_take_complete.unwrap().progress,
                        Progress::CopyKept(copy.clone())
                    );
                    assert_eq!(kept.state.project.as_ref(), &before);
                }
                let adopt = |request_id| {
                    if keep_then_apply {
                        Request::RecoverDeviceAutomationTake {
                            request_id,
                            context: origin.context,
                            target: scope.target.draft_target(),
                            expected: scope.stamp,
                            path: copy.clone(),
                            identity: hex_session::service::AutomationRecoveryFileIdentity::Copy(
                                receipt,
                            ),
                        }
                    } else {
                        Request::DeviceAutomationTake {
                            request_id,
                            scope,
                            action: Action::AdoptFailedPluginCopy { max_events: 16 },
                        }
                    }
                };
                if case == "session_writer_source_lost" {
                    h.audio.as_ref().unwrap().control.stop();
                    h.render_plugin_block();
                    let refused = super::automation::edit_with_running_callback(h, adopt(494));
                    assert!(refused.error.is_some());
                    assert_eq!(refused.state.project.as_ref(), &before);
                    assert_eq!(std::fs::read(&paths[0]).unwrap(), original);
                    assert_eq!(std::fs::read(&copy).unwrap(), copy_bytes);
                    let inspected = super::automation::edit_with_running_callback(
                        h,
                        Request::DeviceAutomationTake {
                            request_id: 495,
                            scope,
                            action: Action::InspectFailedPluginCopy,
                        },
                    );
                    assert!(
                        matches!(inspected.device_automation_take_complete.unwrap().progress, Progress::VerifiedCopy { receipt: found, .. } if found == receipt)
                    );
                    return;
                }
                if case == "session_writer_refusal" {
                    h.refuse.store(true, std::sync::atomic::Ordering::Release);
                    let refused = super::automation::edit_with_running_callback(h, adopt(494));
                    h.refuse.store(false, std::sync::atomic::Ordering::Release);
                    assert!(refused.error.is_some());
                    assert_eq!(refused.state.project.as_ref(), &before);
                    assert_eq!(std::fs::read(&paths[0]).unwrap(), original);
                    let inspected = super::automation::edit_with_running_callback(
                        h,
                        Request::DeviceAutomationTake {
                            request_id: 495,
                            scope,
                            action: Action::InspectFailedPluginCopy,
                        },
                    );
                    assert!(
                        matches!(inspected.device_automation_take_complete.unwrap().progress, Progress::VerifiedCopy { receipt: found, .. } if found == receipt)
                    );
                }
                let committed = super::automation::edit_with_running_callback(h, adopt(496));
                assert!(committed.error.is_none(), "{:?}", committed.error);
                assert_eq!(
                    committed.device_automation_take_complete.unwrap().progress,
                    Progress::Committed
                );
                assert_ne!(committed.state.project.as_ref(), &before);
                let after = committed.state.project.clone();
                assert_eq!(std::fs::read(&paths[0]).unwrap(), original);
                assert_eq!(std::fs::read(&copy).unwrap(), copy_bytes);
                let mut cleared = super::automation::edit_with_running_callback(
                    h,
                    Request::QueryFailedPluginAutomation { request_id: 498 },
                );
                assert!(cleared.error.is_none(), "{:?}", cleared.error);
                let cleared = cleared.device_automation_take_complete.take().unwrap();
                assert_eq!(cleared.request_id, 498);
                assert_eq!(cleared.progress, Progress::FailedPluginUnavailable);
                let duplicate = super::automation::edit_with_running_callback(h, adopt(497));
                assert!(duplicate.error.is_some());
                assert_eq!(duplicate.state.project, after);
                let undone = super::automation::edit_with_running_callback(h, Request::Undo);
                assert!(undone.error.is_none(), "{:?}", undone.error);
                assert_eq!(undone.state.project.as_ref(), &before);
                let redone = super::automation::edit_with_running_callback(h, Request::Redo);
                assert!(redone.error.is_none(), "{:?}", redone.error);
                assert_eq!(redone.state.project, after);
                if keep_then_apply {
                    let path = directory.join("applied.hex");
                    let saved = super::automation::edit_with_running_callback(
                        h,
                        Request::SaveNew(path.clone()),
                    );
                    assert!(saved.error.is_none(), "{:?}", saved.error);
                    assert_eq!(hex_storage::load(path).unwrap(), *after);
                }
            }
            return;
        }
        #[cfg(not(feature = "recording-fault-test-support"))]
        assert!(
            !case.starts_with("session_writer_"),
            "fixture feature is required"
        );
        if case == "session_empty" {
            client.finish_plugin(origin.initial).unwrap();
            let empty = send(h, &mut client, true);
            assert_eq!(empty.state.project.as_ref(), &before);
            return;
        }
        for value in [0.75, 0.375] {
            client
                .change_plugin(PluginParameterValue::new(value).unwrap())
                .unwrap();
            assert_eq!(send(h, &mut client, true).state.project.as_ref(), &before);
            assert!((peak(h) - baseline * value as f32 / 0.25).abs() < 0.0001);
        }
        if stop_recording {
            let before_stop = h
                .audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .applied_revision
                .load(Acquire);
            h.audio.as_ref().unwrap().control.stop();
            h.render_plugin_block();
            client.cancel().unwrap();
            let lost = send(h, &mut client, false);
            assert!(lost.gesture_ack.unwrap().source_lost);
            assert_eq!(lost.state.project.as_ref(), &before);
            let mut restored = h.run(Request::RestoreStoppedPlayback);
            if !restored.stopped_playback_restore_complete {
                restored = restored_terminal(h);
            }
            assert!(restored.error.is_none(), "{:?}", restored.error);
            h.applied(before_stop);
            wait(|| {
                h.pump();
                h.audio
                    .as_mut()
                    .unwrap()
                    .control
                    .command(PlaybackCommand::Play)
                    .is_ok()
            });
            assert!((peak(h) - baseline).abs() < 0.0001);
            client.begin_plugin_recording(query(h, 24)).unwrap();
            send(h, &mut client, true);
            client.cancel().unwrap();
            send(h, &mut client, true);
            return;
        }
        if matches!(
            case.as_str(),
            "session_barrier_undo" | "session_barrier_export"
        ) {
            let files = Files::new();
            let path = files.0.join("recorded.wav");
            h.service
                .submit(if case == "session_barrier_undo" {
                    Request::Undo
                } else {
                    Request::ExportRange {
                        path: path.clone(),
                        start: 0,
                        frames: Some(512),
                        signal: hex_session::service::ExportSignal::Main,
                        control: hex_session::service::ExportControl::default(),
                        format: hex_session::service::ExportFormat::Pcm16,
                        overwrite: false,
                    }
                })
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut recorded = None;
            let completed = loop {
                h.pump();
                h.render_worker_step();
                if let Ok(event) = h.service.try_event() {
                    assert!(event.error.is_none(), "{case}: {:?}", event.error);
                    if let Some(ack) = event.gesture_ack.as_ref() {
                        assert!(ack.accepted && !ack.preview_retained && !ack.source_lost);
                        assert!(recorded.replace(event.state.project.clone()).is_none());
                        assert_ne!(event.state.project.as_ref(), &before);
                    } else if event.request_complete {
                        assert!(recorded.is_some(), "take must precede original action");
                        break event;
                    }
                }
                assert!(Instant::now() < deadline, "{case}: terminal timed out");
                std::thread::yield_now();
            };
            if case == "session_barrier_undo" {
                assert_eq!(completed.state.project.as_ref(), &before);
                let redone = super::automation::edit_with_running_callback(h, Request::Redo);
                assert!(redone.error.is_none(), "{:?}", redone.error);
                assert_eq!(redone.state.project, recorded.unwrap());
            } else {
                assert_eq!(completed.state.project, recorded.unwrap());
                loop {
                    h.pump();
                    h.render_worker_step();
                    if let Ok(export) = h.service.try_export_event() {
                        assert_eq!(export.revision, completed.state.revision);
                        // Offline foreign automation has a separate Engine/SDK
                        // owner (#2955). This barrier must preserve its explicit
                        // refusal at the exact committed revision, never export
                        // the old curve or publish a misleading file.
                        assert!(
                            export.result.unwrap_err().message().contains(
                                "vst3 automation requires interactive arrangement playback"
                            )
                        );
                        assert!(!path.exists());
                        break;
                    }
                    assert!(Instant::now() < deadline, "export timed out");
                    std::thread::yield_now();
                }
            }
            return;
        }
        if matches!(
            case.as_str(),
            "session_barrier_refused" | "session_barrier_stop"
        ) {
            let files = Files::new();
            let path = files.0.join("must-not-save.hex");
            let correlation = hex_session::service::SaveCorrelation {
                request_id: 33,
                project_generation: origin.context.project_generation,
                revision: origin.context.editor_revision,
            };
            if case == "session_barrier_refused" {
                h.refuse.store(true, std::sync::atomic::Ordering::Release);
            }
            h.service
                .submit_correlated_save(Request::SaveNew(path.clone()), correlation)
                .unwrap();
            if case == "session_barrier_stop" {
                h.audio.as_ref().unwrap().control.stop();
            }
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut terminal = false;
            loop {
                h.pump();
                h.render_worker_step();
                if let Ok(event) = h.service.try_event() {
                    if let Some(ack) = event.gesture_ack.as_ref() {
                        assert!(!terminal);
                        terminal = true;
                        assert!(!ack.accepted && !ack.preview_retained, "{case}: {ack:?}");
                    }
                    if event.save_complete {
                        assert!(terminal && event.error.is_some());
                        assert_eq!(event.save_correlation, Some(correlation));
                        assert_eq!(event.state.project.as_ref(), &before);
                        assert!(!path.exists());
                        break;
                    }
                }
                assert!(Instant::now() < deadline, "{case}: refusal timed out");
                std::thread::yield_now();
            }
            h.refuse.store(false, std::sync::atomic::Ordering::Release);
            return;
        }
        if matches!(case.as_str(), "session_barriers" | "session_save") {
            let files = Files::new();
            let path = files.0.join("after-recording.hex");
            let correlation = hex_session::service::SaveCorrelation {
                request_id: 31,
                project_generation: origin.context.project_generation,
                revision: origin.context.editor_revision,
            };
            h.service
                .submit_correlated_save(Request::SaveNew(path.clone()), correlation)
                .unwrap();
            // Later work stays in the existing FIFO until the original Save has
            // finished; it must neither overtake nor cause a second take.
            h.service.submit(Request::Undo).unwrap();
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut terminal = None;
            let saved = loop {
                h.pump();
                h.render_worker_step();
                if let Ok(event) = h.service.try_event() {
                    assert!(event.error.is_none(), "{case}: {:?}", event.error);
                    if let Some(ack) = event.gesture_ack.as_ref() {
                        assert!(
                            terminal.replace(ack.clone()).is_none(),
                            "one gesture terminal"
                        );
                        assert!(ack.accepted && !ack.preview_retained && !ack.source_lost);
                        assert_eq!(ack.key, client.snapshot().unwrap().key);
                        assert!(event.active_device_gesture.is_none());
                    }
                    if event.save_complete {
                        assert_eq!(event.save_correlation, Some(correlation));
                        assert!(terminal.is_some(), "gesture must finish before save");
                        break event;
                    }
                }
                assert!(Instant::now() < deadline, "recording save timed out");
                std::thread::yield_now();
            };
            assert_ne!(saved.state.project.as_ref(), &before);
            assert_eq!(hex_storage::load(path).unwrap(), *saved.state.project);
            let target = DeviceAutomationTarget::Vst3 { parameter: 7 };
            let recorded = saved
                .state
                .project
                .device(device)
                .unwrap()
                .automation_target(target)
                .unwrap();
            assert_eq!(recorded.touch.len(), 1);
            let undone = loop {
                h.pump();
                h.render_worker_step();
                if let Ok(event) = h.service.try_event() {
                    assert!(event.gesture_ack.is_none(), "one gesture terminal");
                    if event.request_complete {
                        break event;
                    }
                }
                assert!(Instant::now() < deadline, "queued Undo timed out");
                std::thread::yield_now();
            };
            assert!(undone.error.is_none(), "{:?}", undone.error);
            assert_eq!(undone.state.project.as_ref(), &before);
            return;
        }
        if case == "session_cancel" {
            client.cancel().unwrap();
            let cancelled = send(h, &mut client, true);
            assert_eq!(cancelled.state.project.as_ref(), &before);
            assert!((peak(h) - baseline).abs() < 0.0001);
            return;
        }
        let final_value = if case == "session_record_changed" || first_lane {
            PluginParameterValue::new(0.75).unwrap()
        } else {
            origin.initial
        };
        client.finish_plugin(final_value).unwrap();
        let committed = if matches!(case.as_str(), "session_refused" | "session_recover") {
            h.refuse.store(true, std::sync::atomic::Ordering::Release);
            let refused = send(h, &mut client, false);
            h.refuse.store(false, std::sync::atomic::Ordering::Release);
            assert_eq!(refused.state.project.as_ref(), &before);
            assert!(!refused.gesture_ack.unwrap().preview_retained);
            assert!((peak(h) - baseline).abs() < 0.0001);
            if case == "session_refused" {
                client.begin_plugin_recording(query(h, 13)).unwrap();
                send(h, &mut client, true);
                client.cancel().unwrap();
                send(h, &mut client, true);
                return;
            }
            use hex_session::service::DeviceAutomationTakeProgress;
            let directory = recovery_directory.unwrap().join("automation-drafts");
            let paths = std::fs::read_dir(directory)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect::<Vec<_>>();
            assert_eq!(paths.len(), 1);
            let path = paths[0].clone();
            let original = std::fs::read(&path).unwrap();
            let inspected = super::automation::edit_with_running_callback(
                h,
                Request::InspectSealedAutomationRecovery {
                    request_id: 400,
                    path: path.clone(),
                },
            );
            assert!(inspected.error.is_none(), "{:?}", inspected.error);
            assert_eq!(inspected.state.project.as_ref(), &before);
            assert_eq!(inspected.state.revision, origin.context.editor_revision);
            let DeviceAutomationTakeProgress::SealedFile(selected) =
                inspected.device_automation_take_complete.unwrap().progress
            else {
                panic!("publication refusal did not leave a selectable finished take");
            };
            assert_eq!(selected.path, path);
            let request = |request_id| Request::RecoverDeviceAutomationTake {
                request_id,
                target: selected.target,
                expected: selected.provenance,
                path: selected.path.clone(),
                identity: hex_session::service::AutomationRecoveryFileIdentity::Sealed(
                    selected.identity,
                ),
                context: origin.context,
            };
            h.service.submit(request(401)).unwrap();
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut fragment = 0usize;
            let recovered = loop {
                h.pump();
                if let Ok(event) = h.service.try_event()
                    && event
                        .device_automation_take_complete
                        .as_ref()
                        .is_some_and(|done| done.request_id == 401)
                {
                    break event;
                }
                // Probe the actual publication interval, including its first
                // callback, while retaining the original held instrument gate.
                render_takeover_partial(h, [3, 5, 117, 1, 130][fragment % 5], true, &case, None);
                fragment += 1;
                while h
                    .callback
                    .lock()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .native_workers_idle_on_control()
                    != Some(true)
                {
                    assert!(Instant::now() < deadline, "recovery DSP prefix timed out");
                    std::thread::yield_now();
                }
                assert!(Instant::now() < deadline, "recovery publication timed out");
                std::thread::yield_now();
            };
            assert!(recovered.error.is_none(), "{:?}", recovered.error);
            assert_eq!(
                recovered
                    .device_automation_take_complete
                    .as_ref()
                    .unwrap()
                    .progress,
                DeviceAutomationTakeProgress::Committed
            );
            let duplicate = super::automation::edit_with_running_callback(h, request(402));
            assert!(duplicate.error.is_some());
            assert_eq!(duplicate.state.project, recovered.state.project);
            assert_eq!(std::fs::read(path).unwrap(), original);
            recovered
        } else {
            send(h, &mut client, true)
        };
        assert_ne!(committed.state.project.as_ref(), &before);
        applied = h.applied(applied).0;
        assert_eq!(h.plugin_progress.lock().unwrap().binding, binding);
        let result = committed.state.project.device(device).unwrap();
        let prior = before.device(device).unwrap();
        assert_eq!(
            result.state(),
            prior.state(),
            "recording must preserve the canonical plugin base"
        );
        let target = DeviceAutomationTarget::Vst3 { parameter: 7 };
        let lane = result.automation_target(target).unwrap();
        assert!(lane.is_reading());
        if first_lane {
            assert!(prior.automation_target(target).is_none());
            assert!(
                lane.points.is_empty(),
                "a first take must not invent a base knot"
            );
        } else {
            assert_eq!(lane.points, prior.automation_target(target).unwrap().points);
        }
        assert_eq!(lane.vst3_class, Some(class));
        assert_eq!(lane.touch.len(), 1);
        assert_eq!(
            lane.touch[0]
                .steps
                .iter()
                .map(|step| step.value.get())
                .collect::<Vec<_>>(),
            [0.75, 0.375, final_value.normalized() as f32]
        );
        assert_eq!(
            result.automation_target(DeviceAutomationTarget::Vst3 { parameter: 11 }),
            prior.automation_target(DeviceAutomationTarget::Vst3 { parameter: 11 })
        );
        assert!(
            (peak(h) - baseline).abs() < 0.0001,
            "release resumes the prior value with the held MIDI gate"
        );
        let undone = super::automation::edit_with_running_callback(h, Request::Undo);
        assert!(undone.error.is_none(), "{:?}", undone.error);
        assert_eq!(undone.state.project.as_ref(), &before);
        applied = h.applied(applied).0;
        assert!((peak(h) - baseline).abs() < 0.0001);
        let redone = super::automation::edit_with_running_callback(h, Request::Redo);
        assert!(redone.error.is_none(), "{:?}", redone.error);
        assert_eq!(redone.state.project, committed.state.project);
        h.applied(applied);
        assert!((peak(h) - baseline).abs() < 0.0001);
        let files = Files::new();
        let path = files.0.join("recorded-instrument.hex");
        let saved = h.run(Request::SaveNew(path.clone()));
        assert!(
            saved.save_complete && saved.error.is_none(),
            "{:?}",
            saved.error
        );
        assert_eq!(hex_storage::load(&path).unwrap(), *saved.state.project);
        // Build an ordinary Step programme from the original SDK delivery
        // observations, independently of the draft/adoption/touch evaluator.
        // Reopening both projects exercises actual new SDK instances, their
        // MIDI scheduling and foreign PCM path with identical transport input.
        use hex_session::service::{
            PluginParameterObservation as Observation, automation_draft::Vst3ApplicationValue,
        };
        let originals =
            crate::native_audio::tests::resident_preview::SESSION_RECORDING_OBSERVATIONS
                .lock()
                .unwrap()
                .clone();
        let mut values =
            std::collections::BTreeMap::from([(BeatTime::ZERO, origin.initial.normalized())]);
        let mut end_frame = None;
        for observation in originals {
            let (point, value) = match observation {
                Observation::Delivered(point) => {
                    let Vst3ApplicationValue::Value(value) = point.application.value else {
                        panic!("point value");
                    };
                    (point, value)
                }
                Observation::Restored(point) => {
                    assert!(
                        end_frame
                            .replace(point.application.origin.content_frame)
                            .is_none()
                    );
                    (point, origin.initial.normalized())
                }
                Observation::Ready { .. } => continue,
                _ => panic!("normal recording must not abort"),
            };
            values.insert(
                before
                    .tempo_map()
                    .frame_to_beat(point.application.origin.content_frame)
                    .unwrap(),
                value,
            );
        }
        let mut reference = hex_project::Editor::new(before.clone(), 8).unwrap();
        reference
            .execute(Command::ReplaceDeviceAutomationTarget {
                device,
                target,
                points: values
                    .into_iter()
                    .map(|(beat, value)| AutomationPoint {
                        beat,
                        value: AutomationValue::new(value as f32).unwrap(),
                        outgoing: AutomationInterpolation::Step,
                    })
                    .collect(),
            })
            .unwrap();
        let reference_path = files.0.join("independent-delivered-steps.hex");
        hex_storage::save_new(&reference_path, reference.project()).unwrap();
        let end_frame = end_frame.expect("a committed take has an actual restored output witness");
        let blocks = usize::try_from(end_frame / 256 + 8).unwrap();
        assert!(blocks < 512, "bounded signed replay fixture");
        let expected = replay_reopened_instrument(h, &reference_path, device, blocks);
        let actual = replay_reopened_instrument(h, &path, device, blocks);
        assert_eq!(expected.len(), actual.len());
        let error = expected
            .iter()
            .flatten()
            .zip(actual.iter().flatten())
            .map(|(left, right)| (left - right).abs())
            .fold(0.0, f32::max);
        let peak = actual
            .iter()
            .flatten()
            .copied()
            .map(f32::abs)
            .fold(0.0, f32::max);
        assert!(
            peak > baseline * 2.9,
            "reopened touch must actually sound above its base: {peak}"
        );
        assert!(
            error < 1.0e-6,
            "reopened recorded PCM differs from original delivered Step oracle: {error}"
        );
        return;
    }
    let touches = if case == "touch_stress" { 100 } else { 2 };
    for request_id in 12..12 + touches {
        client.begin_plugin(query(h, request_id)).unwrap();
        assert_eq!(send(h, &mut client, true).state.project.as_ref(), &before);
        if case == "touch_close_begin" {
            return;
        }
        if case == "touch_empty" {
            client.cancel().unwrap();
            send(h, &mut client, true);
            return;
        }
        client
            .change_plugin(PluginParameterValue::new(0.75).unwrap())
            .unwrap();
        assert_eq!(
            send(
                h,
                &mut client,
                !matches!(
                    case.as_str(),
                    "delivery_retire" | "touch_close_pending" | "touch_close_queued"
                )
            )
            .state
            .project
            .as_ref(),
            &before
        );
        if matches!(
            case.as_str(),
            "delivery_retire" | "touch_close_point" | "touch_close_pending" | "touch_close_queued"
        ) {
            return;
        }
        if playing {
            assert!(
                (peak(h) - baseline * 3.0).abs() < 0.0001,
                "held MIDI must continue through preview"
            );
        }
        if matches!(case.as_str(), "touch_commit" | "touch_close_restored") {
            client
                .finish_plugin(PluginParameterValue::new(0.75).unwrap())
                .unwrap();
            // This producer-only fixture injects recording below an ordinary
            // Session gesture, without a durable take owner. Opaque Commit must
            // refuse; the opt-in Session regression covers canonical adoption.
            let ended = send(h, &mut client, false);
            assert_eq!(ended.state.project.as_ref(), &before);
            if case == "touch_commit" {
                assert!((peak(h) - baseline).abs() < 0.0001);
            }
            return;
        }
        client.cancel().unwrap();
        assert_eq!(send(h, &mut client, true).state.project.as_ref(), &before);
        if playing {
            assert!(
                (peak(h) - baseline).abs() < 0.0001,
                "cancel must resume authored input"
            );
        }
    }
    if matches!(
        case.as_str(),
        "touch" | "touch_stress" | "touch_control_lag"
    ) {
        if case == "touch_control_lag" {
            assert_eq!(h.plugin_completed.load(Acquire), 0);
        }
        return;
    }
    client.begin_plugin(query(h, 14)).unwrap();
    send(h, &mut client, true);
    let manual = PluginParameterValue::new(0.5).unwrap();
    client.change_plugin(manual).unwrap();
    send(h, &mut client, true);
    client.finish_plugin(manual).unwrap();
    if case == "refused" {
        h.refuse.store(true, std::sync::atomic::Ordering::Release);
        let refused = send(h, &mut client, false);
        h.refuse.store(false, std::sync::atomic::Ordering::Release);
        assert_eq!(refused.state.project.as_ref(), &before);
        std::thread::sleep(Duration::from_millis(20));
        assert!(
            (peak(h) - baseline * 2.0).abs() < 0.0001,
            "a refused commit must keep the acknowledged preview audible"
        );
        client.cancel().unwrap();
        send(h, &mut client, true);
        assert!((peak(h) - baseline).abs() < 0.0001);
        println!("instrument commit refusal preserved preview until cancel");
        return;
    }
    let committed = send(h, &mut client, true);
    applied = h.applied(applied).0;
    let result = committed.state.project.device(device).unwrap();
    let target = DeviceAutomationTarget::Vst3 { parameter: 7 };
    let other = DeviceAutomationTarget::Vst3 { parameter: 11 };
    if reading {
        assert!(!result.automation_target(target).unwrap().is_reading());
    } else {
        assert!(result.automation_target(target).is_none());
    }
    assert_eq!(
        result.automation_target(other),
        before.device(device).unwrap().automation_target(other)
    );
    let decoded = hex_plugin_state::decode(result.state()).unwrap();
    let component = decoded.component.unwrap();
    assert_eq!(
        f32::from_le_bytes(component[8..12].try_into().unwrap()),
        2.0
    );
    assert_eq!(
        f64::from_le_bytes(component[32..40].try_into().unwrap()),
        1.0,
        "the other authored DSP value must not replace its canonical base"
    );
    if idle_unlaned {
        assert!(
            !h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .playing
                .load(Acquire)
        );
        let source = h.submitted.unwrap().1.context;
        let mut serial = 17;
        let after = super::installed_edits::query(h, device, Some(0), &mut serial);
        assert_eq!(after.query.context, source);
        assert_eq!(
            after
                .rows
                .unwrap()
                .into_iter()
                .find(|row| row.id == 7)
                .unwrap()
                .normalized,
            0.5
        );
        assert!(original_binding.is_some());
        assert_ne!(
            after.source, original_binding,
            "fixture must replace the worker"
        );
        assert!(after.source.is_some());
        let undone = super::automation::edit_with_running_callback(h, Request::Undo);
        assert!(undone.error.is_none(), "{:?}", undone.error);
        assert_eq!(undone.state.project.as_ref(), &before);
        h.applied(applied);
        return;
    }
    if playing {
        assert!(
            (peak(h) - baseline * 2.0).abs() < 0.0001,
            "commit must preserve the held MIDI gate"
        );
    }
    let undone = super::automation::edit_with_running_callback(h, Request::Undo);
    assert!(undone.error.is_none(), "{:?}", undone.error);
    assert_eq!(undone.state.project.as_ref(), &before);
    applied = h.applied(applied).0;
    if playing {
        let restored = peak(h);
        assert!(
            (restored - baseline).abs() < 0.0001,
            "undo must restore audible authored input: case={case} actual={restored} expected={baseline} phase={:?} source={:?} fault={:?}",
            h.callback
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .native_worker_phase_for_test(),
            h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .recording_source(),
            h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .render_fault_snapshot(),
        );
    }
    let redone = super::automation::edit_with_running_callback(h, Request::Redo);
    if redone.error.is_some() {
        let diagnostic = h.audio.as_mut().unwrap().control.take_audio_diagnostic();
        let status = h.audio.as_ref().unwrap().status().unwrap();
        panic!(
            "redo: {:?}; source {:?}; render fault {:?}; audio diagnostic {:?}; preparation {:?}",
            redone.error,
            status.native_effect_source(),
            status.render_fault_snapshot(),
            diagnostic,
            *h.plugin_progress.lock().unwrap()
        );
    }
    assert_eq!(redone.state.project, committed.state.project);
    applied = h.applied(applied).0;
    if playing {
        assert!(
            (peak(h) - baseline * 2.0).abs() < 0.0001,
            "redo must restore audible manual base"
        );
    }
    // A later gesture uses the privately approved current canonical state,
    // rather than demanding the original source's frozen constructor bytes.
    client.begin_plugin(query(h, 15)).unwrap();
    assert_eq!(
        send(h, &mut client, true).state.project,
        redone.state.project
    );
    client
        .change_plugin(PluginParameterValue::new(0.75).unwrap())
        .unwrap();
    send(h, &mut client, true);
    client.cancel().unwrap();
    assert_eq!(
        send(h, &mut client, true).state.project,
        redone.state.project
    );
    if playing {
        assert!((peak(h) - baseline * 2.0).abs() < 0.0001);
    }
    let enabled = super::automation::edit_with_running_callback(
        h,
        Request::DeviceAutomationControl {
            project_generation: context.project_generation,
            revision: redone.state.revision,
            edit: DeviceAutomationControlEdit::Reenable,
        },
    );
    assert!(enabled.error.is_none(), "{:?}", enabled.error);
    if reading {
        h.applied(applied);
    } else {
        assert_eq!(enabled.state.project, redone.state.project);
    }
    if playing {
        assert!(
            (peak(h) - baseline * if reading { 1.0 } else { 2.0 }).abs() < 0.0001,
            "re-enable resumes existing lanes and preserves unlaned manual bases"
        );
    }
    let files = Files::new();
    let path = files.0.join("instrument-takeover.hex");
    let saved = h.run(Request::SaveNew(path.clone()));
    assert!(
        saved.save_complete && saved.error.is_none(),
        "{:?}",
        saved.error
    );
    assert_eq!(hex_storage::load(path).unwrap(), *saved.state.project);
    if case == "renew" {
        let before_stop = h
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(Acquire);
        h.audio.as_ref().unwrap().control.stop();
        h.render_plugin_block();
        let mut restored = h.run(Request::RestoreStoppedPlayback);
        if !restored.stopped_playback_restore_complete {
            restored = restored_terminal(h);
        }
        assert!(restored.error.is_none(), "{:?}", restored.error);
        assert_eq!(restored.state.project, saved.state.project);
        h.applied(before_stop);
        wait(|| h.plugin_completed.load(Acquire) == 0);
        wait(|| {
            h.pump();
            h.audio
                .as_mut()
                .unwrap()
                .control
                .command(PlaybackCommand::Play)
                .is_ok()
        });
        assert!(
            (peak(h) - baseline * 2.0).abs() < 0.0001,
            "renewal must retain the manual canonical base"
        );
        client.begin_plugin(query(h, 16)).unwrap();
        send(h, &mut client, true);
        client
            .change_plugin(PluginParameterValue::new(0.75).unwrap())
            .unwrap();
        send(h, &mut client, true);
        assert!((peak(h) - baseline * 3.0).abs() < 0.0001);
        client.cancel().unwrap();
        send(h, &mut client, true);
        assert!((peak(h) - baseline * 2.0).abs() < 0.0001);
    }
    println!("instrument takeover {case} passed");
}

#[test]
#[ignore = "requires exact source-bound native helpers and original VST3 fixtures"]
fn native_vst3_instrument_and_streamed_audio_export() {
    const CHILD: &str = "HEX_DESKTOP_INSTRUMENT_EXPORT_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let name = std::thread::current().name().unwrap().to_owned();
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--ignored", "--nocapture"])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "127")
            .env("HEX_ORIGINAL_INSTRUMENT_LIVE_PRESET", "1")
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&child.stdout),
            String::from_utf8_lossy(&child.stderr)
        );
        return;
    }
    use hex_session::service::{ExportControl, ExportSignal};
    let _native = native_fixture();
    let files = Files::new();
    let mut selected = selection();
    let module = files.0.canonicalize().unwrap().join("original.vst3");
    super::ui::copy_original_module(&selected.module, &module);
    selected.module = module;
    hex_ui::write_plugin_effect_test_catalog(&files.0, &selected, "Instrument|Synth", 0).unwrap();
    let recovery = files.0.join("recovery");
    fs::create_dir(&recovery).unwrap();
    let mut h = ResidentHarness::with_plugins_at_rate_and_catalog_recovery(
        host(),
        None,
        48_000,
        8_192,
        files.0.canonicalize().unwrap().join("plugin-catalog.json"),
        recovery.join("instrument-export.hex-journal"),
    );
    assert!(h.next().error.is_none());
    let (applied, _) = h.applied(0);
    let source = files.wav(48_000, 16_384);
    let imported = h.run(Request::ImportToNewTrack {
        path: source.clone(),
        before_track: None,
        start: 0,
    });
    let (applied, _) = h.applied(applied);
    let added = h.run(Request::Edit(Command::AddMidiTrack {
        name: "instrument".into(),
    }));
    let Some(Created::Track(track)) = added.created else {
        panic!("midi track")
    };
    let (applied, _) = h.applied(applied);
    let placed = h.run(Request::Edit(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::ONE,
    }));
    let Some(Created::Clip(clip)) = placed.created else {
        panic!("midi clip")
    };
    let (applied, _) = h.applied(applied);
    let note = h.run(Request::Edit(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::from_ratio(1, 2).unwrap(),
        pitch: 60,
        channel: 0,
        velocity: 100,
    }));
    let (applied, _) = h.applied(applied);
    let mut insertion = request(&note.state, PluginEffectDestination::Instrument(track));
    insertion.selection = selected;
    h.service
        .submit(Request::InsertPluginEffect(insertion))
        .unwrap();
    let inserted = terminal(&mut h);
    assert!(inserted.error.is_none(), "{:?}", inserted.error);
    h.applied(applied);
    assert!(
        imported
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .sources()
            .values()
            .any(|source| matches!(source, hex_session::AudioSourceBinding::VerifiedFile(_)))
    );
    let before = Arc::clone(&inserted.state.project);
    let live = h.plugin_progress.lock().unwrap().binding;
    let path = files.0.join("instrument-export.wav");
    let queued = super::super::resident_sections::run_callback(
        &mut h,
        Request::ExportRange {
            path: path.clone(),
            start: 0,
            frames: Some(1024),
            signal: ExportSignal::Main,
            control: ExportControl::default(),
            format: hex_export::ExportFormat::Pcm16,
            overwrite: false,
        },
    );
    assert!(queued.error.is_none(), "{:?}", queued.error);
    let deadline = Instant::now() + Duration::from_secs(10);
    let completed = loop {
        h.pump();
        h.render();
        if let Ok(event) = h.service.try_export_event() {
            break event;
        }
        assert!(Instant::now() < deadline, "instrument export timed out");
        std::thread::yield_now();
    };
    assert_eq!(completed.result.expect("instrument export").frames, 1024);
    let wav = fs::read(path).unwrap();
    let pcm: Vec<_> = wav[44..]
        .chunks_exact(4)
        .map(|frame| i16::from_le_bytes([frame[0], frame[1]]))
        .collect();
    assert!(
        pcm.iter().any(|sample| i32::from(*sample) > 8194),
        "instrument must add audible PCM"
    );
    assert_eq!(queued.state.project, before);
    assert_eq!(h.plugin_progress.lock().unwrap().binding, live);
    assert_eq!(fs::read(source).unwrap().len(), 44 + 16_384 * 2);
}

#[test]
#[ignore = "requires an explicitly selected installed instrument, catalog and native helper"]
fn installed_instrument_and_effect_export_with_streamed_audio() {
    use hex_session::service::{ExportControl, ExportSignal};
    let _native = native_fixture();
    let bundle = std::env::var("HEX_TEST_VST3_INSTRUMENT_BUNDLE").unwrap();
    let original_catalog =
        PathBuf::from(std::env::var_os("HEX_TEST_VST3_INSTRUMENT_CATALOG").unwrap());
    let instrument = installed_selection(
        &original_catalog,
        &bundle,
        hex_plugin_catalog::PluginRole::Instrument,
    );
    let files = Files::new();
    let root = files.0.canonicalize().unwrap();
    let catalog = root.join("plugin-catalog.json");
    fs::copy(&original_catalog, &catalog).unwrap();
    let mut effect = selection();
    let module = root.join("original-effect.vst3");
    super::ui::copy_original_module(&effect.module, &module);
    effect.module = module;
    hex_ui::write_plugin_effect_test_catalog(&root, &effect, "Fx", 0).unwrap();
    let recovery = root.join("recovery");
    fs::create_dir(&recovery).unwrap();
    let mut h = ResidentHarness::with_plugins_at_rate_and_catalog_recovery(
        host(),
        None,
        48_000,
        8_192,
        catalog,
        recovery.join("mixed-export.hex-journal"),
    );
    assert!(h.next().error.is_none());
    let (applied, _) = h.applied(0);
    let probe = std::env::var_os("HEX_TEST_EXPORT_PROBE").is_some();
    let complete = std::env::var_os("HEX_TEST_EXPORT_COMPLETE").is_some();
    let full_playthrough = std::env::var_os("HEX_TEST_FULL_MIXED_PLAYTHROUGH").is_some();
    let live_playback = std::env::var_os("HEX_TEST_EXPORT_LIVE_PLAYBACK").is_some();
    let verify_live_state =
        full_playthrough || std::env::var_os("HEX_TEST_EXPORT_LIVE_STATE").is_some();
    let live_edit = full_playthrough || std::env::var_os("HEX_TEST_EXPORT_LIVE_EDIT").is_some();
    assert!(
        !live_edit || live_playback,
        "live edit requires mixed playback"
    );
    assert!(
        !full_playthrough || (complete && !probe),
        "full playthrough requires a complete export"
    );
    assert!(!full_playthrough || live_playback);
    // The fixture uses the project default of 120 BPM at 48 kHz.
    const FRAMES_PER_QUARTER: usize = 24_000;
    const PHRASE_QUARTERS: usize = 8;
    const SECOND_NOTE_QUARTER: usize = 4;
    const PHRASE_LENGTH: usize = PHRASE_QUARTERS * FRAMES_PER_QUARTER;
    const SECOND_NOTE: usize = SECOND_NOTE_QUARTER * FRAMES_PER_QUARTER;
    // Leave room for the installed instrument's natural release on each note.
    const PHRASE_WINDOWS: [(usize, usize); 4] = [
        (4096, 8192),
        (81920, 90112),
        (SECOND_NOTE + 4096, SECOND_NOTE + 8192),
        (180224, 188416),
    ];
    let late_phrases: &[(usize, u8)] = if full_playthrough {
        &[(64, 48), (192, 55), (320, 53)]
    } else {
        &[]
    };
    let check_phrase = |context: &str, beat: usize, peaks: [f32; 4]| {
        let [first, gap, second, release] = peaks;
        assert!(
            first > 0.01 && gap < 0.005 && second > 0.01 && release < 0.005,
            "{context} phrase at quarter {beat}: {peaks:?}"
        );
    };
    let source_frames = if probe || complete { 8_000_001 } else { 16_384 };
    let source = files.wav(48_000, source_frames);
    // The original quiet markers catch shifted source pages. Separate rests
    // isolate the later instrument phrases from the otherwise audible stream.
    let markers = [(4_000_000usize, 8192usize), (7_950_000, 8192)];
    if complete {
        let mut marked = fs::read(&source).unwrap();
        for (start, length) in markers {
            marked[44 + start * 2..44 + (start + length) * 2].fill(0);
        }
        for &(beat, _) in late_phrases {
            let start = beat * FRAMES_PER_QUARTER;
            marked[44 + (start - 2048) * 2..44 + (start + PHRASE_LENGTH) * 2].fill(0);
        }
        fs::write(&source, marked).unwrap();
    }
    let source_bytes = fs::read(&source).unwrap();
    let imported = h.run(Request::ImportToNewTrack {
        path: source.clone(),
        before_track: None,
        start: 0,
    });
    let (applied, _) = h.applied(applied);
    let audio_track = imported.state.project.tracks()[0].id();
    let (imported, applied) = if full_playthrough {
        let marker =
            files.constant_wav("insert-witness.wav", 48_000, MIXED_WITNESS_FRAMES, &[0, 0]);
        let mut bytes = fs::read(&marker).unwrap();
        for (index, frame) in bytes[44..].chunks_exact_mut(4).enumerate() {
            frame[..2].copy_from_slice(&mixed_witness_sample(index as u64).to_le_bytes());
        }
        fs::write(&marker, bytes).unwrap();
        let event = h.run(Request::ImportToNewTrack {
            path: marker,
            before_track: None,
            start: 0,
        });
        let mut resident = false;
        let mut file_backed = false;
        for (_, source) in event
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .required_sources()
        {
            match source {
                hex_session::AudioSourceBinding::Resident(_) => resident = true,
                hex_session::AudioSourceBinding::VerifiedFile(_) => file_backed = true,
            }
        }
        assert!(
            resident && file_backed,
            "edit qualification needs both source kinds"
        );
        (event, h.applied(applied).0)
    } else {
        (imported, applied)
    };
    let mut insertion = request(&imported.state, PluginEffectDestination::Track(audio_track));
    insertion.selection = effect;
    h.service
        .submit(Request::InsertPluginEffect(insertion))
        .unwrap();
    let effected = terminal(&mut h);
    assert!(effected.error.is_none(), "{:?}", effected.error);
    let (applied, _) = h.applied(applied);
    let added = h.run(Request::Edit(Command::AddMidiTrack {
        name: "instrument".into(),
    }));
    let Some(Created::Track(track)) = added.created else {
        panic!("midi track")
    };
    let (applied, _) = h.applied(applied);
    let placed = h.run(Request::Edit(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::ONE,
    }));
    let Some(Created::Clip(clip)) = placed.created else {
        panic!("midi clip")
    };
    let (applied, _) = h.applied(applied);
    let mut note = h.run(Request::Edit(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::from_ratio(1, 2).unwrap(),
        pitch: 60,
        channel: 0,
        velocity: 100,
    }));
    let (mut applied, _) = h.applied(applied);
    for &(beat, pitch) in late_phrases {
        let placed = h.run(Request::Edit(Command::PlaceMidiClip {
            track,
            start: BeatTime::from_quarters(beat as u64).unwrap(),
            length: BeatTime::from_quarters(PHRASE_QUARTERS as u64).unwrap(),
        }));
        let Some(Created::Clip(clip)) = placed.created else {
            panic!("late midi clip");
        };
        (applied, _) = h.applied(applied);
        for (quarter, pitch) in [(0, pitch), (SECOND_NOTE_QUARTER as u64, pitch + 7)] {
            note = h.run(Request::Edit(Command::AddMidiNote {
                clip,
                start: BeatTime::from_quarters(quarter).unwrap(),
                duration: BeatTime::from_ratio(1, 2).unwrap(),
                pitch,
                channel: 0,
                velocity: 100,
            }));
            (applied, _) = h.applied(applied);
        }
    }
    let instrument_destination = if full_playthrough {
        // Leave the left channel as an independent streamed-source witness
        // even if the installed synth has a sub-audible residual on the right.
        let balanced = h.run(Request::Edit(Command::Balance {
            track,
            balance: 1.0,
        }));
        (applied, _) = h.applied(applied);
        balanced
    } else {
        note
    };
    let mut insertion = request(
        &instrument_destination.state,
        PluginEffectDestination::Instrument(track),
    );
    insertion.selection = instrument;
    h.service
        .submit(Request::InsertPluginEffect(insertion))
        .unwrap();
    let mut inserted = terminal(&mut h);
    assert!(inserted.error.is_none(), "{:?}", inserted.error);
    let (mut applied, _) = h.applied(applied);
    let before_insert = Arc::clone(&inserted.state.project);
    let mut expected_revision = inserted.state.revision;
    let mut expected_can_undo = inserted.state.can_undo;
    let mut expected_can_redo = inserted.state.can_redo;
    if live_playback {
        assert!(
            complete,
            "live mixed qualification requires the full streamed fixture"
        );
        // The default effect fixture serializes its processing counter in a
        // field it refuses to restore after playback. Variant 37 retains the
        // normal processing law while serializing restorable latency state.
        assert!(
            matches!(
                std::env::var("HEX_ORIGINAL_INSTANCE_VARIANT").as_deref(),
                Ok("37")
            ),
            "live mixed qualification requires the restorable effect fixture"
        );
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
        let mut pcm = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while pcm.len() < 4096 {
            let block = render_completed_block(&h);
            let status = h.audio.as_ref().unwrap().status().unwrap();
            if status.stream_buffering.load(Acquire) {
                assert_eq!(block, [[0.0; 2]; 256]);
                assert!(Instant::now() < deadline, "mixed stream did not start");
            } else {
                pcm.extend(block);
            }
        }
        assert!(pcm.iter().flatten().any(|sample| sample.abs() > 0.001));
        assert!(
            pcm.iter().enumerate().skip(512).any(|(index, frame)| {
                let dry = 0.25 * (0.5 + ((index - 512) / 256) as f32 * 0.125);
                (frame[0] - dry).abs() > 1e-4 || (frame[1] - dry).abs() > 1e-4
            }),
            "live mix did not differ from the controlled effect alone"
        );
        if full_playthrough {
            inserted = exercise_mixed_native_insert(&mut h, &inserted.state);
            (applied, _) = h.applied(applied);
            expected_revision = inserted.state.revision;
            expected_can_undo = inserted.state.can_undo;
            expected_can_redo = inserted.state.can_redo;
        }
        if full_playthrough {
            let callback_period = std::env::var("HEX_TEST_FULL_MIXED_CALLBACK_PERIOD_US")
                .ok()
                .map(|value| Duration::from_micros(value.parse().expect("callback period in us")))
                .unwrap_or_default();
            let status = h.audio.as_ref().unwrap().status().unwrap();
            let initial_starvation = status.stream_starved_callbacks.load(Acquire);
            let started = Instant::now();
            let mut quiet_markers = [0usize; 2];
            let mut phrase_peaks = [[0.0_f32; 4]; 3];
            let mut audible_blocks = 0usize;
            let mut buffering_blocks = 0usize;
            let mut consecutive_holds = 0usize;
            let mut longest_hold = 0usize;
            let mut hold_started = None;
            let mut total_hold = Duration::ZERO;
            let mut longest_hold_time = Duration::ZERO;
            let mut held_positions = 0usize;
            let mut page_boundaries = 0usize;
            let mut readiness_refusals = 0usize;
            let mut previous = status.position.load(Acquire);
            while previous < u64::from(source_frames) {
                let callback_started = Instant::now();
                let block = h.render_plugin_block();
                // This phase has no edit/retirement acknowledgement in flight.
                // Wait for every native DSP prefix, while allowing the existing
                // control owner to poll independently rather than gating each block.
                let deadline = Instant::now() + Duration::from_millis(800);
                while h
                    .callback
                    .lock()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .native_workers_idle_on_control()
                    != Some(true)
                {
                    assert!(Instant::now() < deadline);
                    std::thread::yield_now();
                }
                assert!(status.playing.load(Acquire), "mixed playback stopped early");
                assert!(
                    status.render_fault_snapshot().is_none(),
                    "full-song render fault"
                );
                assert!(
                    status.rejected_action().is_none(),
                    "full-song action rejected"
                );
                let position = status.position.load(Acquire);
                assert!(position >= previous, "full-song cursor moved backwards");
                assert!(block.iter().flatten().all(|sample| sample.is_finite()));
                if status.stream_buffering.load(Acquire) {
                    buffering_blocks += 1;
                    if hold_started.is_none() {
                        hold_started = Some(callback_started);
                        held_positions += 1;
                        page_boundaries += usize::from(
                            position.is_multiple_of(hex_engine::STREAM_PAGE_FRAMES as u64),
                        );
                    }
                    readiness_refusals +=
                        usize::from(status.stream_readiness_refused.load(Acquire));
                    consecutive_holds += 1;
                    longest_hold = longest_hold.max(consecutive_holds);
                    assert_eq!(position, previous, "buffering advanced the cursor");
                    assert_eq!(block, [[0.0; 2]; 256]);
                } else if position > previous {
                    if let Some(hold) = hold_started.take() {
                        let elapsed = callback_started.duration_since(hold);
                        total_hold += elapsed;
                        longest_hold_time = longest_hold_time.max(elapsed);
                    }
                    consecutive_holds = 0;
                    assert_eq!(position - previous, 256, "full-song cursor skipped frames");
                    let peak = block
                        .iter()
                        .flatten()
                        .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
                    if let Some(index) = markers.iter().position(|(start, length)| {
                        position >= (start + 2048) as u64
                            && position < (start + length - 2048) as u64
                    }) {
                        assert!(
                            block.iter().all(|frame| frame[0].abs() < 0.005),
                            "streamed source marker {index} was not quiet"
                        );
                        quiet_markers[index] += 1;
                    } else if let Some((index, &(beat, _))) =
                        late_phrases.iter().enumerate().find(|(_, (beat, _))| {
                            let start = beat * FRAMES_PER_QUARTER;
                            position >= (start - 4096) as u64
                                && position < (start + PHRASE_LENGTH + 2048) as u64
                        })
                    {
                        let start = beat * FRAMES_PER_QUARTER;
                        for ((from, to), measured) in
                            PHRASE_WINDOWS.into_iter().zip(&mut phrase_peaks[index])
                        {
                            if position >= (start + from + 256) as u64
                                && position <= (start + to) as u64
                            {
                                *measured = measured.max(peak);
                            }
                        }
                    } else if position < u64::from(source_frames)
                        && !markers.iter().any(|(start, length)| {
                            position >= (start - 2048) as u64
                                && position < (start + length + 2048) as u64
                        })
                    {
                        assert!(peak > 0.001, "missing audio at position {position}");
                        audible_blocks += 1;
                    }
                }
                previous = position;
                assert!(
                    started.elapsed() < Duration::from_secs(25),
                    "full-song test exceeded the issue boundary budget"
                );
                let remaining = callback_period.saturating_sub(callback_started.elapsed());
                if !remaining.is_zero() {
                    std::thread::sleep(remaining);
                }
            }
            assert!(quiet_markers.into_iter().all(|blocks| blocks >= 8));
            assert!(audible_blocks > (source_frames as usize / 256) * 9 / 10);
            for (&(beat, _), peaks) in late_phrases.iter().zip(phrase_peaks) {
                check_phrase("live", beat, peaks);
            }
            println!(
                "full_live frames={source_frames} elapsed_ms={} audible_blocks={audible_blocks} buffering_blocks={buffering_blocks} longest_hold={longest_hold} quiet_markers={quiet_markers:?} phrase_peaks={phrase_peaks:?}",
                started.elapsed().as_millis()
            );
            println!(
                "full_live_readiness callback_period_us={} held_positions={held_positions} page_boundaries={page_boundaries} readiness_refusals={readiness_refusals} starved_callbacks={} hold_wall_us={} longest_hold_wall_us={}",
                callback_period.as_micros(),
                status.stream_starved_callbacks.load(Acquire) - initial_starvation,
                total_hold.as_micros(),
                longest_hold_time.as_micros(),
            );
        }
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Seek(4_000_000))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            render_completed_block(&h);
            h.pump();
            let status = h.audio.as_ref().unwrap().status().unwrap();
            assert!(
                status.render_fault_snapshot().is_none(),
                "mixed seek render fault"
            );
            assert!(
                status.rejected_action().is_none(),
                "mixed seek rejected: {:?}",
                status.rejected_action()
            );
            if status.position.load(Acquire) >= 4_000_000 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "mixed streamed seek did not complete"
            );
        }
        let mut heard_after_marker = false;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let block = render_completed_block(&h);
            let status = h.audio.as_ref().unwrap().status().unwrap();
            assert!(
                status.render_fault_snapshot().is_none(),
                "mixed playback fault after seek"
            );
            if status.position.load(Acquire) >= 4_009_216
                && !status.stream_buffering.load(Acquire)
                && block.iter().flatten().any(|sample| sample.abs() > 0.01)
            {
                heard_after_marker = true;
                break;
            }
        }
        assert!(
            heard_after_marker,
            "mixed playback did not resume after the streamed marker"
        );
        if live_edit {
            applied = exercise_mixed_streamed_loop(&mut h, applied, markers[0]);
            let cleared = super::automation::edit_with_running_callback(&mut h, Request::Undo);
            assert!(
                cleared.error.is_none(),
                "mixed loop undo: {:?}",
                cleared.error
            );
            assert_eq!(cleared.state.project, inserted.state.project);
            (applied, _) = h.applied(applied);

            let position_before = h
                .audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .position
                .load(Acquire);
            let muted = super::automation::edit_with_running_callback(
                &mut h,
                Request::Edit(Command::Mute {
                    track: audio_track,
                    muted: true,
                }),
            );
            assert!(muted.error.is_none(), "mixed mute: {:?}", muted.error);
            (applied, _) = h.applied(applied);
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut quiet_blocks = 0;
            loop {
                let block = render_completed_block(&h);
                let status = h.audio.as_ref().unwrap().status().unwrap();
                assert!(status.playing.load(Acquire), "playback stopped during mute");
                assert!(
                    status.render_fault_snapshot().is_none(),
                    "mute render fault"
                );
                // Other active sources may retain a quiet tail after seek;
                // the audible streamed track is above 0.01 here.
                if !status.stream_buffering.load(Acquire)
                    && block.iter().flatten().all(|sample| sample.abs() < 0.005)
                {
                    quiet_blocks += 1;
                    if quiet_blocks == 8 {
                        break;
                    }
                } else {
                    quiet_blocks = 0;
                }
                assert!(Instant::now() < deadline, "mixed mute did not reach output");
            }
            let undone = super::automation::edit_with_running_callback(&mut h, Request::Undo);
            assert!(undone.error.is_none(), "mixed undo: {:?}", undone.error);
            assert_eq!(undone.state.project, inserted.state.project);
            (applied, _) = h.applied(applied);
            expected_revision = undone.state.revision;
            expected_can_undo = undone.state.can_undo;
            expected_can_redo = undone.state.can_redo;
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let block = render_completed_block(&h);
                let status = h.audio.as_ref().unwrap().status().unwrap();
                assert!(status.playing.load(Acquire), "playback stopped during undo");
                assert!(
                    status.render_fault_snapshot().is_none(),
                    "undo render fault"
                );
                if !status.stream_buffering.load(Acquire)
                    && block.iter().flatten().any(|sample| sample.abs() > 0.01)
                {
                    break;
                }
                assert!(Instant::now() < deadline, "mixed undo did not reach output");
            }
            assert!(
                h.audio
                    .as_ref()
                    .unwrap()
                    .status()
                    .unwrap()
                    .position
                    .load(Acquire)
                    > position_before,
                "mixed playback did not advance across edit and undo"
            );
        }
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Stop)
            .unwrap();
        h.render_plugin_block();
        assert!(
            !h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .playing
                .load(Acquire)
        );
        h.service.submit(Request::RestoreStoppedPlayback).unwrap();
        let restored = restored_terminal(&mut h);
        assert!(
            restored.error.is_none(),
            "mixed restore: {:?}",
            restored.error
        );
        assert_eq!(restored.state.project, inserted.state.project);
        (applied, _) = h.applied(applied);
        if full_playthrough {
            let undone = h.run(Request::Undo);
            assert_eq!(undone.state.project, before_insert);
            (applied, _) = h.applied(applied);
            let redone = h.run(Request::Redo);
            assert_eq!(redone.state.project, inserted.state.project);
            (applied, _) = h.applied(applied);
            expected_revision = redone.state.revision;
            expected_can_undo = redone.state.can_undo;
            expected_can_redo = redone.state.can_redo;
        }
    }
    let before = Arc::clone(&inserted.state.project);
    let live_binding = h.plugin_progress.lock().unwrap().binding;
    let saved_path = root.join("live-state.hex");
    let captured_before = if verify_live_state {
        let saved = h.run(Request::SaveNew(saved_path.clone()));
        assert!(saved.save_complete);
        assert_eq!(saved.state.project, before);
        assert_eq!(saved.state.revision, expected_revision);
        Some(hex_storage::load(&saved_path).unwrap())
    } else {
        None
    };
    let path = root.join("mixed.wav");
    let control = ExportControl::default();
    let queued = super::super::resident_sections::run_callback(
        &mut h,
        Request::ExportRange {
            path: path.clone(),
            start: 0,
            frames: Some(if probe || complete {
                u64::from(source_frames)
            } else {
                1024
            }),
            signal: ExportSignal::Main,
            control: control.clone(),
            format: hex_export::ExportFormat::Pcm16,
            overwrite: false,
        },
    );
    assert!(queued.error.is_none(), "{:?}", queued.error);
    let started = Instant::now();
    let until = started
        + Duration::from_secs(if probe {
            3
        } else if complete {
            27
        } else {
            10
        });
    let result = loop {
        h.pump();
        if !probe && !complete {
            h.render();
        }
        if let Ok(event) = h.service.try_export_event() {
            break event.result;
        }
        let now = Instant::now();
        if probe && now >= until {
            let progress = control.progress();
            let elapsed = started.elapsed();
            assert!(control.cancel());
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                h.pump();
                if let Ok(event) = h.service.try_export_event() {
                    assert!(event.result.is_err());
                    break;
                }
                assert!(Instant::now() < deadline, "cancelled probe did not stop");
                std::thread::yield_now();
            }
            assert!(!path.exists());
            println!(
                "export_probe elapsed_ns={} processed_frames={} total_frames={}",
                elapsed.as_nanos(),
                progress.processed_frames,
                progress.total_frames
            );
            return;
        }
        assert!(
            now < until,
            "mixed export timed out: {:?}",
            control.progress()
        );
        std::thread::yield_now();
    };
    let report = result.expect("mixed VST export");
    let wav = fs::read(path).unwrap();
    if complete {
        use sha2::{Digest, Sha256};
        let hash = |bytes: &[u8]| {
            Sha256::digest(bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        assert_eq!(report.frames, u64::from(source_frames));
        assert_eq!(wav.len() as u64, 44 + u64::from(source_frames) * 4);
        assert!(wav[44..44 + 4096].iter().any(|byte| *byte != 0));
        assert!(wav[wav.len() - 4096..].iter().any(|byte| *byte != 0));
        for (start, length) in markers {
            let channel_bytes = if full_playthrough { 2 } else { 4 };
            let silent = |frame: usize| {
                let offset = 44 + frame * 4;
                wav[offset..offset + channel_bytes]
                    .iter()
                    .all(|byte| *byte == 0)
            };
            let observed_start = (start - 2048..start + 2048)
                .find(|frame| silent(*frame))
                .expect("late-song silence begins near its source marker");
            let observed_end = (start + length - 2048..start + length + 2048)
                .find(|frame| !silent(*frame))
                .expect("late-song audio resumes near its source marker");
            assert_eq!(
                (observed_start, observed_end),
                (start, start + length),
                "source marker must be sample-aligned"
            );
            assert!((observed_start..observed_end).all(silent));
            println!("source_marker={start} output_marker={observed_start} length={length}");
        }
        for &(beat, _) in late_phrases {
            let start = beat * FRAMES_PER_QUARTER;
            let peaks = PHRASE_WINDOWS.map(|(from, to)| {
                wav[44 + (start + from) * 4..44 + (start + to) * 4]
                    .chunks_exact(2)
                    .map(|bytes| {
                        f32::from(i16::from_le_bytes([bytes[0], bytes[1]])).abs() / 32768.0
                    })
                    .fold(0.0_f32, f32::max)
            });
            check_phrase("export", beat, peaks);
            println!("export_phrase quarter={beat} peaks={peaks:?}");
        }
        println!(
            "complete_export elapsed_ns={} frames={} output_sha256={} source_sha256={}",
            started.elapsed().as_nanos(),
            report.frames,
            hash(&wav),
            hash(&source_bytes)
        );
    } else {
        assert_eq!(report.frames, 1024);
        assert_eq!(wav.len(), 44 + 1024 * 4);
    }
    assert!(
        wav[44..]
            .chunks_exact(4)
            .any(|frame| i16::from_le_bytes([frame[0], frame[1]]) != 0)
    );
    assert_eq!(queued.state.project, before);
    assert_eq!(h.plugin_progress.lock().unwrap().binding, live_binding);
    assert_eq!(fs::read(&source).unwrap(), source_bytes);
    if let Some(captured_before) = captured_before {
        let saved = h.run(Request::SaveCurrent);
        assert!(saved.save_complete);
        assert_eq!(saved.state.project, before);
        assert_eq!(saved.state.revision, expected_revision);
        assert_eq!(saved.state.can_undo, expected_can_undo);
        assert_eq!(saved.state.can_redo, expected_can_redo);
        let captured_after = hex_storage::load(&saved_path).unwrap();
        let instrument_state = |project: &hex_project::Project| {
            project
                .track(track)
                .unwrap()
                .instrument()
                .unwrap()
                .state()
                .clone()
        };
        let effect_state = |project: &hex_project::Project| {
            project.track(audio_track).unwrap().devices()[0]
                .state()
                .clone()
        };
        assert_eq!(
            instrument_state(&captured_before),
            instrument_state(&captured_after)
        );
        assert_eq!(
            effect_state(&captured_before),
            effect_state(&captured_after)
        );
    }
    let muted = h.run(Request::Edit(Command::Mute { track, muted: true }));
    h.applied(applied);
    let silent_instrument_path = root.join("muted-instrument.wav");
    let muted_queued = super::super::resident_sections::run_callback(
        &mut h,
        Request::ExportRange {
            path: silent_instrument_path.clone(),
            start: 0,
            frames: Some(1024),
            signal: ExportSignal::Main,
            control: ExportControl::default(),
            format: hex_export::ExportFormat::Pcm16,
            overwrite: false,
        },
    );
    assert!(muted_queued.error.is_none(), "{:?}", muted_queued.error);
    let until = Instant::now() + Duration::from_secs(10);
    let result = loop {
        h.pump();
        h.render();
        if let Ok(event) = h.service.try_export_event() {
            break event.result;
        }
        assert!(Instant::now() < until, "muted export timed out");
        std::thread::yield_now();
    };
    assert_eq!(result.expect("muted export").frames, 1024);
    let without_instrument = fs::read(silent_instrument_path).unwrap();
    assert!(
        without_instrument[44..]
            .chunks_exact(4)
            .any(|frame| (1000..=7000).contains(&i16::from_le_bytes([frame[0], frame[1]]))),
        "effect must audibly process the streamed source"
    );
    assert_ne!(
        &wav[44..44 + 1024 * 4],
        &without_instrument[44..],
        "instrument must change the rendered mix"
    );
    assert_eq!(muted_queued.state.project, muted.state.project);
    if complete {
        if full_playthrough || std::env::var_os("HEX_TEST_EXPORT_REOPEN").is_some() {
            assert!(verify_live_state, "reopen requires the saved mixed project");
            let saved_project = hex_storage::load(&saved_path).unwrap();
            let old_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
            let reopened = h.run(Request::Open(saved_path));
            assert!(reopened.open_complete && reopened.project_replaced);
            assert_eq!(reopened.state.project.snapshot(), saved_project.snapshot());
            super::super::resident_lifecycle::finish_replacement(&mut h, &reopened, old_output);
            h.audio
                .as_mut()
                .unwrap()
                .control
                .command(PlaybackCommand::Play)
                .unwrap();
            let mut pcm = Vec::new();
            let deadline = Instant::now() + Duration::from_secs(5);
            while pcm.len() < 4096 {
                let block = render_completed_block(&h);
                let status = h.audio.as_ref().unwrap().status().unwrap();
                assert!(
                    status.render_fault_snapshot().is_none(),
                    "reopened render fault"
                );
                if !status.stream_buffering.load(Acquire) {
                    pcm.extend(block);
                }
                assert!(Instant::now() < deadline, "reopened stream did not start");
            }
            assert!(pcm.iter().flatten().any(|sample| sample.abs() > 0.001));
            assert!(
                pcm.iter().enumerate().skip(512).any(|(index, frame)| {
                    let dry = 0.25 * (0.5 + ((index - 512) / 256) as f32 * 0.125);
                    (frame[0] - dry).abs() > 1e-4 || (frame[1] - dry).abs() > 1e-4
                }),
                "reopened mix did not differ from the controlled effect alone"
            );
            assert_eq!(fs::read(&source).unwrap(), source_bytes);
        }
        return;
    }
    for changed in [false, true] {
        if changed {
            let mut altered = source_bytes.clone();
            altered[48] ^= 1;
            fs::write(&source, altered).unwrap();
        }
        let destination = root.join("existing.wav");
        fs::write(&destination, b"preserve existing").unwrap();
        let control = ExportControl::default();
        if !changed {
            assert!(control.cancel());
        }
        let refused = super::super::resident_sections::run_callback(
            &mut h,
            Request::ExportRange {
                path: destination.clone(),
                start: 0,
                frames: Some(1024),
                signal: ExportSignal::Main,
                control,
                format: hex_export::ExportFormat::Pcm16,
                overwrite: true,
            },
        );
        assert_eq!(refused.state.project, muted.state.project);
        let until = Instant::now() + Duration::from_secs(10);
        let result = loop {
            h.pump();
            h.render();
            if let Ok(event) = h.service.try_export_event() {
                break event.result;
            }
            assert!(Instant::now() < until, "refused export timed out");
            std::thread::yield_now();
        };
        assert!(result.is_err());
        assert_eq!(fs::read(destination).unwrap(), b"preserve existing");
        if changed {
            fs::write(&source, &source_bytes).unwrap();
        }
    }
    assert_eq!(fs::read(source).unwrap(), source_bytes);
}

const MIXED_WITNESS_FRAMES: u32 = 8192;

fn mixed_witness_sample(frame: u64) -> i16 {
    (((frame * 73) ^ ((frame >> 3) * 17)) & 1023) as i16 - 512
}

fn assert_mixed_insert_pcm(block: &[hex_engine::Frame; 256], start: u64) {
    assert!(start + block.len() as u64 <= u64::from(MIXED_WITNESS_FRAMES));
    let baseline = block[0][0] - f32::from(mixed_witness_sample(start)) / 32768.0;
    assert!(
        baseline > 0.01,
        "streamed tone is absent from the marker block"
    );
    // The SDK effect changes its constant output level between processing
    // quanta. Adjacent pairs distinguish the resident samples from that level.
    for (offset, pair) in block.chunks_exact(2).enumerate() {
        let frame = start + 2 * offset as u64;
        let expected =
            f32::from(mixed_witness_sample(frame + 1) - mixed_witness_sample(frame)) / 32768.0;
        let actual = pair[1][0] - pair[0][0];
        assert!(
            (actual - expected).abs() < 2e-5,
            "resident marker in block at {start}, frame {frame}: {actual} != {expected}"
        );
    }
    let (low, high) = block
        .iter()
        .map(|frame| frame[1])
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(low, high), sample| {
            (low.min(sample), high.max(sample))
        });
    assert!(
        high - low > 0.01,
        "expected a held Diva note in the right channel"
    );
}

fn exercise_mixed_native_insert(
    h: &mut ResidentHarness,
    before: &hex_session::service::ViewState,
) -> Event {
    use hex_session::service::DeviceInsertOutcome;

    h.enable_confirmed_device_inserts();
    let status = h.audio.as_ref().unwrap().status().unwrap();
    let predecessor = status.recording_source().unwrap();
    let previous_start = status.audible_position.load(Acquire);
    assert_mixed_insert_pcm(&render_completed_block(h), previous_start);
    let (submitted, resume) = h.probe_next_submitted_device_insert();
    let state = hex_engine::default_native_device_state(hex_engine::DeviceTypeId::from_static(
        "hex.utility",
    ))
    .unwrap();
    h.service
        .submit(Request::Edit(Command::InsertMainDevice {
            before: None,
            state,
        }))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let next = loop {
        if let Ok(next) = submitted.try_recv() {
            break next;
        }
        h.pump();
        if let Ok(event) = h.service.try_event() {
            assert!(
                event.error.is_none(),
                "mixed Utility staging: {:?}",
                event.error
            );
            assert_eq!(event.state.project, before.project);
        }
        assert!(
            Instant::now() < deadline,
            "mixed Utility submission stalled"
        );
        std::thread::yield_now();
    };
    let status = h.audio.as_ref().unwrap().status().unwrap();
    let position = status.position.load(Acquire);
    let start = status.audible_position.load(Acquire);
    assert_eq!(start, previous_start + 256);
    assert_eq!(status.recording_source(), Some(predecessor));
    // The Session worker is parked after enqueue; this block owns admission.
    let output = h.render_plugin_block();
    let installed = status.recording_source().unwrap();
    assert_eq!(installed.context, next.context);
    assert_eq!(installed.audio_revision, next.audio_revision);
    assert_eq!(installed.output_identity, predecessor.output_identity);
    assert_eq!(status.position.load(Acquire), position + 256);
    assert_mixed_insert_pcm(&output, start);
    println!(
        "seamless_utility output={} predecessor_revision={} installed_revision={} marked_start={previous_start}..{}",
        installed.output_identity,
        predecessor.audio_revision,
        installed.audio_revision,
        start + 256
    );
    resume.send(()).unwrap();
    loop {
        let event = h.next();
        assert!(
            event.error.is_none(),
            "mixed Utility install: {:?}",
            event.error
        );
        assert!(event.device_insert_confirmation.is_none());
        if let Some(receipt) = event.device_insert_complete {
            assert_eq!(receipt.submission, next);
            assert_eq!(receipt.outcome, DeviceInsertOutcome::Installed);
            assert_eq!(event.state.revision, before.revision + 1);
            assert_eq!(event.state.project.main_devices().len(), 1);
            assert_eq!(
                event.state.project.main_devices()[0].state().type_id(),
                hex_engine::DeviceTypeId::from_static("hex.utility")
            );
            return event;
        }
        assert_eq!(event.state.project, before.project);
    }
}

fn exercise_mixed_streamed_loop(
    h: &mut ResidentHarness,
    applied: u64,
    (marker, marker_length): (usize, usize),
) -> u64 {
    let marker = marker as u64;
    let marker_end = marker + marker_length as u64;
    let start = marker - 4096;
    // Playhead movement during edit admission is in 64-frame steps. Force
    // the loop end to split a physical block at any of those alignments.
    let end = marker_end + 4096 + 17;
    let edited = super::automation::edit_with_running_callback(
        h,
        Request::Edit(Command::SetLoopRegion {
            start,
            end,
            enabled: true,
        }),
    );
    assert!(edited.error.is_none(), "mixed loop: {:?}", edited.error);
    let (revision, metadata) = h.applied(applied);
    assert_eq!(metadata.context.editor_revision, edited.state.revision);
    let status = h.audio.as_ref().unwrap().status().unwrap();
    let mut previous = status.position.load(Acquire);
    assert!((start..end).contains(&previous), "loop cursor {previous}");
    let mut complete_passes = 0;
    let mut started_pass = false;
    let mut quiet_peak = 0.0_f32;
    let mut resumed_peak = 0.0_f32;
    let mut quiet_blocks = 0;
    let mut partial_holds = 0;
    let deadline = Instant::now() + Duration::from_secs(5);
    while complete_passes < 2 {
        let block = render_completed_block(h);
        assert!(status.playing.load(Acquire), "mixed loop stopped playing");
        assert!(
            status.render_fault_snapshot().is_none(),
            "mixed loop render fault"
        );
        assert!(
            status.rejected_action().is_none(),
            "mixed loop action refused"
        );
        let position = status.position.load(Acquire);
        let advanced = if status.stream_buffering.load(Acquire) {
            let committed = usize::try_from(status.stream_committed_frames.load(Acquire)).unwrap();
            assert!(committed < block.len());
            assert!(block[committed..].iter().all(|frame| *frame == [0.0; 2]));
            partial_holds += usize::from(committed != 0);
            committed
        } else {
            block.len()
        };
        assert_eq!(
            position,
            start + (previous - start + advanced as u64) % (end - start),
            "loop cursor must advance only by rendered frames"
        );
        if position < previous {
            if started_pass {
                assert!(quiet_blocks >= 8, "loop skipped the streamed marker");
                assert!(
                    resumed_peak > 0.01 && quiet_peak < resumed_peak * 0.05,
                    "stream marker did not repeat: quiet={quiet_peak}, resumed={resumed_peak}"
                );
                complete_passes += 1;
            }
            started_pass = true;
            quiet_peak = 0.0;
            resumed_peak = 0.0;
            quiet_blocks = 0;
        } else if started_pass && advanced == block.len() {
            assert!(block.iter().flatten().all(|sample| sample.is_finite()));
            let peak = block
                .iter()
                .flatten()
                .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
            if (marker + 2048..marker_end - 2048).contains(&position) {
                quiet_blocks += 1;
                quiet_peak = quiet_peak.max(peak);
            } else if position >= marker_end + 2048 {
                assert!(peak > 0.01, "stream did not resume at {position}: {peak}");
                resumed_peak = resumed_peak.max(peak);
            }
        }
        previous = position;
        assert!(
            Instant::now() < deadline,
            "mixed loop did not complete two passes"
        );
    }
    println!("mixed loop {start}..{end} passes={complete_passes} partial_holds={partial_holds}");
    revision
}
