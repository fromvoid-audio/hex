use super::*;

#[test]
fn wide_analytic_release_and_span_storage_stays_within_derived_fixed_bound() {
    let sampler_bytes = std::mem::size_of::<hex_lattice::PreparedAnalyticNote>();
    assert_eq!(sampler_bytes, 352);
    let added_sampler_bytes = sampler_bytes - 40;
    let release_capacity = super::SESSION_RELEASE_CAPACITY;
    let release_entries_bytes = release_capacity
        .checked_mul(std::mem::size_of::<super::ReleaseEntry>())
        .expect("release entry byte count is bounded")
        .checked_mul(2)
        .expect("paired release entry byte count is bounded");
    let span_buffer_bytes = std::mem::size_of::<MidiSpanBuffer>();
    let current_bytes = release_entries_bytes
        .checked_add(span_buffer_bytes)
        .expect("fixed MIDI storage byte count is bounded");
    let shared_owner_slots = crate::shared_analytic::MAX_RETAINED_SHARED_CONTEXTS
        .checked_mul(std::mem::size_of::<
            Option<Arc<crate::shared_analytic::SharedContextOwner>>,
        >())
        .expect("shared context slot table is bounded");
    let lease_plan_bytes = crate::shared_analytic::MAX_RETAINED_SHARED_CONTEXTS
        .checked_mul(std::mem::size_of::<crate::shared_analytic::ContextLeasePlan>())
        .expect("shared context remap plan is bounded");
    let scoped_bytes = current_bytes
        .checked_add(shared_owner_slots)
        .and_then(|bytes| bytes.checked_add(lease_plan_bytes))
        .expect("shared-context playback scratch is bounded");
    assert!(scoped_bytes <= 18 * 1024 * 1024);
    assert!(scoped_bytes.checked_mul(2).unwrap() <= 36 * 1024 * 1024);
    assert_eq!(
        crate::shared_analytic::MAX_RETAINED_SHARED_CONTEXT_BYTES,
        2 * 1024 * 1024
    );
    assert_eq!(
        crate::shared_analytic::MAX_RETAINED_SHARED_CONTEXTS,
        SESSION_RELEASE_CAPACITY
    );
    #[cfg(target_pointer_width = "64")]
    {
        // The power fade adds 16 bytes to each bounded release entry.
        // No per-entry allocation; the 18/36 MiB scoped ceilings stay unchanged.
        assert_eq!(std::mem::size_of::<super::ReleaseEntry>(), 544);
        assert_eq!(span_buffer_bytes, 82_952 + 192 * 8);
        assert_eq!(current_bytes, 17_910_280);
        assert_eq!(shared_owner_slots, 131_072);
        assert_eq!(lease_plan_bytes, 393_216);
        assert_eq!(scoped_bytes, 18_434_568);
    }
    let previous_release_entry_size = std::mem::size_of::<super::ReleaseEntry>()
        .checked_sub(added_sampler_bytes)
        .expect("previous sampler fits the entry");
    let previous_span_buffer_size = span_buffer_bytes
        .checked_sub(crate::midi_template::MIDI_SPAN_BUFFER_CAPACITY * added_sampler_bytes)
        .expect("previous sampler fits the span buffer");
    let previous_bytes = release_capacity
        .checked_mul(previous_release_entry_size)
        .and_then(|bytes| bytes.checked_mul(2))
        .and_then(|bytes| bytes.checked_add(previous_span_buffer_size))
        .expect("previous fixed MIDI storage byte count is bounded");
    let expected_growth = release_capacity
        .checked_mul(added_sampler_bytes)
        .and_then(|bytes| bytes.checked_mul(2))
        .and_then(|bytes| {
            bytes.checked_add(crate::midi_template::MIDI_SPAN_BUFFER_CAPACITY * added_sampler_bytes)
        })
        .expect("fixed MIDI storage growth is bounded");
    assert_eq!(current_bytes - previous_bytes, expected_growth);
}

#[test]
fn post_node_observation_preserves_pcm_and_exact_loop_render_offsets() {
    use hex_lattice_contract::DeviceTypeId;

    let table = Arc::new(hex_devices::SineTable::new());
    let make_playback = |looping: bool| {
        let mut snapshot = project(0.5, 0, 8 << 32).snapshot();
        let state = crate::default_native_device_state_for_class(
            crate::NativeDeviceClass::Effect,
            DeviceTypeId::from_static("hex.eq8"),
        )
        .unwrap();
        snapshot.tracks[0].devices.push(DeviceSnapshot {
            modulation: Default::default(),
            parent: None,
            automation: Vec::new(),
            comparison: None,
            id: 7,
            bypassed: false,
            state,
        });
        snapshot.next_id = snapshot.next_id.max(8);
        if looping {
            snapshot.loop_region = Some(LoopRegion::new(0, 64, true).unwrap());
        } else {
            snapshot.tempo_points = vec![
                TempoPoint::new(BeatTime::ZERO, Tempo::DEFAULT),
                TempoPoint::new(
                    // The tempo transition falls within the first 256-frame
                    // render chunk; it must not alter the observed render path.
                    BeatTime::from_ratio(1, 100).unwrap(),
                    Tempo::new(90_000_000).unwrap(),
                ),
            ];
        }
        let project = legacy_project_from_snapshot(snapshot).unwrap();
        let prepared = revision(&project, Arc::clone(&table));
        let mut playback = Playback::try_from_prepared_with_session(prepared).unwrap();
        playback.apply(PlaybackCommand::Play);
        playback
    };

    for (looping, expected_chunks) in [
        (
            true,
            vec![
                (0, 64),
                (64, 64),
                (128, 64),
                (192, 64),
                (256, 64),
                (320, 64),
                (384, 64),
                (448, 64),
            ],
        ),
        (false, vec![(0, 256), (256, 256)]),
    ] {
        let mut ordinary = make_playback(looping);
        let mut observed = make_playback(looping);
        let mut ordinary_output = [[0.0; 2]; 512];
        let mut observed_output = [[0.0; 2]; 512];
        let mut calls = Vec::new();
        ordinary.render(&mut ordinary_output).unwrap();
        observed
            .render_with_post_node_observer(
                &mut observed_output,
                Some(&mut |identity, offset, frames| {
                    if identity.device_type.as_str() == Ok("hex.eq8") {
                        calls.push((identity, offset, frames.len()));
                    }
                }),
            )
            .unwrap();

        assert_eq!(observed_output, ordinary_output);
        let chunks: Vec<_> = calls
            .iter()
            .map(|(_, offset, length)| (*offset, *length))
            .collect();
        assert_eq!(chunks, expected_chunks);
        assert!(calls.iter().all(|(identity, _, _)| {
            identity.owner == hex_lattice::ProcessorChainOwner::Track(1) && identity.device_id == 7
        }));
        let mut paired = make_playback(looping);
        let mut paired_output = [[0.0; 2]; 512];
        let mut paired_calls = Vec::new();
        paired
            .render_with_node_signal_observer(
                &mut paired_output,
                Some(&mut |identity: crate::PostNodeIdentity,
                           phase: crate::NodeSignalPhase,
                           offset: usize,
                           frames: &[Frame]| {
                    assert_eq!(identity.owner, hex_lattice::ProcessorChainOwner::Track(1));
                    assert_eq!(identity.device_id, 7);
                    paired_calls.push((phase, offset, frames.len()));
                }),
            )
            .unwrap();
        assert_eq!(paired_output, ordinary_output);
        let expected_pairs: Vec<_> = expected_chunks
            .iter()
            .flat_map(|&(offset, count)| {
                [
                    (crate::NodeSignalPhase::Input, offset, count),
                    (crate::NodeSignalPhase::Output, offset, count),
                ]
            })
            .collect();
        assert_eq!(paired_calls, expected_pairs);
    }
}

