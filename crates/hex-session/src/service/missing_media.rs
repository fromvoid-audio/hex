//! Read-only missing-media report, candidate checks and bounded folder search
//! for relinking a project before it opens.
//!
//! Everything here runs on control/background threads and only reads the file
//! system: user media is never written, moved or deleted. The result is a set
//! of `AssetId -> PathBuf` relinks for [`super::Request::OpenWith`], whose open
//! path still decodes and verifies every file; this module only pre-screens.
//!
//! Matching rules and limits:
//! - candidates are matched by file name (ASCII case-insensitive), then by the
//!   WAV header's source frame count and sample rate against saved asset
//!   metadata. Ordinary assets store no content fingerprint, so two different
//!   files with equal name, length and rate are indistinguishable here.
//!   Device-original assets carry a content digest that the open path verifies.
//! - headers that cannot be read without decoding (e.g. adpcm) are reported as
//!   unverified rather than mismatched; the open path remains the authority.
//! - folder search is bounded by entry count, depth and candidates per item,
//!   follows directory symlinks at most once per canonical directory, counts
//!   unreadable entries instead of failing, and stops promptly when cancelled.
use super::ProjectSource;
use super::recovery::read_source;
use hex_project::{AssetId, Project};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

/// Header scan ceilings for candidate checks; no PCM is decoded.
const CANDIDATE_LIMITS: hex_media::WavReaderLimits =
    hex_media::WavReaderLimits::new(4 * 1024 * 1024 * 1024, u64::MAX, 64, 1);
/// Usage rows retained per missing file; further uses are only counted.
const MAX_USAGES: usize = 16;

/// One saved asset whose file is absent at its last known location.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissingMedia {
    pub asset: AssetId,
    /// Last known location, resolved against the project folder.
    pub expected_path: PathBuf,
    pub source_frames: u64,
    pub source_sample_rate: u32,
    /// Device-owned original whose saved content digest the open path verifies.
    pub device_original: bool,
    /// At most `MAX_USAGES` places that use this asset.
    pub usages: Vec<MediaUsage>,
    /// Total use count, including uses beyond `usages`.
    pub usage_count: usize,
}

impl MissingMedia {
    /// File name as last saved, for display and name matching.
    pub fn file_name(&self) -> String {
        self.expected_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// Where a missing asset is used, with user-authored names preserved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaUsage {
    ArrangementClip {
        track: String,
        clip: String,
    },
    SessionClip {
        track: String,
        scene: String,
        clip: String,
    },
    Device {
        track: String,
        device: String,
    },
}

impl MediaUsage {
    /// Compact description; app text is lowercased by the UI theme.
    pub fn describe(&self) -> String {
        match self {
            Self::ArrangementClip { track, clip } => format!("{track} · {clip}"),
            Self::SessionClip { track, scene, clip } => format!("{track} · {scene} · {clip}"),
            Self::Device { track, device } => format!("{track} · {device}"),
        }
    }
}

/// How a candidate file compares to the saved asset metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CandidateFit {
    /// Source frame count and sample rate equal the saved asset.
    Matches,
    /// Readable, but the open path would refuse it.
    Mismatch(String),
    /// Exists, but its header could not be checked without decoding.
    Unverified(String),
    /// Absent, unreadable or not a regular file.
    Unusable(String),
}

impl CandidateFit {
    /// Whether the panel may submit this file for the open path to verify.
    pub fn usable(&self) -> bool {
        matches!(self, Self::Matches | Self::Unverified(_))
    }
}

/// Lists every asset whose file is absent, with its uses. An existing path that
/// is not a regular file, or one that cannot be inspected, is an error so the
/// caller does not offer to replace media that may still be reachable.
pub fn missing_media_report(source: &ProjectSource) -> Result<Vec<MissingMedia>, String> {
    let (project, base, _) = read_source(source)?;
    report_for(&project, &base)
}

