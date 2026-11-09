use super::*;

fn capture_fixture() -> (Project, DraftTarget, DraftProvenance, CaptureClock) {
    let (source, target, mut provenance) = fixture();
    provenance.sample_rate = source.sample_rate();
    let clock = CaptureClock {
        sample_rate: source.sample_rate(),
        loop_region: None,
    };
    (source, target, provenance, clock)
}

/// Reframe a header-only test file, keeping integrity valid so metadata validation is exercised.
pub(super) fn changed_metadata(
    original: &[u8],
    version: u32,
    change: impl FnOnce(&mut serde_json::Value),
) -> Vec<u8> {
    let source_end = PREFIX_BYTES + u64_at(original, 16) as usize;
    let meta_end = source_end + u64_at(original, 24) as usize;
    assert_eq!(meta_end + 32, original.len());
    let mut metadata: serde_json::Value =
        serde_json::from_slice(&original[source_end..meta_end]).unwrap();
    change(&mut metadata);
    let metadata = serde_json::to_vec(&metadata).unwrap();
    let mut bytes = original[..source_end].to_vec();
    bytes[8..12].copy_from_slice(&version.to_le_bytes());
    bytes[24..32].copy_from_slice(&(metadata.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&metadata);
    let chain: [u8; 32] = Sha256::digest(&bytes).into();
    bytes.extend_from_slice(&chain);
    bytes
}

#[test]
fn legacy_header_stays_version_one_and_does_not_invent_capture_clock() {
    let dir = TestDir::new();
    let path = dir.path();
    let (source, target, provenance) = fixture();
    drop(DraftWriter::create_new(&path, &source, target, provenance).unwrap());
    let original = fs::read(&path).unwrap();
    assert_eq!(u32_at(&original, 8), 1);
    let source_end = PREFIX_BYTES + u64_at(&original, 16) as usize;
    let meta_end = source_end + u64_at(&original, 24) as usize;
    let DraftTarget::TrackGain { track } = target else {
        panic!("gain fixture")
    };
    let expected = format!(
        "{{\"target_kind\":0,\"owner\":{},\"parameter\":0,\"provenance\":{}}}",
        track.value(),
        serde_json::to_string(&provenance).unwrap()
    );
    assert_eq!(&original[source_end..meta_end], expected.as_bytes());
    let reader = open(&path);
    assert_eq!(reader.capture_clock(), None);
    assert_eq!(reader.provenance(), provenance);
    assert_ne!(provenance.sample_rate, source.sample_rate());
    drop(reader);
    assert_eq!(fs::read(&path).unwrap(), original);

    let extended = changed_metadata(&original, 1, |metadata| {
        metadata["capture_clock"] = serde_json::json!({
            "sample_rate": source.sample_rate(),
            "loop_region": { "kind": "linear" }
        });
    });
    fs::write(&path, &extended).unwrap();
    assert!(DraftReader::open(&path, limits()).is_err());
    assert_eq!(fs::read(&path).unwrap(), extended);
}

#[test]
fn clocked_headers_round_trip_explicit_linear_and_transient_loop_contexts() {
    let (source, target, provenance, mut capture) = capture_fixture();
    for region in [
        None,
        Some(hex_project::LoopRegion::new(64, 128, true).unwrap()),
    ] {
        let dir = TestDir::new();
        let path = dir.path();
        capture.loop_region = region;
        let mut writer =
            DraftWriter::create_with_clock(&path, &source, target, provenance, capture).unwrap();
        writer.append(&[event(0), event(1)]).unwrap();
        let end = clock(event(2));
        writer.finish(DraftTerminal::Finished(end)).unwrap();
        drop(writer);
        let original = fs::read(&path).unwrap();
        assert_eq!(u32_at(&original, 8), 2);
        assert_eq!(source.loop_region(), None);
        let mut completed = CompletedDraft::open(&path, limits()).unwrap();
        assert_eq!(completed.source(), &source);
        assert_eq!(completed.target(), target);
        assert_eq!(completed.provenance(), provenance);
        assert_eq!(completed.capture_clock(), Some(capture));
        assert_eq!(completed.end_clock(), end);
        assert_eq!(completed.event_count(), 2);
        assert_eq!(completed.path(), path);
        let mut page = [None; 3];
        assert_eq!(completed.read_events(0, &mut page).unwrap(), 2);
        assert_eq!(page, [Some(event(0)), Some(event(1)), None]);
        drop(completed);
        assert_eq!(fs::read(&path).unwrap(), original);
    }
}

#[test]
fn clocked_creation_refuses_rate_mismatch_and_disabled_loop_before_publication() {
    let (source, target, provenance, capture) = capture_fixture();
    let cases = [
        (
            DraftProvenance {
                sample_rate: 48_000,
                ..provenance
            },
            capture,
        ),
        (
            provenance,
            CaptureClock {
                sample_rate: 48_000,
                ..capture
            },
        ),
        (
            provenance,
            CaptureClock {
                sample_rate: 0,
                ..capture
            },
        ),
        (
            provenance,
            CaptureClock {
                loop_region: Some(hex_project::LoopRegion::new(1, 2, false).unwrap()),
                ..capture
            },
        ),
    ];
    for (provenance, capture) in cases {
        let dir = TestDir::new();
        let path = dir.path();
        let error = DraftWriter::create_with_clock(&path, &source, target, provenance, capture)
            .err()
            .unwrap();
        assert!(!error.published);
        assert_eq!(error.progress, DraftProgress::default());
        assert!(!path.exists());
    }
}

#[test]
fn clocked_headers_require_strict_explicit_and_well_formed_clock_metadata() {
    let dir = TestDir::new();
    let path = dir.path();
    let (source, target, provenance, capture) = capture_fixture();
    drop(DraftWriter::create_with_clock(&path, &source, target, provenance, capture).unwrap());
    let original = fs::read(&path).unwrap();
    let changes: &[fn(&mut serde_json::Value)] = &[
        |v| {
            v.as_object_mut().unwrap().remove("capture_clock");
        },
        |v| v["capture_clock"] = serde_json::Value::Null,
        |v| {
            v["capture_clock"]
                .as_object_mut()
                .unwrap()
                .remove("loop_region");
        },
        |v| v["capture_clock"]["loop_region"] = serde_json::Value::Null,
        |v| {
            v["capture_clock"]["loop_region"] =
                serde_json::json!({ "kind": "region", "start": 9, "end": 9 })
        },
        |v| {
            v["capture_clock"]["loop_region"] =
                serde_json::json!({ "kind": "region", "start": 10, "end": 9 })
        },
        |v| v["capture_clock"]["loop_region"]["unexpected"] = serde_json::json!(0),
        |v| v["capture_clock"]["sample_rate"] = serde_json::json!(0),
        |v| v["capture_clock"]["sample_rate"] = serde_json::json!(48_000),
        |v| v["capture_clock"]["unexpected"] = serde_json::json!(0),
        |v| v["metadata"]["unexpected"] = serde_json::json!(0),
        |v| v["unexpected"] = serde_json::json!(0),
    ];
    for change in changes {
        let malformed = changed_metadata(&original, 2, change);
        fs::write(&path, &malformed).unwrap();
        assert!(DraftReader::open(&path, limits()).is_err());
        assert_eq!(fs::read(&path).unwrap(), malformed);
    }
    let unsupported = changed_metadata(&original, VST3_CLOCK_VERSION + 1, |_| {});
    fs::write(&path, &unsupported).unwrap();
    assert!(
        DraftReader::open(&path, limits())
            .err()
            .unwrap()
            .to_string()
            .contains("unsupported")
    );
    assert_eq!(fs::read(&path).unwrap(), unsupported);
}

#[test]
fn completed_sources_retain_file_lock_and_refuse_incomplete_or_damaged_terminals() {
    let (source, target, provenance) = fixture();
    for terminal in [
        None,
        Some(DraftTerminal::Interrupted),
        Some(DraftTerminal::Aborted(DraftAbortReason::OutputFailed)),
    ] {
        let dir = TestDir::new();
        let path = dir.path();
        let mut writer = DraftWriter::create_new(&path, &source, target, provenance).unwrap();
        writer.append(&[event(0)]).unwrap();
        if let Some(terminal) = terminal {
            writer.finish(terminal).unwrap();
        }
        drop(writer);
        let original = fs::read(&path).unwrap();
        assert!(CompletedDraft::open(&path, limits()).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(open(&path).progress().durable_events, 1);
    }

    let dir = TestDir::new();
    let path = dir.path();
    let mut writer = DraftWriter::create_new(&path, &source, target, provenance).unwrap();
    let end = clock(event(0));
    writer.finish(DraftTerminal::Finished(end)).unwrap();
    drop(writer);
    let mut completed = open(&path).into_completed().unwrap();
    assert_eq!(completed.event_count(), 0);
    assert_eq!(completed.end_clock(), end);
    assert_eq!(completed.event_at_or_before(u64::MAX).unwrap(), None);
    let external = OpenOptions::new().write(true).open(&path).unwrap();
    assert!(fs2::FileExt::try_lock_exclusive(&external).is_err());
    drop(completed);
    fs2::FileExt::try_lock_exclusive(&external).unwrap();
    fs2::FileExt::unlock(&external).unwrap();
    drop(external);
    OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&[1])
        .unwrap();
    let damaged = fs::read(&path).unwrap();
    assert!(CompletedDraft::open(&path, limits()).is_err());
    assert_eq!(
        open(&path).outcome(),
        DraftOutcome::Damaged {
            reason: DraftDamage::TrailingData
        }
    );
    assert_eq!(fs::read(&path).unwrap(), damaged);
}

#[test]
fn completed_predecessor_queries_preserve_ties_and_page_boundaries_without_an_index() {
    let dir = TestDir::new();
    let path = dir.path();
    let (source, target, provenance, capture) = capture_fixture();
    let events: Vec<_> = (0..130)
        .map(|index| AppliedEvent {
            sequence: index + 1,
            performance_frame: index / 2 * 10 + 5,
            content_frame: index / 2 * 10 + 15,
            loop_pass: 0,
            value: AutomationValue::new((index % 65) as f32 / 64.0).unwrap(),
        })
        .collect();
    let mut writer =
        DraftWriter::create_with_clock(&path, &source, target, provenance, capture).unwrap();
    for batch in events.chunks(MAX_APPEND_EVENTS) {
        writer.append(batch).unwrap();
    }
    writer
        .finish(DraftTerminal::Finished(DraftClock {
            performance_frame: 650,
            content_frame: 660,
            loop_pass: 0,
        }))
        .unwrap();
    drop(writer);
    let mut completed = CompletedDraft::open(&path, limits()).unwrap();
    for frame in (0..=660).chain([u64::MAX]) {
        let expected = events
            .iter()
            .rev()
            .find(|event| event.performance_frame <= frame)
            .copied();
        assert_eq!(
            completed.event_at_or_before(frame).unwrap(),
            expected,
            "frame={frame}"
        );
    }
    let mut page = [None; 10];
    assert_eq!(completed.read_events(60, &mut page).unwrap(), 10);
    for (actual, expected) in page.iter().zip(&events[60..70]) {
        assert_eq!(*actual, Some(*expected));
    }

    // Every candidate read now encounters changed, still well-typed event values.
    // Detection must not depend on the implementation's choice of binary-search midpoint.
    let bytes = fs::read(&path).unwrap();
    let records_start = PREFIX_BYTES as u64 + u64_at(&bytes, 16) + u64_at(&bytes, 24) + 32;
    let mut external = OpenOptions::new().write(true).open(&path).unwrap();
    for ordinal in 0..events.len() as u64 {
        external
            .seek(SeekFrom::Start(
                records_start + ordinal * RECORD_BYTES as u64 + 40,
            ))
            .unwrap();
        external
            .write_all(&u64::from(0.7_f32.to_bits()).to_le_bytes())
            .unwrap();
    }
    external.sync_all().unwrap();
    let damaged = fs::read(&path).unwrap();
    assert!(completed.event_at_or_before(u64::MAX).is_err());
    assert!(completed.read_events(60, &mut page).is_err());
    assert!(page.iter().all(Option::is_none));
    assert_eq!(fs::read(&path).unwrap(), damaged);
}