#[test]
fn post_node_observation_covers_return_chains_bypass_and_session_rendering() {
    use crate::{PlaybackAction, SessionPlaybackCommand};
    use hex_lattice_contract::DeviceTypeId;
    use hex_project::{ReturnTrackSnapshot, SendLevelSnapshot};

    let table = Arc::new(hex_devices::SineTable::new());
    let eq8 = crate::default_native_device_state_for_class(
        crate::NativeDeviceClass::Effect,
        DeviceTypeId::from_static("hex.eq8"),
    )
    .unwrap();
    let make_return_project = |bypassed: bool| {
        let mut snapshot = project(0.5, 0, 8 << 32).snapshot();
        snapshot.tracks[0].sends.push(SendLevelSnapshot {
            return_track: 70,
            gain: 0.5,
        });
        snapshot.return_tracks.push(ReturnTrackSnapshot {
            id: 70,
            name: "return".into(),
            gain: 1.0,
            muted: false,
            devices: vec![DeviceSnapshot {
                modulation: Default::default(),
                parent: None,
                automation: Vec::new(),
                comparison: None,
                id: 80,
                bypassed,
                state: eq8.clone(),
            }],
        });
        snapshot.next_id = snapshot.next_id.max(81);
        legacy_project_from_snapshot(snapshot).unwrap()
    };
    let return_identity = crate::PostNodeIdentity::new(
        hex_lattice::ProcessorChainOwner::Return(70),
        80,
        DeviceTypeId::from_static("hex.eq8"),
    );

    let render_return = |bypassed: bool| {
        let project = make_return_project(bypassed);
        let mut playback =
            Playback::try_from_prepared_with_session(revision(&project, Arc::clone(&table)))
                .unwrap();
        playback.apply(PlaybackCommand::Play);
        let mut output = [[0.0; 2]; 256];
        let mut calls = Vec::new();
        playback
            .render_with_post_node_observer(
                &mut output,
                Some(&mut |identity, offset, frames| {
                    calls.push((identity, offset, frames.len()));
                }),
            )
            .unwrap();
        let mut paired =
            Playback::try_from_prepared_with_session(revision(&project, Arc::clone(&table)))
                .unwrap();
        paired.apply(PlaybackCommand::Play);
        let mut paired_output = [[0.0; 2]; 256];
        let mut signals = Vec::new();
        paired
            .render_with_node_signal_observer(
                &mut paired_output,
                Some(&mut |identity: crate::PostNodeIdentity,
                           phase: crate::NodeSignalPhase,
                           offset: usize,
                           frames: &[Frame]| {
                    if identity == return_identity {
                        signals.push((phase, offset, frames.to_vec()));
                    }
                }),
            )
            .unwrap();
        assert_eq!(paired_output, output);
        assert_eq!(signals.len(), 2);
        assert_eq!(signals[0].0, crate::NodeSignalPhase::Input);
        assert_eq!(signals[1].0, crate::NodeSignalPhase::Output);
        assert_eq!(signals[0].1, signals[1].1);
        if bypassed {
            assert_eq!(signals[0].2, signals[1].2);
        }
        calls
    };

    let active_return_calls = render_return(false);
    assert!(
        active_return_calls
            .iter()
            .any(|(identity, _, _)| *identity == return_identity)
    );
    let bypassed_return_calls = render_return(true);
    // A bypassed processor still owns a post-node boundary: the runtime
    // advances its bypass state and exposes the dry signal at that point.
    assert!(
        bypassed_return_calls
            .iter()
            .any(|(identity, _, _)| *identity == return_identity)
    );

    let project = make_return_project(false);
    let clip = match project.session_cells()[0].content() {
        hex_project::SessionCellContent::Midi(clip) => clip.id(),
        _ => panic!("fixture session cell must be MIDI"),
    };
    let prepared = revision(&project, table);
    let mut playback = Playback::try_from_prepared_with_session(prepared).unwrap();
    playback
        .try_apply_action(PlaybackAction::Session(
            SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: crate::LaunchQuantization::Immediate,
            },
        ))
        .unwrap();
    playback
        .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Play))
        .unwrap();
    let mut output = [[0.0; 2]; 256];
    let mut calls = Vec::new();
    playback
        .render_with_post_node_observer(
            &mut output,
            Some(&mut |identity, offset, frames| {
                calls.push((identity, offset, frames.len()));
            }),
        )
        .unwrap();
    assert!(
        calls
            .iter()
            .any(|(identity, _, _)| *identity == return_identity)
    );
}

#[test]
fn removed_track_release_retains_its_original_device_matrix() {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter as utility_parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    use hex_project::{Command, DeviceSnapshot, Editor};

    let table = Arc::new(hex_devices::SineTable::new());
    let base = project(0.5, 0, 8 << 32);
    let mut snapshot = base.snapshot();
    snapshot.tracks[0].content = TrackContentSnapshot::Instrument(Vec::new());
    let state = UTILITY_DEFINITION.default_state().unwrap();
    let state = UTILITY_DEFINITION
        .patch_parameter(
            &state,
            utility_parameter::INVERT_RIGHT,
            ParameterValue::Bool(true),
        )
        .unwrap();
    snapshot.tracks[0].devices.push(DeviceSnapshot {
        modulation: Default::default(),
        parent: None,
        automation: Vec::new(),
        comparison: None,
        id: 10,
        bypassed: false,
        state,
    });
    snapshot.next_id = 11;
    let old_project = legacy_project_from_snapshot(snapshot).unwrap();
    let mut old = SessionPlayback::new(revision(&old_project, Arc::clone(&table))).unwrap();
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut before = [[0.0; 2]; 64];
    old.render(&mut before).unwrap();

    let mut editor = Editor::new(old_project, 8).unwrap();
    let track_id = editor.project().tracks()[0].id();
    editor.execute(Command::RemoveTrack(track_id)).unwrap();
    let mut next = SessionPlayback::new(revision(editor.project(), Arc::clone(&table))).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut tail = [[0.0; 2]; 64];
    next.render(&mut tail).unwrap();
    assert!(tail.iter().any(|frame| frame[0].abs() > 1.0e-5));
    assert!(tail.iter().all(|frame| frame[1] == -frame[0]));
}

#[test]
fn ongoing_track_release_keeps_its_original_device_matrix_across_revision() {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter as utility_parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    use hex_project::DeviceSnapshot;

    let table = Arc::new(hex_devices::SineTable::new());
    let base = project(0.5, 0, 8 << 32);
    let mut old_snapshot = base.snapshot();
    old_snapshot.tracks[0].content = TrackContentSnapshot::Instrument(Vec::new());
    let state = UTILITY_DEFINITION.default_state().unwrap();
    let state = UTILITY_DEFINITION
        .patch_parameter(
            &state,
            utility_parameter::INVERT_RIGHT,
            ParameterValue::Bool(true),
        )
        .unwrap();
    old_snapshot.tracks[0].devices.push(DeviceSnapshot {
        modulation: Default::default(),
        parent: None,
        automation: Vec::new(),
        comparison: None,
        id: 10,
        bypassed: false,
        state,
    });
    old_snapshot.next_id = 11;
    let old_project = legacy_project_from_snapshot(old_snapshot).unwrap();
    let mut old = SessionPlayback::new(revision(&old_project, Arc::clone(&table))).unwrap();
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut before = [[0.0; 2]; 64];
    old.render(&mut before).unwrap();

    let mut new_snapshot = old_project.snapshot();
    let previous_state = &new_snapshot.tracks[0].devices[0].state;
    new_snapshot.tracks[0].devices[0].state = UTILITY_DEFINITION
        .patch_parameter(
            previous_state,
            utility_parameter::INVERT_RIGHT,
            ParameterValue::Bool(false),
        )
        .unwrap();
    let new_project = legacy_project_from_snapshot(new_snapshot).unwrap();
    let mut next = SessionPlayback::new(revision(&new_project, Arc::clone(&table))).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut tail = [[0.0; 2]; 64];
    next.render(&mut tail).unwrap();
    assert!(tail.iter().any(|frame| frame[0].abs() > 1.0e-5));
    assert!(tail.iter().all(|frame| frame[1] == -frame[0]));
}

