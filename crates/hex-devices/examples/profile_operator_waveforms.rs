//! Headless profile for the bounded Operator waveform table path.
use hex_devices::{
    SineTable,
    fm4::{Oscillator, PreparedOperatorConfiguration, PreparedOperatorFm4, Routing},
    operator_envelope::OperatorEnvelope,
    operator_waveform::OperatorWaveform,
};
use std::{error::Error, hint::black_box, mem::size_of_val, time::Instant};

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
    let sine = PreparedOperatorConfiguration::new(48_000, oscillators, routing, envelopes)?;
    let sine_waveforms = PreparedOperatorConfiguration::new_with_waveforms(
        48_000,
        oscillators,
        routing,
        envelopes,
        [OperatorWaveform::Sine; 4],
    )?;
    let bank_start = Instant::now();
    let mixed = PreparedOperatorConfiguration::new_with_waveforms(
        48_000,
        oscillators,
        routing,
        envelopes,
        [
            OperatorWaveform::Saw,
            OperatorWaveform::Square,
            OperatorWaveform::Triangle,
            OperatorWaveform::Saw,
        ],
    )?;
    let bank_and_mixed_prepare = bank_start.elapsed();
    let sine_notes: Vec<_> = (0..100)
        .map(|index| sine.prepare_note(60 + index % 12, 100))
        .collect::<Result<_, _>>()?;
    let sine_waveform_notes: Vec<_> = (0..100)
        .map(|index| sine_waveforms.prepare_note(60 + index % 12, 100))
        .collect::<Result<_, _>>()?;
    let mixed_notes: Vec<_> = (0..100)
        .map(|index| mixed.prepare_note(60 + index % 12, 100))
        .collect::<Result<_, _>>()?;
    let mixed_standalone: Vec<_> = (0..100)
        .map(|index| {
            PreparedOperatorFm4::new_with_waveforms(
                48_000,
                60 + index % 12,
                100,
                oscillators,
                routing,
                envelopes,
                [
                    OperatorWaveform::Saw,
                    OperatorWaveform::Square,
                    OperatorWaveform::Triangle,
                    OperatorWaveform::Saw,
                ],
            )
        })
        .collect::<Result<_, _>>()?;
    let sine_render = |age: u64| {
        let mut block = [0.0; 128];
        for note in &sine_notes {
            for (offset, output) in block.iter_mut().enumerate() {
                *output += sine.sample(&table, note, age + offset as u64, u64::MAX);
            }
        }
        block
    };
    let sine_waveforms_render = |age: u64| {
        let mut block = [0.0; 128];
        for note in &sine_waveform_notes {
            for (offset, output) in block.iter_mut().enumerate() {
                *output += sine_waveforms.sample(&table, note, age + offset as u64, u64::MAX);
            }
        }
        block
    };
    let mixed_render = |age: u64| {
        let mut block = [0.0; 128];
        for note in &mixed_notes {
            for (offset, output) in block.iter_mut().enumerate() {
                *output += mixed.sample(&table, note, age + offset as u64, u64::MAX);
            }
        }
        block
    };
    let mixed_standalone_render = |age: u64| {
        let mut block = [0.0; 128];
        for note in &mixed_standalone {
            for (offset, output) in block.iter_mut().enumerate() {
                *output += note.sample(&table, age + offset as u64, u64::MAX);
            }
        }
        block
    };
    for block in 0..2100 {
        let age = block * 128;
        if sine_render(age) != sine_waveforms_render(age) {
            return Err("sine selector changed exact Operator PCM".into());
        }
        if mixed_render(age) != mixed_standalone_render(age) {
            return Err("mixed compact Operator PCM differs from standalone".into());
        }
    }
    println!(
        "48000hz, 100 held voices, 4-stage, 128frames, 100warmups, 2000blocks; sine exact, mixed exact"
    );
    println!(
        "mixed context={}B, compact note={}B, global waveform sample data=245880B, control bank+configuration={:.3}ms",
        std::mem::size_of::<PreparedOperatorConfiguration>(),
        size_of_val(&mixed_notes[0]),
        bank_and_mixed_prepare.as_secs_f64() * 1e3
    );
    measure("sine", sine_render);
    measure("mixed", mixed_render);
    Ok(())
}
