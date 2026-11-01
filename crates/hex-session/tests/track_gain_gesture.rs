//! Public actor contract for held track-volume gestures, independent of UI state.
use hex_lattice_contract::ParameterValue;
use hex_project::{BeatTime, Command, Created, TrackId};
use hex_session::{
    device_gesture::{GestureKey, GestureRequest, GestureTarget},
    service::{Event, Request, Service},
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

fn wait(service: &Service) -> Event {
    let deadline = Instant::now() + std::time::Duration::from_secs(60);
    loop {
        match service.try_event() {
            Ok(event) => return event,
            Err(std::sync::mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1))
            }
            Err(error) => panic!("missing actor event: {error}"),
        }
    }
}
fn request(service: &Service, request: Request) -> Event {
    service.submit(request).unwrap();
    wait(service)
}
fn edit(service: &Service, command: Command) -> Event {
    let event = request(service, Request::Edit(command));
    assert!(event.error.is_none(), "{:?}", event.error);
    event
}
fn fixture() -> (Service, TrackId, TrackId) {
    let service = Service::new(8_000, 32, 0).unwrap();
    wait(&service);
    let Some(Created::Track(track)) = edit(
        &service,
        Command::InsertTrackWithDevice {
            name: "held voice".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        },
    )
    .created
    else {
        panic!("track")
    };
    let Some(Created::Clip(clip)) = edit(
        &service,
        Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(8).unwrap(),
        },
    )
    .created
    else {
        panic!("clip")
    };
    edit(
        &service,
        Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_quarters(8).unwrap(),
            pitch: 60,
            channel: 0,
            velocity: 100,
        },
    );
    let Some(Created::Track(other)) = edit(
        &service,
        Command::AddTrack {
            name: "untouched".into(),
        },
    )
    .created
    else {
        panic!("track")
    };
    (service, track, other)
}
fn begin(service: &Service, track: TrackId, token: u64) -> GestureKey {
    let key = GestureKey {
        generation: 0,
        token,
    };
    let event = request(
        service,
        Request::DeviceGesture(GestureRequest::Begin {
            key,
            target: GestureTarget::TrackGain { track },
        }),
    );
    assert!(event.gesture_ack.unwrap().accepted, "{:?}", event.error);
    key
}
fn change(service: &Service, key: GestureKey, sequence: u64, gain: f32) -> Event {
    request(
        service,
        Request::DeviceGesture(GestureRequest::Change {
            key,
            sequence,
            value: ParameterValue::Float(gain),
        }),
    )
}
fn pcm(mut event: Event) -> Vec<[f32; 2]> {
    let mut playback = event.playback.take().expect("actual compiled playback");
    playback.apply(hex_engine::playback::PlaybackCommand::PlayFrom(0));
    let mut frames = vec![[0.0; 2]; 256];
    playback.render(&mut frames).unwrap();
    frames
}

#[test]
fn held_gain_changes_pcm_before_commit_and_commits_one_undo_step() {
    let (service, track, other) = fixture();
    let base = edit(&service, Command::Gain { track, gain: 0.5 });
    let revision = base.state.revision;
    let dirty = base.state.unsaved;
    let canonical = base.state.project.clone();
    let reference = pcm(base);
    assert!(reference.iter().flatten().any(|value| value.abs() > 1e-5));
    let key = begin(&service, track, 1);
    for (sequence, gain) in [0.0, 1.0, 2.0, 0.0, 0.75].into_iter().enumerate() {
        let event = change(&service, key, sequence as u64 + 1, gain);
        assert!(
            event.gesture_ack.as_ref().unwrap().accepted,
            "{:?}",
            event.error
        );
        assert_eq!(event.state.revision, revision);
        assert_eq!(event.state.unsaved, dirty);
        assert_eq!(event.state.project, canonical);
        assert_eq!(
            event
                .device_preview
                .as_ref()
                .unwrap()
                .state
                .project
                .track(track)
                .unwrap()
                .gain(),
            gain
        );
        assert_eq!(
            event
                .device_preview
                .as_ref()
                .unwrap()
                .state
                .project
                .track(other)
                .unwrap()
                .gain(),
            1.0
        );
        let frames = pcm(event);
        for (actual, original) in frames.iter().flatten().zip(reference.iter().flatten()) {
            assert!(
                (actual - original * (gain / 0.5)).abs() < 1e-5,
                "held preview must apply track gain exactly once"
            );
        }
    }
    let committed = request(
        &service,
        Request::DeviceGesture(GestureRequest::Commit {
            manual_edit: false,
            key,
            sequence: 6,
            value: ParameterValue::Float(0.75),
        }),
    );
    assert!(committed.gesture_ack.unwrap().accepted);
    assert_eq!(committed.state.revision, revision + 1);
    assert_eq!(committed.state.project.track(track).unwrap().gain(), 0.75);
    let undone = request(&service, Request::Undo);
    assert_eq!(undone.state.project, canonical);
    let redone = request(&service, Request::Redo);
    assert_eq!(redone.state.project.track(track).unwrap().gain(), 0.75);
}

