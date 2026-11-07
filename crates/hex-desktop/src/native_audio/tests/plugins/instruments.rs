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

fn render_completed_block(harness: &ResidentHarness) -> [hex_engine::Frame; 256] {
    let previous = harness.plugin_completed.load(Acquire);
    let frames = harness.render_plugin_block();
    let deadline = Instant::now() + Duration::from_millis(800);
    while harness.plugin_completed.load(Acquire) <= previous {
        let status = harness.audio.as_ref().unwrap().status().unwrap();
        assert!(
            Instant::now() < deadline,
            "worker did not complete next block after {previous}; playing={} source={:?} fault={:?}",
            status.playing.load(Acquire),
            status.native_effect_source(),
            status.render_fault_snapshot(),
        );
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
        for case in ["reading", "unread", "idle", "refused"] {
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

#[derive(Clone, Copy)]
enum InstrumentWorkflow {
    Playback,
    Authoring,
    Takeover,
}

fn instrument_entrypoint(workflow: InstrumentWorkflow) {
    let shared_authoring = !matches!(workflow, InstrumentWorkflow::Playback);
    let takeover = matches!(workflow, InstrumentWorkflow::Takeover);
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
    let mut harness = if automation_parameter.is_some() || (shared_authoring && installed.is_some())
    {
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
    let (clip_length, note_duration) = if takeover {
        (
            BeatTime::from_quarters(16).unwrap(),
            BeatTime::from_quarters(16).unwrap(),
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
    let mut insertion = request(&note.state, PluginEffectDestination::Instrument(track));
    if let Some(bundle) = installed {
        insertion.selection = installed_selection(
            &PathBuf::from(std::env::var_os("HEX_TEST_VST3_INSTRUMENT_CATALOG").unwrap()),
            &bundle,
            hex_plugin_catalog::PluginRole::Instrument,
        );
    }
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

fn verify_instrument_takeover(
    h: &mut ResidentHarness,
    original: &hex_project::Project,
    device: hex_project::DeviceId,
    mut applied: u64,
) {
    use hex_project::{
        AutomationInterpolation, AutomationPoint, AutomationValue, DeviceAutomationTarget,
    };
    use hex_session::device_gesture::{GestureClient, PluginParameterOrigin, PluginParameterValue};
    use hex_session::service::{DeviceAutomationControlEdit, PluginParameterQuery};
    let case = std::env::var("HEX_DESKTOP_INSTRUMENT_TAKEOVER_CASE").unwrap();
    let reading = case != "unread";
    let playing = case != "idle";
    let context = h.submitted.unwrap().1.context;
    let output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    h.run(Request::QueryPluginParameters(PluginParameterQuery {
        context,
        device,
        output_identity: output,
        request_id: 11,
    }));
    let rows = h.next().plugin_parameters.unwrap().rows.unwrap();
    assert_eq!(rows.len(), 2);
    let class = rows
        .iter()
        .find(|r| r.id == 7)
        .unwrap()
        .automation_class
        .unwrap();
    let mut before = original.clone();
    let mut revision = context.editor_revision;
    for (id, value) in [(7, 0.25), (11, 0.5)] {
        if id == 7 && !reading {
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
    let query = |h: &mut ResidentHarness, request_id| {
        let context = h.submitted.unwrap().1.context;
        h.run(Request::QueryPluginParameters(PluginParameterQuery {
            context,
            device,
            output_identity: output,
            request_id,
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
        loop {
            h.pump();
            if let Ok(event) = h.service.try_event()
                && let Some(ack) = event.gesture_ack.as_ref()
                && ack.key == request.key()
                && ack.phase == request.phase()
            {
                assert_eq!(ack.accepted, accepted, "{case}: {ack:?} {:?}", event.error);
                assert!(!ack.source_lost, "{case}: {ack:?}");
                assert!(client.acknowledge(ack));
                return event;
            }
            assert!(
                Instant::now() < deadline,
                "instrument gesture timed out: {request:?}"
            );
            h.render_worker_step();
            std::thread::yield_now();
        }
    };
    if case == "unread" {
        let mut client = GestureClient::default();
        client.begin_plugin(query(h, 12)).unwrap();
        let refused = send(h, &mut client, false);
        assert!(
            refused
                .error
                .as_ref()
                .unwrap()
                .contains("create an automation lane")
        );
        assert_eq!(refused.state.project.as_ref(), &before);
        assert!((peak(h) - baseline).abs() < 0.0001);
        println!("instrument unarmed target remains explicitly guarded");
        return;
    }
    let mut client = GestureClient::default();
    client.begin_plugin(query(h, 12)).unwrap();
    assert_eq!(send(h, &mut client, true).state.project.as_ref(), &before);
    client
        .change_plugin(PluginParameterValue::new(0.75).unwrap())
        .unwrap();
    assert_eq!(send(h, &mut client, true).state.project.as_ref(), &before);
    if playing {
        assert!(
            (peak(h) - baseline * 3.0).abs() < 0.0001,
            "held MIDI must continue through preview"
        );
    }
    client.cancel().unwrap();
    assert_eq!(send(h, &mut client, true).state.project.as_ref(), &before);
    if playing {
        assert!(
            (peak(h) - baseline).abs() < 0.0001,
            "cancel must resume authored input"
        );
    }
    client.begin_plugin(query(h, 13)).unwrap();
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
        assert!(
            (peak(h) - baseline).abs() < 0.0001,
            "undo must restore audible authored input"
        );
    }
    let redone = super::automation::edit_with_running_callback(h, Request::Redo);
    assert!(redone.error.is_none(), "{:?}", redone.error);
    assert_eq!(redone.state.project, committed.state.project);
    applied = h.applied(applied).0;
    if playing {
        assert!(
            (peak(h) - baseline * 2.0).abs() < 0.0001,
            "redo must restore audible manual base"
        );
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
    h.applied(applied);
    if playing {
        assert!(
            (peak(h) - baseline).abs() < 0.0001,
            "re-enable must release the original worker hold"
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
    println!("instrument takeover {case} passed");
}
