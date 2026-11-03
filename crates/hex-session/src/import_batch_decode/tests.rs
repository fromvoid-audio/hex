use super::*;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::AtomicU64,
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "hex-batch-decode-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        Self(dir)
    }
    fn wav(&self, name: &str, rate: u32, frames: u32, value: i16) -> PathBuf {
        let path = self.0.join(name);
        let mut bytes = header(rate, frames);
        for _ in 0..frames {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        fs::write(&path, bytes).unwrap();
        path
    }
    fn bad(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, b"unsupported media").unwrap();
        path
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn header(rate: u32, frames: u32) -> Vec<u8> {
    let bytes = frames * 2;
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + bytes).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * 2).to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&bytes.to_le_bytes());
    wav
}
fn context() -> AudioBatchContext {
    AudioBatchContext {
        project_generation: 7,
        revision: 19,
        active_gesture: false,
    }
}
fn session() -> Session {
    Session::new(48_000, 16, 8_000_000).unwrap()
}
fn destination() -> AudioBatchDestination {
    AudioBatchDestination::NewTracks {
        before_track: None,
        start: 8,
    }
}
fn capture(s: &Session, paths: Vec<PathBuf>) -> AudioBatchImport {
    AudioBatchImport::capture(paths, destination(), s, context()).unwrap()
}
fn unchanged(s: &Session, before: &Project, bank: usize, frames: u64) {
    assert_eq!(s.project(), before);
    assert_eq!(s.media.len(), bank);
    assert_eq!(s.resident_frames(), frames);
}
fn asset(result: Created) -> hex_project::AssetId {
    match result {
        Created::Imported { asset, .. } => asset,
        _ => panic!(),
    }
}

#[test]
fn real_ordered_mixed_rate_import_progress_paths_and_undo_redo_without_reopen() {
    let files = Files::new();
    let first = files.wav("z-MiXeD.WAV", 24_000, 5, 8192);
    let last = files.wav("A.wav", 48_000, 7, -8192);
    let input = vec![first.clone(), last.clone()];
    let originals: Vec<_> = input.iter().map(|p| fs::read(p).unwrap()).collect();
    let mut s = session();
    let op = capture(&s, input.clone());
    let control = op.control();
    assert_eq!(control.progress().phase, AudioBatchPhase::Queued);
    assert_eq!(control.progress().total_members, 2);
    let mut phases = Vec::new();
    let (created, _) = op
        .run(&mut s, None, || {
            phases.push(control.progress());
            context()
        })
        .unwrap();
    assert!(phases.iter().any(|p| p.phase == AudioBatchPhase::Decoding
        && p.member == 1
        && p.completed_members == 0));
    assert!(phases.iter().any(|p| p.phase == AudioBatchPhase::Converting
        && p.member == 2
        && p.completed_members == 1));
    assert!(
        phases
            .iter()
            .any(|p| p.phase == AudioBatchPhase::Prepared && p.completed_members == 2)
    );
    assert!(phases.iter().any(|p| p.phase == AudioBatchPhase::Compiling));
    let progress = control.progress();
    assert_eq!(progress.phase, AudioBatchPhase::Completed);
    assert_eq!(progress.completed_members, 2);
    assert_eq!(control.request_cancel(), AudioBatchCancel::TooLate);
    assert_eq!(s.resident_frames(), 22);
    for (i, result) in created.iter().enumerate() {
        let id = asset(*result);
        let metadata = s.project().assets().find(|a| a.id() == id).unwrap();
        assert_eq!(metadata.path(), input[i]);
        assert_eq!(fs::read(&input[i]).unwrap(), originals[i]);
        assert_eq!(
            s.audio(id).unwrap().frames().len(),
            if i == 0 { 10 } else { 7 }
        );
        assert_eq!(
            s.audio(id).unwrap().frames()[2][0].is_sign_positive(),
            i == 0
        );
        fs::remove_file(&input[i]).unwrap();
    }
    let after = s.project().clone();
    assert!(s.undo());
    assert!(s.project().tracks().is_empty());
    assert!(!s.editor.can_undo());
    assert!(s.redo());
    assert_eq!(s.project(), &after);
}

