use crate::{PreparedImport, Session, import_batch::AudioBatchDestination};
use hex_media::{Limits, ResampleLimits, decode_wav_file};
use hex_project::{AssetId, Command, Created, Project, SessionCellContent, TrackId};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

static NEXT: AtomicU64 = AtomicU64::new(0);

/// A private directory whose files stay on disk until the test ends, so a
/// re-import decodes the same unchanged file.
struct Folder(PathBuf);
impl Folder {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hex-import-reuse-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("test folder");
        Self(path)
    }
}
impl Drop for Folder {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_wav(path: &Path, frames: u32) {
    let bytes = frames * 2;
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + bytes).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&48_000_u32.to_le_bytes());
    wav.extend_from_slice(&96_000_u32.to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&bytes.to_le_bytes());
    for index in 0..frames {
        wav.extend_from_slice(&(index as i16 * 97).to_le_bytes());
    }
    fs::write(path, wav).expect("write wav");
}

fn decode(path: &Path) -> hex_media::DecodedAudio {
    decode_wav_file(path, Limits::new(1 << 20, 100_000)).expect("decode")
}

fn prepared(path: &Path) -> PreparedImport {
    PreparedImport::at_rate(
        decode(path),
        48_000,
        ResampleLimits {
            max_source_frames: 100_000,
            max_output_frames: 100_000,
        },
    )
    .expect("prepare")
}

fn session() -> Session {
    Session::new(48_000, 16, 1_000_000).expect("session")
}

fn add_track(session: &mut Session) -> TrackId {
    let Ok(Created::Track(track)) = session.editor.execute(Command::AddTrack {
        name: "target".into(),
    }) else {
        panic!("expected a track");
    };
    track
}

fn batch(session: &mut Session, members: Vec<PreparedImport>, destination: AudioBatchDestination) {
    let expected = session.project().clone();
    session
        .import_prepared_batch_revision(members, destination, &expected, None, || Ok(()))
        .expect("batch import");
}

fn imported(created: &Created) -> (TrackId, AssetId, hex_project::ClipId) {
    let Created::Imported { track, asset, clip } = *created else {
        panic!("expected an import");
    };
    (track, asset, clip)
}

/// Project content; Undo keeps the id allocator's high-water mark by design.
fn content(project: &Project) -> (Vec<hex_project::Track>, Vec<hex_project::Asset>) {
    (
        project.tracks().to_vec(),
        project.assets().cloned().collect(),
    )
}

/// Resident custody totals that a reused member must leave unchanged.
fn residency(session: &Session) -> (usize, u64, usize, usize) {
    (
        session.media.len(),
        session.resident_frames(),
        session.resident_waveform_bins(),
        session.resident_waveform_bytes(),
    )
}

#[test]
fn batch_reimport_of_unchanged_file_reuses_asset_with_one_undo_step() {
    let folder = Folder::new();
    let path = folder.0.join("loop.wav");
    write_wav(&path, 64);
    let mut session = session();
    batch(
        &mut session,
        vec![prepared(&path)],
        AudioBatchDestination::NewTracks {
            before_track: None,
            start: 0,
        },
    );
    let first = session.project().clone();
    let residency_after_first = residency(&session);
    let asset = session.project().assets().next().expect("asset").id();

    // Two members of the same file onto one track: both reuse, concatenated.
    let track = add_track(&mut session);
    let with_track = session.project().clone();
    let expected = session.project().clone();
    let (created, _) = session
        .import_prepared_batch_revision(
            vec![prepared(&path), prepared(&path)],
            AudioBatchDestination::Track { track, start: 10 },
            &expected,
            None,
            || Ok(()),
        )
        .expect("reimport");
    assert_eq!(session.project().assets().count(), 1);
    assert_eq!(residency(&session), residency_after_first);
    let starts = created
        .iter()
        .map(|result| {
            let (on, reused, clip) = imported(result);
            assert_eq!((on, reused), (track, asset));
            session.project().clip(clip).expect("clip").start()
        })
        .collect::<Vec<_>>();
    assert_eq!(starts, vec![10, 74]);
    let reimported = session.project().clone();

    // The whole reuse batch is one Undo step; media residency is unaffected.
    assert!(session.undo());
    assert_eq!(content(session.project()), content(&with_track));
    assert_eq!(residency(&session), residency_after_first);
    assert!(session.redo());
    assert_eq!(session.project(), &reimported);
    assert_eq!(residency(&session), residency_after_first);
    assert!(session.undo());
    assert!(session.undo());
    assert_eq!(content(session.project()), content(&first));
}

#[test]
fn rewritten_file_with_different_length_or_mtime_imports_a_new_asset() {
    let folder = Folder::new();
    let path = folder.0.join("take.wav");
    write_wav(&path, 64);
    let mut session = session();
    let destination = AudioBatchDestination::NewTracks {
        before_track: None,
        start: 0,
    };
    batch(&mut session, vec![prepared(&path)], destination);
    let after_first = residency(&session);

    // Same path and length, only the modified time differs.
    let modified = fs::metadata(&path)
        .expect("metadata")
        .modified()
        .expect("mtime");
    fs::File::options()
        .write(true)
        .open(&path)
        .expect("open")
        .set_modified(modified + Duration::from_secs(3_600))
        .expect("set mtime");
    batch(&mut session, vec![prepared(&path)], destination);
    assert_eq!(session.project().assets().count(), 2);
    assert!(session.resident_frames() > after_first.1);

    // Same path, different length.
    write_wav(&path, 80);
    batch(&mut session, vec![prepared(&path)], destination);
    assert_eq!(session.project().assets().count(), 3);

    // The newest identity is now resident and an unchanged re-import reuses it.
    let before = residency(&session);
    batch(&mut session, vec![prepared(&path)], destination);
    assert_eq!(session.project().assets().count(), 3);
    assert_eq!(residency(&session), before);
}

