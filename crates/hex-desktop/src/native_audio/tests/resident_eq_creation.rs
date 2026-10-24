//! Held EQ creation through the real control worker, resource admission and
//! callback. The fixture substitutes output creation only; every render probes
//! zero total and signed-net callback allocations/frees.
use super::resident_preview::ResidentHarness;
use super::*;
use hex_engine::eq::{EqState, Node, NodeId, NodeKind};
use hex_project::{Command, Created};
use hex_session::device_gesture::{EqNodeCreationSnapshot, GestureKey, GestureRequest};
use std::sync::atomic::Ordering;

fn periodic_source(files: &Files, frames: u32) -> PathBuf {
    let path = files.wav(48_000, frames);
    let mut bytes = fs::read(&path).unwrap();
    for (frame, sample) in bytes[44..].chunks_exact_mut(2).enumerate() {
        let value = ((frame as f32 * std::f32::consts::TAU / 64.0).sin() * 4096.0) as i16;
        sample.copy_from_slice(&value.to_le_bytes());
    }
    fs::write(&path, bytes).unwrap();
    path
}
fn level(h: &ResidentHarness) -> f32 {
    for _ in 0..16 {
        h.render();
    }
    let pcm = h.render();
    (pcm.iter().flatten().map(|v| v * v).sum::<f32>() / 128.0).sqrt()
}

