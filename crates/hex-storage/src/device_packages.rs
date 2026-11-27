//! Atomic control-side validator, installer, and persistent catalog for
//! `.hexdevice` native device packages (`#2991`).
//!
//! # Purpose
//! Parse, validate, and atomically publish immutable native device package
//! generations into a user or factory library directory without ever loading
//! or executing module code during installation or catalog startup.
//!
//! # Security & Trust Semantics
//! - **No code execution on parse/install**: this module never invokes `dlopen`,
//!   executes install scripts, or performs network resolution. Executable slices
//!   are validated structurally (header magic, declared digest, bounded size)
//!   and remain inert bytes on disk until an explicit runtime adapter admits
//!   them.
//! - **Origin authentication vs content integrity**: SHA-256 content digests
//!   verify integrity only; they do not authenticate origin. Admission requires
//!   an explicit [`AdmissionPolicy`] distinguishing signed distribution
//!   credentials from reviewed development/test fixture authorizations.
//! - **OS quarantine preservation**: on macOS, if the source `.hexdevice`
//!   archive carries `com.apple.quarantine`, the installer records
//!   `quarantined = true` in the catalog and propagates the xattr onto the
//!   installed native slice rather than stripping OS trust metadata.
//! - **Atomicity & recovery**: concurrent installs are serialized via an
//!   exclusive advisory lock (`.installer.lock`). Payloads are staged under
//!   `staging/`, synced, published into immutable `generations/`, and finally
//!   committed into `catalog.json` via write-sync-rename. Interrupted stages
//!   are never visible in `catalog.json` and are cleaned safely on startup
//!   recovery.

use fs2::FileExt;
use hex_lattice_contract::package_abi::{
    CAP_AUDIO_EFFECT, CAP_INSTRUMENT, CAP_KNOWN_MASK_V1, DEVICE_KIND_AUDIO_EFFECT,
    DEVICE_KIND_INSTRUMENT, ManifestVersions, PACKAGE_ABI_VERSION_V1, PackageIdentityEnvelope,
};
use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Magic 8-byte header identifying a v1 `.hexdevice` package archive.
pub const HEXDEVICE_ARCHIVE_MAGIC: &[u8; 8] = b"HEXDEV01";
/// Maximum total `.hexdevice` archive size accepted by the installer (32 MiB).
pub const MAX_ARCHIVE_BYTES: usize = 32 * 1024 * 1024;
/// Maximum total expanded payload bytes across all files in one archive (64 MiB).
pub const MAX_EXPANDED_BYTES: usize = 64 * 1024 * 1024;
/// Maximum size of any single file inside a package archive (16 MiB).
pub const MAX_SINGLE_FILE_BYTES: usize = 16 * 1024 * 1024;
/// Maximum number of files inside one `.hexdevice` archive.
pub const MAX_ARCHIVE_FILES: usize = 128;
/// Maximum declared native dependencies per package.
pub const MAX_DECLARED_DEPENDENCIES: usize = 16;
/// Maximum allowed declared expansion ratio (guards decompression bombs).
pub const MAX_EXPANSION_RATIO: usize = 16;

/// Entry kind carried in a `.hexdevice` archive header. Only `RegularFile` is
/// permitted; symlinks and hardlinks are explicitly rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveEntryKind {
    RegularFile,
    Symlink,
    Hardlink,
    Special,
}

/// Compression mode declared on an archive entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveCompression {
    Stored,
    Unsupported,
}

/// Declared self-contained native dependency metadata.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclaredDependency {
    pub name: String,
    pub sha256: String,
    pub bundled_path: String,
}

/// Provenance and origin trust metadata carried inside a package manifest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "tier", rename_all = "snake_case", deny_unknown_fields)]
pub enum PackageProvenance {
    /// Reviewed development or test fixture authorization, strictly separated
    /// from production distribution signing credentials.
    ReviewedDevelopmentTest {
        authorization_sha256: String,
        reviewer_note: String,
    },
    /// Signed distribution provenance bound to a publisher key identifier.
    SignedDistribution {
        publisher_key_id: String,
        signature_hex: String,
    },
}

/// Strict package manifest stored inside `.hexdevice`. Unknown fields (such as
/// install scripts or network hooks) are rejected via `deny_unknown_fields`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageManifest {
    pub schema_version: u32,
    pub publisher_id: String,
    pub type_id: String,
    pub display_name: String,
    pub package_major: u16,
    pub package_minor: u16,
    pub package_patch: u16,
    pub state_version: u32,
    pub abi_version: u32,
    pub ui_version: u32,
    pub device_kind: u32,
    pub capabilities: u64,
    pub target_os: String,
    pub target_arch: String,
    pub min_os_version: String,
    pub native_module_path: String,
    pub ui_schema_path: Option<String>,
    pub presets: Vec<String>,
    pub assets: Vec<String>,
    pub native_dependencies: Vec<DeclaredDependency>,
    pub content_digests: BTreeMap<String, String>,
    pub provenance: PackageProvenance,
}

impl PackageManifest {
    pub fn versions(&self) -> ManifestVersions {
        ManifestVersions {
            package_major: self.package_major,
            package_minor: self.package_minor,
            package_patch: self.package_patch,
            state_version: self.state_version,
            abi_version: self.abi_version,
            ui_version: self.ui_version,
        }
    }

    pub fn version_tuple(&self) -> (u16, u16, u16) {
        (self.package_major, self.package_minor, self.package_patch)
    }
}

/// Single entry inside a `.hexdevice` archive envelope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveEntryHeader {
    pub path: String,
    pub kind: ArchiveEntryKind,
    pub compression: ArchiveCompression,
    pub compressed_bytes: usize,
    pub expanded_bytes: usize,
}

/// Binary archive index header preceding raw payload slices.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HexDeviceArchiveHeader {
    pub manifest: PackageManifest,
    pub entries: Vec<ArchiveEntryHeader>,
}

/// Control-side admission policy separating distribution trust from reviewed
/// development/test fixture admission.
#[derive(Clone, Debug, Default)]
pub struct AdmissionPolicy {
    pub allowed_distribution_keys: HashSet<(String, String)>,
    pub allowed_reviewed_test_authorizations: HashSet<(String, String)>,
    pub current_os: String,
    pub current_arch: String,
}

impl AdmissionPolicy {
    /// Default host target policy with no pre-authorized test fixtures.
    pub fn host_default() -> Self {
        Self {
            allowed_distribution_keys: HashSet::new(),
            allowed_reviewed_test_authorizations: HashSet::new(),
            current_os: std::env::consts::OS.to_owned(),
            current_arch: std::env::consts::ARCH.to_owned(),
        }
    }

    /// Explicitly admit a reviewed development/test fixture publisher and
    /// authorization token digest for offline testing.
    pub fn allow_reviewed_test(
        &mut self,
        publisher_id: impl Into<String>,
        auth_sha256: impl Into<String>,
    ) {
        self.allowed_reviewed_test_authorizations
            .insert((publisher_id.into(), auth_sha256.into()));
    }

    /// Explicitly admit a signed distribution publisher key identifier.
    pub fn allow_distribution_key(
        &mut self,
        publisher_id: impl Into<String>,
        key_id: impl Into<String>,
    ) {
        self.allowed_distribution_keys
            .insert((publisher_id.into(), key_id.into()));
    }
}

/// Origin of an installed package generation (`#2996`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageOrigin {
    #[default]
    User,
    Factory,
}

/// Persisted immutable entry in the installed device package catalog.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledPackageRecord {
    pub type_id: String,
    pub publisher_id: String,
    pub package_major: u16,
    pub package_minor: u16,
    pub package_patch: u16,
    pub state_version: u32,
    pub abi_version: u32,
    pub ui_version: u32,
    pub package_sha256: String,
    pub generation_rel_dir: String,
    pub native_module_rel_path: String,
    pub quarantined: bool,
    #[serde(default)]
    pub origin: PackageOrigin,
    #[serde(default)]
    pub removal_pending: bool,
    pub manifest: PackageManifest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogFile {
    catalog_version: u32,
    packages: Vec<InstalledPackageRecord>,
    #[serde(default)]
    selected_defaults: BTreeMap<String, String>,
}

/// Result of an atomic package installation attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallOutcome {
    Installed(InstalledPackageRecord),
    AlreadyInstalled(InstalledPackageRecord),
}

/// Reason why a pinned `NativeDeviceState` package generation cannot be
/// resolved for active execution (`#2993`). Original opaque state remains
/// preserved verbatim until the user resolves, reinstalls, bypasses, or removes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnavailablePackageReason {
    Missing {
        expected: PackageIdentityEnvelope,
    },
    Quarantined {
        expected: PackageIdentityEnvelope,
        record: Box<InstalledPackageRecord>,
    },
    VersionAbsent {
        expected: PackageIdentityEnvelope,
        installed_versions: Vec<(u16, u16, u16)>,
    },
    HashConflict {
        expected: PackageIdentityEnvelope,
        installed_sha256: String,
    },
    IncompatibleAbi {
        expected: PackageIdentityEnvelope,
        installed_abi: u32,
    },
    PublisherMismatch {
        expected: PackageIdentityEnvelope,
        installed_publisher: String,
    },
}

/// Deterministic exact-version resolution result for a persisted native device
/// package state. Never performs silent latest-version substitution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PackageResolution {
    Available(Box<InstalledPackageRecord>),
    Unavailable(UnavailablePackageReason),
}

