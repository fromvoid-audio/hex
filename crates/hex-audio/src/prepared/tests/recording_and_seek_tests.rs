#[cfg(feature = "allocation-probe")]
use super::*;

#[test]
#[cfg(feature = "allocation-probe")]
fn bound_timing_only_capture_tracks_loop_spans_and_output_confirmation() {
    use crate::sample_recording::{OutputClock, OutputClockIndex};
    use hex_project::{Command, Editor, Project};
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    editor
        .execute(Command::AddTrack {
            name: "input".into(),
        })
        .unwrap();
    editor
        .execute(Command::SetLoopRegion {
            start: 32,
            end: 160,
            enabled: true,
        })
        .unwrap();
    let revision = Arc::new(
        hex_engine::PreparedRevision::prepare(
            editor.project(),
            &std::collections::BTreeMap::new(),
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let playback = Playback::try_from_prepared_with_session(revision).unwrap();
    let (mut control, mut callback) = channel(48_000).unwrap();
    control
        .publish_unaccounted_pcm_obligation(
            playback,
            PublicationPolicy::InstallFreshRuntime,
            7,
            true,
            false,
            Some(hex_engine::AudioSourceContext {
                project_generation: 7,
                editor_revision: 3,
            }),
        )
        .unwrap();
    callback.begin_block();
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    let source = callback.recording_source().unwrap();
    let mut reader = control.sample_recording.start_bound(source).unwrap();
    assert!(reader.sources().is_empty());
    let mut index = OutputClockIndex::new(&reader);
    assert!(reader.ready());
    for block in 0..4 {
        let mut output = [[0.0; 2]; 64];
        let measured = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
            callback.set_sample_recording_output_clock(Some(OutputClock {
                playback: cpal::StreamInstant::new(10, 0),
                offset_frames: block * 64,
                graph_latency_frames: None,
                origin: crate::sample_recording::OutputClockOrigin::Unknown,
            }));
            callback.finish_sample_recording(true);
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
        assert_eq!(index.poll(&mut reader), Ok(None));
    }
    let starts: Vec<_> = (0..index.len())
        .map(|n| {
            index
                .packet(n)
                .unwrap()
                .timing
                .iter()
                .map(|span| (span.take_start, span.take_end, span.timeline_start))
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(
        starts,
        vec![
            vec![(0, 64, 0)],
            vec![(0, 64, 64)],
            vec![(0, 32, 128), (32, 64, 32)],
            vec![(0, 64, 64)]
        ]
    );
    control.sample_recording.stop();
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 1]).unwrap();
    assert_eq!(
        index.poll(&mut reader),
        Ok(Some(crate::sample_recording::End::Stopped))
    );
}

#[test]
#[cfg(feature = "allocation-probe")]
fn recorded_audible_clock_accounts_for_actual_main_limiter_delay() {
    use crate::input_capture::{
        InputConfig, InputTake, InputTakeEnd, InputTiming, SharedClock, align_audible_take,
    };
    use crate::sample_recording::{End, OutputClock, OutputClockIndex};
    use hex_engine::AudioSourceContext;
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    use hex_project::{Command, Created, Editor, Project};
    use std::collections::BTreeMap;

    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { track, asset, clip } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "transient".into(),
            path: "memory/transient.wav".into(),
            source_sample_rate: 48_000,
            source_frames: 4_096,
            playback_frames: 4_096,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("import")
    };
    editor
        .execute(Command::MoveClip {
            clip,
            track,
            start: 4_800,
        })
        .unwrap();
    let definition = &hex_devices::limiter::DEFINITION;
    let state = definition
        .patch_parameter(
            &definition.default_state().unwrap(),
            hex_devices::limiter::parameter::LOOKAHEAD,
            ParameterValue::Choice(2),
        )
        .unwrap();
    editor
        .execute(Command::InsertMainDevice {
            before: None,
            state,
        })
        .unwrap();
    let mut pcm = vec![[0.0; 2]; 4_096];
    pcm[0] = [0.25; 2];
    let revision = Arc::new(
        hex_engine::PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::from([(asset, hex_engine::Audio::new(48_000, pcm).unwrap())]),
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    assert_eq!(revision.arrangement().main_latency_frames(), 288);

    for block_size in [63, 256] {
        let playback = Playback::try_from_prepared_with_session(Arc::clone(&revision)).unwrap();
        let (mut control, mut callback) = channel(48_000).unwrap();
        control
            .publish_unaccounted_pcm_obligation(
                playback,
                PublicationPolicy::InstallFreshRuntime,
                7,
                true,
                false,
                Some(AudioSourceContext {
                    project_generation: 7,
                    editor_revision: 3,
                }),
            )
            .unwrap();
        callback.begin_block();
        control.command(PlaybackCommand::Play).unwrap();
        callback.begin_block();
        let source = callback.recording_source().unwrap();
        let mut reader = control.sample_recording.start_bound(source).unwrap();
        assert!(
            reader.sources().is_empty(),
            "source-bound capture retains timing only"
        );
        let mut index = OutputClockIndex::new(&reader);
        assert!(reader.ready());
        let origin = cpal::StreamInstant::new(10, 0);
        let mut output = vec![[0.0; 2]; 6_000];
        for (number, block) in output.chunks_mut(block_size).enumerate() {
            let measured = allocation_counter::measure(|| {
                callback.begin_block();
                callback.render(block).unwrap();
                callback.set_sample_recording_output_clock(Some(OutputClock {
                    playback: origin,
                    offset_frames: (number * block_size) as u32,
                    graph_latency_frames: None,
                    origin: crate::sample_recording::OutputClockOrigin::Unknown,
                }));
                callback.finish_sample_recording(true);
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
            assert_eq!(index.poll(&mut reader), Ok(None));
        }
        assert!(output[5_088][0] > 0.01);
        control.sample_recording.stop();
        callback.begin_block();
        callback.render(&mut [[0.0; 2]; 1]).unwrap();
        assert_eq!(index.poll(&mut reader), Ok(Some(End::Stopped)));
        let take = InputTake {
            path: std::path::PathBuf::new(),
            input_device: format!("{}:test-input", cpal::default_host().id())
                .parse()
                .unwrap(),
            input_config: InputConfig {
                sample_rate: 48_000,
                channels: 1,
            },
            sample_rate: 48_000,
            frames: 16,
            preroll_frames: 0,
            timing: vec![InputTiming {
                take_start: 0,
                frames: 16,
                captured_at: origin,
                offset_frames: 5_088,
            }],
            end: InputTakeEnd::Stopped,
            open_error: None,
        };
        assert_eq!(
            align_audible_take(SharedClock, &take, &index)
                .unwrap()
                .timeline_start,
            4_800
        );
        assert_eq!(
            index.packet(0).unwrap().clock.graph_latency_frames,
            Some(288)
        );
        control.sample_recording.reclaim();
        let mut seek_take = control.sample_recording.start_bound(source).unwrap();
        assert!(seek_take.ready());
        callback.begin_block();
        callback.render(&mut [[0.0; 2]; 1]).unwrap();
        callback.finish_sample_recording(true);
        let position = control.status.position.load(Acquire);
        control.command(PlaybackCommand::Seek(position)).unwrap();
        let measured = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut [[0.0; 2]; 1]).unwrap();
            callback.finish_sample_recording(true);
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
        assert_eq!(seek_take.drain(|_, _, _| {}), Some(End::TransportChanged));
    }
}

#[test]
#[cfg(feature = "allocation-probe")]
fn bound_output_recording_rejects_replaced_source() {
    use crate::native_callback_owner::{NativeRetirement, reserve_native_worker};
    use crate::sample_recording::{End, OutputClock, OutputClockIndex};
    use hex_engine::AudioSourceContext;

    let _serial = super::candidate_tests::serial();
    let (mut control, mut callback) = channel(48_000).unwrap();
    let context = AudioSourceContext {
        project_generation: 0,
        editor_revision: 4,
    };
    let (graph, _, _, _) = session_audio_graph();
    control
        .publish_unaccounted_pcm_obligation(
            graph,
            PublicationPolicy::InstallFreshRuntime,
            0,
            true,
            false,
            Some(context),
        )
        .unwrap();
    callback.begin_block();
    let root = control.native_candidate_root();
    let (mut data, errors, terminals, mut client, mut publication) =
        reserve_native_worker(control, callback, None);
    let mut retirement = NativeRetirement::new(terminals, root);
    client.activate();
    client.bind_resident_transport(0);
    let source = client.status().unwrap().recording_source().unwrap();
    let wrong = AppliedAudioSource {
        audio_revision: source.audio_revision + 1,
        ..source
    };
    assert!(client.start_bound_output_recording(wrong).is_err());
    let mut take = client.start_bound_output_recording(source).unwrap();
    let mut index = OutputClockIndex::new(&take);
    assert_eq!(index.source_binding(), Some(source));
    assert!(take.ready());
    let mut output = [[0.0; 2]; 8];
    let measured = allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|c, _| {
            c.begin_block();
            c.render(&mut output).unwrap();
            c.set_sample_recording_output_clock(Some(OutputClock {
                playback: cpal::StreamInstant::new(10, 0),
                offset_frames: 0,
                graph_latency_frames: Some(0),
                origin: crate::sample_recording::OutputClockOrigin::Unknown,
            }));
            c.finish_sample_recording(true);
        });
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
    assert_eq!(index.poll(&mut take), Ok(None));
    assert_eq!(index.len(), 1);

    // Hold a rendered packet until after the next source has been applied.
    data.owner_mut().unwrap().run(|c, _| {
        c.begin_block();
        c.render(&mut output).unwrap();
        c.set_sample_recording_output_clock(Some(OutputClock {
            playback: cpal::StreamInstant::new(11, 0),
            offset_frames: 0,
            graph_latency_frames: Some(0),
            origin: crate::sample_recording::OutputClockOrigin::Unknown,
        }));
    });
    let control = publication.control().unwrap();
    let (graph, _, _, _) = session_audio_graph();
    control
        .publish_unaccounted_pcm_obligation(
            graph,
            PublicationPolicy::InheritRuntime,
            0,
            true,
            false,
            Some(context),
        )
        .unwrap();
    let measured = allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|c, _| {
            c.begin_block();
            c.finish_sample_recording(true);
        });
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
    assert_eq!(index.poll(&mut take), Ok(Some(End::SourceChanged)));
    assert_eq!(index.len(), 1);
    assert!(client.start_bound_output_recording(source).is_err());
    client.reclaim_sample_recording();
    let current = client.status().unwrap().recording_source().unwrap();
    let mut next = client.start_bound_output_recording(current).unwrap();
    assert!(next.ready());
    let control = publication.control().unwrap();
    let (graph, _, _, _) = session_audio_graph();
    control
        .publish_unaccounted_pcm_obligation(
            graph,
            PublicationPolicy::InheritRuntime,
            0,
            true,
            false,
            Some(context),
        )
        .unwrap();
    let measured = allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|c, _| {
            c.begin_block();
            c.render(&mut output).unwrap();
            c.finish_sample_recording(true);
        });
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
    assert_eq!(
        next.drain(|_, _, _| panic!("changed source published a packet")),
        Some(End::SourceChanged)
    );
    client.close();
    publication.close();
    drop(data);
    drop(errors);
    assert!(
        crate::lifecycle::test_dispose_backend(
            &mut Some(()),
            &mut false,
            &mut false,
            &mut retirement,
            |_| Ok(())
        )
        .unwrap()
    );
    assert!(retirement.advance().unwrap());
}

