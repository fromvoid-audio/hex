//! Headless native project import and render commands.
use hex_engine::Audio;
use hex_media::{Limits, ResampleLimits, ResampleVersion, decode_wav_file, resample_with_version};
use hex_project::{Command, Created, Editor, Project};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    io,
    path::Path,
    sync::Arc,
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

fn load_arrangement(
    input: &Path,
) -> Result<(Project, BTreeMap<hex_project::AssetId, Audio>), Box<dyn Error>> {
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
        // Reproduce the asset's saved conversion law; v1 media stays v1.
        let prepared = resample_with_version(
            media.audio(),
            project.sample_rate(),
            ResampleLimits {
                max_source_frames: MAX_DECODED_FRAMES as usize,
                max_output_frames: usize::try_from(remaining_frames)?,
            },
            ResampleVersion::for_conversion(asset.conversion_version())?,
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

    Ok((project, media_bank))
}

/// Render the whole project through the canonical automatic Main export range:
/// the causal renderer and declared-tail sizing the UI's `ExportRange` uses
/// when no explicit frame count is given. Publication is create-new only.
pub fn render_project(input: &Path, output: &Path) -> Result<(), Box<dyn Error>> {
    let (project, media_bank) = load_arrangement(input)?;
    let prepared = prepare(&project, &media_bank)?;
    let signal = hex_export::ExportSignal::Main;
    let frames = hex_export::controlled_frame_count(prepared.arrangement(), 0, None, &signal)?;
    let destination = output.to_owned();
    let report = std::thread::Builder::new()
        .name("hex-export".into())
        .stack_size(EXPORT_STACK_BYTES)
        .spawn(move || {
            hex_export::render_prepared_with_policy(
                &destination,
                prepared,
                0,
                frames,
                signal,
                hex_export::ExportFormat::default(),
                &hex_export::ExportControl::default(),
                &hex_export::Publication::CreateNew,
            )
        })?
        .join()
        .map_err(|_| "export worker stopped")??;
    println!(
        "rendered {} frames to {}",
        report.frames,
        report.path.display()
    );
    Ok(())
}

const EXPORT_STACK_BYTES: usize = 8 * 1024 * 1024;

/// `--format=` value: dithered 16-bit PCM (default), packed 24-bit PCM or
/// 32-bit float.
fn parse_format(name: &str) -> Result<hex_export::ExportFormat, Box<dyn Error>> {
    match name {
        "pcm16" => Ok(hex_export::ExportFormat::Pcm16),
        "pcm24" => Ok(hex_export::ExportFormat::Pcm24),
        "float32" => Ok(hex_export::ExportFormat::Float32),
        _ => Err("unknown export format; use pcm16, pcm24 or float32".into()),
    }
}

fn prepare(
    project: &Project,
    media: &BTreeMap<hex_project::AssetId, Audio>,
) -> Result<Arc<hex_engine::PreparedRevision>, Box<dyn Error>> {
    Ok(Arc::new(hex_engine::PreparedRevision::prepare(
        project,
        media,
        Arc::new(hex_engine::SineTable::new()),
        None,
    )?))
}

/// Explicit offline range/scope adapter; retains the bounded project/media loader.
pub fn export_project(args: &[std::ffi::OsString]) -> Result<(), Box<dyn Error>> {
    if !(5..=8).contains(&args.len()) {
        return Err("usage: hex-cli export-project <project.hex> <destination> <start-frame> <end-frame> [--tracks=ids|--returns=ids|--stems=track-ids] [--format=pcm16|pcm24|float32] [--overwrite]".into());
    }
    let start: u64 = args[3].to_str().ok_or("invalid start frame")?.parse()?;
    let end: u64 = args[4].to_str().ok_or("invalid end frame")?.parse()?;
    let frames = end
        .checked_sub(start)
        .filter(|frames| *frames > 0)
        .ok_or("export range must be nonempty and ordered")?;
    let mut scope = None;
    let mut overwrite = false;
    let mut format = None;
    for arg in &args[5..] {
        let text = arg.to_str().ok_or("invalid export option")?;
        if text == "--overwrite" && !overwrite {
            overwrite = true;
        } else if let Some(name) = text.strip_prefix("--format=").filter(|_| format.is_none()) {
            format = Some(parse_format(name)?);
        } else if scope.is_none()
            && (text.starts_with("--tracks=")
                || text.starts_with("--returns=")
                || text.starts_with("--stems="))
        {
            scope = Some(text);
        } else {
            return Err("unknown or repeated export option".into());
        }
    }
    let (project, media) = load_arrangement(Path::new(&args[1]))?;
    let signal = if let Some(scope) = scope {
        let (kind, list) = scope.split_once('=').ok_or("invalid export scope")?;
        let ids: Vec<u64> = list.split(',').map(str::parse).collect::<Result<_, _>>()?;
        if ids.is_empty() {
            return Err("export selection is empty".into());
        }
        if kind == "--returns" {
            hex_export::ExportSignal::CapturedMix(
                ids.iter()
                    .map(|id| {
                        project
                            .return_tracks()
                            .iter()
                            .find(|bus| bus.id().value() == *id)
                            .map(|bus| hex_engine::OutputCaptureId::Return(bus.id()))
                            .ok_or("unknown return id")
                    })
                    .collect::<Result<_, _>>()?,
            )
        } else {
            let selected = ids
                .iter()
                .map(|id| {
                    project
                        .tracks()
                        .iter()
                        .find(|track| track.id().value() == *id)
                        .ok_or("unknown track id")
                })
                .collect::<Result<Vec<_>, _>>()?;
            if kind == "--stems" {
                let mut stems: Vec<_> = selected
                    .iter()
                    .map(|track| hex_export::Stem {
                        id: hex_engine::OutputCaptureId::Track(track.id()),
                        name: track.name().to_owned(),
                    })
                    .collect();
                stems.extend(project.return_tracks().iter().map(|bus| hex_export::Stem {
                    id: hex_engine::OutputCaptureId::Return(bus.id()),
                    name: bus.name().to_owned(),
                }));
                hex_export::ExportSignal::Stems(stems)
            } else {
                hex_export::ExportSignal::CapturedMix(
                    selected
                        .iter()
                        .map(|track| hex_engine::OutputCaptureId::Track(track.id()))
                        .collect(),
                )
            }
        }
    } else {
        hex_export::ExportSignal::Main
    };
    let prepared = prepare(&project, &media)?;
    let path = Path::new(&args[2]);
    // --overwrite consents to replacing an existing file; an absent target is
    // still published create-new, matching the UI's confirmed save.
    let publication = if overwrite {
        hex_export::Publication::confirmed_save(path)?
    } else {
        hex_export::Publication::CreateNew
    };
    let control = hex_export::ExportControl::default();
    // Stdin cancellation is handled away from rendering. EOF is harmless; this
    // detached reader never delays completion or joins while waiting for input.
    let cancellation = control.clone();
    std::thread::Builder::new()
        .name("hex-export-cancel".into())
        .spawn(move || {
            let mut line = String::new();
            if std::io::BufRead::read_line(
                &mut std::io::Read::take(std::io::stdin().lock(), 16),
                &mut line,
            )
            .is_ok()
                && line.trim() == "cancel"
            {
                cancellation.cancel();
            }
        })?;
    eprintln!("exporting frames {start}..{end}; type cancel and press enter to cancel");
    let destination = path.to_owned();
    let render_control = control.clone();
    let worker = std::thread::Builder::new()
        .name("hex-export".into())
        .stack_size(EXPORT_STACK_BYTES)
        .spawn(move || {
            hex_export::render_prepared_with_policy(
                &destination,
                prepared,
                start,
                frames,
                signal,
                format.unwrap_or_default(),
                &render_control,
                &publication,
            )
        })?;
    let mut previous = None;
    while !worker.is_finished() {
        let progress = control.progress();
        if previous != Some(progress) {
            eprintln!(
                "export {} / {} frames",
                progress.processed_frames, progress.total_frames
            );
            previous = Some(progress);
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let report = worker.join().map_err(|_| "export worker stopped")??;
    println!(
        "exported {} frames to {}",
        report.frames,
        report.path.display()
    );
    if let Some(recovery) = report.recovery_path {
        println!("previous file preserved at {}", recovery.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_format;
    use hex_export::ExportFormat;

    #[test]
    fn format_option_names_map_to_export_formats_and_refuse_unknown_names() {
        assert_eq!(parse_format("pcm16").unwrap(), ExportFormat::Pcm16);
        assert_eq!(parse_format("pcm24").unwrap(), ExportFormat::Pcm24);
        assert_eq!(parse_format("float32").unwrap(), ExportFormat::Float32);
        for name in ["", "PCM24", "pcm32", "float64"] {
            assert!(parse_format(name).is_err(), "{name}");
        }
    }
}
