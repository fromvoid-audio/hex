//! Verified Arrangement route dispatch through Session and its real audio sink.
use super::*;
use hex_project::{Command, Created, TrackDestination, TrackSelectionIds};
use hex_session::service::DeviceInsertOutcome;

#[test]
fn verified_group_entry_waits_for_the_exact_callback_receipt() {
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
    let status = h.audio.as_ref().unwrap().status().unwrap();
    let deadline = Instant::now() + h.wait;
    while h.render() != [[0.3125; 2]; 64] {
        assert!(
            Instant::now() < deadline,
            "original file-backed mix did not play"
        );
        std::thread::yield_now();
    }
    assert_eq!(status.applied_audio_source(), Some(predecessor));
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
    assert_eq!(status.applied_audio_source(), Some(predecessor));
    h.render();
    assert_eq!(status.applied_audio_source(), Some(predecessor));
    let installed = h.next_with_audio();
    assert!(installed.error.is_none(), "{:?}", installed.error);
    assert!(installed.request_complete);
    let receipt = installed.device_insert_complete.as_ref().unwrap();
    assert_eq!(receipt.outcome, DeviceInsertOutcome::Installed);
    let applied = status.applied_audio_source().unwrap();
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
}
