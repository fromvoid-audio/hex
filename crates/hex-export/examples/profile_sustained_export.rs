//! Finite prepared stereo export profile with a complete PCM oracle.
//! This is a resident, one-track workload, not a device deadline or streaming benchmark.
use hex_engine::{Audio, PreparedRevision, SineTable};
use hex_project::{Command, Created, Editor, Project};
use std::{
    collections::BTreeMap,
    error::Error,
    fs,
    io::{BufReader, Read},
    path::PathBuf,
    sync::Arc,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

const RATE: u32 = 48_000;
const SECONDS: u32 = 60;
const FRAMES: usize = RATE as usize * SECONDS as usize;
const TRIALS: usize = 3;

struct FixtureDir(PathBuf);
impl Drop for FixtureDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn source(index: usize) -> [f32; 2] {
    const VALUES: [f32; 4] = [0.0, 0.25, -0.5, 0.75];
    [VALUES[index % 4], VALUES[(index / 4) % 4]]
}

fn expected(index: usize) -> [i16; 2] {
    // Explicit independent PCM16 oracle for the four exact binary amplitudes.
    const PCM: [i16; 4] = [0, 8192, -16384, 24575];
    [PCM[index % 4], PCM[(index / 4) % 4]]
}

fn verify(path: &std::path::Path) -> Result<u64, Box<dyn Error>> {
    let file = fs::File::open(path)?;
    if file.metadata()?.len() != 44 + FRAMES as u64 * 4 {
        return Err("unexpected WAV length".into());
    }
    let mut reader = BufReader::new(file);
    let mut header = [0_u8; 44];
    reader.read_exact(&mut header)?;
    if &header[0..4] != b"RIFF"
        || &header[8..12] != b"WAVE"
        || &header[36..40] != b"data"
        || header[20..24] != [1, 0, 2, 0]
        || header[24..28] != RATE.to_le_bytes()
        || header[34..36] != [16, 0]
        || header[40..44] != (FRAMES as u32 * 4).to_le_bytes()
    {
        return Err("unexpected WAV header".into());
    }
    let mut digest = 0xcbf29ce484222325_u64;
    let mut block = [0_u8; 4096];
    let mut frame = 0;
    while frame < FRAMES {
        let count = (FRAMES - frame).min(block.len() / 4);
        reader.read_exact(&mut block[..count * 4])?;
        for bytes in block[..count * 4].chunks_exact(4) {
            let actual = [
                i16::from_le_bytes([bytes[0], bytes[1]]),
                i16::from_le_bytes([bytes[2], bytes[3]]),
            ];
            if actual != expected(frame) {
                return Err(format!("PCM mismatch at frame {frame}: {actual:?}").into());
            }
            for byte in bytes {
                digest = (digest ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
            }
            frame += 1;
        }
    }
    Ok(digest)
}

fn main() -> Result<(), Box<dyn Error>> {
    if std::env::args().len() != 1 {
        return Err("usage: profile_sustained_export".into());
    }
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let path =
        std::env::temp_dir().join(format!("hex-export-profile-{}-{nonce}", std::process::id()));
    fs::create_dir(&path)?;
    let directory = FixtureDir(path);
    let mut editor = Editor::new(Project::new(RATE)?, 4)?;
    let Created::Imported { asset, .. } = editor.execute(Command::ImportPreparedAudioTrack {
        name: "profile".into(),
        path: "memory/profile.wav".into(),
        source_sample_rate: RATE,
        source_frames: FRAMES as u64,
        playback_frames: FRAMES as u64,
        conversion_version: hex_project::IDENTITY_CONVERSION,
    })?
    else {
        return Err("fixture import did not create an asset".into());
    };
    let samples: Vec<_> = (0..FRAMES).map(source).collect();
    let media = BTreeMap::from([(asset, Audio::new(RATE, samples)?)]);
    let revision = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &media,
        Arc::new(SineTable::new()),
        None,
    )?);
    for trial in 0..=TRIALS {
        let path = directory.0.join(format!("trial-{trial}.wav"));
        let start = Instant::now();
        let report =
            hex_export::render_prepared_new(&path, Arc::clone(&revision), 0, FRAMES as u64)?;
        let elapsed = start.elapsed();
        if report.sample_rate != RATE
            || report.frames != FRAMES as u64
            || report.data_bytes != FRAMES as u32 * 4
        {
            return Err("unexpected export report".into());
        }
        let digest = verify(&path)?;
        fs::remove_file(path)?;
        println!(
            "trial={trial} warmup={} rate={RATE} channels=2 duration_seconds={SECONDS} frames={FRAMES} format=pcm16 block=256 elapsed_ms={:.3} realtime_factor={:.2} digest={digest:016x}",
            trial == 0,
            elapsed.as_secs_f64() * 1000.0,
            f64::from(SECONDS) / elapsed.as_secs_f64()
        );
    }
    println!(
        "prepared one-track resident source; timed export includes staging, sync and publication; preparation and full-frame verification excluded; no streaming or native device claim"
    );
    Ok(())
}
