use super::*;

fn action_revision(looped: bool) -> Arc<PreparedRevision> {
    action_revision_with_loop(looped.then_some((0, 64)), 48_000)
}

fn action_revision_with_loop(loop_bounds: Option<(u64, u64)>, rate: u32) -> Arc<PreparedRevision> {
    let mut snapshot = audio_project(5, 18, 4).snapshot();
    snapshot.sample_rate = rate;
    let SessionCellContentSnapshot::Audio(clip) = &mut snapshot.session.cells[0].content else {
        panic!("audio fixture")
    };
    clip.source_start = 4094;
    clip.source_end = if rate == 48_000 { 4108 } else { 4700 };
    snapshot.assets[0].source_sample_rate = 24_000;
    snapshot.assets[0].source_frames = 4096;
    snapshot.assets[0].playback_frames =
        hex_project::converted_frame_count(4096, 24_000, rate).unwrap();
    snapshot.assets[0].conversion_version = hex_project::SINC_CONVERSION_V1;
    snapshot.assets[1].source_sample_rate = rate;
    snapshot.tracks[0].content = TrackContentSnapshot::Audio(vec![ClipSnapshot {
        active: true,
        name: "clip".into(),
        flavor: hex_project::ClipFlavor::default(),
        edit: None,
        gain: hex_project::ClipGain::UNITY,
        id: snapshot.next_id,
        asset: 4,
        start: 0,
        duration: 8192,
        source_offset: 0,
        source_length: 8192,
        looping: false,
        fade: ClipFade::neutral(8192).unwrap(),
        source_phase: 0,
    }]);
    snapshot.next_id += 1;
    if let Some((start, length)) = loop_bounds {
        snapshot.loop_region = Some(LoopRegion::new(start, start + length, true).unwrap());
    }
    let project =
        with_utility_filter_state(&Project::from_snapshot(snapshot).unwrap(), true, false);
    let mut media = BTreeMap::new();
    for asset in project.assets() {
        let source = if asset.id().value() == 4 {
            PreparedAudioSource::Streamed(Arc::new(
                StreamedSourceMetadata::new(
                    StreamSourceKey::new(23, asset.id()),
                    asset.source_sample_rate(),
                    project.sample_rate(),
                    asset.source_frames(),
                    asset.playback_frames(),
                    asset.conversion_version(),
                    [0x48; 32],
                )
                .unwrap(),
            ))
        } else {
            PreparedAudioSource::Resident(crate::Audio::new(rate, vec![[0.0; 2]; 32]).unwrap())
        };
        media.insert(asset.id(), source);
    }
    Arc::new(
        PreparedRevision::prepare_with_prepared_audio_sources(
            &project,
            &media,
            23,
            Arc::new(hex_devices::SineTable::new()),
            None,
        )
        .unwrap(),
    )
}