pub(super) fn report_for(project: &Project, base: &Path) -> Result<Vec<MissingMedia>, String> {
    let mut missing = Vec::new();
    for asset in project.assets() {
        let path = if asset.path().is_absolute() {
            asset.path().to_path_buf()
        } else {
            base.join(asset.path())
        };
        match std::fs::metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing.push(MissingMedia {
                    asset: asset.id(),
                    expected_path: path,
                    source_frames: asset.source_frames(),
                    source_sample_rate: asset.source_sample_rate(),
                    device_original: asset.original_source().is_some(),
                    usages: Vec::new(),
                    usage_count: 0,
                })
            }
            Err(error) => {
                return Err(format!(
                    "{}: {error}",
                    hex_storage::text::verbatim(path.display())
                ));
            }
            Ok(metadata) if !metadata.is_file() => {
                return Err(format!(
                    "media is not a regular file: {}",
                    hex_storage::text::verbatim(path.display())
                ));
            }
            Ok(_) => {}
        }
    }
    Ok(with_usages(project, missing))
}

/// Offline assets of an open project (missing when it opened), with their uses,
/// for relinking in place. Relative paths resolve against `base`, the saved
/// project's folder; without one they stay as saved. Reads no files.
pub fn offline_media_report(project: &Project, base: Option<&Path>) -> Vec<MissingMedia> {
    let missing = project
        .assets()
        .filter(|asset| project.asset_offline(asset.id()))
        .map(|asset| MissingMedia {
            asset: asset.id(),
            expected_path: match base {
                Some(base) if !asset.path().is_absolute() => base.join(asset.path()),
                _ => asset.path().to_path_buf(),
            },
            source_frames: asset.source_frames(),
            source_sample_rate: asset.source_sample_rate(),
            device_original: asset.original_source().is_some(),
            usages: Vec::new(),
            usage_count: 0,
        })
        .collect();
    with_usages(project, missing)
}

fn with_usages(project: &Project, mut missing: Vec<MissingMedia>) -> Vec<MissingMedia> {
    if missing.is_empty() {
        return missing;
    }
    let index: HashMap<u64, usize> = missing
        .iter()
        .enumerate()
        .map(|(i, item)| (item.asset.value(), i))
        .collect();
    let mut record = |asset: u64, usage: MediaUsage| {
        if let Some(item) = index.get(&asset).and_then(|i| missing.get_mut(*i)) {
            item.usage_count = item.usage_count.saturating_add(1);
            if item.usages.len() < MAX_USAGES {
                item.usages.push(usage);
            }
        }
    };
    let device_uses =
        |track: &str, device: &hex_project::Device, record: &mut dyn FnMut(u64, MediaUsage)| {
            let type_id = device.state().type_id();
            let name = type_id
                .as_str()
                .ok()
                .and_then(|id| id.rsplit('.').next())
                .unwrap_or("device")
                .to_owned();
            for reference in device.state().asset_references() {
                record(
                    reference.asset.value(),
                    MediaUsage::Device {
                        track: track.to_owned(),
                        device: name.clone(),
                    },
                );
            }
        };
    for track in project.tracks() {
        for clip in track.clips() {
            record(
                clip.asset().value(),
                MediaUsage::ArrangementClip {
                    track: track.name().to_owned(),
                    clip: clip.name().to_owned(),
                },
            );
        }
        for device in track.instrument().into_iter().chain(track.devices()) {
            device_uses(track.name(), device, &mut record);
        }
    }
    for track in project.return_tracks() {
        for device in track.devices() {
            device_uses(track.name(), device, &mut record);
        }
    }
    for cell in project.session_cells() {
        if let hex_project::SessionCellContent::Audio(clip) = cell.content() {
            let track = project
                .tracks()
                .iter()
                .find(|track| track.id() == cell.track())
                .map_or("", |track| track.name());
            let scene = project
                .scenes()
                .iter()
                .find(|scene| scene.id() == cell.scene())
                .map_or("", |scene| scene.name());
            record(
                clip.asset().value(),
                MediaUsage::SessionClip {
                    track: track.to_owned(),
                    scene: scene.to_owned(),
                    clip: clip.name().to_owned(),
                },
            );
        }
    }
    missing
}

