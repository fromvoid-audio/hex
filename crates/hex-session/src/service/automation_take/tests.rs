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
        if let Some(region) = region
            && content == region.end()
        {
            content = region.start();
            pass += 1;
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
                source
                    .captured_coverage()
                    .iter()
                    .flatten()
                    .any(|range| range.contains(&frame)),
                expected.contains_key(&frame)
            );
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
fn projected_preview_matches_forward_replay_with_collisions_and_exclusive_coverage() {
    use hex_project::{BeatTime, Tempo, TempoMap, TempoPoint};
    let files = Files::new();
    let region = LoopRegion::new(3, 8, true).unwrap();
    for (case, (content, pass, duration, region)) in [
        (1, 0, 24, Some(region)),
        (7, 0, 2, Some(region)),
        (5, 7, 11, Some(region)),
        (9, 0, 24, Some(region)),
        (2, 0, 1, None),
        (1, 0, 0, None),
    ]
    .into_iter()
    .enumerate()
    {
        let offsets: Vec<_> = [0, 1, 1, 3, 9, duration]
            .into_iter()
            .filter(|d| *d <= duration)
            .collect();
        let (events, end, expected_source) = history(content, pass, duration, region, &offsets);
        let path = files.path(&format!("projected-{case}"));
        write(&path, Some(region), &events, DraftTerminal::Finished(end));
        let original = fs::read(&path).unwrap();
        let mut source = CompletedAutomationTake::open_on_control(&path, limits()).unwrap();
        let captured = source.source().tempo_map().clone();
        let maps = [
            captured.clone(),
            TempoMap::constant(48_000, Tempo::new(240_000_000).unwrap()).unwrap(),
            TempoMap::constant(96_000, Tempo::new(60_000_000).unwrap()).unwrap(),
            TempoMap::constant(8_000, Tempo::new(1_000_000_000).unwrap()).unwrap(),
            TempoMap::new(
                48_000,
                vec![
                    TempoPoint::new(BeatTime::ZERO, Tempo::new(60_000_000).unwrap()),
                    TempoPoint::new(
                        BeatTime::from_raw(2_000_000),
                        Tempo::new(350_000_000).unwrap(),
                    ),
                ],
            )
            .unwrap(),
        ];
        // Derive contiguous source coverage from chronological replay, without
        // using the analytical source ranges or projection locator under test.
        let mut ranges: Vec<std::ops::Range<u64>> = Vec::new();
        for &frame in expected_source.keys() {
            if let Some(last) = ranges.last_mut().filter(|last| last.end == frame) {
                last.end += 1;
            } else {
                ranges.push(frame..frame + 1);
            }
        }
        for destination in maps {
            let forward = |frame| {
                destination
                    .beat_to_frame(captured.frame_to_beat(frame).unwrap())
                    .unwrap()
            };
            let mut expected = BTreeMap::new();
            for range in &ranges {
                let mut changes = BTreeMap::new();
                for frame in range.clone() {
                    changes.insert(forward(frame), expected_source[&frame]);
                }
                for frame in forward(range.start)..forward(range.end) {
                    expected.insert(frame, *changes.range(..=frame).next_back().unwrap().1);
                }
            }
            let positions: Vec<_> = (0..140).rev().collect();
            for frames in positions.chunks(MAX_PREVIEW_QUERIES) {
                let mut actual = vec![None; frames.len()];
                source
                    .read_projected_preview_on_control(&destination, frames, &mut actual)
                    .unwrap();
                let coverage = source.projected_coverage(&destination).unwrap();
                for (&frame, value) in frames.iter().zip(actual) {
                    assert_eq!(
                        value,
                        expected.get(&frame).copied(),
                        "case={case} destination={frame}"
                    );
                    assert_eq!(
                        coverage
                            .iter()
                            .flatten()
                            .any(|range| range.contains(&frame)),
                        expected.contains_key(&frame)
                    );
                }
            }
        }
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
    let destination = std::sync::Arc::new(source.source().tempo_map().clone());
    let mut values = [None; 10];
    source
        .read_projected_values_on_control(destination, 0, &mut values)
        .unwrap();
    assert_eq!(
        values,
        std::array::from_fn(|frame| (2..8).contains(&frame).then_some(event.value))
    );
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
            .to_string()
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
            .to_string()
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
            .to_string()
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
            .to_string()
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
            .to_string()
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
    let destination = source.source().tempo_map().clone();
    preview.fill(Some(events[0]));
    assert!(
        source
            .read_projected_preview_on_control(&destination, &[10, 40], &mut preview)
            .is_err()
    );
    assert_eq!(preview, [None; 2]);
}

#[test]
fn unrepresentable_projected_coverage_is_an_error_even_outside_the_touch() {
    let files = Files::new();
    let event = AppliedEvent {
        sequence: 1,
        performance_frame: 1,
        content_frame: u64::MAX - 10,
        loop_pass: 0,
        value: AutomationValue::new(0.5).unwrap(),
    };
    let path = files.path("unrepresentable-projection");
    write(
        &path,
        Some(None),
        &[event],
        DraftTerminal::Finished(DraftClock {
            performance_frame: 3,
            content_frame: u64::MAX - 8,
            loop_pass: 0,
        }),
    );
    let original = fs::read(&path).unwrap();
    let mut source = CompletedAutomationTake::open_on_control(&path, limits()).unwrap();
    let destination = source.source().tempo_map().clone();
    assert!(source.projected_coverage(&destination).is_err());
    let mut output = [Some(event)];
    assert!(
        source
            .read_projected_preview_on_control(&destination, &[0], &mut output)
            .is_err()
    );
    assert_eq!(output, [None]);
    drop(source);
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn no_events_have_no_coverage_and_the_writer_retains_the_playback_clock() {
    use crate::service::automation_draft::start_with_playback_on_control;
    let files = Files::new();
    let (project, target, provenance) = fixture();
    let session = crate::Session::from_loaded(project.clone(), 8, 0, Vec::new()).unwrap();
    let compiled = hex_engine::compile::project(&project, &BTreeMap::new()).unwrap();
    let playback = hex_engine::playback::Playback::new(compiled);
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
    let captured = fixture().0.tempo_map().clone();
    let destination =
        hex_project::TempoMap::constant(48_000, hex_project::Tempo::new(173_000_000).unwrap())
            .unwrap();
    let forward = |frame| {
        destination
            .beat_to_frame(captured.frame_to_beat(frame).unwrap())
            .unwrap()
    };
    let placed: Vec<_> = events
        .iter()
        .map(|event| (forward(event.content_frame), *event))
        .collect();
    let end = forward(EVENTS * 256);
    let positions: Vec<_> = (0..QUERIES)
        .map(|query| (query * 104_729 + 127) % end)
        .collect();
    let expected: Vec<_> = positions
        .iter()
        .map(|frame| placed[placed.partition_point(|(at, _)| at <= frame) - 1].1)
        .collect();
    for trial in 0..3 {
        let mut source = CompletedAutomationTake::open_on_control(&path, limits()).unwrap();
        let mut output = [None; MAX_PREVIEW_QUERIES];
        for _ in 0..2 {
            source
                .read_projected_preview_on_control(
                    &destination,
                    &positions[..MAX_PREVIEW_QUERIES],
                    &mut output,
                )
                .unwrap();
        }
        let start = Instant::now();
        for (frames, expected) in positions
            .chunks(MAX_PREVIEW_QUERIES)
            .zip(expected.chunks(MAX_PREVIEW_QUERIES))
        {
            source
                .read_projected_preview_on_control(&destination, frames, &mut output)
                .unwrap();
            for (actual, expected) in output.iter().zip(expected) {
                assert_eq!(*actual, Some(*expected));
            }
        }
        let seconds = start.elapsed().as_secs_f64();
        println!(
            "projected_automation_queries trial={trial} events={EVENTS} warmup_queries=128 measured_queries={QUERIES} batch_queries={MAX_PREVIEW_QUERIES} captured_tempo=120000000 destination_tempo=173000000 query_seconds={seconds:.6} mean_query_microseconds={:.3}",
            seconds * 1e6 / QUERIES as f64
        );
    }
}

fn span_values(
    source: &mut CompletedAutomationTake,
    destination: &std::sync::Arc<hex_project::TempoMap>,
    start: u64,
    frames: usize,
    budget: usize,
    slots: usize,
) -> (Vec<Option<AutomationValue>>, usize) {
    let mut cursor = source
        .begin_projected_spans_on_control(
            std::sync::Arc::clone(destination),
            start..start + frames as u64,
        )
        .unwrap();
    let mut values = vec![None; frames];
    let mut output = vec![None; slots];
    let mut steps = 0;
    let mut empty_pending = 0;
    let mut last_end = start;
    loop {
        let progress = source
            .read_projected_spans_on_control(destination, &mut cursor, budget, &mut output)
            .unwrap();
        assert!(progress.records_read <= budget);
        assert!(progress.records_visited <= budget);
        steps += 1;
        assert!(steps <= source.event_count() as usize * 3 + 20);
        assert!(progress.written <= slots);
        assert!(output[progress.written..].iter().all(Option::is_none));
        for span in output[..progress.written].iter().flatten() {
            assert!(span.start >= last_end && span.start < span.end);
            values[(span.start - start) as usize..(span.end - start) as usize]
                .fill(Some(span.value));
            last_end = span.end;
        }
        if progress.state == SpanReadState::Finished {
            assert_eq!(
                source
                    .read_projected_spans_on_control(destination, &mut cursor, budget, &mut output)
                    .unwrap(),
                SpanReadProgress {
                    written: 0,
                    records_read: 0,
                    records_visited: 0,
                    state: SpanReadState::Finished
                }
            );
            break;
        }
        if progress.written == 0 {
            empty_pending += 1;
        }
    }
    (values, empty_pending)
}

#[test]
fn sequential_span_batches_match_projected_previews_across_wraps_gaps_and_collisions() {
    use hex_project::{Tempo, TempoMap};
    use std::sync::Arc;
    let files = Files::new();
    let region = LoopRegion::new(3, 8, true).unwrap();
    for (case, (content, pass, duration, region)) in [
        (1, 0, 24, Some(region)),
        (7, 0, 2, Some(region)),
        (5, 7, 11, Some(region)),
        (9, 0, 24, Some(region)),
        (1, 0, 0, None),
    ]
    .into_iter()
    .enumerate()
    {
        let offsets: Vec<_> = (0..=duration).flat_map(|offset| [offset, offset]).collect();
        let (events, end, _) = history(content, pass, duration, region, &offsets);
        let path = files.path(&format!("span-case-{case}"));
        write(&path, Some(region), &events, DraftTerminal::Finished(end));
        let original = fs::read(&path).unwrap();
        let mut source = CompletedAutomationTake::open_on_control(&path, limits()).unwrap();
        for (rate, tempo) in [
            (48_000, 120_000_000),
            (48_000, 240_000_000),
            (96_000, 60_000_000),
            (8_000, 1_000_000_000),
        ] {
            let destination =
                Arc::new(TempoMap::constant(rate, Tempo::new(tempo).unwrap()).unwrap());
            let positions: Vec<_> = (0..128).collect();
            let mut expected = Vec::new();
            for frames in positions.chunks(MAX_PREVIEW_QUERIES) {
                let mut values = vec![None; frames.len()];
                source
                    .read_projected_preview_on_control(&destination, frames, &mut values)
                    .unwrap();
                expected.extend(values.into_iter().map(|event| event.map(|e| e.value)));
            }
            for (budget, slots) in [(1, 1), (7, 3), (64, 64)] {
                let (actual, _) = span_values(&mut source, &destination, 0, 128, budget, slots);
                assert_eq!(
                    actual, expected,
                    "case={case} budget={budget} slots={slots}"
                );
            }
            let mut actual = [None; 128];
            source
                .read_projected_values_on_control(Arc::clone(&destination), 0, &mut actual)
                .unwrap();
            assert_eq!(actual.as_slice(), expected);
        }
        drop(source);
        assert_eq!(fs::read(&path).unwrap(), original);
    }
}

#[test]
fn dense_collisions_yield_pending_and_covered_zero_is_distinct_from_gaps() {
    use hex_project::{Tempo, TempoMap};
    use std::sync::Arc;
    let files = Files::new();
    let (events, end, _) = history(0, 0, 1024, None, &(0..1024).collect::<Vec<_>>());
    let path = files.path("dense-collisions");
    write(&path, Some(None), &events, DraftTerminal::Finished(end));
    let mut source = CompletedAutomationTake::open_on_control(&path, limits()).unwrap();
    let destination =
        Arc::new(TempoMap::constant(8_000, Tempo::new(1_000_000_000).unwrap()).unwrap());
    let (actual, empty_pending) = span_values(&mut source, &destination, 0, 32, 1, 1);
    assert!(
        empty_pending > 0,
        "tiny work budgets must yield during collapsed event runs"
    );
    let mut expected = [None; 32];
    source
        .read_projected_preview_on_control(
            &destination,
            &(0..32).collect::<Vec<_>>(),
            &mut expected,
        )
        .unwrap();
    assert_eq!(actual, expected.map(|event| event.map(|e| e.value)));
    drop(source);
    let event = AppliedEvent {
        sequence: 1,
        performance_frame: 100,
        content_frame: 10,
        loop_pass: 0,
        value: AutomationValue::new(0.0).unwrap(),
    };
    let path = files.path("covered-zero");
    write(
        &path,
        Some(None),
        &[event],
        DraftTerminal::Finished(DraftClock {
            performance_frame: 110,
            content_frame: 20,
            loop_pass: 0,
        }),
    );
    let mut source = CompletedAutomationTake::open_on_control(&path, limits()).unwrap();
    let destination = Arc::new(source.source().tempo_map().clone());
    let mut values = [None; 32];
    source
        .read_projected_values_on_control(destination, 0, &mut values)
        .unwrap();
    assert!(values[..10].iter().all(Option::is_none));
    assert!(
        values[10..20]
            .iter()
            .all(|value| *value == Some(event.value))
    );
    assert!(values[20..].iter().all(Option::is_none));
}

#[test]
fn cursors_interleave_and_refuse_foreign_reader_or_projection_without_restamping() {
    use std::sync::Arc;
    let files = Files::new();
    let (events, end, _) = history(0, 0, 100, None, &(0..100).collect::<Vec<_>>());
    let path = files.path("interleaved");
    write(&path, Some(None), &events, DraftTerminal::Finished(end));
    let mut source = CompletedAutomationTake::open_on_control(&path, limits()).unwrap();
    let mut foreign = CompletedAutomationTake::open_on_control(&path, limits()).unwrap();
    let destination = Arc::new(source.source().tempo_map().clone());
    let other_projection = Arc::new(destination.as_ref().clone());
    let mut a = source
        .begin_projected_spans_on_control(Arc::clone(&destination), 0..40)
        .unwrap();
    let mut b = source
        .begin_projected_spans_on_control(Arc::clone(&destination), 60..100)
        .unwrap();
    let mut output = [None; 1];
    assert!(
        foreign
            .read_projected_spans_on_control(&destination, &mut a, 1, &mut output)
            .is_err()
    );
    assert_eq!(output, [None]);
    assert!(
        source
            .read_projected_spans_on_control(&other_projection, &mut a, 1, &mut output)
            .is_err()
    );
    assert_eq!(output, [None]);
    let mut a_values = [None; 40];
    let mut b_values = [None; 40];
    let mut done = [false; 2];
    for _ in 0..150 {
        for (index, cursor, values, start) in [
            (0, &mut a, &mut a_values, 0),
            (1, &mut b, &mut b_values, 60),
        ] {
            if done[index] {
                continue;
            }
            let progress = source
                .read_projected_spans_on_control(&destination, cursor, 1, &mut output)
                .unwrap();
            for span in output[..progress.written].iter().flatten() {
                values[(span.start - start) as usize..(span.end - start) as usize]
                    .fill(Some(span.value));
            }
            done[index] = progress.state == SpanReadState::Finished;
        }
        if done.into_iter().all(|done| done) {
            break;
        }
    }
    assert_eq!(done, [true, true]);
    assert_eq!(
        a_values,
        std::array::from_fn(|frame| Some(events[frame].value))
    );
    assert_eq!(
        b_values,
        std::array::from_fn(|frame| Some(events[frame + 60].value))
    );
}

#[test]
fn span_read_failure_seals_cursor_and_clears_public_output_without_touching_file() {
    use std::{
        io::{Seek, SeekFrom, Write},
        sync::Arc,
    };
    let files = Files::new();
    let (events, end, _) = history(0, 0, 10, None, &(0..10).collect::<Vec<_>>());
    let path = files.path("span-damage");
    write(&path, Some(None), &events, DraftTerminal::Finished(end));
    let mut source = CompletedAutomationTake::open_on_control(&path, limits()).unwrap();
    let destination = Arc::new(source.source().tempo_map().clone());
    let mut cursor = source
        .begin_projected_spans_on_control(Arc::clone(&destination), 0..10)
        .unwrap();
    let mut output = [None; 1];
    assert_eq!(
        source
            .read_projected_spans_on_control(&destination, &mut cursor, 1, &mut output)
            .unwrap()
            .written,
        1
    );
    let bytes = fs::read(&path).unwrap();
    let records = 32
        + u64::from_le_bytes(bytes[16..24].try_into().unwrap())
        + u64::from_le_bytes(bytes[24..32].try_into().unwrap())
        + 32;
    let mut changed = fs::OpenOptions::new().write(true).open(&path).unwrap();
    changed
        .seek(SeekFrom::Start(records + 2 * 96 + 40))
        .unwrap();
    changed
        .write_all(&u64::from(0.9_f32.to_bits()).to_le_bytes())
        .unwrap();
    changed.sync_all().unwrap();
    let damaged = fs::read(&path).unwrap();
    assert!(
        source
            .read_projected_spans_on_control(&destination, &mut cursor, 1, &mut output)
            .is_err()
    );
    assert_eq!(output, [None]);
    assert!(
        source
            .read_projected_spans_on_control(&destination, &mut cursor, 1, &mut output)
            .is_err()
    );
    let mut values = [Some(events[0].value); 10];
    assert!(
        source
            .read_projected_values_on_control(destination, 0, &mut values)
            .is_err()
    );
    assert_eq!(values, [None; 10]);
    assert_eq!(fs::read(&path).unwrap(), damaged);
}

#[test]
#[ignore = "manual optimized sequential scalar-range profile; no callback or deadline claim"]
fn profile_projected_scalar_ranges() {
    use hex_project::{Tempo, TempoMap};
    use std::{sync::Arc, time::Instant};
    const EVENTS: u64 = 65_536;
    const PAGES: u64 = 16;
    const FRAMES: usize = 4096;
    let files = Files::new();
    for spacing in [256, 1] {
        let path = files.path(&format!("range-profile-{spacing}"));
        let events: Vec<_> = (0..EVENTS)
            .map(|index| AppliedEvent {
                sequence: index + 1,
                performance_frame: 100 + index * spacing,
                content_frame: index * spacing,
                loop_pass: 0,
                value: AutomationValue::new(((index + 1) % 17) as f32 / 16.0).unwrap(),
            })
            .collect();
        write(
            &path,
            Some(None),
            &events,
            DraftTerminal::Finished(DraftClock {
                performance_frame: 100 + EVENTS * spacing,
                content_frame: EVENTS * spacing,
                loop_pass: 0,
            }),
        );
        let destination =
            Arc::new(TempoMap::constant(48_000, Tempo::new(173_000_000).unwrap()).unwrap());
        let captured = fixture().0.tempo_map().clone();
        let end = destination
            .beat_to_frame(captured.frame_to_beat(EVENTS * spacing).unwrap())
            .unwrap();
        let positions: Vec<Vec<u64>> = (0..PAGES)
            .map(|page| {
                let start = (page * 104_729 + 127) % (end - FRAMES as u64);
                (start..start + FRAMES as u64).collect()
            })
            .collect();
        let mut source = CompletedAutomationTake::open_on_control(&path, limits()).unwrap();
        let expected: Vec<Vec<Option<AutomationValue>>> = positions
            .iter()
            .map(|frames| {
                let mut values = Vec::new();
                for frames in frames.chunks(MAX_PREVIEW_QUERIES) {
                    let mut page = [None; MAX_PREVIEW_QUERIES];
                    source
                        .read_projected_preview_on_control(&destination, frames, &mut page)
                        .unwrap();
                    values.extend(page.into_iter().map(|event| event.map(|e| e.value)));
                }
                values
            })
            .collect();
        let mut range = [None; FRAMES];
        for frames in &positions[..2] {
            source
                .read_projected_values_on_control(Arc::clone(&destination), frames[0], &mut range)
                .unwrap();
        }
        for trial in 0..3 {
            let mut seconds = [0.0; 2];
            for order in 0..2 {
                let sequential = (trial + order) % 2 == 1;
                let start = Instant::now();
                for (frames, expected) in positions.iter().zip(&expected) {
                    if sequential {
                        source
                            .read_projected_values_on_control(
                                Arc::clone(&destination),
                                frames[0],
                                &mut range,
                            )
                            .unwrap();
                        assert_eq!(range.as_slice(), expected);
                    } else {
                        for (frames, expected) in frames
                            .chunks(MAX_PREVIEW_QUERIES)
                            .zip(expected.chunks(MAX_PREVIEW_QUERIES))
                        {
                            let mut preview = [None; MAX_PREVIEW_QUERIES];
                            source
                                .read_projected_preview_on_control(
                                    &destination,
                                    frames,
                                    &mut preview,
                                )
                                .unwrap();
                            for (actual, expected) in preview.iter().zip(expected) {
                                assert_eq!(actual.map(|e| e.value), *expected);
                            }
                        }
                    }
                }
                seconds[usize::from(sequential)] = start.elapsed().as_secs_f64();
            }
            println!(
                "projected_scalar_ranges trial={trial} events={EVENTS} frame_spacing={spacing} measured_pages={PAGES} page_frames={FRAMES} warmup_range_pages=2 alternating_order=true point_preview_seconds={:.6} sequential_range_seconds={:.6} mean_range_page_microseconds={:.3}",
                seconds[0],
                seconds[1],
                seconds[1] * 1e6 / PAGES as f64
            );
        }
    }
}

#[test]
fn early_piece_sentinels_do_not_spend_the_fetch_budget_again_after_discarding_prefetch() {
    use std::sync::Arc;
    let files = Files::new();
    let region = LoopRegion::new(100, 1000, true).unwrap();
    let (events, end, _) = history(0, 0, 1901, Some(region), &(0..1901).collect::<Vec<_>>());
    let path = files.path("prefetch-budget");
    write(
        &path,
        Some(Some(region)),
        &events,
        DraftTerminal::Finished(end),
    );
    let mut source = CompletedAutomationTake::open_on_control(&path, limits()).unwrap();
    let destination = Arc::new(source.source().tempo_map().clone());
    // Three effective pieces: intro99..100, final pass100..101, preceding
    // pass101..102. The first/last need only one sentinel from a larger batch.
    let mut cursor = source
        .begin_projected_spans_on_control(Arc::clone(&destination), 99..102)
        .unwrap();
    let mut output = [None; MAX_SPANS_PER_STEP];
    let mut actual = [None; 3];
    let mut finished = false;
    for _ in 0..4 {
        let progress = source
            .read_projected_spans_on_control(&destination, &mut cursor, 64, &mut output)
            .unwrap();
        assert!(
            progress.records_read <= 64,
            "fetched {} data records with budget64",
            progress.records_read
        );
        assert!(progress.records_visited <= 64);
        for span in output[..progress.written].iter().flatten() {
            actual[(span.start - 99) as usize..(span.end - 99) as usize].fill(Some(span.value));
        }
        if progress.state == SpanReadState::Finished {
            finished = true;
            break;
        }
    }
    assert!(finished);
    let mut expected = [None; 3];
    source
        .read_projected_preview_on_control(&destination, &[99, 100, 101], &mut expected)
        .unwrap();
    assert_eq!(actual, expected.map(|event| event.map(|e| e.value)));
}