#[test]
fn projected_windows_match_rendered_reads_across_loop_and_due_release() {
    struct ReadPages {
        frames: Vec<Frame>,
        reads: Vec<(StreamSourceKey, u64)>,
    }
    impl MediaPages for ReadPages {
        fn page(&mut self, key: StreamSourceKey, index: u64) -> Option<BorrowedMediaPage<'_>> {
            self.reads.push((key, index));
            (index < 2).then_some(BorrowedMediaPage {
                page_index: index,
                frames: &self.frames,
            })
        }
        fn request_page(&mut self, _key: StreamSourceKey, _index: u64) {}
    }

    for (rate, loop_length) in [
        (48_000, Some(1)),
        (48_000, Some(64)),
        (48_000, None),
        (192_000, Some(1)),
        (192_000, Some(64)),
        (192_000, None),
    ] {
        let start = u64::from(rate) * 2 - 8;
        let mut actual = Playback::from_prepared(action_revision_with_loop(
            loop_length.map(|length| (start, length)),
            rate,
        ));
        actual
            .try_apply_action(PlaybackAction::Transport(PlaybackCommand::PlayFrom(start)))
            .unwrap();
        let clip = actual.prepared_revision().unwrap().session().audio_clips()[0].id;
        actual
            .try_apply_action(PlaybackAction::Session(
                SessionPlaybackCommand::LaunchClip {
                    clip,
                    quantization: LaunchQuantization::Immediate,
                },
            ))
            .unwrap();
        actual
            .render_with_pages(&mut [[0.0; 2]; 1], &mut pages())
            .unwrap();
        let action = PlaybackAction::Session(SessionPlaybackCommand::BackToArrangement {
            quantization: LaunchQuantization::FourQuarter,
        });
        let prepared = actual.prepare_stream_action(action).unwrap();
        let due = prepared.scheduled_due_frame().unwrap();
        actual.commit_stream_action(prepared).unwrap();

        // Test-only model: carry the copied route and its release through
        // each future window; this is not an admitted realtime transaction.
        let fade = (rate as usize * 128 + 24_000) / 48_000;
        let total = usize::try_from(due - actual.performance_frame()).unwrap() + 300.max(fade + 8);
        let projected = {
            let session = actual.session_runtime_mut_for_stream_action().unwrap();
            let mut routes = session.routes.clone();
            let mut performance = session.performance_frame;
            let mut remaining = total;
            let mut windows = Vec::new();
            while remaining > 0 {
                for route in &mut routes {
                    if let Some(pending) = route.pending.filter(|p| p.frame <= performance) {
                        let previous = route.owner;
                        route.owner = session
                            .owner_for_target(pending.target, pending.beat, pending.frame)
                            .unwrap();
                        route.pending = None;
                        super::super::audio_release::begin(
                            route,
                            previous,
                            performance,
                            session.audio_release_fade(),
                        )
                        .unwrap();
                    }
                }
                let (arrangement, pass) = session.project_stream_arrangement(performance).unwrap();
                let mut extents = Vec::new();
                let window = session
                    .visit_stream_chunk_sources_at(
                        &routes,
                        performance,
                        arrangement,
                        session.route_state_generation,
                        pass,
                        true,
                        remaining,
                        &mut |extent| {
                            extents.push(extent);
                            Ok(())
                        },
                    )
                    .unwrap();
                assert!(window.frames > 0);
                remaining -= window.frames;
                performance += window.frames as u64;
                windows.push((window, extents));
            }
            windows
        };
        assert!(
            projected
                .iter()
                .any(|(window, _)| window.performance_frame == due)
        );
        if loop_length == Some(1) {
            assert!(projected.len() >= 300.max(fade));
        }
        if loop_length.is_none() {
            assert!(projected.iter().any(|(window, _)| window.frames == 256));
        }
        let initial_generation = projected[0].0.route_generation;
        let mut pages = ReadPages {
            frames: vec![[0.25; 2]; 4096],
            reads: Vec::new(),
        };
        let mut output = [[0.0; 2]; 256];
        let mut reads = 0;
        let mut outgoing_reads = 0;
        let mut generation_changed = false;
        for (predicted, extents) in projected {
            let mut current = Vec::new();
            let window = actual
                .visit_next_stream_chunk(predicted.frames, None, &mut |extent| {
                    current.push(extent);
                    Ok(())
                })
                .unwrap();
            generation_changed |= window.route_generation != initial_generation;
            assert_eq!(
                (
                    window.performance_frame,
                    window.arrangement_frame,
                    window.frames
                ),
                (
                    predicted.performance_frame,
                    predicted.arrangement_frame,
                    predicted.frames
                )
            );
            assert_eq!(current, extents);
            pages.reads.clear();
            actual
                .render_with_pages(&mut output[..window.frames], &mut pages)
                .unwrap();
            reads += pages.reads.len();
            for (source, page) in &pages.reads {
                if window.performance_frame >= due && *page == 1 {
                    outgoing_reads += 1;
                }
                assert!(
                    extents
                        .iter()
                        .any(|e| { e.source == (*source).into() && e.page_index == *page })
                );
            }
        }
        assert!(
            reads > 0,
            "the oracle must observe actual streamed page reads"
        );
        assert!(
            outgoing_reads > 0,
            "the outgoing release must read its streamed page"
        );
        assert!(
            generation_changed,
            "route generation advances after activation"
        );
    }
}
struct Pages(Vec<Frame>);
impl MediaPages for Pages {
    fn page(&mut self, _key: StreamSourceKey, index: u64) -> Option<BorrowedMediaPage<'_>> {
        (index < 2).then_some(BorrowedMediaPage {
            page_index: index,
            frames: &self.0,
        })
    }
    fn request_page(&mut self, _key: StreamSourceKey, _index: u64) {}
}
fn pages() -> Pages {
    Pages(vec![[0.25; 2]; 4096])
}

