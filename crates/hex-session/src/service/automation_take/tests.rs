use super::*;
use hex_project::{AutomationValue, Command, Created, Editor};
use hex_storage::automation_draft::{CaptureClock, DraftTerminal, DraftWriter};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hex-automation-take-{}-{}",
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
            name: "take source".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    // Deliberately different from the captured section loop used below.
    editor
        .execute(Command::SetLoopRegion {
            start: 1000,
            end: 2000,
            enabled: true,
        })
        .unwrap();
    (
        editor.project().clone(),
        DraftTarget::TrackGain { track },
        DraftProvenance {
            sample_rate: 48_000,
            output_identity: 9,
            project_generation: 0,
            editor_revision: 2,
            audio_revision: 7,
            transport_epoch: 1,
            gesture_serial: 1,
        },
    )
}
fn limits() -> DraftReadLimits {
    DraftReadLimits {
        max_source_bytes: 64 * 1024,
    }
}
fn write(
    path: &Path,
    clock: Option<Option<LoopRegion>>,
    events: &[AppliedEvent],
    terminal: DraftTerminal,
) {
    let (source, target, provenance) = fixture();
    let mut writer = match clock {
        Some(loop_region) => DraftWriter::create_with_clock(
            path,
            &source,
            target,
            provenance,
            CaptureClock {
                sample_rate: 48_000,
                loop_region,
            },
        )
        .unwrap(),
        None => DraftWriter::create_new(path, &source, target, provenance).unwrap(),
    };
    for batch in events.chunks(MAX_APPEND_EVENTS) {
        writer.append(batch).unwrap();
    }
    writer.finish(terminal).unwrap();
}

/// Independent chronological transport/replay oracle, with one explicit frame
/// step and last-write-wins coverage. It never calls the analytical mapping.
fn history(
    first_content: u64,
    first_pass: u64,
    duration: u64,
    region: Option<LoopRegion>,
    offsets: &[u64],
) -> (Vec<AppliedEvent>, DraftClock, BTreeMap<u64, AppliedEvent>) {
    let mut content = first_content;
    let mut pass = first_pass;
    let mut events = Vec::new();
    let mut replay = BTreeMap::new();
    let region = region.filter(|r| first_pass > 0 || first_content < r.end());
    for delta in 0..=duration {
        for &offset in offsets {
            if offset == delta {
                let sequence = events.len() as u64 + 1;
                events.push(AppliedEvent {
                    sequence,
                    performance_frame: 100 + delta,
                    content_frame: content,
                    loop_pass: pass,
                    value: AutomationValue::new((sequence % 17) as f32 / 16.0).unwrap(),
                });
            }
        }
        if delta == duration {
            return (
                events,
                DraftClock {
                    performance_frame: 100 + delta,
                    content_frame: content,
                    loop_pass: pass,
                },
                replay,
            );
        }
        if let Some(event) = events.last() {
            replay.insert(content, *event);
        }
        content += 1;
        if let Some(region) = region {
            if content == region.end() {
                content = region.start();
                pass += 1;
            }
        }
    }
    panic!("finite chronological oracle")
}

#[test]
fn completed_take_content_queries_match_chronological_last_pass_replay() {
    let files = Files::new();
    let region = LoopRegion::new(3, 8, true).unwrap();
    for (case, (content, pass, duration, region)) in [
        (1, 0, 0, Some(region)),
        (1, 0, 1, Some(region)),
        (1, 0, 24, Some(region)),
        (5, 7, 11, Some(region)),
        (7, 0, 4, Some(region)),
        (9, 0, 24, Some(region)),
        (5, 6, 33, None),
    ]
    .into_iter()
    .enumerate()
    {
        let offsets: Vec<_> = [0, 3, 3, 9, duration]
            .into_iter()
            .filter(|d| *d <= duration)
            .collect();
        let (events, end, expected) = history(content, pass, duration, region, &offsets);
        let path = files.path(&format!("case-{case}"));
        write(&path, Some(region), &events, DraftTerminal::Finished(end));
        let original = fs::read(&path).unwrap();
        let mut source = CompletedAutomationTake::open_on_control(&path, limits()).unwrap();
        assert_eq!(source.event_count(), events.len() as u64);
        for frame in 0..40 {
            assert_eq!(
                source.value_at_on_control(frame).unwrap(),
                expected.get(&frame).copied(),
                "case={case} frame={frame}"
            );
        }
        let positions = [7, 1, 5, 8, 6, 0];
        let mut preview = [None; 6];
        source
            .read_preview_on_control(&positions, &mut preview)
            .unwrap();
        assert_eq!(
            preview,
            positions.map(|frame| expected.get(&frame).copied())
        );
        drop(source);
        assert_eq!(fs::read(&path).unwrap(), original);
    }
}