#[test]
fn one_file_with_legacy_riff_size_matches_worker_decode_and_single_import() {
    let files = Files::new();
    let path = files.wav("Source.wav", 24_000, 6, 1234);
    let mut original = fs::read(&path).unwrap();
    // Complete PCM chunks with the observed legacy four-byte container overrun.
    let legacy_size = u32::try_from(original.len() - 4).unwrap();
    original[4..8].copy_from_slice(&legacy_size.to_le_bytes());
    fs::write(&path, &original).unwrap();
    // Both import routes start from the same authored project. Cyan is the
    // only unused hue, so creation remains deterministic without replacing
    // production randomness or discarding colors from the comparison.
    let mut fixture = hex_project::Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    for color in hex_project::ElementColor::TRACK_CHOICES
        .into_iter()
        .filter(|color| *color != hex_project::ElementColor::Cyan)
    {
        let Created::Track(track) = fixture
            .execute(hex_project::Command::AddTrack {
                name: "authored color fixture".into(),
            })
            .unwrap()
        else {
            panic!("expected track")
        };
        fixture
            .execute(hex_project::Command::SetTrackColor { track, color })
            .unwrap();
    }
    let baseline = fixture.project().clone();
    let mut old = Session::from_loaded(baseline.clone(), 16, 8_000_000, Vec::new()).unwrap();
    let mut new = Session::from_loaded(baseline, 16, 8_000_000, Vec::new()).unwrap();
    let decoded = crate::resident_import::decode_resident_import(&path).unwrap();
    let prepared = PreparedImport::at_rate(
        decoded,
        48_000,
        ResampleLimits {
            max_source_frames: SOURCE_FRAMES,
            max_output_frames: 8_000_000,
        },
    )
    .unwrap();
    old.import_prepared_audio_track_at_revision(prepared, None, 8, None)
        .unwrap();
    capture(&new, vec![path.clone()])
        .run(&mut new, None, context)
        .unwrap();
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(
        old.project().tracks().last().unwrap().color(),
        hex_project::ElementColor::Cyan
    );
    assert_eq!(old.project(), new.project());
    assert_eq!(old.resident_frames(), new.resident_frames());
    assert_eq!(old.resident_waveform_bytes(), new.resident_waveform_bytes());
    let after = old.project().clone();
    assert!(old.undo() && new.undo());
    assert_eq!(
        old.project(),
        new.project(),
        "complete canonical undo states"
    );
    assert!(old.redo() && new.redo());
    assert_eq!(old.project(), &after);
    assert_eq!(new.project(), &after);
}

#[test]
fn unsupported_middle_and_end_and_missing_paths_refuse_whole_batch() {
    let files = Files::new();
    let good = files.wav("good.wav", 48_000, 3, 1);
    let bad = files.bad("bad.wav");
    for index in [1, 2] {
        let mut s = session();
        let before = s.project().clone();
        let mut paths = vec![good.clone(); 3];
        paths[index] = bad.clone();
        let op = capture(&s, paths);
        let control = op.control();
        let error = op.run(&mut s, None, context).unwrap_err();
        assert!(matches!(error,AudioBatchError::Member {member,..} if member==index+1));
        assert!(error.to_string().contains("audio batch refused at file"));
        unchanged(&s, &before, 0, 0);
        assert!(!s.editor.can_undo());
        assert_eq!(control.progress().phase, AudioBatchPhase::Failed);
        assert_eq!(control.progress().completed_members, index);
        assert_eq!(control.request_cancel(), AudioBatchCancel::Finished);
    }
    let mut s = session();
    let before = s.project().clone();
    assert!(matches!(
        capture(&s, vec![files.0.join("missing.wav")]).run(&mut s, None, context),
        Err(AudioBatchError::Member { .. })
    ));
    unchanged(&s, &before, 0, 0);
}

#[test]
fn cancellation_before_io_between_members_and_after_compile_never_commits() {
    let files = Files::new();
    let good = files.wav("good.wav", 48_000, 3, 1);
    for moment in [0, 1, 2] {
        let mut s = session();
        let before = s.project().clone();
        let op = capture(&s, vec![good.clone(), good.clone()]);
        let control = op.control();
        if moment == 0 {
            assert_eq!(control.request_cancel(), AudioBatchCancel::Requested);
        }
        let mut compiling_checks = 0;
        let result = op.run(&mut s, None, || {
            let p = control.progress();
            if p.phase == AudioBatchPhase::Compiling {
                compiling_checks += 1;
            }
            if (moment == 1 && p.phase == AudioBatchPhase::Prepared && p.completed_members == 1)
                || (moment == 2 && compiling_checks == 2)
            {
                assert_eq!(control.request_cancel(), AudioBatchCancel::Requested);
            }
            context()
        });
        assert!(matches!(result, Err(AudioBatchError::Canceled)));
        unchanged(&s, &before, 0, 0);
        assert!(!s.editor.can_undo());
        assert_eq!(control.progress().phase, AudioBatchPhase::Canceled);
        if moment == 2 {
            assert_eq!(compiling_checks, 2);
        }
    }
}

