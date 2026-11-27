//! Control-side Finder `.hexdevice` package drop ingress, live catalog
//! discovery projection, and distinct browser-item insertion contracts (`#2995`).
//!
//! # Purpose
//! - Classify external OS file drops so `.hexdevice` drops route strictly to
//!   the bounded control-side [`DevicePackageCatalog`] installer without ever
//!   inserting a device onto a hovered track, while preserving `#2385`
//!   audio-file drag behavior and explicitly refusing mixed audio+package drops.
//! - Project discoverable installed package generations into searchable
//!   browser entries without requiring an app restart and without performing
//!   any filesystem or catalog work on audio callbacks.
//! - Execute distinct browser-item insertion/replacement commands guarded by
//!   captured `(project_generation, revision, track, replacing/before)` identity,
//!   hybrid `TrackId`, and existing effect/instrument slot rules.

use hex_lattice_contract::NativeDeviceState;
use hex_lattice_contract::package_abi::{DEVICE_KIND_AUDIO_EFFECT, DEVICE_KIND_INSTRUMENT};
use hex_project::{Command, Created, DeviceId, Editor, TrackId};
use hex_storage::device_packages::{
    AdmissionPolicy, DevicePackageCatalog, InstallOutcome, InstalledPackageRecord,
    PackageInstallError, PackageOrigin,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Maximum number of `.hexdevice` files accepted in a single Finder drop batch.
pub const MAX_PACKAGE_DROP_BATCH: usize = 32;

/// Classification of an external OS file drop onto Hex.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExternalDropKind {
    Empty,
    TooMany {
        count: usize,
        limit: usize,
    },
    /// Zero `.hexdevice` files: preserve existing `#2385` audio-file drop path.
    AudioOnly,
    /// Only `.hexdevice` files: install into the package library without
    /// mutating any hovered track.
    PackagesOnly(Vec<PathBuf>),
    /// Mixed `.hexdevice` and non-package files: refused deterministically so
    /// neither audio tracks nor package installs occur unexpectedly.
    Mixed {
        packages: usize,
        other: usize,
    },
}

/// Returns true if `path` has a `.hexdevice` file extension (case-insensitive).
pub fn is_package_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("hexdevice"))
}

/// Classify external Finder-dropped paths deterministically before any I/O or
/// track/project inspection occurs.
pub fn classify_external_drop(paths: &[PathBuf]) -> ExternalDropKind {
    if paths.is_empty() {
        return ExternalDropKind::Empty;
    }
    let mut packages = Vec::new();
    let mut other = 0usize;
    for path in paths {
        if is_package_file(path) {
            packages.push(path.clone());
        } else {
            other += 1;
        }
    }
    if packages.is_empty() {
        ExternalDropKind::AudioOnly
    } else if other > 0 {
        ExternalDropKind::Mixed {
            packages: packages.len(),
            other,
        }
    } else if packages.len() > MAX_PACKAGE_DROP_BATCH {
        ExternalDropKind::TooMany {
            count: packages.len(),
            limit: MAX_PACKAGE_DROP_BATCH,
        }
    } else {
        ExternalDropKind::PackagesOnly(packages)
    }
}

/// Device slot category advertised by an installed package.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PackageCategory {
    Effect,
    Instrument,
}

impl PackageCategory {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Effect => "effect",
            Self::Instrument => "instrument",
        }
    }
}

/// Browser-visible availability state for an installed package generation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PackageAvailability {
    Available,
    Quarantined,
    Unavailable(String),
}

impl PackageAvailability {
    pub fn as_lowercase_label(&self) -> String {
        match self {
            Self::Available => "available".to_owned(),
            Self::Quarantined => "quarantined · review trust before loading".to_owned(),
            Self::Unavailable(reason) => format!("unavailable · {}", reason.to_lowercase()),
        }
    }
}

/// Searchable browser entry projected from an installed package record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageBrowserEntry {
    pub type_id: String,
    pub display_name: String,
    pub category: PackageCategory,
    pub publisher_id: String,
    pub version_label: String,
    pub package_sha256: String,
    pub availability: PackageAvailability,
    pub origin: PackageOrigin,
}