#[test]
fn streamed_paired_observation_covers_each_phase_at_exact_loop_offsets() {
    for looped in [false, true] {
        let revision = action_revision(looped);
        let mut observed = Playback::from_prepared(Arc::clone(&revision));
        let mut oracle = Playback::from_prepared(revision);
        for runtime in [&mut observed, &mut oracle] {
            runtime
                .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Play))
                .unwrap();
        }
        let mut actual = [[0.0; 2]; 512];
        let mut expected = actual;
        let mut calls = Vec::new();
        let mut project_master = [[0.0; 2]; 512];
        let mut project_track = [[0.0; 2]; 512];
        let mut project_seen = [false; 512];
        {
            let mut project_observer = |signal, offset: usize, frames: &[Frame]| {
                let end = offset + frames.len();
                match signal {
                    crate::ProjectSignal::Master => {
                        assert!(project_seen[offset..end].iter().all(|v| !v));
                        project_seen[offset..end].fill(true);
                        project_master[offset..end].copy_from_slice(frames);
                    }
                    crate::ProjectSignal::Track(1) => {
                        for (to, from) in project_track[offset..end].iter_mut().zip(frames) {
                            to[0] += from[0];
                            to[1] += from[1];
                        }
                    }
                    _ => panic!("unexpected project owner"),
                }
            };
            let mut project_tap =
                crate::ProjectOutputTap::new(&[1], &mut project_observer).unwrap();
            observed
                .render_with_pages_and_signal_observers(
                    &mut actual,
                    &mut pages(),
                    Some(&mut |identity, phase, offset, frames: &[Frame]| {
                        calls.push((identity, phase, offset, frames.len()));
                    }),
                    Some(&mut project_tap),
                )
                .unwrap();
        }
        assert!(project_seen.iter().all(|v| *v));
        assert_eq!(
            project_master, actual,
            "streamed Master has exact global offsets"
        );
        assert_eq!(
            project_track, actual,
            "unity single track remains independent and complete across wraps"
        );
        oracle
            .render_with_pages(&mut expected, &mut pages())
            .unwrap();
        assert_eq!(actual, expected);
        assert!(!calls.is_empty());
        for pair in calls.chunks_exact(2) {
            assert_eq!(pair[0].0, pair[1].0);
            assert_eq!(pair[0].1, crate::NodeSignalPhase::Input);
            assert_eq!(pair[1].1, crate::NodeSignalPhase::Output);
            assert_eq!((pair[0].2, pair[0].3), (pair[1].2, pair[1].3));
        }
        assert_eq!(calls.len() % 2, 0);
        let identity = calls[0].0;
        let mut covered = [0_u8; 512];
        for (_, phase, offset, count) in calls.into_iter().filter(|(id, _, _, _)| *id == identity) {
            if phase == crate::NodeSignalPhase::Input {
                for sample in &mut covered[offset..offset + count] {
                    *sample += 1;
                }
            }
        }
        assert_eq!(covered, [1; 512]);
    }
}

