//! Create-new evidence for a continuous region incorrectly treated as attacks.
use hex_warp::*;
use std::{io::Write, path::Path};
fn wav(path: &Path, frames: &[Frame]) -> std::io::Result<()> {
    let bytes = (frames.len() * 4) as u32;
    let mut f = std::fs::File::create_new(path)?;
    for chunk in [
        b"RIFF".as_slice(),
        &(36 + bytes).to_le_bytes(),
        b"WAVEfmt ",
        &16_u32.to_le_bytes(),
        &1_u16.to_le_bytes(),
        &2_u16.to_le_bytes(),
        &8_000_u32.to_le_bytes(),
        &32_000_u32.to_le_bytes(),
        &4_u16.to_le_bytes(),
        &16_u16.to_le_bytes(),
        b"data",
        &bytes.to_le_bytes(),
    ] {
        f.write_all(chunk)?;
    }
    for frame in frames {
        for sample in frame {
            f.write_all(&((sample.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes())?;
        }
    }
    Ok(())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arg = std::env::args()
        .nth(1)
        .ok_or("pass a new output directory")?;
    let directory = Path::new(&arg);
    std::fs::create_dir(directory)?;
    let source = [[0.4, -0.2]; 2];
    let map = TimeMap::new(
        vec![MapSegment {
            output_start: 0,
            output_end: 8_000,
            source_start_q32: 0,
            source_end_q32: 4_000_i128 << 32,
        }],
        8_000,
    )?;
    wav(
        &directory.join("original-continuous-dc-one-second.wav"),
        &vec![[0.4, -0.2]; 8_000],
    )?;
    for (name, tail) in [
        ("forward", Tail::Forward),
        ("alternating", Tail::Alternating),
    ] {
        let pcm = render(
            &Request {
                input: Input {
                    frames: &source,
                    sample_rate: 8_000,
                    region_start: 0,
                    region_length: 2,
                    phase: 0,
                    looping: true,
                    envelope: &[],
                },
                attacks: &[],
                map: &map,
                output_frames: 8_000,
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
            &directory.join(format!("{name}-region2-loop-dc-ratio2.wav")),
            &pcm.frames,
        )?;
        eprintln!(
            "{name} first16={:?} work={} scratch={}",
            &pcm.frames[..16],
            pcm.work_units,
            pcm.peak_scratch_bytes
        );
    }
    let mut settings = std::fs::File::create_new(directory.join("settings.txt"))?;
    writeln!(
        settings,
        "algorithm_version={}\noriginal synthetic DC0.4/-0.2, source region2 frames at8000Hz looped; unwrapped source4000/output8000;2x time; pitch0; Transients/envelope100; fixedPCM16gain32767; no perfile normalization; no actual listening",
        ALGORITHM_VERSION
    )?;
    Ok(())
}
