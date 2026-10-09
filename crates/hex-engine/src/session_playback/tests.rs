use super::*;
use crate::MidiTiming;
use crate::compile::PreparedAudioSource;
use crate::{BorrowedMediaPage, MediaPages, StreamSourceKey, StreamedSourceMetadata};
use crate::{PreparedRevision, PreparedSessionInventory};
use hex_project::{
    AssetSnapshot, BeatTime, ClipFade, ClipSnapshot, DeviceSnapshot, LoopRegion, Meter, MeterPoint,
    MidiClipSnapshot, MidiNoteSnapshot, MidiSourceSnapshot, Project, SceneSnapshot,
    SessionCellContentSnapshot, SessionCellSnapshot, SessionMidiClipSnapshot, SessionSnapshot,
    Snapshot, Tempo, TempoPoint, TrackContentSnapshot, TrackSnapshot,
};
use std::{collections::BTreeMap, sync::Arc};

#[test]
fn wide_analytic_release_and_span_storage_stays_within_derived_fixed_bound() {
    let sampler_bytes = std::mem::size_of::<hex_lattice::PreparedAnalyticNote>();
    assert_eq!(sampler_bytes, 344);
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
    #[cfg(target_pointer_width = "64")]
    {
        assert_eq!(std::mem::size_of::<super::ReleaseEntry>(), 488);
        assert_eq!(span_buffer_bytes, 81_416);
        assert_eq!(current_bytes, 16_072_200);
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
    assert!(current_bytes < 16 * 1024 * 1024);
}

/// These hand-authored fixtures predate schema 15 and model the schema 1–14
/// migration that installs a default sine slot on every track carrying MIDI.
fn legacy_project_from_snapshot(mut snapshot: Snapshot) -> Result<Project, hex_project::Invalid> {
    let state = hex_project::default_instrument_state()?;
    for track in &mut snapshot.tracks {
        let has_midi = match &track.content {
            TrackContentSnapshot::Instrument(clips) => !clips.is_empty(),
            TrackContentSnapshot::Hybrid { midi_clips, .. } => !midi_clips.is_empty(),
            TrackContentSnapshot::Audio(_) => false,
        } || snapshot.session.cells.iter().any(|cell| {
            cell.track == track.id && matches!(cell.content, SessionCellContentSnapshot::Midi(_))
        });
        if has_midi && track.instrument.is_none() {
            let id = snapshot.next_id;
            snapshot.next_id = snapshot
                .next_id
                .checked_add(1)
                .ok_or(hex_project::Invalid("fixture id capacity"))?;
            track.instrument = Some(DeviceSnapshot {
                id,
                bypassed: false,
                state: state.clone(),
            });
        }
    }
    Project::from_snapshot(snapshot)
}

fn project(gain: f32, note_start: u64, note_duration: u64) -> Project {
    project_at_tempo(gain, note_start, note_duration, Tempo::DEFAULT)
}

fn audio_project(source_start: u64, source_end: u64, asset_id: u64) -> Project {
    legacy_project_from_snapshot(Snapshot {
        sample_rate: 48_000,
        next_id: 7,
        assets: vec![
            AssetSnapshot {
                id: 4,
                path: "shared.wav".into(),
                source_sample_rate: 48_000,
                source_frames: 32,
                playback_frames: 32,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            },
            AssetSnapshot {
                id: 5,
                path: "replacement.wav".into(),
                source_sample_rate: 48_000,
                source_frames: 32,
                playback_frames: 32,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            },
        ],
        tracks: vec![TrackSnapshot {
            id: 1,
            name: "audio".into(),
            gain: 1.0,
            balance: 0.0,
            muted: false,
            soloed: false,
            devices: Vec::new(),
            instrument: None,
            content: TrackContentSnapshot::Audio(Vec::new()),
        }],
        session: SessionSnapshot {
            scenes: vec![SceneSnapshot {
                id: 2,
                name: "scene".into(),
            }],
            cells: vec![SessionCellSnapshot {
                track: 1,
                scene: 2,
                content: SessionCellContentSnapshot::Audio(hex_project::SessionAudioClipSnapshot {
                    id: 3,
                    name: "one shot".into(),
                    asset: asset_id,
                    source_start,
                    source_end,
                }),
            }],
        },
        loop_region: None,
        master_gain: 1.0,
        tempo_points: vec![TempoPoint::new(BeatTime::ZERO, Tempo::DEFAULT)],
        meter_points: vec![MeterPoint::new(BeatTime::ZERO, Meter::COMMON_TIME)],
    })
    .expect("valid Session audio project")
}

fn audio_media(
    project: &Project,
    asset4: &crate::Audio,
    asset5: &crate::Audio,
) -> BTreeMap<hex_project::AssetId, crate::Audio> {
    project
        .assets()
        .map(|asset| {
            (
                asset.id(),
                if asset.id().value() == 4 {
                    asset4.clone()
                } else {
                    asset5.clone()
                },
            )
        })
        .collect()
}

fn mixed_audio_midi_project(source_start: u64, source_end: u64) -> Project {
    let mut snapshot = audio_project(source_start, source_end, 4).snapshot();
    snapshot.next_id = 12;
    snapshot.tracks.push(TrackSnapshot {
        id: 6,
        name: "keys".into(),
        gain: 1.0,
        balance: 0.0,
        muted: false,
        soloed: false,
        devices: Vec::new(),
        instrument: None,
        content: TrackContentSnapshot::Instrument(vec![MidiClipSnapshot {
            id: 8,
            start_raw: 0,
            length_raw: 4 << 32,
            source_length_raw: 4 << 32,
            source_start_raw: 0,
            loop_start_raw: 0,
            loop_end_raw: 4 << 32,
            looping: false,
            notes: vec![MidiNoteSnapshot {
                id: 9,
                start_raw: 0,
                duration_raw: 1 << 32,
                pitch: 69,
                channel: 0,
                velocity: 100,
            }],
        }]),
    });
    snapshot.session.scenes[0].name = "audio and keys".into();
    snapshot.session.cells.push(SessionCellSnapshot {
        track: 6,
        scene: 2,
        content: SessionCellContentSnapshot::Midi(SessionMidiClipSnapshot {
            id: 10,
            name: "empty midi".into(),
            source: MidiSourceSnapshot {
                source_length_raw: 4 << 32,
                source_start_raw: 0,
                loop_start_raw: 0,
                loop_end_raw: 4 << 32,
                looping: false,
                notes: Vec::new(),
            },
        }),
    });
    snapshot.session.scenes.push(SceneSnapshot {
        id: 7,
        name: "stop audio".into(),
    });
    snapshot.session.cells.push(SessionCellSnapshot {
        track: 1,
        scene: 7,
        content: SessionCellContentSnapshot::Stop,
    });
    legacy_project_from_snapshot(snapshot).expect("mixed audio and midi project")
}

fn session_audio_midi_arrangement_mix_project() -> Project {
    let mut snapshot = mixed_audio_midi_project(5, 18).snapshot();
    snapshot.next_id = 16;
    let Some(SessionCellContentSnapshot::Midi(clip)) = snapshot
        .session
        .cells
        .iter_mut()
        .find(|cell| cell.track == 6)
        .map(|cell| &mut cell.content)
    else {
        panic!("Session MIDI fixture expected")
    };
    clip.source.notes.push(MidiNoteSnapshot {
        id: 11,
        start_raw: 0,
        duration_raw: 1 << 32,
        pitch: 69,
        channel: 0,
        velocity: 100,
    });
    snapshot.tracks.push(TrackSnapshot {
        id: 14,
        name: "arrangement bed".into(),
        gain: 1.0,
        balance: 0.0,
        muted: false,
        soloed: false,
        devices: Vec::new(),
        instrument: None,
        content: TrackContentSnapshot::Audio(vec![ClipSnapshot {
            id: 15,
            asset: 4,
            start: 0,
            duration: 32,
            source_offset: 0,
            source_length: 32,
            looping: false,
            source_phase: 0,
            fade: ClipFade::neutral(32).unwrap(),
        }]),
    });
    legacy_project_from_snapshot(snapshot)
        .expect("mixed Session and Arrangement audio/MIDI project")
}

fn project_at_tempo(gain: f32, note_start: u64, note_duration: u64, tempo: Tempo) -> Project {
    legacy_project_from_snapshot(Snapshot {
        sample_rate: 48_000,
        next_id: 8,
        assets: Vec::new(),
        tracks: vec![TrackSnapshot {
            id: 1,
            name: "track".into(),
            gain,
            balance: 0.0,
            muted: false,
            soloed: false,
            devices: Vec::new(),
            instrument: None,
            content: TrackContentSnapshot::Instrument(vec![MidiClipSnapshot {
                id: 2,
                start_raw: 0,
                length_raw: 16 << 32,
                source_length_raw: 32 << 32,
                source_start_raw: 0,
                loop_start_raw: 0,
                loop_end_raw: 32 << 32,
                looping: false,
                notes: vec![MidiNoteSnapshot {
                    id: 3,
                    start_raw: note_start,
                    duration_raw: note_duration,
                    pitch: 60,
                    channel: 0,
                    velocity: 100,
                }],
            }]),
        }],
        session: SessionSnapshot {
            scenes: vec![SceneSnapshot {
                id: 4,
                name: "scene".into(),
            }],
            cells: vec![SessionCellSnapshot {
                track: 1,
                scene: 4,
                content: SessionCellContentSnapshot::Midi(SessionMidiClipSnapshot {
                    id: 5,
                    name: "clip".into(),
                    source: MidiSourceSnapshot {
                        source_length_raw: 32 << 32,
                        source_start_raw: 0,
                        loop_start_raw: 0,
                        loop_end_raw: 32 << 32,
                        looping: false,
                        notes: vec![MidiNoteSnapshot {
                            id: 6,
                            start_raw: note_start,
                            duration_raw: note_duration,
                            pitch: 60,
                            channel: 0,
                            velocity: 100,
                        }],
                    },
                }),
            }],
        },
        loop_region: None,
        master_gain: 1.0,
        tempo_points: vec![TempoPoint::new(BeatTime::ZERO, tempo)],
        meter_points: vec![MeterPoint::new(BeatTime::ZERO, Meter::COMMON_TIME)],
    })
    .unwrap()
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
        id: 9,
        bypassed: false,
        state,
    });
    snapshot.next_id = 10;
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
        id: 9,
        bypassed: false,
        state,
    });
    old_snapshot.next_id = 10;
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
fn outgoing_session_release_keeps_the_old_stateful_filter_settings() {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    let table = Arc::new(hex_devices::SineTable::new());
    let mut snapshot = project(0.5, 0, 8 << 32).snapshot();
    snapshot.tracks[0].content = TrackContentSnapshot::Instrument(Vec::new());
    let state = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            parameter::DC_REMOVE,
            ParameterValue::Bool(true),
        )
        .unwrap();
    snapshot.tracks[0]
        .devices
        .push(hex_project::DeviceSnapshot {
            id: snapshot.next_id,
            bypassed: false,
            state,
        });
    snapshot.next_id += 1;
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
    let current_dc = with_utility_filter_state(&stopped, true, false);
    let current_bass = with_utility_filter_state(&stopped, false, true);
    // The candidate route is silent. Its new device configuration must not
    // change outgoing voices, which continue through the captured old chain.
    assert_eq!(render_old_tail(&current_dc), render_old_tail(&current_bass));
}