#[test]
fn a_trillion_loop_passes_with_no_new_events_keep_held_coverage() {
    let files = Files::new();
    let region = LoopRegion::new(2, 8, true).unwrap();
    let event = AppliedEvent {
        sequence: 1,
        performance_frame: 100,
        content_frame: 3,
        loop_pass: 7,
        value: AutomationValue::new(0.75).unwrap(),
    };
    let end = DraftClock {
        performance_frame: 100 + 6_000_000_000_000,
        content_frame: 3,
        loop_pass: 1_000_000_000_007,
    };
    let path = files.path("held");
    write(
        &path,
        Some(Some(region)),
        &[event],
        DraftTerminal::Finished(end),
    );
    let mut source = CompletedAutomationTake::open_on_control(&path, limits()).unwrap();
    assert_eq!(source.event_count(), 1);
    for frame in 0..10 {
        assert_eq!(
            source.value_at_on_control(frame).unwrap(),
            (2..8).contains(&frame).then_some(event)
        );
    }
}

#[test]
fn every_page_is_validated_before_queries_and_legacy_wrapped_context_is_not_guessed() {
    let files = Files::new();
    let (events, end, expected) = history(
        10,
        9,
        2000,
        None,
        &(0..=2000).step_by(3).collect::<Vec<_>>(),
    );
    let path = files.path("pages");
    write(&path, None, &events, DraftTerminal::Finished(end));
    let mut source = CompletedAutomationTake::open_on_control(&path, limits()).unwrap();
    for frame in [9, 10, 201, 202, 203, 1001, 2009, 2010] {
        assert_eq!(
            source.value_at_on_control(frame).unwrap(),
            expected.get(&frame).copied()
        );
    }
    drop(source);
    let mut broken = events.clone();
    broken[129].content_frame += 1; // valid monotonic bytes, wrong continuous clock
    let bad = files.path("discontinuous");
    write(&bad, None, &broken, DraftTerminal::Finished(end));
    let original = fs::read(&bad).unwrap();
    assert!(
        CompletedAutomationTake::open_on_control(&bad, limits())
            .err()
            .unwrap()
            .contains("discontinuous")
    );
    assert_eq!(fs::read(&bad).unwrap(), original);
    let region = LoopRegion::new(3, 8, true).unwrap();
    let (events, end, _) = history(4, 0, 20, Some(region), &[0, 7, 13]);
    let legacy = files.path("unknown-loop");
    write(&legacy, None, &events, DraftTerminal::Finished(end));
    assert!(
        CompletedAutomationTake::open_on_control(&legacy, limits())
            .err()
            .unwrap()
            .contains("no captured loop context")
    );
    let known_linear = files.path("false-linear");
    write(
        &known_linear,
        Some(None),
        &events,
        DraftTerminal::Finished(end),
    );
    assert!(CompletedAutomationTake::open_on_control(&known_linear, limits()).is_err());
}

#[test]
fn release_rate_overflow_and_preview_failures_preserve_original_drafts() {
    use std::io::{Seek, SeekFrom, Write};
    let files = Files::new();
    let (events, mut end, _) = history(10, 0, 40, None, &[0, 10, 30]);
    end.content_frame += 1;
    let path = files.path("bad-release");
    write(&path, Some(None), &events, DraftTerminal::Finished(end));
    assert!(
        CompletedAutomationTake::open_on_control(&path, limits())
            .err()
            .unwrap()
            .contains("release clock")
    );
    let (source, target, mut stamp) = fixture();
    stamp.sample_rate = 44_100;
    let path = files.path("rate");
    let mut writer = DraftWriter::create_new(&path, &source, target, stamp).unwrap();
    writer.append(&events).unwrap();
    writer
        .finish(DraftTerminal::Finished(DraftClock {
            performance_frame: 140,
            content_frame: 50,
            loop_pass: 0,
        }))
        .unwrap();
    drop(writer);
    assert!(
        CompletedAutomationTake::open_on_control(&path, limits())
            .err()
            .unwrap()
            .contains("sample rates differ")
    );
    let event = AppliedEvent {
        performance_frame: 1,
        content_frame: u64::MAX - 1,
        ..events[0]
    };
    let path = files.path("overflow");
    write(
        &path,
        Some(None),
        &[event],
        DraftTerminal::Finished(DraftClock {
            performance_frame: 4,
            content_frame: u64::MAX,
            loop_pass: 0,
        }),
    );
    assert!(
        CompletedAutomationTake::open_on_control(&path, limits())
            .err()
            .unwrap()
            .contains("overflow")
    );
    let (events, end, _) = history(10, 0, 40, None, &[0, 10, 30]);
    let path = files.path("preview");
    write(&path, Some(None), &events, DraftTerminal::Finished(end));
    let mut source = CompletedAutomationTake::open_on_control(&path, limits()).unwrap();
    let mut output = [Some(events[0]); 65];
    assert!(
        source
            .read_preview_on_control(&[10; 65], &mut output)
            .is_err()
    );
    assert_eq!(output, [None; 65]);
    assert!(source.read_preview_on_control(&[10], &mut output).is_err());
    assert_eq!(output, [None; 65]);
    let bytes = fs::read(&path).unwrap();
    let source_bytes = u64::from_le_bytes(bytes[16..24].try_into().unwrap());
    let metadata_bytes = u64::from_le_bytes(bytes[24..32].try_into().unwrap());
    let records = 32 + source_bytes + metadata_bytes + 32;
    // Deliberately bypass the cooperative reader lock to exercise a later read
    // failure after the first preview position has already succeeded.
    let mut changed = fs::OpenOptions::new().write(true).open(&path).unwrap();
    changed
        .seek(SeekFrom::Start(records + 2 * 96 + 40))
        .unwrap();
    changed
        .write_all(&u64::from(0.25_f32.to_bits()).to_le_bytes())
        .unwrap();
    changed.sync_all().unwrap();
    let mut preview = [None; 2];
    assert!(
        source
            .read_preview_on_control(&[10, 40], &mut preview)
            .is_err()
    );
    assert_eq!(preview, [None; 2]);
}