#[test]
fn prepared_stream_seek_is_pure_preserves_real_filter_history_and_refuses_stale_runtime() {
    let revision = action_revision(false);
    let mut actual = Playback::from_prepared(Arc::clone(&revision));
    let mut oracle = Playback::from_prepared(revision);
    for runtime in [&mut actual, &mut oracle] {
        runtime
            .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Play))
            .unwrap();
        runtime
            .render_with_pages(&mut [[0.0; 2]; 8], &mut pages())
            .unwrap();
    }
    let before = (
        actual.position(),
        actual.performance_frame(),
        actual.session_route_status(0),
    );
    let token = actual
        .prepare_stream_action(PlaybackAction::Transport(PlaybackCommand::Seek(4094)))
        .unwrap();
    let mut extents = Vec::new();
    let projected = actual
        .visit_prepared_stream_action(&token, 8, &mut |row| {
            extents.push(row);
            Ok(())
        })
        .unwrap();
    assert_eq!(
        (
            projected.performance_frame,
            projected.arrangement_frame,
            projected.frames
        ),
        (4094, 4094, 8)
    );
    assert_eq!(
        extents
            .iter()
            .map(|e| (e.page_index, e.start, e.end))
            .collect::<Vec<_>>(),
        vec![(0, 4094, 4096), (1, 0, 6)]
    );
    assert!(
        actual
            .visit_prepared_stream_action(&token, 8, &mut |_| Err(crate::Invalid(
                "reserved extent capacity"
            )))
            .is_err()
    );
    assert!(
        oracle
            .visit_prepared_stream_action(&token, 8, &mut |_| Ok(()))
            .is_err()
    );
    assert_eq!(
        (
            actual.position(),
            actual.performance_frame(),
            actual.session_route_status(0)
        ),
        before
    );
    let mut a = [[0.0; 2]; 8];
    let mut b = a;
    actual.render_with_pages(&mut a, &mut pages()).unwrap();
    oracle.render_with_pages(&mut b, &mut pages()).unwrap();
    assert_eq!(a, b); // Real warmed Utility history was never reset by preview.
    assert!(actual.commit_stream_action(token).is_err());
    actual.render_with_pages(&mut a, &mut pages()).unwrap();
    oracle.render_with_pages(&mut b, &mut pages()).unwrap();
    assert_eq!(a, b); // Stale commit also refused before the reset frontier.
    let token = actual
        .prepare_stream_action(PlaybackAction::Transport(PlaybackCommand::Seek(4094)))
        .unwrap();
    actual.commit_stream_action(token).unwrap();
    oracle
        .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Seek(4094)))
        .unwrap();
    actual.render_with_pages(&mut a, &mut pages()).unwrap();
    oracle.render_with_pages(&mut b, &mut pages()).unwrap();
    assert_eq!(a, b);
    assert_eq!(actual.position(), 4102);
    let stale = actual
        .prepare_stream_action(PlaybackAction::Transport(PlaybackCommand::PlayFrom(4094)))
        .unwrap();
    actual
        .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Pause))
        .unwrap();
    assert!(actual.commit_stream_action(stale).is_err());
    let paused = actual
        .prepare_stream_action(PlaybackAction::Transport(PlaybackCommand::Seek(5000)))
        .unwrap();
    assert_eq!(
        actual
            .visit_prepared_stream_action(&paused, 8, &mut |_| panic!(
                "paused action needs no pages"
            ))
            .unwrap()
            .frames,
        0
    );
    actual.commit_stream_action(paused).unwrap();
    assert_eq!(actual.position(), 5000);
    assert!(!actual.playing());
    let play = actual
        .prepare_stream_action(PlaybackAction::Transport(PlaybackCommand::PlayFrom(4094)))
        .unwrap();
    assert_eq!(
        actual
            .visit_prepared_stream_action(&play, 8, &mut |_| Ok(()))
            .unwrap()
            .frames,
        8
    );
    actual.commit_stream_action(play).unwrap();
    oracle
        .try_apply_action(PlaybackAction::Transport(PlaybackCommand::PlayFrom(4094)))
        .unwrap();
    actual.render_with_pages(&mut a, &mut pages()).unwrap();
    oracle.render_with_pages(&mut b, &mut pages()).unwrap();
    assert_eq!(a, b);
}