#[test]
fn completed_tail_group_is_retained_on_old_owner_and_its_slot_is_reusable() {
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

    // Model a completed release while retaining its owner until old-graph
    // retirement. The next candidate may reuse its numeric slot, but must not
    // take or drop the completed chain on the callback handoff path.
    let mut next_release = *first.releases.first().unwrap();
    first.releases.clear();
    // Keep a distinct live release on the current track. The next handoff
    // must allocate a tail group for it and can reuse slot zero only if the
    // completed group remains on the retired graph.
    next_release.tail_group = None;
    next_release.track_index = 0;
    first.releases.push(next_release);
    let changed_again = with_utility_filter_state(&base, true, false);
    let mut second = SessionPlayback::new(revision(&changed_again, table)).unwrap();
    second.try_inherit_state_from(&mut first).unwrap();
    assert!(first.tail_groups[0].is_some());
    assert_eq!(second.tail_groups[0].as_ref().unwrap().stateful_count(), 1);
    assert_eq!(second.tail_groups.iter().flatten().count(), 1);
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
            grouped.tail_group = Some(u16::try_from(slot).unwrap());
            active.releases.push(grouped);
        }
        let mut current_chain_release = release;
        current_chain_release.tail_group = None;
        current_chain_release.track_index = 0;
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
fn stateful_filter_history_freezes_on_pause_resets_on_seek_stop_and_continues_across_loop() {
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
    paused.apply(SessionPlaybackCommand::Pause).unwrap();
    let mut silence = [[1.0; 2]; 64];
    paused.render(&mut silence).unwrap();
    assert_eq!(silence, [[0.0; 2]; 64]);
    assert_eq!(paused.performance_frame(), frame);
    paused.apply(SessionPlaybackCommand::Play).unwrap();
    let mut after_pause = [[0.0; 2]; 64];
    paused.render(&mut after_pause).unwrap();
    let mut continuous = start(Arc::clone(&prepared), clip);
    let mut continuous_prefix = [[0.0; 2]; 32];
    continuous.render(&mut continuous_prefix).unwrap();
    let mut continuous_tail = [[0.0; 2]; 64];
    continuous.render(&mut continuous_tail).unwrap();
    assert_eq!(after_pause, continuous_tail);

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
    let project = with_utility_filter_state(&project(0.5, 0, 8 << 32), true, false);
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
        .capture_outgoing(0, owner, failed.arrangement_frame, failed.performance_frame)
        .unwrap();
    assert!(!failed.release_scratch.is_empty());
    let mut invalid_tail = failed.release_scratch[0];
    invalid_tail.track_index = usize::MAX;
    invalid_tail.tail_group = Some(u16::MAX);
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

fn with_session_note_id(project: &Project, note_id: u64) -> Project {
    let mut snapshot = project.snapshot();
    if let SessionCellContentSnapshot::Midi(clip) = &mut snapshot.session.cells[0].content {
        clip.source.notes[0].id = note_id;
    }
    snapshot.next_id = note_id + 1;
    legacy_project_from_snapshot(snapshot).unwrap()
}

fn revision(project: &Project, table: Arc<hex_devices::SineTable>) -> Arc<PreparedRevision> {
    Arc::new(PreparedRevision::prepare(project, &BTreeMap::new(), table, None).unwrap())
}

fn with_utility_filter_state(project: &Project, dc_remove: bool, bass_mono: bool) -> Project {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    let mut snapshot = project.snapshot();
    let mut state = UTILITY_DEFINITION.default_state().unwrap();
    state = UTILITY_DEFINITION
        .patch_parameter(
            &state,
            parameter::DC_REMOVE,
            ParameterValue::Bool(dc_remove),
        )
        .unwrap();
    state = UTILITY_DEFINITION
        .patch_parameter(
            &state,
            parameter::BASS_MONO,
            ParameterValue::Bool(bass_mono),
        )
        .unwrap();
    if let Some(device) = snapshot.tracks[0].devices.first_mut() {
        device.state = state;
        device.bypassed = false;
    } else {
        snapshot.tracks[0]
            .devices
            .push(hex_project::DeviceSnapshot {
                id: snapshot.next_id,
                bypassed: false,
                state,
            });
        snapshot.next_id += 1;
    }
    legacy_project_from_snapshot(snapshot).unwrap()
}

fn without_session_cells(project: &Project) -> Project {
    let mut snapshot = project.snapshot();
    snapshot.session.cells.clear();
    legacy_project_from_snapshot(snapshot).unwrap()
}

#[test]
fn gain_only_handoff_preserves_route_clock_and_active_audio() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip: old.revision.session().midi_clips()[0].id,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut first = [[0.0; 2]; 64];
    old.render(&mut first).unwrap();
    assert!(first.iter().any(|frame| frame[0] != 0.0));

    let changed = project(0.75, 0, 8 << 32);
    let mut next = SessionPlayback::new(revision(&changed, table)).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(next.performance_frame(), 64);
    assert_eq!(
        next.owner(old.routes[0].id),
        Some(SessionPlaybackOwner::Session {
            clip: old.revision.session().midi_clips()[0].id
        })
    );
    let mut continued = [[0.0; 2]; 64];
    next.render(&mut continued).unwrap();
    assert!(continued.iter().any(|frame| frame[0] != 0.0));
}

#[test]
fn carried_voice_audio_is_partition_invariant_after_tempo_handoff() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 64];
    old.render(&mut prefix).unwrap();

    let slower = project_at_tempo(0.5, 0, 8 << 32, Tempo::new(60_000_000).unwrap());
    let next_revision = revision(&slower, Arc::clone(&table));
    let mut whole = SessionPlayback::new(Arc::clone(&next_revision)).unwrap();
    let mut split = SessionPlayback::new(next_revision).unwrap();
    whole.try_inherit_state_from(&mut old).unwrap();
    split.try_inherit_state_from(&mut old).unwrap();
    let mut expected = vec![[0.0; 2]; 513];
    whole.render(&mut expected).unwrap();
    let mut actual = Vec::new();
    let mut remaining = 513;
    for width in [1, 31, 256, 7, 218] {
        if remaining == 0 {
            break;
        }
        let width = width.min(remaining);
        let mut block = vec![[0.0; 2]; width];
        split.render(&mut block).unwrap();
        actual.extend(block);
        remaining -= width;
    }
    assert_eq!(remaining, 0);
    assert_eq!(actual, expected);
}

#[test]
fn repeated_loop_occurrence_descriptor_and_pcm_survive_tempo_handoff() {
    let table = Arc::new(hex_devices::SineTable::new());
    let base = project(0.5, 0, 8 << 32);
    let mut snapshot = base.snapshot();
    if let SessionCellContentSnapshot::Midi(clip) = &mut snapshot.session.cells[0].content {
        clip.source.source_start_raw = 1 << 32;
        clip.source.loop_start_raw = 2 << 32;
        clip.source.loop_end_raw = 4 << 32;
        clip.source.looping = true;
        clip.source.notes = vec![MidiNoteSnapshot {
            id: 6,
            start_raw: 5_u64 << 31, // loop phase 1/2; first repeat begins at 3.5
            duration_raw: 1 << 32,
            pitch: 67,
            channel: 0,
            velocity: 90,
        }];
    }
    snapshot.next_id = 9;
    let original = legacy_project_from_snapshot(snapshot).unwrap();
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut before_cut = vec![[0.0; 2]; 100_000];
    old.render(&mut before_cut).unwrap();
    assert!(before_cut.iter().any(|frame| frame[0] != 0.0));

    let mut old_spans = MidiSpanBuffer::default();
    let launch = match old.routes[0].owner {
        RouteOwner::Session { launch, .. } => launch,
        _ => panic!("Session source is selected"),
    };
    old.revision
        .session()
        .midi_clip(clip)
        .unwrap()
        .instrument_template()
        .query(launch, &old.timing_view(), 100_000, 100_001, &mut old_spans)
        .unwrap();
    let repeated = old_spans
        .spans()
        .find(|span| span.note.value() == 6 && span.iteration == 1)
        .copied()
        .expect("first repeated occurrence remains held at the cut");
    assert_eq!(repeated.start_frame, 84_000);
    assert_eq!(repeated.gate_end_frame, 108_000);

    let slower = project_at_tempo(0.5, 0, 8 << 32, Tempo::new(60_000_000).unwrap());
    let mut slower_snapshot = slower.snapshot();
    slower_snapshot.session = original.snapshot().session;
    let slower = legacy_project_from_snapshot(slower_snapshot).unwrap();
    let next_revision = revision(&slower, Arc::clone(&table));
    let mut whole = SessionPlayback::new(Arc::clone(&next_revision)).unwrap();
    let mut split = SessionPlayback::new(next_revision).unwrap();
    whole.try_inherit_state_from(&mut old).unwrap();
    split.try_inherit_state_from(&mut old).unwrap();
    assert!(whole.releases.iter().any(|entry| {
        entry
            .continuation
            .is_some_and(|continuation| continuation.key.iteration == 1)
    }));
    assert!(whole.suppressions.iter().any(|entry| entry.iteration == 1));

    let mut expected = [[0.0; 2]; 256];
    let mut actual = Vec::new();
    whole.render(&mut expected).unwrap();
    for width in [1, 31, 64, 127, 33] {
        let mut block = vec![[0.0; 2]; width];
        split.render(&mut block).unwrap();
        actual.extend(block);
    }
    assert_eq!(actual.as_slice(), expected);
    assert!(expected.iter().any(|frame| frame[0] != 0.0));
}

