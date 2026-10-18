//! Evidence-only bounded stereo PCM-f32 adapter. No production file ingress.
use hex_warp::*;
use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 4 && args.len() != 6 {
        return Err(
            "pass interleaved stereo f32 input, sample rate, and new output directory".into(),
        );
    }
    if args.len() == 6
        && (!matches!(args[4].as_str(), "0.5" | "1.375" | "2")
            || !matches!(args[5].as_str(), "stop" | "forward" | "alternating"))
    {
        return Err("optional case must name an existing ratio and tail".into());
    }
    let input = std::path::Path::new(&args[1]);
    // The six authorised originals are at most 16.821 seconds. Decoded f32 is
    // bounded here independently of the renderer's unchanged resource limits.
    if std::fs::metadata(input)?.len() > 6_000_000 {
        return Err("evidence input exceeds bounded decoded recording size".into());
    }
    let bytes = std::fs::read(input)?;
    if bytes.is_empty() || bytes.len() % 8 != 0 {
        return Err("expected nonempty complete stereo f32 frames".into());
    }
    let source: Vec<Frame> = bytes
        .chunks_exact(8)
        .map(|b| {
            [
                f32::from_le_bytes(b[..4].try_into().unwrap()),
                f32::from_le_bytes(b[4..].try_into().unwrap()),
            ]
        })
        .collect();
    if !source.iter().flatten().all(|x| x.is_finite()) {
        return Err("nonfinite evidence input".into());
    }
    let rate: u32 = args[2].parse()?;
    let directory = std::path::Path::new(&args[3]);
    std::fs::create_dir(directory)?;
    let mut metrics = std::fs::File::create_new(directory.join("metrics.csv"))?;
    writeln!(metrics, "ratio,tail,frames,work,scratch,peak,rms,result")?;
    std::fs::write(
        directory.join("settings.txt"),
        format!(
            "algorithm_version={ALGORITHM_VERSION}\nrate={rate}\nframes={}\nPreserve=Transients; envelope100; pitch0; no clip fade; nonlooping; original BPM unknown/not assigned; no normalisation\n",
            source.len()
        ),
    )?;
    for ratio in [0.5, 1.375, 2.0] {
        if args.len() == 6 && args[4].parse::<f64>()? != ratio {
            continue;
        }
        let output = (source.len() as f64 * ratio).round() as usize;
        let map = TimeMap::new(
            vec![MapSegment {
                output_start: 0,
                output_end: output as u64,
                source_start_q32: 0,
                source_end_q32: (source.len() as i128) << 32,
            }],
            output,
        )?;
        for (tail, name) in [
            (Tail::Off, "stop"),
            (Tail::Forward, "forward"),
            (Tail::Alternating, "alternating"),
        ] {
            if args.len() == 6 && args[5] != name {
                continue;
            }
            let result = render(
                &Request {
                    input: Input {
                        frames: &source,
                        sample_rate: rate,
                        region_start: 0,
                        region_length: source.len() as u64,
                        phase: 0,
                        looping: false,
                        envelope: &[],
                    },
                    attacks: &[],
                    map: &map,
                    output_frames: output,
                    pitch_cents: 0,
                    mode: Mode::Beats {
                        division_frames: None,
                        tail,
                        decay: 100,
                    },
                },
                &mut || false,
            );
            match result {
                Ok(rendered) => {
                    let mut file = std::fs::File::create_new(
                        directory.join(format!("ratio{ratio}-{name}.f32")),
                    )?;
                    let mut peak = 0.0_f32;
                    let mut square = 0.0;
                    for frame in &rendered.frames {
                        for sample in frame {
                            peak = peak.max(sample.abs());
                            square += f64::from(*sample).powi(2);
                            file.write_all(&sample.to_le_bytes())?;
                        }
                    }
                    writeln!(
                        metrics,
                        "{ratio},{name},{},{},{},{peak},{},ok",
                        rendered.frames.len(),
                        rendered.work_units,
                        rendered.peak_scratch_bytes,
                        (square / (rendered.frames.len() * 2) as f64).sqrt()
                    )?;
                }
                Err(error) => writeln!(metrics, "{ratio},{name},0,0,0,0,0,{error}")?,
            }
            metrics.flush()?;
        }
    }
    Ok(())
}
