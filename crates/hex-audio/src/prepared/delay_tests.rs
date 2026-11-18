//! Production registered delay through prepared callback publication; no OS audio.
use super::*;
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{
    BeatTime, Command, Created, Editor, Project, TrackDestination, TrackSelectionIds,
};
fn graph(project: &Project) -> Playback {
    let revision = Arc::new(
        hex_engine::PreparedRevision::prepare(
            project,
            &std::collections::BTreeMap::new(),
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    Playback::try_from_prepared_with_session(revision).unwrap()
}
fn block(callback: &mut Callback, output: &mut [Frame]) {
    let measured = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(output).unwrap();
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
}
fn editor_with_delay() -> (Editor, hex_project::TrackId, hex_project::DeviceId) {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "delay".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("track")
    };
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
            pitch: 60,
            channel: 1,
            velocity: 100,
        })
        .unwrap();
    let d = &hex_devices::delay::DEFINITION;
    let mut initial = d.default_state().unwrap();
    for id in [hex_devices::delay::LEFT_SYNC, hex_devices::delay::FILTER] {
        initial = d
            .patch_parameter(&initial, id, ParameterValue::Bool(false))
            .unwrap();
    }
    let state = d
        .patch_parameter(
            &initial,
            hex_devices::delay::TIME,
            ParameterValue::Float(10.0),
        )
        .unwrap();
    let state = d
        .patch_parameter(
            &state,
            hex_devices::delay::DRY_WET,
            ParameterValue::Float(1.0),
        )
        .unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: state.clone(),
        })
        .unwrap()
    else {
        panic!("delay")
    };
    (editor, track, device)
}

