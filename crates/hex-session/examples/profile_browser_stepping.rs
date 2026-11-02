//! Browser stepping profile: selects large WAV files one after another at a
//! fixed interval and reports how long the last selection waits for its
//! waveform and audition PCM. No device, GPUI or realtime claim.
//!
//! Each selection requests a waveform and an audition preparation. Two lanes
//! model `hex-ui`'s browser scheduler: each runs one job at a time and keeps
//! only the newest pending request. `--mode shared` uses [`PreviewSources`]
//! (one shared decode per file, superseded jobs cancelled, as after #3952).
//! `--mode legacy` reproduces the pre-#3952 call sequence with the same
//! hex-media decode and V1 conversion: every job decodes and converts the whole
//! file, uncancellably, and the waveform summarises the converted PCM.
//!
//! Run each mode in its own process so peak RSS stays separate, for example:
//! `/usr/bin/time -l cargo run --release -p hex-session --example
//! profile_browser_stepping -- --dir <scratch> --mode shared`.
//! Fixtures are generated into `--dir` when missing and are left in place for
//! the other mode; delete the directory afterwards.
use hex_media::{Limits, ResampleLimits, decode_wav_file, resample, waveform::summarize_peaks};
use hex_session::service::{PreviewFailure, PreviewSources};
use std::{
    env, fs,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

const OUTPUT_RATE: u32 = 48_000;
const PREVIEW_FRAME_LIMIT: usize = 32 * 1024 * 1024;
const PREVIEW_FILE_LIMIT: usize = 512 * 1024 * 1024;
/// Stereo 32-bit float fixtures, 192–248 MiB each, at mixed rates. Each fits
/// the 33,554,432-frame preview cap before and after conversion to 48 kHz.
const FIXTURES: [(u32, usize); 8] = [
    (44_100, 25_165_824),
    (48_000, 27_262_976),
    (96_000, 30_408_704),
    (44_100, 26_214_400),
    (88_200, 29_360_128),
    (48_000, 25_165_824),
    (96_000, 32_505_856),
    (44_100, 28_311_552),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Shared,
    Legacy,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Job {
    Waveform,
    Audition,
}

/// One preview kind on the model UI thread: one in-flight job's cancellation
/// flag and the newest pending selection.
#[derive(Default)]
struct Lane {
    in_flight: Option<Arc<AtomicBool>>,
    pending: Option<usize>,
}

struct Finished {
    job: Job,
    index: usize,
    outcome: &'static str,
    at: Instant,
}

struct Scheduler {
    mode: Mode,
    paths: Vec<PathBuf>,
    sources: PreviewSources,
    finished: mpsc::Sender<Finished>,
    lanes: [Lane; 2],
}

impl Scheduler {
    /// A selection requests both previews; a busy lane keeps only the newest
    /// request and, after #3952, cancels the superseded in-flight job.
    fn select(&mut self, index: usize) {
        for job in [Job::Waveform, Job::Audition] {
            let lane = &mut self.lanes[job as usize];
            if let Some(cancel) = &lane.in_flight {
                lane.pending = Some(index);
                if self.mode == Mode::Shared {
                    cancel.store(true, Ordering::Relaxed);
                }
            } else {
                self.start(job, index);
            }
        }
    }

    fn finish(&mut self, job: Job) {
        let lane = &mut self.lanes[job as usize];
        lane.in_flight = None;
        if let Some(index) = lane.pending.take() {
            self.start(job, index);
        }
    }

    fn start(&mut self, job: Job, index: usize) {
        let cancel = Arc::new(AtomicBool::new(false));
        self.lanes[job as usize].in_flight = Some(Arc::clone(&cancel));
        let (mode, path, sources, finished) = (
            self.mode,
            self.paths[index].clone(),
            self.sources.clone(),
            self.finished.clone(),
        );
        thread::spawn(move || {
            let outcome = match mode {
                Mode::Shared => shared(job, &sources, &path, &cancel),
                Mode::Legacy => legacy(job, &path),
            };
            let _ = finished.send(Finished {
                job,
                index,
                outcome,
                at: Instant::now(),
            });
        });
    }
}

fn shared(job: Job, sources: &PreviewSources, path: &Path, cancel: &AtomicBool) -> &'static str {
    let result = match job {
        Job::Waveform => sources
            .prepare_waveform(path, OUTPUT_RATE, cancel)
            .map(drop),
        Job::Audition => sources
            .prepare_playback(path, OUTPUT_RATE, cancel)
            .map(drop),
    };
    match result {
        Ok(()) => "ready",
        Err(PreviewFailure::Cancelled) => "cancelled",
        Err(PreviewFailure::Failed(_)) => "failed",
    }
}

/// The pre-#3952 jobs: whole decode and V1 conversion, never cancelled.
fn legacy(job: Job, path: &Path) -> &'static str {
    let Ok(decoded) = decode_wav_file(path, Limits::new(PREVIEW_FILE_LIMIT, PREVIEW_FRAME_LIMIT))
    else {
        return "failed";
    };
    let limits = ResampleLimits {
        max_source_frames: PREVIEW_FRAME_LIMIT,
        max_output_frames: PREVIEW_FRAME_LIMIT,
    };
    let Ok(audio) = resample(decoded.audio(), OUTPUT_RATE, limits) else {
        return "failed";
    };
    if job == Job::Waveform && summarize_peaks(&audio, 4096).is_err() {
        return "failed";
    }
    "ready"
}

