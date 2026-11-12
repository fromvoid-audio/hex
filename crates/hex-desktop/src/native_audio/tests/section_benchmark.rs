//! Native-headless section-edit benchmark (Pulse #3819), not an ordinary test.
//!
//! Runs the production Session worker, resident publication and headless native
//! callback over a temporary read-only copy of a saved project. Each operation
//! is applied and then undone, at an early and a late cursor, stopped and
//! playing. Totals are submit to client receipt on the test thread; phases are
//! the worker's `action_diag` `OutcomeReady` fields (`worker_us`,
//! `section_*_us`). The callback is rendered in 64-frame blocks with 1 ms
//! sleeps, so fence waits approximate, but are not, a physical device cadence.
//! UI install, paint and present are not measured here.
//!
//! Run the generated reference workload in a fresh directory.
//!
//! ```sh
//! fixture_dir="$(mktemp -d)/section-fixture"
//! cargo run --release --locked -p hex-session --example create_section_benchmark -- "$fixture_dir"
//! HEX_SECTION_BENCH_PROJECT="$fixture_dir/project.hex" HEX_SECTION_BENCH_REPS=7 \
//!   cargo test --release --locked -p hex-desktop --features allocation-probe --bin hex-desktop \
//!   -- native_audio::tests::section_benchmark::section_edit_benchmark \
//!   --ignored --exact --nocapture --test-threads=1
//! ```
//!
//! A supplied saved project needs at least two song sections. The benchmark
//! reads the original and applies edits to its temporary copy.
//! `HEX_SECTION_BENCH_MEDIA_REMAP` rewrites one absolute media-path prefix in
//! the temporary copy when the project's library has moved.
//! `HEX_SECTION_BENCH_RECOVERY=1` enables a temporary recovery journal;
//! `HEX_SECTION_BENCH_DUPLICATE_ONLY=1` limits the run to an early, playing
//! section copy and its undo. Applied timing is the first observed matching
//! callback revision, not a physical-device or UI measurement.
use super::resident_preview::ResidentHarness;
use super::*;
use crate::audio_policy::MAX_RESIDENT_FRAMES;
use hex_project::{BeatTime, Command, ElementColor, SectionId, TrimEdge};
use hex_session::service::{
    ProjectSource,
    diagnostics::{DiagnosticObserver, DiagnosticStage},
};

const PHASES: [&str; 8] = [
    "worker_us",
    "section_fence_us",
    "section_edit_us",
    "section_prepare_us",
    "section_publish_us",
    "section_finish_us",
    "section_view_us",
    "duplicate_recovery_us",
];

/// One completed request: client-observed total and worker phase fields.
struct Sample {
    total: Duration,
    applied: Option<Duration>,
    phases: [Option<u64>; 8],
}

fn parse_phases(details: &str) -> [Option<u64>; 8] {
    PHASES.map(|name| {
        details.split_whitespace().find_map(|field| {
            field
                .strip_prefix(name)
                .and_then(|rest| rest.strip_prefix('='))
                .and_then(|value| value.parse().ok())
        })
    })
}

#[test]
fn section_benchmark_parses_view_phase() {
    let phases = parse_phases("worker_us=20 section_view_us=7 duplicate_recovery_us=11");
    assert_eq!(phases[0], Some(20));
    assert_eq!(phases[6], Some(7));
    assert_eq!(phases[1], None);
    assert_eq!(phases[7], Some(11));
}