#[test]
fn cancel_restores_latest_canonical_without_discarding_intervening_edit() {
    let (service, track, other) = fixture();
    let key = begin(&service, track, 1);
    assert!(change(&service, key, 1, 0.1).gesture_ack.unwrap().accepted);
    edit(&service, Command::Gain { track, gain: 1.25 });
    let renamed = edit(
        &service,
        Command::RenameTrack {
            track: other,
            name: "preserved".into(),
        },
    );
    let expected = renamed.state.project.clone();
    // Ordinary edits during a held gesture deliberately publish its rebased
    // overlay. Build the latest canonical oracle, rather than that overlay.
    let prepared = hex_engine::PreparedRevision::prepare(
        &expected,
        &Default::default(),
        Arc::new(hex_engine::SineTable::new()),
        None,
    )
    .unwrap();
    let mut oracle =
        hex_engine::playback::Playback::try_from_prepared_with_session(Arc::new(prepared)).unwrap();
    oracle.apply(hex_engine::playback::PlaybackCommand::PlayFrom(0));
    let mut expected_pcm = vec![[0.0; 2]; 256];
    oracle.render(&mut expected_pcm).unwrap();
    let canceled = request(
        &service,
        Request::DeviceGesture(GestureRequest::Cancel { key }),
    );
    assert!(canceled.gesture_ack.as_ref().unwrap().accepted);
    assert!(canceled.active_device_gesture.is_none());
    assert_eq!(canceled.state.project, expected);
    assert_eq!(pcm(canceled), expected_pcm);
    let undone = request(&service, Request::Undo);
    assert_eq!(
        undone.state.project.track(track).unwrap().gain(),
        1.25,
        "cancel adds no history entry"
    );
}

#[test]
fn invalid_final_gain_and_deleted_target_refuse_without_mutating_canonical_state() {
    let (service, track, _) = fixture();
    let key = begin(&service, track, 1);
    let accepted = change(&service, key, 1, 0.2);
    let canonical = accepted.state.project.clone();
    for (sequence, gain) in [f32::NAN, f32::INFINITY, -0.1, 2.1].into_iter().enumerate() {
        let event = change(&service, key, sequence as u64 + 2, gain);
        assert!(!event.gesture_ack.unwrap().accepted);
        assert_eq!(event.state.project, canonical);
        assert!(event.device_preview.is_none());
    }
    let rejected = request(
        &service,
        Request::DeviceGesture(GestureRequest::Commit {
            manual_edit: false,
            key,
            sequence: 7,
            value: ParameterValue::Float(2.1),
        }),
    );
    assert!(!rejected.gesture_ack.unwrap().accepted);
    assert_eq!(rejected.state.project, canonical);
    let key = begin(&service, track, 2);
    edit(&service, Command::RemoveTrack(track));
    let stale = request(
        &service,
        Request::DeviceGesture(GestureRequest::Commit {
            manual_edit: false,
            key,
            sequence: 1,
            value: ParameterValue::Float(0.2),
        }),
    );
    assert!(!stale.gesture_ack.unwrap().accepted);
    assert!(stale.state.project.track(track).is_none());
}