#[test]
fn removed_track_release_runs_only_the_captured_instrument_suffix() {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter as utility_parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    use hex_project::{Command, DeviceSnapshot, Editor};

    let table = Arc::new(hex_devices::SineTable::new());
    let mut snapshot = project(0.5, 0, 8 << 32).snapshot();
    snapshot.tracks[0].content = TrackContentSnapshot::Instrument(Vec::new());
    let prefix = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            utility_parameter::INVERT_LEFT,
            ParameterValue::Bool(true),
        )
        .unwrap();
    let suffix = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            utility_parameter::CHANNEL,
            ParameterValue::Choice(3), // swap
        )
        .unwrap();
    snapshot.tracks[0].devices = vec![
        DeviceSnapshot {
            modulation: Default::default(),
            parent: None,
            automation: Vec::new(),
            comparison: None,
            id: 10,
            bypassed: false,
            state: prefix,
        },
        DeviceSnapshot {
            modulation: Default::default(),
            parent: None,
            automation: Vec::new(),
            comparison: None,
            id: 11,
            bypassed: false,
            state: suffix,
        },
    ];
    snapshot.tracks[0].instrument_position = Some(1);
    snapshot.next_id = 12;
    let old_project = legacy_project_from_snapshot(snapshot).unwrap();
    let mut old = SessionPlayback::new(revision(&old_project, Arc::clone(&table))).unwrap();
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut before = [[0.0; 2]; 64];
    old.render(&mut before).unwrap();

    let mut editor = Editor::new(old_project, 8).unwrap();
    let track_id = editor.project().tracks()[0].id();
    editor.execute(Command::RemoveTrack(track_id)).unwrap();
    let mut next = SessionPlayback::new(revision(editor.project(), Arc::clone(&table))).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut tail = [[0.0; 2]; 64];
    next.render(&mut tail).unwrap();
    let audible: Vec<_> = tail
        .iter()
        .filter(|frame| frame[0].abs() > 1.0e-5 || frame[1].abs() > 1.0e-5)
        .collect();
    assert!(!audible.is_empty());
    // The captured suffix swaps stereo. Applying the old prefix would invert
    // only the left channel before that swap and make the channels differ.
    assert!(audible.iter().all(|frame| frame[0] == frame[1]));
}

