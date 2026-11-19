//! Create a reproducible section-edit workload in a new directory.
//! Usage: `cargo run -p hex-session --example create_section_benchmark -- DIR`
//! Writes two deterministic 44.1 kHz PCM16 sources and a saved project with
//! 16 tracks, 512 clips, 16 sections and one stateful Utility filter per track.
//! Existing directories are refused. Use the resulting project.hex with the
//! desktop section_edit_benchmark; this generator measures no runtime timings.
use hex_engine::DeviceTypeId;
use hex_lattice_contract::{ParameterId, ParameterValue};
use hex_project::{BeatTime, Command, Created, Editor, ElementColor, Project};
use std::{error::Error, fs, path::Path};

const RATE: u32 = 44_100;
const FRAMES: u32 = RATE * 2;
type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn source(seed: u32) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(44 + FRAMES as usize * 2);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + FRAMES * 2).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&RATE.to_le_bytes());
    bytes.extend_from_slice(&(RATE * 2).to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(FRAMES * 2).to_le_bytes());
    let mut state = seed;
    for _ in 0..FRAMES {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let sample = ((state >> 20) as i16) - 2048;
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

fn generate(directory: &Path) -> Result<()> {
    fs::create_dir(directory)?;
    let directory = directory.canonicalize()?;
    let mut editor = Editor::new(Project::new(RATE)?, 8)?;
    let mut assets = Vec::new();
    for seed in [1, 91] {
        let path = directory.join(format!("source-{seed}.wav"));
        fs::write(&path, source(seed))?;
        let Created::Imported { track, asset, .. } =
            editor.execute(Command::ImportPreparedAudioTrack {
                name: format!("audio {seed}"),
                path,
                source_sample_rate: RATE,
                source_frames: u64::from(FRAMES),
                playback_frames: u64::from(FRAMES),
                conversion_version: hex_project::IDENTITY_CONVERSION,
            })?
        else {
            return Err("expected imported audio".into());
        };
        assets.push((track, asset));
    }
    for index in 0..16 {
        let (track, first) = if index < 2 {
            (assets[index].0, 1)
        } else {
            let Created::Track(track) = editor.execute(Command::AddTrack {
                name: format!("audio {index}"),
            })?
            else {
                return Err("expected track".into());
            };
            (track, 0)
        };
        let asset = assets[index % 2].1;
        editor.execute(Command::Gain { track, gain: 0.2 })?;
        let state = hex_engine::patch_native_device_parameter(
            &hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.utility"))?,
            ParameterId::new(11)?,
            ParameterValue::Bool(true),
        )?;
        editor.execute(Command::InsertDevice {
            track,
            before: None,
            state,
        })?;
        for clip in first..32 {
            editor.execute(Command::PlaceClip {
                track,
                asset,
                start: u64::from(FRAMES) * clip,
                duration: u64::from(FRAMES),
                source_offset: 0,
                source_length: u64::from(FRAMES),
                looping: false,
            })?;
        }
    }
    for index in 0..16 {
        editor.execute(Command::AddArrangementSection {
            name: format!("section {index}"),
            color: ElementColor::Cyan,
            start: BeatTime::from_quarters(index * 8)?,
            end: BeatTime::from_quarters((index + 1) * 8)?,
        })?;
    }
    fs::write(
        directory.join("project.hex"),
        hex_storage::encode(editor.project())?,
    )?;
    Ok(())
}

fn main() -> Result<()> {
    let directory = std::env::args_os()
        .nth(1)
        .ok_or("usage: create_section_benchmark DIR")?;
    generate(Path::new(&directory))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn baseline_reopens_with_complete_known_sources_and_refuses_overwrite() -> Result<()> {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "hex-section-fixture-{}-{unique}",
            std::process::id()
        ));
        generate(&root)?;
        let project_bytes = fs::read(root.join("project.hex"))?;
        let project = hex_storage::decode(&project_bytes)?;
        assert_eq!(project.tracks().len(), 16);
        assert_eq!(project.arrangement_sections().len(), 16);
        assert_eq!(
            project
                .tracks()
                .iter()
                .map(|t| t.clips().len())
                .sum::<usize>(),
            512
        );
        for seed in [1, 91] {
            let bytes = fs::read(root.join(format!("source-{seed}.wav")))?;
            assert_eq!(bytes, source(seed));
            let audio =
                hex_media::decode_wav(&bytes, hex_media::Limits::new(1_000_000, FRAMES as usize))?;
            assert_eq!(audio.sample_rate(), RATE);
            assert_eq!(audio.frames().len(), FRAMES as usize);
        }
        assert!(generate(&root).is_err());
        assert_eq!(fs::read(root.join("project.hex"))?, project_bytes);
        fs::remove_dir_all(root)?;
        Ok(())
    }
}
