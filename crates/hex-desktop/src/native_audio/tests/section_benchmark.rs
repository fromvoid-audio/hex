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
//! ```text
//! HEX_SECTION_BENCH_PROJECT=/path/to/project.hex [HEX_SECTION_BENCH_REPS=5] \
//!   cargo test --release -p hex-desktop --features allocation-probe --bin hex-desktop \
//!   section_edit_benchmark -- --ignored --nocapture
//! ```
//!
//! The project needs at least two song sections; the original is only read.
use super::resident_preview::ResidentHarness;
use super::resident_sections::run_callback;
use super::*;
use crate::audio_policy::MAX_RESIDENT_FRAMES;
use hex_project::{BeatTime, Command, ElementColor, SectionId, TrimEdge};
use hex_session::service::{
    ProjectSource,
    diagnostics::{DiagnosticObserver, DiagnosticStage},
};

const PHASES: [&str; 6] = [
    "worker_us",
    "section_fence_us",
    "section_edit_us",
    "section_prepare_us",
    "section_publish_us",
    "section_finish_us",
];

/// One completed request: client-observed total and worker phase fields.
struct Sample {
    total: Duration,
    phases: [Option<u64>; 6],
}

fn parse_phases(details: &str) -> [Option<u64>; 6] {
    PHASES.map(|name| {
        details.split_whitespace().find_map(|field| {
            field
                .strip_prefix(name)
                .and_then(|rest| rest.strip_prefix('='))
                .and_then(|value| value.parse().ok())
        })
    })
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
        if let Ok(event) = h.service.try_event()
            && event.project_replaced
        {
            assert!(event.error.is_none(), "{:?}", event.error);
            return event;
        }
        assert!(Instant::now() < until, "project open timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn wait_applied(h: &mut ResidentHarness) {
    let until = Instant::now() + Duration::from_secs(300);
    loop {
        h.pump();
        if h.audio
            .as_ref()
            .and_then(EditorAudio::status)
            .is_some_and(|status| status.applied_revision.load(Acquire) > 0)
        {
            return;
        }
        if h.callback.lock().unwrap().is_some() {
            h.render();
        }
        assert!(Instant::now() < until, "resident project never applied");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn position(h: &mut ResidentHarness, frame: u64, playing: bool) {
    let audio = h.audio.as_mut().unwrap();
    audio.control.command(PlaybackCommand::Pause).unwrap();
    audio.control.command(PlaybackCommand::Seek(frame)).unwrap();
    if playing {
        audio.control.command(PlaybackCommand::Play).unwrap();
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
    let started = Instant::now();
    let event = run_callback(h, request);
    let total = started.elapsed();
    if let Some(error) = &event.error {
        return Err(error.clone());
    }
    let details = outcomes.lock().unwrap().last().cloned().unwrap_or_default();
    Ok((
        event,
        Sample {
            total,
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
        "{label:<28} n={} total_ms={}",
        samples.len(),
        stat(samples.iter().map(|s| s.total.as_micros()).collect())
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
    fs::copy(&original, &copy).unwrap();
    let mut permissions = fs::metadata(&copy).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&copy, permissions).unwrap();

    let mut h = ResidentHarness::with_frame_limit_at_rate(MAX_RESIDENT_FRAMES, 44_100);
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
    let loaded = Instant::now();
    let mut event = open_copy(&mut h, copy.clone());
    wait_applied(&mut h);
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
                    let edit = match timed(&mut h, &outcomes, request) {
                        Ok(edit) => edit,
                        Err(error) => {
                            failure = Some(error);
                            break;
                        }
                    };
                    let undo = timed(&mut h, &outcomes, Request::Undo).expect("undo");
                    assert_eq!(sections(&undo.0), song, "undo restores the song");
                    event = undo.0;
                    if rep > 0 {
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
    let audio = h.audio.as_mut().unwrap();
    audio.control.command(PlaybackCommand::Stop).unwrap();
    h.render();
    let mut permissions = fs::metadata(&copy).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    let _ = fs::set_permissions(&copy, permissions);
    let _ = fs::remove_dir_all(&directory);
}
