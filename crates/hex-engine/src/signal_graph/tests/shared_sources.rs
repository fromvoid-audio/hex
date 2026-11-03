//! Shared source preparation preserves local audio paths and MIDI identities.
use super::*;
use hex_project::{BeatTime, ClipId, SourceId};

fn shared_fixture() -> (Fixture, SourceId, [ClipId; 2]) {
    let mut f = fixture(&[&[3], &[7]], 32);
    let Created::NativeSource(source) = f
        .editor
        .execute(Command::CreateNativeSource {
            name: "shared".into(),
            devices: vec![InstrumentDefinition.default_state().unwrap()],
            instrument_index: 0,
        })
        .unwrap()
    else {
        panic!("source")
    };
    let mut clips = Vec::new();
    for (index, track) in f.tracks.iter().copied().enumerate() {
        f.editor
            .execute(Command::BindTrackSource {
                track,
                source: Some(source),
            })
            .unwrap();
        let Created::Clip(clip) = f
            .editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ONE,
                length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("midi clip")
        };
        f.editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 60 + index as u8,
                velocity: 100,
                channel: 0,
            })
            .unwrap();
        clips.push(clip);
    }
    (f, source, [clips[0], clips[1]])
}
fn compile(f: &Fixture) -> Project {
    try_compile(f).unwrap()
}
fn try_compile(f: &Fixture) -> Result<Project, Invalid> {
    let effects: [&dyn NativeDeviceDefinition; 1] = [&Definition];
    let instruments: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&InstrumentDefinition];
    crate::compile::project_with_midi_preview_and_table_and_instrument_registry(
        f.editor.project(),
        &f.media,
        None,
        &NativeDeviceRegistry::new(&effects).unwrap(),
        &hex_lattice::NativeInstrumentRegistry::new(&instruments).unwrap(),
    )
    .map(|(project, _)| project)
}

#[test]
fn shared_analytic_receiver_compensation_preserves_mixed_routes_and_loop_partitions() {
    use crate::{LaunchQuantization, SessionPlaybackCommand};
    let mut editor = Editor::new(hex_project::Project::new(48_000).unwrap(), 16).unwrap();
    let state = crate::default_native_device_state_for_class(
        crate::NativeDeviceClass::Instrument,
        hex_devices::SINE_INSTRUMENT_TYPE_ID,
    )
    .unwrap();
    let Created::NativeSource(source) = editor
        .execute(Command::CreateNativeSource {
            name: "shared".into(),
            devices: vec![state],
            instrument_index: 0,
        })
        .unwrap()
    else {
        panic!("source")
    };
    let mut tracks = Vec::new();
    for pitch in [60, 64, 67] {
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: "midi".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        editor
            .execute(Command::BindTrackSource {
                track,
                source: Some(source),
            })
            .unwrap();
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        else {
            panic!("clip")
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch,
                velocity: 96,
                channel: 0,
            })
            .unwrap();
        tracks.push(track);
    }
    editor
        .execute(Command::SetNativeSourceReceiver {
            source,
            track: Some(tracks[1]),
        })
        .unwrap();
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(clip) = editor
        .execute(Command::CreateSessionMidiClip {
            track: tracks[0],
            scene,
            name: "session".into(),
            source_length: BeatTime::from_quarters(4).unwrap(),
        })
        .unwrap()
    else {
        panic!("session clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 72,
            velocity: 96,
            channel: 0,
        })
        .unwrap();
    editor
        .execute(Command::SetLoopRegion {
            start: 0,
            end: 257,
            enabled: true,
        })
        .unwrap();
    let limiter = crate::default_native_device_state_for_class(
        crate::NativeDeviceClass::Effect,
        DeviceTypeId::from_static("hex.limiter"),
    )
    .unwrap();
    editor
        .execute(Command::InsertDevice {
            track: tracks[1],
            before: None,
            state: limiter,
        })
        .unwrap();
    let revision = Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &std::collections::BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let render = |partition| {
        let mut playback = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        playback
            .apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        let mut output = [[0.0; 2]; 1024];
        for chunk in output.chunks_mut(partition) {
            playback.render(chunk).unwrap();
        }
        (
            output,
            playback.performance_frame(),
            playback.arrangement_frame(),
        )
    };
    let expected = render(1);
    assert!(
        expected
            .0
            .iter()
            .flatten()
            .any(|sample| sample.abs() > 1.0e-6)
    );
    assert_eq!((expected.1, expected.2), (1024, 253));
    for partition in [17, 64, 256, 512] {
        assert_eq!(render(partition), expected, "partition {partition}");
    }
}