#[test]
fn quantized_action_projects_original_due_across_real_loop_wrap_then_matches_activation() {
    let mut actual = Playback::from_prepared(action_revision(true));
    actual
        .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Play))
        .unwrap();
    actual
        .render_with_pages(&mut [[0.0; 2]; 8], &mut pages())
        .unwrap();
    let clip = actual.prepared_revision().unwrap().session().audio_clips()[0].id;
    let command = PlaybackAction::Session(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::FourQuarter,
    });
    let before = (
        actual.position(),
        actual.performance_frame(),
        actual.session_route_status(0),
    );
    let token = actual.prepare_stream_action(command).unwrap();
    let due = token.scheduled_due_frame().unwrap();
    assert_eq!(due, 96000);
    let mut projected = Vec::new();
    let window = actual
        .visit_prepared_stream_action(&token, 8, &mut |row| {
            projected.push(row);
            Ok(())
        })
        .unwrap();
    assert_eq!(
        (
            window.performance_frame,
            window.arrangement_frame,
            window.frames
        ),
        (due, 0, 8)
    );
    assert_eq!(
        projected
            .iter()
            .map(|e| (e.page_index, e.start, e.end))
            .collect::<Vec<_>>(),
        vec![(0, 4094, 4096), (1, 0, 6)]
    );
    assert_eq!(
        (
            actual.position(),
            actual.performance_frame(),
            actual.session_route_status(0)
        ),
        before
    );
    actual.commit_stream_action(token).unwrap();
    assert_eq!(
        actual
            .session_route_status(0)
            .unwrap()
            .pending
            .unwrap()
            .performance_frame,
        due
    );
    let mut early = Vec::new();
    actual
        .visit_pending_stream_chunk(due, 8, &mut |row| {
            early.push(row);
            Ok(())
        })
        .unwrap();
    assert_eq!(early, projected);
    actual
        .render_with_pages(&mut vec![[0.0; 2]; (due - 8) as usize], &mut pages())
        .unwrap();
    assert_eq!(actual.performance_frame(), due);
    assert!(actual.session_route_status(0).unwrap().pending.is_some());
    let mut exact = Vec::new();
    actual
        .visit_next_stream_chunk(8, None, &mut |row| {
            exact.push(row);
            Ok(())
        })
        .unwrap();
    assert_eq!(exact, projected);
    actual
        .render_with_pages(&mut [[0.0; 2]; 8], &mut pages())
        .unwrap();
    assert_eq!(actual.session_route_status(0).unwrap().pending, None);
    assert_eq!(actual.performance_frame(), due + 8);
    actual
        .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Pause))
        .unwrap();
    actual
        .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Seek(5000)))
        .unwrap();
    let paused = actual
        .prepare_stream_action(PlaybackAction::Session(
            SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            },
        ))
        .unwrap();
    let zero = actual
        .visit_prepared_stream_action(&paused, 8, &mut |_| {
            panic!("paused launch has no page demand")
        })
        .unwrap();
    assert_eq!(
        (zero.performance_frame, zero.arrangement_frame, zero.frames),
        (5000, 5000, 0)
    );
    actual.commit_stream_action(paused).unwrap();
    assert_eq!(actual.position(), 5000);
    assert!(!actual.playing());
}

#[test]
fn prewarm_includes_the_outgoing_session_audio_release() {
    let mut actual = Playback::from_prepared(action_revision(true));
    actual
        .try_apply_action(PlaybackAction::Transport(PlaybackCommand::PlayFrom(95_992)))
        .unwrap();
    let clip = actual.prepared_revision().unwrap().session().audio_clips()[0].id;
    actual
        .try_apply_action(PlaybackAction::Session(
            SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            },
        ))
        .unwrap();
    actual
        .render_with_pages(&mut [[0.0; 2]; 1], &mut pages())
        .unwrap();
    let action = PlaybackAction::Session(SessionPlaybackCommand::BackToArrangement {
        quantization: LaunchQuantization::FourQuarter,
    });
    let prepared = actual.prepare_stream_action(action).unwrap();
    let due = prepared.scheduled_due_frame().unwrap();
    let mut prepared_demand = Vec::new();
    actual
        .visit_prepared_stream_action(&prepared, 8, &mut |row| {
            prepared_demand.push(row);
            Ok(())
        })
        .unwrap();
    actual.commit_stream_action(prepared).unwrap();
    let mut early = Vec::new();
    actual
        .visit_pending_stream_chunk(due, 8, &mut |row| {
            early.push(row);
            Ok(())
        })
        .unwrap();
    let distance = usize::try_from(due - actual.performance_frame()).unwrap();
    actual
        .render_with_pages(&mut vec![[0.0; 2]; distance], &mut pages())
        .unwrap();
    let mut at_due = Vec::new();
    actual
        .visit_next_stream_chunk(8, None, &mut |row| {
            at_due.push(row);
            Ok(())
        })
        .unwrap();
    assert!(at_due.iter().any(|extent| extent.page_index == 1));
    assert_eq!(early, at_due, "prewarm must cover the actual due demand");
    assert_eq!(
        prepared_demand, at_due,
        "prepared demand must cover the actual due demand"
    );
}

