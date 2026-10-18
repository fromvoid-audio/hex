//! Original synthesized listening fixtures; never reads or modifies user media.
use hex_warp::*;
use std::{io::Write, path::Path};

fn wav(path: &Path, pcm: &[Frame], rate: u32) -> std::io::Result<()> {
    let bytes = (pcm.len() * 4) as u32;
    let mut file = std::fs::File::create_new(path)?;
    file.write_all(b"RIFF")?;
    file.write_all(&(36 + bytes).to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16_u32.to_le_bytes())?;
    file.write_all(&1_u16.to_le_bytes())?;
    file.write_all(&2_u16.to_le_bytes())?;
    file.write_all(&rate.to_le_bytes())?;
    file.write_all(&(rate * 4).to_le_bytes())?;
    file.write_all(&4_u16.to_le_bytes())?;
    file.write_all(&16_u16.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&bytes.to_le_bytes())?;
    for frame in pcm {
        for value in frame {
            file.write_all(&((value.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes())?;
        }
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = std::env::args()
        .nth(1)
        .ok_or("pass a new output directory")?;
    let directory = Path::new(&directory);
    std::fs::create_dir(directory)?;
    let rate = 48_000;
    let length = 24_000;
    let source: Vec<Frame> = (0..length)
        .map(|i| {
            let age = i % 6_000;
            let phase = std::f64::consts::TAU * 73.0 * age as f64 / rate as f64;
            let body = 0.42 * phase.sin() * (-(age as f64) / 8_000.0).exp();
            let attack = if age < 32 {
                0.4 * (1.0 - age as f64 / 32.0)
            } else {
                0.0
            };
            [
                (0.12 + body + attack) as f32,
                (-0.09 + body * 0.6 - attack * 0.5) as f32,
            ]
        })
        .collect();
    wav(&directory.join("original.wav"), &source, rate)?;
    let frames = length * 2;
    let map = TimeMap::new(
        vec![MapSegment {
            output_start: 0,
            output_end: frames as u64,
            source_start_q32: 0,
            source_end_q32: (length as i128) << 32,
        }],
        frames,
    )?;
    for (name, tail) in [
        ("stop", Tail::Off),
        ("forward", Tail::Forward),
        ("back-and-forth", Tail::Alternating),
    ] {
        let rendered = render(
            &Request {
                input: Input {
                    frames: &source,
                    sample_rate: rate,
                    region_start: 0,
                    region_length: length as u64,
                    phase: 0,
                    looping: false,
                    envelope: &[],
                },
                attacks: &[0, 6_000, 12_000, 18_000],
                map: &map,
                output_frames: frames,
                pitch_cents: 0,
                mode: Mode::Beats {
                    division_frames: None,
                    tail,
                    decay: 100,
                },
            },
            &mut || false,
        )?;
        wav(
            &directory.join(format!("{name}-envelope100.wav")),
            &rendered.frames,
            rate,
        )?;
        println!(
            "{name}: {} frames, work {}, scratch {}",
            rendered.frames.len(),
            rendered.work_units,
            rendered.peak_scratch_bytes
        );
    }
    Ok(())
}