#[test]
fn shared_source_main_only_latency_preserves_receiver_mix_and_partitions() {
    for mutable in [false, true] {
        let kind = if mutable {
            hex_devices::operator_state::OPERATOR_TYPE_ID
        } else {
            hex_devices::SINE_INSTRUMENT_TYPE_ID
        };
        let mut state = crate::default_native_device_state_for_class(
            crate::NativeDeviceClass::Instrument,
            kind,
        )
        .unwrap();
        if mutable {
            state = crate::patch_native_device_parameter_for_class(
                crate::NativeDeviceClass::Instrument,
                &state,
                hex_devices::operator_state::parameter::feedback(1),
                ParameterValue::Float(0.24),
            )
            .unwrap();
        }
        let mut editor = Editor::new(hex_project::Project::new(48_000).unwrap(), 1).unwrap();
        let Created::NativeSource(source) = editor
            .execute(Command::CreateNativeSource {
                name: "shared".into(),
                devices: vec![state],
                instrument_index: 0,
            })
            .unwrap()
        else {
            panic!("source")
        };
        let mut tracks = Vec::new();
        for (index, pitch) in [60, 67].into_iter().enumerate() {
            let Created::Track(track) = editor
                .execute(Command::AddMidiTrack {
                    name: "midi".into(),
                })
                .unwrap()
            else {
                panic!("track")
            };
            editor
                .execute(Command::BindTrackSource {
                    track,
                    source: Some(source),
                })
                .unwrap();
            editor
                .execute(Command::Gain {
                    track,
                    gain: if index == 1 { 0.37 } else { 1.7 },
                })
                .unwrap();
            editor
                .execute(Command::Balance {
                    track,
                    balance: -0.3,
                })
                .unwrap();
            let Created::Clip(clip) = editor
                .execute(Command::PlaceMidiClip {
                    track,
                    start: BeatTime::ZERO,
                    length: BeatTime::ONE,
                })
                .unwrap()
            else {
                panic!("clip")
            };
            editor
                .execute(Command::AddMidiNote {
                    clip,
                    start: BeatTime::ZERO,
                    duration: BeatTime::ONE,
                    pitch,
                    channel: 0,
                    velocity: 96,
                })
                .unwrap();
            tracks.push(track);
        }
        editor
            .execute(Command::SetNativeSourceReceiver {
                source,
                track: Some(tracks[1]),
            })
            .unwrap();
        let table = Arc::new(SineTable::new());
        let media = std::collections::BTreeMap::new();
        let dry_project = editor.project().clone();
        let dry_revision = Arc::new(
            PreparedRevision::prepare(&dry_project, &media, Arc::clone(&table), None).unwrap(),
        );
        assert_eq!(dry_revision.has_arrangement_mutable_instruments(), mutable);
        let dry = session(dry_revision, 4096, 63);
        assert!(dry.iter().flatten().any(|sample| sample.abs() > 1e-6));
        let limiter = crate::default_native_device_state_for_class(
            crate::NativeDeviceClass::Effect,
            DeviceTypeId::from_static("hex.limiter"),
        )
        .unwrap();
        let limiter = crate::patch_native_device_parameter_for_class(
            crate::NativeDeviceClass::Effect,
            &limiter,
            ParameterId::from_static(6),
            ParameterValue::Choice(2),
        )
        .unwrap();
        editor
            .execute(Command::InsertMainDevice {
                before: None,
                state: limiter,
            })
            .unwrap();
        let revision =
            Arc::new(PreparedRevision::prepare(editor.project(), &media, table, None).unwrap());
        assert!(
            !revision
                .arrangement()
                .signal_graph
                .as_ref()
                .unwrap()
                .needs_upstream_frame_routing()
        );
        let delay = revision.arrangement().main_latency_frames() as usize;
        assert_eq!(delay, 288);
        // Keyboard monitoring plays through the batched Main latency (#3772).
        let mut monitor =
            crate::playback::Playback::try_from_prepared_with_session(Arc::clone(&revision))
                .unwrap();
        let keys = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
        keys.set_key(0, Some((60, 100, 1))).unwrap();
        monitor
            .prepare_computer_keyboard(editor.project(), tracks[0], keys)
            .unwrap();
        let mut monitored = vec![[0.0; 2]; delay + 512];
        monitor.render(&mut monitored).unwrap();
        assert!(monitored[..delay].iter().all(|frame| *frame == [0.0; 2]));
        assert!(
            monitored[delay..]
                .iter()
                .flatten()
                .any(|sample| *sample != 0.0)
        );
        for partition in [1, 63, 257] {
            let output = session(Arc::clone(&revision), dry.len(), partition);
            assert!(output[..delay].iter().all(|frame| *frame == [0.0; 2]));
            let error = output[delay..]
                .iter()
                .flatten()
                .zip(dry[..dry.len() - delay].iter().flatten())
                .map(|(actual, expected)| (actual - expected).abs())
                .fold(0.0_f32, f32::max);
            assert!(
                error < 1e-6,
                "mutable={mutable}, partition={partition}: {error}"
            );
        }
    }
}
#[test]
fn shared_source_has_one_generator_and_preserves_both_local_chains_and_midi_banks() {
    let (f, _, clips) = shared_fixture();
    let compiled = compile(&f);
    let graph = compiled.signal_graph.as_ref().unwrap();
    assert!(graph.tracks[0].generator.is_some());
    assert!(graph.tracks[1].generator.is_none());
    assert_eq!(graph.tracks[0].devices.len(), 1);
    assert_eq!(graph.tracks[1].devices.len(), 1);
    assert_eq!(compiled.tracks[0].instrument_latency, Some(5));
    assert_eq!(compiled.tracks[1].instrument_latency, Some(0));
    for (index, clip) in clips.iter().enumerate() {
        assert_eq!(
            compiled.tracks[index].instrument.as_ref().unwrap().spans[0].clip,
            *clip
        );
        assert_eq!(compiled.tracks[index].clips.len(), 1);
    }
    assert_eq!(compiled.tracks[1].source_receiver_index(), Some(0));
}
#[test]
fn receiver_none_preserves_authored_midi_without_any_generator() {
    let (mut f, source, _) = shared_fixture();
    f.editor
        .execute(Command::SetNativeSourceReceiver {
            source,
            track: None,
        })
        .unwrap();
    let compiled = compile(&f);
    assert!(
        compiled
            .signal_graph
            .as_ref()
            .unwrap()
            .tracks
            .iter()
            .all(|track| track.generator.is_none())
    );
    assert!(
        f.editor
            .project()
            .tracks()
            .iter()
            .all(|track| track.midi_clips()[0].notes().len() == 1)
    );
}
/// Pulse #4188: delay history copies by typed endpoints. A graph maps from
/// itself, but dropping a latent generator moves the arrivals behind it, so
/// its history must not map in either direction.
#[test]
fn history_mapping_refuses_a_removed_latent_generator() {
    let (mut f, source, _) = shared_fixture();
    let bound = compile(&f);
    let bound = bound.signal_graph.as_ref().unwrap();
    assert!(bound.pre_main_history_maps_from(bound));
    f.editor
        .execute(Command::SetNativeSourceReceiver {
            source,
            track: None,
        })
        .unwrap();
    let unbound = compile(&f);
    let unbound = unbound.signal_graph.as_ref().unwrap();
    assert!(unbound.tracks.iter().all(|track| track.generator.is_none()));
    assert!(!unbound.pre_main_compatible(bound));
    assert!(!unbound.pre_main_history_maps_from(bound));
    assert!(!bound.pre_main_history_maps_from(unbound));
}
#[test]
fn duplicate_selected_root_for_one_shared_source_is_refused() {
    let (mut f, _, _) = shared_fixture();
    f.editor
        .execute(Command::SetMidiSource {
            source: Some(f.tracks[0]),
            destination: f.tracks[1],
        })
        .unwrap();
    assert_eq!(
        crate::compile::validate_source_midi_roots(f.editor.project()),
        Err(Invalid("shared source has ambiguous duplicate midi roots"))
    );
}