impl PackageBrowserEntry {
    pub fn from_record(record: &InstalledPackageRecord) -> Self {
        let category = match record.manifest.device_kind {
            DEVICE_KIND_INSTRUMENT => PackageCategory::Instrument,
            DEVICE_KIND_AUDIO_EFFECT => PackageCategory::Effect,
            _ => PackageCategory::Effect,
        };
        let availability = if record.quarantined {
            PackageAvailability::Quarantined
        } else if record.removal_pending {
            PackageAvailability::Unavailable("removal pending".to_owned())
        } else {
            PackageAvailability::Available
        };
        Self {
            type_id: record.type_id.clone(),
            display_name: record.manifest.display_name.to_lowercase(),
            category,
            publisher_id: record.publisher_id.to_lowercase(),
            version_label: format!(
                "{}.{}.{}",
                record.package_major, record.package_minor, record.package_patch
            ),
            package_sha256: record.package_sha256.clone(),
            availability,
            origin: record.origin,
        }
    }

    /// Case-insensitive browser search across display name, type ID, publisher,
    /// category, and version.
    pub fn matches_query(&self, query: &str) -> bool {
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return true;
        }
        self.display_name.contains(&needle)
            || self.type_id.to_lowercase().contains(&needle)
            || self.publisher_id.contains(&needle)
            || self.category.as_str().contains(&needle)
            || self.version_label.contains(&needle)
    }

    /// Lowercase single-line browser description for UI rendering via `theme::text`.
    pub fn summary_label(&self) -> String {
        format!(
            "{} · {} · {} · v{} · {}",
            self.display_name,
            self.category.as_str(),
            self.publisher_id,
            self.version_label,
            self.availability.as_lowercase_label()
        )
    }
}

/// Deterministic per-file outcome in a Finder `.hexdevice` install batch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PackageDropItemOutcome {
    Installed(PackageBrowserEntry),
    AlreadyInstalled(PackageBrowserEntry),
    Conflict { path: PathBuf, message: String },
    Canceled { path: PathBuf },
    Failed { path: PathBuf, message: String },
}

/// Complete deterministic report for a bounded multi-file `.hexdevice` drop.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PackageDropBatchReport {
    pub items: Vec<PackageDropItemOutcome>,
}

impl PackageDropBatchReport {
    pub fn installed_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| matches!(i, PackageDropItemOutcome::Installed(_)))
            .count()
    }

    pub fn already_installed_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| matches!(i, PackageDropItemOutcome::AlreadyInstalled(_)))
            .count()
    }

    pub fn conflict_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| matches!(i, PackageDropItemOutcome::Conflict { .. }))
            .count()
    }

    pub fn canceled_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| matches!(i, PackageDropItemOutcome::Canceled { .. }))
            .count()
    }

    pub fn failed_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| matches!(i, PackageDropItemOutcome::Failed { .. }))
            .count()
    }

    /// Lowercase summary string suitable for Hex status/error reporting.
    pub fn summary_message(&self) -> String {
        let installed = self.installed_count();
        let already = self.already_installed_count();
        let conflicts = self.conflict_count();
        let canceled = self.canceled_count();
        let failed = self.failed_count();
        format!(
            "package install · {installed} installed · {already} already installed · {conflicts} conflicts · {failed} failed · {canceled} canceled"
        )
    }
}