#[test]
fn generation_revision_gesture_and_actual_project_changes_refuse() {
    let files = Files::new();
    let good = files.wav("good.wav", 48_000, 3, 1);
    for changed in [
        AudioBatchContext {
            revision: 20,
            ..context()
        },
        AudioBatchContext {
            project_generation: 8,
            ..context()
        },
        AudioBatchContext {
            active_gesture: true,
            ..context()
        },
    ] {
        let mut s = session();
        let before = s.project().clone();
        let op = capture(&s, vec![good.clone()]);
        let control = op.control();
        let mut compiling_checks = 0;
        let result = op.run(&mut s, None, || {
            if control.progress().phase == AudioBatchPhase::Compiling {
                compiling_checks += 1;
            }
            if compiling_checks == 2 {
                changed
            } else {
                context()
            }
        });
        assert!(matches!(result, Err(AudioBatchError::ContextChanged)));
        unchanged(&s, &before, 0, 0);
        assert_eq!(compiling_checks, 2);
    }
    let mut s = session();
    let op = capture(&s, vec![good]);
    s.editor
        .execute(hex_project::Command::MasterGain { gain: 0.5 })
        .unwrap();
    let before = s.project().clone();
    assert!(matches!(
        op.run(&mut s, None, context),
        Err(AudioBatchError::ContextChanged)
    ));
    unchanged(&s, &before, 0, 0);
}

#[test]
fn aggregate_budget_counts_retained_undo_media_and_converted_lengths() {
    let files = Files::new();
    let ten = files.wav("ten.wav", 48_000, 10, 1);
    let six = files.wav("six.wav", 24_000, 3, 2);
    let mut s = Session::new(48_000, 16, 20).unwrap();
    capture(&s, vec![ten]).run(&mut s, None, context).unwrap();
    assert!(s.undo());
    let before = s.project().clone();
    let op = capture(&s, vec![six.clone(), six]);
    let control = op.control();
    assert!(matches!(
        op.run(&mut s, None, context),
        Err(AudioBatchError::Member { member: 2, .. })
    ));
    unchanged(&s, &before, 1, 10);
    assert!(s.editor.can_redo());
    assert!(!s.editor.can_undo());
    assert_eq!(control.progress().completed_members, 1);
    assert!(s.redo());
}

#[test]
fn real_compile_failure_drops_staged_batch_and_preserves_history() {
    let files = Files::new();
    let good = files.wav("good.wav", 48_000, 3, 1);
    let mut s = session();
    let track = match s
        .editor
        .execute(hex_project::Command::AddTrack {
            name: "target".into(),
        })
        .unwrap()
    {
        Created::Track(id) => id,
        _ => panic!(),
    };
    let state = hex_lattice_contract::NativeDeviceState::new(
        hex_lattice_contract::DeviceTypeId::from_static("test.unregistered.decode.batch"),
        1,
        Vec::new(),
    )
    .unwrap();
    s.editor
        .execute(hex_project::Command::InsertDevice {
            track,
            before: None,
            state,
        })
        .unwrap();
    let before = s.project().clone();
    let op = capture(&s, vec![good]);
    let control = op.control();
    assert!(matches!(
        op.run(&mut s, None, context),
        Err(AudioBatchError::Session(SessionError::Engine(_)))
    ));
    unchanged(&s, &before, 0, 0);
    assert!(s.editor.can_undo());
    assert_eq!(control.progress().phase, AudioBatchPhase::Failed);
}

