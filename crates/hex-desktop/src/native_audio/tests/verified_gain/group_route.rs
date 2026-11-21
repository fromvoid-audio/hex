//! Verified Arrangement route dispatch through Session and its real audio sink.
use super::*;
use hex_project::{Command, Created, TrackDestination, TrackSelectionIds};
use hex_session::service::DeviceInsertOutcome;

#[test]
fn verified_group_entry_waits_for_the_exact_callback_receipt() {
    check_verified_group_entry(FollowUp::Reorder);
}

#[test]
fn verified_group_route_history_waits_for_each_exact_callback_receipt() {
    check_verified_group_entry(FollowUp::History);
}

enum FollowUp {
    Reorder,
    History,
}

fn check_verified_group_entry(follow_up: FollowUp) {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = actual_native_harness();
    h.wait = Duration::from_secs(2);
    h.next();
    h.open();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.open();
    h.applied(h.context.unwrap());
    let imported = h.run(Request::Import(files.constant_wav(
        "child.wav",
        48_000,
        32_768,
        &[2048, 2048],
    )));
    h.applied(h.context.unwrap());
    let child = imported.state.project.tracks()[0].id();
    let imported = h.run(Request::Import(files.constant_wav(
        "moving.wav",
        48_000,
        32_768,
        &[8192, 8192],
    )));
    h.applied(h.context.unwrap());
    let leaf = imported.state.project.tracks()[1].id();
    let original = h.run(Request::Edit(Command::GroupTracks {
        tracks: TrackSelectionIds::new(&[child]).unwrap(),
        name: "group".into(),
    }));
    let Some(Created::Track(group)) = original.created else {
        panic!("group");
    };
    let predecessor = h.applied(h.context.unwrap());
    assert_eq!(predecessor.context.editor_revision, original.state.revision);
    let rows = |event: &Event| {
        event
            .state
            .project
            .tracks()
            .iter()
            .map(|track| track.id())
            .collect::<Vec<_>>()
    };
    assert_eq!(rows(&original), [group, child, leaf]);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let applied_source = |h: &Harness| {
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_audio_source()
    };
    let deadline = Instant::now() + h.wait;
    while h.render() != [[0.3125; 2]; 64] {
        assert!(
            Instant::now() < deadline,
            "original file-backed mix did not play"
        );
        std::thread::yield_now();
    }
    assert_eq!(applied_source(&h), Some(predecessor));
    let generation = predecessor.context.project_generation;
    let selection = TrackSelectionIds::new(&[leaf]).unwrap();
    let pending = h.run(Request::ReparentTracks {
        project_generation: generation,
        revision: original.state.revision,
        tracks: selection.clone(),
        destination: TrackDestination {
            parent_group: Some(group),
            before: None,
        },
    });
    assert!(
        !pending.request_complete,
        "track move completed before audio"
    );
    assert_eq!(
        pending.status.as_deref(),
        Some("track move pending · waiting for audio")
    );
    assert_eq!(pending.state.project, original.state.project);
    assert_eq!(pending.state.revision, original.state.revision);
    assert_eq!(applied_source(&h), Some(predecessor));
    h.render();
    assert_eq!(applied_source(&h), Some(predecessor));
    let installed = h.next_with_audio();
    assert!(installed.error.is_none(), "{:?}", installed.error);
    assert!(installed.request_complete);
    let receipt = installed.device_insert_complete.as_ref().unwrap();
    assert_eq!(receipt.outcome, DeviceInsertOutcome::Installed);
    let applied = applied_source(&h).unwrap();
    assert_eq!(applied.context, receipt.submission.context);
    assert_eq!(applied.audio_revision, receipt.submission.audio_revision);
    assert_eq!(applied.output_identity, receipt.submission.output_identity);
    assert_eq!(applied.output_identity, predecessor.output_identity);
    assert_eq!(installed.state.revision, original.state.revision + 1);
    assert_eq!(
        installed.state.project.track(leaf).unwrap().parent_group(),
        Some(group)
    );
    assert_eq!(rows(&installed), [group, child, leaf]);
    if matches!(follow_up, FollowUp::History) {
        let before = Arc::clone(&original.state.project);
        let after = Arc::clone(&installed.state.project);
        let mut current = installed;
        let mut source = applied;
        for (request, expected, label) in [
            (Request::Undo, before, "Undo"),
            (Request::Redo, after, "Redo"),
        ] {
            let pending = h.run(request);
            assert!(!pending.request_complete, "{label} completed before audio");
            assert_eq!(
                pending.status.as_deref(),
                Some("history pending · waiting for audio")
            );
            assert_eq!(pending.state.project, current.state.project);
            assert_eq!(pending.state.revision, current.state.revision);
            assert_eq!(applied_source(&h), Some(source));
            h.render();
            assert_eq!(applied_source(&h), Some(source));
            let committed = h.next_with_audio();
            assert!(committed.error.is_none(), "{:?}", committed.error);
            assert!(committed.request_complete);
            assert_eq!(committed.status.as_deref(), Some("history committed"));
            assert_eq!(committed.state.project, expected);
            assert_eq!(committed.state.revision, current.state.revision + 1);
            let receipt = committed.device_insert_complete.as_ref().unwrap();
            assert_eq!(receipt.outcome, DeviceInsertOutcome::Installed);
            source = applied_source(&h).unwrap();
            assert_eq!(source.context, receipt.submission.context);
            assert_eq!(source.audio_revision, receipt.submission.audio_revision);
            assert_eq!(source.output_identity, predecessor.output_identity);
            assert_eq!(source.output_identity, receipt.submission.output_identity);
            current = committed;
        }
        return;
    }
    let reordered = h.run(Request::ReorderTracks {
        project_generation: generation,
        revision: installed.state.revision,
        tracks: selection,
        before: Some(child),
    });
    assert!(reordered.request_complete);
    assert!(reordered.device_insert_complete.is_none());
    assert_eq!(reordered.state.revision, installed.state.revision + 1);
    assert_eq!(
        reordered.state.project.track(leaf).unwrap().parent_group(),
        Some(group)
    );
    assert_eq!(rows(&reordered), [group, leaf, child]);
    h.applied(h.context.unwrap());
    let undo_reorder = h.run(Request::Undo);
    assert!(undo_reorder.error.is_none(), "{:?}", undo_reorder.error);
    assert!(undo_reorder.request_complete);
    assert!(undo_reorder.device_insert_complete.is_none());
    assert_eq!(undo_reorder.state.project, installed.state.project);
}
