use super::*;

#[test]
fn unchanged_blocks_do_not_rescan_or_rewrite_route_slots() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    assert!(control.publish(session_playback(101)).is_ok());
    callback.begin_block();
    let sequence = control.status.route_sequence.load(SeqCst);
    let slot_visits = callback.route_slot_visits;
    let mut output = [[0.0; 2]; 128];
    callback.render(&mut output).unwrap();
    assert_eq!(control.status.route_sequence.load(SeqCst), sequence);
    assert_eq!(callback.route_slot_visits, slot_visits);
    assert_eq!(slot_visits, MAX_TRACKS);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn deferred_route_candidate_keeps_active_snapshot_until_acceptance() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    assert!(control.publish(session_playback(101)).is_ok());
    callback.begin_block();
    let mut routes = [SessionRouteEntry::default(); 1];
    let active = control.status.session_routes(&mut routes).unwrap();
    assert_eq!(active.audio_revision, 1);
    assert_eq!(routes[0].track_id, 101);

    assert!(control.publish(session_playback(202)).is_ok());
    callback.test_transient_handoff_refusals = 1;
    callback.begin_block();
    let deferred = control.status.session_routes(&mut routes).unwrap();
    assert_eq!(deferred.audio_revision, 1);
    assert_eq!(routes[0].track_id, 101);
    assert_eq!(control.status.applied_revision.load(Acquire), 1);

    callback.begin_block();
    let accepted = control.status.session_routes(&mut routes).unwrap();
    assert_eq!(accepted.audio_revision, 2);
    assert_eq!(routes[0].track_id, 202);
    assert_eq!(control.status.applied_revision.load(Acquire), 2);
    control.reclaim();
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn concurrent_route_reader_never_observes_mixed_revision_or_track_list() {
    use std::sync::atomic::AtomicBool;
    use std::thread;

    let (mut control, mut callback) = channel(48_000).unwrap();
    let status = Arc::clone(&control.status);
    let done = Arc::new(AtomicBool::new(false));
    let reader_done = Arc::clone(&done);
    let reader = thread::spawn(move || {
        let mut entries = [SessionRouteEntry {
            track_id: 0,
            owner: SessionRouteOwner::Silence,
            active_origin: None,
            audio_launch_frame: None,
            pending: None,
        }; 1];
        let mut observed = 0;
        while !reader_done.load(Acquire) {
            let Some(snapshot) = status.session_routes(&mut entries) else {
                continue;
            };
            if snapshot.audio_revision == 0 {
                assert_eq!(snapshot.count, 0);
                assert_eq!(snapshot.capability, SessionCapabilityStatus::Legacy);
            } else {
                assert_eq!(snapshot.count, 1);
                assert_eq!(snapshot.capability, SessionCapabilityStatus::Available);
                let expected_track = if snapshot.audio_revision % 2 == 1 {
                    101
                } else {
                    202
                };
                assert_eq!(entries[0].track_id, expected_track);
            }
            observed += 1;
        }
        observed
    });

    for revision in 1..=64 {
        let track_id = if revision % 2 == 1 { 101 } else { 202 };
        assert_eq!(
            control
                .publish(session_playback(track_id))
                .unwrap_or_else(|error| panic!("{}", error.reason)),
            revision
        );
        callback.begin_block();
        control.reclaim();
    }
    done.store(true, Release);
    assert!(reader.join().unwrap() > 0);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[cfg(feature = "allocation-probe")]
#[test]
fn contextual_monitor_input_rebind_keeps_keys_across_edits_and_fresh_seek_without_history() {
    use hex_engine::computer_keyboard::ComputerKeyboardInput;
    use hex_project::{Command, Created, Editor, Project};
    // An explicit generation zero is legitimate; the legacy default zero is unknown.
    for (old_context, next_context, fresh, retained) in [
        (Some(0), Some(0), false, true),
        (Some(7), Some(7), true, true),
        (None, None, false, false),
        (Some(7), Some(8), false, false),
    ] {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "keys".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let input = Arc::new(ComputerKeyboardInput::default());
        input.set_key(0, Some((60, 100, 1))).unwrap();
        let audition = Arc::new(hex_engine::tuning_audition::TuningAuditionInput::new(
            old_context.unwrap_or(0),
        ));
        audition.set_key(0, Some((64, 100, 2))).unwrap();
        let graph = |project: &Project| {
            let revision = Arc::new(
                hex_engine::PreparedRevision::prepare(
                    project,
                    &std::collections::BTreeMap::new(),
                    Arc::new(hex_engine::SineTable::new()),
                    None,
                )
                .unwrap(),
            );
            let mut playback = Playback::try_from_prepared_with_session(revision).unwrap();
            playback
                .prepare_computer_keyboard(project, track, Arc::clone(&input))
                .unwrap();
            playback
                .prepare_tuning_audition(project, Arc::clone(&audition))
                .unwrap();
            playback
        };
        let (mut control, mut callback) = channel(48_000).unwrap();
        let first = graph(editor.project());
        match old_context {
            Some(generation) => control
                .publish_with_policy_and_context_generation(
                    first,
                    PublicationPolicy::InheritRuntime,
                    generation,
                )
                .unwrap(),
            None => control.publish(first).unwrap(),
        };
        let mut output = [[0.0; 2]; 64];
        callback.begin_block();
        callback.render(&mut output).unwrap();
        assert!(output.iter().flatten().any(|sample| sample.abs() > 1e-6));
        editor.execute(Command::Gain { track, gain: 0.7 }).unwrap();
        let next = graph(editor.project());
        let policy = if fresh {
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(
                control.capture_fresh_runtime_fence().unwrap(),
            )
        } else {
            PublicationPolicy::InheritRuntime
        };
        let revision = match next_context {
            Some(generation) => control
                .publish_with_policy_and_context_generation(next, policy, generation)
                .unwrap(),
            None => control.publish_with_policy(next, policy).unwrap(),
        };
        // Queue pressure returns the candidate and cannot detach the audible input.
        let refused = control.publish(graph(editor.project())).unwrap_err();
        drop(refused);
        assert!(!input.is_cancelled());
        let measured = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
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
        if retained {
            assert_eq!(control.status.applied_revision.load(Acquire), revision);
            assert!(!input.is_cancelled());
        } else {
            // Without a matching input-context generation, the callback
            // cannot transfer source-owned live IDs safely. Keep the old
            // graph and input installed rather than retiring their only
            // shared DSP owner or pretending the new revision applied.
            assert_eq!(control.status.applied_revision.load(Acquire), 1);
            assert_eq!(
                control.status.rejected_revision(),
                Some((revision, PlaybackHandoffRejection::SessionRuntime)),
                "context={old_context:?}->{next_context:?}, fresh={fresh}"
            );
            assert!(!input.is_cancelled());
        }
        // Normal inheritance retains the existing certified audition continuation. Fresh
        // runtime installation instead rebinds only its input after the final fence cut.
        if fresh {
            assert!(!audition.is_cancelled());
        }
        assert!(output.iter().flatten().any(|sample| sample.abs() > 1e-6));
        control.reclaim();
        control.stop();
        callback.begin_block();
        assert!(input.is_cancelled());
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }
}

#[cfg(feature = "allocation-probe")]
#[test]
fn computer_keyboard_callback_publication_polyphony_release_pressure_stop_and_retirement_allocate_nothing()
 {
    use hex_engine::computer_keyboard::ComputerKeyboardInput;
    use hex_project::{Command, Created, Editor, Project};
    for (routed, mutable) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "keys".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().expect("fixture instrument state"),
            })
            .unwrap()
        else {
            panic!("track")
        };
        if mutable {
            let class = hex_engine::NativeDeviceClass::Instrument;
            let state = hex_engine::default_native_device_state_for_class(
                class,
                hex_engine::DeviceTypeId::from_static("hex.instrument.operator"),
            )
            .unwrap();
            let state = hex_engine::patch_native_device_parameter_for_class(
                class,
                &state,
                hex_lattice_contract::ParameterId::from_static(78),
                hex_lattice_contract::ParameterValue::Float(0.4),
            )
            .unwrap();
            let device = editor
                .project()
                .track(track)
                .unwrap()
                .instrument()
                .unwrap()
                .id();
            editor
                .execute(Command::ReplaceDeviceState { device, state })
                .unwrap();
        }
        if routed {
            let Created::Track(bus) = editor
                .execute(Command::AddTrack { name: "bus".into() })
                .unwrap()
            else {
                panic!("bus")
            };
            editor
                .execute(Command::SetAudioFeed {
                    source: hex_project::RoutingOwner::Track(track),
                    destination: bus,
                    gain: 0.5,
                })
                .unwrap();
        }
        let revision = Arc::new(
            hex_engine::PreparedRevision::prepare(
                editor.project(),
                &std::collections::BTreeMap::new(),
                Arc::new(hex_engine::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let input = Arc::new(ComputerKeyboardInput::default());
        input.set_key(0, Some((60, 100, 1))).unwrap();
        input.set_key(1, Some((64, 80, 2))).unwrap();
        let mut playback = Playback::from_prepared(Arc::clone(&revision));
        playback
            .prepare_computer_keyboard(editor.project(), track, Arc::clone(&input))
            .unwrap();
        let (mut control, mut callback) = channel(48_000).unwrap();
        control.publish(playback).unwrap();
        let mut output = [[0.0; 2]; 256];
        let measure = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        });
        assert_eq!(
            (
                measure.count_total,
                measure.bytes_total,
                measure.count_current,
                measure.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert!(output.iter().flatten().any(|sample| sample.abs() > 1e-6));
        input.set_key(0, None).unwrap();
        input.set_key(1, None).unwrap();
        let measure = allocation_counter::measure(|| {
            for _ in 0..if mutable { 2000 } else { 12 } {
                callback.begin_block();
                callback.render(&mut output).unwrap();
            }
        });
        assert_eq!(
            (
                measure.count_total,
                measure.bytes_total,
                measure.count_current,
                measure.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(output, [[0.0; 2]; 256]);
        input.set_key(0, Some((60, 100, 3))).unwrap();
        callback.begin_block();
        callback.render(&mut output).unwrap();
        for _ in 0..COMMANDS {
            control.command(PlaybackCommand::Pause).unwrap();
        }
        assert!(control.command(PlaybackCommand::Pause).is_err());
        control.stop();
        let measure = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        });
        assert_eq!(
            (
                measure.count_total,
                measure.bytes_total,
                measure.count_current,
                measure.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert!(input.is_cancelled());
        assert_eq!(output, [[0.0; 2]; 256]);
        control.publish(Playback::from_prepared(revision)).unwrap();
        let measure = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        });
        assert_eq!(
            (
                measure.count_total,
                measure.bytes_total,
                measure.count_current,
                measure.bytes_current
            ),
            (0, 0, 0, 0)
        );
        control.reclaim();
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }
}

#[test]
fn native_epoch_successor_accepts_same_or_newer_revision_in_same_generation() {
    let ctx = |project_generation, editor_revision| {
        Some(hex_engine::AudioSourceContext {
            project_generation,
            editor_revision,
        })
    };
    assert!(native_epoch_accepts_successor(ctx(1, 15), ctx(1, 15)));
    assert!(native_epoch_accepts_successor(ctx(1, 15), ctx(1, 16)));
    assert!(!native_epoch_accepts_successor(ctx(1, 15), ctx(1, 14)));
    assert!(!native_epoch_accepts_successor(ctx(1, 15), ctx(2, 16)));
    assert!(!native_epoch_accepts_successor(None, ctx(1, 15)));
    assert!(!native_epoch_accepts_successor(ctx(1, 15), None));
}