#[test]
fn fenced_live_retained_receiver_restart_installs_at_current_cursor_without_allocations() {
    let (mut editor, receiver, _) = editor_with_delay();
    let source = editor
        .project()
        .track(receiver)
        .unwrap()
        .source_id()
        .unwrap();
    let Created::Track(peer) = editor
        .execute(Command::AddMidiTrack {
            name: "peer".into(),
        })
        .unwrap()
    else {
        panic!("peer")
    };
    editor
        .execute(Command::BindTrackSource {
            track: peer,
            source: Some(source),
        })
        .unwrap();
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track: peer,
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
            pitch: 64,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let ordinary = editor.project().clone();
    let mut snapshot = ordinary.snapshot();
    snapshot.retained_source_paths.push(receiver.value());
    let retained = Project::from_snapshot(snapshot).unwrap();
    for (from, to) in [(&ordinary, &retained), (&retained, &ordinary)] {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let revision = control
            .publish_with_policy_and_context_generation(
                graph(from),
                PublicationPolicy::InstallFreshRuntime,
                7,
            )
            .unwrap();
        let mut output = [[0.0; 2]; 127];
        block(&mut callback, &mut output);
        control.command(PlaybackCommand::Play).unwrap();
        let mut old_audible = false;
        for _ in 0..8 {
            block(&mut callback, &mut output);
            old_audible |= output.iter().flatten().any(|sample| sample.abs() > 1e-6);
        }
        assert!(
            old_audible,
            "the predecessor must have active delay history"
        );
        let mut unfenced = graph(to);
        assert!(unfenced.request_live_restart());
        let refused = control
            .publish_with_policy_and_context_generation(
                unfenced,
                PublicationPolicy::InheritRuntime,
                7,
            )
            .unwrap();
        block(&mut callback, &mut output);
        assert_ne!(refused, revision);
        assert_eq!(control.status.applied_revision.load(Acquire), revision);
        assert!(control.status.playing.load(Acquire));
        control.reclaim();
        let gate = control
            .request_live_arrangement_edit_fence(7, revision)
            .unwrap();
        block(&mut callback, &mut output);
        assert_eq!(control.arrangement_edit_playing(gate), Some(true));
        let mut next = graph(to);
        assert!(next.request_live_restart());
        let installed = control
            .publish_with_policy_and_context_generation(
                next,
                PublicationPolicy::InheritUnderArrangement(gate),
                7,
            )
            .unwrap();
        let before = control.status.position.load(Acquire);
        block(&mut callback, &mut output);
        assert_eq!(control.status.applied_revision.load(Acquire), installed);
        assert_eq!(control.status.position.load(Acquire), before + 127);
        assert!(control.status.playing.load(Acquire));
        assert_eq!(
            output, [[0.0; 2]; 127],
            "old delay history must not be inherited"
        );
        control.release_stopped_edit_fence(gate);
        let mut peer_audible = false;
        for _ in 0..8 {
            block(&mut callback, &mut output);
            peer_audible |= output.iter().flatten().any(|sample| sample.abs() > 1e-6);
        }
        assert!(
            peer_audible,
            "the linked peer must resume through the fresh source"
        );
        control.stop();
        block(&mut callback, &mut output);
        control.request_shutdown();
        block(&mut callback, &mut output);
        control.reclaim();

        // A priority Stop that races the ready live fence remains effective.
        let (mut control, mut callback) = channel(48_000).unwrap();
        let revision = control
            .publish_with_policy_and_context_generation(
                graph(from),
                PublicationPolicy::InstallFreshRuntime,
                7,
            )
            .unwrap();
        block(&mut callback, &mut output);
        control.command(PlaybackCommand::Play).unwrap();
        block(&mut callback, &mut output);
        let stale = control
            .request_live_arrangement_edit_fence(7, revision)
            .unwrap();
        block(&mut callback, &mut output);
        control.release_stopped_edit_fence(stale);
        let mut refused = graph(to);
        assert!(refused.request_live_restart());
        assert!(
            control
                .publish_with_policy_and_context_generation(
                    refused,
                    PublicationPolicy::InheritUnderArrangement(stale),
                    7,
                )
                .is_err()
        );
        assert_eq!(control.status.applied_revision.load(Acquire), revision);
        assert!(control.status.playing.load(Acquire));
        let cancelled = control
            .request_live_arrangement_edit_fence(7, revision)
            .unwrap();
        block(&mut callback, &mut output);
        let mut pending = graph(to);
        assert!(pending.request_live_restart());
        let cancelled_revision = control
            .publish_with_policy_and_context_generation(
                pending,
                PublicationPolicy::InheritUnderArrangement(cancelled),
                7,
            )
            .unwrap();
        control.release_stopped_edit_fence(cancelled);
        block(&mut callback, &mut output);
        assert_ne!(cancelled_revision, revision);
        assert_eq!(control.status.applied_revision.load(Acquire), revision);
        assert!(control.status.playing.load(Acquire));
        control.reclaim();
        let gate = control
            .request_live_arrangement_edit_fence(7, revision)
            .unwrap();
        block(&mut callback, &mut output);
        let mut next = graph(to);
        assert!(next.request_live_restart());
        let installed = control
            .publish_with_policy_and_context_generation(
                next,
                PublicationPolicy::InheritUnderArrangement(gate),
                7,
            )
            .unwrap();
        control.stop();
        block(&mut callback, &mut output);
        assert_eq!(control.status.applied_revision.load(Acquire), installed);
        assert!(!control.status.playing.load(Acquire));
        assert_eq!(control.status.position.load(Acquire), 0);
        control.release_stopped_edit_fence(gate);
        control.request_shutdown();
        block(&mut callback, &mut output);
        control.reclaim();

        // A stopped candidate installs fresh under the stopped fence at its
        // captured cursor, without inheriting the old source history.
        let (mut control, mut callback) = channel(48_000).unwrap();
        let mut old = graph(from);
        old.apply(PlaybackCommand::Seek(1234));
        let revision = control
            .publish_with_policy_and_context_generation(
                old,
                PublicationPolicy::InstallFreshRuntime,
                7,
            )
            .unwrap();
        block(&mut callback, &mut output);
        let gate = control.request_arrangement_edit_fence(7, revision).unwrap();
        block(&mut callback, &mut output);
        assert_eq!(control.arrangement_edit_position(gate), Some(1234));
        let mut next = graph(to);
        next.apply(PlaybackCommand::Seek(1234));
        let installed = control
            .publish_with_policy_and_context_generation(
                next,
                PublicationPolicy::InstallUnderStoppedArrangement(gate),
                7,
            )
            .unwrap();
        block(&mut callback, &mut output);
        assert_eq!(control.status.applied_revision.load(Acquire), installed);
        assert_eq!(control.status.position.load(Acquire), 1234);
        assert!(!control.status.playing.load(Acquire));
        control.release_stopped_edit_fence(gate);
        control.request_shutdown();
        block(&mut callback, &mut output);
        control.reclaim();
    }
}