/// Errors returned by `.hexdevice` validation, installation, or recovery.
#[derive(Debug)]
pub enum PackageInstallError {
    Io(io::Error),
    MalformedArchive(&'static str),
    InvalidManifest(&'static str),
    InvalidPath(String),
    PathCollision(String),
    ForbiddenEntryKind(String),
    DecompressionBomb(&'static str),
    LimitExceeded(&'static str),
    UnsupportedPlatform(&'static str),
    UnsupportedExecutable(&'static str),
    DigestMismatch(String),
    UntrustedProvenance(&'static str),
    PublisherTakeover {
        type_id: String,
        existing_publisher: String,
        candidate_publisher: String,
    },
    VersionHashConflict {
        type_id: String,
        version: (u16, u16, u16),
    },
    Cancelled,
}

impl fmt::Display for PackageInstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "package io error: {e}"),
            Self::MalformedArchive(m) => write!(f, "malformed package archive: {m}"),
            Self::InvalidManifest(m) => write!(f, "invalid package manifest: {m}"),
            Self::InvalidPath(p) => write!(f, "invalid package path: {p}"),
            Self::PathCollision(p) => write!(f, "conflicting package path: {p}"),
            Self::ForbiddenEntryKind(p) => write!(f, "forbidden archive entry kind: {p}"),
            Self::DecompressionBomb(m) => write!(f, "package expansion limit exceeded: {m}"),
            Self::LimitExceeded(m) => write!(f, "package bound exceeded: {m}"),
            Self::UnsupportedPlatform(m) => write!(f, "unsupported package platform: {m}"),
            Self::UnsupportedExecutable(m) => write!(f, "unsupported native executable: {m}"),
            Self::DigestMismatch(p) => write!(f, "content digest mismatch: {p}"),
            Self::UntrustedProvenance(m) => write!(f, "untrusted package provenance: {m}"),
            Self::PublisherTakeover {
                type_id,
                existing_publisher,
                candidate_publisher,
            } => write!(
                f,
                "publisher takeover refused for {type_id}: owned by {existing_publisher}, got {candidate_publisher}"
            ),
            Self::VersionHashConflict { type_id, version } => write!(
                f,
                "package {type_id} {}.{}.{} already installed with different content hash",
                version.0, version.1, version.2
            ),
            Self::Cancelled => f.write_str("package installation cancelled"),
        }
    }
}

impl std::error::Error for PackageInstallError {}

impl From<io::Error> for PackageInstallError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

/// Encode a deterministic `.hexdevice` archive in memory for fixtures and
/// reproducible package builders (`#2997`).
pub fn encode_hexdevice_archive(
    manifest: &PackageManifest,
    files: &[(&str, ArchiveEntryKind, ArchiveCompression, usize, &[u8])],
) -> Result<Vec<u8>, PackageInstallError> {
    let entries: Vec<ArchiveEntryHeader> = files
        .iter()
        .map(
            |&(path, kind, compression, expanded_bytes, bytes)| ArchiveEntryHeader {
                path: path.to_owned(),
                kind,
                compression,
                compressed_bytes: bytes.len(),
                expanded_bytes,
            },
        )
        .collect();
    let header = HexDeviceArchiveHeader {
        manifest: manifest.clone(),
        entries,
    };
    let header_json = serde_json::to_vec(&header)
        .map_err(|_| PackageInstallError::MalformedArchive("failed to encode archive header"))?;
    let header_len = u32::try_from(header_json.len())
        .map_err(|_| PackageInstallError::LimitExceeded("archive header exceeds u32"))?;

    let mut out = Vec::with_capacity(
        HEXDEVICE_ARCHIVE_MAGIC.len()
            + 4
            + header_json.len()
            + files.iter().map(|f| f.4.len()).sum::<usize>(),
    );
    out.extend_from_slice(HEXDEVICE_ARCHIVE_MAGIC);
    out.extend_from_slice(&header_len.to_le_bytes());
    out.extend_from_slice(&header_json);
    for &(_, _, _, _, bytes) in files {
        out.extend_from_slice(bytes);
    }
    Ok(out)
}

/// Compute lowercase hex SHA-256 digest for arbitrary bytes.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    sha256_bytes_to_hex(&digest)
}

fn sha256_bytes_to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

fn decode_sha256_hex(hex: &str) -> Result<[u8; 32], PackageInstallError> {
    if hex.len() != 64 {
        return Err(PackageInstallError::InvalidManifest(
            "invalid sha256 hex length",
        ));
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16)
            .map_err(|_| PackageInstallError::InvalidManifest("invalid sha256 hex byte"))?;
    }
    Ok(out)
}

/// Normalize a relative archive path into a collision key that detects ASCII
/// case collisions and Unicode combining-mark/compatibility collisions on
/// default APFS volumes.
fn apfs_collision_key(path: &str) -> String {
    let mut key = String::with_capacity(path.len());
    for ch in path.chars() {
        for lower in ch.to_lowercase() {
            // Strip Unicode combining diacritical marks (U+0300..=U+036F) and
            // treat non-ASCII lookalike whitespace/separators conservatively.
            if ('\u{0300}'..='\u{036F}').contains(&lower) {
                continue;
            }
            key.push(lower);
        }
    }
    key
}

fn validate_relative_path(path: &str) -> Result<(), PackageInstallError> {
    if path.is_empty() || path.len() > 240 {
        return Err(PackageInstallError::InvalidPath(path.to_owned()));
    }
    if path.starts_with('/')
        || path.ends_with('/')
        || path.contains('\\')
        || path.contains('\0')
        || path.contains(':')
        || path.chars().any(|c| c.is_control())
    {
        return Err(PackageInstallError::InvalidPath(path.to_owned()));
    }
    for segment in path.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(PackageInstallError::InvalidPath(path.to_owned()));
        }
    }
    Ok(())
}

fn validate_publisher_id(publisher: &str) -> Result<(), PackageInstallError> {
    if publisher.is_empty()
        || publisher.len() > 64
        || !publisher.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'-' | b'_')
        })
    {
        return Err(PackageInstallError::InvalidManifest("invalid publisher_id"));
    }
    Ok(())
}

fn validate_executable_header(target_os: &str, bytes: &[u8]) -> Result<(), PackageInstallError> {
    if bytes.len() < 4 {
        return Err(PackageInstallError::UnsupportedExecutable(
            "native slice shorter than executable header",
        ));
    }
    let magic = [bytes[0], bytes[1], bytes[2], bytes[3]];
    match target_os {
        "macos" => {
            // Mach-O 64-bit little-endian (CF FA ED FE) or universal/fat Mach-O.
            let valid = magic == [0xCF, 0xFA, 0xED, 0xFE]
                || magic == [0xCA, 0xFE, 0xBA, 0xBE]
                || magic == [0xBE, 0xBA, 0xFE, 0xCA];
            if !valid {
                return Err(PackageInstallError::UnsupportedExecutable(
                    "expected Mach-O 64-bit or universal binary for macos target",
                ));
            }
            Ok(())
        }
        "linux" => {
            if magic != [0x7F, b'E', b'L', b'F'] {
                return Err(PackageInstallError::UnsupportedExecutable(
                    "expected ELF binary for linux target",
                ));
            }
            Ok(())
        }
        "windows" => {
            if magic[0] != b'M' || magic[1] != b'Z' {
                return Err(PackageInstallError::UnsupportedExecutable(
                    "expected PE/MZ binary for windows target",
                ));
            }
            Ok(())
        }
        _ => Err(PackageInstallError::UnsupportedPlatform(
            "unsupported target_os",
        )),
    }
}

fn validate_provenance(
    manifest: &PackageManifest,
    policy: &AdmissionPolicy,
) -> Result<(), PackageInstallError> {
    match &manifest.provenance {
        PackageProvenance::ReviewedDevelopmentTest {
            authorization_sha256,
            reviewer_note,
        } => {
            if reviewer_note.trim().is_empty() || authorization_sha256.len() != 64 {
                return Err(PackageInstallError::UntrustedProvenance(
                    "incomplete reviewed development/test provenance",
                ));
            }
            if !policy
                .allowed_reviewed_test_authorizations
                .contains(&(manifest.publisher_id.clone(), authorization_sha256.clone()))
            {
                return Err(PackageInstallError::UntrustedProvenance(
                    "development/test package not explicitly authorized by admission policy",
                ));
            }
            Ok(())
        }
        PackageProvenance::SignedDistribution {
            publisher_key_id,
            signature_hex,
        } => {
            if publisher_key_id.is_empty()
                || signature_hex.len() < 64
                || !signature_hex.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err(PackageInstallError::UntrustedProvenance(
                    "malformed distribution signature metadata",
                ));
            }
            if !policy
                .allowed_distribution_keys
                .contains(&(manifest.publisher_id.clone(), publisher_key_id.clone()))
            {
                return Err(PackageInstallError::UntrustedProvenance(
                    "publisher signing key not admitted",
                ));
            }
            Ok(())
        }
    }
}

struct ParsedPackage<'a> {
    manifest: PackageManifest,
    package_sha256: String,
    files: Vec<(String, &'a [u8])>,
}

