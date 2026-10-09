//! Headless native project import and render commands.
use hex_engine::{Audio, compile};
use hex_media::{Limits, ResampleLimits, decode_wav_file, resample};
use hex_project::{Command, Created, Editor, Project};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    io,
    path::Path,
};

const MAX_MEDIA_FILE_BYTES: usize = 64 * 1024 * 1024;
const MAX_DECODED_FRAMES: u64 = 8_000_000;

pub fn import_project(input: &Path, output: &Path) -> Result<(), Box<dyn Error>> {
    let media = decode_wav_file(
        input,
        Limits::new(MAX_MEDIA_FILE_BYTES, MAX_DECODED_FRAMES as usize),
    )?;
    let source_path = if input.is_absolute() {
        input.to_path_buf()
    } else {
        std::env::current_dir()?.join(input)
    };
    let frames = u64::try_from(media.frames())?;
    let mut editor = Editor::new(Project::new(media.sample_rate())?, 1)?;
    match editor.execute(Command::ImportAudioTrack {
        name: "imported audio".into(),
        path: source_path,
        sample_rate: media.sample_rate(),
        frames,
    })? {
        Created::Imported { .. } => {}
        _ => return Err(io::Error::other("audio import did not create a project track").into()),
    }
    hex_storage::save_new(output, editor.project())?;
    println!("created project metadata at {}", output.display());
    Ok(())
}

pub fn render_project(input: &Path, output: &Path) -> Result<(), Box<dyn Error>> {
    let project = hex_storage::load(input)?;
    let mut referenced_assets = BTreeSet::new();
    for clip in project.tracks().iter().flat_map(|track| track.clips()) {
        referenced_assets.insert(clip.asset());
    }

    let project_dir = input
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut media_bank: BTreeMap<_, Audio> = BTreeMap::new();
    let mut decoded_frames = 0_u64;
    for asset_id in referenced_assets {
        let asset = project.asset(asset_id).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "project media reference is missing",
            )
        })?;
        let source_path = if asset.path().is_absolute() {
            asset.path().to_path_buf()
        } else {
            project_dir.join(asset.path())
        };
        let remaining_frames = MAX_DECODED_FRAMES
            .checked_sub(decoded_frames)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "project media exceeds decode limit",
                )
            })?;
        if asset.playback_frames() > remaining_frames {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "project media exceeds prepared playback frame limit",
            )
            .into());
        }
        let media = decode_wav_file(
            &source_path,
            Limits::new(MAX_MEDIA_FILE_BYTES, MAX_DECODED_FRAMES as usize),
        )?;
        let media_frames = u64::try_from(media.frames())?;
        if media.sample_rate() != asset.source_sample_rate()
            || media_frames != asset.source_frames()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "referenced media rate or frame count differs from project metadata",
            )
            .into());
        }
        let prepared = resample(
            media.audio(),
            project.sample_rate(),
            ResampleLimits {
                max_source_frames: MAX_DECODED_FRAMES as usize,
                max_output_frames: usize::try_from(remaining_frames)?,
            },
        )?;
        let playback_frames = u64::try_from(prepared.frames().len())?;
        if playback_frames != asset.playback_frames() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "prepared media length differs from project metadata",
            )
            .into());
        }
        decoded_frames = decoded_frames
            .checked_add(playback_frames)
            .filter(|total| *total <= MAX_DECODED_FRAMES)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "project media exceeds decode limit",
                )
            })?;
        media_bank.insert(asset_id, prepared);
    }

    let playback = compile::project(&project, &media_bank)?;
    let timeline_end = playback
        .audible_end_frame()
        .filter(|end| *end > 0)
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "project has no clips to render")
        })?;
    super::wav::render(output, &playback, timeline_end)?;
    println!("rendered project to {}", output.display());
    Ok(())
}
