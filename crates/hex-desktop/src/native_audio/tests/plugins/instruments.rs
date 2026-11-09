//! Native instrument insertion through the real Session and desktop worker.
use super::*;
use hex_engine::{LaunchQuantization::Immediate, SessionPlaybackCommand};
use hex_project::BeatTime;

#[test]
#[ignore = "requires explicit source-bound SDK receipts and optional installed catalog"]
fn native_vst3_instrument_bounces_to_new_track() {
    const CHILD: &str = "HEX_DESKTOP_TEST_INSTRUMENT_BOUNCE_CHILD";
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
    let mut harness = ResidentHarness::with_plugins_at_rate_and_catalog(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
        rate,
        catalog,
        2_000_000,
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
        assert!(
            sample > 0.001,
            "offline bridge latency must be trimmed: {sample}"
        );
    }
    harness.applied(applied);
    assert_eq!(
        harness.plugin_progress.lock().unwrap().binding,
        Some(binding)
    );
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
    Authoring,
    PacedAuthoring,
    PacedMonitoring,
    Takeover,
}

fn instrument_entrypoint(workflow: InstrumentWorkflow) {
    let shared_authoring = !matches!(workflow, InstrumentWorkflow::Playback);
    let takeover = matches!(workflow, InstrumentWorkflow::Takeover);
    let paced = matches!(
        workflow,
        InstrumentWorkflow::PacedAuthoring | InstrumentWorkflow::PacedMonitoring
    );
    let monitor = matches!(workflow, InstrumentWorkflow::PacedMonitoring);
    let name = std::thread::current().name().unwrap().to_owned();
    const CHILD: &str = "HEX_DESKTOP_TEST_INSTRUMENT_CHILD";
    let installed = std::env::var("HEX_TEST_VST3_INSTRUMENT_BUNDLE").ok();
    if std::env::var_os(CHILD).is_none() {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .args(["--exact", &name, "--ignored", "--nocapture"])
            .env(CHILD, "1");
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
    let (clip_length, note_duration) = if takeover || paced {
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
            source_length: BeatTime::from_quarters(1).unwrap(),
        }));
        let Some(Created::SessionClip(console)) = console.created else {
            panic!("console clip")
        };
        (applied, _) = harness.applied(applied);
        note = harness.run(Request::Edit(Command::AddMidiNote {
            clip: console,
            start: BeatTime::ZERO,
            duration: BeatTime::from_quarters(1).unwrap(),
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
            project_track.instrument().unwrap().id(),
            applied,
            monitor,
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
    }
}

fn verify_paced_instrument_authoring(
    harness: &mut ResidentHarness,
    device: hex_project::DeviceId,
    mut applied: u64,
    monitor: bool,
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
    let mut peak = 0.0_f32;
    let mut tail_peak = 0.0_f32;
    for block in 0..600_u32 {
        let deadline = start + period * block;
        if let Some(delay) = deadline.checked_duration_since(Instant::now()) {
            std::thread::sleep(delay);
        }
        harness.pump();
        max_lateness = max_lateness.max(Instant::now().saturating_duration_since(deadline));
        let frames = harness.render_plugin_block();
        let block_peak = frames
            .into_iter()
            .flatten()
            .fold(0.0_f32, |value, sample| value.max(sample.abs()));
        peak = peak.max(block_peak);
        if block >= 550 {
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
        if !pending && sent < 3 && block >= 50 + sent * 125 {
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
            harness
                .service
                .submit(Request::DeviceAutomation {
                    project_generation: context.project_generation,
                    revision,
                    device,
                    target,
                    vst3_class: Some(class),
                    points,
                })
                .unwrap();
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
            "vst endpoint degraded after block {block}, maximum lateness {:?}: {:?}",
            max_lateness,
            harness.degraded_devices
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
    assert_eq!((sent, completed, pending), (3, 3, false));
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
        "paced_vst3 rate={} blocks=600 edits={completed} applied={applied} max_late_us={} peak={peak} tail_peak={tail_peak}",
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
    let first_lane = matches!(
        case.as_str(),
        "session_first_lane" | "session_first_lane_with_other" | "session_first_lane_stop"
    );
    let stop_recording = matches!(case.as_str(), "session_stop" | "session_first_lane_stop");
    let reading = !first_lane && !matches!(case.as_str(), "unread" | "empty" | "renew");
    let playing = case != "idle";
    let context = h.submitted.unwrap().1.context;
    let output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
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
        let rows = h.next().plugin_parameters.unwrap().rows.unwrap();
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
            "empty" | "renew" | "author_empty" | "session_first_lane" | "session_first_lane_stop"
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
                            export.result.unwrap_err().contains(
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
        let replay = |h: &mut ResidentHarness, path: &std::path::Path| -> Vec<[f32; 2]> {
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
            let foreign_ready = |h: &ResidentHarness| {
                h.submitted_observation().unwrap().foreign_processor(
                    device.value(),
                    before.device(device).unwrap().state().type_id(),
                ) == Some(true)
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
                h.submitted_observation().unwrap().foreign_processor(
                    device.value(),
                    before.device(device).unwrap().state().type_id()
                ),
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
        };
        let expected = replay(h, &reference_path);
        let actual = replay(h, &path);
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
