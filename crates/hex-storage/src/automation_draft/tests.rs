use super::*;
use hex_project::{Command, Created, Editor};
use std::sync::atomic::{AtomicU64, Ordering};

#[path = "completed_tests.rs"]
mod completed_tests;

static NEXT_TEST: AtomicU64 = AtomicU64::new(0);

#[test]
fn journal_record_hash_matches_legacy_digest() {
    // SHA-256 of a zero chain followed by bytes 0 through 63, generated with
    // the previous 0.10 backend. Existing journal chains must remain readable.
    let mut record = [0; RECORD_BYTES];
    for (index, byte) in record[..PAYLOAD_BYTES].iter_mut().enumerate() {
        *byte = index as u8;
    }
    let sealed = seal(record, [0; 32]);
    assert_eq!(
        &sealed[PAYLOAD_BYTES..],
        &[
            0xdc, 0x7a, 0x48, 0x01, 0x4f, 0xc1, 0xfa, 0xc8, 0xb5, 0x2a, 0xf3, 0x9b, 0xc7, 0xea,
            0x5c, 0xaf, 0xaf, 0xab, 0xf8, 0xbb, 0x81, 0xfb, 0x8f, 0x88, 0x0f, 0xdf, 0x3b, 0x4a,
            0x45, 0x66, 0x79, 0x5c,
        ]
    );
    assert!(valid_hash(&sealed, [0; 32]));
}
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
// Tests that have dropped their writer wait for the actual shared lock before
// opening. A retained descriptor still fails after the bound; production reader
// admission remains nonblocking and its contention tests use it directly.
#[cfg(unix)]
pub(super) fn wait_for_shared_lock(
    path: &Path,
    bound: std::time::Duration,
) -> std::io::Result<File> {
    let file = File::open(path)?;
    let start = std::time::Instant::now();
    loop {
        match fs2::FileExt::try_lock_shared(&file) {
            Ok(()) => return Ok(file),
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock && start.elapsed() < bound =>
            {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Err(error) => return Err(error),
        }
    }
}
fn open(path: &Path) -> DraftReader {
    #[cfg(unix)]
    let _released = wait_for_shared_lock(path, std::time::Duration::from_millis(250)).unwrap();
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
        // This test exercises large-draft paging, not per-batch crash durability.
        // The real finish below syncs the entire file; the separate sync-failure
        // and interrupted-process tests cover the append durability boundary.
        writer
            .append_with(
                &batch[..count],
                |file, bytes| file.write_all(bytes),
                |_| Ok(()),
            )
            .unwrap();
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
    for (offset, value) in [(8, 3), (32, b'x'), (16, 255)] {
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
    let Created::NativeSource(source_id) = editor
        .execute(Command::CreateNativeSource {
            name: "retained instrument".into(),
            devices: vec![hex_project::default_instrument_state().unwrap()],
            instrument_index: 0,
        })
        .unwrap()
    else {
        panic!("native source")
    };
    let (base, _, provenance) = fixture();
    let source = editor.project();
    let device = source.native_source(source_id).unwrap().instrument().id();
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
    let offset = reader.records.records_start + RECORD_BYTES as u64 + 40;
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

#[test]
fn independent_little_endian_record_contract_keeps_version_one_wire_stable() {
    let chain = [0x31; 32];
    let application = AppliedEvent {
        sequence: 7,
        performance_frame: 1024,
        content_frame: 17,
        loop_pass: 2,
        value: AutomationValue::new(0.25).unwrap(),
    };
    let mut payload = [0u8; 64];
    payload[0] = 1; // event kind, little-endian u64
    payload[8] = 7;
    payload[17] = 4; // 1024
    payload[24] = 17;
    payload[32] = 2;
    payload[42] = 0x80;
    payload[43] = 0x3e; // canonical f32 0.25 in the low half of a u64
    let record = event_record(application, chain);
    assert_eq!(&record[..64], &payload);
    let mut independent = Sha256::new();
    independent.update(chain);
    independent.update(payload);
    let expected: [u8; 32] = independent.finalize().into();
    assert_eq!(&record[64..], &expected);
    let clear = terminal_record(
        DraftTerminal::Finished(DraftClock {
            performance_frame: 1024,
            content_frame: 17,
            loop_pass: 2,
        }),
        chain,
    );
    payload[0] = 2;
    payload[8] = 0;
    payload[42] = 0;
    payload[43] = 0;
    assert_eq!(
        &clear[..64],
        &payload,
        "finished has a real clock and no invented value or sequence"
    );
    let aborted = terminal_record(
        DraftTerminal::Aborted(DraftAbortReason::ClockExhausted),
        chain,
    );
    let mut aborted_payload = [0u8; 64];
    aborted_payload[0] = 3;
    aborted_payload[48] = 7;
    assert_eq!(
        &aborted[..64],
        &aborted_payload,
        "an abort never carries an end clock"
    );
}

#[test]
#[ignore = "manual optimized profile of durable event I/O; no native callback or deadline claim"]
fn profile_durable_automation_io() {
    use std::time::Instant;
    const WARM: usize = 2048;
    const EVENTS: usize = 65536;
    for trial in 0..3 {
        let dir = TestDir::new();
        let path = dir.path();
        let (source, target, stamp) = fixture();
        let mut writer = DraftWriter::create_new(&path, &source, target, stamp).unwrap();
        let make = |index: usize| AppliedEvent {
            sequence: index as u64 + 1,
            performance_frame: index as u64 * 256,
            content_frame: index as u64 * 256,
            loop_pass: 0,
            value: AutomationValue::new((index % 129) as f32 / 128.0).unwrap(),
        };
        let mut batch = [make(0); MAX_APPEND_EVENTS];
        for start in (0..WARM).step_by(MAX_APPEND_EVENTS) {
            for (offset, slot) in batch.iter_mut().enumerate() {
                *slot = make(start + offset);
            }
            writer.append(&batch).unwrap();
        }
        let start = Instant::now();
        for first in (WARM..WARM + EVENTS).step_by(MAX_APPEND_EVENTS) {
            for (offset, slot) in batch.iter_mut().enumerate() {
                *slot = make(first + offset);
            }
            writer.append(&batch).unwrap();
        }
        writer
            .finish(DraftTerminal::Finished(DraftClock {
                performance_frame: ((WARM + EVENTS) * 256) as u64,
                content_frame: ((WARM + EVENTS) * 256) as u64,
                loop_pass: 0,
            }))
            .unwrap();
        let write_seconds = start.elapsed().as_secs_f64();
        drop(writer);
        let start = Instant::now();
        let mut reader = open(&path);
        let open_seconds = start.elapsed().as_secs_f64();
        assert_eq!(reader.progress().durable_events, (WARM + EVENTS) as u64);
        let start = Instant::now();
        let mut page = [None; MAX_APPEND_EVENTS];
        let mut at = 0;
        loop {
            let count = reader.read_events(at, &mut page).unwrap();
            if count == 0 {
                break;
            }
            at += count as u64;
        }
        let read_seconds = start.elapsed().as_secs_f64();
        assert_eq!(at, (WARM + EVENTS) as u64);
        println!(
            "automation_draft_io trial={trial} synthetic_clock_rate=48000 synthetic_frame_stride=256 warmup_events={WARM} measured_events={EVENTS} batch_events={MAX_APPEND_EVENTS} record_bytes={RECORD_BYTES} write_sync_seconds={write_seconds:.6} events_per_second={:.3} validated_open_seconds={open_seconds:.6} ordinal_read_seconds={read_seconds:.6} file_bytes={}",
            EVENTS as f64 / write_seconds,
            fs::metadata(&path).unwrap().len()
        );
    }
}

#[test]
#[ignore = "child-process helper for abrupt automation writer restart"]
fn automation_draft_crash_child() {
    let path = std::env::var_os("HEX_AUTOMATION_DRAFT_CRASH_PATH").expect("child draft path");
    let (source, target, stamp) = fixture();
    let mut writer = DraftWriter::create_new(PathBuf::from(path), &source, target, stamp).unwrap();
    let batch = std::array::from_fn::<_, MAX_APPEND_EVENTS, _>(|index| event(index as u64));
    let progress = writer.append(&batch).unwrap();
    assert_eq!(progress.durable_events, MAX_APPEND_EVENTS as u64);
    // No Rust destructors run: the OS must release the writer lock and preserve the synced prefix.
    std::process::exit(23);
}

#[test]
fn actual_process_exit_releases_writer_lock_and_recovers_the_exact_durable_prefix() {
    let dir = TestDir::new();
    let path = dir.path();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "automation_draft::tests::automation_draft_crash_child",
            "--nocapture",
        ])
        .env("HEX_AUTOMATION_DRAFT_CRASH_PATH", &path)
        .status()
        .unwrap();
    assert_eq!(
        status.code(),
        Some(23),
        "zero matching helpers or a failed child is not restart evidence"
    );
    let original = fs::read(&path).unwrap();
    let mut recovered = open(&path);
    assert_eq!(recovered.outcome(), DraftOutcome::Interrupted);
    assert_eq!(
        recovered.progress().durable_events,
        MAX_APPEND_EVENTS as u64
    );
    assert_eq!(recovered.progress().durable_terminal, None);
    let mut page = [None; MAX_APPEND_EVENTS];
    assert_eq!(
        recovered.read_events(0, &mut page).unwrap(),
        MAX_APPEND_EVENTS
    );
    for (index, actual) in page.iter().enumerate() {
        assert_eq!(*actual, Some(event(index as u64)));
    }
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[cfg(unix)]
#[test]
fn test_wait_observes_actual_shared_lock_and_does_not_hide_a_retained_owner() {
    let dir = TestDir::new();
    let path = dir.path();
    let (source, target, provenance) = fixture();
    let writer = DraftWriter::create_new(&path, &source, target, provenance).unwrap();
    let retained = writer.file.try_clone().unwrap();
    drop(writer);
    let error = wait_for_shared_lock(&path, std::time::Duration::from_millis(20)).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
    let release = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(10));
        drop(retained);
    });
    let reader = open(&path);
    assert_eq!(reader.outcome(), DraftOutcome::Interrupted);
    release.join().unwrap();
}