#[test]
fn same_revision_session_release_enters_at_the_instrument_gap() {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter as utility_parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    use hex_project::DeviceSnapshot;

    let table = Arc::new(hex_devices::SineTable::new());
    let mut snapshot = project(0.5, 0, 8 << 32).snapshot();
    snapshot.tracks[0].content = TrackContentSnapshot::Instrument(Vec::new());
    let prefix = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            utility_parameter::INVERT_LEFT,
            ParameterValue::Bool(true),
        )
        .unwrap();
    let suffix = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            utility_parameter::CHANNEL,
            ParameterValue::Choice(3),
        )
        .unwrap();
    snapshot.tracks[0].devices = vec![
        DeviceSnapshot {
            modulation: Default::default(),
            parent: None,
            automation: Vec::new(),
            comparison: None,
            id: 10,
            bypassed: false,
            state: prefix,
        },
        DeviceSnapshot {
            modulation: Default::default(),
            parent: None,
            automation: Vec::new(),
            comparison: None,
            id: 11,
            bypassed: false,
            state: suffix,
        },
    ];
    snapshot.tracks[0].instrument_position = Some(1);
    snapshot.next_id = 12;
    let project = legacy_project_from_snapshot(snapshot).unwrap();
    let mut playback = SessionPlayback::new(revision(&project, table)).unwrap();
    let clip = playback.revision.session().midi_clips()[0].id;
    let track = playback.routes[0].id;
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut before = [[0.0; 2]; 64];
    playback.render(&mut before).unwrap();
    playback
        .apply(SessionPlaybackCommand::StopTrack {
            track,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    let mut tail = [[0.0; 2]; 64];
    playback.render(&mut tail).unwrap();
    let audible: Vec<_> = tail
        .iter()
        .filter(|frame| frame[0].abs() > 1.0e-5 || frame[1].abs() > 1.0e-5)
        .collect();
    assert!(!audible.is_empty());
    // A same-revision release joins after the invert-left prefix, then the
    // current swap suffix runs once. Seeing equal channels catches prefix
    // reprocessing or a second suffix pass.
    assert!(audible.iter().all(|frame| frame[0] == frame[1]));
}

#[test]
fn outgoing_session_release_keeps_the_old_stateful_filter_settings() {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    let table = Arc::new(hex_devices::SineTable::new());
    let mut snapshot = project(0.5, 0, 8 << 32).snapshot();
    snapshot.tracks[0].content = TrackContentSnapshot::Instrument(Vec::new());
    let neutral = UTILITY_DEFINITION.default_state().unwrap();
    let state = UTILITY_DEFINITION
        .patch_parameter(&neutral, parameter::DC_REMOVE, ParameterValue::Bool(true))
        .unwrap();
    snapshot.tracks[0].devices = vec![
        hex_project::DeviceSnapshot {
            modulation: Default::default(),
            parent: None,
            automation: Vec::new(),
            comparison: None,
            id: snapshot.next_id,
            bypassed: false,
            state: neutral,
        },
        hex_project::DeviceSnapshot {
            modulation: Default::default(),
            parent: None,
            automation: Vec::new(),
            comparison: None,
            id: snapshot.next_id + 1,
            bypassed: false,
            state,
        },
    ];
    snapshot.next_id += 2;
    // Put DC removal in the suffix so the outgoing Session voice must keep and
    // execute the captured old stateful processor after the instrument.
    snapshot.tracks[0].instrument_position = Some(1);
    let old_project = legacy_project_from_snapshot(snapshot).unwrap();
    let old_revision = revision(&old_project, Arc::clone(&table));
    let clip = old_revision.session().midi_clips()[0].id;

    let render_old_tail = |new_project: &Project| {
        let mut previous = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
        previous
            .apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        previous.apply(SessionPlaybackCommand::Play).unwrap();
        let mut prefix = [[0.0; 2]; 64];
        previous.render(&mut prefix).unwrap();

        let mut next = SessionPlayback::new(revision(new_project, Arc::clone(&table))).unwrap();
        next.try_inherit_state_from(&mut previous).unwrap();
        let mut tail = [[0.0; 2]; 64];
        next.render(&mut tail).unwrap();
        tail
    };

    let stopped = without_session_cells(&old_project);
    let current_dc = with_utility_filter_state_at(&stopped, 1, true, false);
    let current_bass = with_utility_filter_state_at(&stopped, 1, false, true);
    // The candidate route is silent. Its new device configuration must not
    // change outgoing voices, which continue through the captured old chain.
    let old_settings_tail = render_old_tail(&current_dc);
    let new_settings_tail = render_old_tail(&current_bass);
    assert!(
        old_settings_tail
            .iter()
            .flatten()
            .any(|sample| sample.abs() > 1.0e-6)
    );
    assert_eq!(old_settings_tail, new_settings_tail);
}

#[test]
fn outgoing_arrangement_release_keeps_the_old_stateful_suffix() {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    let table = Arc::new(hex_devices::SineTable::new());
    let mut snapshot = project(0.5, 0, 8 << 32).snapshot();
    let neutral = UTILITY_DEFINITION.default_state().unwrap();
    let dc_state = UTILITY_DEFINITION
        .patch_parameter(&neutral, parameter::DC_REMOVE, ParameterValue::Bool(true))
        .unwrap();
    snapshot.tracks[0].devices = vec![
        DeviceSnapshot {
            modulation: Default::default(),
            parent: None,
            automation: Vec::new(),
            comparison: None,
            id: snapshot.next_id,
            bypassed: false,
            state: neutral,
        },
        DeviceSnapshot {
            modulation: Default::default(),
            parent: None,
            automation: Vec::new(),
            comparison: None,
            id: snapshot.next_id + 1,
            bypassed: false,
            state: dc_state,
        },
    ];
    snapshot.next_id += 2;
    snapshot.tracks[0].instrument_position = Some(1);
    let old_project = legacy_project_from_snapshot(snapshot).unwrap();
    let old_revision = revision(&old_project, Arc::clone(&table));

    let render_old_tail = |new_project: &Project| {
        let mut previous = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
        previous.apply(SessionPlaybackCommand::Play).unwrap();
        let mut prefix = [[0.0; 2]; 64];
        previous.render(&mut prefix).unwrap();
        assert!(prefix.iter().any(|frame| frame[0].abs() > 1.0e-6));

        let mut next = SessionPlayback::new(revision(new_project, Arc::clone(&table))).unwrap();
        assert!(!next.revision.arrangement().tracks()[0].instrument_enabled());
        assert!(
            !old_revision.arrangement().tracks()[0]
                .instrument_config_matches(&next.revision.arrangement().tracks()[0])
        );
        next.try_inherit_state_from(&mut previous).unwrap();
        assert!(next.releases.iter().any(|entry| {
            entry.track_id == next.routes[0].id && entry.location.tail_group().is_some()
        }));
        let mut tail = [[0.0; 2]; 64];
        next.render(&mut tail).unwrap();
        tail
    };

    let stopped = without_session_cells(&old_project);
    let mut disabled_snapshot = stopped.snapshot();
    disabled_snapshot.tracks[0]
        .instrument
        .as_mut()
        .unwrap()
        .bypassed = true;
    // Keep the current route's own chain output out of the oracle. This leaves
    // the captured release group as the only audible owner after the cut.
    disabled_snapshot.tracks[0].muted = true;
    let disabled = legacy_project_from_snapshot(disabled_snapshot).unwrap();
    let current_dc = with_utility_filter_state_at(&disabled, 1, true, false);
    let current_bass = with_utility_filter_state_at(&disabled, 1, false, true);
    let old_settings_tail = render_old_tail(&current_dc);
    let new_settings_tail = render_old_tail(&current_bass);
    assert!(
        old_settings_tail
            .iter()
            .flatten()
            .any(|sample| sample.abs() > 1.0e-6)
    );
    assert_eq!(old_settings_tail, new_settings_tail);
    // Detaching a live voice mid-fader ramp captures the audible gain, not
    // the target that the old track has not reached yet.
    let registry = hex_devices::native_instrument_registry().unwrap();
    let mut previous = ordinary_mutable_playback(&old_project, &registry);
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    previous.render(&mut [[0.0; 2]; 64]).unwrap();
    let mut fading_snapshot = old_project.snapshot();
    fading_snapshot.tracks[0].gain = 0.0;
    let fading_project = legacy_project_from_snapshot(fading_snapshot).unwrap();
    let mut fading = ordinary_mutable_playback(&fading_project, &registry);
    fading.try_inherit_state_from(&mut previous).unwrap();
    fading.render(&mut [[0.0; 2]; 64]).unwrap();
    let mut detached_snapshot = disabled.snapshot();
    detached_snapshot.tracks[0].gain = 0.0;
    let detached_project = legacy_project_from_snapshot(detached_snapshot).unwrap();
    let mut detached = ordinary_mutable_playback(&detached_project, &registry);
    detached.try_inherit_state_from(&mut fading).unwrap();
    let release = detached
        .releases
        .iter()
        .find(|entry| {
            entry.location.tail_group().is_some()
                && entry
                    .source_origin
                    .is_some_and(|origin| !origin.follows_current_configuration)
        })
        .unwrap();
    let boundary_gain = 0.5 * (1.0 - 64.0 / 240.0);
    assert!((release.mix.channel_gain()[0] - boundary_gain).abs() < 1e-6);
}

#[test]
fn detached_arrangement_effect_suffix_renders_after_its_source_ends() {
    let table = Arc::new(hex_devices::SineTable::new());
    let mut snapshot = audio_project(0, 32, 4).snapshot();
    snapshot.tracks[0].content = TrackContentSnapshot::Audio(vec![ClipSnapshot {
        active: true,
        name: "clip".into(),
        flavor: hex_project::ClipFlavor::default(),
        edit: None,
        gain: hex_project::ClipGain::UNITY,
        id: 6,
        asset: 4,
        start: 0,
        duration: 32,
        source_offset: 0,
        source_length: 32,
        looping: false,
        source_phase: 0,
        fade: ClipFade::neutral(32).unwrap(),
    }]);
    snapshot.next_id = 7;
    let source_project = legacy_project_from_snapshot(snapshot).unwrap();
    let old_project = with_utility_filter_state(&source_project, true, false);
    let mut bypassed_snapshot = old_project.snapshot();
    bypassed_snapshot.tracks[0].devices[0].bypassed = true;
    let bypassed_project = legacy_project_from_snapshot(bypassed_snapshot).unwrap();
    let changed_configuration = with_utility_filter_state(&old_project, false, true);
    let mut unchanged_snapshot = old_project.snapshot();
    unchanged_snapshot.tracks[0].name = "renamed".into();
    let unchanged_project = legacy_project_from_snapshot(unchanged_snapshot).unwrap();
    let mut changed_mix_snapshot = old_project.snapshot();
    changed_mix_snapshot.tracks[0].gain = 0.25;
    let changed_mix_project = legacy_project_from_snapshot(changed_mix_snapshot).unwrap();

    let media = audio_media(
        &old_project,
        &crate::Audio::new(48_000, vec![[0.3, -0.1]; 32]).unwrap(),
        &crate::Audio::new(48_000, vec![[0.0, 0.0]; 32]).unwrap(),
    );
    let prepare = |project: &Project| {
        Arc::new(PreparedRevision::prepare(project, &media, Arc::clone(&table), None).unwrap())
    };
    let old_revision = prepare(&old_project);

    // Each transition owns the same two real playback values, but their
    // lifetime is one helper call. Keeping all four pairs in this debug test
    // frame exceeded the ordinary test-thread stack before rendering.
    fn start_old(revision: Arc<PreparedRevision>) -> SessionPlayback {
        let mut playback = SessionPlayback::new(revision).unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        let mut prefix = [[0.0; 2]; 32];
        playback.render(&mut prefix).unwrap();
        assert!(prefix.iter().any(|frame| frame[0].abs() > 1.0e-6));
        assert!(!playback.device_chains[0].committed_range_is_quiescent(0..1));
        playback
    }
    fn reference_output(revision: Arc<PreparedRevision>) -> [Frame; 256] {
        let mut reference = start_old(revision);
        let mut expected = [[0.0; 2]; 256];
        let mut offset = 0;
        for size in [1, 7, 31, 64, 153] {
            reference
                .render(&mut expected[offset..offset + size])
                .unwrap();
            offset += size;
        }
        assert_eq!(offset, expected.len());
        expected
    }
    fn transition_output(
        old: Arc<PreparedRevision>,
        candidate: Arc<PreparedRevision>,
        detached: bool,
    ) -> [Frame; 256] {
        let mut previous = start_old(old);
        let mut current = SessionPlayback::new(candidate).unwrap();
        current.try_inherit_state_from(&mut previous).unwrap();
        assert_eq!(
            current.tail_groups.iter().flatten().count(),
            usize::from(detached)
        );
        if detached {
            assert!(current.tail_output_active[0]);
        }
        let mut output = [[0.0; 2]; 256];
        current.render(&mut output).unwrap();
        output
    }

    let expected = reference_output(Arc::clone(&old_revision));
    // A non-device edit preserves the same owner; history transfers once to
    // the candidate and does not create a second retained old suffix.
    assert_eq!(
        transition_output(
            Arc::clone(&old_revision),
            prepare(&unchanged_project),
            false
        ),
        expected
    );
    // Bypass fades this same residual owner to dry (the source has ended).
    // It must not detach an independently audible, indefinitely retained suffix.
    let bypassed = transition_output(Arc::clone(&old_revision), prepare(&bypassed_project), false);
    for (index, (actual, reference)) in bypassed.iter().zip(expected).enumerate() {
        let wet = 1.0 - (index + 1).min(240) as f32 / 240.0;
        for channel in 0..2 {
            assert!((actual[channel] - reference[channel] * wet).abs() < 1e-6);
        }
    }
    // A compatible runtime inherits filter history for outgoing residual audio
    // without also copying that old-configuration history into the new chain.
    assert_eq!(
        transition_output(
            Arc::clone(&old_revision),
            prepare(&changed_configuration),
            true
        ),
        expected
    );
    // Mix-only edits keep the same filter history under the new gain. They
    // must not capture an independently audible old-mix suffix.
    let changed = transition_output(
        Arc::clone(&old_revision),
        prepare(&changed_mix_project),
        false,
    );
    for (index, (actual, reference)) in changed.iter().zip(expected).enumerate() {
        let gain = 1.0 - 0.75 * (index + 1).min(240) as f32 / 240.0;
        for channel in 0..2 {
            assert!((actual[channel] - reference[channel] * gain).abs() < 1e-6);
        }
    }

    // Device removal mid-ramp freezes the old suffix at the gain that
    // actually reached the output, rather than jumping to the target.
    let mut zero_snapshot = old_project.snapshot();
    zero_snapshot.tracks[0].gain = 0.0;
    let zero = legacy_project_from_snapshot(zero_snapshot.clone()).unwrap();
    zero_snapshot.tracks[0].devices.clear();
    let detached = legacy_project_from_snapshot(zero_snapshot).unwrap();
    let mut previous = start_old(old_revision);
    let mut fading = SessionPlayback::new(prepare(&zero)).unwrap();
    fading.try_inherit_state_from(&mut previous).unwrap();
    let mut during = [[0.0; 2]; 64];
    fading.render(&mut during).unwrap();
    assert!(during.iter().flatten().any(|sample| sample.abs() > 1e-6));
    let mut after = SessionPlayback::new(prepare(&detached)).unwrap();
    after.try_inherit_state_from(&mut fading).unwrap();
    assert!(after.tail_output_active[0]);
    let mut tail = [[0.0; 2]; 64];
    after.render(&mut tail).unwrap();
    let boundary_gain = 1.0 - 64.0 / 240.0;
    for (actual, reference) in tail.iter().zip(&expected[64..128]) {
        for channel in 0..2 {
            assert!((actual[channel] - reference[channel] * boundary_gain).abs() < 1e-6);
        }
    }
}

#[test]
fn eq8_parameter_edits_inherit_arrangement_effect_history_without_retaining_old_suffix() {
    use hex_devices::eq8::parameter as eq8_parameter;
    use hex_lattice::NativeDeviceDefinition;
    use hex_lattice_contract::ParameterValue;

    let table = Arc::new(hex_devices::SineTable::new());
    let make_project = |gain_db: f32, q: f32, kind: u32| {
        let mut snapshot = project(0.5, 0, 8 << 32).snapshot();
        let mut state = crate::default_native_device_state_for_class(
            crate::NativeDeviceClass::Effect,
            hex_lattice_contract::DeviceTypeId::from_static("hex.eq8"),
        )
        .unwrap();
        for parameter in [eq8_parameter::enabled(0, 0), eq8_parameter::enabled(1, 0)] {
            state =
                crate::patch_native_device_parameter(&state, parameter, ParameterValue::Bool(true))
                    .unwrap();
        }
        for (parameter, value) in [
            (eq8_parameter::gain(0, 0), ParameterValue::Float(gain_db)),
            (eq8_parameter::q(0, 0), ParameterValue::Float(q)),
            (eq8_parameter::kind(0, 0), ParameterValue::Choice(kind)),
        ] {
            state = crate::patch_native_device_parameter(&state, parameter, value).unwrap();
        }
        snapshot.tracks[0].devices.push(DeviceSnapshot {
            modulation: Default::default(),
            parent: None,
            automation: Vec::new(),
            comparison: None,
            id: 7,
            bypassed: false,
            state: state.clone(),
        });
        snapshot.next_id = snapshot.next_id.max(8);
        (legacy_project_from_snapshot(snapshot).unwrap(), state)
    };
    let mut source_snapshot = project(0.5, 0, 8 << 32).snapshot();
    source_snapshot.master_gain = 1.0;
    let source_project = legacy_project_from_snapshot(source_snapshot).unwrap();
    let mut source = SessionPlayback::new(revision(&source_project, Arc::clone(&table))).unwrap();
    source.apply(SessionPlaybackCommand::Play).unwrap();

    let (old_project, old_state) = make_project(3.0, 1.0, 0);
    let mut old = SessionPlayback::new(revision(&old_project, Arc::clone(&table))).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let old_prepared = hex_devices::eq8::EQ8_DEFINITION
        .prepare_runtime_factory(
            &old_state,
            &hex_lattice::DevicePreparationContext {
                sample_rate: 48_000,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap()
        .unwrap();
    let hex_lattice::PreparedProcessorFactory::Owned(old_owned) = old_prepared else {
        panic!("expected owned EQ8 factory");
    };
    let mut reference_runtime = old_owned.create_runtime().unwrap();
    let mut input = [[0.0; 2]; 128];
    let mut expected = input;
    let mut output = input;
    source.render(&mut input).unwrap();
    expected.copy_from_slice(&input);
    reference_runtime.begin_block();
    reference_runtime.process_block(&mut expected, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    reference_runtime.commit_block();
    old.render(&mut output).unwrap();
    assert_eq!(output, expected, "initial registered EQ8 PCM oracle");
    assert!(!old.device_chains[0].committed_range_is_quiescent(0..1));

    for (gain_db, q, kind) in [(6.0, 1.0, 0), (6.0, 2.0, 0), (6.0, 2.0, 1)] {
        let (candidate_project, candidate_state) = make_project(gain_db, q, kind);
        let mut candidate =
            SessionPlayback::new(revision(&candidate_project, Arc::clone(&table))).unwrap();
        candidate.try_inherit_state_from(&mut old).unwrap();
        assert_eq!(candidate.tail_groups.iter().flatten().count(), 0);
        assert!(!candidate.device_chains[0].committed_range_is_quiescent(0..1));
        let prepared = hex_devices::eq8::EQ8_DEFINITION
            .prepare_runtime_factory(
                &candidate_state,
                &hex_lattice::DevicePreparationContext {
                    sample_rate: 48_000,
                    tuning: &hex_lattice::TuningTable::default(),
                    resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                },
            )
            .unwrap()
            .unwrap();
        let hex_lattice::PreparedProcessorFactory::Owned(owned) = prepared else {
            panic!("expected owned EQ8 factory");
        };
        let mut next_reference = owned.create_runtime().unwrap();
        assert!(next_reference.can_inherit_from(reference_runtime.as_ref()));
        next_reference.inherit_from(reference_runtime.as_ref());
        input.fill([0.0; 2]);
        expected.fill([0.0; 2]);
        output.fill([0.0; 2]);
        source.render(&mut input).unwrap();
        expected.copy_from_slice(&input);
        next_reference.begin_block();
        next_reference.process_block(&mut expected, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        next_reference.commit_block();
        candidate.render(&mut output).unwrap();
        assert_eq!(output, expected, "retargeted EQ8 parameter PCM oracle");
        assert!(
            output
                .iter()
                .all(|frame| frame.iter().all(|sample| sample.is_finite()))
        );
        assert!(output.iter().any(|frame| frame[0].abs() > 1.0e-6));
        reference_runtime = next_reference;
        old = candidate;
    }
}

#[test]
fn removing_track_keeps_its_unquiescent_arrangement_effect_suffix() {
    let table = Arc::new(hex_devices::SineTable::new());
    let mut snapshot = audio_project(0, 32, 4).snapshot();
    snapshot.tracks[0].content = TrackContentSnapshot::Audio(vec![ClipSnapshot {
        active: true,
        name: "clip".into(),
        flavor: hex_project::ClipFlavor::default(),
        edit: None,
        gain: hex_project::ClipGain::UNITY,
        id: 6,
        asset: 4,
        start: 0,
        duration: 32,
        source_offset: 0,
        source_length: 32,
        looping: false,
        source_phase: 0,
        fade: ClipFade::neutral(32).unwrap(),
    }]);
    snapshot.next_id = 7;
    let project = with_utility_filter_state(
        &legacy_project_from_snapshot(snapshot).unwrap(),
        true,
        false,
    );
    let mut media = BTreeMap::new();
    let asset_id = project.assets().next().unwrap().id();
    media.insert(
        asset_id,
        crate::Audio::new(48_000, vec![[0.3, -0.1]; 32]).unwrap(),
    );
    let table_for_revision = Arc::clone(&table);
    let mut old = SessionPlayback::new(Arc::new(
        PreparedRevision::prepare(&project, &media, table_for_revision, None).unwrap(),
    ))
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 32];
    old.render(&mut prefix).unwrap();
    assert!(prefix.iter().any(|frame| frame[0].abs() > 1.0e-6));

    let mut expected = [[0.0; 2]; 256];
    old.render(&mut expected).unwrap();
    assert!(expected.iter().any(|frame| frame[0].abs() > 1.0e-6));

    let mut previous = SessionPlayback::new(Arc::clone(&old.revision)).unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut previous_prefix = [[0.0; 2]; 32];
    previous.render(&mut previous_prefix).unwrap();

    let mut editor = Editor::new(project, 8).unwrap();
    let track_id = editor.project().tracks()[0].id();
    editor.execute(Command::RemoveTrack(track_id)).unwrap();
    let candidate_revision =
        Arc::new(PreparedRevision::prepare(editor.project(), &media, table, None).unwrap());
    let mut candidate = SessionPlayback::new(candidate_revision).unwrap();
    candidate.try_inherit_state_from(&mut previous).unwrap();
    assert_eq!(candidate.tail_groups.iter().flatten().count(), 1);
    let mut actual = [[0.0; 2]; 256];
    candidate.render(&mut actual).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn utility_mute_handoff_ramps_warmed_filter_and_keeps_its_history() {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    // The existing Utility transition lasts five milliseconds at 48 kHz.
    const RAMP_FRAMES: usize = 240;
    let check_ramp =
        |actual: &[[f32; 2]; RAMP_FRAMES], reference: &[[f32; 2]; RAMP_FRAMES], unmuting: bool| {
            for (index, (actual, reference)) in actual.iter().zip(reference).enumerate() {
                let position = (index + 1) as f32 / RAMP_FRAMES as f32;
                let gain = if unmuting { position } else { 1.0 - position };
                for (actual, reference) in actual.iter().zip(reference) {
                    assert!((actual - reference * gain).abs() < 1.0e-7, "frame {index}");
                }
            }
        };

    for (on_return, dc, bass, audition) in [
        (false, true, false, false),
        (false, false, true, false),
        (false, false, true, true),
        (true, true, false, false),
        (true, false, true, false),
        (true, false, true, true),
    ] {
        let mut snapshot = audio_project(0, 32, 4).snapshot();
        let mut state = UTILITY_DEFINITION.default_state().unwrap();
        for (parameter, value) in [
            (parameter::DC_REMOVE, dc),
            (parameter::BASS_MONO, bass),
            (parameter::BASS_MONO_AUDITION, audition),
        ] {
            state = UTILITY_DEFINITION
                .patch_parameter(&state, parameter, ParameterValue::Bool(value))
                .unwrap();
        }
        let device = DeviceSnapshot {
            modulation: Default::default(),
            parent: None,
            automation: Vec::new(),
            comparison: None,
            id: snapshot.next_id,
            bypassed: false,
            state: state.clone(),
        };
        snapshot.next_id += 1;
        if on_return {
            let return_id = snapshot.next_id;
            snapshot.next_id += 1;
            snapshot.tracks[0]
                .sends
                .push(hex_project::SendLevelSnapshot {
                    return_track: return_id,
                    gain: 1.0,
                });
            snapshot
                .return_tracks
                .push(hex_project::ReturnTrackSnapshot {
                    id: return_id,
                    name: "filtered return".into(),
                    gain: 1.0,
                    muted: false,
                    devices: vec![device],
                });
        } else {
            snapshot.tracks[0].devices.push(device);
        }
        let project = legacy_project_from_snapshot(snapshot).unwrap();
        let table = Arc::new(hex_devices::SineTable::new());
        let asset = project
            .assets()
            .find(|asset| asset.id().value() == 4)
            .unwrap()
            .id();
        let media = BTreeMap::from([(
            asset,
            crate::Audio::new(48_000, vec![[0.3, -0.1]; 32]).unwrap(),
        )]);
        let prepare = |project: &Project| {
            Arc::new(PreparedRevision::prepare(project, &media, Arc::clone(&table), None).unwrap())
        };
        let revision = prepare(&project);
        let clip = revision.session().audio_clips()[0].id;
        let mut previous = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let mut reference = SessionPlayback::new(revision).unwrap();
        for playback in [&mut previous, &mut reference] {
            playback
                .apply(SessionPlaybackCommand::LaunchClip {
                    clip,
                    quantization: LaunchQuantization::Immediate,
                })
                .unwrap();
            playback.apply(SessionPlaybackCommand::Play).unwrap();
            let mut warmup = [[0.0; 2]; 32];
            playback.render(&mut warmup).unwrap();
            assert!(warmup.iter().flatten().any(|sample| sample.abs() > 1.0e-6));
        }

        let mut muted_snapshot = project.snapshot();
        let devices = if on_return {
            &mut muted_snapshot.return_tracks[0].devices
        } else {
            &mut muted_snapshot.tracks[0].devices
        };
        devices[0].state = UTILITY_DEFINITION
            .patch_parameter(&state, parameter::MUTE, ParameterValue::Bool(true))
            .unwrap();
        let muted_project = legacy_project_from_snapshot(muted_snapshot).unwrap();
        let mut muted = SessionPlayback::new(prepare(&muted_project)).unwrap();
        muted.try_inherit_state_from(&mut previous).unwrap();
        let mut transition = [[0.0; 2]; RAMP_FRAMES];
        let mut oracle = [[0.0; 2]; RAMP_FRAMES];
        muted.render(&mut transition).unwrap();
        reference.render(&mut oracle).unwrap();
        assert!(oracle.iter().flatten().any(|sample| sample.abs() > 1.0e-6));
        check_ramp(&transition, &oracle, false);
        let mut output = [[1.0; 2]; 16];
        muted.render(&mut output).unwrap();
        assert_eq!(
            output, [[0.0; 2]; 16],
            "mute retained an audible old filter for return={on_return}, dc={dc}, bass={bass}, audition={audition}"
        );
        assert!(muted.tail_groups.iter().all(Option::is_none));
        // The reference's source has ended, so both filters advance through
        // exactly sixteen zero-input frames. Unmute must recover that history.
        reference.render(&mut [[0.0; 2]; 16]).unwrap();
        let mut unmuted = SessionPlayback::new(prepare(&project)).unwrap();
        unmuted.try_inherit_state_from(&mut muted).unwrap();
        unmuted.render(&mut transition).unwrap();
        reference.render(&mut oracle).unwrap();
        check_ramp(&transition, &oracle, true);
        let mut expected = [[0.0; 2]; 16];
        reference.render(&mut expected).unwrap();
        unmuted.render(&mut output).unwrap();
        assert_eq!(output, expected);
    }
}

#[test]
fn compatible_stateful_history_survives_crossing_the_instrument_gap() {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    use hex_project::Command;

    let table = Arc::new(hex_devices::SineTable::new());
    let mut snapshot = audio_project(0, 32, 4).snapshot();
    let instrument_state = hex_project::default_instrument_state().unwrap();
    let instrument_id = snapshot.next_id;
    snapshot.next_id += 1;
    let device_id = snapshot.next_id;
    snapshot.next_id += 1;
    let dc_state = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            parameter::DC_REMOVE,
            ParameterValue::Bool(true),
        )
        .unwrap();
    snapshot.tracks[0].instrument = Some(DeviceSnapshot {
        modulation: Default::default(),
        parent: None,
        automation: Vec::new(),
        comparison: None,
        id: instrument_id,
        bypassed: false,
        state: instrument_state,
    });
    snapshot.tracks[0].devices = vec![DeviceSnapshot {
        modulation: Default::default(),
        parent: None,
        automation: Vec::new(),
        comparison: None,
        id: device_id,
        bypassed: false,
        state: dc_state,
    }];
    snapshot.tracks[0].instrument_position = Some(1);
    let old_project = legacy_project_from_snapshot(snapshot).unwrap();
    let track_id = old_project.tracks()[0].id();
    let instrument_id = old_project.tracks()[0].instrument().unwrap().id();
    let device_id = old_project.tracks()[0].devices()[0].id();
    let asset_id = old_project.assets().next().unwrap().id();
    let media = BTreeMap::from([(
        asset_id,
        crate::Audio::new(48_000, vec![[0.3, -0.1]; 32]).unwrap(),
    )]);
    let old_revision = Arc::new(
        PreparedRevision::prepare(&old_project, &media, Arc::clone(&table), None).unwrap(),
    );
    let mut previous = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
    let clip = old_revision.session().audio_clips()[0].id;
    previous
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut warmup = [[0.0; 2]; 16];
    let mut reference_warmup = [[0.0; 2]; 16];
    let mut reference = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
    reference
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    reference.apply(SessionPlaybackCommand::Play).unwrap();
    reference.render(&mut reference_warmup).unwrap();
    previous.render(&mut warmup).unwrap();
    assert_eq!(warmup, reference_warmup);

    let mut editor = Editor::new(old_project, 8).unwrap();
    editor
        .execute(Command::MoveTrackDevice {
            device: instrument_id,
            track: track_id,
            before: Some(device_id),
        })
        .unwrap();
    let candidate_revision = Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &media,
            table,
            Some(old_revision.session()),
        )
        .unwrap(),
    );
    let mut candidate = SessionPlayback::new(candidate_revision).unwrap();
    candidate.try_inherit_state_from(&mut previous).unwrap();

    let mut expected = [[0.0; 2]; 16];
    let mut actual = [[0.0; 2]; 16];
    reference.render(&mut expected).unwrap();
    candidate.render(&mut actual).unwrap();
    assert_eq!(actual, expected);
    assert!(actual.iter().flatten().any(|sample| sample.abs() > 1.0e-6));
}

#[test]
fn stateful_prefix_and_suffix_match_the_finite_arrangement_across_session_blocks() {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    let table = Arc::new(hex_devices::SineTable::new());
    let mut snapshot = project(0.5, 0, 8 << 32).snapshot();
    let state = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            parameter::DC_REMOVE,
            ParameterValue::Bool(true),
        )
        .unwrap();
    snapshot.tracks[0].devices = vec![
        hex_project::DeviceSnapshot {
            modulation: Default::default(),
            parent: None,
            automation: Vec::new(),
            comparison: None,
            id: snapshot.next_id,
            bypassed: false,
            state: state.clone(),
        },
        hex_project::DeviceSnapshot {
            modulation: Default::default(),
            parent: None,
            automation: Vec::new(),
            comparison: None,
            id: snapshot.next_id + 1,
            bypassed: false,
            state,
        },
    ];
    snapshot.next_id += 2;
    snapshot.tracks[0].instrument_position = Some(1);
    let prepared = revision(&legacy_project_from_snapshot(snapshot).unwrap(), table);
    let clip = prepared.session().midi_clips()[0].id;
    let mut finite = Playback::from_prepared(Arc::clone(&prepared));
    let mut session = SessionPlayback::new(prepared).unwrap();
    finite.apply(PlaybackCommand::Play);
    session
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    session.apply(SessionPlaybackCommand::Play).unwrap();

    let mut expected = [[0.0; 2]; 12_000];
    let mut actual = [[0.0; 2]; 12_000];
    finite.render(&mut expected).unwrap();
    let mut offset = 0;
    for count in [1, 17, 127, 256, 31, 1024, 4096, 6448] {
        session.render(&mut actual[offset..offset + count]).unwrap();
        offset += count;
    }
    assert_eq!(offset, actual.len());
    assert_eq!(actual, expected);
}

#[test]
fn active_tail_group_moves_with_its_unquiescent_suffix_across_revision() {
    let table = Arc::new(hex_devices::SineTable::new());
    let base = with_utility_filter_state(&project(0.5, 0, 8 << 32), true, false);
    let base_revision = revision(&base, Arc::clone(&table));
    let mut previous = SessionPlayback::new(Arc::clone(&base_revision)).unwrap();
    let clip = previous.revision.session().midi_clips()[0].id;
    previous
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 64];
    previous.render(&mut prefix).unwrap();

    let changed = with_utility_filter_state(
        &project_at_tempo(0.5, 0, 8 << 32, Tempo::new(60_000_000).unwrap()),
        true,
        false,
    );
    let mut first = SessionPlayback::new(revision(&changed, Arc::clone(&table))).unwrap();
    first.try_inherit_state_from(&mut previous).unwrap();
    assert_eq!(first.tail_groups.iter().flatten().count(), 1);

    // This suffix is still producing residual output, so the next candidate
    // must inherit its group and continue advancing it. A quiescent group may
    // remain on the old owner for control-side retirement instead.
    let changed_again = with_utility_filter_state(&base, true, false);
    let mut second = SessionPlayback::new(revision(&changed_again, table)).unwrap();
    second.try_inherit_state_from(&mut first).unwrap();
    assert!(first.tail_groups[0].is_none());
    assert_eq!(second.tail_groups[0].as_ref().unwrap().stateful_count(), 1);
    assert_eq!(second.tail_groups.iter().flatten().count(), 1);
    assert!(second.tail_output_active[0]);
}

