use super::*;
use hex_project::{Command, Created, Editor};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEST: AtomicU64 = AtomicU64::new(0);
struct TestDir(PathBuf);
impl TestDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hex-automation-draft-{}-{}",
            std::process::id(),
            NEXT_TEST.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> PathBuf {
        self.0.join("capture.hex-automation-draft")
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn fixture() -> (Project, DraftTarget, DraftProvenance) {
    let mut editor = Editor::new(Project::new(44_100).unwrap(), 4).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "capture".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    (
        editor.project().clone(),
        DraftTarget::TrackGain { track },
        DraftProvenance {
            sample_rate: 48_000,
            output_identity: 1,
            project_generation: 0,
            editor_revision: 1,
            audio_revision: 1,
            transport_epoch: 1,
            gesture_serial: 1,
        },
    )
}
fn event(ordinal: u64) -> AppliedEvent {
    AppliedEvent {
        sequence: ordinal * 2 + 1,
        performance_frame: ordinal * 17,
        content_frame: ordinal % 997 * 17,
        loop_pass: ordinal / 997,
        value: AutomationValue::new((ordinal % 1025) as f32 / 1024.0).unwrap(),
    }
}
fn limits() -> DraftReadLimits {
    DraftReadLimits {
        max_source_bytes: 16 * 1024 * 1024,
    }
}
fn open(path: &Path) -> DraftReader {
    DraftReader::open(path, limits()).unwrap()
}

#[test]
fn duration_independent_pages_preserve_more_than_65536_exact_applications() {
    let dir = TestDir::new();
    let path = dir.path();
    let (source, target, provenance) = fixture();
    let mut writer = DraftWriter::create_new(&path, &source, target, provenance).unwrap();
    // All event storage is these fixed arrays. More than the authored model's
    // 65,536 point bound is retained without simplifying or capping the draft.
    const COUNT: u64 = 65_731;
    let mut batch = [event(0); MAX_APPEND_EVENTS];
    for first in (0..COUNT).step_by(MAX_APPEND_EVENTS) {
        let count = (COUNT - first).min(MAX_APPEND_EVENTS as u64) as usize;
        for (offset, slot) in batch[..count].iter_mut().enumerate() {
            *slot = event(first + offset as u64);
        }
        writer.append(&batch[..count]).unwrap();
    }
    let end = clock(event(COUNT));
    assert_eq!(
        writer
            .finish(DraftTerminal::Finished(end))
            .unwrap()
            .durable_events,
        COUNT
    );
    assert!(writer.append(&[event(COUNT)]).is_err());
    drop(writer);
    let mut reader = open(&path);
    assert_eq!(reader.source(), &source);
    assert!(
        reader
            .source_matches(&decode(&encode(&source).unwrap()).unwrap())
            .unwrap()
    );
    assert_eq!(reader.target(), target);
    assert_eq!(reader.provenance(), provenance);
    assert_eq!(reader.outcome(), DraftOutcome::Finished(end));
    let mut page = [None; 113];
    for first in (0..COUNT).step_by(page.len()) {
        let count = reader.read_events(first, &mut page).unwrap();
        for (offset, actual) in page[..count].iter().enumerate() {
            assert_eq!(*actual, Some(event(first + offset as u64)));
        }
        assert!(page[count..].iter().all(Option::is_none));
    }
    assert_eq!(reader.read_events(COUNT, &mut page).unwrap(), 0);
    assert!(page.iter().all(Option::is_none));
    assert!(reader.read_events(COUNT + 1, &mut page).is_err());
    assert!(path.exists(), "owner drops never delete durable work");
}

#[test]
fn terminals_distinguish_finished_aborted_explicit_quiescence_and_interrupted_eof() {
    let (source, target, provenance) = fixture();
    let cases = [
        (
            Some(DraftTerminal::Finished(clock(event(3)))),
            DraftOutcome::Finished(clock(event(3))),
        ),
        (
            Some(DraftTerminal::Aborted(DraftAbortReason::OutputFailed)),
            DraftOutcome::Aborted(DraftAbortReason::OutputFailed),
        ),
        (Some(DraftTerminal::Interrupted), DraftOutcome::Interrupted),
        (None, DraftOutcome::Interrupted),
    ];
    for (terminal, outcome) in cases {
        let dir = TestDir::new();
        let path = dir.path();
        let mut writer = DraftWriter::create_new(&path, &source, target, provenance).unwrap();
        writer.append(&[event(0), event(1)]).unwrap();
        if let Some(terminal) = terminal {
            writer.finish(terminal).unwrap();
        }
        drop(writer);
        let reader = open(&path);
        assert_eq!(reader.outcome(), outcome);
        assert_eq!(reader.progress().durable_terminal, terminal);
        assert_eq!(reader.progress().durable_events, 2);
    }
}

#[test]
fn append_validation_is_atomic_and_terminal_clocks_use_actual_order() {
    let dir = TestDir::new();
    let path = dir.path();
    let (source, target, provenance) = fixture();
    let mut writer = DraftWriter::create_new(&path, &source, target, provenance).unwrap();
    let initial_bytes = fs::read(&path).unwrap();
    let mut invalid = event(1);
    invalid.sequence = event(0).sequence;
    assert!(writer.append(&[event(0), invalid]).is_err());
    assert_eq!(fs::read(&path).unwrap(), initial_bytes);
    assert_eq!(writer.progress(), DraftProgress::default());
    assert!(writer.append(&[event(0); 65]).is_err());
    writer.append(&[event(0), event(1)]).unwrap();
    assert!(
        writer
            .finish(DraftTerminal::Finished(clock(event(0))))
            .is_err()
    );
    let mut backwards = event(2);
    backwards.content_frame = 0;
    assert!(writer.append(&[backwards]).is_err());
    backwards.loop_pass = 1;
    writer.append(&[backwards]).unwrap();
    writer
        .finish(DraftTerminal::Aborted(DraftAbortReason::TransportChanged))
        .unwrap();
    drop(writer);
    assert_eq!(open(&path).progress().durable_events, 3);
}

#[test]
fn partial_write_and_sync_failure_are_sticky_and_keep_recoverable_prefix() {
    let (source, target, provenance) = fixture();
    for fail_sync in [false, true] {
        let dir = TestDir::new();
        let path = dir.path();
        let mut writer = DraftWriter::create_new(&path, &source, target, provenance).unwrap();
        writer.append(&[event(0)]).unwrap();
        let error = if fail_sync {
            writer
                .append_with(
                    &[event(1)],
                    |file, bytes| file.write_all(bytes),
                    |_| Err(io::Error::other("injected sync failure")),
                )
                .unwrap_err()
        } else {
            writer
                .append_with(
                    &[event(1)],
                    |file, bytes| {
                        file.write_all(&bytes[..37])?;
                        Err(io::Error::other("injected partial write"))
                    },
                    File::sync_all,
                )
                .unwrap_err()
        };
        assert_eq!(error.progress.accepted_events, 2);
        assert_eq!(error.progress.durable_events, 1);
        assert!(writer.append(&[event(2)]).is_err());
        assert!(writer.finish(DraftTerminal::Interrupted).is_err());
        let failed_bytes = fs::read(&path).unwrap();
        drop(writer);
        let mut reader = open(&path);
        assert_eq!(
            reader.progress().durable_events,
            if fail_sync { 2 } else { 1 }
        );
        assert_eq!(
            reader.outcome(),
            if fail_sync {
                DraftOutcome::Interrupted
            } else {
                DraftOutcome::Damaged {
                    reason: DraftDamage::TruncatedRecord,
                }
            }
        );
        let mut page = [None; 3];
        assert_eq!(
            reader.read_events(0, &mut page).unwrap(),
            if fail_sync { 2 } else { 1 }
        );
        assert_eq!(page[0], Some(event(0)));
        assert_eq!(fs::read(&path).unwrap(), failed_bytes);
    }
}

#[test]
fn terminal_sync_failure_does_not_acknowledge_completion() {
    let dir = TestDir::new();
    let path = dir.path();
    let (source, target, provenance) = fixture();
    let mut writer = DraftWriter::create_new(&path, &source, target, provenance).unwrap();
    writer.append(&[event(0)]).unwrap();
    let terminal = DraftTerminal::Finished(clock(event(1)));
    let error = writer
        .finish_with(
            terminal,
            |f, b| f.write_all(b),
            |_| Err(io::Error::other("injected sync")),
        )
        .unwrap_err();
    assert_eq!(error.progress.durable_terminal, None);
    assert!(writer.finish(terminal).is_err());
    drop(writer);
    assert_eq!(
        open(&path).outcome(),
        DraftOutcome::Finished(clock(event(1)))
    );
}

#[test]
fn source_header_budget_future_version_and_integrity_refuse_without_rewriting() {
    let dir = TestDir::new();
    let path = dir.path();
    let (source, target, provenance) = fixture();
    drop(DraftWriter::create_new(&path, &source, target, provenance).unwrap());
    let original = fs::read(&path).unwrap();
    assert!(
        DraftReader::open(
            &path,
            DraftReadLimits {
                max_source_bytes: 1
            }
        )
        .is_err()
    );
    for (offset, value) in [(8, 2), (32, b'x'), (16, 255)] {
        let mut damaged = original.clone();
        damaged[offset] = value;
        fs::write(&path, &damaged).unwrap();
        assert!(DraftReader::open(&path, limits()).is_err());
        assert_eq!(fs::read(&path).unwrap(), damaged);
    }
    for length in [0, 31, original.len() - 1] {
        fs::write(&path, &original[..length]).unwrap();
        assert!(DraftReader::open(&path, limits()).is_err());
        assert_eq!(fs::read(&path).unwrap(), original[..length]);
    }
}

#[test]
fn corrupt_torn_reordered_and_trailing_records_expose_only_valid_prefix() {
    let dir = TestDir::new();
    let path = dir.path();
    let (source, target, provenance) = fixture();
    let mut writer = DraftWriter::create_new(&path, &source, target, provenance).unwrap();
    writer.append(&[event(0), event(1), event(2)]).unwrap();
    writer
        .finish(DraftTerminal::Finished(clock(event(3))))
        .unwrap();
    drop(writer);
    let original = fs::read(&path).unwrap();
    let start = original.len() - RECORD_BYTES * 4;
    let cases = [
        (start + RECORD_BYTES + 33, 1, DraftDamage::TruncatedRecord),
        (
            start + RECORD_BYTES * 3 + 70,
            3,
            DraftDamage::TruncatedRecord,
        ),
    ];
    for (length, count, reason) in cases {
        fs::write(&path, &original[..length]).unwrap();
        let mut reader = open(&path);
        assert_eq!(reader.progress().durable_events, count);
        assert_eq!(reader.outcome(), DraftOutcome::Damaged { reason });
        let mut page = [None; 4];
        assert_eq!(reader.read_events(0, &mut page).unwrap(), count as usize);
        assert_eq!(fs::read(&path).unwrap(), original[..length]);
        drop(reader);
    }
    let mut damaged = original.clone();
    damaged[start + RECORD_BYTES + 20] ^= 1;
    fs::write(&path, &damaged).unwrap();
    assert_eq!(
        open(&path).outcome(),
        DraftOutcome::Damaged {
            reason: DraftDamage::Integrity
        }
    );
    assert_eq!(open(&path).progress().durable_events, 1);
    // A well-hashed but semantically invalid record is also rejected.
    let mut seed = [0; 32];
    seed.copy_from_slice(&original[start + PAYLOAD_BYTES..start + RECORD_BYTES]);
    damaged = original.clone();
    let invalid = event_record(event(0), seed);
    damaged[start + RECORD_BYTES..start + RECORD_BYTES * 2].copy_from_slice(&invalid);
    fs::write(&path, &damaged).unwrap();
    assert_eq!(
        open(&path).outcome(),
        DraftOutcome::Damaged {
            reason: DraftDamage::InvalidRecord
        }
    );
    let mut trailing = original.clone();
    trailing.push(0);
    fs::write(&path, &trailing).unwrap();
    assert_eq!(
        open(&path).outcome(),
        DraftOutcome::Damaged {
            reason: DraftDamage::TrailingData
        }
    );
    assert_eq!(fs::read(&path).unwrap(), trailing);
}

#[test]
fn locking_collision_publication_failure_and_drop_preserve_original_files() {
    let dir = TestDir::new();
    let path = dir.path();
    let (source, target, provenance) = fixture();
    let writer = DraftWriter::create_new(&path, &source, target, provenance).unwrap();
    assert!(
        DraftReader::open(&path, limits()).is_err(),
        "exclusive writer excludes shared readers"
    );
    let original = fs::read(&path).unwrap();
    assert!(DraftWriter::create_new(&path, &source, target, provenance).is_err());
    assert_eq!(fs::read(&path).unwrap(), original);
    drop(writer);
    let reader = open(&path);
    let second = open(&path);
    let file = OpenOptions::new().write(true).open(&path).unwrap();
    assert!(fs2::FileExt::try_lock_exclusive(&file).is_err());
    drop(reader);
    drop(second);
    fs2::FileExt::try_lock_exclusive(&file).unwrap();
    drop(file);
    assert_eq!(fs::read(&path).unwrap(), original);
    let published = dir.0.join("postpublication");
    let error = DraftWriter::create_with_finish(&published, &source, target, provenance, |_, _| {
        Err((
            "synchronize draft directory",
            io::Error::other("injected publication sync"),
        ))
    })
    .err()
    .unwrap();
    assert!(error.published);
    assert_eq!(open(&published).source(), &source);
    assert_eq!(
        fs::read_dir(&dir.0).unwrap().count(),
        2,
        "existing helper cleans only unpublished temporary custody"
    );
    #[cfg(unix)]
    {
        let link = dir.0.join("link");
        std::os::unix::fs::symlink("missing", &link).unwrap();
        assert!(DraftWriter::create_new(&link, &source, target, provenance).is_err());
        assert_eq!(fs::read_link(&link).unwrap(), Path::new("missing"));
        assert!(DraftReader::open(&link, limits()).is_err());
    }
}

#[test]
fn device_targets_resolve_from_complete_source_inventory_and_source_changes_do_not_match() {
    let dir = TestDir::new();
    let path = dir.path();
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
    editor
        .execute(Command::AddMidiTrack {
            name: "instrument".into(),
        })
        .unwrap();
    let (base, _, provenance) = fixture();
    let source = editor.project();
    let device = source.native_sources()[0].instrument().id();
    let target = DraftTarget::Device {
        device,
        target: DeviceAutomationTarget::Parameter(hex_lattice_contract::ParameterId::from_static(
            1,
        )),
    };
    drop(DraftWriter::create_new(&path, source, target, provenance).unwrap());
    let reader = open(&path);
    assert_eq!(reader.target(), target);
    assert!(reader.source_matches(source).unwrap());
    assert!(!reader.source_matches(&base).unwrap());
    assert!(DraftWriter::create_new(dir.0.join("absent"), &base, target, provenance).is_err());
}

#[test]
fn source_media_paths_require_explicit_absolute_normalization() {
    let dir = TestDir::new();
    let (source, target, provenance) = fixture();
    let mut snapshot = source.snapshot();
    snapshot.assets.push(hex_project::AssetSnapshot {
        original_source: None,
        id: snapshot.next_id,
        path: PathBuf::from("MixedCase.wav"),
        source_sample_rate: source.sample_rate(),
        source_frames: 100,
        playback_frames: 100,
        conversion_version: hex_project::IDENTITY_CONVERSION,
    });
    snapshot.next_id += 1;
    let relative = Project::from_snapshot(snapshot.clone()).unwrap();
    assert!(DraftWriter::create_new(dir.path(), &relative, target, provenance).is_err());
    assert!(!dir.path().exists());
    snapshot.assets[0].path = dir.0.join("MixedCase.wav");
    let absolute = Project::from_snapshot(snapshot).unwrap();
    drop(DraftWriter::create_new(dir.path(), &absolute, target, provenance).unwrap());
    assert_eq!(open(&dir.path()).source(), &absolute);
    assert_eq!(
        relative.assets().next().unwrap().path(),
        Path::new("MixedCase.wav")
    );
}

#[test]
fn well_hashed_nonfinite_unknown_and_noncanonical_records_are_visible_damage() {
    let dir = TestDir::new();
    let path = dir.path();
    let (source, target, provenance) = fixture();
    let mut writer = DraftWriter::create_new(&path, &source, target, provenance).unwrap();
    writer.append(&[event(0)]).unwrap();
    drop(writer);
    let original = fs::read(&path).unwrap();
    let start = original.len() - RECORD_BYTES;
    let mut seed = [0; 32];
    seed.copy_from_slice(&original[start - 32..start]);
    // Hashes alone do not establish typed validity, current record kinds or
    // canonical normalized values (negative zero must not enter saved state).
    for (field, value) in [
        (0, 99),
        (1, 0),
        (5, u64::from(f32::NAN.to_bits())),
        (5, u64::from(f32::INFINITY.to_bits())),
        (5, u64::from((-0.0_f32).to_bits())),
        (5, u64::from(1.1_f32.to_bits())),
        (7, 1),
    ] {
        let mut record = event_record(event(0), seed);
        put(&mut record, field, value);
        let record = seal(record, seed);
        let mut damaged = original[..start].to_vec();
        damaged.extend_from_slice(&record);
        fs::write(&path, &damaged).unwrap();
        let reader = open(&path);
        assert_eq!(
            reader.outcome(),
            DraftOutcome::Damaged {
                reason: DraftDamage::InvalidRecord
            }
        );
        assert_eq!(reader.progress().durable_events, 0);
        assert_eq!(fs::read(&path).unwrap(), damaged);
    }
}

#[test]
fn all_native_abort_reasons_round_trip_without_end_clocks() {
    let (source, target, provenance) = fixture();
    for reason in [
        DraftAbortReason::Cancelled,
        DraftAbortReason::SourceChanged,
        DraftAbortReason::TargetUnavailable,
        DraftAbortReason::TransportChanged,
        DraftAbortReason::OutputFailed,
        DraftAbortReason::UnsupportedOutput,
        DraftAbortReason::ClockExhausted,
    ] {
        let dir = TestDir::new();
        let path = dir.path();
        let mut writer = DraftWriter::create_new(&path, &source, target, provenance).unwrap();
        writer.finish(DraftTerminal::Aborted(reason)).unwrap();
        drop(writer);
        let reader = open(&path);
        assert_eq!(reader.outcome(), DraftOutcome::Aborted(reason));
        assert_eq!(reader.progress().durable_events, 0);
    }
}

#[test]
fn page_reads_recheck_integrity_after_open_and_refuse_partial_output() {
    let dir = TestDir::new();
    let path = dir.path();
    let (source, target, stamp) = fixture();
    let mut writer = DraftWriter::create_new(&path, &source, target, stamp).unwrap();
    writer.append(&[event(0), event(1)]).unwrap();
    writer
        .finish(DraftTerminal::Finished(clock(event(2))))
        .unwrap();
    drop(writer);
    let mut reader = open(&path);
    // Simulate a noncooperating external write after the initial full-chain validation.
    // The changed value is still well-typed; only integrity can distinguish it.
    let offset = reader.records_start + RECORD_BYTES as u64 + 40;
    let mut external = OpenOptions::new().write(true).open(&path).unwrap();
    external.seek(SeekFrom::Start(offset)).unwrap();
    external
        .write_all(&u64::from(0.75_f32.to_bits()).to_le_bytes())
        .unwrap();
    external.sync_all().unwrap();
    let damaged = fs::read(&path).unwrap();
    let mut page = [None; 2];
    assert!(
        reader.read_events(0, &mut page).is_err(),
        "a reader must not expose data changed after validation"
    );
    assert_eq!(page, [None; 2]);
    assert_eq!(
        fs::read(&path).unwrap(),
        damaged,
        "read failure preserves original bytes"
    );
}