fn parse_and_validate_archive<'a>(
    archive_bytes: &'a [u8],
    policy: &AdmissionPolicy,
) -> Result<ParsedPackage<'a>, PackageInstallError> {
    if archive_bytes.len() > MAX_ARCHIVE_BYTES {
        return Err(PackageInstallError::LimitExceeded(
            "archive exceeds MAX_ARCHIVE_BYTES",
        ));
    }
    if archive_bytes.len() < HEXDEVICE_ARCHIVE_MAGIC.len() + 4 {
        return Err(PackageInstallError::MalformedArchive("archive truncated"));
    }
    if &archive_bytes[..8] != HEXDEVICE_ARCHIVE_MAGIC {
        return Err(PackageInstallError::MalformedArchive(
            "invalid archive magic",
        ));
    }
    let header_len = u32::from_le_bytes([
        archive_bytes[8],
        archive_bytes[9],
        archive_bytes[10],
        archive_bytes[11],
    ]) as usize;
    let payload_start =
        12usize
            .checked_add(header_len)
            .ok_or(PackageInstallError::MalformedArchive(
                "header length overflow",
            ))?;
    if payload_start > archive_bytes.len() {
        return Err(PackageInstallError::MalformedArchive(
            "header exceeds archive",
        ));
    }
    let header: HexDeviceArchiveHeader = serde_json::from_slice(&archive_bytes[12..payload_start])
        .map_err(|_| PackageInstallError::InvalidManifest("manifest or header JSON rejected"))?;

    let manifest = &header.manifest;
    if manifest.schema_version != 1 {
        return Err(PackageInstallError::InvalidManifest(
            "unsupported manifest schema",
        ));
    }
    validate_publisher_id(&manifest.publisher_id)?;
    DeviceTypeId::new(&manifest.type_id)
        .map_err(|_| PackageInstallError::InvalidManifest("invalid device type_id"))?;
    if manifest.display_name.trim().is_empty() || manifest.display_name.len() > 64 {
        return Err(PackageInstallError::InvalidManifest("invalid display_name"));
    }
    if manifest.abi_version != PACKAGE_ABI_VERSION_V1 {
        return Err(PackageInstallError::InvalidManifest(
            "unsupported ABI version",
        ));
    }
    if manifest.state_version == 0 || manifest.ui_version == 0 {
        return Err(PackageInstallError::InvalidManifest(
            "state_version and ui_version must be >= 1",
        ));
    }
    if (manifest.capabilities & !CAP_KNOWN_MASK_V1) != 0 {
        return Err(PackageInstallError::InvalidManifest(
            "unknown capability bit",
        ));
    }
    match manifest.device_kind {
        DEVICE_KIND_AUDIO_EFFECT if (manifest.capabilities & CAP_AUDIO_EFFECT) != 0 => {}
        DEVICE_KIND_INSTRUMENT if (manifest.capabilities & CAP_INSTRUMENT) != 0 => {}
        _ => {
            return Err(PackageInstallError::InvalidManifest(
                "device_kind/capability mismatch",
            ));
        }
    }
    if manifest.target_os != policy.current_os || manifest.target_arch != policy.current_arch {
        return Err(PackageInstallError::UnsupportedPlatform(
            "package target os/arch does not match host",
        ));
    }
    if manifest.min_os_version.trim().is_empty() || manifest.min_os_version.len() > 32 {
        return Err(PackageInstallError::InvalidManifest(
            "invalid min_os_version",
        ));
    }
    if manifest.native_dependencies.len() > MAX_DECLARED_DEPENDENCIES {
        return Err(PackageInstallError::LimitExceeded(
            "too many declared native dependencies",
        ));
    }
    validate_provenance(manifest, policy)?;

    if header.entries.is_empty() || header.entries.len() > MAX_ARCHIVE_FILES {
        return Err(PackageInstallError::LimitExceeded(
            "archive file count out of bounds",
        ));
    }

    let mut cursor = payload_start;
    let mut total_expanded = 0usize;
    let mut seen_exact = HashSet::new();
    let mut seen_folded = HashSet::new();
    let mut files = Vec::with_capacity(header.entries.len());

    for entry in &header.entries {
        validate_relative_path(&entry.path)?;
        if entry.kind != ArchiveEntryKind::RegularFile {
            return Err(PackageInstallError::ForbiddenEntryKind(entry.path.clone()));
        }
        if entry.compression != ArchiveCompression::Stored {
            return Err(PackageInstallError::MalformedArchive(
                "unsupported entry compression",
            ));
        }
        if entry.expanded_bytes > MAX_SINGLE_FILE_BYTES
            || entry.compressed_bytes > MAX_SINGLE_FILE_BYTES
        {
            return Err(PackageInstallError::LimitExceeded(
                "entry exceeds MAX_SINGLE_FILE_BYTES",
            ));
        }
        if entry.compressed_bytes > 0
            && entry.expanded_bytes > entry.compressed_bytes.saturating_mul(MAX_EXPANSION_RATIO)
        {
            return Err(PackageInstallError::DecompressionBomb(
                "entry expansion ratio exceeds MAX_EXPANSION_RATIO",
            ));
        }
        if entry.expanded_bytes != entry.compressed_bytes {
            return Err(PackageInstallError::DecompressionBomb(
                "stored entry expanded_bytes differs from compressed_bytes",
            ));
        }
        total_expanded = total_expanded.checked_add(entry.expanded_bytes).ok_or(
            PackageInstallError::DecompressionBomb("expanded size overflow"),
        )?;
        if total_expanded > MAX_EXPANDED_BYTES {
            return Err(PackageInstallError::DecompressionBomb(
                "total expanded bytes exceed MAX_EXPANDED_BYTES",
            ));
        }
        if !seen_exact.insert(entry.path.clone()) {
            return Err(PackageInstallError::PathCollision(entry.path.clone()));
        }
        let folded = apfs_collision_key(&entry.path);
        if !seen_folded.insert(folded) {
            return Err(PackageInstallError::PathCollision(entry.path.clone()));
        }
        let end = cursor.checked_add(entry.compressed_bytes).ok_or(
            PackageInstallError::MalformedArchive("entry offset overflow"),
        )?;
        if end > archive_bytes.len() {
            return Err(PackageInstallError::MalformedArchive(
                "entry payload truncated",
            ));
        }
        let slice = &archive_bytes[cursor..end];
        cursor = end;

        let Some(expected_digest) = manifest.content_digests.get(&entry.path) else {
            return Err(PackageInstallError::DigestMismatch(entry.path.clone()));
        };
        let actual_digest = sha256_hex(slice);
        if &actual_digest != expected_digest {
            return Err(PackageInstallError::DigestMismatch(entry.path.clone()));
        }
        files.push((entry.path.clone(), slice));
    }

    if cursor != archive_bytes.len() {
        return Err(PackageInstallError::MalformedArchive(
            "trailing bytes after archive payload",
        ));
    }
    if manifest.content_digests.len() != files.len() {
        return Err(PackageInstallError::InvalidManifest(
            "manifest content_digests does not match archive entries exactly",
        ));
    }

    // Verify native module slice exists and has a valid executable header for target OS.
    validate_relative_path(&manifest.native_module_path)?;
    let (_, native_bytes) = files
        .iter()
        .find(|(p, _)| p == &manifest.native_module_path)
        .ok_or(PackageInstallError::InvalidManifest(
            "missing declared native_module_path",
        ))?;
    validate_executable_header(&manifest.target_os, native_bytes)?;

    // Verify every declared dependency, UI schema, preset, and asset exists in the archive.
    if let Some(ui) = &manifest.ui_schema_path {
        validate_relative_path(ui)?;
        if !seen_exact.contains(ui) {
            return Err(PackageInstallError::InvalidManifest(
                "missing declared ui_schema_path",
            ));
        }
    }
    for preset in &manifest.presets {
        validate_relative_path(preset)?;
        if !seen_exact.contains(preset) {
            return Err(PackageInstallError::InvalidManifest(
                "missing declared preset",
            ));
        }
    }
    for asset in &manifest.assets {
        validate_relative_path(asset)?;
        if !seen_exact.contains(asset) {
            return Err(PackageInstallError::InvalidManifest(
                "missing declared asset",
            ));
        }
    }
    for dep in &manifest.native_dependencies {
        validate_relative_path(&dep.bundled_path)?;
        let (_, dep_bytes) = files.iter().find(|(p, _)| p == &dep.bundled_path).ok_or(
            PackageInstallError::InvalidManifest("missing declared native dependency"),
        )?;
        if sha256_hex(dep_bytes) != dep.sha256 {
            return Err(PackageInstallError::DigestMismatch(
                dep.bundled_path.clone(),
            ));
        }
    }

    Ok(ParsedPackage {
        manifest: manifest.clone(),
        package_sha256: sha256_hex(archive_bytes),
        files,
    })
}

#[cfg(target_os = "macos")]
fn read_quarantine_xattr(path: &Path) -> Option<Vec<u8>> {
    const QUARANTINE: &str = "com.apple.quarantine";
    let mut buf = vec![0u8; 4096];
    match rustix::fs::getxattr(path, QUARANTINE, &mut buf) {
        Ok(len) if len > 0 => {
            buf.truncate(len);
            Some(buf)
        }
        _ => None,
    }
}

#[cfg(not(target_os = "macos"))]
fn read_quarantine_xattr(_path: &Path) -> Option<Vec<u8>> {
    None
}

#[cfg(target_os = "macos")]
fn write_quarantine_xattr(path: &Path, value: &[u8]) -> io::Result<()> {
    const QUARANTINE: &str = "com.apple.quarantine";
    rustix::fs::setxattr(path, QUARANTINE, value, rustix::fs::XattrFlags::empty())
        .map_err(|e| io::Error::from_raw_os_error(e.raw_os_error()))
}

#[cfg(not(target_os = "macos"))]
fn write_quarantine_xattr(_path: &Path, _value: &[u8]) -> io::Result<()> {
    Ok(())
}

/// Control-side immutable device package installer and catalog.
pub struct DevicePackageCatalog {
    root: PathBuf,
}

