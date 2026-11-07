use super::*;
use crate::automation_draft::{DraftTerminal, DraftWriter};
use hex_project::{AutomationValue, Command, Created, Editor};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hex-prepared-automation-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn fixture() -> (Project, DraftTarget, DraftProvenance) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "retained source".into(),
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
            output_identity: 7,
            project_generation: 0,
            editor_revision: 2,
            audio_revision: 3,
            transport_epoch: 1,
            gesture_serial: 1,
        },
    )
}
fn events(value: f32) -> Vec<AppliedEvent> {
    (0..130)
        .map(|i| AppliedEvent {
            sequence: i + 1,
            performance_frame: 100 + i * 3,
            content_frame: 10 + i * 3,
            loop_pass: 0,
            value: AutomationValue::new(if i % 2 == 0 { value } else { 0.0 }).unwrap(),
        })
        .collect()
}
fn write(path: &Path, version: u32, events: &[AppliedEvent]) {
    let (source, target, provenance) = fixture();
    let mut writer = if version == 1 {
        DraftWriter::create_new(path, &source, target, provenance).unwrap()
    } else {
        DraftWriter::create_with_clock(
            path,
            &source,
            target,
            provenance,
            CaptureClock {
                sample_rate: 48_000,
                loop_region: None,
            },
        )
        .unwrap()
    };
    for batch in events.chunks(MAX_APPEND_EVENTS) {
        writer.append(batch).unwrap();
    }
    writer
        .finish(DraftTerminal::Finished(DraftClock {
            performance_frame: 500,
            content_frame: 410,
            loop_pass: 0,
        }))
        .unwrap();
}
fn limits() -> DraftReadLimits {
    DraftReadLimits {
        max_source_bytes: 64 * 1024,
    }
}

fn open_written(path: &Path) -> CompletedAutomationTake {
    #[cfg(unix)]
    let _released = super::super::super::tests::wait_for_shared_lock(
        path,
        std::time::Duration::from_millis(250),
    )
    .unwrap();
    CompletedAutomationTake::open_on_control(path, limits()).unwrap()
}

#[cfg(unix)]
#[test]
fn retained_writer_descriptor_blocks_completed_take_open_until_released() {
    let files = Files::new();
    let path = files.path("retained-writer");
    let (source, target, provenance) = fixture();
    let mut writer = DraftWriter::create_new(&path, &source, target, provenance).unwrap();
    writer
        .finish(DraftTerminal::Finished(DraftClock {
            performance_frame: 0,
            content_frame: 0,
            loop_pass: 0,
        }))
        .unwrap();
    let retained = writer.file.try_clone().unwrap();
    drop(writer);
    let error = CompletedAutomationTake::open_on_control(&path, limits())
        .err()
        .unwrap();
    assert!(matches!(error, TakeOpenError::Busy(_)), "{error}");
    drop(retained);
    assert!(CompletedAutomationTake::open_on_control(&path, limits()).is_ok());
}