#[test]
fn sparse_oversized_actual_files_enforce_file_and_source_frame_limits() {
    let files = Files::new();
    let bytes = files.0.join("bytes.wav");
    fs::File::create(&bytes)
        .unwrap()
        .set_len((crate::resident_import::IMPORT_FILE_LIMIT as u64) + 1)
        .unwrap();
    let frames = files.0.join("frames.wav");
    fs::write(&frames, header(48_000, (SOURCE_FRAMES as u32) + 1)).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&frames)
        .unwrap()
        .set_len(44 + 2 * ((SOURCE_FRAMES as u64) + 1))
        .unwrap();
    for path in [bytes, frames] {
        let mut s = session();
        let before = s.project().clone();
        assert!(matches!(
            capture(&s, vec![path]).run(&mut s, None, context),
            Err(AudioBatchError::Member {
                source: SessionError::Media(hex_media::ImportError::LimitExceeded(_)),
                ..
            })
        ));
        unchanged(&s, &before, 0, 0);
    }
}

#[test]
fn cancellation_and_commit_arbitration_have_one_winner() {
    for _ in 0..64 {
        let control = Arc::new(AudioBatchControl {
            state: AtomicU8::new(OPEN),
            progress: AtomicU64::new(0),
            total: 1,
        });
        let cancel = Arc::clone(&control);
        let thread = std::thread::spawn(move || cancel.request_cancel());
        let commit = control.claim_commit();
        let answer = thread.join().unwrap();
        match answer {
            AudioBatchCancel::Requested => assert!(commit.is_err()),
            AudioBatchCancel::TooLate => assert!(commit.is_ok()),
            _ => panic!("unexpected arbitration result"),
        }
    }
}

#[test]
fn admission_rejects_empty_oversized_and_busy_payloads_without_io() {
    let s = session();
    assert!(AudioBatchImport::capture(Vec::new(), destination(), &s, context()).is_err());
    assert!(
        AudioBatchImport::capture(
            vec![Path::new("missing.wav").to_owned(); hex_project::MAX_ASSETS + 1],
            destination(),
            &s,
            context()
        )
        .is_err()
    );
    assert!(
        AudioBatchImport::capture(
            vec![PathBuf::from("missing.wav")],
            destination(),
            &s,
            AudioBatchContext {
                active_gesture: true,
                ..context()
            }
        )
        .is_err()
    );
}

#[test]
fn real_lane_batch_and_duplicate_payload_preserve_existing_placement_contract() {
    let files = Files::new();
    let original = files.wav("original.wav", 48_000, 30, 1);
    let incoming = files.wav("incoming.wav", 24_000, 3, 2);
    let mut s = session();
    let (initial, _) = capture(&s, vec![original])
        .run(&mut s, None, context)
        .unwrap();
    let track = match initial[0] {
        Created::Imported { track, .. } => track,
        _ => panic!(),
    };
    let before = s.project().clone();
    let op = AudioBatchImport::capture(
        vec![incoming.clone(), incoming],
        AudioBatchDestination::Track { track, start: 10 },
        &s,
        context(),
    )
    .unwrap();
    let (created, _) = op.run(&mut s, None, context).unwrap();
    assert_ne!(asset(created[0]), asset(created[1]));
    let intervals: Vec<_> = s
        .project()
        .track(track)
        .unwrap()
        .clips()
        .iter()
        .map(|c| (c.start(), c.duration()))
        .collect();
    assert!(intervals.contains(&(10, 6)));
    assert!(intervals.contains(&(16, 6)));
    assert!(intervals.contains(&(8, 2)));
    assert!(intervals.contains(&(22, 16)));
    assert!(s.undo());
    assert_eq!(
        s.project().track(track).unwrap().clips(),
        before.track(track).unwrap().clips()
    );
    assert!(s.redo());
}

#[test]
fn cancel_request_is_observed_after_decode_and_conversion_boundaries() {
    let files = Files::new();
    let path = files.wav("good.wav", 24_000, 6, 1);
    for target in [AudioBatchPhase::Decoding, AudioBatchPhase::Converting] {
        let mut s = session();
        let before = s.project().clone();
        let op = capture(&s, vec![path.clone()]);
        let control = op.control();
        let mut seen = 0;
        let result = op.run(&mut s, None, || {
            if control.progress().phase == target {
                seen += 1;
                if seen == 2 {
                    assert_eq!(control.request_cancel(), AudioBatchCancel::Requested);
                }
            }
            context()
        });
        assert_eq!(seen, 2);
        assert!(matches!(result, Err(AudioBatchError::Canceled)));
        unchanged(&s, &before, 0, 0);
        assert_eq!(control.progress().phase, AudioBatchPhase::Canceled);
    }
}

mod console;