/// Compares a candidate's WAV header with the saved asset metadata.
pub fn check_candidate(missing: &MissingMedia, path: &Path) -> CandidateFit {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return CandidateFit::Unusable("not a regular file".into()),
        Err(error) => return CandidateFit::Unusable(error.to_string()),
    }
    let reader = match hex_media::WavFrameReader::open(path, CANDIDATE_LIMITS) {
        Ok(reader) => reader,
        Err(error) => return CandidateFit::Unverified(error.to_string()),
    };
    if reader.sample_rate() != missing.source_sample_rate {
        return CandidateFit::Mismatch(format!(
            "sample rate {} hz, expected {} hz",
            reader.sample_rate(),
            missing.source_sample_rate
        ));
    }
    if reader.source_frames() != missing.source_frames {
        return CandidateFit::Mismatch(format!(
            "length {} frames, expected {} frames",
            reader.source_frames(),
            missing.source_frames
        ));
    }
    CandidateFit::Matches
}

/// Live-style relative resolve: after `chosen` was located at `new_path`, maps
/// every other unresolved item under the same moved folder to the new folder.
///
/// The moved roots are the old and new paths minus their longest common
/// trailing components, so a sample picked from a renamed parent folder also
/// maps its siblings in other subfolders. Only matching files are returned.
pub fn resolve_relative(
    missing: &[MissingMedia],
    resolved: &BTreeMap<AssetId, PathBuf>,
    chosen: &MissingMedia,
    new_path: &Path,
) -> Vec<(AssetId, PathBuf)> {
    let old: Vec<_> = chosen.expected_path.components().collect();
    let new: Vec<_> = new_path.components().collect();
    let mut shared = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    // A renamed file keeps no trailing component; map from its parent folders.
    shared = shared.max(1);
    let (Some(old_len), Some(new_len)) =
        (old.len().checked_sub(shared), new.len().checked_sub(shared))
    else {
        return Vec::new();
    };
    let old_root: PathBuf = old[..old_len].iter().collect();
    let new_root: PathBuf = new[..new_len].iter().collect();
    if old_root.as_os_str().is_empty() || new_root.as_os_str().is_empty() {
        return Vec::new();
    }
    missing
        .iter()
        .filter(|item| item.asset != chosen.asset && !resolved.contains_key(&item.asset))
        .filter_map(|item| {
            let relative = item.expected_path.strip_prefix(&old_root).ok()?;
            let candidate = new_root.join(relative);
            (check_candidate(item, &candidate) == CandidateFit::Matches)
                .then_some((item.asset, candidate))
        })
        .collect()
}

/// Ceilings for one folder search.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchLimits {
    /// Directory entries examined across all roots.
    pub max_entries: usize,
    /// Directory depth below each root.
    pub max_depth: usize,
    /// Name-matching candidates kept per missing item.
    pub max_candidates: usize,
}

impl Default for SearchLimits {
    fn default() -> Self {
        Self {
            max_entries: 2_000_000,
            max_depth: 32,
            max_candidates: 8,
        }
    }
}

/// Why a search ended before visiting every reachable entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchStop {
    Cancelled,
    EntryLimit,
}

/// Name-matched candidates per item, in discovery order, with their fit.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchOutcome {
    pub candidates: BTreeMap<AssetId, Vec<(PathBuf, CandidateFit)>>,
    pub scanned: usize,
    /// Directories or entries that could not be read and were skipped.
    pub unreadable: usize,
    /// Directories below `max_depth` that were not entered.
    pub depth_skipped: usize,
    pub stopped: Option<SearchStop>,
}

impl SearchOutcome {
    /// The single matching candidate, when exactly one file matches.
    pub fn unique_match(&self, asset: AssetId) -> Option<&Path> {
        let mut matches = self
            .candidates
            .get(&asset)?
            .iter()
            .filter(|(_, fit)| *fit == CandidateFit::Matches);
        let (path, _) = matches.next()?;
        matches.next().is_none().then_some(path.as_path())
    }
}