#[test]
fn held_eq_creation_reaches_track_return_and_main_pcm_before_commit_with_atomic_history() {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    for owner in 0..3 {
        for kind in [NodeKind::Bell, NodeKind::LowCut, NodeKind::Notch] {
            let files = Files::new();
            let mut h = ResidentHarness::new();
            assert!(h.next().error.is_none());
            let (mut applied, _) = h.applied(0);
            let imported = h.run(Request::Import(periodic_source(&files, 4096)));
            (applied, _) = h.applied(applied);
            let track = imported.state.project.tracks()[0].id();
            let transient = owner == 0 && kind != NodeKind::LowCut;
            if !transient {
                h.run(Request::Edit(Command::SetLoopRegion {
                    start: 0,
                    end: 4096,
                    enabled: true,
                }));
                (applied, _) = h.applied(applied);
            }
            let state = EqState::default().encode().unwrap();
            let command = match owner {
                0 => Command::InsertDevice {
                    track,
                    before: None,
                    state,
                },
                1 => {
                    let added = h.run(Request::Edit(Command::AddReturnTrack {
                        name: "eq return".into(),
                    }));
                    (applied, _) = h.applied(applied);
                    let Some(Created::ReturnTrack(return_track)) = added.created else {
                        panic!("return")
                    };
                    h.run(Request::Edit(Command::SetReturnSend {
                        track,
                        return_track,
                        gain: 1.0,
                    }));
                    (applied, _) = h.applied(applied);
                    Command::InsertReturnDevice {
                        return_track,
                        before: None,
                        state,
                    }
                }
                _ => Command::InsertMainDevice {
                    before: None,
                    state,
                },
            };
            let added = h.run(Request::Edit(command));
            (applied, _) = h.applied(applied);
            let Some(Created::Device(device)) = added.created else {
                panic!("eq")
            };
            let before = added.state.project.snapshot();
            if transient {
                // A real section-launch loop is absent from saved project state.
                let mut intent = h.seek_intent(1, added.state.revision, None, None);
                intent.destination_frame = 0;
                intent.section = Some(hex_session::arrangement_seek::SectionLaunch {
                    region: hex_project::LoopRegion::new(0, 4096, true).unwrap(),
                    quantization: hex_engine::LaunchQuantization::Immediate,
                });
                h.service.submit_resident_arrangement_seek(intent).unwrap();
                assert!(h.next().error.is_none());
                assert!(h.seek_result().result.is_ok());
                (applied, _) = h.applied(applied);
                assert!(added.state.project.loop_region().is_none());
            }
            h.audio
                .as_mut()
                .unwrap()
                .control
                .command(PlaybackCommand::Play)
                .unwrap();
            let dry = level(&h);
            assert!(dry > 0.01);
            let mut node = Node::new(NodeId::new(before.next_id).unwrap(), 750.0, 12.0, 0).unwrap();
            node.kind = kind;
            if kind == NodeKind::LowCut {
                node.frequency_hz = 2000.0;
            }
            let draft = EqNodeCreationSnapshot::new(
                node,
                added.state.project.device(device).unwrap().active_variant(),
                before.next_id,
            )
            .unwrap();
            let key = GestureKey {
                generation: 0,
                token: 1,
            };
            let begin = h.run(Request::DeviceGesture(GestureRequest::BeginEqCreation {
                key,
                device,
                snapshot: draft,
            }));
            assert!(begin.gesture_ack.unwrap().accepted);
            if owner == 0 && kind == NodeKind::Bell {
                h.refuse.store(true, Ordering::Release);
                let refused = h.run(Request::DeviceGesture(GestureRequest::ChangeEqCreation {
                    key,
                    sequence: 1,
                    snapshot: draft,
                }));
                assert!(!refused.gesture_ack.unwrap().accepted);
                assert_eq!(refused.state.project.snapshot(), before);
                assert!(refused.device_preview.is_none());
                assert!((level(&h) - dry).abs() < dry * 0.05);
                h.refuse.store(false, Ordering::Release);
            }
            let preview = h.run(Request::DeviceGesture(GestureRequest::ChangeEqCreation {
                key,
                sequence: 1,
                snapshot: draft,
            }));
            let (preview_audio, metadata) = h.applied(applied);
            assert_eq!(metadata.device_preview.unwrap().sequence, 1);
            assert_eq!(preview.state.project.snapshot(), before);
            assert_eq!(
                EqState::decode(
                    preview
                        .device_preview
                        .as_ref()
                        .unwrap()
                        .state
                        .project
                        .device(device)
                        .unwrap()
                        .state()
                )
                .unwrap()
                .nodes,
                vec![node]
            );
            let held = level(&h);
            if transient {
                assert_runtime_loop(&h);
            }
            if kind == NodeKind::Bell {
                assert!(held > dry * 1.5, "owner={owner} held={held} dry={dry}");
            } else {
                assert!(
                    held < dry * 0.8,
                    "owner={owner} kind={kind:?} held={held} dry={dry}"
                );
            }
            if owner == 0 && kind == NodeKind::Bell {
                h.refuse.store(true, Ordering::Release);
                let mut changed = draft;
                changed.node.gain_db = -12.0;
                h.service
                    .submit(Request::DeviceGesture(GestureRequest::CommitEqCreation {
                        key,
                        sequence: 2,
                        snapshot: changed,
                    }))
                    .unwrap();
                let refused = h.next();
                let ack = refused.gesture_ack.unwrap();
                assert!(!ack.accepted && ack.preview_retained);
                assert_eq!(refused.state.project.snapshot(), before);
                assert!(
                    (level(&h) - held).abs() < held * 0.1,
                    "refused final state cannot replace accepted preview"
                );
                h.refuse.store(false, Ordering::Release);
                let cancelled = h.run(Request::DeviceGesture(GestureRequest::Cancel { key }));
                (applied, _) = h.applied(preview_audio);
                assert_eq!(cancelled.state.project.snapshot(), before);
                assert!((level(&h) - dry).abs() < dry * 0.05);
                assert_runtime_loop(&h);
                // The cancelled creation consumed no history: undo removes the EQ.
                h.run(Request::Undo);
                h.applied(applied);
                continue;
            }
            let committed = h.run(Request::DeviceGesture(GestureRequest::CommitEqCreation {
                key,
                sequence: 2,
                snapshot: draft,
            }));
            (applied, _) = h.applied(preview_audio);
            if transient {
                assert_runtime_loop(&h);
            }
            assert_eq!(committed.state.project.next_identity(), before.next_id + 1);
            assert_eq!(
                EqState::decode(committed.state.project.device(device).unwrap().state())
                    .unwrap()
                    .nodes,
                vec![node]
            );
            let undone = h.run(Request::Undo);
            (applied, _) = h.applied(applied);
            let mut restored = undone.state.project.snapshot();
            restored.next_id = before.next_id;
            assert_eq!(restored, before);
            assert!((level(&h) - dry).abs() < dry * 0.05);
            if transient {
                assert_runtime_loop(&h);
            }
            let redone = h.run(Request::Redo);
            h.applied(applied);
            assert_eq!(redone.state.project, committed.state.project);
            if transient {
                assert_runtime_loop(&h);
            }
            assert!(h.publication_refusals.is_empty());
        }
    }
}