fn main() {
    let mut dir = None;
    let mut mode = Mode::Shared;
    let mut interval = Duration::from_millis(150);
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dir" => dir = args.next().map(PathBuf::from),
            "--mode" => {
                mode = match args.next().as_deref() {
                    Some("shared") => Mode::Shared,
                    Some("legacy") => Mode::Legacy,
                    other => panic!("unknown mode {other:?}"),
                }
            }
            "--interval-ms" => {
                interval = Duration::from_millis(
                    args.next()
                        .and_then(|value| value.parse().ok())
                        .expect("interval in milliseconds"),
                )
            }
            other => panic!("unknown argument {other}"),
        }
    }
    let paths = fixtures(&dir.expect("--dir <fixture directory>"));
    let count = paths.len();
    println!(
        "mode {}; {count} files; step {} ms; output {OUTPUT_RATE} hz",
        if mode == Mode::Shared {
            "shared"
        } else {
            "legacy"
        },
        interval.as_millis()
    );
    let (finished, events) = mpsc::channel();
    let mut scheduler = Scheduler {
        mode,
        paths,
        sources: PreviewSources::default(),
        finished,
        lanes: Default::default(),
    };
    let start = Instant::now();
    let mut next = 0;
    let mut last_selected = start;
    let mut last_ready = [None, None];
    while last_ready.iter().any(Option::is_none) {
        let timeout = if next < count {
            (start + interval * next as u32).saturating_duration_since(Instant::now())
        } else {
            Duration::from_secs(600)
        };
        match events.recv_timeout(timeout) {
            Ok(event) => {
                println!(
                    "{:>8.3} s  {:?} #{} {}",
                    (event.at - start).as_secs_f64(),
                    event.job,
                    event.index,
                    event.outcome
                );
                if event.index + 1 == count {
                    last_ready[event.job as usize] = Some(event.at - last_selected);
                }
                scheduler.finish(event.job);
            }
            Err(mpsc::RecvTimeoutError::Timeout) if next < count => {
                last_selected = Instant::now();
                scheduler.select(next);
                next += 1;
            }
            Err(error) => panic!("profile workers stopped: {error}"),
        }
    }
    for (job, label) in [(Job::Waveform, "waveform"), (Job::Audition, "audition")] {
        if let Some(wait) = last_ready[job as usize] {
            println!(
                "last selection time-to-{label}: {:.3} s",
                wait.as_secs_f64()
            );
        }
    }
}

/// Deterministic stereo f32 fixtures; existing files of the right size are reused.
fn fixtures(dir: &Path) -> Vec<PathBuf> {
    fs::create_dir_all(dir).expect("fixture directory");
    FIXTURES
        .iter()
        .enumerate()
        .map(|(index, &(rate, frames))| {
            let path = dir.join(format!("step-{index}-{rate}.wav"));
            let expected = 44 + frames as u64 * 8;
            if fs::metadata(&path).map(|meta| meta.len()).ok() != Some(expected) {
                write_fixture(&path, rate, frames);
            }
            path
        })
        .collect()
}

fn write_fixture(path: &Path, rate: u32, frames: usize) {
    let data_bytes = u32::try_from(frames * 8).expect("fixture fits riff");
    let mut file = BufWriter::new(fs::File::create(path).expect("create fixture"));
    let mut header = Vec::from(*b"RIFF");
    header.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    header.extend_from_slice(b"WAVEfmt ");
    header.extend_from_slice(&16u32.to_le_bytes());
    header.extend_from_slice(&3u16.to_le_bytes());
    header.extend_from_slice(&2u16.to_le_bytes());
    header.extend_from_slice(&rate.to_le_bytes());
    header.extend_from_slice(&(rate * 8).to_le_bytes());
    header.extend_from_slice(&8u16.to_le_bytes());
    header.extend_from_slice(&32u16.to_le_bytes());
    header.extend_from_slice(b"data");
    header.extend_from_slice(&data_bytes.to_le_bytes());
    file.write_all(&header).expect("write header");
    let step = 440.0 * std::f32::consts::TAU / rate as f32;
    for frame in 0..frames {
        let sample = ((frame % rate as usize) as f32 * step).sin() * 0.25;
        file.write_all(&sample.to_le_bytes()).expect("write left");
        file.write_all(&(-sample).to_le_bytes())
            .expect("write right");
    }
    file.flush().expect("flush fixture");
}