#[test]
fn full_stateful_tail_bank_refuses_before_mutating_the_live_graph() {
    let table = Arc::new(hex_devices::SineTable::new());
    let base = with_utility_filter_state(&project(0.5, 0, 8 << 32), true, false);
    let base_revision = revision(&base, Arc::clone(&table));
    let changed = with_utility_filter_state(
        &project_at_tempo(0.5, 0, 8 << 32, Tempo::new(60_000_000).unwrap()),
        true,
        false,
    );
    let clip_id = base_revision.session().midi_clips()[0].id;

    let make_full = || {
        let mut previous = SessionPlayback::new(Arc::clone(&base_revision)).unwrap();
        previous
            .apply(SessionPlaybackCommand::LaunchClip {
                clip: clip_id,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        previous.apply(SessionPlaybackCommand::Play).unwrap();
        let mut prefix = [[0.0; 2]; 64];
        previous.render(&mut prefix).unwrap();

        let mut active = SessionPlayback::new(revision(&changed, Arc::clone(&table))).unwrap();
        active.try_inherit_state_from(&mut previous).unwrap();
        let mut next_prefix = [[0.0; 2]; 64];
        active.render(&mut next_prefix).unwrap();

        let release = *active
            .releases
            .first()
            .expect("first handoff captures a release");
        for slot in 1..SESSION_TAIL_GROUP_CAPACITY {
            active.tail_groups[slot] = Some(hex_lattice::NativeRuntimeChain::empty());
            let mut grouped = release;
            grouped.location = ReleaseLocation::new(
                grouped.location.track_index(),
                Some(u16::try_from(slot).unwrap()),
            )
            .unwrap();
            active.releases.push(grouped);
        }
        let mut current_chain_release = release;
        // This extra voice belongs to the current source chain, unlike the
        // detached historical group copied above. Give it explicit custody.
        current_chain_release.source_origin = ReleaseSourceOrigin::capture(&active.revision, 0);
        current_chain_release.location = ReleaseLocation::new(0, None).unwrap();
        active.releases.push(current_chain_release);
        active
    };

    let mut active = make_full();
    let mut reference = make_full();
    assert_eq!(
        active.tail_groups.iter().flatten().count(),
        SESSION_TAIL_GROUP_CAPACITY
    );
    let next_project = with_utility_filter_state(&base, true, false);
    let mut candidate = SessionPlayback::new(revision(&next_project, Arc::clone(&table))).unwrap();
    assert_eq!(
        candidate.try_inherit_state_from(&mut active),
        Err(SessionPlaybackError::ReleaseBankFull)
    );
    assert_eq!(
        active.tail_groups.iter().flatten().count(),
        SESSION_TAIL_GROUP_CAPACITY
    );
    assert_eq!(active.device_chains[0].stateful_count(), 1);

    let mut after_refusal = [[0.0; 2]; 128];
    let mut reference_output = [[0.0; 2]; 128];
    active.render(&mut after_refusal).unwrap();
    reference.render(&mut reference_output).unwrap();
    assert_eq!(after_refusal, reference_output);
}

#[test]
fn stateful_filter_releases_clip_history_on_pause_resets_on_seek_stop_and_continues_across_loop() {
    let table = Arc::new(hex_devices::SineTable::new());
    let mut base_snapshot = project(0.5, 0, 8 << 32).snapshot();
    base_snapshot.tracks[0].content = TrackContentSnapshot::Instrument(Vec::new());
    let base = with_utility_filter_state(
        &legacy_project_from_snapshot(base_snapshot).unwrap(),
        true,
        false,
    );

    let start = |revision: Arc<PreparedRevision>, clip: hex_project::ClipId| {
        let mut playback = SessionPlayback::new(revision).unwrap();
        playback
            .apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        playback
    };

    let prepared = revision(&base, Arc::clone(&table));
    let clip = prepared.session().midi_clips()[0].id;
    let mut paused = start(Arc::clone(&prepared), clip);
    let mut prefix = [[0.0; 2]; 32];
    paused.render(&mut prefix).unwrap();
    let frame = paused.performance_frame();
    let mut released = start(Arc::clone(&prepared), clip);
    released.render(&mut [[0.0; 2]; 32]).unwrap();
    released
        .apply(SessionPlaybackCommand::StopTrack {
            track: base.tracks()[0].id(),
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    paused.apply(SessionPlaybackCommand::Pause).unwrap();
    let mut paused_release = [[0.0; 2]; 64];
    let mut expected_release = paused_release;
    paused.render(&mut paused_release).unwrap();
    released.render(&mut expected_release).unwrap();
    assert_eq!(
        paused_release, expected_release,
        "pause releases through the existing filter history"
    );
    assert_eq!(paused.performance_frame(), frame);
    paused.apply(SessionPlaybackCommand::Play).unwrap();
    let mut after_pause = [[0.0; 2]; 64];
    let mut expected_tail = after_pause;
    paused.render(&mut after_pause).unwrap();
    released.render(&mut expected_tail).unwrap();
    assert_eq!(
        after_pause, expected_tail,
        "resume does not retrigger the paused clip note"
    );

    let mut sought = start(Arc::clone(&prepared), clip);
    let mut pre_seek = [[0.0; 2]; 32];
    sought.render(&mut pre_seek).unwrap();
    sought.apply(SessionPlaybackCommand::Seek(0)).unwrap();
    sought.apply(SessionPlaybackCommand::Play).unwrap();
    let mut after_seek = [[0.0; 2]; 64];
    sought.render(&mut after_seek).unwrap();
    let mut fresh_seek = start(Arc::clone(&prepared), clip);
    fresh_seek.apply(SessionPlaybackCommand::Seek(0)).unwrap();
    fresh_seek.apply(SessionPlaybackCommand::Play).unwrap();
    let mut fresh_seek_output = [[0.0; 2]; 64];
    fresh_seek.render(&mut fresh_seek_output).unwrap();
    assert_eq!(after_seek, fresh_seek_output);

    let mut stopped = start(Arc::clone(&prepared), clip);
    let mut pre_stop = [[0.0; 2]; 32];
    stopped.render(&mut pre_stop).unwrap();
    stopped.apply(SessionPlaybackCommand::Stop).unwrap();
    stopped.apply(SessionPlaybackCommand::Play).unwrap();
    let mut after_stop = [[0.0; 2]; 64];
    stopped.render(&mut after_stop).unwrap();
    let mut fresh_stop = start(Arc::clone(&prepared), clip);
    fresh_stop.apply(SessionPlaybackCommand::Stop).unwrap();
    fresh_stop.apply(SessionPlaybackCommand::Play).unwrap();
    let mut fresh_stop_output = [[0.0; 2]; 64];
    fresh_stop.render(&mut fresh_stop_output).unwrap();
    assert_eq!(after_stop, fresh_stop_output);

    let mut loop_snapshot = base.snapshot();
    loop_snapshot.loop_region = Some(LoopRegion::new(0, 64, true).unwrap());
    let looped = legacy_project_from_snapshot(loop_snapshot).unwrap();
    let mut with_loop = start(revision(&looped, Arc::clone(&table)), clip);
    let mut without_loop = start(prepared, clip);
    let mut looped_output = [[0.0; 2]; 512];
    let mut plain_output = [[0.0; 2]; 512];
    with_loop.render(&mut looped_output).unwrap();
    without_loop.render(&mut plain_output).unwrap();
    assert_eq!(looped_output, plain_output);
}

#[test]
fn session_render_failure_after_stateful_prefix_rolls_back_filter_history_and_clock() {
    let table = Arc::new(hex_devices::SineTable::new());
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    let mut snapshot = project(0.5, 0, 8 << 32).snapshot();
    let state = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            parameter::DC_REMOVE,
            ParameterValue::Bool(true),
        )
        .unwrap();
    let mut devices = Vec::new();
    for _ in 0..2 {
        let id = snapshot.next_id;
        snapshot.next_id += 1;
        devices.push(hex_project::DeviceSnapshot {
            modulation: Default::default(),
            parent: None,
            automation: Vec::new(),
            comparison: None,
            id,
            bypassed: false,
            state: state.clone(),
        });
    }
    snapshot.tracks[0].devices = devices;
    snapshot.tracks[0].instrument_position = Some(1);
    let project = legacy_project_from_snapshot(snapshot).unwrap();
    let prepared = revision(&project, table);
    let clip = prepared.session().midi_clips()[0].id;
    let mut failed = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    let mut reference = SessionPlayback::new(prepared).unwrap();
    for playback in [&mut failed, &mut reference] {
        playback
            .apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
    }
    let mut prefix = [[0.0; 2]; 64];
    let mut reference_prefix = [[0.0; 2]; 64];
    failed.render(&mut prefix).unwrap();
    reference.render(&mut reference_prefix).unwrap();
    assert_eq!(prefix, reference_prefix);

    failed.release_scratch.clear();
    let owner = failed.routes[0].owner;
    failed
        .capture_outgoing(
            0,
            owner,
            failed.arrangement_frame,
            failed.performance_frame,
            None,
        )
        .unwrap();
    assert!(!failed.release_scratch.is_empty());
    let mut invalid_tail = failed.release_scratch[0];
    invalid_tail.location = ReleaseLocation::corrupt_detached_tail_for_test();
    failed.release_scratch.clear();
    failed.release_scratch.push(invalid_tail);
    std::mem::swap(&mut failed.releases, &mut failed.release_scratch);

    let committed_frame = failed.performance_frame;
    let mut refused = [[1.0; 2]; 256];
    assert_eq!(
        failed.render(&mut refused),
        Err(SessionPlaybackError::DeviceRuntime)
    );
    assert_eq!(refused, [[0.0; 2]; 256]);
    assert_eq!(failed.performance_frame, committed_frame);
    assert_eq!(failed.arrangement_frame, reference.arrangement_frame);

    // Remove only the deliberately corrupt release, then verify the retry has
    // the exact same processor history and clock as the untouched twin.
    failed.releases.clear();
    let mut retry = [[0.0; 2]; 128];
    let mut expected = [[0.0; 2]; 128];
    failed.render(&mut retry).unwrap();
    reference.render(&mut expected).unwrap();
    assert_eq!(retry, expected);
    assert_eq!(failed.performance_frame, reference.performance_frame);
}