#[test]
fn source_edit_at_exact_cut_remains_future_eligible() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 1 << 30);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip: old.revision.session().midi_clips()[0].id,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut first = vec![[0.0; 2]; 20_000];
    old.render(&mut first).unwrap();
    let cut = old
        .timing_view()
        .frame_to_beat(old.performance_frame())
        .unwrap();

    let edited = with_session_note_id(&project(0.5, cut.raw(), 1 << 30), 77);
    let mut next = SessionPlayback::new(revision(&edited, table)).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut at_cut = [[0.0; 2]; 64];
    next.render(&mut at_cut).unwrap();
    assert!(at_cut.iter().any(|frame| frame[0] != 0.0));
}

#[test]
fn inserted_strictly_historical_note_does_not_chase_after_handoff() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 1 << 30);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip: old.revision.session().midi_clips()[0].id,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut first = vec![[0.0; 2]; 20_000];
    old.render(&mut first).unwrap();
    assert!(first.iter().any(|frame| frame[0] != 0.0));
    let cut = old
        .timing_view()
        .frame_to_beat(old.performance_frame())
        .unwrap();
    let edited = with_session_note_id(&project(0.5, cut.raw().saturating_sub(1), 1 << 30), 77);
    let mut next = SessionPlayback::new(revision(&edited, table)).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut after = [[0.0; 2]; 64];
    next.render(&mut after).unwrap();
    assert!(after.iter().all(|frame| *frame == [0.0; 2]));
}

#[test]
fn tempo_handoff_preserves_active_voice_phase_and_remaps_held_gate() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip: old.revision.session().midi_clips()[0].id,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut before = [[0.0; 2]; 64];
    old.render(&mut before).unwrap();

    let slower = project_at_tempo(0.5, 0, 8 << 32, Tempo::new(60_000_000).unwrap());
    let mut next = SessionPlayback::new(revision(&slower, Arc::clone(&table))).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut actual = [[0.0; 2]; 32];
    next.render(&mut actual).unwrap();

    let voice = table.prepare_voice(48_000, 60, 100).unwrap();
    let gate_end = next
        .timing_view()
        .beat_to_frame(BeatTime::from_raw(8 << 32))
        .unwrap();
    for (offset, frame) in actual.iter().enumerate() {
        let absolute = 64 + offset as u64;
        let expected = table.sample(voice, absolute, gate_end) * 0.5;
        assert_eq!(*frame, [expected; 2]);
    }
}

#[test]
fn tempo_handoff_keeps_already_releasing_voice_on_its_original_gate() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 1 << 30);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip: old.revision.session().midi_clips()[0].id,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut before = vec![[0.0; 2]; 6_100];
    old.render(&mut before).unwrap();

    let slower = project_at_tempo(0.5, 0, 1 << 30, Tempo::new(60_000_000).unwrap());
    let mut next = SessionPlayback::new(revision(&slower, Arc::clone(&table))).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut actual = [[0.0; 2]; 32];
    next.render(&mut actual).unwrap();

    let voice = table.prepare_voice(48_000, 60, 100).unwrap();
    let old_gate = old
        .revision
        .session()
        .timing()
        .beat_to_frame(BeatTime::from_raw(1 << 30))
        .unwrap();
    for (offset, frame) in actual.iter().enumerate() {
        let absolute = 6_100 + offset as u64;
        let expected = table.sample(voice, absolute, old_gate) * 0.5;
        assert_eq!(*frame, [expected; 2]);
    }
}

#[test]
fn stopping_after_handoff_cuts_carried_held_voice_without_requerying_history() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    let track = old.routes[0].id;
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut first = [[0.0; 2]; 64];
    old.render(&mut first).unwrap();

    let slower = project_at_tempo(0.5, 0, 8 << 32, Tempo::new(60_000_000).unwrap());
    let mut next = SessionPlayback::new(revision(&slower, table)).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(next.release_voice_count(), 2); // arrangement and carried session voice

    next.apply(SessionPlaybackCommand::StopTrack {
        track,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    assert_eq!(next.owner(track), Some(SessionPlaybackOwner::Silence));
    assert_eq!(next.release_voice_count(), 2);
    assert!(
        next.releases
            .iter()
            .filter(|entry| entry.continuation.is_none())
            .any(|entry| entry.voice.gate_end_frame() == next.performance_frame)
    );
}

#[test]
fn seek_after_anchored_handoff_uses_base_clock_and_resets_history() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    let track = old.routes[0].id;
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut block = [[0.0; 2]; 64];
    old.render(&mut block).unwrap();

    let slower = project_at_tempo(0.5, 0, 8 << 32, Tempo::new(60_000_000).unwrap());
    let mut next = SessionPlayback::new(revision(&slower, table)).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    assert!(next.anchor.is_some());
    next.apply(SessionPlaybackCommand::Seek(0)).unwrap();
    assert_eq!(next.performance_frame(), 0);
    assert_eq!(next.arrangement_frame(), 0);
    assert!(next.anchor.is_none());
    assert!(next.historical_floor.is_none());
    assert!(next.releases.is_empty());
    assert!(next.suppressions.is_empty());
    assert_eq!(
        next.owner(track),
        Some(SessionPlaybackOwner::Session { clip })
    );

    let before = next.performance_frame();
    assert_eq!(
        next.apply(SessionPlaybackCommand::Seek(u64::MAX)),
        Err(SessionPlaybackError::TempoConversion)
    );
    assert_eq!(next.performance_frame(), before);
    assert!(next.anchor.is_none());
}

#[test]
fn pause_and_stop_after_handoff_hold_then_reset_the_reanchored_clock() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    let track = old.routes[0].id;
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut first = [[0.0; 2]; 64];
    old.render(&mut first).unwrap();
    let slower = project_at_tempo(0.5, 0, 8 << 32, Tempo::new(60_000_000).unwrap());
    let mut candidate = SessionPlayback::new(revision(&slower, table)).unwrap();
    candidate.try_inherit_state_from(&mut old).unwrap();
    assert!(candidate.anchor.is_some());
    let before = candidate.performance_frame();
    candidate.apply(SessionPlaybackCommand::Pause).unwrap();
    let mut paused = [[7.0; 2]; 32];
    candidate.render(&mut paused).unwrap();
    assert_eq!(paused, [[0.0; 2]; 32]);
    assert_eq!(candidate.performance_frame(), before);

    candidate.apply(SessionPlaybackCommand::Stop).unwrap();
    assert_eq!(candidate.performance_frame(), 0);
    assert_eq!(candidate.arrangement_frame(), 0);
    assert!(candidate.anchor.is_none());
    assert!(candidate.historical_floor.is_none());
    assert!(candidate.releases.is_empty());
    assert_eq!(
        candidate.owner(track),
        Some(SessionPlaybackOwner::Session { clip })
    );
    assert!(
        matches!(candidate.routes[0].owner, RouteOwner::Session { launch, .. } if launch == BeatTime::ZERO)
    );
}

#[test]
fn failed_handoff_capacity_is_atomic_and_can_be_retried() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut block = [[0.0; 2]; 64];
    old.render(&mut block).unwrap();
    let old_frame = old.performance_frame;
    let old_release_count = old.release_voice_count();

    let slower = project_at_tempo(0.5, 0, 8 << 32, Tempo::new(60_000_000).unwrap());
    let candidate_revision = revision(&slower, Arc::clone(&table));
    let mut candidate = SessionPlayback::new(Arc::clone(&candidate_revision)).unwrap();
    let mut twin = SessionPlayback::new(candidate_revision).unwrap();
    twin.try_inherit_state_from(&mut old).unwrap();
    candidate.release_capacity = 1;
    assert_eq!(
        candidate.try_inherit_state_from(&mut old),
        Err(SessionPlaybackError::ReleaseBankFull)
    );
    assert_eq!(candidate.performance_frame(), 0);
    assert_eq!(candidate.release_voice_count(), 0);
    assert_eq!(old.performance_frame(), old_frame);
    assert_eq!(old.release_voice_count(), old_release_count);

    candidate.release_capacity = SESSION_RELEASE_CAPACITY;
    candidate.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(candidate.performance_frame(), old_frame);
    assert!(candidate.release_voice_count() > 0);
    let mut after_refusal = [[0.0; 2]; 128];
    let mut twin_output = [[0.0; 2]; 128];
    candidate.render(&mut after_refusal).unwrap();
    twin.render(&mut twin_output).unwrap();
    assert_eq!(after_refusal, twin_output);
}

#[test]
fn repeated_handoff_keeps_releasing_voice_when_new_gate_is_unrepresentable() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 1 << 30);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut near_gate = vec![[0.0; 2]; 6_100];
    old.render(&mut near_gate).unwrap(); // voice is already releasing

    let slower = project_at_tempo(0.5, 0, 1 << 30, Tempo::new(60_000_000).unwrap());
    let mut middle = SessionPlayback::new(revision(&slower, Arc::clone(&table))).unwrap();
    middle.try_inherit_state_from(&mut old).unwrap();
    let second_slower = project_at_tempo(0.5, 0, 1 << 30, Tempo::new(1_000_000).unwrap());
    let mut final_revision = SessionPlayback::new(revision(&second_slower, table)).unwrap();
    let cut_beat = middle
        .timing_view()
        .frame_to_beat(middle.performance_frame())
        .unwrap();
    let base_beat = base_timing_for(
        final_revision.revision.session(),
        final_revision.periodic_tempo,
    )
    .frame_to_beat(middle.performance_frame())
    .unwrap();
    let final_timing = timing_view_for(
        final_revision.revision.session(),
        final_revision.periodic_tempo,
        Some(SessionClockAnchor {
            cut_frame: middle.performance_frame(),
            old_beat: cut_beat,
            new_beat: base_beat,
        }),
    );
    assert!(
        final_timing
            .beat_to_frame(BeatTime::from_raw(1 << 30))
            .is_err()
    );
    final_revision.try_inherit_state_from(&mut middle).unwrap();
    assert!(final_revision.release_voice_count() > 0);
}