/// Validate and atomically install a bounded batch of Finder-dropped
/// `.hexdevice` files into the explicit `library_root` on a control/worker
/// thread. Never modifies any track or executes module code.
pub fn install_dropped_packages(
    library_root: &Path,
    paths: &[PathBuf],
    policy: &AdmissionPolicy,
    cancel: &AtomicBool,
) -> Result<PackageDropBatchReport, String> {
    if paths.is_empty() {
        return Err("package drop refused · choose 1..32 .hexdevice packages".to_owned());
    }
    if paths.len() > MAX_PACKAGE_DROP_BATCH {
        return Err(format!(
            "package drop refused · choose 1..{MAX_PACKAGE_DROP_BATCH} .hexdevice packages"
        ));
    }
    let catalog = DevicePackageCatalog::open_and_recover(library_root).map_err(|err| {
        format!(
            "package library unavailable · {}",
            err.to_string().to_lowercase()
        )
    })?;

    let mut report = PackageDropBatchReport {
        items: Vec::with_capacity(paths.len()),
    };
    for path in paths {
        if cancel.load(Ordering::Acquire) {
            report
                .items
                .push(PackageDropItemOutcome::Canceled { path: path.clone() });
            continue;
        }
        if !is_package_file(path) {
            report.items.push(PackageDropItemOutcome::Failed {
                path: path.clone(),
                message: "unsupported file extension · expected .hexdevice".to_owned(),
            });
            continue;
        }
        match catalog.install_file(path, policy, Some(cancel)) {
            Ok(InstallOutcome::Installed(record)) => {
                report.items.push(PackageDropItemOutcome::Installed(
                    PackageBrowserEntry::from_record(&record),
                ));
            }
            Ok(InstallOutcome::AlreadyInstalled(record)) => {
                report.items.push(PackageDropItemOutcome::AlreadyInstalled(
                    PackageBrowserEntry::from_record(&record),
                ));
            }
            Err(PackageInstallError::Cancelled) => {
                report
                    .items
                    .push(PackageDropItemOutcome::Canceled { path: path.clone() });
            }
            Err(
                err @ (PackageInstallError::VersionHashConflict { .. }
                | PackageInstallError::PublisherTakeover { .. }
                | PackageInstallError::UntrustedProvenance(_)
                | PackageInstallError::UnsupportedPlatform { .. }),
            ) => {
                report.items.push(PackageDropItemOutcome::Conflict {
                    path: path.clone(),
                    message: err.to_string().to_lowercase(),
                });
            }
            Err(err) => {
                report.items.push(PackageDropItemOutcome::Failed {
                    path: path.clone(),
                    message: err.to_string().to_lowercase(),
                });
            }
        }
    }
    Ok(report)
}

/// Refresh discoverable installed packages directly from `library_root`
/// without restarting the application. Runs strictly on control threads.
pub fn discover_installed_packages(
    library_root: &Path,
) -> Result<Vec<PackageBrowserEntry>, String> {
    let catalog = DevicePackageCatalog::open_and_recover(library_root).map_err(|err| {
        format!(
            "package library unavailable · {}",
            err.to_string().to_lowercase()
        )
    })?;
    let records = catalog.list_discoverable().map_err(|err| {
        format!(
            "package catalog unreadable · {}",
            err.to_string().to_lowercase()
        )
    })?;
    let mut entries: Vec<PackageBrowserEntry> = records
        .iter()
        .map(PackageBrowserEntry::from_record)
        .collect();
    entries.sort_by(|a, b| {
        a.display_name
            .cmp(&b.display_name)
            .then_with(|| a.type_id.cmp(&b.type_id))
            .then_with(|| a.version_label.cmp(&b.version_label))
    });
    Ok(entries)
}

/// Captured browser drag/insert destination guarded by project generation and
/// revision so stale or cancelled inserts never mutate a track.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PackageInsertDestination {
    pub project_generation: u64,
    pub revision: u64,
    pub track: TrackId,
    pub before_device: Option<DeviceId>,
    pub replace_instrument: Option<DeviceId>,
}