fn open_copy(h: &mut ResidentHarness, path: PathBuf) -> Event {
    h.service
        .submit(Request::OpenWithAudioMode {
            source: ProjectSource::Saved(path),
            relinks: Default::default(),
            mode: DocumentAudioMode::Resident,
            reservation: None,
        })
        .unwrap();
    let until = Instant::now() + Duration::from_secs(300);
    loop {
        h.pump();
        if let Ok(event) = h.service.try_event() {
            assert!(event.error.is_none(), "open refused: {:?}", event.error);
            assert!(
                event.recovery_failure.is_none(),
                "open needs recovery: {:?}",
                event.recovery_failure
            );
            if event.project_replaced {
                return event;
            }
        }
        assert!(Instant::now() < until, "project open timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn wait_applied(h: &mut ResidentHarness, after: u64) {
    let until = Instant::now() + Duration::from_secs(120);
    loop {
        h.pump();
        let status = h.audio.as_ref().and_then(EditorAudio::status);
        if let Some((rejected, reason)) = status.and_then(|s| s.rejected_revision()) {
            panic!("resident graph rejected revision {rejected}: {reason:?}");
        }
        if h.submitted.is_some_and(|(submitted, _)| {
            submitted > after
                && status.is_some_and(|s| s.applied_revision.load(Acquire) == submitted)
        }) {
            return;
        }
        if h.callback.lock().unwrap().is_some() {
            h.render();
        }
        if let Ok(event) = h.service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
        }
        assert!(
            Instant::now() < until,
            "resident project after {after} never applied: audio={} submitted={:?} applied={:?} unavailable={:?} refused={:?}",
            h.audio.is_some(),
            h.submitted.as_ref().map(|(r, _)| *r),
            status.map(|s| s.applied_revision.load(Acquire)),
            h.unavailable,
            h.publication_refusals,
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn applied(h: &ResidentHarness) -> u64 {
    h.audio
        .as_ref()
        .and_then(EditorAudio::status)
        .map_or(0, |status| status.applied_revision.load(Acquire))
}

/// Queue one transport command, rendering until the bounded queue accepts it.
fn command(h: &mut ResidentHarness, command: PlaybackCommand) {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let control = &mut h.audio.as_mut().unwrap().control;
        // The app's UI drains action receipts; the bounded ledger refuses
        // new commands while its 32 receipts are unread.
        if let Some(reader) = control.action_outcomes_mut() {
            while reader.try_take().is_some() {}
        }
        if control.command(command).is_ok() {
            return;
        }
        h.render();
        assert!(Instant::now() < until, "transport queue stayed full");
    }
}

fn position(h: &mut ResidentHarness, frame: u64, playing: bool) {
    command(h, PlaybackCommand::Pause);
    command(h, PlaybackCommand::Seek(frame));
    if playing {
        command(h, PlaybackCommand::Play);
    }
    for _ in 0..16 {
        h.render();
    }
}

fn timed(
    h: &mut ResidentHarness,
    outcomes: &Mutex<Vec<String>>,
    request: Request,
) -> Result<(Event, Sample), String> {
    outcomes.lock().unwrap().clear();
    let previous = applied(h);
    let started = Instant::now();
    h.service
        .submit(request)
        .map_err(|error| format!("{error:?}"))?;
    let mut installed = None;
    let event = loop {
        h.pump();
        h.render();
        if installed.is_none()
            && h.submitted
                .is_some_and(|(revision, _)| revision > previous && applied(h) == revision)
        {
            installed = Some(started.elapsed());
        }
        if let Ok(event) = h.service.try_event() {
            break event;
        }
        if started.elapsed() >= Duration::from_secs(10) {
            return Err("section worker did not complete".into());
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    let total = started.elapsed();
    if let Some(error) = &event.error {
        return Err(error.clone());
    }
    let details = outcomes.lock().unwrap().last().cloned().unwrap_or_default();
    Ok((
        event,
        Sample {
            total,
            applied: installed,
            phases: parse_phases(&details),
        },
    ))
}

fn summarize(label: &str, samples: &[Sample]) {
    let stat = |mut values: Vec<u128>| -> String {
        if values.is_empty() {
            return "-".into();
        }
        values.sort_unstable();
        let median = values[values.len() / 2];
        let max = values[values.len() - 1];
        format!("{:.1}/{:.1}", median as f64 / 1000.0, max as f64 / 1000.0)
    };
    let mut line = format!(
        "{label:<28} n={} total_ms={} applied_ms={}",
        samples.len(),
        stat(samples.iter().map(|s| s.total.as_micros()).collect()),
        stat(
            samples
                .iter()
                .filter_map(|s| s.applied.map(|value| value.as_micros()))
                .collect()
        ),
    );
    for (index, name) in PHASES.iter().enumerate() {
        let values = samples
            .iter()
            .filter_map(|s| s.phases[index].map(u128::from))
            .collect();
        let name = name.trim_start_matches("section_").trim_end_matches("_us");
        line.push_str(&format!(" {name}_ms={}", stat(values)));
    }
    eprintln!("{line}");
}

fn sections(event: &Event) -> Vec<(SectionId, BeatTime, BeatTime)> {
    event
        .state
        .project
        .arrangement_sections()
        .iter()
        .map(|s| (s.id(), s.start(), s.end()))
        .collect()
}

#[test]
#[ignore = "benchmark: requires HEX_SECTION_BENCH_PROJECT and a release build"]
fn section_edit_benchmark() {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    let original = PathBuf::from(
        std::env::var_os("HEX_SECTION_BENCH_PROJECT").expect("HEX_SECTION_BENCH_PROJECT"),
    );
    let reps: usize = std::env::var("HEX_SECTION_BENCH_REPS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(3);
    let directory = std::env::temp_dir().join(format!("hex-section-bench-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let copy = directory.join("project.hex");
    let mut text = fs::read_to_string(&original).unwrap();
    if let Ok(remap) = std::env::var("HEX_SECTION_BENCH_MEDIA_REMAP") {
        let (from, to) = remap.split_once('=').expect("remap is FROM=TO");
        text = text.replace(&format!("\"{from}"), &format!("\"{to}"));
    }
    fs::write(&copy, text).unwrap();
    let mut permissions = fs::metadata(&copy).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&copy, permissions).unwrap();

    let recovery = std::env::var_os("HEX_SECTION_BENCH_RECOVERY").is_some();
    let duplicate_only = std::env::var_os("HEX_SECTION_BENCH_DUPLICATE_ONLY").is_some();
    let mut h = if recovery {
        ResidentHarness::with_recovery_at_rate(
            MAX_RESIDENT_FRAMES,
            directory.join("untitled.hex-journal"),
            44_100,
        )
    } else {
        ResidentHarness::with_frame_limit_at_rate(MAX_RESIDENT_FRAMES, 44_100)
    };
    let outcomes = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&outcomes);
    h.service
        .set_diagnostic_observer(Arc::new(DiagnosticObserver::new(move |event| {
            if matches!(event.stage, DiagnosticStage::OutcomeReady { .. }) {
                sink.lock().unwrap().push(event.details.clone());
            }
        })))
        .unwrap();
    assert!(h.next().error.is_none());
    h.applied(0);
    let old_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    let loaded = Instant::now();
    let mut event = open_copy(&mut h, copy.clone());
    super::resident_lifecycle::finish_replacement(&mut h, &event, old_output);
    let song = sections(&event);
    assert!(song.len() >= 2, "benchmark needs at least two sections");
    let tempo = event.state.project.tempo_map().clone();
    eprintln!(
        "section benchmark: sections={} tracks={} rate={} open+apply={:?} reps={reps} (+1 warm-up)",
        song.len(),
        event.state.project.tracks().len(),
        event.state.project.sample_rate(),
        loaded.elapsed(),
    );
    let first = song[0];
    let last = song[song.len() - 1];
    let bar_after = BeatTime::from_raw(
        last.2
            .raw()
            .checked_add(BeatTime::from_quarters(4).unwrap().raw())
            .unwrap(),
    );
    let cursors = [
        ("early", tempo.beat_to_frame(first.1).unwrap()),
        ("late", tempo.beat_to_frame(last.1).unwrap()),
    ];
    let operations: [(&str, Command); 4] = [
        (
            "reorder",
            Command::ReorderSectionAfter {
                section: first.0,
                after: last.0,
            },
        ),
        (
            "duplicate_at",
            Command::DuplicateArrangementSectionAt {
                section: first.0,
                target: last.0,
                after: true,
            },
        ),
        (
            "resize_edge",
            Command::ResizeArrangementSectionEdge {
                section: last.0,
                edge: TrimEdge::Right,
                boundary: bar_after,
            },
        ),
        (
            "add_marker",
            Command::AddArrangementSection {
                name: "bench".into(),
                color: ElementColor::Cyan,
                start: last.2,
                end: bar_after,
            },
        ),
    ];
    for (cursor_name, frame) in cursors {
        for playing in [false, true] {
            let state = if playing { "playing" } else { "stopped" };
            for (name, command) in &operations {
                if duplicate_only && (cursor_name != "early" || !playing || *name != "duplicate_at")
                {
                    continue;
                }
                let mut edits = Vec::new();
                let mut undos = Vec::new();
                let mut failure = None;
                for rep in 0..=reps {
                    position(&mut h, frame, playing);
                    let request = Request::EditArrangementSection {
                        project_generation: event
                            .state
                            .canonical_audio_source
                            .as_ref()
                            .map_or(0, |s| s.context().project_generation),
                        revision: event.state.revision,
                        command: command.clone(),
                    };
                    let before = applied(&h);
                    let edit = match timed(&mut h, &outcomes, request) {
                        Ok(edit) => edit,
                        Err(error) => {
                            failure = Some(error);
                            break;
                        }
                    };
                    // The next request needs the callback to have installed this one.
                    wait_applied(&mut h, before);
                    let before = applied(&h);
                    let undo = timed(&mut h, &outcomes, Request::Undo).expect("undo");
                    assert_eq!(sections(&undo.0), song, "undo restores the song");
                    wait_applied(&mut h, before);
                    event = undo.0;
                    if rep > 0 {
                        if recovery && *name == "duplicate_at" {
                            assert!(
                                edit.1.phases[7].is_some(),
                                "missing duplicate recovery phase"
                            );
                            assert!(
                                edit.1.applied.is_some(),
                                "missing callback application before terminal"
                            );
                        }
                        edits.push(edit.1);
                        undos.push(undo.1);
                    }
                }
                let label = format!("{cursor_name}/{state}/{name}");
                if let Some(error) = failure {
                    eprintln!("{label:<28} refused: {error}");
                    continue;
                }
                summarize(&label, &edits);
                summarize(&format!("{label}/undo"), &undos);
            }
        }
    }
    command(&mut h, PlaybackCommand::Stop);
    h.render();
    let mut permissions = fs::metadata(&copy).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    let _ = fs::set_permissions(&copy, permissions);
    let _ = fs::remove_dir_all(&directory);
}