#[test]
fn retained_receiver_reopened_from_storage_renders_peer_without_callback_allocations() {
    let (mut editor, receiver, _) = editor_with_delay();
    let source = editor
        .project()
        .track(receiver)
        .unwrap()
        .source_id()
        .unwrap();
    let Created::Track(peer) = editor
        .execute(Command::AddMidiTrack {
            name: "peer".into(),
        })
        .unwrap()
    else {
        panic!("peer")
    };
    editor
        .execute(Command::BindTrackSource {
            track: peer,
            source: Some(source),
        })
        .unwrap();
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track: peer,
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
            pitch: 64,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let mut snapshot = editor.project().snapshot();
    snapshot.retained_source_paths.push(receiver.value());
    let retained = Project::from_snapshot(snapshot).unwrap();
    let bytes = hex_storage::encode(&retained).unwrap();
    let reopened = hex_storage::decode(&bytes).unwrap();
    assert_eq!(reopened.snapshot(), retained.snapshot());
    let (mut control, mut callback) = channel(48_000).unwrap();
    control.publish(graph(&reopened)).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 127];
    let mut audible = false;
    for _ in 0..12 {
        block(&mut callback, &mut output);
        audible |= output.iter().flatten().any(|sample| sample.abs() > 1e-6);
    }
    assert!(
        audible,
        "the peer remains audible through the retained source"
    );
    control.stop();
    block(&mut callback, &mut output);
    control.request_shutdown();
    block(&mut callback, &mut output);
    control.reclaim();
}

#[test]
fn play_from_chases_vector_with_fresh_delay_without_callback_allocations() {
    let (mut editor, track, _) = editor_with_delay();
    let device = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let operator = &hex_devices::operator_state::OPERATOR_METADATA;
    let state = operator
        .patch_parameter(
            &operator.default_state().unwrap(),
            hex_devices::operator_state::parameter::feedback(1),
            ParameterValue::Float(0.3),
        )
        .unwrap();
    editor
        .execute(Command::ReplaceDeviceState { device, state })
        .unwrap();
    let (mut control, mut callback) = channel(48_000).unwrap();
    control.publish(graph(editor.project())).unwrap();
    let mut output = [[0.0; 2]; 127];
    block(&mut callback, &mut output);
    for destination in [0, 12_000, 12_000, 23_000, 48_000_000] {
        control
            .command(PlaybackCommand::PlayFrom(destination))
            .unwrap();
        // Includes actual command consumption, voice/effect reset, held-note
        // chase and rendering, with all construction outside the probe.
        block(&mut callback, &mut output);
        assert_eq!(control.status.position.load(Acquire), destination + 127);
        assert_eq!(output, [[0.0; 2]; 127], "old delay history must reset");
        let mut audible = output.iter().flatten().any(|v| v.abs() > 1e-6);
        for _ in 0..5 {
            block(&mut callback, &mut output);
            audible |= output.iter().flatten().any(|v| v.abs() > 1e-6);
        }
        assert_eq!(audible, destination < 24_000);
        control.command(PlaybackCommand::Pause).unwrap();
        let paused = control.status.position.load(Acquire);
        block(&mut callback, &mut output);
        // The 128-frame queued Pause fade is still rendering (#4151).
        assert_eq!(control.status.position.load(Acquire), paused + 127);
        // Pause releases clip gates while the same source and its effect tails
        // remain available to live input. Stop below still resets to silence.
        assert!(output.iter().flatten().all(|sample| sample.is_finite()));
        assert_eq!(
            output.iter().flatten().any(|sample| sample.abs() > 1e-6),
            destination < 24_000,
            "active notes keep their bounded release/delay tail; an after-song start stays silent",
        );
    }
    control.stop();
    block(&mut callback, &mut output);
    assert_eq!(output, [[0.0; 2]; 127]);
    control.request_shutdown();
    block(&mut callback, &mut output);
    control.reclaim();
}
#[test]
fn delay_prepared_publication_parameter_handoff_tail_retirement_and_stop_allocate_nothing() {
    let (mut editor, _, device) = editor_with_delay();
    let d = &hex_devices::delay::DEFINITION;
    let state = editor.project().device(device).unwrap().state().clone();
    let (mut control, mut callback) = channel(48000).unwrap();
    control.publish(graph(editor.project())).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 127];
    let mut audible = false;
    for _ in 0..12 {
        block(&mut callback, &mut output);
        audible |= output.iter().flatten().any(|v| v.abs() > 1e-6);
    }
    assert!(audible);
    let changed = d
        .patch_parameter(
            &state,
            hex_devices::delay::TIME,
            ParameterValue::Float(20.0),
        )
        .unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: changed,
        })
        .unwrap();
    control.publish(graph(editor.project())).unwrap();
    block(&mut callback, &mut output); // actual full-history handoff is measured too
    assert!(output.iter().flatten().all(|v| v.is_finite()));
    control.reclaim();
    let frozen = d
        .patch_parameter(
            editor.project().device(device).unwrap().state(),
            hex_devices::delay::FREEZE,
            ParameterValue::Bool(true),
        )
        .unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: frozen,
        })
        .unwrap();
    control.publish(graph(editor.project())).unwrap();
    block(&mut callback, &mut output);
    control.reclaim();
    editor.execute(Command::RemoveDevice(device)).unwrap();
    control.publish(graph(editor.project())).unwrap();
    for _ in 0..4 {
        block(&mut callback, &mut output);
    }
    control.stop();
    block(&mut callback, &mut output);
    assert_eq!(output, [[0.0; 2]; 127]);
    control.command(PlaybackCommand::Seek(0)).unwrap();
    block(&mut callback, &mut output);
    control.reclaim();
    control.request_shutdown();
    block(&mut callback, &mut output);
    control.reclaim();
}

