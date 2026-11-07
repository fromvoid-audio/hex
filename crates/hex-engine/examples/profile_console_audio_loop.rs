//! Optimized offline resident Console audio rendering; no physical device claim.
//! Use `cargo run -r -p hex-engine --example profile_console_audio_loop --locked`.
use hex_engine::{
    Audio, LaunchQuantization, PreparedRevision, SessionPlayback, SessionPlaybackCommand, SineTable,
};
use hex_project::{Command, Created, Editor, Project, SessionAudioLoop};
use std::{collections::BTreeMap, error::Error, hint::black_box, sync::Arc, time::Instant};
fn main() -> Result<(), Box<dyn Error>> {
    const TRACKS: usize = 32;
    const BLOCK: usize = 128;
    const WARMUP: usize = 100;
    const SAMPLES: usize = 1000;
    let one_shot = std::env::args().any(|arg| arg == "--one-shot");
    for length in [None, Some(24000), Some(1)] {
        if one_shot && length.is_some() {
            continue;
        }
        let mut e = Editor::new(Project::new(48000)?, 8)?;
        let Created::Asset(asset) = e.execute(Command::AddAsset {
            path: "memory/loop.wav".into(),
            sample_rate: 48000,
            frames: 262144,
        })?
        else {
            return Err("asset".into());
        };
        let Created::Scene(scene) = e.execute(Command::AddScene {
            name: "scene".into(),
        })?
        else {
            return Err("scene".into());
        };
        for i in 0..TRACKS {
            let Created::Track(track) = e.execute(Command::AddTrack {
                name: format!("audio {i}"),
            })?
            else {
                return Err("track".into());
            };
            let Created::SessionClip(clip) = e.execute(Command::CreateSessionAudioClip {
                track,
                scene,
                name: "cell".into(),
                asset,
                source_start: 0,
                source_end: 262144,
            })?
            else {
                return Err("clip".into());
            };
            if let Some(n) = length {
                e.execute(Command::SetSessionAudioClipLoop {
                    clip,
                    loop_range: Some(SessionAudioLoop::new(0, n)?),
                })?;
            }
        }
        let media = BTreeMap::from([(asset, Audio::new(48000, vec![[0.003, -0.003]; 262144])?)]);
        let revision = Arc::new(PreparedRevision::prepare(
            e.project(),
            &media,
            Arc::new(SineTable::new()),
            None,
        )?);
        let mut runtime = SessionPlayback::new(revision)?;
        runtime.apply(SessionPlaybackCommand::LaunchScene {
            scene,
            quantization: LaunchQuantization::Immediate,
        })?;
        runtime.apply(SessionPlaybackCommand::Play)?;
        let mut output = [[0.0; 2]; BLOCK];
        let mut times = Vec::with_capacity(SAMPLES);
        for i in 0..WARMUP + SAMPLES {
            let start = Instant::now();
            runtime.render(&mut output)?;
            let elapsed = start.elapsed().as_nanos();
            if i >= WARMUP {
                times.push(elapsed);
            }
            black_box(&output);
        }
        if output
            .iter()
            .any(|frame| (frame[0] - 0.003 * TRACKS as f32).abs() > 1e-6)
        {
            return Err("pcm oracle differs".into());
        }
        times.sort_unstable();
        println!(
            "tracks={TRACKS} rate=48000 block={BLOCK} warmup={WARMUP} samples={SAMPLES} loop_frames={length:?} p50_us={:.3} p95_us={:.3} p99_us={:.3}",
            times[SAMPLES / 2] as f64 / 1000.0,
            times[SAMPLES * 95 / 100] as f64 / 1000.0,
            times[SAMPLES * 99 / 100] as f64 / 1000.0
        );
    }
    Ok(())
}