#[test]
fn quantized_same_clip_retrigger_after_handoff_clears_old_occurrence_suppression() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    let track = old.routes[0].id;
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut first = [[0.0; 2]; 64];
    old.render(&mut first).unwrap();

    let slower = project_at_tempo(0.5, 0, 8 << 32, Tempo::new(60_000_000).unwrap());
    let mut playback = SessionPlayback::new(revision(&slower, Arc::clone(&table))).unwrap();
    playback.try_inherit_state_from(&mut old).unwrap();
    assert!(!playback.suppressions.is_empty());
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::FourQuarter,
        })
        .unwrap();
    let due = playback.routes[0].pending.unwrap().frame;
    let width = usize::try_from(due - playback.performance_frame + 64).unwrap();
    let mut output = vec![[0.0; 2]; width];
    playback.render(&mut output).unwrap();
    assert_eq!(playback.performance_frame, due + 64);
    assert_eq!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Session { clip })
    );
    assert!(playback.suppressions.is_empty()); // outgoing occurrence key was cleared
    assert!(output[width - 64..].iter().any(|frame| frame[0] != 0.0));
}

#[test]
fn repeated_tempo_handoffs_keep_historical_voice_without_requerying_onset() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let mut first = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    first
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: first.revision.session().midi_clips()[0].id,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    first.apply(SessionPlaybackCommand::Play).unwrap();
    let mut warm = [[0.0; 2]; 64];
    first.render(&mut warm).unwrap();

    let slower = project_at_tempo(0.5, 0, 8 << 32, Tempo::new(60_000_000).unwrap());
    let mut second = SessionPlayback::new(revision(&slower, Arc::clone(&table))).unwrap();
    second.try_inherit_state_from(&mut first).unwrap();
    let faster = project_at_tempo(0.5, 0, 8 << 32, Tempo::new(180_000_000).unwrap());
    let mut third = SessionPlayback::new(revision(&faster, Arc::clone(&table))).unwrap();
    third.try_inherit_state_from(&mut second).unwrap();
    let mut actual = [[0.0; 2]; 32];
    third.render(&mut actual).unwrap();
    let voice = table.prepare_voice(48_000, 60, 100).unwrap();
    let gate_end = third
        .timing_view()
        .beat_to_frame(BeatTime::from_raw(8 << 32))
        .unwrap();
    for (offset, frame) in actual.iter().enumerate() {
        let expected = table.sample(voice, 64 + offset as u64, gate_end) * 0.5;
        assert_eq!(*frame, [expected; 2]);
    }
}

#[test]
fn repeated_handoffs_suppress_an_occurrence_exactly_at_the_previous_cut() {
    let table = Arc::new(hex_devices::SineTable::new());
    let baseline = project(0.5, 0, 8 << 32);
    let baseline_revision = revision(&baseline, Arc::clone(&table));
    let cut_beat = baseline_revision
        .session()
        .timing()
        .frame_to_beat(64)
        .unwrap();
    assert_eq!(
        baseline_revision
            .session()
            .timing()
            .beat_to_frame(cut_beat)
            .unwrap(),
        64
    );
    let original = project(0.5, cut_beat.raw(), 8 << 32);
    let mut first = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    first
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: first.revision.session().midi_clips()[0].id,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    first.apply(SessionPlaybackCommand::Play).unwrap();
    let mut before_cut = [[0.0; 2]; 64];
    first.render(&mut before_cut).unwrap();

    let slower = project_at_tempo(
        0.5,
        cut_beat.raw(),
        8 << 32,
        Tempo::new(60_000_000).unwrap(),
    );
    let mut second = SessionPlayback::new(revision(&slower, Arc::clone(&table))).unwrap();
    second.try_inherit_state_from(&mut first).unwrap();
    let faster = project_at_tempo(
        0.5,
        cut_beat.raw(),
        8 << 32,
        Tempo::new(180_000_000).unwrap(),
    );
    let mut third = SessionPlayback::new(revision(&faster, Arc::clone(&table))).unwrap();
    third.try_inherit_state_from(&mut second).unwrap();
    let mut actual = [[0.0; 2]; 32];
    third.render(&mut actual).unwrap();
    let voice = table.prepare_voice(48_000, 60, 100).unwrap();
    let gate = third
        .timing_view()
        .beat_to_frame(BeatTime::from_raw(cut_beat.raw() + (8 << 32)))
        .unwrap();
    for (offset, frame) in actual.iter().enumerate() {
        let expected = table.sample(voice, offset as u64, gate - 64) * 0.5;
        assert_eq!(*frame, [expected; 2]);
    }
}

#[test]
fn handoff_remaps_pending_quantized_target_by_beat_and_cancels_missing_clip() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut first = [[0.0; 2]; 64];
    old.render(&mut first).unwrap();
    let clip = old.revision.session().midi_clips()[0].id;
    let track = old.routes[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::FourQuarter,
    })
    .unwrap();

    let slower = project_at_tempo(0.5, 0, 8 << 32, Tempo::new(60_000_000).unwrap());
    let mut next = SessionPlayback::new(revision(&slower, Arc::clone(&table))).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let target = next
        .timing_view()
        .beat_to_frame(BeatTime::from_raw(4 << 32))
        .unwrap();
    assert!(target > next.performance_frame());
    let mut until = vec![[0.0; 2]; (target - next.performance_frame()) as usize];
    next.render(&mut until).unwrap();
    assert_eq!(next.owner(track), Some(SessionPlaybackOwner::Arrangement));
    let mut at_target = [[0.0; 2]; 1];
    next.render(&mut at_target).unwrap();
    assert_eq!(
        next.owner(track),
        Some(SessionPlaybackOwner::Session { clip })
    );

    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut first).unwrap();
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::FourQuarter,
    })
    .unwrap();
    let mut snapshot = original.snapshot();
    snapshot.session.cells.clear();
    let deleted = legacy_project_from_snapshot(snapshot).unwrap();
    let mut candidate = SessionPlayback::new(revision(&deleted, table)).unwrap();
    let report = candidate.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(report.cancelled_pending_tracks(), &[Some(track)]);
    assert_eq!(
        candidate.owner(track),
        Some(SessionPlaybackOwner::Arrangement)
    );
}

#[test]
fn pending_scene_launch_keeps_its_resolved_clip_when_scene_contents_change() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut first = [[0.0; 2]; 64];
    old.render(&mut first).unwrap();
    let clip = old.revision.session().midi_clips()[0].id;
    let track = old.routes[0].id;
    old.apply(SessionPlaybackCommand::LaunchScene {
        scene: old.revision.session().scenes()[0].id,
        quantization: LaunchQuantization::FourQuarter,
    })
    .unwrap();
    assert_eq!(
        old.routes[0].pending.unwrap().target,
        RouteTarget::Session(clip)
    );

    let mut snapshot = original.snapshot();
    snapshot.session.scenes.push(SceneSnapshot {
        id: 7,
        name: "moved clip".into(),
    });
    snapshot.session.cells[0].scene = 7;
    snapshot.session.cells.push(SessionCellSnapshot {
        track: 1,
        scene: 4,
        content: SessionCellContentSnapshot::Stop,
    });
    let edited = legacy_project_from_snapshot(snapshot).unwrap();
    let mut candidate = SessionPlayback::new(revision(&edited, table)).unwrap();
    let report = candidate.try_inherit_state_from(&mut old).unwrap();
    assert!(report.cancelled_pending_tracks().is_empty());
    let route = candidate
        .routes
        .iter()
        .find(|route| route.id == track)
        .unwrap();
    assert_eq!(route.pending.unwrap().target, RouteTarget::Session(clip));
}

#[test]
fn due_pending_quantization_uses_actual_cut_beat_on_shared_frame_plateau() {
    let table = Arc::new(hex_devices::SineTable::new());
    let mut old_snapshot = project(0.5, 0, 8 << 32).snapshot();
    old_snapshot.sample_rate = 48_000;
    let boundary = 4_u64 << 32;
    let later_same_frame = boundary + 80_000;
    old_snapshot.tempo_points = vec![
        TempoPoint::new(BeatTime::ZERO, Tempo::DEFAULT),
        TempoPoint::new(
            BeatTime::from_raw(later_same_frame),
            Tempo::new(60_000_000).unwrap(),
        ),
    ];
    let old_project = legacy_project_from_snapshot(old_snapshot).unwrap();
    let mut old = SessionPlayback::new(revision(&old_project, Arc::clone(&table))).unwrap();
    let track = old.routes[0].id;
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::Seek(95_999)).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::FourQuarter,
    })
    .unwrap();
    assert_eq!(old.routes[0].pending.unwrap().frame, 96_000);
    assert_eq!(old.routes[0].pending.unwrap().beat.raw(), boundary);
    let mut at_cut = [[0.0; 2]; 1];
    old.render(&mut at_cut).unwrap();
    assert_eq!(old.performance_frame(), 96_000);
    assert_eq!(old.routes[0].pending.unwrap().frame, 96_000);
    let effective_cut = old.timing_view().frame_to_beat(96_000).unwrap();
    assert!(effective_cut.raw() > boundary);
    assert_eq!(
        old.timing_view().beat_to_frame(effective_cut).unwrap(),
        96_000
    );

    let mut new_snapshot = old_project.snapshot();
    new_snapshot.tempo_points = vec![TempoPoint::new(
        BeatTime::ZERO,
        Tempo::new(60_000_000).unwrap(),
    )];
    let new_project = legacy_project_from_snapshot(new_snapshot).unwrap();
    let mut candidate = SessionPlayback::new(revision(&new_project, table)).unwrap();
    candidate.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(candidate.routes[0].pending.unwrap().frame, 96_000);
    assert_eq!(candidate.routes[0].pending.unwrap().beat.raw(), boundary);
    let mut launch = [[0.0; 2]; 1];
    candidate.render(&mut launch).unwrap();
    assert_eq!(
        candidate.owner(track),
        Some(SessionPlaybackOwner::Session { clip })
    );
    let RouteOwner::Session { launch, .. } = candidate.routes[0].owner else {
        panic!("due launch selects the Session clip");
    };
    assert_eq!(launch, effective_cut);
    let mut spans = MidiSpanBuffer::default();
    candidate
        .revision
        .session()
        .midi_clip(clip)
        .unwrap()
        .instrument_template()
        .query(launch, &candidate.timing_view(), 96_000, 96_001, &mut spans)
        .unwrap();
    assert!(spans.spans().any(|span| span.start_frame == 96_000));
}