#[test]
fn no_op_commit_has_no_history_and_new_automation_retires_the_gesture() {
    let (service, track, _) = fixture();
    let key = begin(&service, track, 1);
    let changed = change(&service, key, 1, 0.25);
    let revision = changed.state.revision;
    let committed = request(
        &service,
        Request::DeviceGesture(GestureRequest::Commit {
            manual_edit: false,
            key,
            sequence: 2,
            value: ParameterValue::Float(1.0),
        }),
    );
    assert!(committed.gesture_ack.unwrap().accepted);
    assert_eq!(committed.state.revision, revision);
    let key = begin(&service, track, 2);
    change(&service, key, 1, 0.25);
    let automated = edit(
        &service,
        Command::ReplaceTrackGainEnvelope {
            track,
            points: vec![hex_project::GainPoint {
                incoming_gain: None,
                beat: BeatTime::ZERO,
                gain: 0.8,
                outgoing: hex_project::GainInterpolation::Step,
            }],
        },
    );
    assert!(automated.active_device_gesture.is_none());
    let canonical = automated.state.project.clone();
    let stale = request(
        &service,
        Request::DeviceGesture(GestureRequest::Commit {
            manual_edit: false,
            key,
            sequence: 2,
            value: ParameterValue::Float(0.25),
        }),
    );
    assert!(!stale.gesture_ack.unwrap().accepted);
    assert_eq!(stale.state.project, canonical);
    let refused = request(
        &service,
        Request::DeviceGesture(GestureRequest::Begin {
            key: GestureKey {
                generation: 0,
                token: 3,
            },
            target: GestureTarget::TrackGain { track },
        }),
    );
    assert!(!refused.gesture_ack.unwrap().accepted);
    assert_eq!(refused.state.project, canonical);
}

