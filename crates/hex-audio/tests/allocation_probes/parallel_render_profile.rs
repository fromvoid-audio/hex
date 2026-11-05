//! Opt-in, playback-only scheduling experiment. Each producer owns a real
//! prepared native shard. The collector accepts only the exact requested
//! generation and block and never waits. This does not replace the device
//! callback or establish a live, plugin, or graph-publication policy.
use hex_engine::{
    Audio, DeviceTypeId, Frame, PreparedRevision, ProjectOutputTap, ProjectSignal, SessionPlayback,
    SessionPlaybackCommand, SineTable, default_native_device_state,
};
use hex_lattice::RefuseForeign;
use hex_project::{Command, Created, Editor, Project};
use rtrb::{Consumer, RingBuffer};
use rustix::time::{ClockId, clock_gettime};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const RATE: u32 = 48_000;
const BLOCK: usize = 128;
const TRACKS_PER_SHARD: usize = 24;
const LANES: usize = 2;
const AHEAD: usize = 2;
const WARMUP: usize = 128;
const SAMPLES: usize = 512;
const BLOCKS: usize = WARMUP + SAMPLES;
const GENERATION: u64 = 1;

struct Window {
    generation: u64,
    sequence: usize,
    tracks: [[Frame; BLOCK]; TRACKS_PER_SHARD],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    Stopped,
    Missing,
    Stale,
}

fn collect(
    ready: &mut [Consumer<Window>; LANES],
    stop: &AtomicBool,
    generation: u64,
    sequence: usize,
    output: &mut [Frame; BLOCK],
) -> Result<(), Fault> {
    output.fill([0.0; 2]);
    if stop.load(Ordering::Acquire) {
        return Err(Fault::Stopped);
    }
    if ready.iter().any(|queue| queue.slots() == 0) {
        return Err(Fault::Missing);
    }
    for queue in ready {
        let Ok(window) = queue.pop() else {
            output.fill([0.0; 2]);
            return Err(Fault::Missing);
        };
        if window.generation != generation || window.sequence != sequence {
            output.fill([0.0; 2]);
            return Err(Fault::Stale);
        }
        for track in &window.tracks {
            for (mixed, frame) in output.iter_mut().zip(track) {
                mixed[0] += frame[0];
                mixed[1] += frame[1];
            }
        }
    }
    Ok(())
}

fn cpu_ns(clock: ClockId) -> u128 {
    let now = clock_gettime(clock);
    now.tv_sec as u128 * 1_000_000_000 + now.tv_nsec as u128
}

fn prepare(tracks: usize) -> (SessionPlayback, Vec<u64>) {
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 1).unwrap();
    let Created::Imported { asset, track, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "profile".into(),
            path: "memory.wav".into(),
            source_sample_rate: RATE,
            source_frames: 8192,
            playback_frames: 8192,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("profile asset");
    };
    let imported = editor.project().track(track).unwrap().clips()[0].id();
    editor.execute(Command::RemoveClip(imported)).unwrap();
    let mut ids = Vec::with_capacity(tracks);
    for index in 0..tracks {
        let track = if index == 0 {
            track
        } else {
            let Created::Track(track) = editor
                .execute(Command::AddTrack {
                    name: format!("profile {index}"),
                })
                .unwrap()
            else {
                panic!("profile track");
            };
            track
        };
        ids.push(track.value());
        editor
            .execute(Command::PlaceClip {
                track,
                asset,
                start: 0,
                duration: 8192 * 64,
                source_offset: 0,
                source_length: 8192,
                looping: true,
            })
            .unwrap();
        editor
            .execute(Command::Gain { track, gain: 0.025 })
            .unwrap();
        let second = if index % 4 == 0 {
            "hex.reverb"
        } else {
            "hex.saturator"
        };
        for kind in ["hex.utility", second] {
            editor
                .execute(Command::InsertDevice {
                    track,
                    before: None,
                    state: default_native_device_state(DeviceTypeId::from_static(kind)).unwrap(),
                })
                .unwrap();
        }
    }
    // Use a non-block-aligned Arrangement loop and time-varying source to
    // distinguish a correct replay from a repeated or displaced window.
    editor
        .execute(Command::SetLoopRegion {
            start: 0,
            end: 8197,
            enabled: true,
        })
        .unwrap();
    let frames = (0..8192)
        .map(|frame| {
            let phase = frame as f32 * 0.047;
            [phase.sin() * 0.25, phase.cos() * 0.125]
        })
        .collect();
    let media = BTreeMap::from([(asset, Audio::new(RATE, frames).unwrap())]);
    let revision = Arc::new(
        PreparedRevision::prepare(editor.project(), &media, Arc::new(SineTable::new()), None)
            .unwrap(),
    );
    assert_eq!(revision.arrangement().main_latency_frames(), 11);
    let mut playback = SessionPlayback::new(revision).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    (playback, ids)
}