#[test]
fn single_file_clip_and_session_imports_reuse_resident_asset() {
    let folder = Folder::new();
    let path = folder.0.join("hit.wav");
    write_wav(&path, 32);
    let mut session = session();
    let (first, _) = session
        .import_prepared_revision(prepared(&path), None)
        .expect("first import");
    let (_, asset, _) = imported(&first);
    let before = residency(&session);

    let track = add_track(&mut session);
    let (clip, _) = session
        .import_prepared_audio_clip_with_overwrite_revision(prepared(&path), track, 5, None)
        .expect("clip reimport");
    assert_eq!(imported(&clip).1, asset);
    let (_, _, placed) = imported(&clip);
    assert_eq!(session.project().clip(placed).expect("clip").start(), 5);

    let Ok(Created::Scene(scene)) = session.editor.execute(Command::InsertScene {
        name: "scene 1".into(),
        before: None,
    }) else {
        panic!("expected a scene");
    };
    session
        .import_session_audio_clip_revision(prepared(&path), track, scene, None)
        .expect("session reimport");
    let Some(SessionCellContent::Audio(cell)) = session.project().session_cell(track, scene) else {
        panic!("expected an audio cell");
    };
    assert_eq!(cell.asset(), asset);
    assert_eq!(session.project().assets().count(), 1);
    assert_eq!(residency(&session), before);

    let after_cell = session.project().clone();
    assert!(session.undo());
    assert!(session.project().session_cell(track, scene).is_none());
    assert!(session.redo());
    assert_eq!(session.project(), &after_cell);
}

#[test]
fn refused_reuse_placement_leaves_project_history_and_residency_unchanged() {
    let folder = Folder::new();
    let path = folder.0.join("refused.wav");
    write_wav(&path, 32);
    let mut session = session();
    session
        .import_prepared_revision(prepared(&path), None)
        .expect("first import");
    let track = add_track(&mut session);
    assert!(session.editor.execute(Command::RemoveTrack(track)).is_ok());
    let before = session.project().clone();
    let residency_before = residency(&session);
    let undo_depth = session.editor.can_undo();

    assert!(
        session
            .import_prepared_audio_clip_revision(prepared(&path), track, 0, None)
            .is_err()
    );
    assert_eq!(session.project(), &before);
    assert_eq!(residency(&session), residency_before);
    assert_eq!(session.editor.can_undo(), undo_depth);
    assert!(session.undo());
    assert!(session.project().track(track).is_some());
}

#[test]
fn asset_removed_by_undo_or_reopened_project_follow_their_own_owners() {
    let folder = Folder::new();
    let path = folder.0.join("reopen.wav");
    write_wav(&path, 48);
    let mut session = session();
    let (first, _) = session
        .import_prepared_revision(prepared(&path), None)
        .expect("first import");
    let (_, asset, _) = imported(&first);
    let project: Project = session.project().clone();

    // A reopened project fills the identity in from its own decode.
    let mut reopened =
        Session::from_loaded(project, 16, 1_000_000, vec![(asset, decode(&path))]).expect("open");
    let before = residency(&reopened);
    let (again, _) = reopened
        .import_prepared_revision(prepared(&path), None)
        .expect("reimport after reopen");
    assert_eq!(imported(&again).1, asset);
    assert_eq!(residency(&reopened), before);

    // Undo removes the asset from the project; its retained media is not
    // reusable, so the re-import mints a fresh asset as before.
    assert!(session.undo());
    assert_eq!(session.project().assets().count(), 0);
    let (fresh, _) = session
        .import_prepared_revision(prepared(&path), None)
        .expect("reimport after undo");
    assert_ne!(imported(&fresh).1, asset);
    assert_eq!(session.project().assets().count(), 1);
}

#[test]
fn members_without_file_identity_never_reuse() {
    let folder = Folder::new();
    let path = folder.0.join("anonymous.wav");
    write_wav(&path, 16);
    let mut session = session();
    session
        .import_prepared_revision(prepared(&path), None)
        .expect("first import");
    let mut anonymous = prepared(&path);
    anonymous.file_identity = None;
    assert_eq!(session.reusable_import_asset(&anonymous), None);
    assert!(session.reusable_import_asset(&prepared(&path)).is_some());
}

#[test]
fn pre_decode_lookup_matches_only_an_unchanged_resident_file_and_shares_its_pcm() {
    let folder = Folder::new();
    let path = folder.0.join("hit.wav");
    write_wav(&path, 48);
    let mut session = session();
    let destination = AudioBatchDestination::NewTracks {
        before_track: None,
        start: 0,
    };
    assert!(session.unchanged_resident_import(&path).is_none());
    batch(&mut session, vec![prepared(&path)], destination);
    let residency_after_first = residency(&session);

    let member = session
        .unchanged_resident_import(&path)
        .expect("unchanged file matches");
    let resident = session.media.values().next().expect("resident");
    assert!(member.audio.shares_pcm_with(&resident.audio));
    assert!(member.source_audio.shares_pcm_with(&resident.source_audio));
    batch(&mut session, vec![member], destination);
    assert_eq!(session.project().assets().count(), 1);
    assert_eq!(residency(&session), residency_after_first);

    // Another path to the same bytes, or a changed or missing file, needs a decode.
    let other = folder.0.join("linked.wav");
    fs::hard_link(&path, &other).expect("hard link");
    assert!(session.unchanged_resident_import(&other).is_none());
    write_wav(&path, 49);
    assert!(session.unchanged_resident_import(&path).is_none());
    fs::remove_file(&path).expect("remove");
    assert!(session.unchanged_resident_import(&path).is_none());
}
