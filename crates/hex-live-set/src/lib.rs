//! Control-side, finite Live-set import; no editable Ableton export or parity claim.
mod mapping;
mod xml;
use hex_engine::Audio;
use hex_project::{AssetId, ImportProvenance, MAX_IMPORT_DOCUMENT_BYTES, Project};
use std::{collections::BTreeMap, fmt, fs::File, io::Read, path::Path};

/// Default policy refuses nonneutral pan because Hex's balance law has not been
/// verified against Live. Opt-in retains the value with an explicit diagnostic.
#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    pub accept_hex_balance: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Diagnostic {
    /// Original bytes include all unknown metadata; they never reflect Hex edits.
    OriginalDocumentRetained,
    /// Numerical pan value retained using Hex's unverified balance law.
    HexBalanceLaw,
    /// No imported devices: MIDI notes remain metadata and do not synthesize sound.
    DeviceFreeMidiIsSilent,
}
impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::OriginalDocumentRetained => {
                "original document and unknown metadata retained unchanged"
            }
            Self::HexBalanceLaw => "nonneutral pan uses hex balance; live pan law is unverified",
            Self::DeviceFreeMidiIsSilent => {
                "device-free midi notes retained without an instrument; playback is silent"
            }
        })
    }
}
/// A validated new project and its resident media; no current project is mutated.
/// Publication belongs to callers using hex-storage's create-new/recovery API.
pub struct Imported {
    pub project: Project,
    pub media: BTreeMap<AssetId, Audio>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Errors deliberately contain no source paths, XML text or parser context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Invalid(&'static str),
    Unsupported(&'static str),
    Limit(&'static str),
    Asset(&'static str),
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (category, reason) = match self {
            Self::Invalid(r) => ("invalid live set", r),
            Self::Unsupported(r) => ("unsupported live set", r),
            Self::Limit(r) => ("live set limit", r),
            Self::Asset(r) => ("live set asset", r),
        };
        write!(f, "{category}: {reason}")
    }
}
impl std::error::Error for Error {}
impl From<hex_project::Invalid> for Error {
    fn from(_: hex_project::Invalid) -> Self {
        Self::Invalid("canonical project validation failed")
    }
}

pub fn import_file(path: &Path, sample_rate: u32, options: Options) -> Result<Imported, Error> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|_| Error::Invalid("source cannot be inspected"))?;
    if !metadata.is_file() {
        return Err(Error::Invalid("source must be a regular file"));
    }
    if metadata.len() > MAX_IMPORT_DOCUMENT_BYTES as u64 {
        return Err(Error::Limit("original document bytes"));
    }
    let file = File::open(path).map_err(|_| Error::Invalid("source cannot be opened"))?;
    let mut bytes = Vec::new();
    file.take(MAX_IMPORT_DOCUMENT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Invalid("source cannot be read"))?;
    let base = path
        .parent()
        .ok_or(Error::Asset("source directory missing"))?;
    import_bytes(&bytes, base, sample_rate, options)
}

/// Plain UTF-8 XML or a single gzip member. Local assets are confined to base.
pub fn import_bytes(
    bytes: &[u8],
    base: &Path,
    sample_rate: u32,
    options: Options,
) -> Result<Imported, Error> {
    let xml = xml::decode(bytes)?;
    xml::preflight(&xml)?;
    let doc = roxmltree::Document::parse_with_options(
        &xml,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: 100_000,
        },
    )
    .map_err(|_| Error::Invalid("malformed xml"))?;
    let mut imported = mapping::map(&doc, base, sample_rate, options)?;
    let mut snapshot = imported.project.snapshot();
    snapshot.import_provenance = Some(ImportProvenance::new(
        "ableton-live-set",
        1,
        1,
        bytes.to_vec(),
    )?);
    imported.project = Project::from_snapshot(snapshot)?;
    hex_engine::compile::project(&imported.project, &imported.media)
        .map_err(|_| Error::Invalid("candidate cannot compile"))?;
    Ok(imported)
}
