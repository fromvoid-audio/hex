//! Serial kernel timing distributions; this does not run a device callback.
use std::{hint::black_box, time::Instant};

use hex_engine::{
    MidiSpanBuffer, PreparedMidiInstrumentTemplate, PreparedMidiSourceTemplate, PreparedTempoMap,
};
use hex_project::{BeatTime, Command, Created, Editor, Project};

const RATE: u32 = 48_000;
const FRAMES: usize = 64;
const SOURCES: usize = 100;
const BLOCKS: usize = 10_000;
const WARMUPS: usize = 100;

struct Workload {
    name: &'static str,
    timing: PreparedTempoMap,
    table: hex_devices::SineTable,
    templates: Vec<PreparedMidiInstrumentTemplate>,
    windows: Vec<u64>,
}

impl Workload {
    fn prepare(
        name: &'static str,
        voices: u64,
        looping: bool,
        gate_beats: u64,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut editor = Editor::new(Project::new(RATE)?, 8)?;
        let track = match editor.execute(Command::AddMidiTrack { name: name.into() })? {
            Created::Track(track) => track,
            _ => return Err("expected instrument track".into()),
        };
        let extent = BeatTime::from_quarters(4)?;
        // Dense repeats include the actual intro chord and its first release
        // seam; one-sided tail admission must not count imaginary prior cycles.
        let source_start = BeatTime::ZERO;
        let clip = match editor.execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: extent,
        })? {
            Created::Clip(clip) => clip,
            _ => return Err("expected midi clip".into()),
        };
        editor.execute(Command::SetMidiClipSource {
            clip,
            length: extent,
            source_length: extent,
            source_start,
            loop_start: BeatTime::ZERO,
            loop_end: extent,
            looping,
        })?;
        for voice in 0..voices {
            editor.execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_quarters(gate_beats)?,
                pitch: 48 + (voice % 24) as u8,
                channel: 0,
                velocity: 100,
            })?;
        }
        let project = editor.project();
        let timing = PreparedTempoMap::prepare(project.tempo_map().clone(), RATE)?;
        let source = PreparedMidiSourceTemplate::prepare(
            track,
            project.midi_clip(clip).ok_or("missing midi clip")?,
            &timing,
        )?;
        let table = hex_devices::SineTable::new();
        let mut templates = Vec::new();
        templates.try_reserve_exact(SOURCES)?;
        for _ in 0..SOURCES {
            templates.push(PreparedMidiInstrumentTemplate::prepare(&source, &table)?);
        }
        let period = timing.beat_to_frame(extent)?;
        let gate = timing.beat_to_frame(BeatTime::from_quarters(gate_beats)?)?;
        let first_pass =
            timing.beat_to_frame(BeatTime::from_raw(extent.raw() - source_start.raw()))?;
        let windows = if looping {
            // Include held notes, gate/release transition, and later loop seams.
            vec![
                first_pass + 3_000,
                first_pass + gate - FRAMES as u64,
                first_pass + gate,
                first_pass + period,
                first_pass + period + 64,
                first_pass + period * 2 - 64,
                first_pass + period * 2,
                first_pass + period * 7 + 128,
            ]
        } else {
            vec![3_000, 24_000, 48_000, 72_000, gate - 64, gate, gate + 1_000]
        };
        Ok(Self {
            name,
            timing,
            table,
            templates,
            windows,
        })
    }

    fn render(
        &self,
        frame: u64,
        mixed: &mut [[f32; 2]; FRAMES],
        scratch: &mut [[f32; 2]; FRAMES],
        buffer: &mut MidiSpanBuffer,
    ) -> Result<usize, hex_engine::MidiTemplateError> {
        mixed.fill([0.0; 2]);
        let mut spans = 0;
        for template in &self.templates {
            black_box(template).render(
                BeatTime::ZERO,
                &self.timing,
                &self.table,
                frame,
                scratch,
                buffer,
            )?;
            spans += buffer.len();
            for (output, input) in mixed.iter_mut().zip(scratch.iter()) {
                output[0] += input[0];
                output[1] += input[1];
            }
        }
        Ok(spans)
    }

    fn measure(&self) -> Result<(), Box<dyn std::error::Error>> {
        let mut buffer = MidiSpanBuffer::default();
        let mut mixed = [[0.0; 2]; FRAMES];
        let mut scratch = [[0.0; 2]; FRAMES];
        for index in 0..WARMUPS {
            self.render(
                self.windows[index % self.windows.len()],
                &mut mixed,
                &mut scratch,
                &mut buffer,
            )?;
            black_box(&mixed);
        }
        let mut durations = Vec::new();
        durations.try_reserve_exact(BLOCKS)?;
        let mut checksum = 0_u64;
        let mut min_spans = usize::MAX;
        let mut max_spans = 0;
        let deadline_ns = FRAMES as u128 * 1_000_000_000 / u128::from(RATE);
        let mut over_period = 0;
        let wall_start = Instant::now();
        for index in 0..BLOCKS {
            let frame = black_box(self.windows[index % self.windows.len()]);
            let start = Instant::now();
            let spans = self.render(frame, &mut mixed, &mut scratch, &mut buffer)?;
            let elapsed = start.elapsed().as_nanos();
            durations.push(elapsed);
            over_period += usize::from(elapsed > deadline_ns);
            min_spans = min_spans.min(spans);
            max_spans = max_spans.max(spans);
            checksum =
                checksum.wrapping_add(u64::from(black_box(mixed[index % FRAMES][0]).to_bits()));
        }
        let wall = wall_start.elapsed();
        durations.sort_unstable();
        let percentile = |percent: usize| -> Result<u128, Box<dyn std::error::Error>> {
            let index = (BLOCKS * percent).div_ceil(100).saturating_sub(1);
            durations
                .get(index)
                .copied()
                .ok_or_else(|| "missing timing sample".into())
        };
        let minimum = durations.first().copied().ok_or("missing minimum timing")?;
        let maximum = durations.last().copied().ok_or("missing maximum timing")?;
        println!(
            "workload={} sources={SOURCES} frames={FRAMES} blocks={BLOCKS} min_spans_per_block={min_spans} max_spans_per_block={max_spans} wall_ms={:.3} min_us={:.3} p50_us={:.3} p95_us={:.3} p99_us={:.3} max_us={:.3} above_1333us_period={over_period} checksum={checksum}",
            self.name,
            wall.as_secs_f64() * 1000.0,
            minimum as f64 / 1000.0,
            percentile(50)? as f64 / 1000.0,
            percentile(95)? as f64 / 1000.0,
            percentile(99)? as f64 / 1000.0,
            maximum as f64 / 1000.0
        );
        Ok(())
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "serial original sine kernel; repeated immutable source templates, not a canonical 100-track graph. setup and {WARMUPS} warmups per workload excluded; per-block Instant timing includes render and scratch mixing. no plugins, device callback, graph publication or allocation instrumentation"
    );
    for workload in [
        Workload::prepare("four-voice-loop", 4, true, 2)?,
        Workload::prepare("64-held-voice", 64, false, 4)?,
        Workload::prepare("32-voice-loop-seam", 32, true, 4)?,
    ] {
        workload.measure()?;
    }
    Ok(())
}