#[test]
fn prepared_action_cannot_follow_a_new_runtime_reusing_the_same_box_and_revision() {
    let revision = action_revision(false);
    let mut runtime = Box::new(SessionPlayback::new(Arc::clone(&revision)).unwrap());
    let address = std::ptr::from_ref(&*runtime).addr();
    let prepared = runtime
        .prepare_stream_action_in_scope(
            PlaybackAction::Transport(PlaybackCommand::PlayFrom(0)),
            true,
        )
        .unwrap();
    // Same physical storage, graph Arc, clocks and routes; this is nevertheless
    // a new actual runtime, so a plan from the old instance must be refused.
    *runtime = SessionPlayback::new(revision).unwrap();
    assert_eq!(std::ptr::from_ref(&*runtime).addr(), address);
    assert!(runtime.validate_stream_action(&prepared).is_err());
}

#[test]
fn timed_transport_with_borrowed_pages_matches_prepared_session_spans() {
    use crate::{TimedTransportCommand, playback::Playback};
    let revision = action_revision(false);
    let mut timed = Playback::from_prepared(Arc::clone(&revision));
    let mut oracle = Playback::from_prepared(revision);
    let commands = [
        TimedTransportCommand {
            offset: 0,
            command: PlaybackCommand::PlayFrom(4090),
        },
        TimedTransportCommand {
            offset: 7,
            command: PlaybackCommand::Seek(2),
        },
        TimedTransportCommand {
            offset: 19,
            command: PlaybackCommand::Stop,
        },
        TimedTransportCommand {
            offset: 23,
            command: PlaybackCommand::PlayFrom(4094),
        },
    ];
    let mut actual = [[0.0; 2]; 40];
    let mut expected = [[0.0; 2]; 40];
    let mut master = [[0.0; 2]; 40];
    let mut track = [[0.0; 2]; 40];
    let mut clock = [None; 40];
    let mut nodes = Vec::new();
    {
        let mut observe = |signal, offset, frames: &[Frame]| {
            let target = match signal {
                crate::ProjectSignal::Master => &mut master,
                crate::ProjectSignal::Track(1) => &mut track,
                other => panic!("unexpected signal {other:?}"),
            };
            target[offset..offset + frames.len()].copy_from_slice(frames);
        };
        let mut timing = |offset, frame, len, _: &crate::PreparedTempoMap| {
            for i in 0..len {
                clock[offset + i] = Some(frame + i as u64);
            }
        };
        let mut tap = crate::ProjectOutputTap::new(&[1], &mut observe)
            .unwrap()
            .with_timing(&mut timing);
        timed
            .render_with_timed_transport_pages_and_signal_observers(
                &mut actual,
                &commands,
                &mut pages(),
                Some(&mut |identity, phase, offset, frames: &[Frame]| {
                    nodes.push((identity, phase, offset, frames.len()))
                }),
                Some(&mut tap),
            )
            .unwrap();
    }
    assert_eq!(master, actual);
    assert_eq!(track, actual);
    assert!(!nodes.is_empty());
    assert!(nodes.iter().all(|(_, _, offset, len)| offset + len <= 40));
    for offset in [0, 7, 23] {
        assert!(nodes.iter().any(|(_, _, at, _)| *at == offset));
    }
    assert_eq!(clock[0], Some(4090));
    assert_eq!(clock[7], Some(2));
    assert_eq!(clock[19], None);
    assert_eq!(clock[23], Some(4094));
    let mut provider = pages();
    for (index, command) in commands.iter().enumerate() {
        oracle
            .try_apply_action(PlaybackAction::Transport(command.command))
            .unwrap();
        let end = commands
            .get(index + 1)
            .map_or(expected.len(), |next| next.offset);
        oracle
            .render_with_pages(&mut expected[command.offset..end], &mut provider)
            .unwrap();
    }
    assert_eq!(actual, expected);
    assert!(actual.iter().flatten().any(|sample| *sample != 0.0));
    assert_eq!(timed.position(), oracle.position());
}
