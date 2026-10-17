use super::*;

fn action_revision(looped: bool) -> Arc<PreparedRevision> {
    let mut snapshot = audio_project(5, 18, 4).snapshot();
    let SessionCellContentSnapshot::Audio(clip) = &mut snapshot.session.cells[0].content else {
        panic!("audio fixture")
    };
    clip.source_start = 4094;
    clip.source_end = 4108;
    snapshot.assets[0].source_sample_rate = 24_000;
    snapshot.assets[0].source_frames = 4096;
    snapshot.assets[0].playback_frames = 8192;
    snapshot.assets[0].conversion_version = hex_project::SINC_CONVERSION_V1;
    snapshot.tracks[0].content = TrackContentSnapshot::Audio(vec![ClipSnapshot {
        name: "clip".into(),
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
    if looped {
        snapshot.loop_region = Some(LoopRegion::new(0, 64, true).unwrap());
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
            PreparedAudioSource::Resident(crate::Audio::new(48_000, vec![[0.0; 2]; 32]).unwrap())
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
