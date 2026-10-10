//! Original standalone Operator comparison; no native callback/deadline claim.
use hex_devices::{
    SineTable,
    fm4::{Oscillator, PreparedOperatorConfiguration, PreparedOperatorFm4, Routing},
    operator_envelope::OperatorEnvelope,
};
use std::{error::Error, hint::black_box, time::Instant};

fn measure(label: &str, render: impl Fn(u64) -> [f32; 128]) {
    for block in 0..100 {
        black_box(render(block * 128));
    }
    let mut times = Vec::with_capacity(2000);
    let start = Instant::now();
    for block in 100..2100 {
        let begin = Instant::now();
        black_box(render(block * 128));
        times.push(begin.elapsed());
    }
    let total = start.elapsed();
    times.sort_unstable();
    println!(
        "{label}: total={:.3}s p50/p95/p99/max={:.1}/{:.1}/{:.1}/{:.1}us",
        total.as_secs_f64(),
        times[1000].as_secs_f64() * 1e6,
        times[1900].as_secs_f64() * 1e6,
        times[1980].as_secs_f64() * 1e6,
        times[1999].as_secs_f64() * 1e6
    );
}

fn main() -> Result<(), Box<dyn Error>> {
    let table = SineTable::new();
    let mut matrix = [[0.0; 4]; 4];
    matrix[3][2] = 0.05;
    matrix[2][1] = 0.1;
    matrix[1][0] = 0.2;
    let routing = Routing::new(matrix, [1.0, 0.0, 0.0, 0.0])?;
    let oscillators = [Oscillator::new(1.0, 0.25, 0.0)?; 4];
    let envelopes = [OperatorEnvelope::new(0.0, 1.0, 0.5, 0.005, 0.1, 0.05)?; 4];
    let context = PreparedOperatorConfiguration::new(48000, oscillators, routing, envelopes)?;
    let full: Vec<_> = (0..100)
        .map(|i| PreparedOperatorFm4::new(48000, 60 + i % 12, 100, oscillators, routing, envelopes))
        .collect::<Result<_, _>>()?;
    let compact: Vec<_> = (0..100)
        .map(|i| context.prepare_note(60 + i % 12, 100))
        .collect::<Result<_, _>>()?;
    let full_render = |age: u64| {
        let mut block = [0.0; 128];
        for note in &full {
            for (offset, out) in block.iter_mut().enumerate() {
                *out += note.sample(&table, age + offset as u64, u64::MAX);
            }
        }
        block
    };
    let compact_render = |age: u64| {
        let mut block = [0.0; 128];
        for note in &compact {
            for (offset, out) in block.iter_mut().enumerate() {
                *out += context.sample(&table, note, age + offset as u64, u64::MAX);
            }
        }
        block
    };
    // Complete measured-range comparison is outside the timing region.
    for block in 0..2100 {
        if full_render(block * 128) != compact_render(block * 128) {
            return Err("compact Operator PCM differs from standalone kernel".into());
        }
    }
    println!(
        "48000hz, 100 held voices, 4-stage, 128frames, 100warmups, 2000blocks; full/compact PCM exact"
    );
    println!(
        "full note={}B; shared context={}B, compact note={}B",
        size_of::<PreparedOperatorFm4>(),
        size_of::<PreparedOperatorConfiguration>(),
        size_of_val(&compact[0])
    );
    measure("standalone", full_render);
    measure("compact", compact_render);
    Ok(())
}