#[test]
fn provider_constructor_runs_once_per_source_in_each_preparation_boundary() {
    let (f, _, _) = shared_fixture();
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&InstrumentDefinition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    INSTRUMENT_PREPARATIONS.with(|count| count.set(0));
    CONTEXT_PREPARATIONS.with(|count| count.set(0));
    let contexts = crate::shared_analytic::SharedAnalyticContextTable::prepare(
        f.editor.project(),
        &registry,
        None,
    )
    .unwrap();
    assert_eq!(INSTRUMENT_PREPARATIONS.with(std::cell::Cell::get), 1);
    assert_eq!(CONTEXT_PREPARATIONS.with(std::cell::Cell::get), 1);
    assert_eq!(contexts.owners().count(), 1);
    let providers =
        crate::compile::prepare_source_instruments(f.editor.project(), &registry, &contexts, None)
            .unwrap();
    assert_eq!(providers.len(), 1);
    assert_eq!(INSTRUMENT_PREPARATIONS.with(std::cell::Cell::get), 2);
}

#[test]
fn aggregate_arrangement_density_cannot_multiply_the_source_voice_budget() {
    let (mut f, _, clips) = shared_fixture();
    for clip in clips {
        for pitch in 0..32 {
            f.editor
                .execute(Command::AddMidiNote {
                    clip,
                    start: BeatTime::ZERO,
                    duration: BeatTime::ONE,
                    pitch,
                    velocity: 100,
                    channel: 0,
                })
                .unwrap();
        }
    }
    assert!(matches!(
        try_compile(&f),
        Err(Invalid(
            "shared source exceeds its aggregate voice capacity"
        ))
    ));
}