#[test]
fn later_chunk_capacity_refusal_rolls_back_carried_voice_and_retries_against_twin() {
    let table = Arc::new(hex_devices::SineTable::new());
    let base = project_at_tempo(0.5, 0, 1 << 30, Tempo::new(600_000_000).unwrap());
    let mut snapshot = base.snapshot();
    if let SessionCellContentSnapshot::Midi(clip) = &mut snapshot.session.cells[0].content {
        clip.source.notes.push(MidiNoteSnapshot {
            id: 13,
            start_raw: 1_u64 << 27, // begins 1/32 beat after the carried note
            duration_raw: 1 << 30,
            pitch: 67,
            channel: 0,
            velocity: 90,
        });
    }
    // Keep the Arrangement-only note outside the handoff window so launching
    // Session does not create a second, unrelated release-bank entry.
    if let TrackContentSnapshot::Hybrid {
        midi_clips: clips, ..
    } = &mut snapshot.tracks[0].content
    {
        clips[0].notes[0].start_raw = 8 << 32;
    }
    snapshot.next_id = 14;
    let old_project = legacy_project_from_snapshot(snapshot).unwrap();
    let mut old = SessionPlayback::new(revision(&old_project, Arc::clone(&table))).unwrap();
    let track = old.routes[0].id;
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();

    let mut changed_snapshot = old_project.snapshot();
    changed_snapshot.tempo_points = vec![TempoPoint::new(
        BeatTime::ZERO,
        Tempo::new(500_000_000).unwrap(),
    )];
    let changed = legacy_project_from_snapshot(changed_snapshot).unwrap();
    let candidate_revision = revision(&changed, Arc::clone(&table));
    let mut failed = SessionPlayback::new(Arc::clone(&candidate_revision)).unwrap();
    failed.release_capacity = 1;
    failed.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(failed.release_voice_count(), 1);
    failed.routes[0].pending = Some(PendingRoute {
        frame: 256,
        beat: BeatTime::from_raw(1 << 32),
        target: RouteTarget::Silence,
    });

    let mut prefix_probe = SessionPlayback::new(Arc::clone(&candidate_revision)).unwrap();
    prefix_probe.release_capacity = 1;
    prefix_probe.try_inherit_state_from(&mut old).unwrap();
    prefix_probe.routes[0].pending = failed.routes[0].pending;
    let mut first_chunk = [[0.0; 2]; 256];
    prefix_probe.render(&mut first_chunk).unwrap();
    assert!(first_chunk.iter().any(|frame| frame[0] != 0.0));
    assert_eq!(prefix_probe.performance_frame(), 256);

    let mut twin = SessionPlayback::new(candidate_revision).unwrap();
    twin.release_capacity = 1;
    twin.try_inherit_state_from(&mut old).unwrap();
    twin.routes[0].pending = failed.routes[0].pending;

    let routes = failed.routes.clone();
    let releases = failed.releases.clone();
    let suppressions = failed.suppressions.clone();
    let release_order = failed.release_order;
    let route_generation = failed.route_state_generation();
    let mut whole = [[9.0; 2]; 512];
    assert_eq!(
        failed.render(&mut whole),
        Err(SessionPlaybackError::ReleaseBankFull)
    );
    assert_eq!(whole, [[0.0; 2]; 512]);
    assert_eq!(failed.performance_frame(), 0);
    assert_eq!(failed.arrangement_frame(), 0);
    assert_eq!(failed.route_state_generation(), route_generation);
    assert_eq!(failed.routes, routes);
    assert_eq!(failed.release_order, release_order);
    assert_eq!(failed.releases.len(), releases.len());
    assert_eq!(failed.suppressions, suppressions);
    assert_eq!(
        failed.owner(track),
        Some(SessionPlaybackOwner::Session { clip })
    );
    assert_eq!(failed.routes[0].pending, routes[0].pending);
    assert_eq!(
        failed.releases[0].voice.end_frame(),
        releases[0].voice.end_frame()
    );

    failed
        .apply(SessionPlaybackCommand::CancelPending { track: None })
        .unwrap();
    twin.apply(SessionPlaybackCommand::CancelPending { track: None })
        .unwrap();
    let mut retry = [[0.0; 2]; 512];
    let mut expected = [[0.0; 2]; 512];
    failed.render(&mut retry).unwrap();
    twin.render(&mut expected).unwrap();
    assert_eq!(retry, expected);
}

#[test]
fn streamed_later_chunk_refusal_zeros_block_and_rolls_back_after_page_requests() {
    let table = Arc::new(hex_devices::SineTable::new());
    let mut original_snapshot = mixed_audio_midi_project(5, 18).snapshot();
    let original_note_id = original_snapshot.next_id;
    let Some(SessionCellContentSnapshot::Midi(original_clip)) = original_snapshot
        .session
        .cells
        .iter_mut()
        .find(|cell| cell.track == 6)
        .map(|cell| &mut cell.content)
    else {
        panic!("Session MIDI fixture expected")
    };
    original_clip.source.notes.push(MidiNoteSnapshot {
        id: original_note_id,
        start_raw: 0,
        duration_raw: 1 << 30,
        pitch: 60,
        channel: 0,
        velocity: 100,
    });
    original_snapshot.next_id += 1;
    if let TrackContentSnapshot::Hybrid {
        midi_clips: clips, ..
    } = &mut original_snapshot.tracks[1].content
    {
        clips[0].notes.clear();
    }
    let original = legacy_project_from_snapshot(original_snapshot).unwrap();
    let stream_asset = original
        .assets()
        .find(|asset| asset.id().value() == 4)
        .unwrap();
    let stream_source = StreamedSourceMetadata::new(
        StreamSourceKey::new(71, stream_asset.id()),
        stream_asset.source_sample_rate(),
        original.sample_rate(),
        stream_asset.source_frames(),
        stream_asset.playback_frames(),
        stream_asset.conversion_version(),
        [0x71; 32],
    )
    .unwrap();
    let make_sources = |project: &Project| {
        let resident = crate::Audio::new(48_000, vec![[0.25, -0.25]; 32]).unwrap();
        project
            .assets()
            .map(|asset| {
                let source = if asset.id().value() == 4 {
                    PreparedAudioSource::Streamed(Arc::new(stream_source.clone()))
                } else {
                    PreparedAudioSource::Resident(resident.clone())
                };
                (asset.id(), source)
            })
            .collect::<BTreeMap<_, _>>()
    };
    let old_revision = Arc::new(
        PreparedRevision::prepare_with_prepared_audio_sources(
            &original,
            &make_sources(&original),
            71,
            Arc::clone(&table),
            None,
        )
        .unwrap(),
    );
    let mut old = SessionPlayback::new(old_revision).unwrap();
    let midi_clip = old
        .revision
        .session()
        .midi_clips()
        .iter()
        .find(|clip| clip.track.value() == 6)
        .unwrap()
        .id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip: midi_clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();

    let mut changed_snapshot = original.snapshot();
    let added_note_id = changed_snapshot.next_id;
    let Some(SessionCellContentSnapshot::Midi(clip)) = changed_snapshot
        .session
        .cells
        .iter_mut()
        .find(|cell| cell.track == 6)
        .map(|cell| &mut cell.content)
    else {
        panic!("Session MIDI fixture expected")
    };
    clip.source.notes.push(MidiNoteSnapshot {
        id: added_note_id,
        start_raw: 1_u64 << 27,
        duration_raw: 1 << 30,
        pitch: 67,
        channel: 0,
        velocity: 90,
    });
    changed_snapshot.next_id += 1;
    if let TrackContentSnapshot::Hybrid {
        midi_clips: clips, ..
    } = &mut changed_snapshot.tracks[1].content
    {
        clips[0].notes.clear();
    }
    changed_snapshot.tempo_points = vec![TempoPoint::new(
        BeatTime::ZERO,
        Tempo::new(500_000_000).unwrap(),
    )];
    let changed = legacy_project_from_snapshot(changed_snapshot).unwrap();
    let candidate_revision = Arc::new(
        PreparedRevision::prepare_with_prepared_audio_sources(
            &changed,
            &make_sources(&changed),
            71,
            Arc::clone(&table),
            Some(old.revision.session()),
        )
        .unwrap(),
    );
    let mut failed = SessionPlayback::new(Arc::clone(&candidate_revision)).unwrap();
    failed.release_capacity = 1;
    failed.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(failed.release_voice_count(), 1);
    let audio_clip = failed.revision.session().audio_clips()[0].id;
    failed
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: audio_clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    failed.routes[1].pending = Some(PendingRoute {
        frame: 256,
        beat: BeatTime::from_raw(1 << 32),
        target: RouteTarget::Silence,
    });

    #[derive(Default)]
    struct MissingPages(Vec<(StreamSourceKey, u64)>);
    impl MediaPages for MissingPages {
        fn page(
            &mut self,
            _key: StreamSourceKey,
            _page_index: u64,
        ) -> Option<BorrowedMediaPage<'_>> {
            None
        }

        fn request_page(&mut self, key: StreamSourceKey, page_index: u64) {
            self.0.push((key, page_index));
        }
    }

    let routes = failed.routes.clone();
    let release_end = failed.releases[0].voice.end_frame();
    let route_generation = failed.route_state_generation();
    let mut pages = MissingPages::default();
    let mut output = [[9.0; 2]; 512];
    assert_eq!(
        failed.render_with_pages(&mut output, &mut pages),
        Err(SessionPlaybackError::ReleaseBankFull)
    );
    assert_eq!(output, [[0.0; 2]; 512]);
    assert_eq!(pages.0, [(stream_source.key(), 0)]);
    assert_eq!(failed.performance_frame(), 0);
    assert_eq!(failed.arrangement_frame(), 0);
    assert_eq!(failed.route_state_generation(), route_generation);
    assert_eq!(failed.routes, routes);
    assert_eq!(failed.release_voice_count(), 1);
    assert_eq!(failed.releases[0].voice.end_frame(), release_end);
}

