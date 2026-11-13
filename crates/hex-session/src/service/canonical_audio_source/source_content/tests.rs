use super::*;
use crate::service::verified_source_tests::{Files, reservation};
use crate::service::{AudioBatchContext, AudioBatchDestination, AudioBatchImport};
use crate::{DocumentAudioMode, PreparedImport, Session};
use hex_project::{BeatTime, Command, Created, Tempo};
use std::sync::Arc;

fn capture(session: &Session, generation: u64, revision: u64) -> CanonicalAudioSourceSnapshot {
    CanonicalAudioSourceSnapshot::capture(
        &Arc::new(session.project().clone()),
        generation,
        revision,
        session,
    )
}

fn fixture() -> (
    Files,
    Session,
    hex_project::TrackId,
    hex_project::AssetId,
    hex_project::ClipId,
) {
    let files = Files::new();
    let path = files.wav("source.wav", 48_000, 128, 8192);
    let decoded = hex_media::decode_wav_file(path, hex_media::Limits::new(8192, 1024)).unwrap();
    let mut session = Session::new(48_000, 16, 8192).unwrap();
    let Created::Imported { track, asset, clip } = session
        .import(PreparedImport::new(decoded).unwrap())
        .unwrap()
    else {
        panic!("import")
    };
    (files, session, track, asset, clip)
}

#[test]
fn raw_audio_witness_tracks_exact_canonical_content_and_contexts() {
    let (_files, mut session, track, asset, clip) = fixture();
    let old = capture(&session, 7, 1);
    session
        .command(Command::Mute { track, muted: true })
        .unwrap();
    let candidate = capture(&session, 7, 2);
    let witness = old
        .raw_arrangement_content_witness(&candidate, track)
        .expect("unchanged raw source");
    assert!(witness.matches(old.context(), candidate.context(), track));
    assert!(!witness.matches(candidate.context(), old.context(), track));
    assert!(
        old.raw_arrangement_content_witness(&capture(&session, 8, 2), track)
            .is_none()
    );
    assert!(
        old.raw_arrangement_content_witness(&capture(&session, 7, 1), track)
            .is_none()
    );

    let mut replaced = capture(&session, 7, 2);
    replaced.sources.insert(
        asset,
        AudioSourceBinding::Resident(hex_engine::Audio::new(48_000, vec![[0.25; 2]; 128]).unwrap()),
    );
    assert!(
        old.raw_arrangement_content_witness(&replaced, track)
            .is_none()
    );
    replaced.sources.remove(&asset);
    assert!(
        old.raw_arrangement_content_witness(&replaced, track)
            .is_none()
    );
    let mut other_mode = capture(&session, 7, 2);
    other_mode.mode = DocumentAudioMode::VerifiedStreaming;
    assert!(
        old.raw_arrangement_content_witness(&other_mode, track)
            .is_none()
    );
    let mut relink = session.project().snapshot();
    relink
        .assets
        .iter_mut()
        .find(|row| row.id == asset.value())
        .unwrap()
        .path = "other.wav".into();
    let mut changed_asset = capture(&session, 7, 2);
    changed_asset.project = Arc::new(hex_project::Project::from_snapshot(relink).unwrap());
    assert!(
        old.raw_arrangement_content_witness(&changed_asset, track)
            .is_none()
    );

    session
        .command(Command::MoveClip {
            clip,
            track,
            start: 16,
        })
        .unwrap();
    assert!(
        old.raw_arrangement_content_witness(&capture(&session, 7, 3), track)
            .is_none()
    );
}

#[test]
fn raw_audio_witness_refuses_tempo_changes_and_missing_tracks() {
    let (_files, mut session, track, _, _) = fixture();
    let old = capture(&session, 1, 1);
    session
        .command(Command::SetTempoPoint {
            beat: BeatTime::ZERO,
            tempo: Tempo::new(130_000_000).unwrap(),
        })
        .unwrap();
    assert!(
        old.raw_arrangement_content_witness(&capture(&session, 1, 2), track)
            .is_none()
    );
    let mut missing = capture(&session, 1, 2);
    let empty = Session::new(48_000, 16, 8192).unwrap();
    missing.project = Arc::new(empty.project().clone());
    assert!(
        old.raw_arrangement_content_witness(&missing, track)
            .is_none()
    );
}

#[test]
fn raw_audio_witness_compares_verified_file_facts_and_excludes_session_cells() {
    let files = Files::new();
    let path = files.wav("verified.wav", 48_000, 128, 8192);
    let mut session = Session::new_with_audio_mode(
        48_000,
        16,
        8192,
        DocumentAudioMode::VerifiedStreaming,
        Some(reservation(2)),
    )
    .unwrap();
    let context = AudioBatchContext {
        project_generation: 1,
        revision: 0,
        active_gesture: false,
    };
    let batch = AudioBatchImport::capture(
        vec![path],
        AudioBatchDestination::NewTracks {
            before_track: None,
            start: 0,
        },
        &session,
        context,
    )
    .unwrap();
    batch.run(&mut session, None, || context).unwrap();
    let track = session.project().tracks()[0].id();
    let clip = session.project().track(track).unwrap().clips()[0].id();
    let old = capture(&session, 1, 1);
    session
        .command(Command::Mute { track, muted: true })
        .unwrap();
    let candidate = capture(&session, 1, 2);
    assert!(
        old.raw_arrangement_content_witness(&candidate, track)
            .is_some()
    );
    let asset = session.project().track(track).unwrap().clips()[0].asset();
    let replacement = files.wav("other.wav", 48_000, 128, 4096);
    let other =
        crate::VerifiedAudioFile::inspect(replacement, 48_000, reservation(2).reader, || false)
            .unwrap();
    let mut changed_source = capture(&session, 1, 2);
    changed_source
        .sources
        .insert(asset, AudioSourceBinding::VerifiedFile(other));
    assert!(
        old.raw_arrangement_content_witness(&changed_source, track)
            .is_none()
    );
    let Created::Scene(scene) = session
        .command(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    session
        .command(Command::CopyArrangementAudioClipToSession {
            source: clip,
            scene,
        })
        .unwrap();
    let with_cell = capture(&session, 1, 3);
    session
        .command(Command::Mute {
            track,
            muted: false,
        })
        .unwrap();
    assert!(
        with_cell
            .raw_arrangement_content_witness(&capture(&session, 1, 4), track)
            .is_none()
    );
}

#[test]
fn raw_audio_witness_refuses_edited_audio_even_when_both_snapshots_match() {
    let (_files, mut session, track, _, clip) = fixture();
    session
        .command(Command::EditAudioEvents {
            clip,
            operation: hex_project::AudioEventEdit::Split { at: 64 },
        })
        .unwrap();
    let old = capture(&session, 1, 1);
    session
        .command(Command::Mute { track, muted: true })
        .unwrap();
    assert!(
        old.raw_arrangement_content_witness(&capture(&session, 1, 2), track)
            .is_none()
    );
}

#[test]
fn raw_audio_witness_refuses_hybrid_midi_even_when_both_snapshots_match() {
    let (_files, mut session, track, _, _) = fixture();
    session
        .command(Command::PlaceMidiClip {
            track,
            start: BeatTime::from_quarters(1).unwrap(),
            length: BeatTime::from_quarters(1).unwrap(),
        })
        .unwrap();
    let old = capture(&session, 1, 1);
    session
        .command(Command::Mute { track, muted: true })
        .unwrap();
    assert!(
        old.raw_arrangement_content_witness(&capture(&session, 1, 2), track)
            .is_none()
    );
}
