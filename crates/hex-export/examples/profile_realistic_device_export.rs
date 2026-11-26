//! Realistic multi-track, multi-device 120-second sustained offline export profile (Pulse #2389).
//!
//! Workload: 8 resident audio tracks + 2 shared Return buses + Main bus at 48 kHz stereo (120 s =
//! 5,760,000 frames), populated with native EQ8, Compressor v4, Gate v2, Saturator, Filter, Delay,
//! AutoPan, Overdrive, Reverb, and Main Limiter (True Peak mode).
//!
//! Measures:
//! 1. Warmup + 3 sustained Main PCM16 WAV exports (elapsed ms, realtime factor, deterministic digest).
//! 2. 4-stem batch export (`render_stems_new`) through the full device graph.
//! 3. Mid-export cancellation responsiveness (`ExportControl::cancel`).
//!
//! This is an offline export measurement, not a real-time physical audio device deadline claim.
use hex_engine::{
    Audio, OutputCaptureId, PreparedRevision, SineTable, default_native_device_state,
};
use hex_export::{ExportControl, ExportError, Stem, render_prepared_new, render_stems_new};
use hex_lattice_contract::DeviceTypeId;
use hex_project::{Command, Created, Editor, Project};
use std::{
    collections::BTreeMap,
    error::Error,
    fs,
    io::{BufReader, Read},
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const RATE: u32 = 48_000;
const SECONDS: u64 = 120;
const FRAMES: usize = (RATE as usize) * (SECONDS as usize);
const TRIALS: usize = 3;

struct TempDir(PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn digest_wav(path: &Path) -> Result<(u64, u64), Box<dyn Error>> {
    let file = fs::File::open(path)?;
    let len = file.metadata()?.len();
    let mut reader = BufReader::new(file);
    let mut buf = [0_u8; 8192];
    let mut digest = 0xcbf29ce484222325_u64;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        for &b in &buf[..n] {
            digest = (digest ^ u64::from(b)).wrapping_mul(0x100000001b3);
        }
    }
    Ok((len, digest))
}

fn main() -> Result<(), Box<dyn Error>> {
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let root = std::env::temp_dir().join(format!(
        "hex-realistic-export-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir(&root)?;
    let dir = TempDir(root);

    let mut editor = Editor::new(Project::new(RATE)?, 64)?;
    // 2 shared Return buses: Reverb + Delay.
    let Created::ReturnTrack(ret_reverb) = editor.execute(Command::AddReturnTrack {
        name: "reverb".into(),
    })?
    else {
        return Err("reverb return failed".into());
    };
    let Created::ReturnTrack(ret_delay) = editor.execute(Command::AddReturnTrack {
        name: "delay".into(),
    })?
    else {
        return Err("delay return failed".into());
    };
    editor.execute(Command::InsertReturnDevice {
        return_track: ret_reverb,
        before: None,
        state: default_native_device_state(DeviceTypeId::from_static("hex.reverb"))?,
    })?;
    editor.execute(Command::InsertReturnDevice {
        return_track: ret_delay,
        before: None,
        state: default_native_device_state(DeviceTypeId::from_static("hex.delay"))?,
    })?;

    // Main bus: EQ8 + Compressor + Limiter.
    for dev in ["hex.eq8", "hex.compressor", "hex.limiter"] {
        editor.execute(Command::InsertMainDevice {
            before: None,
            state: default_native_device_state(DeviceTypeId::from_static(dev))?,
        })?;
    }

    // 8 tracks with distinct native effect chains, sends, and deterministic audio buffers.
    let track_chains: [&[&str]; 8] = [
        &["hex.gate", "hex.eq8", "hex.compressor"],
        &["hex.eq8", "hex.saturator", "hex.compressor"],
        &["hex.filter", "hex.overdrive", "hex.utility"],
        &["hex.eq8", "hex.auto-pan", "hex.delay"],
        &["hex.gate", "hex.compressor", "hex.utility"],
        &["hex.eq8", "hex.filter", "hex.saturator"],
        &["hex.compressor", "hex.delay", "hex.utility"],
        &["hex.eq8", "hex.compressor", "hex.auto-pan"],
    ];

    let mut media = BTreeMap::new();
    let mut tracks = Vec::with_capacity(8);
    // Reuse a 2-second synthesized stereo waveform repeated across 120s clips to keep RAM lean.
    let pattern_frames = RATE as usize * 2;
    for (idx, chain) in track_chains.iter().enumerate() {
        let freq = 110.0 * (1.0 + idx as f32 * 0.25);
        let Created::Imported { track, asset, .. } =
            editor.execute(Command::ImportPreparedAudioTrack {
                name: format!("track-{}", idx + 1),
                path: format!("memory/track-{}.wav", idx + 1).into(),
                source_sample_rate: RATE,
                source_frames: FRAMES as u64,
                playback_frames: FRAMES as u64,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            })?
        else {
            return Err("track import failed".into());
        };
        let pattern: Vec<[f32; 2]> = (0..pattern_frames)
            .map(|i| {
                let t = i as f32 / RATE as f32;
                let s = (std::f32::consts::TAU * freq * t).sin() * 0.12;
                let env = if (i / 12_000) % 2 == 0 { 1.0 } else { 0.25 };
                [s * env, -s * env * 0.85]
            })
            .collect();
        let mut samples = Vec::with_capacity(FRAMES);
        while samples.len() < FRAMES {
            let take = (FRAMES - samples.len()).min(pattern.len());
            samples.extend_from_slice(&pattern[..take]);
        }
        media.insert(asset, Audio::new(RATE, samples)?);
        for dev in *chain {
            editor.execute(Command::InsertDevice {
                track,
                before: None,
                state: default_native_device_state(DeviceTypeId::from_static(dev))?,
            })?;
        }
        editor.execute(Command::SetReturnSend {
            track,
            return_track: if idx % 2 == 0 { ret_reverb } else { ret_delay },
            gain: 0.25,
        })?;
        tracks.push(track);
    }

    let prep_start = Instant::now();
    let revision = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &media,
        Arc::new(SineTable::new()),
        None,
    )?);
    let prep_ms = prep_start.elapsed().as_secs_f64() * 1000.0;
    println!(
        "workload tracks=8 returns=2 devices=29 rate={RATE} duration_s={SECONDS} frames={FRAMES} prep_ms={prep_ms:.2}"
    );

    let mut expected_digest = None;
    for trial in 0..=TRIALS {
        let path = dir.0.join(format!("main-trial-{trial}.wav"));
        let start = Instant::now();
        let report = render_prepared_new(&path, Arc::clone(&revision), 0, FRAMES as u64)?;
        let elapsed = start.elapsed();
        let (bytes, digest) = digest_wav(&path)?;
        fs::remove_file(&path)?;
        if let Some(prev) = expected_digest {
            if prev != digest {
                return Err(format!(
                    "non-deterministic export digest: {prev:016x} != {digest:016x}"
                )
                .into());
            }
        } else {
            expected_digest = Some(digest);
        }
        println!(
            "main trial={trial} warmup={} elapsed_ms={:.2} realtime_factor={:.2}x frames={} bytes={bytes} clipped={} digest={digest:016x}",
            trial == 0,
            elapsed.as_secs_f64() * 1000.0,
            SECONDS as f64 / elapsed.as_secs_f64(),
            report.frames,
            report.clipped_samples
        );
    }

    // 4-stem batch export (2 tracks + 2 returns) over full 120s.
    let stems = [
        Stem {
            id: OutputCaptureId::Track(tracks[0]),
            name: "track-1".into(),
        },
        Stem {
            id: OutputCaptureId::Track(tracks[1]),
            name: "track-2".into(),
        },
        Stem {
            id: OutputCaptureId::Return(ret_reverb),
            name: "reverb".into(),
        },
        Stem {
            id: OutputCaptureId::Return(ret_delay),
            name: "delay".into(),
        },
    ];
    let stem_dir = dir.0.join("stems-batch");
    let stem_start = Instant::now();
    let stem_control = ExportControl::default();
    let stem_report = render_stems_new(
        &stem_dir,
        Arc::clone(&revision),
        0,
        FRAMES as u64,
        &stems,
        &stem_control,
    )?;
    let stem_elapsed = stem_start.elapsed();
    println!(
        "stems count=4 elapsed_ms={:.2} realtime_factor={:.2}x frames={}",
        stem_elapsed.as_secs_f64() * 1000.0,
        SECONDS as f64 / stem_elapsed.as_secs_f64(),
        stem_report.frames
    );

    // Mid-export cancellation responsiveness check.
    let cancel_dir = dir.0.join("stems-cancel");
    let cancel_control = ExportControl::default();
    let trigger = cancel_control.clone();
    let cancel_rev = Arc::clone(&revision);
    let cancel_stems = stems.clone();
    let handle = thread::spawn(move || {
        render_stems_new(
            &cancel_dir,
            cancel_rev,
            0,
            FRAMES as u64,
            &cancel_stems,
            &cancel_control,
        )
    });
    while trigger.progress().processed_frames == 0 {
        thread::sleep(Duration::from_millis(1));
    }
    let cancel_req = Instant::now();
    let accepted = trigger.cancel();
    let cancel_res = handle.join().map_err(|_| "cancel worker panicked")?;
    let cancel_latency_ms = cancel_req.elapsed().as_secs_f64() * 1000.0;
    if !accepted || !matches!(cancel_res, Err(ExportError::Cancelled)) {
        return Err(format!(
            "expected Cancelled result, got accepted={accepted} res={cancel_res:?}"
        )
        .into());
    }
    println!(
        "cancellation accepted={accepted} latency_ms={cancel_latency_ms:.3} processed_frames={}",
        trigger.progress().processed_frames
    );
    Ok(())
}