#[test]
fn audio_preview_preserves_balance_mute_and_postfader_return_and_save_finalizes_once() {
    struct Temp(std::path::PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let temp = Temp(std::env::temp_dir().join(format!(
            "hex-track-gain-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )));
    std::fs::create_dir(&temp.0).unwrap();
    let path = temp.0.join("tone.wav");
    let data = vec![1000_i16; 4096]
        .into_iter()
        .flat_map(i16::to_le_bytes)
        .collect::<Vec<_>>();
    let mut wav = b"RIFF".to_vec();
    wav.extend((36 + data.len() as u32).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16_u32.to_le_bytes());
    wav.extend(1_u16.to_le_bytes());
    wav.extend(1_u16.to_le_bytes());
    wav.extend(48_000_u32.to_le_bytes());
    wav.extend(96_000_u32.to_le_bytes());
    wav.extend(2_u16.to_le_bytes());
    wav.extend(16_u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend((data.len() as u32).to_le_bytes());
    wav.extend(data);
    std::fs::write(&path, wav).unwrap();
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
    wait(&service);
    let imported = request(&service, Request::Import(path));
    assert!(imported.error.is_none(), "{:?}", imported.error);
    let track = imported.state.project.tracks()[0].id();
    let Some(Created::ReturnTrack(return_track)) = edit(
        &service,
        Command::AddReturnTrack {
            name: "postfader".into(),
        },
    )
    .created
    else {
        panic!("return")
    };
    edit(
        &service,
        Command::SetReturnSend {
            track,
            return_track,
            gain: 0.7,
        },
    );
    edit(
        &service,
        Command::Balance {
            track,
            balance: 0.6,
        },
    );
    let reference = pcm(edit(&service, Command::Gain { track, gain: 0.5 }));
    assert!(
        reference
            .iter()
            .any(|frame| frame[0].abs() > 1e-5 && frame[0] != frame[1])
    );
    let key = begin(&service, track, 1);
    for (sequence, gain) in [0.0, 1.0, 2.0].into_iter().enumerate() {
        let event = change(&service, key, sequence as u64 + 1, gain);
        assert!(event.gesture_ack.as_ref().unwrap().accepted);
        for (actual, original) in pcm(event).iter().flatten().zip(reference.iter().flatten()) {
            assert!(
                (actual - original * (gain / 0.5)).abs() < 1e-5,
                "postfader return must not square or omit gain"
            );
        }
    }
    let saved_path = temp.0.join("gain.hex");
    let saved = request(&service, Request::SaveNew(saved_path.clone()));
    assert!(saved.error.is_none(), "{:?}", saved.error);
    assert!(saved.save_complete && !saved.state.unsaved);
    assert_eq!(saved.state.project.track(track).unwrap().gain(), 2.0);
    assert_eq!(
        hex_storage::load(&saved_path).unwrap(),
        *saved.state.project
    );
    let undone = request(&service, Request::Undo);
    assert_eq!(undone.state.project.track(track).unwrap().gain(), 0.5);
    edit(&service, Command::Mute { track, muted: true });
    let key = begin(&service, track, 2);
    let muted = change(&service, key, 1, 2.0);
    assert!(pcm(muted).iter().flatten().all(|v| *v == 0.0));
}

#[test]
fn track_gain_coalesces_backlog_and_reserves_final_absolute_value() {
    use hex_session::device_gesture::GestureClient;
    let (service, track, _) = fixture();
    let mut client = GestureClient::default();
    client
        .begin(
            0,
            GestureTarget::TrackGain { track },
            ParameterValue::Float(1.0),
        )
        .unwrap();
    let begin = client.next_request().unwrap();
    assert_eq!(
        client.next_request(),
        Some(begin),
        "a full submit queue must retain the exact request"
    );
    let begun = request(&service, Request::DeviceGesture(begin));
    client.mark_submitted();
    for i in 0..1_000 {
        client
            .change(ParameterValue::Float((i % 200) as f32 / 100.0))
            .unwrap();
    }
    assert_eq!(
        client.next_request(),
        None,
        "only one request may be in flight"
    );
    let begin_ack = begun.gesture_ack.unwrap();
    assert!(client.acknowledge(&begin_ack));
    let change = client.next_request().unwrap();
    let preview = request(&service, Request::DeviceGesture(change));
    client.mark_submitted();
    assert!(
        !client.acknowledge(&begin_ack),
        "late ack must not release another request"
    );
    for _ in 0..1_000 {
        client.change(ParameterValue::Float(0.1)).unwrap();
    }
    client.finish(ParameterValue::Float(0.75)).unwrap();
    assert!(client.change(ParameterValue::Float(0.5)).is_err());
    assert!(client.acknowledge(&preview.gesture_ack.unwrap()));
    let terminal = client.next_request().unwrap();
    assert!(matches!(
        terminal,
        GestureRequest::Commit {
            value: ParameterValue::Float(0.75),
            ..
        }
    ));
    assert_eq!(client.next_request(), Some(terminal));
    let committed = request(&service, Request::DeviceGesture(terminal));
    client.mark_submitted();
    assert!(client.acknowledge(&committed.gesture_ack.unwrap()));
    assert!(client.snapshot().is_none());
    assert_eq!(committed.state.project.track(track).unwrap().gain(), 0.75);
    let undone = request(&service, Request::Undo);
    assert_eq!(undone.state.project.track(track).unwrap().gain(), 1.0);
}

#[test]
fn track_gain_worker_profile_covers_populated_projects_and_retains_one_history_step() {
    // Control-worker roundtrip only, with 1ms event polling. This deliberately
    // does not claim physical device latency or a new performance pass threshold.
    for track_count in [1, 24] {
        let service = Service::new(48_000, 32, 0).unwrap();
        wait(&service);
        let mut target = None;
        for index in 0..track_count {
            let Some(Created::Track(track)) = edit(
                &service,
                Command::AddMidiTrack {
                    name: format!("profile {index}"),
                },
            )
            .created
            else {
                panic!("track")
            };
            target.get_or_insert(track);
            let Some(Created::Clip(clip)) = edit(
                &service,
                Command::PlaceMidiClip {
                    track,
                    start: BeatTime::ZERO,
                    length: BeatTime::from_quarters(8).unwrap(),
                },
            )
            .created
            else {
                panic!("clip")
            };
            for beat in 0..8 {
                edit(
                    &service,
                    Command::AddMidiNote {
                        clip,
                        start: BeatTime::from_quarters(beat).unwrap(),
                        duration: BeatTime::ONE,
                        pitch: 48 + index as u8,
                        channel: 0,
                        velocity: 80,
                    },
                );
            }
        }
        let track = target.unwrap();
        let key = begin(&service, track, 1);
        let mut timings = Vec::new();
        let mut revision = None;
        for sequence in 1..=64 {
            let gain = if sequence % 2 == 0 { 0.25 } else { 1.5 };
            let started = Instant::now();
            let event = change(&service, key, sequence, gain);
            timings.push(started.elapsed().as_micros());
            assert!(event.gesture_ack.as_ref().unwrap().accepted);
            assert_eq!(
                *revision.get_or_insert(event.state.revision),
                event.state.revision
            );
            assert_eq!(event.state.project.track(track).unwrap().gain(), 1.0);
            assert!(
                event.playback.is_some(),
                "accepted update must carry actual prepared playback"
            );
        }
        let committed = request(
            &service,
            Request::DeviceGesture(GestureRequest::Commit {
                manual_edit: false,
                key,
                sequence: 65,
                value: ParameterValue::Float(0.25),
            }),
        );
        assert!(committed.gesture_ack.unwrap().accepted);
        assert_eq!(committed.state.revision, revision.unwrap() + 1);
        assert_eq!(
            request(&service, Request::Undo)
                .state
                .project
                .track(track)
                .unwrap()
                .gain(),
            1.0
        );
        timings.sort_unstable();
        println!(
            "track_gain_worker_profile tracks={track_count} instruments={track_count} midi_notes={} accepted=64 p50_us={} p95_us={} max_us={} polling_ms=1 physical_audio=false",
            track_count * 8,
            timings[31],
            timings[60],
            timings[63]
        );
    }
}

#[test]
fn verified_track_gain_preview_preserves_canonical_state_and_other_mixer_limits() {
    use hex_session::{DocumentAudioMode, SourcePlanReservation};
    let service = Service::new(48_000, 16, 0).unwrap();
    wait(&service);
    let fresh = request(
        &service,
        Request::NewWithAudioMode {
            mode: DocumentAudioMode::VerifiedStreaming,
            reservation: Some(SourcePlanReservation {
                reader: hex_media::VerifiedWavReaderLimits::new(
                    hex_media::WavReaderLimits::new(64 * 1024 * 1024, 8_000_000, 32, 4096),
                    1024 * 1024,
                ),
                max_retained_sources: 1,
                max_path_bytes: 1024 * 1024,
                max_manifest_bytes: 1024 * 1024,
            }),
        },
    );
    assert!(fresh.error.is_none());
    assert!(fresh.project_replaced);
    let before = edit(
        &service,
        Command::AddTrack {
            name: "streamed volume".into(),
        },
    );
    let track = before.state.project.tracks()[0].id();
    let source = before.state.canonical_audio_source.as_ref().unwrap();
    assert_eq!(source.audio_mode(), DocumentAudioMode::VerifiedStreaming);
    let key = GestureKey {
        generation: source.context().project_generation,
        token: 1,
    };
    let began = request(
        &service,
        Request::DeviceGesture(GestureRequest::Begin {
            key,
            target: GestureTarget::TrackGain { track },
        }),
    );
    assert!(began.gesture_ack.as_ref().unwrap().accepted);
    let preview = request(
        &service,
        Request::DeviceGesture(GestureRequest::Change {
            key,
            sequence: 1,
            value: ParameterValue::Float(0.5),
        }),
    );
    assert!(preview.gesture_ack.as_ref().unwrap().accepted);
    assert_eq!(
        preview
            .device_preview
            .as_ref()
            .unwrap()
            .state
            .project
            .track(track)
            .unwrap()
            .gain(),
        0.5
    );
    let cancelled = request(
        &service,
        Request::DeviceGesture(GestureRequest::Cancel { key }),
    );
    assert!(cancelled.gesture_ack.as_ref().unwrap().accepted);
    assert!(cancelled.active_device_gesture.is_none());
    for event in [&began, &preview, &cancelled] {
        assert!(event.playback.is_none(), "no resident fallback");
        assert_eq!(event.state.revision, before.state.revision);
        assert_eq!(event.state.unsaved, before.state.unsaved);
        assert_eq!(event.state.can_undo, before.state.can_undo);
        assert_eq!(event.state.can_redo, before.state.can_redo);
        assert!(Arc::ptr_eq(&event.state.project, &before.state.project));
        assert!(Arc::ptr_eq(
            event.state.canonical_audio_source.as_ref().unwrap(),
            source
        ));
    }
    // The existing explicit +/- path remains a normal undoable canonical edit.
    let changed = edit(&service, Command::Gain { track, gain: 0.5 });
    assert_eq!(changed.state.project.track(track).unwrap().gain(), 0.5);
    assert!(changed.playback.is_none());
    let restored = request(&service, Request::Undo);
    assert!(restored.error.is_none());
    assert_eq!(restored.state.project, before.state.project);
    let bus_event = edit(
        &service,
        Command::AddReturnTrack {
            name: "streamed bus".into(),
        },
    );
    let Some(Created::ReturnTrack(bus)) = bus_event.created else {
        panic!("return")
    };
    for (token, target) in [
        GestureTarget::TrackBalance { track },
        GestureTarget::ReturnGain { return_track: bus },
    ]
    .into_iter()
    .enumerate()
    {
        let refused = request(
            &service,
            Request::DeviceGesture(GestureRequest::Begin {
                key: GestureKey {
                    generation: key.generation,
                    token: token as u64 + 2,
                },
                target,
            }),
        );
        assert!(!refused.gesture_ack.as_ref().unwrap().accepted);
        assert!(
            refused
                .gesture_ack
                .as_ref()
                .unwrap()
                .reason
                .as_deref()
                .unwrap()
                .contains("resident")
        );
        assert!(
            refused.active_device_gesture.is_none()
                && refused.device_preview.is_none()
                && refused.playback.is_none()
        );
        assert_eq!(refused.state.project, bus_event.state.project);
        assert_eq!(refused.state.revision, bus_event.state.revision);
    }
    let changed = edit(
        &service,
        Command::ReturnGain {
            return_track: bus,
            gain: 0.5,
        },
    );
    assert_eq!(changed.state.project.return_tracks()[0].gain(), 0.5);
    assert!(changed.playback.is_none());
    assert_eq!(
        request(&service, Request::Undo).state.project,
        bus_event.state.project
    );
}

#[test]
fn header_pan_previews_real_pcm_and_commits_or_cancels_one_canonical_edit() {
    let (service, track, other) = fixture();
    let base = edit(&service, Command::Gain { track, gain: 0.75 });
    let canonical = base.state.project.clone();
    let revision = base.state.revision;
    let reference = pcm(base);
    assert!(reference.iter().flatten().any(|x| x.abs() > 1e-5));
    let key = GestureKey {
        generation: 0,
        token: 1,
    };
    let begun = request(
        &service,
        Request::DeviceGesture(GestureRequest::Begin {
            key,
            target: GestureTarget::TrackBalance { track },
        }),
    );
    assert!(begun.gesture_ack.unwrap().accepted);
    for (sequence, balance) in [-1.0_f32, -0.25, 0.0, 0.75, 1.0].into_iter().enumerate() {
        let event = change(&service, key, sequence as u64 + 1, balance);
        assert!(
            event.gesture_ack.as_ref().unwrap().accepted,
            "{:?}",
            event.error
        );
        assert_eq!(event.state.project, canonical);
        assert_eq!(event.state.revision, revision);
        assert_eq!(
            event
                .device_preview
                .as_ref()
                .unwrap()
                .state
                .project
                .track(track)
                .unwrap()
                .balance(),
            balance
        );
        let frames = pcm(event);
        for (actual, original) in frames.iter().zip(&reference) {
            for (channel, factor) in [1.0 - balance.max(0.0), 1.0 + balance.min(0.0)]
                .into_iter()
                .enumerate()
            {
                assert!((actual[channel] - original[channel] * factor).abs() < 1e-6);
            }
        }
    }
    let committed = request(
        &service,
        Request::DeviceGesture(GestureRequest::Commit {
            manual_edit: false,
            key,
            sequence: 6,
            value: ParameterValue::Float(0.75),
        }),
    );
    assert!(committed.gesture_ack.unwrap().accepted);
    assert_eq!(committed.state.revision, revision + 1);
    let accepted = committed.state.project.clone();
    assert_eq!(accepted.track(other).unwrap().balance(), 0.0);
    assert_eq!(request(&service, Request::Undo).state.project, canonical);
    assert_eq!(request(&service, Request::Redo).state.project, accepted);
    let key = GestureKey {
        generation: 0,
        token: 2,
    };
    assert!(
        request(
            &service,
            Request::DeviceGesture(GestureRequest::Begin {
                key,
                target: GestureTarget::TrackBalance { track }
            })
        )
        .gesture_ack
        .unwrap()
        .accepted
    );
    assert!(change(&service, key, 1, -0.5).gesture_ack.unwrap().accepted);
    let canceled = request(
        &service,
        Request::DeviceGesture(GestureRequest::Cancel { key }),
    );
    assert!(canceled.gesture_ack.unwrap().accepted);
    assert_eq!(canceled.state.project, accepted);
    assert_eq!(
        request(&service, Request::Undo).state.project,
        canonical,
        "cancel creates no undo step"
    );
}

#[test]
fn header_return_gain_previews_real_bus_pcm_and_preserves_current_save_open_and_history() {
    let (service, track, _) = fixture();
    let Some(Created::ReturnTrack(bus)) = edit(
        &service,
        Command::AddReturnTrack {
            name: "header bus".into(),
        },
    )
    .created
    else {
        panic!("return")
    };
    edit(
        &service,
        Command::SetReturnSend {
            track,
            return_track: bus,
            gain: 1.0,
        },
    );
    let direct = pcm(edit(
        &service,
        Command::ReturnGain {
            return_track: bus,
            gain: 0.0,
        },
    ));
    assert!(direct.iter().flatten().any(|x| x.abs() > 1e-5));
    let base = edit(
        &service,
        Command::ReturnGain {
            return_track: bus,
            gain: 1.0,
        },
    );
    let canonical = base.state.project.clone();
    let revision = base.state.revision;
    let key = GestureKey {
        generation: 0,
        token: 1,
    };
    assert!(
        request(
            &service,
            Request::DeviceGesture(GestureRequest::Begin {
                key,
                target: GestureTarget::ReturnGain { return_track: bus }
            })
        )
        .gesture_ack
        .unwrap()
        .accepted
    );
    for (sequence, gain) in [0.0, 0.25, 1.0, 2.0].into_iter().enumerate() {
        let event = change(&service, key, sequence as u64 + 1, gain);
        assert!(
            event.gesture_ack.as_ref().unwrap().accepted,
            "{:?}",
            event.error
        );
        assert_eq!(event.state.project, canonical);
        assert_eq!(
            event
                .device_preview
                .as_ref()
                .unwrap()
                .state
                .project
                .return_tracks()[0]
                .gain(),
            gain
        );
        for (actual, original) in pcm(event).iter().flatten().zip(direct.iter().flatten()) {
            assert!(
                (actual - original * (1.0 + gain)).abs() < 1e-6,
                "direct plus independent return contribution"
            );
        }
    }
    let committed = request(
        &service,
        Request::DeviceGesture(GestureRequest::Commit {
            manual_edit: false,
            key,
            sequence: 5,
            value: ParameterValue::Float(0.25),
        }),
    );
    assert!(committed.gesture_ack.unwrap().accepted);
    assert_eq!(committed.state.revision, revision + 1);
    let accepted = committed.state.project.clone();
    assert_eq!(request(&service, Request::Undo).state.project, canonical);
    assert_eq!(request(&service, Request::Redo).state.project, accepted);
    let path =
        std::env::temp_dir().join(format!("hex439-return-{}-current.hex", std::process::id()));
    assert!(!path.exists());
    let saved = request(&service, Request::SaveNew(path.clone()));
    assert!(saved.error.is_none(), "{:?}", saved.error);
    assert!(!saved.state.unsaved);
    edit(
        &service,
        Command::ReturnGain {
            return_track: bus,
            gain: 1.5,
        },
    );
    let reopened = request(&service, Request::Open(path.clone()));
    assert!(reopened.error.is_none(), "{:?}", reopened.error);
    assert_eq!(reopened.state.project, accepted);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn header_mix_targets_refuse_nonfinite_out_of_range_and_removed_owners() {
    let (service, track, _) = fixture();
    let Some(Created::ReturnTrack(bus)) =
        edit(&service, Command::AddReturnTrack { name: "bus".into() }).created
    else {
        panic!("return")
    };
    for (index, target) in [
        GestureTarget::TrackBalance { track },
        GestureTarget::ReturnGain { return_track: bus },
    ]
    .into_iter()
    .enumerate()
    {
        let key = GestureKey {
            generation: 0,
            token: index as u64 + 1,
        };
        let begun = request(
            &service,
            Request::DeviceGesture(GestureRequest::Begin { key, target }),
        );
        assert!(begun.gesture_ack.unwrap().accepted);
        let canonical = begun.state.project.clone();
        for (seq, value) in [f32::NAN, f32::INFINITY, -2.0, 3.0].into_iter().enumerate() {
            let rejected = change(&service, key, seq as u64 + 1, value);
            assert!(!rejected.gesture_ack.unwrap().accepted);
            assert!(rejected.device_preview.is_none());
            assert_eq!(rejected.state.project, canonical);
        }
        let command = match target {
            GestureTarget::TrackBalance { track } => Command::RemoveTrack(track),
            GestureTarget::ReturnGain { return_track } => Command::RemoveReturnTrack(return_track),
            _ => unreachable!(),
        };
        let removed = edit(&service, command);
        let stale = request(
            &service,
            Request::DeviceGesture(GestureRequest::Commit {
                manual_edit: false,
                key,
                sequence: 5,
                value: ParameterValue::Float(0.5),
            }),
        );
        assert!(!stale.gesture_ack.unwrap().accepted);
        assert_eq!(stale.state.project, removed.state.project);
    }
}
