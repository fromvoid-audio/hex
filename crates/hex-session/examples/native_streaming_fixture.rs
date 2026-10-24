//! Create-new original media and saved projects for native streaming acceptance.
//! Generation/verification is not native playback or listening evidence.
use hex_project::{Command, Created, Editor, Project, converted_frame_count};
use serde_json::{Value, json};
use std::{
    error::Error,
    fs::{self, File, OpenOptions},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    process::Command as Process,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const RATES: [u32; 3] = [24_000, 44_100, 48_000];
const PROJECT_RATES: [u32; 2] = [44_100, 48_000];
const BLOCK_FRAMES: usize = 4096;
const AMPLITUDE: i64 = 8192;
const TRACK_GAIN: f32 = 0.04;
const MASTER_GAIN: f32 = 0.5;

#[derive(Clone, Copy)]
struct Specification {
    tracks: usize,
    seconds: u32,
}
impl Specification {
    fn validate(self) -> Result<Self> {
        if !(1..=100).contains(&self.tracks) || !(1..=120).contains(&self.seconds) {
            return Err("fixture requires 1..100 tracks and 1..120 seconds".into());
        }
        for rate in RATES {
            let frames = u64::from(rate)
                .checked_mul(u64::from(self.seconds))
                .ok_or("source frame count overflow")?;
            let bytes = frames.checked_mul(2).ok_or("source byte count overflow")?;
            if frames > 8_000_000 || bytes.checked_add(44).is_none_or(|n| n >= 64 * 1024 * 1024) {
                return Err("fixture exceeds existing per-source limits".into());
            }
        }
        Ok(self)
    }
}

struct Source {
    path: PathBuf,
    rate: u32,
    frames: u64,
    frequency: u64,
    phase: u64,
}
fn sources(spec: Specification) -> Result<Vec<Source>> {
    let spec = spec.validate()?;
    let mut sources = Vec::new();
    sources.try_reserve_exact(spec.tracks)?;
    for index in 0..spec.tracks {
        let rate = RATES[index % RATES.len()];
        let ordinal = u64::try_from(index)?;
        sources.push(Source {
            path: PathBuf::from(format!("media/source-{:03}-{rate}.wav", index + 1)),
            rate,
            frames: u64::from(rate) * u64::from(spec.seconds),
            frequency: 73 + ordinal * 3,
            phase: ordinal * u64::from(rate) / 101,
        });
    }
    Ok(sources)
}

fn project(sources: &[Source], rate: u32) -> Result<Project> {
    let mut editor = Editor::new(Project::new(rate)?, 1)?;
    let mut scenes = Vec::new();
    for name in ["all sources", "offset 30 seconds"] {
        let Created::Scene(scene) = editor.execute(Command::AddScene { name: name.into() })? else {
            return Err("scene creation returned an unexpected identity".into());
        };
        scenes.push(scene);
    }
    for (index, source) in sources.iter().enumerate() {
        let frames = converted_frame_count(source.frames, source.rate, rate)?;
        let Created::Imported { track, asset, .. } =
            editor.execute(Command::ImportPreparedAudioTrackAt {
                name: format!("stream {:03} · {} hz", index + 1, source.rate),
                before_track: None,
                start: 0,
                path: source.path.clone(),
                source_sample_rate: source.rate,
                source_frames: source.frames,
                playback_frames: frames,
                conversion_version: if source.rate == rate {
                    hex_project::IDENTITY_CONVERSION
                } else {
                    hex_project::SINC_CONVERSION_V1
                },
            })?
        else {
            return Err("import returned an unexpected identity".into());
        };
        editor.execute(Command::Gain {
            track,
            gain: TRACK_GAIN,
        })?;
        for (scene_index, scene) in scenes.iter().enumerate() {
            // Short test fixtures still have valid, nonempty second scenes.
            let offset = if scene_index == 0 {
                0
            } else {
                (u64::from(rate) * 30).min(frames / 2)
            };
            editor.execute(Command::CreateSessionAudioClip {
                track,
                scene: *scene,
                name: format!("source {:03}", index + 1),
                asset,
                source_start: offset,
                source_end: frames,
            })?;
        }
    }
    editor.execute(Command::MasterGain { gain: MASTER_GAIN })?;
    let project = editor.project().clone();
    // This validates the existing closed class, not native admission/readiness.
    hex_engine::AudioCandidateReservationQuote::for_project(&project, sources.len())
        .map_err(|error| format!("unsupported fixture: {error:?}"))?;
    Ok(project)
}

fn write_wav(path: &Path, source: &Source) -> Result<()> {
    let data_bytes = u32::try_from(source.frames.checked_mul(2).ok_or("wav size overflow")?)?;
    let file = OpenOptions::new().create_new(true).write(true).open(path)?;
    let mut writer = BufWriter::with_capacity(16 * 1024, file);
    writer.write_all(b"RIFF")?;
    writer.write_all(
        &data_bytes
            .checked_add(36)
            .ok_or("riff size overflow")?
            .to_le_bytes(),
    )?;
    writer.write_all(b"WAVEfmt ")?;
    writer.write_all(&16u32.to_le_bytes())?;
    writer.write_all(&1u16.to_le_bytes())?; // PCM
    writer.write_all(&1u16.to_le_bytes())?; // mono
    writer.write_all(&source.rate.to_le_bytes())?;
    writer.write_all(
        &source
            .rate
            .checked_mul(2)
            .ok_or("byte rate overflow")?
            .to_le_bytes(),
    )?;
    writer.write_all(&2u16.to_le_bytes())?;
    writer.write_all(&16u16.to_le_bytes())?;
    writer.write_all(b"data")?;
    writer.write_all(&data_bytes.to_le_bytes())?;
    let mut block = [0u8; BLOCK_FRAMES * 2];
    let mut written = 0u64;
    while written < source.frames {
        let count = usize::try_from((source.frames - written).min(BLOCK_FRAMES as u64))?;
        for index in 0..count {
            // Original integer triangle: deterministic PCM without platform
            // transcendental rounding, full-duration signal, no silent gaps.
            let phase = ((written + index as u64) * source.frequency + source.phase)
                % u64::from(source.rate);
            let phase = i64::try_from(phase * 4)?;
            let rate = i64::from(source.rate);
            let numerator = if phase < rate {
                phase
            } else if phase < rate * 3 {
                rate * 2 - phase
            } else {
                phase - rate * 4
            };
            let sample = i16::try_from(numerator * AMPLITUDE / rate)?;
            block[index * 2..index * 2 + 2].copy_from_slice(&sample.to_le_bytes());
        }
        writer.write_all(&block[..count * 2])?;
        written += count as u64;
    }
    writer.flush()?;
    writer.get_ref().sync_all()?;
    Ok(())
}
fn reader_limits() -> hex_media::VerifiedWavReaderLimits {
    hex_media::VerifiedWavReaderLimits::new(
        hex_media::WavReaderLimits::new(64 * 1024 * 1024, 8_000_000, 32, BLOCK_FRAMES),
        1024 * 1024,
    )
}
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        value.push(char::from(DIGITS[usize::from(byte >> 4)]));
        value.push(char::from(DIGITS[usize::from(byte & 15)]));
    }
    value
}
fn generate(directory: &Path, spec: Specification, provenance: Value) -> Result<Value> {
    let spec = spec.validate()?;
    let sources = sources(spec)?;
    // Validate complete canonical projects before touching the destination.
    let projects = PROJECT_RATES
        .map(|rate| project(&sources, rate))
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
    fs::create_dir(directory)?; // refuse even an existing empty destination
    fs::create_dir(directory.join("media"))?;
    let mut files = Vec::new();
    for source in &sources {
        let path = directory.join(&source.path);
        write_wav(&path, source)?;
        let reader = hex_media::VerifiedWavReader::open(&path, reader_limits())?;
        if reader.source_frames() != source.frames || reader.sample_rate() != source.rate {
            return Err("generated wav differs from its source metadata".into());
        }
        files.push(json!({
            "path": source.path, "sha256": hex(reader.fingerprint()),
            "file_bytes": fs::metadata(&path)?.len(), "sample_rate": source.rate,
            "source_frames": source.frames, "frequency_hz": source.frequency,
            "phase_numerator": source.phase, "channels": 1, "bits_per_sample": 16,
        }));
    }
    let mut documents = Vec::new();
    for project in projects {
        let name = format!("{}-{}.hex", spec.tracks, project.sample_rate());
        let path = directory.join(&name);
        hex_storage::save_new(&path, &project)?;
        if hex_storage::load(&path)? != project {
            return Err("saved fixture failed canonical roundtrip".into());
        }
        documents.push(json!({
            "path": name, "sample_rate": project.sample_rate(), "tracks": spec.tracks,
            "assets": sources.len(), "arrangement_clips": spec.tracks,
            "session_scenes": 2, "session_cells": spec.tracks * 2,
            "duration_frames": u64::from(project.sample_rate()) * u64::from(spec.seconds),
            "aggregate_converted_pcm_frames": u64::from(project.sample_rate()) * u64::from(spec.seconds) * spec.tracks as u64,
        }));
    }
    let manifest = json!({
        "schema": 1, "profile": "hex-native-streaming-independent-v1",
        "generator": provenance, "tracks": spec.tracks, "duration_seconds": spec.seconds,
        "waveform": "original integer triangle", "peak_pcm16": AMPLITUDE,
        "track_gain": TRACK_GAIN, "master_gain": MASTER_GAIN,
        "worst_case_output_peak_bound": spec.tracks as f64 * AMPLITUDE as f64 / 32768.0 * f64::from(TRACK_GAIN) * f64::from(MASTER_GAIN),
        "source_files": files, "projects": documents,
        "native_playback_listening_timing": "not measured by generation",
    });
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(directory.join("manifest.json"))?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, &manifest)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    writer.get_ref().sync_all()?;
    File::open(directory.join("media"))?.sync_all()?;
    File::open(directory)?.sync_all()?;
    Ok(manifest)
}
fn provenance() -> Result<Value> {
    let git = |args: &[&str]| -> Result<String> {
        let output = Process::new("git")
            .args(args)
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()?;
        if !output.status.success() {
            return Err("could not identify generator source".into());
        }
        Ok(String::from_utf8(output.stdout)?.trim().to_owned())
    };
    Ok(
        json!({ "source": git(&["rev-parse", "HEAD"])?, "dirty": !git(&["status", "--porcelain"])?.is_empty(), "package_version": env!("CARGO_PKG_VERSION") }),
    )
}
fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let directory = args
        .next()
        .ok_or("usage: native_streaming_fixture new-directory")?;
    if args.next().is_some() {
        return Err("expected exactly one new fixture directory".into());
    }
    let manifest = generate(
        Path::new(&directory),
        Specification {
            tracks: 100,
            seconds: 120,
        },
        provenance()?,
    )?;
    writeln!(
        std::io::stdout().lock(),
        "created {} independent sources, {} seconds, 44100/48000 hz projects; native acceptance remains pending",
        manifest["tracks"],
        manifest["duration_seconds"]
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!(
                "hex-native-streaming-fixture-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )))
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn independent_mixed_rate_sources_roundtrip_shared_session_assets_and_preserve_existing_output()
    -> Result<()> {
        let directory = Directory::new();
        let spec = Specification {
            tracks: 3,
            seconds: 1,
        };
        let manifest = generate(&directory.0, spec, json!({"fixture": true}))?;
        let original = fs::read(directory.0.join("manifest.json"))?;
        let mut hashes = std::collections::BTreeSet::new();
        for source in manifest["source_files"]
            .as_array()
            .ok_or("missing sources")?
        {
            let path = directory
                .0
                .join(source["path"].as_str().ok_or("missing path")?);
            let mut reader = hex_media::VerifiedWavReader::open(path, reader_limits())?;
            assert_eq!(hex(reader.fingerprint()), source["sha256"]);
            hashes.insert(hex(reader.fingerprint()));
            let mut samples = [[0.0f32; 2]; 32];
            reader.read_frames_into(0, &mut samples)?;
            assert!(samples.iter().any(|frame| frame[0] != 0.0));
            assert!(samples.iter().flatten().all(|value| value.abs() <= 0.25));
        }
        assert_eq!(hashes.len(), 3);
        for rate in PROJECT_RATES {
            let project = hex_storage::load(directory.0.join(format!("3-{rate}.hex")))?;
            assert_eq!(project.assets().count(), 3);
            assert_eq!(project.session_cells().len(), 6);
            assert_eq!(project.scenes().len(), 2);
            assert!(project.return_tracks().is_empty());
            assert!(project.assets().all(|asset| asset.path().is_relative()));
            assert!(
                project
                    .tracks()
                    .iter()
                    .all(|track| track.instrument().is_none()
                        && track.devices().is_empty()
                        && track.midi_clips().is_empty())
            );
            assert_eq!(manifest["projects"].as_array().ok_or("projects")?.len(), 2);
        }
        assert!(generate(&directory.0, spec, Value::Null).is_err());
        assert_eq!(fs::read(directory.0.join("manifest.json"))?, original);
        Ok(())
    }
    #[test]
    fn invalid_dimensions_refuse_before_creating_a_destination() {
        for spec in [
            Specification {
                tracks: 0,
                seconds: 120,
            },
            Specification {
                tracks: 101,
                seconds: 120,
            },
            Specification {
                tracks: 100,
                seconds: 121,
            },
        ] {
            let directory = Directory::new();
            assert!(generate(&directory.0, spec, Value::Null).is_err());
            assert!(!directory.0.exists());
        }
    }
}