#[test]
#[cfg(feature = "allocation-probe")]
fn source_bound_seek_preserves_playback_and_rejects_replaced_source() {
    use crate::native_callback_owner::{NativeRetirement, reserve_native_worker};
    use hex_engine::AudioSourceContext;

    let _serial = super::candidate_tests::serial();
    let (mut control, mut callback) = channel(48_000).unwrap();
    let mut reader = wire_action_outcomes(&mut control, &mut callback);
    let context = AudioSourceContext {
        project_generation: 0,
        editor_revision: 4,
    };
    let (graph, _, _, _) = session_audio_graph();
    control
        .publish_unaccounted_pcm_obligation(
            graph,
            PublicationPolicy::InstallFreshRuntime,
            0,
            true,
            false,
            Some(context),
        )
        .unwrap();
    callback.begin_block();
    let root = control.native_candidate_root();
    let (mut data, errors, terminals, mut client, mut publication) =
        reserve_native_worker(control, callback, None);
    let mut retirement = NativeRetirement::new(terminals, root);
    client.activate();
    client.bind_resident_transport(0);
    let source = client.status().unwrap().recording_source().unwrap();
    let wrong_output = AppliedAudioSource {
        output_identity: source.output_identity + 1,
        ..source
    };
    assert!(
        client
            .seek_from_source_with_ticket(1, wrong_output)
            .is_err()
    );
    let wrong_context = AppliedAudioSource {
        context: AudioSourceContext {
            project_generation: 1,
            ..context
        },
        ..source
    };
    assert!(
        client
            .seek_from_source_with_ticket(1, wrong_context)
            .is_err()
    );

    let ticket = client.seek_from_source_with_ticket(17, source).unwrap();
    let measured =
        allocation_counter::measure(|| data.owner_mut().unwrap().run(|c, _| c.begin_block()));
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(
        reader.try_take(),
        Some(ActionOutcomeReceipt {
            ticket,
            outcome: ActionOutcome::Applied
        })
    );
    assert!(!client.status().unwrap().playing.load(Acquire));
    assert_eq!(client.status().unwrap().position.load(Acquire), 17);

    let start = client.play_from_source_with_ticket(17, source).unwrap();
    data.owner_mut().unwrap().run(|c, _| c.begin_block());
    assert_eq!(
        reader.try_take(),
        Some(ActionOutcomeReceipt {
            ticket: start,
            outcome: ActionOutcome::Applied
        })
    );
    let ticket = client.seek_from_source_with_ticket(29, source).unwrap();
    let mut output = [[0.0; 2]; 129];
    // Running transport seeks complete after the 128-frame declick fade.
    let measured = allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|c, _| {
            c.begin_block();
            c.render(&mut output).unwrap();
        })
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
    assert_eq!(
        reader.try_take(),
        Some(ActionOutcomeReceipt {
            ticket,
            outcome: ActionOutcome::Applied
        })
    );
    assert!(client.status().unwrap().playing.load(Acquire));
    assert_eq!(client.status().unwrap().position.load(Acquire), 30);
    client.stop();
    data.owner_mut().unwrap().run(|c, _| c.begin_block());

    let control = publication.control().unwrap();
    let fence = control.capture_fresh_runtime_fence().unwrap();
    let (graph, _, _, _) = session_audio_graph();
    control
        .publish_unaccounted_pcm_obligation(
            graph,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
            0,
            true,
            false,
            Some(context),
        )
        .unwrap();
    let barrier = Arc::new(Barrier::new(2));
    data.owner_mut()
        .unwrap()
        .run(|c, _| c.test_after_fresh_fence_validation = Some(Arc::clone(&barrier)));
    let ticket = std::thread::scope(|scope| {
        let callback = scope.spawn(|| {
            allocation_counter::measure(|| data.owner_mut().unwrap().run(|c, _| c.begin_block()))
        });
        barrier.wait();
        let ticket = client.seek_from_source_with_ticket(47, source).unwrap();
        barrier.wait();
        let measured = callback.join().unwrap();
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        ticket
    });
    assert_eq!(
        reader.try_take(),
        Some(ActionOutcomeReceipt {
            ticket,
            outcome: ActionOutcome::Rejected(PlaybackActionRejection::AudioSourceChanged)
        })
    );
    assert!(!client.status().unwrap().playing.load(Acquire));
    assert_ne!(client.status().unwrap().position.load(Acquire), 47);
    assert!(client.seek_from_source_with_ticket(47, source).is_err());
    let current = client.status().unwrap().recording_source().unwrap();
    for _ in 0..COMMANDS {
        client.seek_from_source_with_ticket(2, current).unwrap();
    }
    assert!(client.seek_from_source_with_ticket(2, current).is_err());
    client.stop();
    data.owner_mut().unwrap().run(|c, _| c.begin_block());
    assert!(!client.status().unwrap().playing.load(Acquire));
    client.close();
    publication.close();
    drop(data);
    drop(errors);
    assert!(
        crate::lifecycle::test_dispose_backend(
            &mut Some(()),
            &mut false,
            &mut false,
            &mut retirement,
            |_| Ok(())
        )
        .unwrap()
    );
    assert!(retirement.advance().unwrap());
}