#[test]
fn solo_contributor_keeps_receiver_audible_without_receiver_local_content() {
    let (mut f, _, _) = shared_fixture();
    f.editor
        .execute(Command::ToggleSolo {
            track: f.tracks[1],
            exclusive: false,
        })
        .unwrap();
    let compiled = compile(&f);
    let receiver = &compiled.tracks[0];
    let contributor = &compiled.tracks[1];
    assert!(!receiver.mix.muted);
    assert!(!receiver.source_input_enabled());
    assert!(!receiver.audio_content_enabled());
    assert!(contributor.source_input_enabled());
    assert!(contributor.audio_content_enabled());
    assert!(!contributor.mix.muted);
    f.editor
        .execute(Command::Mute {
            track: f.tracks[0],
            muted: true,
        })
        .unwrap();
    assert!(compile(&f).tracks[0].mix.muted);
}

#[test]
fn mixed_arrangement_and_console_share_one_combined_voice_budget() {
    let (mut f, _, clips) = shared_fixture();
    for pitch in 0..31 {
        f.editor
            .execute(Command::AddMidiNote {
                clip: clips[0],
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch,
                velocity: 100,
                channel: 0,
            })
            .unwrap();
    }
    let Created::Scene(scene) = f
        .editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    for (index, track) in f.tracks.iter().copied().enumerate() {
        let Created::SessionClip(clip) = f
            .editor
            .execute(Command::CreateSessionMidiClip {
                track,
                scene,
                name: "console".into(),
                source_length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("console clip")
        };
        for pitch in 0..if index == 0 { 1 } else { 32 } {
            f.editor
                .execute(Command::AddMidiNote {
                    clip,
                    start: BeatTime::ZERO,
                    duration: BeatTime::ONE,
                    pitch,
                    velocity: 100,
                    channel: 0,
                })
                .unwrap();
        }
    }
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&InstrumentDefinition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let inventory = crate::PreparedSessionInventory::prepare_with_media_and_instrument_registry(
        f.editor.project(),
        &f.media,
        Arc::new(SineTable::new()),
        None,
        &registry,
    )
    .unwrap();
    // Either inventory alone uses 33 voices. Their independently selectable
    // contributor maxima use exactly the one source's 64-voice capacity.
    inventory
        .validate_shared_source_capacity(f.editor.project(), &compile(&f))
        .unwrap();
    f.editor
        .execute(Command::AddMidiNote {
            clip: clips[0],
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 31,
            velocity: 100,
            channel: 0,
        })
        .unwrap();
    let arrangement = compile(&f);
    assert_eq!(
        inventory.validate_shared_source_capacity(f.editor.project(), &arrangement),
        Err(Invalid(
            "shared source exceeds its combined arrangement and console capacity"
        ))
    );
}

#[test]
fn solo_shared_contributor_admits_receiver_group_without_receiver_local_content() {
    let (mut f, _, _) = shared_fixture();
    let Created::Track(group) = f
        .editor
        .execute(Command::GroupTracks {
            tracks: hex_project::TrackSelectionIds::new(&[f.tracks[0]]).unwrap(),
            name: "receiver group".into(),
        })
        .unwrap()
    else {
        panic!("group")
    };
    f.editor
        .execute(Command::ToggleSolo {
            track: f.tracks[1],
            exclusive: false,
        })
        .unwrap();
    let compiled = compile(&f);
    let receiver = compiled
        .tracks
        .iter()
        .find(|track| track.canonical_track_id() == Some(f.tracks[0]))
        .unwrap();
    let group = compiled
        .tracks
        .iter()
        .find(|track| track.canonical_track_id() == Some(group))
        .unwrap();
    assert!(!group.mix.muted);
    assert!(!receiver.mix.muted);
    assert!(!receiver.source_input_enabled());
    assert!(!receiver.audio_content_enabled());
}
