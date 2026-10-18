//! Create-new, original synthetic before/after audio and unnormalised metrics.
use hex_warp::*;
use std::{io::Write, path::Path};

fn wav(path: &Path, pcm: &[Frame], rate: u32) -> std::io::Result<()> {
    let bytes = u32::try_from(pcm.len() * 4).unwrap();
    let mut f = std::fs::File::create_new(path)?;
    for bytes in [
        b"RIFF".as_slice(),
        &(36 + bytes).to_le_bytes(),
        b"WAVEfmt ",
        &16_u32.to_le_bytes(),
        &1_u16.to_le_bytes(),
        &2_u16.to_le_bytes(),
        &rate.to_le_bytes(),
        &(rate * 4).to_le_bytes(),
        &4_u16.to_le_bytes(),
        &16_u16.to_le_bytes(),
        b"data",
        &bytes.to_le_bytes(),
    ] {
        f.write_all(bytes)?;
    }
    for frame in pcm {
        for value in frame {
            f.write_all(&((value.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes())?;
        }
    }
    Ok(())
}
fn source(kind: &str, rate: u32) -> Vec<Frame> {
    (0..rate as usize / 2)
        .map(|i| {
            let t = i as f64 / f64::from(rate);
            let age = (i % (rate as usize / 8)) as f64 / f64::from(rate);
            let phase = std::f64::consts::TAU * 73.0 * t;
            let kick = (std::f64::consts::TAU * (55.0 * age + 2.0 * (1.0 - (-80.0 * age).exp())))
                .sin()
                * (-16.0 * age).exp()
                * 0.65;
            let noise = (((i as u64 * 1_664_525 + 1_013_904_223) ^ (i as u64 * 7_919)) % 65_521)
                as f64
                / 32_760.0
                - 1.0;
            match kind {
                "dc" => [0.5, -0.25],
                "bass55" => {
                    let x = (std::f64::consts::TAU * 55.0 * t).sin() * 0.5;
                    [x as f32, (-x * 0.7) as f32]
                }
                "drum-bass" => {
                    let bass = (std::f64::consts::TAU * 55.0 * t).sin() * 0.15;
                    if i % (rate as usize / 8) == 0 {
                        [0.9, -0.8]
                    } else {
                        [
                            (bass + kick * 0.7) as f32,
                            (-bass * 0.7 + kick * 0.5) as f32,
                        ]
                    }
                }
                "stereo-tone" => [
                    (0.15 + 0.5 * phase.sin()) as f32,
                    (-0.1 + 0.3 * (phase + 0.7).sin()) as f32,
                ],
                "kick-sub" => [(kick + 0.08) as f32, (kick * 0.8 - 0.05) as f32],
                "hats" => {
                    let x = noise * (-110.0 * age).exp() * 0.55;
                    [x as f32, -x as f32]
                }
                _ => unreachable!(),
            }
        })
        .collect()
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = std::env::args()
        .nth(1)
        .ok_or("pass a new output directory")?;
    let automatic = std::env::args().nth(2).as_deref() == Some("transients");
    let directory = Path::new(&directory);
    std::fs::create_dir(directory)?;
    let mut metrics = std::fs::File::create_new(directory.join("metrics.csv"))?;
    writeln!(
        metrics,
        "fixture,rate,ratio,tail,output_frames,source_boundary,output_boundary,left_step,right_step,original_left_step,original_right_step,work,scratch"
    )?;
    let mut settings = std::fs::File::create_new(directory.join("settings.txt"))?;
    writeln!(
        settings,
        "algorithm_version={}\noriginal synthetic fixtures; Preserve={}; pitch0; envelope100; no source/clip fade; nonlooping root; PCM16 fixed32767 quantisation, no per-file normalisation; before/after use same listening gain",
        ALGORITHM_VERSION,
        if automatic {
            "automatic Transients"
        } else {
            "4 fixed divisions"
        }
    )?;
    let kinds: &[&str] = if automatic {
        &[
            "dc",
            "stereo-tone",
            "kick-sub",
            "hats",
            "bass55",
            "drum-bass",
        ]
    } else {
        &["dc", "stereo-tone", "kick-sub", "hats"]
    };
    for rate in [44_100, 48_000] {
        for &kind in kinds {
            let source = source(kind, rate);
            wav(
                &directory.join(format!("{kind}-{rate}-original.wav")),
                &source,
                rate,
            )?;
            for ratio in [0.5, 1.375, 2.0] {
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
                                division_frames: (!automatic).then_some((source.len() / 4) as u64),
                                tail,
                                decay: 100,
                            },
                        },
                        &mut || false,
                    )?;
                    wav(
                        &directory.join(format!("{kind}-{rate}-ratio{ratio}-{name}.wav")),
                        &rendered.frames,
                        rate,
                    )?;
                    for boundary in 1..4 {
                        let src = source.len() / 4 * boundary;
                        let at = map.output_for_source((src as i128) << 32).unwrap() as usize;
                        let step = |pcm: &[Frame], c: usize, i: usize| pcm[i][c] - pcm[i - 1][c];
                        writeln!(
                            metrics,
                            "{kind},{rate},{ratio},{name},{output},{src},{at},{},{},{},{},{},{}",
                            step(&rendered.frames, 0, at),
                            step(&rendered.frames, 1, at),
                            step(&source, 0, src),
                            step(&source, 1, src),
                            rendered.work_units,
                            rendered.peak_scratch_bytes
                        )?;
                    }
                }
            }
        }
    }
    Ok(())
}