#[test]
#[cfg(feature = "allocation-probe")]
fn source_bound_start_rejects_same_project_replacement_and_preserves_stop() {
    for note_chase in [hex_engine::NoteChase::On, hex_engine::NoteChase::Off] {
        source_bound_start_fences(note_chase);
    }
}

#[test]
#[cfg(feature = "allocation-probe")]
fn document_bound_client_rejects_commands_racing_a_successor_graph() {
    use crate::native_callback_owner::{NativeRetirement, reserve_native_worker};
    use hex_engine::{AudioSourceContext, LaunchQuantization, SessionPlaybackCommand};

    let _serial = super::candidate_tests::serial();
    let (mut control, mut callback) = channel(48_000).unwrap();
    let mut reader = wire_action_outcomes(&mut control, &mut callback);
    let old = AudioSourceContext {
        project_generation: 0,
        editor_revision: 4,
    };
    let (old_graph, _, clip, _) = session_audio_graph();
    control
        .publish_unaccounted_pcm_obligation(
            old_graph,
            PublicationPolicy::InstallFreshRuntime,
            old.project_generation,
            true,
            false,
            Some(old),
        )
        .unwrap();
    callback.begin_block();
    let root = control.native_candidate_root();
    let (mut data, errors, terminals, mut client, mut publication) =
        reserve_native_worker(control, callback, None);
    let mut retirement = NativeRetirement::new(terminals, root);
    client.activate();
    client.bind_resident_transport(old.project_generation);

    let control = publication.control().unwrap();
    let fence = control.capture_fresh_runtime_fence().unwrap();
    let new = AudioSourceContext {
        project_generation: 2,
        editor_revision: 1,
    };
    let (new_graph, _, new_clip, _) = session_audio_graph();
    assert_eq!(clip, new_clip);
    control
        .publish_unaccounted_pcm_obligation(
            new_graph,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
            2,
            true,
            false,
            Some(new),
        )
        .unwrap();
    let barrier = Arc::new(Barrier::new(2));
    data.owner_mut().unwrap().run(|callback, _| {
        callback.test_after_fresh_fence_validation = Some(Arc::clone(&barrier));
    });
    let actions = [
        PlaybackAction::Transport(PlaybackCommand::PlayFrom(47)),
        PlaybackAction::Transport(PlaybackCommand::Seek(61)),
        PlaybackAction::LaunchAndPlay(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        }),
    ];
    let tickets = std::thread::scope(|scope| {
        let callback = scope.spawn(|| {
            allocation_counter::measure(|| {
                data.owner_mut()
                    .unwrap()
                    .run(|callback, _| callback.begin_block());
            })
        });
        barrier.wait();
        let tickets = actions.map(|action| client.action_with_ticket(action));
        barrier.wait();
        let count = callback.join().unwrap();
        assert_eq!(
            (
                count.count_total,
                count.bytes_total,
                count.count_current,
                count.bytes_current
            ),
            (0, 0, 0, 0)
        );
        tickets.map(Result::unwrap)
    });
    for ticket in tickets {
        assert_eq!(
            reader.try_take(),
            Some(ActionOutcomeReceipt {
                ticket,
                outcome: ActionOutcome::Rejected(PlaybackActionRejection::ProjectChanged),
            })
        );
    }
    let status = client.status().unwrap();
    assert_eq!(
        status.rejected_action(),
        Some((tickets[2], PlaybackActionRejection::ProjectChanged))
    );
    assert_eq!(status.recording_source().unwrap().context, new);
    assert_eq!(status.position.load(Acquire), 0);
    assert!(!status.playing.load(Acquire));
    assert!(client.command_with_ticket(PlaybackCommand::Play).is_err());

    client.bind_resident_transport(new.project_generation);
    let control = publication.control().unwrap();
    assert_eq!(control.reclaim(), 1);
    let edited = AudioSourceContext {
        editor_revision: 2,
        ..new
    };
    let (edited_graph, _, _, _) = session_audio_graph();
    control
        .publish_unaccounted_pcm_obligation(
            edited_graph,
            PublicationPolicy::InstallFreshRuntime,
            2,
            true,
            false,
            Some(edited),
        )
        .unwrap();
    let next = client
        .command_with_ticket(PlaybackCommand::Seek(29))
        .unwrap();
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    assert_eq!(
        reader.try_take(),
        Some(ActionOutcomeReceipt {
            ticket: next,
            outcome: ActionOutcome::Applied
        })
    );
    assert_eq!(
        client.status().unwrap().recording_source().unwrap().context,
        edited
    );
    assert_eq!(client.status().unwrap().position.load(Acquire), 29);
    for _ in 0..COMMANDS {
        client.command_with_ticket(PlaybackCommand::Play).unwrap();
    }
    assert!(client.command_with_ticket(PlaybackCommand::Play).is_err());
    client.stop();
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    assert!(!client.status().unwrap().playing.load(Acquire));
    assert_eq!(client.status().unwrap().position.load(Acquire), 0);

    client.close();
    publication.close();
    drop(data);
    drop(errors);
    assert!(
        crate::lifecycle::test_dispose_backend(
            &mut Some(()),
            &mut false,
            &mut false,
            &mut retirement,
            |_| Ok(())
        )
        .unwrap()
    );
    assert!(retirement.advance().unwrap());
}