fn assert_runtime_loop(h: &ResidentHarness) {
    for _ in 0..70 {
        h.render();
    }
    let status = h.audio.as_ref().unwrap().status().unwrap();
    assert!(status.playing.load(Ordering::Acquire));
    assert!(status.position.load(Ordering::Acquire) < 4096);
}

#[test]
fn held_sampler_note_survives_eq_creation_commit_and_history_inside_a_launched_loop() {
    use hex_project::BeatTime;
    let _native = NATIVE_FIXTURE.lock().unwrap();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(8_000_000);
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let added = h.run(Request::Edit(Command::AddMidiTrack {
        name: "held sampler".into(),
    }));
    let Some(Created::Track(track)) = added.created else {
        panic!("track")
    };
    (applied, _) = h.applied(applied);
    h.run(Request::LoadFirstSampler {
        request_id: 1,
        project_generation: 0,
        revision: added.state.revision,
        track,
        empty_sampler: None,
        // Keep natural source exhaustion beyond the held-note/release checks.
        path: periodic_source(&files, 192_000),
        cancel: Arc::new(AtomicBool::new(false)),
    });
    (applied, _) = h.applied(applied);
    h.run(Request::Edit(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::ONE,
    }));
    (applied, _) = h.applied(applied);
    let added = h.run(Request::Edit(Command::InsertDevice {
        track,
        before: None,
        state: EqState::default().encode().unwrap(),
    }));
    let Some(Created::Device(device)) = added.created else {
        panic!("eq")
    };
    (applied, _) = h.applied(applied);
    let input = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
    h.run(Request::PrepareComputerKeyboard {
        project_generation: 0,
        revision: added.state.revision,
        request_id: 2,
        track,
        input: Arc::clone(&input),
    });
    (applied, _) = h.applied(applied);
    let mut intent = h.seek_intent(1, added.state.revision, None, None);
    intent.destination_frame = 0;
    intent.section = Some(hex_session::arrangement_seek::SectionLaunch {
        region: hex_project::LoopRegion::new(0, 4096, true).unwrap(),
        quantization: hex_engine::LaunchQuantization::Immediate,
    });
    h.service.submit_resident_arrangement_seek(intent).unwrap();
    assert!(h.next().error.is_none());
    assert!(h.seek_result().result.is_ok());
    (applied, _) = h.applied(applied);
    input.set_key(0, Some((60, 127, 1))).unwrap();
    let dry = level(&h);
    assert!(dry > 0.01);
    let before = added.state.project.snapshot();
    let draft = EqNodeCreationSnapshot::new(
        Node::new(NodeId::new(before.next_id).unwrap(), 750.0, 12.0, 0).unwrap(),
        added.state.project.device(device).unwrap().active_variant(),
        before.next_id,
    )
    .unwrap();
    let key = GestureKey {
        generation: 0,
        token: 1,
    };
    assert!(
        h.run(Request::DeviceGesture(GestureRequest::BeginEqCreation {
            key,
            device,
            snapshot: draft,
        }))
        .gesture_ack
        .unwrap()
        .accepted
    );
    let preview = h.run(Request::DeviceGesture(GestureRequest::ChangeEqCreation {
        key,
        sequence: 1,
        snapshot: draft,
    }));
    (applied, _) = h.applied(applied);
    assert_eq!(preview.state.project.snapshot(), before);
    let held = level(&h);
    assert!(held > dry * 1.5);
    assert_runtime_loop(&h);
    let committed = h.run(Request::DeviceGesture(GestureRequest::CommitEqCreation {
        key,
        sequence: 2,
        snapshot: draft,
    }));
    (applied, _) = h.applied(applied);
    assert!((level(&h) - held).abs() < held * 0.05);
    assert_runtime_loop(&h);
    let undone = h.run(Request::Undo);
    (applied, _) = h.applied(applied);
    let mut restored = undone.state.project.snapshot();
    restored.next_id = before.next_id;
    assert_eq!(restored, before);
    assert!((level(&h) - dry).abs() < dry * 0.05);
    assert_runtime_loop(&h);
    let redone = h.run(Request::Redo);
    h.applied(applied);
    assert_eq!(redone.state.project, committed.state.project);
    assert!((level(&h) - held).abs() < held * 0.05);
    assert_runtime_loop(&h);
    input.set_key(0, None).unwrap();
    for _ in 0..128 {
        h.render();
    }
    assert!(h.render().iter().flatten().all(|sample| sample.abs() < 1e-7));
    assert!(h.publication_refusals.is_empty());
}
