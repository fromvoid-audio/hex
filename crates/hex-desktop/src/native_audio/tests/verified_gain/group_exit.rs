//! Exercise the typed public exit with a real following root at the same row.
use super::*;
use hex_project::{Command, Created, Track, TrackDestination, TrackSelectionIds};
use hex_session::device_gesture::{ParameterId, ParameterValue};
use hex_session::service::DeviceInsertOutcome;

#[test]
fn verified_group_exit_before_the_following_root_waits_for_audio() {
    verify_explicit_exit(ExitCase::Populated);
}

#[test]
fn verified_last_child_exit_before_the_following_root_waits_for_audio() {
    verify_explicit_exit(ExitCase::LastChild);
}

#[test]
fn pausing_verified_last_child_exit_keeps_the_group_source_and_history() {
    verify_explicit_exit(ExitCase::InterruptedLastChild);
}

enum ExitCase {
    Populated,
    LastChild,
    InterruptedLastChild,
}

fn verify_explicit_exit(case: ExitCase) {
    let surviving_child = matches!(case, ExitCase::Populated);
    let _native = native_fixture();
    let files = Files::new();
    let mut h = actual_native_harness();
    h.next();
    h.open();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.open();
    h.applied(h.context.unwrap());
    let import = |h: &mut Harness, name, sample| {
        let imported = h.run(Request::Import(files.constant_wav(
            name,
            48_000,
            32_768,
            &[sample; 2],
        )));
        h.applied(h.context.unwrap());
        imported.state.project.tracks().last().unwrap().id()
    };
    let child = surviving_child.then(|| import(&mut h, "existing-child.wav", 2048));
    let moving_leaf = import(&mut h, "moving-leaf.wav", 8192);
    let following_root = import(&mut h, "following-root.wav", 4096);
    let children: Vec<_> = child.into_iter().chain([moving_leaf]).collect();
    let grouped = group_imports(&mut h, &children);
    let predecessor = h.applied(h.context.unwrap());
    assert_eq!(predecessor.context.editor_revision, grouped.state.revision);
    let group = &grouped.state.project.tracks()[0];
    let leaf = grouped.state.project.track(moving_leaf).unwrap();
    let following = grouped.state.project.track(following_root).unwrap();
    let expected_rows: Vec<_> = [group.id()]
        .into_iter()
        .chain(children)
        .chain([following.id()])
        .collect();
    assert!(
        grouped
            .state
            .project
            .tracks()
            .iter()
            .map(Track::id)
            .eq(expected_rows.iter().copied())
    );
    if let Some(child) = child {
        assert_eq!(
            grouped.state.project.track(child).unwrap().parent_group(),
            Some(group.id())
        );
    }
    assert_eq!(leaf.parent_group(), Some(group.id()));
    assert_eq!(following.parent_group(), None);
    let applied_source = |h: &Harness| {
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_audio_source()
    };
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    // The Group inverts the left channel. The following root contributes
    // +0.125 on both channels throughout the cut.
    let sibling = if surviving_child { 0.0625 } else { 0.0 };
    let old = [0.125 - (0.25 + sibling), 0.125 + (0.25 + sibling)];
    let deadline = Instant::now() + h.wait;
    while h.render() != [old; 64] {
        assert!(Instant::now() < deadline, "old grouped PCM did not play");
        std::thread::yield_now();
    }
    assert_eq!(applied_source(&h), Some(predecessor));
    let pending = h.run(Request::ReparentTracks {
        project_generation: predecessor.context.project_generation,
        revision: grouped.state.revision,
        tracks: TrackSelectionIds::new(&[leaf.id()]).unwrap(),
        destination: TrackDestination {
            parent_group: None,
            before: Some(following.id()),
        },
    });
    assert!(
        !pending.request_complete,
        "explicit Group exit completed before audio"
    );
    assert_eq!(pending.state.project, grouped.state.project);
    assert_eq!(pending.state.revision, grouped.state.revision);
    assert_eq!(applied_source(&h), Some(predecessor));
    if matches!(case, ExitCase::InterruptedLastChild) {
        let deadline = Instant::now() + h.wait;
        loop {
            let pcm = h.render();
            assert_eq!(applied_source(&h), Some(predecessor));
            assert!(
                pcm.iter()
                    .all(|[left, right]| (left + right - 0.25).abs() < 1e-6)
            );
            if pcm.iter().any(|&frame| frame != old) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the old Group did not start fading"
            );
            std::thread::yield_now();
        }
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Pause)
            .unwrap();
        let refused = h.next_with_audio();
        assert!(refused.request_complete);
        let receipt = refused.device_insert_complete.as_ref().unwrap();
        assert!(matches!(
            receipt.outcome,
            DeviceInsertOutcome::Refused(_) | DeviceInsertOutcome::Cancelled
        ));
        assert_eq!(refused.state.project, grouped.state.project);
        assert_eq!(refused.state.revision, grouped.state.revision);
        assert_eq!(
            (refused.state.can_undo, refused.state.can_redo),
            (grouped.state.can_undo, grouped.state.can_redo),
        );
        assert_eq!(applied_source(&h), Some(predecessor));
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
        let deadline = Instant::now() + h.wait;
        while h.render() != [old; 64] {
            assert!(
                Instant::now() < deadline,
                "the original Group did not resume"
            );
            std::thread::yield_now();
        }
        assert_eq!(applied_source(&h), Some(predecessor));
        return;
    }
    for [left, right] in h.render() {
        assert!((left + right - 0.25).abs() < 1e-6);
    }
    assert_eq!(applied_source(&h), Some(predecessor));
    let committed = h.next_with_audio();
    assert!(committed.error.is_none(), "{:?}", committed.error);
    assert!(committed.request_complete);
    assert_eq!(committed.state.revision, grouped.state.revision + 1);
    assert!(
        committed
            .state
            .project
            .tracks()
            .iter()
            .map(Track::id)
            .eq(expected_rows)
    );
    assert_eq!(
        committed
            .state
            .project
            .track(leaf.id())
            .unwrap()
            .parent_group(),
        None
    );
    if let Some(child) = child {
        assert_eq!(
            committed.state.project.track(child).unwrap().parent_group(),
            Some(group.id())
        );
    }
    let receipt = committed.device_insert_complete.as_ref().unwrap();
    assert_eq!(receipt.outcome, DeviceInsertOutcome::Installed);
    let applied = applied_source(&h).unwrap();
    assert_eq!(applied.context, receipt.submission.context);
    assert_eq!(applied.audio_revision, receipt.submission.audio_revision);
    assert_eq!(applied.output_identity, receipt.submission.output_identity);
    assert_eq!(applied.output_identity, predecessor.output_identity);
    let deadline = Instant::now() + h.wait;
    let direct = [0.375 - sibling, 0.375 + sibling];
    while h.render() != [direct; 64] {
        assert!(Instant::now() < deadline, "new direct PCM did not play");
        std::thread::yield_now();
    }
}

fn group_imports(h: &mut Harness, children: &[hex_project::TrackId]) -> Event {
    let grouped = h.run(Request::Edit(Command::GroupTracks {
        tracks: TrackSelectionIds::new(children).unwrap(),
        name: "group".into(),
    }));
    let Some(Created::Track(group)) = grouped.created else {
        panic!("group");
    };
    h.applied(h.context.unwrap());
    let utility = hex_engine::default_native_device_state(hex_engine::DeviceTypeId::from_static(
        "hex.utility",
    ))
    .unwrap();
    let inversion = hex_engine::patch_native_device_parameter(
        &utility,
        ParameterId::from_static(5),
        ParameterValue::Bool(true),
    )
    .unwrap();
    h.run_verified_device_insert(Command::InsertDevice {
        track: group,
        before: None,
        state: inversion,
    })
}