#[test]
fn deleting_the_last_track_keeps_its_copied_session_voice_tail() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip: old.revision.session().midi_clips()[0].id,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut before = [[0.0; 2]; 64];
    old.render(&mut before).unwrap();
    let mut snapshot = original.snapshot();
    snapshot.tracks.clear();
    snapshot.session.cells.clear();
    let deleted = legacy_project_from_snapshot(snapshot).unwrap();
    let mut candidate = SessionPlayback::new(revision(&deleted, table)).unwrap();
    candidate.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(candidate.release_voice_count(), 2);
    assert_eq!(candidate.performance_frame(), old.performance_frame());
    assert_eq!(candidate.arrangement_frame(), old.arrangement_frame());
    candidate
        .apply(SessionPlaybackCommand::BackToArrangement {
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    candidate
        .apply(SessionPlaybackCommand::StopAllClips {
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    assert_eq!(candidate.release_voice_count(), 2);
    let mut tail = [[0.0; 2]; 64];
    candidate.render(&mut tail).unwrap();
    assert!(tail.iter().any(|frame| frame[0] != 0.0));
}

#[test]
fn reordered_tracks_keep_project_order_and_deleted_arrangement_tail_uses_old_mix() {
    let table = Arc::new(hex_devices::SineTable::new());
    let base = project(0.5, 0, 8 << 32);
    let mut snapshot = base.snapshot();
    snapshot.next_id = 15;
    for (track_id, clip_id, note_id, gain) in [(7, 9, 14, 0.25), (10, 11, 12, 0.75)] {
        snapshot.tracks.push(TrackSnapshot {
            id: track_id,
            name: format!("track {track_id}"),
            gain,
            balance: 0.2,
            muted: false,
            soloed: false,
            devices: Vec::new(),
            instrument: None,
            content: TrackContentSnapshot::Instrument(vec![MidiClipSnapshot {
                id: clip_id,
                start_raw: 0,
                length_raw: 16 << 32,
                source_length_raw: 32 << 32,
                source_start_raw: 0,
                loop_start_raw: 0,
                loop_end_raw: 32 << 32,
                looping: false,
                notes: vec![MidiNoteSnapshot {
                    id: note_id,
                    start_raw: 0,
                    duration_raw: 8 << 32,
                    pitch: 60 + track_id as u8,
                    channel: 0,
                    velocity: 90,
                }],
            }]),
        });
    }
    let original = legacy_project_from_snapshot(snapshot).unwrap();
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut before = [[0.0; 2]; 64];
    old.render(&mut before).unwrap();

    let mut edited = original.snapshot();
    edited.tracks = vec![edited.tracks[1].clone(), edited.tracks[0].clone()];
    let candidate_project = legacy_project_from_snapshot(edited).unwrap();
    let candidate_revision = revision(&candidate_project, Arc::clone(&table));
    let mut candidate = SessionPlayback::new(Arc::clone(&candidate_revision)).unwrap();
    candidate.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(candidate.routes[0].id.value(), 7);
    assert_eq!(candidate.routes[1].id.value(), 1);
    let deleted_tail = candidate
        .releases
        .iter()
        .find(|entry| entry.track_id.value() == 10)
        .copied()
        .unwrap();

    let mut actual = [[0.0; 2]; 64];
    candidate.render(&mut actual).unwrap();
    let mut expected = [[0.0; 2]; 64];
    for track in candidate_revision.arrangement().tracks() {
        track
            .render_additive(candidate.arrangement_frame - 64, &mut expected)
            .unwrap();
    }
    for (offset, frame) in expected.iter_mut().enumerate() {
        let absolute = candidate.performance_frame - 64 + offset as u64;
        if let Some(sample) = deleted_tail.voice.sample_at(absolute) {
            deleted_tail.mix.add_frame(frame, sample);
        }
    }
    candidate_revision
        .arrangement()
        .master_mix()
        .apply(&mut expected);
    assert_eq!(actual, expected);
}

#[test]
fn source_start_and_loop_edit_releases_active_intro_occurrence_without_reusing_phase_key() {
    let table = Arc::new(hex_devices::SineTable::new());
    let base = project(0.5, 0, 8 << 32);
    let mut snapshot = base.snapshot();
    if let TrackContentSnapshot::Hybrid {
        midi_clips: clips, ..
    } = &mut snapshot.tracks[0].content
    {
        let clip = &mut clips[0];
        clip.source_start_raw = 1 << 32;
        clip.loop_start_raw = 2 << 32;
        clip.loop_end_raw = 4 << 32;
        clip.looping = true;
        clip.notes = vec![
            MidiNoteSnapshot {
                id: 3,
                start_raw: 3_u64 << 31, // intro onset, gate crosses the first loop seam
                duration_raw: 4 << 32,
                pitch: 60,
                channel: 0,
                velocity: 100,
            },
            MidiNoteSnapshot {
                id: 13,
                start_raw: 5_u64 << 31, // repeated loop phase
                duration_raw: 1 << 32,
                pitch: 67,
                channel: 0,
                velocity: 90,
            },
        ];
    }
    if let SessionCellContentSnapshot::Midi(clip) = &mut snapshot.session.cells[0].content {
        clip.source.source_start_raw = 1 << 32;
        clip.source.loop_start_raw = 2 << 32;
        clip.source.loop_end_raw = 4 << 32;
        clip.source.looping = true;
        clip.source.notes = vec![
            MidiNoteSnapshot {
                id: 14,
                start_raw: 3_u64 << 31,
                duration_raw: 4 << 32,
                pitch: 60,
                channel: 0,
                velocity: 100,
            },
            MidiNoteSnapshot {
                id: 15,
                start_raw: 5_u64 << 31,
                duration_raw: 1 << 32,
                pitch: 67,
                channel: 0,
                velocity: 90,
            },
        ];
    }
    snapshot.next_id = 16;
    let original = legacy_project_from_snapshot(snapshot).unwrap();
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut intro = vec![[0.0; 2]; 24_000];
    old.render(&mut intro).unwrap();

    let mut edited_snapshot = original.snapshot();
    if let TrackContentSnapshot::Hybrid {
        midi_clips: clips, ..
    } = &mut edited_snapshot.tracks[0].content
    {
        clips[0].source_start_raw = 0;
        clips[0].loop_start_raw = 2 << 32;
        clips[0].loop_end_raw = 5 << 32;
    }
    if let SessionCellContentSnapshot::Midi(clip) = &mut edited_snapshot.session.cells[0].content {
        clip.source.source_start_raw = 0;
        clip.source.loop_start_raw = 2 << 32;
        clip.source.loop_end_raw = 5 << 32;
    }
    let edited = legacy_project_from_snapshot(edited_snapshot).unwrap();
    let mut candidate = SessionPlayback::new(revision(&edited, table)).unwrap();
    candidate.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(
        candidate.owner(old.routes[0].id),
        Some(SessionPlaybackOwner::Session { clip })
    );
    assert!(
        candidate
            .releases
            .iter()
            .any(|entry| entry.continuation.is_none() && entry.voice.end_frame() > 24_000)
    );
}

#[test]
fn moving_selected_clip_to_another_track_releases_old_route() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    let old_track = old.routes[0].id;
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut before = [[0.0; 2]; 64];
    old.render(&mut before).unwrap();

    let mut snapshot = original.snapshot();
    snapshot.tracks.push(TrackSnapshot {
        id: 7,
        name: "other".into(),
        gain: 1.0,
        balance: 0.0,
        muted: false,
        soloed: false,
        devices: Vec::new(),
        instrument: None,
        content: TrackContentSnapshot::Instrument(Vec::new()),
    });
    snapshot.session.cells[0].track = 7;
    let moved = legacy_project_from_snapshot(snapshot).unwrap();
    let mut candidate = SessionPlayback::new(revision(&moved, table)).unwrap();
    candidate.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(
        candidate.owner(old_track),
        Some(SessionPlaybackOwner::Silence)
    );
    assert_eq!(candidate.release_voice_count(), 2);
}

#[test]
fn unrepresentable_matching_gate_remap_refuses_without_candidate_commit() {
    let table = Arc::new(hex_devices::SineTable::new());
    let base = project(0.5, 0, 8 << 32);
    let mut snapshot = base.snapshot();
    if let TrackContentSnapshot::Hybrid {
        midi_clips: clips, ..
    } = &mut snapshot.tracks[0].content
    {
        clips[0].length_raw = u64::MAX;
        clips[0].source_length_raw = u64::MAX;
        clips[0].loop_end_raw = u64::MAX;
        clips[0].notes[0].duration_raw = u64::MAX;
    }
    if let SessionCellContentSnapshot::Midi(clip) = &mut snapshot.session.cells[0].content {
        clip.source.source_length_raw = u64::MAX;
        clip.source.loop_end_raw = u64::MAX;
        clip.source.notes[0].duration_raw = u64::MAX;
    }
    let long = legacy_project_from_snapshot(snapshot).unwrap();
    let mut old = SessionPlayback::new(revision(&long, Arc::clone(&table))).unwrap();
    let track = old.routes[0].id;
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let cut = old
        .revision
        .session()
        .timing()
        .beat_to_frame(BeatTime::from_raw(1_u64 << 62))
        .unwrap();
    old.performance_frame = cut;
    old.arrangement_frame = cut;

    let mut faster_snapshot = long.snapshot();
    faster_snapshot.tempo_points = vec![TempoPoint::new(
        BeatTime::ZERO,
        Tempo::new(240_000_000).unwrap(),
    )];
    let faster = legacy_project_from_snapshot(faster_snapshot).unwrap();
    let mut candidate = SessionPlayback::new(revision(&faster, table)).unwrap();
    assert_eq!(
        candidate.try_inherit_state_from(&mut old),
        Err(SessionPlaybackError::TempoConversion)
    );
    assert_eq!(candidate.performance_frame(), 0);
    assert_eq!(
        candidate.owner(track),
        Some(SessionPlaybackOwner::Arrangement)
    );
    assert_eq!(candidate.release_voice_count(), 0);
}

#[test]
fn reduced_shared_bank_refuses_a_cut_atomically_and_recovers_after_expiry() {
    let table = Arc::new(hex_devices::SineTable::new());
    let project = project(0.5, 0, 8 << 32);
    let mut playback = SessionPlayback::new(revision(&project, table)).unwrap();
    let track = playback.routes[0].id;
    let clip = playback.revision.session().midi_clips()[0].id;
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut initial = [[0.0; 2]; 2_500];
    playback.render(&mut initial).unwrap();
    playback.release_capacity = 1;
    playback
        .apply(SessionPlaybackCommand::StopTrack {
            track,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    assert_eq!(playback.release_voice_count(), 1);
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();

    assert_eq!(
        playback.apply(SessionPlaybackCommand::StopTrack {
            track,
            quantization: LaunchQuantization::Immediate,
        }),
        Err(SessionPlaybackError::ReleaseBankFull)
    );
    assert_eq!(playback.release_voice_count(), 1);
    assert_eq!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Session { clip })
    );

    let mut after_expiry = [[0.0; 2]; 2_500];
    playback.render(&mut after_expiry).unwrap();
    assert_eq!(playback.release_voice_count(), 0);
    playback
        .apply(SessionPlaybackCommand::StopTrack {
            track,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    assert_eq!(playback.owner(track), Some(SessionPlaybackOwner::Silence));
}

#[test]
fn route_status_generation_tracks_committed_commands_and_due_transitions() {
    let table = Arc::new(hex_devices::SineTable::new());
    let project = project(0.5, 0, 8 << 32);
    let revision = revision(&project, Arc::clone(&table));
    let track = project.tracks()[0].id();
    let clip = match project.session_cells()[0].content() {
        hex_project::SessionCellContent::Midi(clip) => clip.id(),
        hex_project::SessionCellContent::Audio(_) => panic!("expected Session MIDI clip"),
        hex_project::SessionCellContent::Stop => panic!("expected Session clip"),
    };
    let mut playback = SessionPlayback::new(revision).unwrap();
    let initial = playback.route_state_generation();
    let route = playback.route_status(0).unwrap();
    assert_eq!(route.track, track);
    assert_eq!(route.owner, SessionPlaybackOwner::Arrangement);
    assert_eq!(route.active_launch, None);
    assert_eq!(route.pending, None);

    playback.apply(SessionPlaybackCommand::Play).unwrap();
    assert_eq!(playback.route_state_generation(), initial);
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    let launched = playback.route_state_generation();
    assert_eq!(launched, initial.wrapping_add(1));
    let route = playback.route_status(0).unwrap();
    assert_eq!(route.owner, SessionPlaybackOwner::Session { clip });
    assert_eq!(route.active_launch, Some(BeatTime::ZERO));

    let mut advance = [[0.0; 2]; 64];
    playback.render(&mut advance).unwrap();
    let before_pending = playback.route_state_generation();
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::FourQuarter,
        })
        .unwrap();
    let pending_generation = playback.route_state_generation();
    assert_eq!(pending_generation, before_pending.wrapping_add(1));
    let route = playback.route_status(0).unwrap();
    let pending = route.pending.unwrap();
    assert_eq!(pending.target, SessionRouteTarget::Session { clip });
    assert_eq!(pending.performance_frame, 96_000);
    assert_eq!(pending.beat, BeatTime::from_quarters(4).unwrap());

    playback
        .apply(SessionPlaybackCommand::CancelPending { track: Some(track) })
        .unwrap();
    assert_eq!(
        playback.route_state_generation(),
        pending_generation.wrapping_add(1)
    );
    assert_eq!(playback.route_status(0).unwrap().pending, None);

    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::FourQuarter,
        })
        .unwrap();
    let due_generation = playback.route_state_generation();
    let mut until_due = vec![[0.0; 2]; 96_000 - 64];
    playback.render(&mut until_due).unwrap();
    let mut first_due_block = [[0.0; 2]; 1];
    playback.render(&mut first_due_block).unwrap();
    assert_eq!(
        playback.route_state_generation(),
        due_generation.wrapping_add(1)
    );
    assert_eq!(playback.route_status(0).unwrap().pending, None);

    let before_stop = playback.route_state_generation();
    playback.apply(SessionPlaybackCommand::Stop).unwrap();
    assert_eq!(
        playback.route_state_generation(),
        before_stop.wrapping_add(1)
    );
    assert_eq!(playback.performance_frame(), 0);
    assert_eq!(
        playback.route_status(0).unwrap().active_launch,
        Some(BeatTime::ZERO)
    );
}