fn start_worker(
    mut playback: SessionPlayback,
    ids: Vec<u64>,
    stop: Arc<AtomicBool>,
) -> (Consumer<Window>, thread::JoinHandle<u128>) {
    let (mut queue, ready) = RingBuffer::new(AHEAD);
    let worker = thread::spawn(move || {
        let cpu = cpu_ns(ClockId::ThreadCPUTime);
        let mut main = [[0.0; 2]; BLOCK];
        for sequence in 0..BLOCKS {
            while queue.is_full() && !stop.load(Ordering::Acquire) {
                thread::sleep(Duration::from_micros(50));
            }
            if stop.load(Ordering::Acquire) {
                break;
            }
            let mut window = Window {
                generation: GENERATION,
                sequence,
                tracks: [[[0.0; 2]; BLOCK]; TRACKS_PER_SHARD],
            };
            let mut observe = |signal, offset, frames: &[Frame]| {
                if let ProjectSignal::Track(id) = signal {
                    let row = ids.binary_search(&id).unwrap();
                    window.tracks[row][offset..offset + frames.len()].copy_from_slice(frames);
                }
            };
            let mut tap = ProjectOutputTap::new(&ids, &mut observe).unwrap();
            playback
                .render_with_foreign_processors(
                    &mut main,
                    None,
                    None,
                    Some(&mut tap),
                    &mut RefuseForeign,
                )
                .unwrap();
            assert!(queue.push(window).is_ok());
        }
        cpu_ns(ClockId::ThreadCPUTime) - cpu
    });
    (ready, worker)
}

fn cadence(start: Instant, budget: Duration, sequence: usize) {
    if let Some(remaining) =
        (start + budget * sequence as u32).checked_duration_since(Instant::now())
    {
        thread::sleep(remaining);
    }
}

fn report(
    mode: &str,
    times: &mut [Duration],
    budget: Duration,
    process_cpu_ns: u128,
    worker_cpu_ns: [u128; LANES],
    faults: usize,
) {
    times.sort_unstable();
    let percentile = |percent: usize| times[(times.len() - 1) * percent / 100].as_nanos();
    let over_budget = times.iter().filter(|&&duration| duration > budget).count();
    println!(
        "parallel_probe mode={mode} rate={RATE} block={BLOCK} tracks={} warmup={WARMUP} samples={} budget_ns={} p50_ns={} p95_ns={} p99_ns={} misses={over_budget} faults={faults} process_cpu_ns={process_cpu_ns} worker_cpu_ns={worker_cpu_ns:?}",
        TRACKS_PER_SHARD * LANES,
        times.len(),
        budget.as_nanos(),
        percentile(50),
        percentile(95),
        percentile(99),
    );
}