/// Distinct browser-item insertion/replacement action. Preserves hybrid
/// `TrackId` and existing effect/instrument slot rules, and refuses stale
/// destinations, cancelled requests, or quarantined/unavailable packages.
pub fn insert_discovered_package(
    editor: &mut Editor,
    current_generation: u64,
    current_revision: u64,
    entry: &PackageBrowserEntry,
    default_state: NativeDeviceState,
    destination: PackageInsertDestination,
    cancelled: bool,
) -> Result<DeviceId, String> {
    if cancelled {
        return Err("package insert canceled".to_owned());
    }
    if destination.project_generation != current_generation
        || destination.revision != current_revision
    {
        return Err("package insert canceled · project changed".to_owned());
    }
    if entry.availability != PackageAvailability::Available {
        return Err(format!(
            "package insert refused · {}",
            entry.availability.as_lowercase_label()
        ));
    }
    if default_state.type_id().as_str().ok() != Some(entry.type_id.as_str()) {
        return Err("package insert refused · device state type mismatch".to_owned());
    }
    let track = editor.project().track(destination.track).ok_or_else(|| {
        "package insert canceled · target track is no longer available".to_owned()
    })?;

    let command = match entry.category {
        PackageCategory::Effect => {
            if let Some(before) = destination.before_device
                && !track.devices().iter().any(|device| device.id() == before)
            {
                return Err(
                    "package insert canceled · insertion slot is no longer available".to_owned(),
                );
            }
            Command::InsertDevice {
                track: destination.track,
                before: destination.before_device,
                state: default_state,
            }
        }
        PackageCategory::Instrument => {
            if track.is_group() {
                return Err(
                    "package insert refused · group tracks cannot host instruments".to_owned(),
                );
            }
            match (track.instrument(), destination.replace_instrument) {
                (None, None) => Command::InsertInstrument {
                    track: destination.track,
                    state: default_state,
                },
                (Some(existing), Some(expected)) if existing.id() == expected => {
                    Command::ReplaceTrackInstrument {
                        track: destination.track,
                        replacing: expected,
                        state: default_state,
                    }
                }
                (Some(_), None) => {
                    return Err(
                        "package insert refused · track already has an instrument".to_owned()
                    );
                }
                _ => {
                    return Err("package insert canceled · instrument slot changed".to_owned());
                }
            }
        }
    };

    match editor.execute(command) {
        Ok(Created::Device(id)) => Ok(id),
        Ok(_) => Err("package insert returned unexpected created item".to_owned()),
        Err(err) => Err(format!(
            "package insert refused · {}",
            err.to_string().to_lowercase()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice_contract::DeviceTypeId;
    use hex_lattice_contract::package_abi::{
        CAP_AUDIO_EFFECT, CAP_INSTRUMENT, CAP_ZERO_LATENCY, PACKAGE_ABI_VERSION_V1,
        PackageIdentityEnvelope,
    };
    use hex_project::Project;
    use hex_storage::device_packages::{
        ArchiveCompression, ArchiveEntryKind, HEXDEVICE_ARCHIVE_MAGIC, PackageManifest,
        PackageProvenance,
    };
    use std::collections::BTreeMap;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "hex-pkg-ingress-{tag}-{}-{nanos}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        // Use a minimal deterministic test helper via hex_storage's exact format
        // by building valid archives with a known Mach-O header and matching
        // manifest digest. Since `hex-session` doesn't depend on `sha2`, reuse
        // Python/std? Wait: let's compute real SHA-256 in ~35 lines of pure Rust!
        const K: [u32; 64] = [
            0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
            0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
            0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
            0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
            0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
            0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
            0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
            0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
            0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
            0xc67178f2,
        ];
        let mut h: [u32; 8] = [
            0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
            0x5be0cd19,
        ];
        let bit_len = (bytes.len() as u64) * 8;
        let mut msg = bytes.to_vec();
        msg.push(0x80);
        while (msg.len() % 64) != 56 {
            msg.push(0);
        }
        msg.extend_from_slice(&bit_len.to_be_bytes());
        for chunk in msg.chunks_exact(64) {
            let mut w = [0u32; 64];
            for i in 0..16 {
                w[i] = u32::from_be_bytes([
                    chunk[i * 4],
                    chunk[i * 4 + 1],
                    chunk[i * 4 + 2],
                    chunk[i * 4 + 3],
                ]);
            }
            for i in 16..64 {
                let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
                let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
                w[i] = w[i - 16]
                    .wrapping_add(s0)
                    .wrapping_add(w[i - 7])
                    .wrapping_add(s1);
            }
            let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
            for i in 0..64 {
                let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
                let ch = (e & f) ^ ((!e) & g);
                let temp1 = hh
                    .wrapping_add(s1)
                    .wrapping_add(ch)
                    .wrapping_add(K[i])
                    .wrapping_add(w[i]);
                let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
                let maj = (a & b) ^ (a & c) ^ (b & c);
                let temp2 = s0.wrapping_add(maj);
                hh = g;
                g = f;
                f = e;
                e = d.wrapping_add(temp1);
                d = c;
                c = b;
                b = a;
                a = temp1.wrapping_add(temp2);
            }
            for (slot, delta) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
                *slot = slot.wrapping_add(delta);
            }
        }
        h.iter().map(|word| format!("{word:08x}")).collect()
    }

    fn build_test_hexdevice(
        type_id: &str,
        display_name: &str,
        device_kind: u32,
        target_os: &str,
        target_arch: &str,
        payload_tag: u8,
    ) -> Vec<u8> {
        let mut dylib = vec![0xCF, 0xFA, 0xED, 0xFE]; // 64-bit Mach-O LE magic
        dylib.extend_from_slice(&[payload_tag; 28]);
        let native_path = "bin/device.dylib".to_owned();
        let mut digests = BTreeMap::new();
        digests.insert(native_path.clone(), sha256_hex(&dylib));

        let manifest = PackageManifest {
            schema_version: 1,
            publisher_id: "hex.factory".to_owned(),
            type_id: type_id.to_owned(),
            display_name: display_name.to_owned(),
            package_major: 1,
            package_minor: 0,
            package_patch: 0,
            state_version: 1,
            abi_version: PACKAGE_ABI_VERSION_V1,
            ui_version: 1,
            device_kind,
            capabilities: if device_kind == DEVICE_KIND_INSTRUMENT {
                CAP_INSTRUMENT | CAP_ZERO_LATENCY
            } else {
                CAP_AUDIO_EFFECT | CAP_ZERO_LATENCY
            },
            target_os: target_os.to_owned(),
            target_arch: target_arch.to_owned(),
            min_os_version: "14.0".to_owned(),
            native_module_path: native_path.clone(),
            ui_schema_path: None,
            presets: vec![],
            assets: vec![],
            native_dependencies: vec![],
            provenance: PackageProvenance::ReviewedDevelopmentTest {
                authorization_sha256: "a".repeat(64),
                reviewer_note: "test".to_owned(),
            },
            content_digests: digests,
        };
        let header = serde_json::json!({
            "manifest": manifest,
            "entries": [{
                "path": native_path,
                "kind": ArchiveEntryKind::RegularFile,
                "compression": ArchiveCompression::Stored,
                "compressed_bytes": dylib.len(),
                "expanded_bytes": dylib.len(),
            }]
        });
        let header_bytes = serde_json::to_vec(&header).unwrap();
        let mut out = Vec::new();
        out.extend_from_slice(HEXDEVICE_ARCHIVE_MAGIC);
        out.extend_from_slice(&(header_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&header_bytes);
        out.extend_from_slice(&dylib);
        out
    }

    fn test_policy() -> AdmissionPolicy {
        let mut policy = AdmissionPolicy::host_default();
        policy.allow_reviewed_test("hex.factory", "a".repeat(64));
        policy
    }

    fn dummy_package_state(type_id: &str) -> NativeDeviceState {
        let env = PackageIdentityEnvelope {
            publisher_id: "hex.factory".to_owned(),
            type_id: type_id.to_owned(),
            package_major: 1,
            package_minor: 0,
            package_patch: 0,
            state_version: 1,
            abi_version: PACKAGE_ABI_VERSION_V1,
            ui_version: 1,
            package_sha256: [0x55; 32],
        };
        let payload = env.encode_with_raw_state(&[1, 2, 3]).unwrap();
        NativeDeviceState::new(DeviceTypeId::new(type_id).unwrap(), 1, payload).unwrap()
    }

    #[test]
    fn finder_drop_classification_preserves_audio_refuses_mixed_and_bounds_batches() {
        assert_eq!(classify_external_drop(&[]), ExternalDropKind::Empty);
        assert_eq!(
            classify_external_drop(&[
                PathBuf::from("/tmp/loop.wav"),
                PathBuf::from("/tmp/hit.aif")
            ]),
            ExternalDropKind::AudioOnly
        );
        assert_eq!(
            classify_external_drop(&[PathBuf::from("/tmp/utility.HEXDEVICE")]),
            ExternalDropKind::PackagesOnly(vec![PathBuf::from("/tmp/utility.HEXDEVICE")])
        );
        assert_eq!(
            classify_external_drop(&[
                PathBuf::from("/tmp/utility.hexdevice"),
                PathBuf::from("/tmp/loop.wav")
            ]),
            ExternalDropKind::Mixed {
                packages: 1,
                other: 1
            }
        );
        let too_many: Vec<_> = (0..=MAX_PACKAGE_DROP_BATCH)
            .map(|i| PathBuf::from(format!("/tmp/dev_{i}.hexdevice")))
            .collect();
        assert_eq!(
            classify_external_drop(&too_many),
            ExternalDropKind::TooMany {
                count: MAX_PACKAGE_DROP_BATCH + 1,
                limit: MAX_PACKAGE_DROP_BATCH,
            }
        );
    }

    #[test]
    fn finder_drop_installs_refreshes_browser_and_distinct_insert_enforces_slots_and_liveness() {
        let dir = unique_dir("end-to-end");
        let lib_root = dir.join("library");
        let drop_dir = dir.join("drops");
        fs::create_dir_all(&drop_dir).unwrap();

        let util_path = drop_dir.join("utility.hexdevice");
        let sine_path = drop_dir.join("sine.hexdevice");
        let bad_platform_path = drop_dir.join("win_only.hexdevice");
        let corrupt_path = drop_dir.join("corrupt.hexdevice");
        fs::write(
            &util_path,
            build_test_hexdevice(
                "hex.pkg.utility",
                "utility",
                DEVICE_KIND_AUDIO_EFFECT,
                std::env::consts::OS,
                std::env::consts::ARCH,
                0x11,
            ),
        )
        .unwrap();
        fs::write(
            &sine_path,
            build_test_hexdevice(
                "hex.pkg.sine",
                "sine",
                DEVICE_KIND_INSTRUMENT,
                std::env::consts::OS,
                std::env::consts::ARCH,
                0x22,
            ),
        )
        .unwrap();
        fs::write(
            &bad_platform_path,
            build_test_hexdevice(
                "hex.pkg.winonly",
                "winonly",
                DEVICE_KIND_AUDIO_EFFECT,
                "windows",
                "x86_64",
                0x33,
            ),
        )
        .unwrap();
        fs::write(&corrupt_path, b"NOT_A_HEXDEVICE").unwrap();

        let policy = test_policy();
        let cancel = AtomicBool::new(false);

        // Project tracks before Finder drop remain completely untouched by install.
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "lead".into(),
            })
            .unwrap()
        else {
            panic!("track");
        };
        assert!(editor.project().track(track).unwrap().devices().is_empty());
        assert!(
            editor
                .project()
                .track(track)
                .unwrap()
                .instrument()
                .is_none()
        );

        // 1. Valid multi-file drop + wrong-platform + corrupt in one bounded batch.
        let report = install_dropped_packages(
            &lib_root,
            &[
                util_path.clone(),
                sine_path.clone(),
                bad_platform_path,
                corrupt_path,
            ],
            &policy,
            &cancel,
        )
        .expect("batch report");
        assert_eq!(report.installed_count(), 2);
        assert_eq!(report.conflict_count(), 1);
        assert_eq!(report.failed_count(), 1);
        assert!(report.summary_message().chars().all(|c| !c.is_uppercase()));
        // Installing must NEVER auto-insert onto the hovered track.
        assert!(editor.project().track(track).unwrap().devices().is_empty());
        assert!(
            editor
                .project()
                .track(track)
                .unwrap()
                .instrument()
                .is_none()
        );

        // 2. Repeat same-hash drop is idempotent (`AlreadyInstalled`).
        let repeat = install_dropped_packages(
            &lib_root,
            std::slice::from_ref(&util_path),
            &policy,
            &cancel,
        )
        .expect("repeat drop");
        assert_eq!(repeat.already_installed_count(), 1);
        assert_eq!(repeat.installed_count(), 0);

        // 3. Aborted/cancelled drop marks items Canceled without corrupting catalog.
        let aborted = AtomicBool::new(true);
        let cancel_report =
            install_dropped_packages(&lib_root, &[util_path], &policy, &aborted).unwrap();
        assert_eq!(cancel_report.canceled_count(), 1);

        // 4. Browser catalog refreshes live without restart and supports search.
        let discovered = discover_installed_packages(&lib_root).unwrap();
        assert_eq!(discovered.len(), 2);
        let sine_entry = discovered
            .iter()
            .find(|e| e.matches_query("sine") && e.matches_query("instrument"))
            .expect("discover sine")
            .clone();
        let util_entry = discovered
            .iter()
            .find(|e| e.matches_query("utility") && e.matches_query("effect"))
            .expect("discover utility")
            .clone();
        assert!(sine_entry.summary_label().contains("instrument"));
        assert!(util_entry.summary_label().contains("effect"));

        // 5. Distinct browser insert enforces stale generation/revision, cancellation,
        //    and hybrid TrackId + effect/instrument slot rules.
        let stale_dest = PackageInsertDestination {
            project_generation: 99,
            revision: 1,
            track,
            before_device: None,
            replace_instrument: None,
        };
        assert!(
            insert_discovered_package(
                &mut editor,
                1,
                1,
                &util_entry,
                dummy_package_state("hex.pkg.utility"),
                stale_dest,
                false,
            )
            .is_err()
        );
        let valid_dest = PackageInsertDestination {
            project_generation: 1,
            revision: 1,
            track,
            before_device: None,
            replace_instrument: None,
        };
        assert!(
            insert_discovered_package(
                &mut editor,
                1,
                1,
                &util_entry,
                dummy_package_state("hex.pkg.utility"),
                valid_dest,
                true, // cancelled
            )
            .is_err()
        );
        let inserted_fx = insert_discovered_package(
            &mut editor,
            1,
            1,
            &util_entry,
            dummy_package_state("hex.pkg.utility"),
            valid_dest,
            false,
        )
        .expect("insert effect");
        assert_eq!(
            editor.project().track(track).unwrap().devices()[0].id(),
            inserted_fx
        );

        let inserted_inst = insert_discovered_package(
            &mut editor,
            1,
            2,
            &sine_entry,
            dummy_package_state("hex.pkg.sine"),
            PackageInsertDestination {
                project_generation: 1,
                revision: 2,
                track,
                before_device: None,
                replace_instrument: None,
            },
            false,
        )
        .expect("insert instrument");
        assert_eq!(
            editor
                .project()
                .track(track)
                .unwrap()
                .instrument()
                .unwrap()
                .id(),
            inserted_inst
        );
        // Second instrument without explicit replacement is refused.
        assert!(
            insert_discovered_package(
                &mut editor,
                1,
                3,
                &sine_entry,
                dummy_package_state("hex.pkg.sine"),
                PackageInsertDestination {
                    project_generation: 1,
                    revision: 3,
                    track,
                    before_device: None,
                    replace_instrument: None,
                },
                false,
            )
            .is_err()
        );
        // Explicit replacement swaps instrument cleanly.
        let replaced_inst = insert_discovered_package(
            &mut editor,
            1,
            3,
            &sine_entry,
            dummy_package_state("hex.pkg.sine"),
            PackageInsertDestination {
                project_generation: 1,
                revision: 3,
                track,
                before_device: None,
                replace_instrument: Some(inserted_inst),
            },
            false,
        )
        .expect("replace instrument");
        assert_ne!(replaced_inst, inserted_inst);

        // 6. Unavailable library root (a file blocking the directory) fails cleanly.
        let blocked_root = dir.join("not_a_dir");
        fs::write(&blocked_root, b"occupied").unwrap();
        let err = discover_installed_packages(&blocked_root).unwrap_err();
        assert!(err.contains("package library unavailable"));

        let _ = fs::remove_dir_all(&dir);
    }
}