#[test]
fn no_events_have_no_coverage_and_the_writer_retains_the_playback_clock() {
    use crate::service::automation_draft::start_with_playback_on_control;
    let files = Files::new();
    let (project, target, provenance) = fixture();
    let session = crate::Session::from_loaded(project.clone(), 8, 0, Vec::new()).unwrap();
    let compiled = hex_engine::compile::project(&project, &BTreeMap::new()).unwrap();
    let playback = hex_engine::Playback::new(compiled);
    assert_eq!(playback.automation_capture_loop(), project.loop_region());
    let path = files.path("empty");
    let (mut input, worker) =
        start_with_playback_on_control(&session, None, path.clone(), target, provenance, &playback)
            .unwrap();
    input
        .finish(DraftTerminal::Finished(DraftClock {
            performance_frame: 100,
            content_frame: 9000,
            loop_pass: 0,
        }))
        .unwrap();
    assert!(worker.join_on_control().unwrap().failure.is_none());
    let raw = CompletedDraft::open(&path, limits()).unwrap();
    assert_eq!(
        raw.capture_clock().unwrap().loop_region,
        project.loop_region()
    );
    let mut take = CompletedAutomationTake::from_completed_on_control(raw).unwrap();
    assert_eq!(take.event_count(), 0);
    for frame in [0, 1000, 9000, u64::MAX] {
        assert_eq!(take.value_at_on_control(frame).unwrap(), None);
    }
}

#[test]
#[ignore = "manual optimized control-side lookup profile; no callback or deadline claim"]
fn profile_completed_automation_queries() {
    use std::time::Instant;
    const EVENTS: u64 = 65_536;
    const QUERIES: u64 = 4096;
    let files = Files::new();
    let path = files.path("query-profile");
    let events: Vec<_> = (0..EVENTS)
        .map(|index| AppliedEvent {
            sequence: index + 1,
            performance_frame: 100 + index * 256,
            content_frame: index * 256,
            loop_pass: 0,
            value: AutomationValue::new(((index + 1) % 17) as f32 / 16.0).unwrap(),
        })
        .collect();
    write(
        &path,
        Some(None),
        &events,
        DraftTerminal::Finished(DraftClock {
            performance_frame: 100 + EVENTS * 256,
            content_frame: EVENTS * 256,
            loop_pass: 0,
        }),
    );
    for trial in 0..3 {
        let start = Instant::now();
        let mut source = CompletedAutomationTake::open_on_control(&path, limits()).unwrap();
        let validated_open_seconds = start.elapsed().as_secs_f64();
        for frame in 0..128 {
            source.value_at_on_control(frame * 17).unwrap();
        }
        let start = Instant::now();
        for query in 0..QUERIES {
            let frame = (query * 104_729 + 127) % (EVENTS * 256);
            let actual = source.value_at_on_control(frame).unwrap().unwrap();
            assert_eq!(actual.sequence, frame / 256 + 1);
            assert_eq!(actual.value.get(), (actual.sequence % 17) as f32 / 16.0);
        }
        let query_seconds = start.elapsed().as_secs_f64();
        println!(
            "completed_automation_queries trial={trial} events={EVENTS} warmup_queries=128 measured_queries={QUERIES} validated_open_seconds={validated_open_seconds:.6} query_seconds={query_seconds:.6} mean_query_microseconds={:.3} max_binary_search_steps=17 max_record_reads_per_step=2",
            query_seconds * 1e6 / QUERIES as f64
        );
    }
}