#[test]
#[ignore = "opt-in optimized, paced native multicore experiment"]
fn two_workers_compare_exact_audio_and_total_work() {
    require_release();
    let (mut serial, _) = prepare(TRACKS_PER_SHARD * LANES);
    let (first, first_ids) = prepare(TRACKS_PER_SHARD);
    let (second, second_ids) = prepare(TRACKS_PER_SHARD);
    let budget = Duration::from_nanos(BLOCK as u64 * 1_000_000_000 / u64::from(RATE));
    let mut expected = vec![[[0.0; 2]; BLOCK]; BLOCKS];
    let mut serial_times = Vec::with_capacity(SAMPLES);
    let start = Instant::now();
    let cpu = cpu_ns(ClockId::ProcessCPUTime);
    for (sequence, output) in expected.iter_mut().enumerate() {
        cadence(start, budget, sequence);
        let begin = Instant::now();
        serial.render(output).unwrap();
        if sequence >= WARMUP {
            serial_times.push(begin.elapsed());
        }
    }
    report(
        "serial",
        &mut serial_times,
        budget,
        cpu_ns(ClockId::ProcessCPUTime) - cpu,
        [0; LANES],
        0,
    );
    assert!(
        expected
            .iter()
            .flatten()
            .flatten()
            .any(|sample| *sample != 0.0)
    );

    let stop = Arc::new(AtomicBool::new(false));
    let cpu = cpu_ns(ClockId::ProcessCPUTime);
    let (first_ready, first_join) = start_worker(first, first_ids, Arc::clone(&stop));
    let (second_ready, second_join) = start_worker(second, second_ids, Arc::clone(&stop));
    let mut ready = [first_ready, second_ready];
    let prime_deadline = Instant::now() + Duration::from_secs(2);
    while ready.iter().any(|queue| queue.slots() < AHEAD) {
        if Instant::now() >= prime_deadline {
            stop.store(true, Ordering::Release);
            let results = [first_join.join(), second_join.join()];
            panic!("parallel probe failed to prime: {results:?}");
        }
        thread::sleep(Duration::from_micros(100));
    }
    let start = Instant::now();
    let mut output = [[0.0; 2]; BLOCK];
    let mut times = Vec::with_capacity(SAMPLES);
    let mut fault = None;
    let mut mismatch = None;
    // The experiment deliberately inserts a fixed two-quantum delay on
    // playback. It makes no claim about live or physical monitor latency.
    for callback in 0..BLOCKS + AHEAD {
        cadence(start, budget, callback);
        let Some(sequence) = callback.checked_sub(AHEAD) else {
            output.fill([0.0; 2]);
            continue;
        };
        let begin = Instant::now();
        let result = collect(&mut ready, &stop, GENERATION, sequence, &mut output);
        if sequence >= WARMUP {
            times.push(begin.elapsed());
        }
        if let Err(error) = result {
            fault = Some((sequence, error));
            break;
        }
        if output
            .iter()
            .flatten()
            .zip(expected[sequence].iter().flatten())
            .any(|(actual, expected)| actual.to_bits() != expected.to_bits())
        {
            mismatch = Some(sequence);
            break;
        }
    }
    stop.store(true, Ordering::Release);
    let worker_cpu = [first_join.join(), second_join.join()].map(Result::unwrap);
    if !times.is_empty() {
        report(
            "two_workers_2q",
            &mut times,
            budget,
            cpu_ns(ClockId::ProcessCPUTime) - cpu,
            worker_cpu,
            usize::from(fault.is_some()),
        );
    }
    assert_eq!(fault, None);
    assert_eq!(mismatch, None, "track-order PCM changed");
    assert_eq!(times.len(), SAMPLES);
}

fn require_release() {
    #[cfg(debug_assertions)]
    panic!("run this profile with --release");
}

#[test]
fn collector_refuses_a_stall_stale_generation_and_stop_without_allocating() {
    let (mut a, first) = RingBuffer::new(AHEAD);
    let (mut b, second) = RingBuffer::new(AHEAD);
    let mut ready = [first, second];
    let stop = AtomicBool::new(false);
    let window = |generation, sequence| Window {
        generation,
        sequence,
        tracks: [[[0.25; 2]; BLOCK]; TRACKS_PER_SHARD],
    };
    let mut output = [[9.0; 2]; BLOCK];
    let check = |ready: &mut _, stop: &_, output: &mut _, expected| {
        let count = allocation_counter::measure(|| {
            assert_eq!(collect(ready, stop, GENERATION, 0, output), expected);
        });
        assert_eq!(
            (
                count.count_total,
                count.bytes_total,
                count.count_current,
                count.bytes_current
            ),
            (0, 0, 0, 0)
        );
    };
    a.push(window(GENERATION, 0)).ok().unwrap();
    check(&mut ready, &stop, &mut output, Err(Fault::Missing));
    assert_eq!(output, [[0.0; 2]; BLOCK]);
    b.push(window(GENERATION - 1, 0)).ok().unwrap();
    check(&mut ready, &stop, &mut output, Err(Fault::Stale));
    assert_eq!(output, [[0.0; 2]; BLOCK]);
    a.push(window(GENERATION, 0)).ok().unwrap();
    b.push(window(GENERATION, 1)).ok().unwrap();
    check(&mut ready, &stop, &mut output, Err(Fault::Stale));
    assert_eq!(output, [[0.0; 2]; BLOCK]);
    a.push(window(GENERATION, 0)).ok().unwrap();
    b.push(window(GENERATION, 0)).ok().unwrap();
    check(&mut ready, &stop, &mut output, Ok(()));
    assert_eq!(output, [[12.0; 2]; BLOCK]);
    for sequence in 1..=AHEAD {
        a.push(window(GENERATION, sequence)).ok().unwrap();
        b.push(window(GENERATION, sequence)).ok().unwrap();
    }
    stop.store(true, Ordering::Release);
    check(&mut ready, &stop, &mut output, Err(Fault::Stopped));
    assert_eq!(output, [[0.0; 2]; BLOCK]);
    assert!(ready.iter().all(|queue| queue.slots() == AHEAD));
}