#[test]
fn session_audio_matches_direct_frame_oracle_over_irregular_blocks_and_ends_silently() {
    let source: Vec<Frame> = (0..32)
        .map(|index| [index as f32 * 0.013 - 0.17, 0.31 - index as f32 * 0.007])
        .collect();
    let audio = crate::Audio::new(48_000, source.clone()).unwrap();
    let other = crate::Audio::new(48_000, vec![[0.0; 2]; 32]).unwrap();
    let project = audio_project(5, 18, 4);
    let media = audio_media(&project, &audio, &other);
    let prepared = Arc::new(
        PreparedRevision::prepare(
            &project,
            &media,
            Arc::new(hex_devices::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let clip = prepared.session().audio_clips()[0].id;
    let mut playback = SessionPlayback::new(prepared).unwrap();
    let track = playback.routes[0].id;
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut rendered = Vec::new();
    for width in [1, 5, 2, 7, 4, 9, 3] {
        let mut block = vec![[0.0; 2]; width];
        playback.render(&mut block).unwrap();
        rendered.extend(block);
    }
    for (elapsed, actual) in rendered.iter().enumerate() {
        let expected = if elapsed < 13 {
            source[5 + elapsed]
        } else {
            [0.0; 2]
        };
        assert_eq!(*actual, expected, "source-relative frame {elapsed}");
    }
    assert_eq!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Session { clip })
    );
}

#[test]
fn session_audio_seek_pause_stop_scene_stop_and_return_are_explicit_and_keep_midi_routes() {
    let source: Vec<Frame> = (0..32)
        .map(|index| [index as f32 * 0.011 + 0.1, 0.8 - index as f32 * 0.009])
        .collect();
    let audio = crate::Audio::new(48_000, source.clone()).unwrap();
    let other = crate::Audio::new(48_000, vec![[0.0; 2]; 32]).unwrap();
    let project = mixed_audio_midi_project(5, 18);
    let media = audio_media(&project, &audio, &other);
    let revision = Arc::new(
        PreparedRevision::prepare(
            &project,
            &media,
            Arc::new(hex_devices::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let audio_clip = revision.session().audio_clips()[0].id;
    let midi_clip = revision.session().midi_clips()[0].id;
    let audio_track = revision.session().audio_clips()[0].track;
    let midi_track = revision.session().midi_clips()[0].track;
    let scene = revision.session().scenes()[0].id;
    let stop_scene = revision.session().scenes()[1].id;
    let mut playback = SessionPlayback::new(revision).unwrap();
    playback
        .apply(SessionPlaybackCommand::LaunchScene {
            scene,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    assert_eq!(
        playback.owner(audio_track),
        Some(SessionPlaybackOwner::Session { clip: audio_clip })
    );
    assert_eq!(
        playback.owner(midi_track),
        Some(SessionPlaybackOwner::Session { clip: midi_clip })
    );
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: audio_clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut first = [[0.0; 2]; 3];
    playback.render(&mut first).unwrap();
    assert_eq!(first[0], source[5]);
    playback.apply(SessionPlaybackCommand::Pause).unwrap();
    let position = playback.performance_frame();
    let mut paused = [[1.0; 2]; 2];
    playback.render(&mut paused).unwrap();
    assert_eq!(paused, [[0.0; 2]; 2]);
    assert_eq!(playback.performance_frame(), position);
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut resumed = [[0.0; 2]; 1];
    playback.render(&mut resumed).unwrap();
    assert_eq!(resumed[0], source[8]);
    playback.apply(SessionPlaybackCommand::Seek(100)).unwrap();
    let mut seeked = [[0.0; 2]; 1];
    playback.render(&mut seeked).unwrap();
    assert_eq!(seeked[0], source[5]);
    playback.apply(SessionPlaybackCommand::Stop).unwrap();
    assert_eq!(playback.performance_frame(), 0);
    assert_eq!(
        playback.owner(audio_track),
        Some(SessionPlaybackOwner::Session { clip: audio_clip })
    );
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut restarted = [[0.0; 2]; 1];
    playback.render(&mut restarted).unwrap();
    assert_eq!(restarted[0], source[5]);
    playback
        .apply(SessionPlaybackCommand::LaunchScene {
            scene: stop_scene,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    assert_eq!(
        playback.owner(audio_track),
        Some(SessionPlaybackOwner::Silence)
    );
    assert_eq!(
        playback.owner(midi_track),
        Some(SessionPlaybackOwner::Session { clip: midi_clip })
    );
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: audio_clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    playback
        .apply(SessionPlaybackCommand::BackToArrangement {
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    assert_eq!(
        playback.owner(audio_track),
        Some(SessionPlaybackOwner::Arrangement)
    );
    assert_eq!(
        playback.owner(midi_track),
        Some(SessionPlaybackOwner::Arrangement)
    );
}

#[test]
fn session_audio_midi_and_arrangement_routes_mix_additively_against_independent_renders() {
    let source: Vec<Frame> = (0..32)
        .map(|index| [0.01 * (index as f32 + 1.0), -0.006 * (index as f32 + 2.0)])
        .collect();
    let audio = crate::Audio::new(48_000, source).unwrap();
    let other = crate::Audio::new(48_000, vec![[0.0; 2]; 32]).unwrap();
    let project = session_audio_midi_arrangement_mix_project();
    let media = audio_media(&project, &audio, &other);
    let render = |launch_audio: bool, launch_midi: bool| {
        let revision = Arc::new(
            PreparedRevision::prepare(
                &project,
                &media,
                Arc::new(hex_devices::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let audio_clip = revision.session().audio_clips()[0].id;
        let midi_clip = revision.session().midi_clips()[0].id;
        let mut playback = SessionPlayback::new(revision).unwrap();
        if launch_audio {
            playback
                .apply(SessionPlaybackCommand::LaunchClip {
                    clip: audio_clip,
                    quantization: LaunchQuantization::Immediate,
                })
                .unwrap();
        }
        if launch_midi {
            playback
                .apply(SessionPlaybackCommand::LaunchClip {
                    clip: midi_clip,
                    quantization: LaunchQuantization::Immediate,
                })
                .unwrap();
        }
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        let mut output = [[0.0; 2]; 32];
        playback.render(&mut output).unwrap();
        output
    };
    let both = render(true, true);
    let audio_only = render(true, false);
    let midi_only = render(false, true);
    let arrangement_only = render(false, false);
    for index in 0..both.len() {
        for channel in 0..2 {
            let expected = audio_only[index][channel] + midi_only[index][channel]
                - arrangement_only[index][channel];
            assert!(
                (both[index][channel] - expected).abs() <= 1.0e-6,
                "frame {index} channel {channel}: actual {}, independent sum {expected}",
                both[index][channel]
            );
        }
    }
    assert!(both.iter().any(|frame| frame[0].abs() > 0.02));
}

#[test]
fn session_audio_quantization_cancel_and_tempo_handoff_keep_frame_domain() {
    let source_a: Vec<Frame> = (0..32)
        .map(|index| [index as f32 + 1.0, -(index as f32 + 2.0)])
        .collect();
    let source_b: Vec<Frame> = (0..32)
        .map(|index| [200.0 + index as f32, -400.0 - index as f32])
        .collect();
    let audio_a = crate::Audio::new(48_000, source_a.clone()).unwrap();
    let audio_b = crate::Audio::new(48_000, source_b.clone()).unwrap();
    let base = audio_project(5, 18, 4);
    let media = audio_media(&base, &audio_a, &audio_b);
    let mut playback = SessionPlayback::new(Arc::new(
        PreparedRevision::prepare(&base, &media, Arc::new(hex_devices::SineTable::new()), None)
            .unwrap(),
    ))
    .unwrap();
    let clip = playback.revision.session().audio_clips()[0].id;
    let track = playback.routes[0].id;
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut initial_frame = [[0.0; 2]; 1];
    playback.render(&mut initial_frame).unwrap();
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::FourQuarter,
        })
        .unwrap();
    assert_eq!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Arrangement)
    );
    assert!(playback.route_status(0).unwrap().pending.is_some());
    playback
        .apply(SessionPlaybackCommand::CancelPending { track: Some(track) })
        .unwrap();
    assert_eq!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Arrangement)
    );
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    let mut prefix = [[0.0; 2]; 5];
    playback.render(&mut prefix).unwrap();

    let mut replacement_snapshot = base.snapshot();
    replacement_snapshot.tempo_points = vec![TempoPoint::new(
        BeatTime::ZERO,
        Tempo::new(60_000_000).unwrap(),
    )];
    let SessionCellContentSnapshot::Audio(audio_clip) =
        &mut replacement_snapshot.session.cells[0].content
    else {
        panic!("audio cell expected")
    };
    audio_clip.asset = 5;
    audio_clip.source_start = 2;
    audio_clip.source_end = 20;
    let replacement = legacy_project_from_snapshot(replacement_snapshot).unwrap();
    let replacement_media = audio_media(&replacement, &audio_a, &audio_b);
    let candidate_revision = Arc::new(
        PreparedRevision::prepare(
            &replacement,
            &replacement_media,
            Arc::clone(playback.revision.session().sine_table()),
            Some(playback.revision.session()),
        )
        .unwrap(),
    );
    let mut candidate = SessionPlayback::new(candidate_revision).unwrap();
    candidate.try_inherit_state_from(&mut playback).unwrap();
    let mut next = [[0.0; 2]; 1];
    candidate.render(&mut next).unwrap();
    assert_eq!(next[0], source_b[7]);
    assert_eq!(
        candidate.owner(track),
        Some(SessionPlaybackOwner::Session { clip })
    );
    assert_eq!(candidate.performance_frame(), 7);

    let mut short_snapshot = replacement.snapshot();
    let SessionCellContentSnapshot::Audio(audio_clip) =
        &mut short_snapshot.session.cells[0].content
    else {
        panic!("audio cell expected")
    };
    audio_clip.source_start = 2;
    audio_clip.source_end = 4;
    let short = legacy_project_from_snapshot(short_snapshot).unwrap();
    let short_media = audio_media(&short, &audio_a, &audio_b);
    let short_revision = Arc::new(
        PreparedRevision::prepare(
            &short,
            &short_media,
            Arc::clone(candidate.revision.session().sine_table()),
            Some(candidate.revision.session()),
        )
        .unwrap(),
    );
    let mut shortened = SessionPlayback::new(short_revision).unwrap();
    shortened.try_inherit_state_from(&mut candidate).unwrap();
    let mut silent = [[1.0; 2]; 3];
    shortened.render(&mut silent).unwrap();
    assert_eq!(silent, [[0.0; 2]; 3]);
    assert_eq!(
        shortened.owner(track),
        Some(SessionPlaybackOwner::Session { clip })
    );
}

#[test]
fn session_audio_quantized_launch_begins_on_the_due_performance_frame() {
    let source: Vec<Frame> = (0..32)
        .map(|index| [index as f32 + 0.25, -((index as f32) + 0.5)])
        .collect();
    let audio = crate::Audio::new(48_000, source.clone()).unwrap();
    let other = crate::Audio::new(48_000, vec![[0.0; 2]; 32]).unwrap();
    let project = audio_project(5, 18, 4);
    let media = audio_media(&project, &audio, &other);
    let revision = Arc::new(
        PreparedRevision::prepare(
            &project,
            &media,
            Arc::new(hex_devices::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let clip = revision.session().audio_clips()[0].id;
    let track = revision.session().audio_clips()[0].track;
    let mut playback = SessionPlayback::new(revision).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut first_frame = [[0.0; 2]; 1];
    playback.render(&mut first_frame).unwrap();
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::FourQuarter,
        })
        .unwrap();
    assert_eq!(
        playback
            .route_status(0)
            .unwrap()
            .pending
            .unwrap()
            .performance_frame,
        96_000
    );

    let mut before_due = vec![[1.0; 2]; 96_000 - 1];
    playback.render(&mut before_due).unwrap();
    assert!(before_due.iter().all(|frame| *frame == [0.0; 2]));
    assert_eq!(playback.performance_frame(), 96_000);
    assert_eq!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Arrangement)
    );

    let mut rendered = Vec::new();
    for width in [1, 3, 2, 5, 4, 7] {
        let mut block = vec![[0.0; 2]; width];
        playback.render(&mut block).unwrap();
        rendered.extend(block);
    }
    for (elapsed, actual) in rendered.iter().enumerate() {
        let expected = if elapsed < 13 {
            source[5 + elapsed]
        } else {
            [0.0; 2]
        };
        assert_eq!(*actual, expected, "due-relative frame {elapsed}");
    }
    assert_eq!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Session { clip })
    );
}

#[test]
fn prepared_session_audio_refuses_missing_rate_or_length_mismatched_pcm_and_reuses_shared_pcm() {
    let project = audio_project(1, 7, 4);
    let audio = crate::Audio::new(48_000, vec![[0.2, -0.3]; 32]).unwrap();
    let other = crate::Audio::new(48_000, vec![[0.4, -0.5]; 32]).unwrap();
    let media = audio_media(&project, &audio, &other);
    let first = PreparedSessionInventory::prepare_with_media(
        &project,
        &media,
        Arc::new(hex_devices::SineTable::new()),
        None,
    )
    .unwrap();
    let second = PreparedSessionInventory::prepare_with_media(
        &project,
        &media,
        Arc::clone(first.sine_table()),
        Some(&first),
    )
    .unwrap();
    assert!(
        second
            .audio_clip(first.audio_clips()[0].id)
            .unwrap()
            .source_matches(&first.audio_clips()[0])
    );
    let changed_pcm = crate::Audio::new(48_000, vec![[9.0, -9.0]; 32]).unwrap();
    let changed_media = audio_media(&project, &changed_pcm, &other);
    let changed = PreparedSessionInventory::prepare_with_media(
        &project,
        &changed_media,
        Arc::clone(first.sine_table()),
        Some(&first),
    )
    .unwrap();
    assert!(
        !changed
            .audio_clip(first.audio_clips()[0].id)
            .unwrap()
            .source_matches(&first.audio_clips()[0])
    );
    assert_eq!(
        changed
            .audio_clip(first.audio_clips()[0].id)
            .unwrap()
            .resident_audio()
            .unwrap()
            .frames()[0],
        [9.0, -9.0]
    );
    assert!(
        PreparedSessionInventory::prepare_with_media(
            &project,
            &BTreeMap::new(),
            Arc::new(hex_devices::SineTable::new()),
            None
        )
        .is_err()
    );
    let wrong_rate = crate::Audio::new(44_100, vec![[0.0; 2]; 32]).unwrap();
    let bad_media = audio_media(&project, &wrong_rate, &other);
    assert!(
        PreparedSessionInventory::prepare_with_media(
            &project,
            &bad_media,
            Arc::new(hex_devices::SineTable::new()),
            None
        )
        .is_err()
    );
    let short_pcm = crate::Audio::new(48_000, vec![[0.0; 2]; 31]).unwrap();
    let bad_length = audio_media(&project, &short_pcm, &other);
    assert!(
        PreparedSessionInventory::prepare_with_media(
            &project,
            &bad_length,
            Arc::new(hex_devices::SineTable::new()),
            None
        )
        .is_err()
    );
}