#[test]
fn repeated_solo_toggles_with_live_delay_apply_without_retaining_mix_only_tails() {
    for session in [false, true] {
        solo_toggles(session);
    }
}
fn solo_toggles(session: bool) {
    let (mut editor, track, _) = editor_with_delay();
    let launch = session.then(|| {
        let Created::Scene(scene) = editor
            .execute(Command::AddScene {
                name: "scene".into(),
            })
            .unwrap()
        else {
            panic!("scene")
        };
        let source = editor.project().track(track).unwrap().midi_clips()[0].id();
        editor
            .execute(Command::CopyArrangementMidiToSession { source, scene })
            .unwrap();
        scene
    });
    let Created::Track(solo_track) = editor
        .execute(Command::AddTrack {
            name: "solo".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let (mut control, mut callback) = channel(48000).unwrap();
    control.publish(graph(editor.project())).unwrap();
    if let Some(scene) = launch {
        control
            .launch_and_play_with_ticket(hex_engine::SessionPlaybackCommand::LaunchScene {
                scene,
                quantization: hex_engine::LaunchQuantization::Immediate,
            })
            .unwrap();
    } else {
        control.command(PlaybackCommand::Play).unwrap();
    }
    let mut output = [[0.0; 2]; 127];
    for _ in 0..12 {
        block(&mut callback, &mut output);
    }
    assert!(output.iter().flatten().any(|v| v.abs() > 1e-6));
    for iteration in 0..130 {
        editor
            .execute(Command::ToggleSolo {
                track: solo_track,
                exclusive: true,
            })
            .unwrap();
        let revision = control.publish(graph(editor.project())).unwrap();
        block(&mut callback, &mut output);
        assert_eq!(
            control.status.applied_revision.load(Acquire),
            revision,
            "solo toggle {iteration} did not apply"
        );
        assert!(control.status.rejected_revision().is_none());
        if iteration % 2 == 0 {
            assert_eq!(
                output, [[0.0; 2]; 127],
                "solo must mute the existing delay output"
            );
        }
        control.reclaim();
    }
    control.stop();
    block(&mut callback, &mut output);
    assert_eq!(output, [[0.0; 2]; 127]);
    control.request_shutdown();
    block(&mut callback, &mut output);
    control.reclaim();
}

#[test]
fn changing_a_silent_routed_track_preserves_compensated_callback_audio() {
    for (partition, grouped) in [(1, false), (257, false), (1, true), (257, true)] {
        let (mut editor, track, _) = editor_with_delay();
        editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: hex_devices::limiter::DEFINITION.default_state().unwrap(),
            })
            .unwrap();
        let Created::ReturnTrack(bus) = editor
            .execute(Command::AddReturnTrack { name: "bus".into() })
            .unwrap()
        else {
            panic!("return");
        };
        editor
            .execute(Command::SetReturnSend {
                track,
                return_track: bus,
                gain: 0.5,
            })
            .unwrap();
        let group = grouped.then(|| {
            let Created::Track(group) = editor
                .execute(Command::GroupTracks {
                    tracks: TrackSelectionIds::new(&[track]).unwrap(),
                    name: "group".into(),
                })
                .unwrap()
            else {
                panic!("group");
            };
            let Created::Track(destination) = editor
                .execute(Command::AddTrack {
                    name: "feed".into(),
                })
                .unwrap()
            else {
                panic!("feed destination");
            };
            editor
                .execute(Command::SetAudioFeed {
                    source: hex_project::RoutingOwner::Track(track),
                    destination,
                    gain: 0.25,
                })
                .unwrap();
            group
        });
        let (mut control, mut callback) = channel(48_000).unwrap();
        let (mut reference_control, mut reference) = channel(48_000).unwrap();
        let original = graph(editor.project());
        assert!(original.main_output_latency_frames() > 0);
        control.publish(original).unwrap();
        reference_control.publish(graph(editor.project())).unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        reference_control.command(PlaybackCommand::Play).unwrap();

        let mut output = [[0.0; 2]; 257];
        let mut expected = output;
        for _ in 0..8 {
            block(&mut callback, &mut output);
            block(&mut reference, &mut expected);
        }
        assert!(expected.iter().flatten().any(|v| v.abs() > 1e-6));
        let Created::Track(empty) = editor
            .execute(Command::AddTrack {
                name: "empty".into(),
            })
            .unwrap()
        else {
            panic!("track");
        };
        let route = if let Some(group) = group {
            Command::ReparentTracks {
                tracks: TrackSelectionIds::new(&[empty]).unwrap(),
                destination: TrackDestination {
                    parent_group: Some(group),
                    before: None,
                },
            }
        } else {
            Command::SetReturnSend {
                track: empty,
                return_track: bus,
                gain: 0.75,
            }
        };
        editor.execute(route).unwrap();
        for removed in [false, true] {
            if removed {
                editor.execute(Command::RemoveTrack(empty)).unwrap();
            }
            let revision = control.publish(graph(editor.project())).unwrap();
            for (actual, expected) in output
                .chunks_mut(partition)
                .zip(expected.chunks_mut(partition))
            {
                block(&mut callback, actual);
                block(&mut reference, expected);
                assert_eq!(
                    actual, expected,
                    "partition {partition}, grouped {grouped}, removed {removed}"
                );
            }
            assert_eq!(control.status.applied_revision.load(Acquire), revision);
            assert!(control.status.rejected_revision().is_none());
            assert!(control.status.playing.load(Acquire));
            control.reclaim();
        }
    }
}