/// Recursively searches `roots` for files named like the missing items.
/// `progress` receives the running entry count; `cancel` is polled per entry.
pub fn search_folders(
    missing: &[MissingMedia],
    roots: &[PathBuf],
    limits: SearchLimits,
    cancel: &AtomicBool,
    progress: &AtomicUsize,
) -> SearchOutcome {
    let mut wanted: HashMap<String, Vec<&MissingMedia>> = HashMap::new();
    for item in missing {
        wanted
            .entry(item.file_name().to_ascii_lowercase())
            .or_default()
            .push(item);
    }
    let mut outcome = SearchOutcome::default();
    let mut visited = HashSet::new();
    let mut stack: Vec<(PathBuf, usize)> = roots.iter().rev().map(|r| (r.clone(), 0)).collect();
    while let Some((directory, depth)) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            outcome.stopped = Some(SearchStop::Cancelled);
            return outcome;
        }
        // Canonical identity breaks symlink cycles and repeated roots.
        let Ok(canonical) = std::fs::canonicalize(&directory) else {
            outcome.unreadable += 1;
            continue;
        };
        if !visited.insert(canonical) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&directory) else {
            outcome.unreadable += 1;
            continue;
        };
        let mut subdirectories = Vec::new();
        for entry in entries {
            if cancel.load(Ordering::Relaxed) {
                outcome.stopped = Some(SearchStop::Cancelled);
                return outcome;
            }
            if outcome.scanned >= limits.max_entries {
                outcome.stopped = Some(SearchStop::EntryLimit);
                return outcome;
            }
            outcome.scanned += 1;
            progress.store(outcome.scanned, Ordering::Relaxed);
            let Ok(entry) = entry else {
                outcome.unreadable += 1;
                continue;
            };
            let path = entry.path();
            // Follows symlinks; broken links count as unreadable.
            let Ok(metadata) = std::fs::metadata(&path) else {
                outcome.unreadable += 1;
                continue;
            };
            if metadata.is_dir() {
                if depth < limits.max_depth {
                    subdirectories.push((path, depth + 1));
                } else {
                    outcome.depth_skipped += 1;
                }
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            let Some(items) = wanted.get(&name) else {
                continue;
            };
            for item in items {
                let list = outcome.candidates.entry(item.asset).or_default();
                if list.len() < limits.max_candidates {
                    list.push((path.clone(), check_candidate(item, &path)));
                }
            }
        }
        // Visit subdirectories in name order for deterministic results.
        subdirectories.sort();
        stack.extend(subdirectories.into_iter().rev());
    }
    outcome
}

/// Checks candidates proposed by another source, such as the browser's sample
/// catalog, with the same name and frame/rate rules as [`search_folders`]. The
/// outcome feeds the panel exactly like a folder search, so there is no second
/// matcher. Unknown assets, and paths whose file name differs from the missing
/// file, are ignored; at most `max_candidates` paths are checked per item.
/// `scanned` counts checked paths. Read-only.
pub fn check_candidate_lists(
    missing: &[MissingMedia],
    lists: &BTreeMap<AssetId, Vec<PathBuf>>,
    max_candidates: usize,
    cancel: &AtomicBool,
) -> SearchOutcome {
    let mut outcome = SearchOutcome::default();
    for item in missing {
        let Some(paths) = lists.get(&item.asset) else {
            continue;
        };
        let wanted = item.file_name().to_ascii_lowercase();
        let mut checked = Vec::new();
        for path in paths
            .iter()
            .filter(|path| {
                path.file_name()
                    .is_some_and(|name| name.to_string_lossy().to_ascii_lowercase() == wanted)
            })
            .take(max_candidates)
        {
            if cancel.load(Ordering::Relaxed) {
                outcome.stopped = Some(SearchStop::Cancelled);
                return outcome;
            }
            outcome.scanned += 1;
            let fit = check_candidate(item, path);
            if matches!(fit, CandidateFit::Unusable(_)) {
                outcome.unreadable += 1;
            }
            checked.push((path.clone(), fit));
        }
        if !checked.is_empty() {
            outcome.candidates.insert(item.asset, checked);
        }
    }
    outcome
}

#[cfg(test)]
mod tests;