impl DevicePackageCatalog {
    /// Open the package library directory, acquire the installer lock, and
    /// recover from any interrupted staging directories without harming
    /// committed generations or unrelated user files.
    pub fn open_and_recover(root: impl AsRef<Path>) -> Result<Self, PackageInstallError> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(root.join("staging"))?;
        fs::create_dir_all(root.join("generations"))?;
        let catalog = Self { root };
        let _lock = catalog.lock_exclusive()?;
        catalog.clean_abandoned_stages()?;
        let _ = catalog.read_catalog_unlocked()?;
        Ok(catalog)
    }

    /// Return the library root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// List all committed installed package generations from disk, including
    /// pending-removal generations still pinned by live owners.
    pub fn list(&self) -> Result<Vec<InstalledPackageRecord>, PackageInstallError> {
        let _lock = self.lock_exclusive()?;
        let file = self.read_catalog_unlocked()?;
        Ok(file.packages)
    }

    /// List generations eligible for browser discovery and new insertions
    /// (excludes `removal_pending` and `quarantined` generations) (`#2996`).
    pub fn list_discoverable(&self) -> Result<Vec<InstalledPackageRecord>, PackageInstallError> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|rec| !rec.removal_pending && !rec.quarantined)
            .collect())
    }

    /// Select the active default generation for new device insertions of
    /// `type_id` (`#2996`). Honors explicit user-pinned defaults (including
    /// older/custom generations) and otherwise picks the highest discoverable
    /// version without ever downgrading silently.
    pub fn default_generation_for(
        &self,
        type_id: &str,
    ) -> Result<Option<InstalledPackageRecord>, PackageInstallError> {
        let _lock = self.lock_exclusive()?;
        let catalog = self.read_catalog_unlocked()?;
        if let Some(pinned_sha) = catalog.selected_defaults.get(type_id)
            && let Some(rec) = catalog.packages.iter().find(|r| {
                r.type_id == type_id
                    && &r.package_sha256 == pinned_sha
                    && !r.removal_pending
                    && !r.quarantined
            })
        {
            return Ok(Some(rec.clone()));
        }
        Ok(catalog
            .packages
            .into_iter()
            .filter(|r| r.type_id == type_id && !r.removal_pending && !r.quarantined)
            .max_by_key(|r| (r.package_major, r.package_minor, r.package_patch)))
    }

    /// Explicitly pin the default generation for new insertions of `type_id`.
    /// Refuses unrequested downgrades unless `allow_downgrade` is `true`, and
    /// refuses quarantined or pending-removal generations (`#2996`).
    pub fn select_default_generation(
        &self,
        type_id: &str,
        version: (u16, u16, u16),
        allow_downgrade: bool,
    ) -> Result<InstalledPackageRecord, PackageInstallError> {
        let _lock = self.lock_exclusive()?;
        let mut catalog = self.read_catalog_unlocked()?;
        let current_ver = catalog
            .selected_defaults
            .get(type_id)
            .and_then(|sha| catalog.packages.iter().find(|r| &r.package_sha256 == sha))
            .map(|r| (r.package_major, r.package_minor, r.package_patch));
        let Some(target) = catalog
            .packages
            .iter()
            .find(|r| {
                r.type_id == type_id
                    && (r.package_major, r.package_minor, r.package_patch) == version
            })
            .cloned()
        else {
            return Err(PackageInstallError::InvalidManifest(
                "selected default generation is not installed",
            ));
        };
        if target.quarantined || target.removal_pending {
            return Err(PackageInstallError::UntrustedProvenance(
                "cannot select quarantined or pending-removal generation as default",
            ));
        }
        if let Some(cur) = current_ver
            && version < cur
            && !allow_downgrade
        {
            return Err(PackageInstallError::InvalidManifest(
                "unrequested default downgrade refused",
            ));
        }
        catalog
            .selected_defaults
            .insert(type_id.to_owned(), target.package_sha256.clone());
        self.write_catalog_atomic_unlocked(&catalog)?;
        Ok(target)
    }

    /// Hide a generation from future discovery and mark it pending removal
    /// without deleting its immutable directory while live instances, tails,
    /// Undo history, or active exports still hold references (`#2996`).
    pub fn request_uninstall(
        &self,
        type_id: &str,
        version: (u16, u16, u16),
    ) -> Result<bool, PackageInstallError> {
        let _lock = self.lock_exclusive()?;
        let mut catalog = self.read_catalog_unlocked()?;
        let Some(entry) = catalog.packages.iter_mut().find(|r| {
            r.type_id == type_id && (r.package_major, r.package_minor, r.package_patch) == version
        }) else {
            return Ok(false);
        };
        entry.removal_pending = true;
        let sha = entry.package_sha256.clone();
        if catalog.selected_defaults.get(type_id) == Some(&sha) {
            catalog.selected_defaults.remove(type_id);
        }
        self.write_catalog_atomic_unlocked(&catalog)?;
        Ok(true)
    }

    /// Reclaim pending-removal generations on control ONLY once every active
    /// host owner (playback, prepared candidates, tails, Undo history, active
    /// exports) reports quiescence (`is_generation_in_use(rec) == false`) (`#2996`).
    pub fn reclaim_pending_removals(
        &self,
        is_generation_in_use: impl Fn(&InstalledPackageRecord) -> bool,
    ) -> Result<usize, PackageInstallError> {
        let _lock = self.lock_exclusive()?;
        let mut catalog = self.read_catalog_unlocked()?;
        let mut to_delete = Vec::new();
        let mut kept = Vec::with_capacity(catalog.packages.len());
        for rec in catalog.packages {
            if rec.removal_pending && !is_generation_in_use(&rec) {
                to_delete.push(rec);
            } else {
                kept.push(rec);
            }
        }
        if to_delete.is_empty() {
            return Ok(0);
        }
        catalog.packages = kept;
        self.write_catalog_atomic_unlocked(&catalog)?;
        for rec in &to_delete {
            let dir = self.root.join(&rec.generation_rel_dir);
            if dir.is_dir() {
                let _ = fs::remove_dir_all(&dir);
            }
        }
        Ok(to_delete.len())
    }

    /// Verify every on-disk file in an installed generation against its
    /// declared manifest SHA-256 digests (`#2996`). If tampering or corruption
    /// is detected, atomically quarantines the generation so persisted project
    /// states enter state-preserving recovery instead of loading tampered code.
    pub fn verify_generation_integrity(
        &self,
        type_id: &str,
        version: (u16, u16, u16),
    ) -> Result<(), PackageInstallError> {
        let _lock = self.lock_exclusive()?;
        let mut catalog = self.read_catalog_unlocked()?;
        let Some(idx) = catalog.packages.iter().position(|r| {
            r.type_id == type_id && (r.package_major, r.package_minor, r.package_patch) == version
        }) else {
            return Err(PackageInstallError::InvalidManifest(
                "generation not installed",
            ));
        };
        let gen_dir = self.root.join(&catalog.packages[idx].generation_rel_dir);
        let mut tampered_path: Option<String> = None;
        for (rel, expected_sha) in &catalog.packages[idx].manifest.content_digests {
            let full = gen_dir.join(rel);
            let Ok(bytes) = fs::read(&full) else {
                tampered_path = Some(rel.clone());
                break;
            };
            if &sha256_hex(&bytes) != expected_sha {
                tampered_path = Some(rel.clone());
                break;
            }
        }
        if let Some(bad_path) = tampered_path {
            catalog.packages[idx].quarantined = true;
            self.write_catalog_atomic_unlocked(&catalog)?;
            return Err(PackageInstallError::DigestMismatch(bad_path));
        }
        Ok(())
    }

    /// Look up an exact installed package generation by type ID and version.
    pub fn find_exact(
        &self,
        type_id: &str,
        version: (u16, u16, u16),
    ) -> Result<Option<InstalledPackageRecord>, PackageInstallError> {
        let all = self.list()?;
        Ok(all.into_iter().find(|rec| {
            rec.type_id == type_id
                && (rec.package_major, rec.package_minor, rec.package_patch) == version
        }))
    }

    /// Update quarantine state on an installed generation atomically (e.g., when
    /// tampering or OS quarantine policy blocks execution while preserving state).
    pub fn set_quarantined(
        &self,
        type_id: &str,
        version: (u16, u16, u16),
        quarantined: bool,
    ) -> Result<bool, PackageInstallError> {
        let _lock = self.lock_exclusive()?;
        let mut catalog = self.read_catalog_unlocked()?;
        let Some(entry) = catalog.packages.iter_mut().find(|rec| {
            rec.type_id == type_id
                && (rec.package_major, rec.package_minor, rec.package_patch) == version
        }) else {
            return Ok(false);
        };
        entry.quarantined = quarantined;
        self.write_catalog_atomic_unlocked(&catalog)?;
        Ok(true)
    }

    /// Deterministically resolve a persisted `NativeDeviceState` against the
    /// installed catalog by exact `(publisher, type_id, version, state_version,
    /// abi_version, package_sha256)` without loading or executing module code
    /// and without silent latest-version substitution (`#2993`).
    pub fn resolve_exact_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<PackageResolution, PackageInstallError> {
        let (expected, _raw) = PackageIdentityEnvelope::decode_with_raw_state(state.payload())
            .map_err(|_| {
                PackageInstallError::InvalidManifest("malformed package identity envelope")
            })?;
        let type_id = state.type_id();
        let state_type = type_id
            .as_str()
            .map_err(|_| PackageInstallError::InvalidManifest("invalid state type_id"))?;
        if state_type != expected.type_id || state.state_version() != expected.state_version {
            return Err(PackageInstallError::InvalidManifest(
                "state metadata disagrees with pinned package identity envelope",
            ));
        }

        let all = self.list()?;
        let matching_type: Vec<InstalledPackageRecord> = all
            .into_iter()
            .filter(|rec| rec.type_id == expected.type_id)
            .collect();
        if matching_type.is_empty() {
            return Ok(PackageResolution::Unavailable(
                UnavailablePackageReason::Missing { expected },
            ));
        }
        if let Some(mismatch) = matching_type
            .iter()
            .find(|rec| rec.publisher_id != expected.publisher_id)
        {
            return Ok(PackageResolution::Unavailable(
                UnavailablePackageReason::PublisherMismatch {
                    expected,
                    installed_publisher: mismatch.publisher_id.clone(),
                },
            ));
        }
        let expected_ver = (
            expected.package_major,
            expected.package_minor,
            expected.package_patch,
        );
        let Some(exact_ver) = matching_type
            .iter()
            .find(|rec| (rec.package_major, rec.package_minor, rec.package_patch) == expected_ver)
        else {
            let installed_versions = matching_type
                .iter()
                .map(|r| (r.package_major, r.package_minor, r.package_patch))
                .collect();
            return Ok(PackageResolution::Unavailable(
                UnavailablePackageReason::VersionAbsent {
                    expected,
                    installed_versions,
                },
            ));
        };
        if exact_ver.abi_version != expected.abi_version {
            return Ok(PackageResolution::Unavailable(
                UnavailablePackageReason::IncompatibleAbi {
                    expected,
                    installed_abi: exact_ver.abi_version,
                },
            ));
        }
        let expected_hex = sha256_bytes_to_hex(&expected.package_sha256);
        if exact_ver.package_sha256 != expected_hex {
            return Ok(PackageResolution::Unavailable(
                UnavailablePackageReason::HashConflict {
                    expected,
                    installed_sha256: exact_ver.package_sha256.clone(),
                },
            ));
        }
        if exact_ver.quarantined {
            return Ok(PackageResolution::Unavailable(
                UnavailablePackageReason::Quarantined {
                    expected,
                    record: Box::new(exact_ver.clone()),
                },
            ));
        }
        Ok(PackageResolution::Available(Box::new(exact_ver.clone())))
    }

    /// Explicitly upgrade or rebind a pinned `NativeDeviceState` to a
    /// compatible installed generation (`#2993`). Refuses quarantined targets,
    /// publisher/type/ABI/state-version mismatches, and unrequested downgrades.
    /// Preserves raw device state bytes, asset references, and host metadata.
    pub fn rebind_compatible_state(
        &self,
        state: &NativeDeviceState,
        target: &InstalledPackageRecord,
        allow_downgrade: bool,
    ) -> Result<NativeDeviceState, PackageInstallError> {
        if target.quarantined {
            return Err(PackageInstallError::UntrustedProvenance(
                "cannot rebind to a quarantined package generation",
            ));
        }
        let (expected, raw_inner) = PackageIdentityEnvelope::decode_with_raw_state(state.payload())
            .map_err(|_| {
                PackageInstallError::InvalidManifest("malformed package identity envelope")
            })?;
        if target.type_id != expected.type_id {
            return Err(PackageInstallError::InvalidManifest(
                "cannot rebind to a different device type_id",
            ));
        }
        if target.publisher_id != expected.publisher_id {
            return Err(PackageInstallError::PublisherTakeover {
                type_id: target.type_id.clone(),
                existing_publisher: expected.publisher_id,
                candidate_publisher: target.publisher_id.clone(),
            });
        }
        if target.abi_version != expected.abi_version
            || target.state_version != expected.state_version
        {
            return Err(PackageInstallError::InvalidManifest(
                "target generation ABI or state_version is incompatible for rebind",
            ));
        }
        let from_ver = (
            expected.package_major,
            expected.package_minor,
            expected.package_patch,
        );
        let to_ver = (
            target.package_major,
            target.package_minor,
            target.package_patch,
        );
        if to_ver < from_ver && !allow_downgrade {
            return Err(PackageInstallError::InvalidManifest(
                "unrequested package downgrade refused",
            ));
        }
        let sha = decode_sha256_hex(&target.package_sha256)?;
        let rebound_env = PackageIdentityEnvelope {
            publisher_id: target.publisher_id.clone(),
            type_id: target.type_id.clone(),
            package_major: target.package_major,
            package_minor: target.package_minor,
            package_patch: target.package_patch,
            state_version: target.state_version,
            abi_version: target.abi_version,
            ui_version: target.ui_version,
            package_sha256: sha,
        };
        let payload = rebound_env
            .encode_with_raw_state(raw_inner)
            .map_err(|_| PackageInstallError::InvalidManifest("failed to encode rebound state"))?;
        let rebound = NativeDeviceState::new(state.type_id(), target.state_version, payload)
            .and_then(|s| s.with_host_metadata_from(state))
            .map_err(|_| {
                PackageInstallError::InvalidManifest("failed to construct rebound state")
            })?;
        Ok(rebound)
    }

    /// Atomically validate and install a `.hexdevice` archive file into an
    /// immutable generation directory and commit it to `catalog.json`.
    pub fn install_file(
        &self,
        archive_path: &Path,
        policy: &AdmissionPolicy,
        cancel: Option<&AtomicBool>,
    ) -> Result<InstallOutcome, PackageInstallError> {
        if cancel.is_some_and(|flag| flag.load(Ordering::Acquire)) {
            return Err(PackageInstallError::Cancelled);
        }
        let quarantine = read_quarantine_xattr(archive_path);
        let meta = fs::metadata(archive_path)?;
        if meta.len() as usize > MAX_ARCHIVE_BYTES {
            return Err(PackageInstallError::LimitExceeded(
                "archive file exceeds MAX_ARCHIVE_BYTES",
            ));
        }
        let archive_bytes = fs::read(archive_path)?;
        self.install_bytes_with_quarantine(
            &archive_bytes,
            PackageOrigin::User,
            quarantine.as_deref(),
            policy,
            cancel,
        )
    }

    /// Atomically validate and install raw `.hexdevice` archive bytes as a
    /// user-installed generation.
    pub fn install_bytes(
        &self,
        archive_bytes: &[u8],
        policy: &AdmissionPolicy,
        cancel: Option<&AtomicBool>,
    ) -> Result<InstallOutcome, PackageInstallError> {
        self.install_bytes_with_origin(archive_bytes, PackageOrigin::User, None, policy, cancel)
    }

    /// Atomically validate and install raw `.hexdevice` archive bytes with an
    /// explicit [`PackageOrigin`] (`Factory` vs `User`) (`#2996`).
    pub fn install_bytes_with_origin(
        &self,
        archive_bytes: &[u8],
        origin: PackageOrigin,
        quarantine_xattr: Option<&[u8]>,
        policy: &AdmissionPolicy,
        cancel: Option<&AtomicBool>,
    ) -> Result<InstallOutcome, PackageInstallError> {
        self.install_bytes_with_quarantine(archive_bytes, origin, quarantine_xattr, policy, cancel)
    }

    fn install_bytes_with_quarantine(
        &self,
        archive_bytes: &[u8],
        origin: PackageOrigin,
        quarantine_xattr: Option<&[u8]>,
        policy: &AdmissionPolicy,
        cancel: Option<&AtomicBool>,
    ) -> Result<InstallOutcome, PackageInstallError> {
        if cancel.is_some_and(|flag| flag.load(Ordering::Acquire)) {
            return Err(PackageInstallError::Cancelled);
        }
        let parsed = parse_and_validate_archive(archive_bytes, policy)?;
        let _lock = self.lock_exclusive()?;
        if cancel.is_some_and(|flag| flag.load(Ordering::Acquire)) {
            return Err(PackageInstallError::Cancelled);
        }

        let mut catalog = self.read_catalog_unlocked()?;

        // Check publisher ownership and idempotency / hash conflict rules.
        for existing in &mut catalog.packages {
            if existing.type_id == parsed.manifest.type_id {
                if existing.publisher_id != parsed.manifest.publisher_id {
                    return Err(PackageInstallError::PublisherTakeover {
                        type_id: parsed.manifest.type_id.clone(),
                        existing_publisher: existing.publisher_id.clone(),
                        candidate_publisher: parsed.manifest.publisher_id.clone(),
                    });
                }
                if (
                    existing.package_major,
                    existing.package_minor,
                    existing.package_patch,
                ) == parsed.manifest.version_tuple()
                {
                    if existing.package_sha256 == parsed.package_sha256 {
                        // Reinstalling a previously removal-pending generation with the
                        // exact same verified content restores discovery atomically.
                        if existing.removal_pending {
                            existing.removal_pending = false;
                            let restored = existing.clone();
                            self.write_catalog_atomic_unlocked(&catalog)?;
                            return Ok(InstallOutcome::Installed(restored));
                        }
                        return Ok(InstallOutcome::AlreadyInstalled(existing.clone()));
                    }
                    return Err(PackageInstallError::VersionHashConflict {
                        type_id: parsed.manifest.type_id.clone(),
                        version: parsed.manifest.version_tuple(),
                    });
                }
            }
        }

        let short_hash = &parsed.package_sha256[..16];
        let version_dir = format!(
            "{}.{}.{}-{short_hash}",
            parsed.manifest.package_major,
            parsed.manifest.package_minor,
            parsed.manifest.package_patch
        );
        let gen_rel = format!("generations/{}/{}", parsed.manifest.type_id, version_dir);
        let final_gen_dir = self.root.join(&gen_rel);
        let stage_dir = self
            .root
            .join("staging")
            .join(format!("stage-{}-{short_hash}", std::process::id()));
        if stage_dir.exists() {
            let _ = fs::remove_dir_all(&stage_dir);
        }
        fs::create_dir_all(&stage_dir)?;

        // Write and sync every validated payload file inside staging/.
        let write_result = (|| -> Result<(), PackageInstallError> {
            for (rel_path, bytes) in &parsed.files {
                if cancel.is_some_and(|flag| flag.load(Ordering::Acquire)) {
                    return Err(PackageInstallError::Cancelled);
                }
                let dest = stage_dir.join(rel_path);
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent)?;
                }
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&dest)?;
                file.write_all(bytes)?;
                file.sync_all()?;
                if let Some(q) = quarantine_xattr {
                    write_quarantine_xattr(&dest, q)?;
                }
            }
            Ok(())
        })();

        if let Err(err) = write_result {
            let _ = fs::remove_dir_all(&stage_dir);
            return Err(err);
        }

        if cancel.is_some_and(|flag| flag.load(Ordering::Acquire)) {
            let _ = fs::remove_dir_all(&stage_dir);
            return Err(PackageInstallError::Cancelled);
        }

        if let Some(parent) = final_gen_dir.parent() {
            fs::create_dir_all(parent)?;
        }
        if final_gen_dir.exists() {
            let _ = fs::remove_dir_all(&stage_dir);
            return Err(PackageInstallError::VersionHashConflict {
                type_id: parsed.manifest.type_id.clone(),
                version: parsed.manifest.version_tuple(),
            });
        }

        // Atomically move immutable staged generation into generations/.
        if let Err(err) = fs::rename(&stage_dir, &final_gen_dir) {
            let _ = fs::remove_dir_all(&stage_dir);
            return Err(PackageInstallError::Io(err));
        }

        let record = InstalledPackageRecord {
            type_id: parsed.manifest.type_id.clone(),
            publisher_id: parsed.manifest.publisher_id.clone(),
            package_major: parsed.manifest.package_major,
            package_minor: parsed.manifest.package_minor,
            package_patch: parsed.manifest.package_patch,
            state_version: parsed.manifest.state_version,
            abi_version: parsed.manifest.abi_version,
            ui_version: parsed.manifest.ui_version,
            package_sha256: parsed.package_sha256,
            generation_rel_dir: gen_rel,
            native_module_rel_path: parsed.manifest.native_module_path.clone(),
            quarantined: quarantine_xattr.is_some(),
            origin,
            removal_pending: false,
            manifest: parsed.manifest,
        };
        catalog.packages.push(record.clone());
        if let Err(err) = self.write_catalog_atomic_unlocked(&catalog) {
            // Roll back unreferenced generation dir if catalog commit failed.
            let _ = fs::remove_dir_all(&final_gen_dir);
            return Err(err);
        }

        Ok(InstallOutcome::Installed(record))
    }

    fn lock_exclusive(&self) -> Result<File, PackageInstallError> {
        let lock_path = self.root.join(".installer.lock");
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(lock_path)?;
        file.lock_exclusive()?;
        Ok(file)
    }

    fn clean_abandoned_stages(&self) -> Result<(), PackageInstallError> {
        let staging = self.root.join("staging");
        if !staging.exists() {
            return Ok(());
        }
        for entry in fs::read_dir(&staging)? {
            let entry = entry?;
            let path = entry.path();
            if entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("stage-"))
                && path.is_dir()
            {
                let _ = fs::remove_dir_all(&path);
            }
        }
        Ok(())
    }

    fn catalog_path(&self) -> PathBuf {
        self.root.join("catalog.json")
    }

    fn read_catalog_unlocked(&self) -> Result<CatalogFile, PackageInstallError> {
        let path = self.catalog_path();
        if !path.exists() {
            return Ok(CatalogFile {
                catalog_version: 1,
                packages: Vec::new(),
                selected_defaults: BTreeMap::new(),
            });
        }
        let bytes = fs::read(&path)?;
        let mut parsed: CatalogFile = serde_json::from_slice(&bytes)
            .map_err(|_| PackageInstallError::MalformedArchive("corrupt catalog.json"))?;
        if parsed.catalog_version != 1 {
            return Err(PackageInstallError::InvalidManifest(
                "unsupported catalog version",
            ));
        }
        // Only retain entries whose immutable generation directory still exists.
        parsed
            .packages
            .retain(|rec| self.root.join(&rec.generation_rel_dir).is_dir());
        Ok(parsed)
    }

    fn write_catalog_atomic_unlocked(
        &self,
        catalog: &CatalogFile,
    ) -> Result<(), PackageInstallError> {
        let bytes = serde_json::to_vec_pretty(catalog)
            .map_err(|_| PackageInstallError::MalformedArchive("catalog serialization failed"))?;
        let tmp_path = self
            .root
            .join(format!(".catalog-{}.tmp", std::process::id()));
        {
            let mut file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&tmp_path)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        fs::rename(&tmp_path, self.catalog_path())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice_contract::package_abi::{CAP_PARAMETER_EVENTS, CAP_ZERO_LATENCY};
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    struct TempLib(PathBuf);
    impl TempLib {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!(
                "hex-devpkg-{tag}-{}-{}",
                std::process::id(),
                SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for TempLib {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn dummy_native_slice(salt: u8) -> Vec<u8> {
        let mut bytes = match std::env::consts::OS {
            "macos" => vec![0xCF, 0xFA, 0xED, 0xFE],
            "linux" => vec![0x7F, b'E', b'L', b'F'],
            "windows" => vec![b'M', b'Z', 0x90, 0x00],
            _ => vec![0xCF, 0xFA, 0xED, 0xFE],
        };
        bytes.extend_from_slice(&[salt; 60]);
        bytes
    }

    fn test_policy() -> (AdmissionPolicy, String) {
        let auth = sha256_hex(b"reviewed-test-fixture-token-v1");
        let mut policy = AdmissionPolicy::host_default();
        policy.allow_reviewed_test("hex.factory", auth.clone());
        (policy, auth)
    }

    fn make_valid_archive(
        publisher: &str,
        type_id: &str,
        version: (u16, u16, u16),
        salt: u8,
        auth_sha256: &str,
    ) -> Vec<u8> {
        let native = dummy_native_slice(salt);
        let ui = br#"{"controls":["gain","balance"]}"#;
        let mut digests = BTreeMap::new();
        digests.insert("bin/device.dylib".to_owned(), sha256_hex(&native));
        digests.insert("ui/panel.json".to_owned(), sha256_hex(ui));
        let manifest = PackageManifest {
            schema_version: 1,
            publisher_id: publisher.to_owned(),
            type_id: type_id.to_owned(),
            display_name: "utility".to_owned(),
            package_major: version.0,
            package_minor: version.1,
            package_patch: version.2,
            state_version: 1,
            abi_version: PACKAGE_ABI_VERSION_V1,
            ui_version: 1,
            device_kind: DEVICE_KIND_AUDIO_EFFECT,
            capabilities: CAP_AUDIO_EFFECT | CAP_PARAMETER_EVENTS | CAP_ZERO_LATENCY,
            target_os: std::env::consts::OS.to_owned(),
            target_arch: std::env::consts::ARCH.to_owned(),
            min_os_version: "14.0".to_owned(),
            native_module_path: "bin/device.dylib".to_owned(),
            ui_schema_path: Some("ui/panel.json".to_owned()),
            presets: Vec::new(),
            assets: Vec::new(),
            native_dependencies: Vec::new(),
            content_digests: digests,
            provenance: PackageProvenance::ReviewedDevelopmentTest {
                authorization_sha256: auth_sha256.to_owned(),
                reviewer_note: "offline test fixture".to_owned(),
            },
        };
        encode_hexdevice_archive(
            &manifest,
            &[
                (
                    "bin/device.dylib",
                    ArchiveEntryKind::RegularFile,
                    ArchiveCompression::Stored,
                    native.len(),
                    &native,
                ),
                (
                    "ui/panel.json",
                    ArchiveEntryKind::RegularFile,
                    ArchiveCompression::Stored,
                    ui.len(),
                    ui,
                ),
            ],
        )
        .unwrap()
    }

    #[test]
    fn install_restart_idempotency_conflict_and_publisher_takeover() {
        let tmp = TempLib::new("lifecycle");
        let (mut policy, auth) = test_policy();
        let other_auth = sha256_hex(b"other-publisher-token");
        policy.allow_reviewed_test("evil.publisher", other_auth.clone());

        let catalog = DevicePackageCatalog::open_and_recover(&tmp.0).unwrap();
        let pkg = make_valid_archive("hex.factory", "hex.pkg.utility", (1, 0, 0), 7, &auth);

        // First install succeeds.
        let installed = match catalog.install_bytes(&pkg, &policy, None).unwrap() {
            InstallOutcome::Installed(rec) => rec,
            other => panic!("expected Installed, got {other:?}"),
        };
        assert_eq!(installed.type_id, "hex.pkg.utility");
        assert!(
            tmp.0
                .join(&installed.generation_rel_dir)
                .join("bin/device.dylib")
                .is_file()
        );

        // Restart catalog finds exact package.
        let reopened = DevicePackageCatalog::open_and_recover(&tmp.0).unwrap();
        let found = reopened
            .find_exact("hex.pkg.utility", (1, 0, 0))
            .unwrap()
            .expect("persisted across restart");
        assert_eq!(found.package_sha256, installed.package_sha256);

        // Same hash reinstall is idempotent.
        let second = reopened.install_bytes(&pkg, &policy, None).unwrap();
        assert_eq!(second, InstallOutcome::AlreadyInstalled(found));

        // Same ID & version with different hash refuses without harming prior install.
        let mutated = make_valid_archive("hex.factory", "hex.pkg.utility", (1, 0, 0), 9, &auth);
        assert!(matches!(
            reopened.install_bytes(&mutated, &policy, None),
            Err(PackageInstallError::VersionHashConflict { .. })
        ));

        // Different publisher attempting takeover of same type_id refuses.
        let takeover = make_valid_archive(
            "evil.publisher",
            "hex.pkg.utility",
            (2, 0, 0),
            11,
            &other_auth,
        );
        assert!(matches!(
            reopened.install_bytes(&takeover, &policy, None),
            Err(PackageInstallError::PublisherTakeover { .. })
        ));
    }

    #[test]
    fn rejects_traversal_symlinks_apfs_collisions_bombs_bad_exec_and_untrusted() {
        let tmp = TempLib::new("rejections");
        let (policy, auth) = test_policy();
        let catalog = DevicePackageCatalog::open_and_recover(&tmp.0).unwrap();
        let native = dummy_native_slice(1);

        // Untrusted provenance (strict default policy rejects dev/test fixture).
        let valid_pkg = make_valid_archive("hex.factory", "hex.pkg.utility", (1, 0, 0), 1, &auth);
        assert!(matches!(
            catalog.install_bytes(&valid_pkg, &AdmissionPolicy::host_default(), None),
            Err(PackageInstallError::UntrustedProvenance(_))
        ));

        // Path traversal rejected.
        let mut digests = BTreeMap::new();
        digests.insert("../escape.dylib".to_owned(), sha256_hex(&native));
        let mut base_manifest = PackageManifest {
            schema_version: 1,
            publisher_id: "hex.factory".to_owned(),
            type_id: "hex.pkg.bad".to_owned(),
            display_name: "bad".to_owned(),
            package_major: 1,
            package_minor: 0,
            package_patch: 0,
            state_version: 1,
            abi_version: PACKAGE_ABI_VERSION_V1,
            ui_version: 1,
            device_kind: DEVICE_KIND_AUDIO_EFFECT,
            capabilities: CAP_AUDIO_EFFECT,
            target_os: std::env::consts::OS.to_owned(),
            target_arch: std::env::consts::ARCH.to_owned(),
            min_os_version: "14.0".to_owned(),
            native_module_path: "../escape.dylib".to_owned(),
            ui_schema_path: None,
            presets: Vec::new(),
            assets: Vec::new(),
            native_dependencies: Vec::new(),
            content_digests: digests,
            provenance: PackageProvenance::ReviewedDevelopmentTest {
                authorization_sha256: auth.clone(),
                reviewer_note: "test".to_owned(),
            },
        };
        let traversal = encode_hexdevice_archive(
            &base_manifest,
            &[(
                "../escape.dylib",
                ArchiveEntryKind::RegularFile,
                ArchiveCompression::Stored,
                native.len(),
                &native,
            )],
        )
        .unwrap();
        assert!(matches!(
            catalog.install_bytes(&traversal, &policy, None),
            Err(PackageInstallError::InvalidPath(_))
        ));

        // Symlink entry rejected.
        base_manifest.native_module_path = "bin/device.dylib".to_owned();
        base_manifest.content_digests.clear();
        base_manifest
            .content_digests
            .insert("bin/device.dylib".to_owned(), sha256_hex(&native));
        let symlink_pkg = encode_hexdevice_archive(
            &base_manifest,
            &[(
                "bin/device.dylib",
                ArchiveEntryKind::Symlink,
                ArchiveCompression::Stored,
                native.len(),
                &native,
            )],
        )
        .unwrap();
        assert!(matches!(
            catalog.install_bytes(&symlink_pkg, &policy, None),
            Err(PackageInstallError::ForbiddenEntryKind(_))
        ));

        // APFS case/Unicode collision rejected.
        base_manifest
            .content_digests
            .insert("bin/Device.dylib".to_owned(), sha256_hex(&native));
        let case_collision = encode_hexdevice_archive(
            &base_manifest,
            &[
                (
                    "bin/device.dylib",
                    ArchiveEntryKind::RegularFile,
                    ArchiveCompression::Stored,
                    native.len(),
                    &native,
                ),
                (
                    "bin/Device.dylib",
                    ArchiveEntryKind::RegularFile,
                    ArchiveCompression::Stored,
                    native.len(),
                    &native,
                ),
            ],
        )
        .unwrap();
        assert!(matches!(
            catalog.install_bytes(&case_collision, &policy, None),
            Err(PackageInstallError::PathCollision(_))
        ));

        // Decompression bomb ratio rejected.
        base_manifest.content_digests.clear();
        base_manifest
            .content_digests
            .insert("bin/device.dylib".to_owned(), sha256_hex(&native));
        let bomb = encode_hexdevice_archive(
            &base_manifest,
            &[(
                "bin/device.dylib",
                ArchiveEntryKind::RegularFile,
                ArchiveCompression::Stored,
                native.len() * 100,
                &native,
            )],
        )
        .unwrap();
        assert!(matches!(
            catalog.install_bytes(&bomb, &policy, None),
            Err(PackageInstallError::DecompressionBomb(_))
        ));

        // Unsupported executable payload (shell script instead of native binary) rejected.
        let script = b"#!/bin/sh\necho pwned\n";
        base_manifest.content_digests.clear();
        base_manifest
            .content_digests
            .insert("bin/device.dylib".to_owned(), sha256_hex(script));
        let bad_exec = encode_hexdevice_archive(
            &base_manifest,
            &[(
                "bin/device.dylib",
                ArchiveEntryKind::RegularFile,
                ArchiveCompression::Stored,
                script.len(),
                script,
            )],
        )
        .unwrap();
        assert!(matches!(
            catalog.install_bytes(&bad_exec, &policy, None),
            Err(PackageInstallError::UnsupportedExecutable(_))
        ));
    }

    #[test]
    fn cancellation_startup_recovery_and_concurrent_installs_preserve_integrity() {
        let tmp = TempLib::new("recovery");
        let (policy, auth) = test_policy();
        let catalog = Arc::new(DevicePackageCatalog::open_and_recover(&tmp.0).unwrap());

        // Pre-seed an abandoned stage directory as if a previous crash occurred.
        let abandoned = tmp.0.join("staging").join("stage-99999-deadbeef");
        fs::create_dir_all(&abandoned).unwrap();
        fs::write(abandoned.join("partial.bin"), b"partial").unwrap();
        let recovered = DevicePackageCatalog::open_and_recover(&tmp.0).unwrap();
        assert!(!abandoned.exists());
        assert!(recovered.list().unwrap().is_empty());

        // Cancelled installation exposes nothing.
        let cancel = AtomicBool::new(true);
        let pkg1 = make_valid_archive("hex.factory", "hex.pkg.utility", (1, 0, 0), 3, &auth);
        assert!(matches!(
            catalog.install_bytes(&pkg1, &policy, Some(&cancel)),
            Err(PackageInstallError::Cancelled)
        ));
        assert!(catalog.list().unwrap().is_empty());

        // Concurrent installations of the same package serialize cleanly: one
        // installs and the others observe AlreadyInstalled.
        let pkg_arc = Arc::new(pkg1);
        let policy_arc = Arc::new(policy);
        let mut handles = Vec::new();
        for _ in 0..4 {
            let c = Arc::clone(&catalog);
            let p = Arc::clone(&pkg_arc);
            let pol = Arc::clone(&policy_arc);
            handles.push(std::thread::spawn(move || {
                c.install_bytes(&p, &pol, None).unwrap()
            }));
        }
        let mut installed_count = 0usize;
        let mut already_count = 0usize;
        for h in handles {
            match h.join().unwrap() {
                InstallOutcome::Installed(_) => installed_count += 1,
                InstallOutcome::AlreadyInstalled(_) => already_count += 1,
            }
        }
        assert_eq!(installed_count, 1);
        assert_eq!(already_count, 3);
        assert_eq!(catalog.list().unwrap().len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn read_only_failure_preserves_prior_install_and_exposes_no_partial_entry() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempLib::new("readonly");
        let (policy, auth) = test_policy();
        let catalog = DevicePackageCatalog::open_and_recover(&tmp.0).unwrap();
        let pkg1 = make_valid_archive("hex.factory", "hex.pkg.utility", (1, 0, 0), 1, &auth);
        let pkg2 = make_valid_archive("hex.factory", "hex.pkg.utility", (1, 1, 0), 2, &auth);
        assert!(matches!(
            catalog.install_bytes(&pkg1, &policy, None).unwrap(),
            InstallOutcome::Installed(_)
        ));

        // Make staging read-only to simulate a filesystem write failure mid-install.
        let staging = tmp.0.join("staging");
        fs::set_permissions(&staging, fs::Permissions::from_mode(0o555)).unwrap();
        let res = catalog.install_bytes(&pkg2, &policy, None);
        fs::set_permissions(&staging, fs::Permissions::from_mode(0o755)).unwrap();

        assert!(matches!(res, Err(PackageInstallError::Io(_))));
        let listed = catalog.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].package_minor, 0);
    }

    #[test]
    fn exact_version_pinning_save_reopen_undo_quarantine_reinstall_and_rebind() {
        use hex_project::{Command, Editor, Project};

        let tmp = TempLib::new("pinning");
        let (policy, auth) = test_policy();
        let lib_a = tmp.0.join("lib_a");
        let lib_b = tmp.0.join("lib_b");
        let cat_a = DevicePackageCatalog::open_and_recover(&lib_a).unwrap();
        let cat_b = DevicePackageCatalog::open_and_recover(&lib_b).unwrap();

        let pkg_v1 = make_valid_archive("hex.factory", "hex.pkg.utility", (1, 0, 0), 10, &auth);
        let pkg_v1_1 = make_valid_archive("hex.factory", "hex.pkg.utility", (1, 1, 0), 20, &auth);

        let rec_v1 = match cat_a.install_bytes(&pkg_v1, &policy, None).unwrap() {
            InstallOutcome::Installed(r) => r,
            other => panic!("expected Installed, got {other:?}"),
        };
        let rec_v1_1 = match cat_a.install_bytes(&pkg_v1_1, &policy, None).unwrap() {
            InstallOutcome::Installed(r) => r,
            other => panic!("expected Installed, got {other:?}"),
        };

        // Construct pinned NativeDeviceState for v1.0.0 with custom opaque payload bytes.
        let env_v1 = PackageIdentityEnvelope {
            publisher_id: rec_v1.publisher_id.clone(),
            type_id: rec_v1.type_id.clone(),
            package_major: 1,
            package_minor: 0,
            package_patch: 0,
            state_version: 1,
            abi_version: PACKAGE_ABI_VERSION_V1,
            ui_version: 1,
            package_sha256: decode_sha256_hex(&rec_v1.package_sha256).unwrap(),
        };
        let raw_payload = b"opaque-authored-device-state-bytes";
        let state_v1 = NativeDeviceState::new(
            DeviceTypeId::new("hex.pkg.utility").unwrap(),
            1,
            env_v1.encode_with_raw_state(raw_payload).unwrap(),
        )
        .unwrap();

        // Insert TWO copied instances into a real Project -> distinct DeviceIds, identical pinned envelope.
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        editor
            .execute(Command::InsertMainDevice {
                before: None,
                state: state_v1.clone(),
            })
            .unwrap();
        editor
            .execute(Command::InsertMainDevice {
                before: None,
                state: state_v1.clone(),
            })
            .unwrap();
        let dev_a = editor.project().main_devices()[0].id();
        let dev_b = editor.project().main_devices()[1].id();
        assert_ne!(dev_a, dev_b);

        // Save and reopen via canonical hex-storage without evaluating any untrusted module codec.
        let proj_path = tmp.0.join("pinned.hex");
        crate::save_new(&proj_path, editor.project()).unwrap();
        let reopened = crate::load(&proj_path).unwrap();
        let reopened_state = reopened.device(dev_a).unwrap().state().clone();
        assert_eq!(reopened_state, state_v1);

        // 1. Exact resolution succeeds when v1.0.0 is installed.
        assert_eq!(
            cat_a.resolve_exact_state(&reopened_state).unwrap(),
            PackageResolution::Available(Box::new(rec_v1.clone()))
        );

        // 2. Missing package in empty catalog preserves opaque state and reports Missing.
        assert!(matches!(
            cat_b.resolve_exact_state(&reopened_state).unwrap(),
            PackageResolution::Unavailable(UnavailablePackageReason::Missing { .. })
        ));

        // 3. Installing ONLY v1.1.0 in lib_b must NOT silently substitute for pinned v1.0.0!
        let _ = cat_b.install_bytes(&pkg_v1_1, &policy, None).unwrap();
        assert!(matches!(
            cat_b.resolve_exact_state(&reopened_state).unwrap(),
            PackageResolution::Unavailable(UnavailablePackageReason::VersionAbsent {
                ref installed_versions,
                ..
            }) if installed_versions == &vec![(1, 1, 0)]
        ));

        // 4. Reinstalling the exact original v1.0.0 package restores Available without mutating project!
        let _ = cat_b.install_bytes(&pkg_v1, &policy, None).unwrap();
        assert_eq!(
            cat_b.resolve_exact_state(&reopened_state).unwrap(),
            PackageResolution::Available(Box::new(rec_v1.clone()))
        );

        // 5. Quarantining v1.0.0 reports Quarantined while keeping opaque bytes intact.
        assert!(
            cat_a
                .set_quarantined("hex.pkg.utility", (1, 0, 0), true)
                .unwrap()
        );
        assert!(matches!(
            cat_a.resolve_exact_state(&reopened_state).unwrap(),
            PackageResolution::Unavailable(UnavailablePackageReason::Quarantined { .. })
        ));
        assert!(
            cat_a
                .set_quarantined("hex.pkg.utility", (1, 0, 0), false)
                .unwrap()
        );

        // 6. Explicit compatible upgrade rebind to v1.1.0 + Undo/Redo preserves opaque payload!
        let upgraded = cat_a
            .rebind_compatible_state(&reopened_state, &rec_v1_1, false)
            .unwrap();
        let (upgraded_env, upgraded_raw) =
            PackageIdentityEnvelope::decode_with_raw_state(upgraded.payload()).unwrap();
        assert_eq!(upgraded_env.package_minor, 1);
        assert_eq!(upgraded_raw, raw_payload);
        assert_eq!(
            cat_a.resolve_exact_state(&upgraded).unwrap(),
            PackageResolution::Available(Box::new(rec_v1_1.clone()))
        );

        // Unrequested downgrade back to v1.0.0 is refused unless allow_downgrade=true.
        assert!(matches!(
            cat_a.rebind_compatible_state(&upgraded, &rec_v1, false),
            Err(PackageInstallError::InvalidManifest(_))
        ));

        // Apply upgrade in Editor, then Undo restores exact v1.0.0 pin and Redo restores v1.1.0.
        editor
            .execute(Command::ReplaceDeviceState {
                device: dev_a,
                state: upgraded.clone(),
            })
            .unwrap();
        assert_eq!(editor.project().device(dev_a).unwrap().state(), &upgraded);
        assert!(editor.undo());
        assert_eq!(editor.project().device(dev_a).unwrap().state(), &state_v1);
        assert!(editor.redo());
        assert_eq!(editor.project().device(dev_a).unwrap().state(), &upgraded);
        // Second copied instance remained pinned to v1.0.0 throughout!
        assert_eq!(editor.project().device(dev_b).unwrap().state(), &state_v1);
    }

    #[test]
    fn immutable_generation_update_uninstall_quiescence_and_tamper_quarantine() {
        let tmp = TempLib::new("update_uninstall");
        let (policy, auth) = test_policy();
        let catalog = DevicePackageCatalog::open_and_recover(&tmp.0).unwrap();

        let pkg_v1 = make_valid_archive("hex.factory", "hex.pkg.utility", (1, 0, 0), 1, &auth);
        let pkg_v1_conflict =
            make_valid_archive("hex.factory", "hex.pkg.utility", (1, 0, 0), 99, &auth);
        let pkg_v2 = make_valid_archive("hex.factory", "hex.pkg.utility", (2, 0, 0), 2, &auth);

        // Install v1 as User origin and pin it as default.
        let rec_v1 = match catalog
            .install_bytes_with_origin(&pkg_v1, PackageOrigin::User, None, &policy, None)
            .unwrap()
        {
            InstallOutcome::Installed(r) => r,
            other => panic!("expected Installed, got {other:?}"),
        };
        assert_eq!(rec_v1.origin, PackageOrigin::User);
        catalog
            .select_default_generation("hex.pkg.utility", (1, 0, 0), false)
            .unwrap();

        // Factory update with conflicting same version refuses and never overwrites user content.
        assert!(matches!(
            catalog.install_bytes_with_origin(
                &pkg_v1_conflict,
                PackageOrigin::Factory,
                None,
                &policy,
                None
            ),
            Err(PackageInstallError::VersionHashConflict { .. })
        ));

        // Factory update v2 installs alongside v1; user-pinned default remains v1!
        let rec_v2 = match catalog
            .install_bytes_with_origin(&pkg_v2, PackageOrigin::Factory, None, &policy, None)
            .unwrap()
        {
            InstallOutcome::Installed(r) => r,
            other => panic!("expected Installed, got {other:?}"),
        };
        assert_eq!(rec_v2.origin, PackageOrigin::Factory);
        assert_eq!(
            catalog
                .default_generation_for("hex.pkg.utility")
                .unwrap()
                .unwrap()
                .package_major,
            1
        );

        // Explicit upgrade of default to v2 succeeds; unrequested downgrade back to v1 refuses.
        catalog
            .select_default_generation("hex.pkg.utility", (2, 0, 0), false)
            .unwrap();
        assert!(matches!(
            catalog.select_default_generation("hex.pkg.utility", (1, 0, 0), false),
            Err(PackageInstallError::InvalidManifest(_))
        ));
        // Explicit revert with allow_downgrade=true succeeds.
        catalog
            .select_default_generation("hex.pkg.utility", (1, 0, 0), true)
            .unwrap();

        // Uninstall v1 while live playback/export still holds a reference to v1:
        // hides from list_discoverable, keeps files on disk, and reclaim refuses until quiescent!
        assert!(
            catalog
                .request_uninstall("hex.pkg.utility", (1, 0, 0))
                .unwrap()
        );
        let discoverable = catalog.list_discoverable().unwrap();
        assert_eq!(discoverable.len(), 1);
        assert_eq!(discoverable[0].package_major, 2);
        let v1_dir = tmp.0.join(&rec_v1.generation_rel_dir);
        assert!(v1_dir.is_dir());

        // Still in use -> 0 reclaimed, directory preserved on disk.
        let reclaimed_busy = catalog
            .reclaim_pending_removals(|r| r.package_sha256 == rec_v1.package_sha256)
            .unwrap();
        assert_eq!(reclaimed_busy, 0);
        assert!(v1_dir.is_dir());

        // Last owner releases -> reclaim deletes v1 cleanly on control.
        let reclaimed_idle = catalog.reclaim_pending_removals(|_| false).unwrap();
        assert_eq!(reclaimed_idle, 1);
        assert!(!v1_dir.exists());

        // Tampering with v2's on-disk dylib triggers automatic quarantine on integrity verification.
        let v2_dylib = tmp
            .0
            .join(&rec_v2.generation_rel_dir)
            .join(&rec_v2.native_module_rel_path);
        fs::write(&v2_dylib, b"tampered-bytes").unwrap();
        assert!(matches!(
            catalog.verify_generation_integrity("hex.pkg.utility", (2, 0, 0)),
            Err(PackageInstallError::DigestMismatch(_))
        ));
        assert!(catalog.list_discoverable().unwrap().is_empty());
        assert!(
            catalog
                .find_exact("hex.pkg.utility", (2, 0, 0))
                .unwrap()
                .unwrap()
                .quarantined
        );
    }
}