#[test]
fn track_mix_edits_preserve_delay_history_under_new_gain_and_balance() {
    let (mut editor, track, _) = editor_with_delay();
    let (mut control, mut callback) = channel(48000).unwrap();
    let (mut reference_control, mut reference) = channel(48000).unwrap();
    control.publish(graph(editor.project())).unwrap();
    reference_control.publish(graph(editor.project())).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    reference_control.command(PlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 127];
    let mut expected = output;
    for _ in 0..12 {
        block(&mut callback, &mut output);
        block(&mut reference, &mut expected);
    }
    assert_eq!(output, expected);
    let mut current_gain = [1.0_f32; 2];
    for (gain, balance) in [(0.5, 0.0), (0.25, -1.0), (0.75, 1.0), (1.0, 0.0)] {
        editor.execute(Command::Gain { track, gain }).unwrap();
        editor.execute(Command::Balance { track, balance }).unwrap();
        let revision = control.publish(graph(editor.project())).unwrap();
        block(&mut callback, &mut output);
        block(&mut reference, &mut expected);
        assert_eq!(control.status.applied_revision.load(Acquire), revision);
        let left = gain * if balance > 0.0 { 1.0 - balance } else { 1.0 };
        let right = gain * if balance < 0.0 { 1.0 + balance } else { 1.0 };
        for (index, (actual, source)) in output.iter().zip(&expected).enumerate() {
            let remaining = (240 - index) as f32;
            for (channel, target) in [left, right].into_iter().enumerate() {
                current_gain[channel] += (target - current_gain[channel]) / remaining;
                assert!((actual[channel] - source[channel] * current_gain[channel]).abs() < 1e-6);
            }
        }
        control.reclaim();
    }
    for (control, callback) in [
        (&mut control, &mut callback),
        (&mut reference_control, &mut reference),
    ] {
        control.request_shutdown();
        block(callback, &mut output);
        control.reclaim();
    }
}