#[test]
fn lean_transfer_releases_token_preserves_lock_and_resumes_the_same_cursor() {
    let files = Files::new();
    let path = files.path("lean");
    write(&path, 2, &events(0.75));
    let original = fs::read(&path).unwrap();
    let mut full = open_written(&path);
    let token = full.core.source.source_token_weak();
    assert!(token.upgrade().is_some());
    let owner = Arc::downgrade(&full.core.cursor_owner);
    let identity = full.content_identity();
    let target = full.target();
    let provenance = full.provenance();
    let capture_clock = full.capture_clock();
    let destination = Arc::new(full.captured_tempo_map().clone());
    let mut expected = [None; 512];
    full.read_projected_values_on_control(Arc::clone(&destination), 0, &mut expected)
        .unwrap();
    let mut cursor = full
        .begin_projected_spans_on_control(Arc::clone(&destination), 0..512)
        .unwrap();
    let mut output = [None; 3];
    let first = full
        .read_projected_spans_on_control(&destination, &mut cursor, 7, &mut output)
        .unwrap();
    let mut actual = [None; 512];
    for span in output[..first.written].iter().flatten() {
        actual[span.start as usize..span.end as usize].fill(Some(span.value));
    }
    let mut prepared = full.into_prepared_on_control().unwrap();
    assert!(
        token.upgrade().is_none(),
        "canonical checkpoint token must not survive lean transfer"
    );
    assert!(owner.upgrade().is_some());
    assert_eq!(prepared.content_identity(), identity);
    assert_eq!(prepared.target(), target);
    assert_eq!(prepared.provenance(), provenance);
    assert_eq!(prepared.capture_clock(), capture_clock);
    assert_eq!(prepared.event_count(), 130);
    assert_eq!(prepared.end_clock().content_frame, 410);
    let external = fs::OpenOptions::new().write(true).open(&path).unwrap();
    assert!(fs2::FileExt::try_lock_exclusive(&external).is_err());
    loop {
        let progress = prepared
            .read_projected_spans_on_control(&destination, &mut cursor, 7, &mut output)
            .unwrap();
        assert!(progress.records_read <= 7 && progress.records_visited <= 7);
        for span in output[..progress.written].iter().flatten() {
            actual[span.start as usize..span.end as usize].fill(Some(span.value));
        }
        if progress.state == SpanReadState::Finished {
            break;
        }
    }
    assert_eq!(actual, expected);
    let mut direct = [None; 512];
    prepared
        .read_projected_values_on_control(Arc::clone(&destination), 0, &mut direct)
        .unwrap();
    assert_eq!(direct, expected);
    drop(prepared);
    fs2::FileExt::try_lock_exclusive(&external).unwrap();
    fs2::FileExt::unlock(&external).unwrap();
    drop(cursor);
    assert!(owner.upgrade().is_none());
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn identity_is_the_validated_terminal_chain_and_exact_length_for_both_formats() {
    let files = Files::new();
    for version in [1, 2] {
        let path = files.path(&format!("identity-{version}"));
        write(&path, version, &events(0.25));
        let bytes = fs::read(&path).unwrap();
        let full = open_written(&path);
        let identity = full.content_identity();
        assert_eq!(identity.version(), version);
        assert_eq!(identity.file_bytes(), bytes.len() as u64);
        assert_eq!(
            identity.terminal_chain().as_slice(),
            &bytes[bytes.len() - 32..]
        );
        let mut prepared = full.into_prepared_on_control().unwrap();
        assert_eq!(prepared.content_identity(), identity);
        assert_eq!(
            prepared
                .value_at_on_control(10)
                .unwrap()
                .unwrap()
                .value
                .get(),
            0.25
        );
        assert_eq!(prepared.content_identity(), identity);
        drop(prepared);
        let changed = files.path(&format!("changed-{version}"));
        write(&changed, version, &events(0.75));
        assert_ne!(open_written(&changed).content_identity(), identity);
    }
}

#[test]
fn lean_layout_exhaustively_reports_actual_path_tempo_and_shared_data() {
    let files = Files::new();
    let path = files.path("layout");
    write(&path, 2, &events(0.5));
    let mut full = open_written(&path);
    full.core.source.reader.records.path.reserve(37);
    let path_capacity = full.core.source.reader.records.path.capacity();
    let group = Arc::as_ptr(&full.core.cursor_owner) as usize;
    let prepared = full.into_prepared_on_control().unwrap();
    let layout = prepared.data_layout();
    assert_eq!(
        layout.inline_bytes(),
        std::mem::size_of::<PreparedAutomationTake>()
    );
    assert_eq!(layout.path_capacity_bytes(), path_capacity);
    assert!(layout.path_capacity_bytes() > prepared.path().as_os_str().len());
    assert_eq!(
        layout.captured_tempo(),
        prepared.captured_tempo_map().owned_allocation_layout()
    );
    assert_eq!(layout.inline_array_bytes(), 64);
    assert_eq!(
        layout.checked_exclusive_data_bytes(),
        Some(
            layout.inline_bytes()
                + path_capacity
                + layout.captured_tempo().checked_backing_bytes().unwrap()
        )
    );
    assert_eq!(layout.source_owner().identity(), group);
    assert_eq!(layout.source_owner().handles(), 1);
    assert_eq!(
        layout.source_owner().data_bytes(),
        std::mem::size_of::<()>()
    );
}

#[test]
fn lean_source_keeps_identity_and_refuses_post_transfer_record_damage() {
    use std::io::{Seek, SeekFrom, Write};
    let files = Files::new();
    let path = files.path("damage");
    write(&path, 2, &events(0.5));
    let full = open_written(&path);
    let mut prepared = full.into_prepared_on_control().unwrap();
    let identity = prepared.content_identity();
    let bytes = fs::read(&path).unwrap();
    let records = 32
        + u64::from_le_bytes(bytes[16..24].try_into().unwrap())
        + u64::from_le_bytes(bytes[24..32].try_into().unwrap())
        + 32;
    let mut external = fs::OpenOptions::new().write(true).open(&path).unwrap();
    external
        .seek(SeekFrom::Start(records + 64 * 96 + 40))
        .unwrap();
    external
        .write_all(&u64::from(0.9_f32.to_bits()).to_le_bytes())
        .unwrap();
    external.sync_all().unwrap();
    let damaged = fs::read(&path).unwrap();
    let destination = Arc::new(prepared.captured_tempo_map().clone());
    let mut output = [Some(AutomationValue::new(0.5).unwrap()); 512];
    assert!(
        prepared
            .read_projected_values_on_control(destination, 0, &mut output)
            .is_err()
    );
    assert!(output.iter().all(Option::is_none));
    assert_eq!(
        prepared.content_identity(),
        identity,
        "damage cannot silently restamp the verified identity"
    );
    assert_eq!(fs::read(&path).unwrap(), damaged);
}

#[test]
fn projection_fingerprint_binds_exact_ordered_map_facts_and_rate() {
    use hex_project::{BeatTime, Tempo, TempoPoint};
    let map = TempoMap::constant(48_000, Tempo::new(120_000_000).unwrap()).unwrap();
    let fingerprint = automation_projection_fingerprint(&map);
    assert_eq!(automation_projection_fingerprint(&map.clone()), fingerprint);
    assert_ne!(
        automation_projection_fingerprint(
            &TempoMap::constant(44_100, Tempo::new(120_000_000).unwrap()).unwrap()
        ),
        fingerprint
    );
    assert_ne!(
        automation_projection_fingerprint(
            &TempoMap::constant(48_000, Tempo::new(121_000_000).unwrap()).unwrap()
        ),
        fingerprint
    );
    let points = vec![
        TempoPoint::new(BeatTime::ZERO, Tempo::new(120_000_000).unwrap()),
        TempoPoint::new(BeatTime::ONE, Tempo::new(120_000_000).unwrap()),
    ];
    let segmented = TempoMap::new(48_000, points.clone()).unwrap();
    assert_ne!(
        automation_projection_fingerprint(&segmented),
        fingerprint,
        "exact segmentation facts remain part of the immutable key"
    );
    let shifted = TempoMap::new(
        48_000,
        vec![
            points[0],
            TempoPoint::new(BeatTime::from_quarters(2).unwrap(), points[1].tempo()),
        ],
    )
    .unwrap();
    assert_ne!(
        automation_projection_fingerprint(&shifted),
        automation_projection_fingerprint(&segmented)
    );
}
