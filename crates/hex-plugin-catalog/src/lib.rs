//! # Purpose
//!
//! Bounded control-side persistence and selected-bundle discovery coordination for
//! VST3 browser metadata. Owns a recoverable current-format catalog, copied records,
//! last-good/error projection and the outer standalone scanner's control lifetime.
//! All filesystem/process work runs on a background control worker.
//!
//! # Dependencies
//!
//! `hex-plugin-scan` supplies copied metadata, fixed package identity and admission
//! checks. Pinned serde/serde_json encode bounded current-format records. Pinned
//! fs2 supplies nonblocking advisory writer exclusion, with locks held only during
//! catalog read-modify-publication, never across foreign-code execution.
//!
//! # Does not own
//!
//! No DSP, ordinary plugin instances, native ABI, process-global signal handlers,
//! GPUI, project edits or audio callback work. Does not launch Hex, load foreign
//! libraries in the application, scan folders automatically or authenticate plugins.
//! The standalone scanner retains its own native helper; outer cancellation uses
//! stdin and keeps draining/reaping, never a process-group kill.
//!
//!
//! The first workflow scans one explicitly selected bundle. Startup restores saved
//! metadata without executing code. A catalog holds at most512 modules/2048 classes
//! and4MiB of JSON; paths are bounded to4096 bytes. The worker drains at most4MiB of
//! report output and64KiB of outer diagnostics in fixed polling batches, with one
//! active operation per browser. Its cancellation token has an atomic publication
//! boundary: cancellation wins until catalog publication begins; later requests
//! report finishing instead of falsely claiming cancellation. Accepted native metadata requires direct helper retirement before that boundary. A stuck native cleanup retains its outer
//! scanner/control owner rather than authorizing a process-group kill.
//!
//! The explicit standard-folder action uses the user's `Library/Audio/Plug-Ins/VST3`
//! and the local `/Library/Audio/Plug-Ins/VST3`. Missing roots are empty; invalid,
//! linked root components and traversal errors refuse the batch before code runs.
//! Traversal never enters bundles or follows encountered links; skipped links have a
//! visible count. Bounds are three roots,4096 entries,1024 directories,8 levels of
//! folder recursion and512 bundles, with4096-byte/64-component paths. Class/JSON/output limits remain fixed. Enumeration alone does not load vendor
//! code or establish compatibility.
//!
//! A batch retains at most one owned outer scanner/native helper chain at a time.
//! Normal per-module failures are recorded alongside successful modules; missing or
//! unconfirmed direct retirement stops the batch before another launch; that bundle
//! is recorded as a failure and the results already obtained are still published.
//! This does not claim cleanup of foreign descendants, which remains unknown. Pending copied
//! reports are validated against class/JSON limits and diagnostics are discarded.
//! Cancellation discards all pending batch results; publication has one atomic token
//! boundary followed by a fresh writer-locked merge. Capacity/write/validation errors
//! preserve the prior file. No missing/stale/duplicate record is deleted here, and
//! no scan runs at startup. The UI has one busy/cancel owner for either scan action.
//!
//! Rows classify copied optional Factory2 subcategory tokens exactly: `Instrument`,
//! `Fx`, or both (the SDK explicitly permits `Fx|Instrument`). Missing or unrecognized
//! type tokens remain unknown; names, substring matches and flags never infer a type.
//! Nonempty class vendor metadata overrides the factory vendor. Type/vendor search
//! and restart retain original metadata bytes. These are reported discovery labels;
//! no row is a playable or insertable device. Custom folders use the same bounded
//! explicit scan; only results persist, with no automatic folder execution.
//!
//! Background browser reload/publication revalidates retained bundle and executable
//! bytes through the scanner's bounded source reader, one module at a time. Missing,
//! linked or changed files produce transient unavailable/rescan labels; they never
//! rewrite last-good metadata, persistent scan failures or the catalog file. These
//! observations are not admission authority: the host must revalidate at use time.
//! Duplicate CIDs remain distinct explicit module/CID rows in path/CID order; no
//! arbitrary replacement is chosen when a path disappears or a binary changes.
//! Cache data never authorizes future plugin instance admission; the processing host
//! must verify current source and resources.
//!
//! Catalog version 3 is the current format. It adds `failed_bundle`: the bundle
//! identity (canonical path and Info.plist digest, never executed code) observed
//! when a scan failed. Folder scans skip a failed bundle while its current identity
//! still equals that record; an explicit single-bundle scan always runs again.
//! Version 2 files migrate in memory on load (no failure has a recorded identity, so
//! each is rescanned once) and are rewritten as version 3 only by the next catalog
//! update. Version 1 alpha files and newer versions remain intact with a visible
//! error, without migration. A binary change that keeps Info.plist byte-identical
//! is not detected by the failure identity; an explicit rescan recovers it.
//! The native integration tests require an explicit signed original package through
//! `HEX_VST3_CATALOG_TEST_APP` and original fixture directory through
//! `HEX_VST3_CATALOG_FIXTURES`; ordinary runs keep these tests ignored. They do not
//! start the Hex application or establish physical device/listening acceptance.
//!
//! Dynamic/discardable or unknown factory flags retain historical metadata only;
//! rows explicitly require a rescan. Stored inventory never establishes future
//! cacheable class availability or authorizes processing admission.
mod resolve;
pub mod roots;
pub mod storage;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod worker;

use hex_plugin_scan::{Discovery, PUBLIC_SDK, SDK, macos::report::Report};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
};

pub const MAX_MODULES: usize = 512;
pub const MAX_CLASSES: usize = 2048;
pub const MAX_CATALOG_BYTES: usize = 4 * 1024 * 1024;
/// Current saved catalog format; see the crate documentation for migration.
pub const CATALOG_VERSION: u32 = 3;
const CATALOG_VERSION_2: u32 = 2;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub path: PathBuf,
    pub last_good: Option<Report>,
    pub source: Option<String>,
    pub failure: Option<String>,
    /// Bundle identity observed by the failed scan, when it was inspected.
    /// Absent in version 2 files; always paired with `failure`.
    #[serde(default)]
    pub failed_bundle: Option<hex_plugin_scan::macos::identity::Bundle>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    version: u32,
    pub entries: Vec<Entry>,
    #[serde(skip)]
    source_errors: BTreeMap<PathBuf, String>,
}
impl Default for Catalog {
    fn default() -> Self {
        Self {
            version: CATALOG_VERSION,
            entries: Vec::new(),
            source_errors: BTreeMap::new(),
        }
    }
}

fn bounded(value: &str, maximum: usize) -> bool {
    value.len() <= maximum && !value.contains('\0')
}
fn digest(value: &str, size: usize) -> bool {
    value.len() == size
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
fn path_valid(path: &Path) -> bool {
    path.is_absolute()
        && path.to_str().is_some_and(|s| bounded(s, 4096))
        && path.components().count() <= 64
        && !path.components().any(|c| matches!(c, Component::ParentDir))
}

pub fn validate_discovery(value: &Discovery) -> Result<(), String> {
    let mut ids = BTreeSet::new();
    if value.request == 0
        || value.sdk != SDK
        || value.public_sdk != PUBLIC_SDK
        || value.platform != "darwin-arm64"
        || !bounded(&value.vendor, 64)
        || !bounded(&value.url, 256)
        || !bounded(&value.email, 128)
        || value.classes.len() > 1024
    {
        return Err("invalid catalog discovery identity or bounds".into());
    }
    for class in &value.classes {
        let extension = class.class_flags.is_some();
        if class.subcategories.is_some() != extension
            || class.class_vendor.is_some() != extension
            || class.class_version.is_some() != extension
            || class.class_sdk_version.is_some() != extension
            || class
                .subcategories
                .as_ref()
                .is_some_and(|s| !bounded(s, 128))
            || class.class_vendor.as_ref().is_some_and(|s| !bounded(s, 64))
            || class.cid == [0; 16]
            || !ids.insert(class.cid)
            || class.cardinality < 0
            || !bounded(&class.category, 32)
            || !bounded(&class.name, 64)
            || class
                .class_version
                .as_ref()
                .is_some_and(|s| !bounded(s, 64))
            || class
                .class_sdk_version
                .as_ref()
                .is_some_and(|s| !bounded(s, 64))
        {
            return Err("invalid catalog class metadata".into());
        }
    }
    Ok(())
}

pub fn validate_report(report: &Report) -> Result<(), String> {
    let executable = report
        .executable
        .as_ref()
        .ok_or("catalog executable missing")?;
    let discovery = report.discovery.as_ref().ok_or("catalog classes missing")?;
    if report.profile != "explicit-user-trusted-macos-factory-only"
        || report.request != 1
        || report.generation != 1
        || discovery.request != report.request
        || report.error.is_some()
        || report.interruption.is_some()
        || !report.cleanup.may_publish()
        || report.cleanup.direct_exit_signal.is_some()
        || report.cleanup.descendants != "unknown"
        || !path_valid(&report.bundle.supplied_path)
        || !path_valid(&report.bundle.canonical_path)
        || !digest(&report.bundle.info_sha256, 64)
        || !digest(&executable.sha256, 64)
        || !digest(&report.helper_sha256, 64)
        || !digest(&report.receipt_sha256, 64)
        || executable.relative_path.len() > 4096
        || executable
            .relative_path
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == "..")
        || Path::new(&executable.relative_path).is_absolute()
        || report
            .bundle_identifier
            .as_ref()
            .is_none_or(|s| s.is_empty() || !bounded(s, 256))
    {
        return Err("incomplete or invalid accepted factory report".into());
    }
    validate_discovery(discovery)
}

impl Catalog {
    /// Upgrade a decoded supported older format in memory. Version 2 has no
    /// `failed_bundle` field, so a decoded value there is malformed, not migrated.
    pub(crate) fn migrate(&mut self) -> Result<(), String> {
        if self.version == CATALOG_VERSION_2 {
            if self
                .entries
                .iter()
                .any(|entry| entry.failed_bundle.is_some())
            {
                return Err("invalid version 2 plugin catalog; existing file preserved".into());
            }
            self.version = CATALOG_VERSION;
        }
        Ok(())
    }

    /// True when `path` last failed with exactly this bundle identity, so a
    /// folder scan may skip it. Pure metadata; the caller inspects `current`.
    pub fn failed_unchanged(
        &self,
        path: &Path,
        current: &hex_plugin_scan::macos::identity::Bundle,
    ) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.path == path && entry.failed_bundle.as_ref() == Some(current))
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.version != CATALOG_VERSION || self.entries.len() > MAX_MODULES {
            return Err("unsupported or oversized plugin catalog".into());
        }
        let mut paths = BTreeSet::new();
        let mut classes = 0_usize;
        for entry in &self.entries {
            if !path_valid(&entry.path)
                || entry.path.extension().and_then(|s| s.to_str()) != Some("vst3")
                || !paths.insert(&entry.path)
                || entry.failure.as_ref().is_some_and(|s| !bounded(s, 512))
                || entry.failed_bundle.as_ref().is_some_and(|bundle| {
                    entry.failure.is_none()
                        || bundle.supplied_path != entry.path
                        || !path_valid(&bundle.canonical_path)
                        || !digest(&bundle.info_sha256, 64)
                })
            {
                return Err("invalid plugin catalog entry".into());
            }
            if let Some(report) = &entry.last_good {
                validate_report(report)?;
                if entry.path != report.bundle.supplied_path
                    || entry.source.as_ref().is_none_or(|s| !digest(s, 40))
                    || !report.stdout.is_empty()
                    || !report.stderr.is_empty()
                {
                    return Err("invalid retained plugin snapshot".into());
                }
                classes = classes
                    .checked_add(report.discovery.as_ref().map_or(0, |d| d.classes.len()))
                    .ok_or("catalog count overflow")?;
            } else if entry.source.is_some() || entry.failure.is_none() {
                return Err("catalog entry has no scan result".into());
            }
        }
        if classes > MAX_CLASSES {
            return Err("plugin catalog class bound exceeded".into());
        }
        Ok(())
    }

    /// Record a scan result. A failure recorded here has no bundle identity and
    /// is rescanned by the next folder scan; see [`Catalog::record_failure`].
    pub fn record(
        &mut self,
        path: PathBuf,
        result: Result<(Report, String), String>,
    ) -> Result<(), String> {
        self.record_outcome(path, result, None)
    }

    /// Record a failed scan with the bundle identity it observed, so folder
    /// scans skip this bundle until its identity changes.
    pub fn record_failure(
        &mut self,
        path: PathBuf,
        error: String,
        bundle: Option<hex_plugin_scan::macos::identity::Bundle>,
    ) -> Result<(), String> {
        self.record_outcome(path, Err(error), bundle)
    }

    fn record_outcome(
        &mut self,
        path: PathBuf,
        result: Result<(Report, String), String>,
        failed_bundle: Option<hex_plugin_scan::macos::identity::Bundle>,
    ) -> Result<(), String> {
        if !path_valid(&path) || path.extension().and_then(|s| s.to_str()) != Some("vst3") {
            return Err("absolute bounded plugin path required".into());
        }
        let mut next = self.clone();
        let index = match next.entries.iter().position(|e| e.path == path) {
            Some(index) => index,
            None => {
                if next.entries.len() == MAX_MODULES {
                    return Err("plugin catalog module capacity reached".into());
                }
                next.entries.push(Entry {
                    path: path.clone(),
                    last_good: None,
                    source: None,
                    failure: None,
                    failed_bundle: None,
                });
                next.entries.len() - 1
            }
        };
        let entry = &mut next.entries[index];
        match result {
            Ok((mut report, source)) => {
                validate_report(&report)?;
                if report.bundle.supplied_path != path || !digest(&source, 40) {
                    return Err("plugin result source/path mismatch".into());
                }
                report.stdout.clear();
                report.stderr.clear();
                entry.last_good = Some(report);
                entry.source = Some(source);
                entry.failure = None;
                entry.failed_bundle = None;
                next.source_errors.remove(&path);
            }
            Err(error) => {
                entry.failure = Some(error.chars().take(128).collect());
                entry.failed_bundle = failed_bundle;
            }
        }
        next.entries.sort_by(|a, b| a.path.cmp(&b.path));
        next.validate()?;
        *self = next;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub module: PathBuf,
    pub cid: Option<[u8; 16]>,
    pub name: String,
    pub vendor: String,
    pub status: String,
}
/// Exact copied metadata for an explicit effect activation request. This is
/// neither source admission nor permission to reuse a cached executable: the
/// processing owner must inspect the selected module again, compare both
/// digests/class and retain its actual native preparation through the terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectSelection {
    pub module: PathBuf,
    pub cid: [u8; 16],
    pub class_name: String,
    pub executable_relative_path: String,
    pub executable_sha256: String,
    pub info_sha256: String,
}
fn reported_kind(class: &hex_plugin_scan::Class) -> &'static str {
    let mut instrument = false;
    let mut effect = false;
    if let Some(categories) = &class.subcategories {
        for token in categories.split('|') {
            instrument |= token == "Instrument";
            effect |= token == "Fx";
        }
    }
    match (instrument, effect) {
        (true, false) => "instrument",
        (false, true) => "effect",
        (true, true) => "instrument and effect",
        (false, false) => "processor · type unknown",
    }
}

impl Catalog {
    /// Capture one explicit module/CID without filesystem work or code loading.
    /// Duplicate CIDs in different modules never select a replacement. Unknown
    /// type tokens, stale/error rows and dynamic inventories require a fresh
    /// successful scan; original last-good records remain untouched.
    pub fn select_effect(&self, module: &Path, cid: [u8; 16]) -> Result<EffectSelection, String> {
        self.validate()?;
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.path == module)
            .ok_or("selected vst3 module is no longer in the catalog")?;
        if entry.failure.is_some() || self.source_errors.contains_key(module) {
            return Err("selected vst3 source is unavailable; rescan before insertion".into());
        }
        let report = entry
            .last_good
            .as_ref()
            .ok_or("selected vst3 module has no successful scan")?;
        let discovery = report
            .discovery
            .as_ref()
            .ok_or("selected vst3 classes are unavailable")?;
        if !discovery.cacheable() {
            return Err("selected vst3 inventory requires a rescan before insertion".into());
        }
        let class = discovery
            .classes
            .iter()
            .find(|class| class.cid == cid)
            .ok_or("selected vst3 class is no longer in the catalog")?;
        if class.category != "Audio Module Class"
            || !class
                .subcategories
                .as_ref()
                .is_some_and(|categories| categories.split('|').any(|token| token == "Fx"))
        {
            return Err("selected vst3 class is not a reported effect".into());
        }
        let executable = report
            .executable
            .as_ref()
            .ok_or("selected vst3 executable is unavailable")?;
        Ok(EffectSelection {
            module: entry.path.clone(),
            cid,
            class_name: class.name.clone(),
            executable_relative_path: executable.relative_path.clone(),
            executable_sha256: executable.sha256.clone(),
            info_sha256: report.bundle.info_sha256.clone(),
        })
    }

    /// Background control only: inspect saved sources without executing code or
    /// changing persistent metadata. Uses the existing per-file byte/path bounds,
    /// sequentially for at most MAX_MODULES; no filesystem work occurs in `rows`.
    /// A later successful refresh clears transient errors, but never a scan failure.
    /// This observation does not authorize loading a module after this call returns.
    pub fn refresh_sources(&mut self) -> Result<(), String> {
        self.validate()?;
        self.source_errors.clear();
        for entry in &self.entries {
            let Some(report) = &entry.last_good else {
                continue;
            };
            let result = match &report.executable {
                Some(executable) => report
                    .bundle
                    .executable_unchanged(executable)
                    .map_err(|e| e.to_string()),
                None => Err("catalog executable missing".into()),
            };
            if let Err(error) = result {
                self.source_errors
                    .insert(entry.path.clone(), error.chars().take(128).collect());
            }
        }
        Ok(())
    }

    /// Pure metadata projection; deliberately no filesystem work or code loading.
    pub fn rows(&self, query: &str) -> Vec<Row> {
        let query = query.to_lowercase();
        let mut rows = Vec::new();
        for entry in &self.entries {
            if entry.last_good.is_none() {
                let name = entry.path.file_name().map_or_else(
                    || "unavailable vst3 module".into(),
                    |name| name.to_string_lossy().into_owned(),
                );
                if query.is_empty() || name.to_lowercase().contains(&query) {
                    rows.push(Row {
                        module: entry.path.clone(),
                        cid: None,
                        name,
                        vendor: String::new(),
                        status: entry
                            .failure
                            .clone()
                            .unwrap_or_else(|| "vst3 unavailable".into()),
                    });
                }
            }
            if let Some(report) = &entry.last_good
                && let Some(discovery) = &report.discovery
            {
                for class in &discovery.classes {
                    if class.category != "Audio Module Class" {
                        continue;
                    }
                    let kind = reported_kind(class);
                    let vendor = class
                        .class_vendor
                        .as_deref()
                        .filter(|s| !s.is_empty())
                        .unwrap_or(&discovery.vendor);
                    if !query.is_empty()
                        && !class.name.to_lowercase().contains(&query)
                        && !vendor.to_lowercase().contains(&query)
                        && !kind.contains(&query)
                    {
                        continue;
                    }
                    rows.push(Row {
                        module: entry.path.clone(),
                        cid: Some(class.cid),
                        name: class.name.clone(),
                        vendor: vendor.into(),
                        status: {
                            let state = if let Some(error) = &entry.failure {
                                format!("last good · {error} · ")
                            } else if !discovery.cacheable() {
                                "prior discovery · rescan required · ".into()
                            } else {
                                String::new()
                            };
                            let source = self.source_errors.get(&entry.path).map_or_else(
                                String::new,
                                |error| {
                                    format!("source unavailable · rescan required · {error} · ")
                                },
                            );
                            let action = if matches!(kind, "effect" | "instrument and effect") {
                                "enter to insert · source validation required"
                            } else {
                                "hosting unavailable"
                            };
                            format!("{source}{state}vst3 {kind} · {action}")
                        },
                    });
                }
            }
        }
        rows.sort_by(|a, b| (&a.module, a.cid).cmp(&(&b.module, b.cid)));
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_plugin_scan::{
        Class,
        macos::{
            identity::{Bundle, Executable},
            macho::Image,
            report::{Cleanup, GroupOutcome},
        },
    };
    use std::{
        fs,
        io::Write,
        sync::atomic::{AtomicU64, Ordering},
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "hex-plugin-catalog-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).expect("owned fixture");
            Self(path.canonicalize().expect("physical temporary root"))
        }
        fn catalog(&self) -> PathBuf {
            self.0.join("catalog.json")
        }
        fn module(&self) -> PathBuf {
            self.0.join("Original.vst3")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    pub(crate) fn report(path: PathBuf) -> Report {
        Report {
            profile: "explicit-user-trusted-macos-factory-only".into(),
            request: 1,
            generation: 1,
            receipt_sha256: "a".repeat(64),
            helper_sha256: "b".repeat(64),
            bundle: Bundle {
                supplied_path: path.clone(),
                canonical_path: path,
                info_sha256: "c".repeat(64),
            },
            executable: Some(Executable {
                relative_path: "Contents/MacOS/Original".into(),
                sha256: "d".repeat(64),
                image: Image::ThinArm64,
            }),
            bundle_identifier: Some("org.hex.original".into()),
            discovery: Some(Discovery {
                request: 1,
                sdk: SDK.into(),
                public_sdk: PUBLIC_SDK.into(),
                platform: "darwin-arm64".into(),
                factory_flags: 0,
                vendor: "Original Vendor".into(),
                url: String::new(),
                email: String::new(),
                classes: vec![Class {
                    cid: [1; 16],
                    cardinality: 1,
                    category: "Audio Module Class".into(),
                    name: "Original Processor".into(),
                    class_flags: None,
                    subcategories: None,
                    class_vendor: None,
                    class_version: None,
                    class_sdk_version: None,
                }],
            }),
            interruption: None,
            error: None,
            stdout: "discard diagnostic bytes".into(),
            stderr: "discard diagnostic bytes".into(),
            cleanup: Cleanup {
                direct_reaped: true,
                direct_exit_code: Some(0),
                exit_observed_before_signals: true,
                original_group_term: Some(GroupOutcome::PermissionDenied { errno: 1 }),
                original_group_kill: Some(GroupOutcome::PermissionDenied { errno: 1 }),
                descendants: "unknown".into(),
                ..Cleanup::default()
            },
        }
    }
    fn effect_report(path: PathBuf, categories: &str) -> Report {
        let mut value = report(path);
        let class = &mut value.discovery.as_mut().unwrap().classes[0];
        class.class_flags = Some(0);
        class.subcategories = Some(categories.into());
        class.class_vendor = Some("Original Vendor".into());
        class.class_version = Some("1".into());
        class.class_sdk_version = Some("3.8".into());
        value
    }

    #[test]
    fn saved_effect_resolution_requires_exact_unique_cacheable_identity() {
        let f = Fixture::new();
        let first = f.module();
        let mut catalog = Catalog::default();
        catalog
            .record(
                first.clone(),
                Ok((effect_report(first.clone(), "Fx"), "e".repeat(40))),
            )
            .unwrap();
        let saved = serde_json::to_vec(&catalog).unwrap();
        assert_eq!(
            catalog
                .resolve_effect([1; 16], [0xdd; 32], [0xcc; 32])
                .unwrap()
                .module,
            first
        );
        for (cid, executable, info) in [
            ([2; 16], [0xdd; 32], [0xcc; 32]),
            ([1; 16], [0xee; 32], [0xcc; 32]),
            ([1; 16], [0xdd; 32], [0xee; 32]),
        ] {
            assert!(catalog.resolve_effect(cid, executable, info).is_err());
        }
        assert_eq!(serde_json::to_vec(&catalog).unwrap(), saved);
        let second = f.0.join("Second.vst3");
        catalog
            .record(
                second.clone(),
                Ok((effect_report(second, "Fx"), "e".repeat(40))),
            )
            .unwrap();
        assert!(
            catalog
                .resolve_effect([1; 16], [0xdd; 32], [0xcc; 32])
                .unwrap_err()
                .contains("ambiguous")
        );
        catalog.entries.pop();
        catalog.entries[0].failure = Some("last scan failed".into());
        assert!(
            catalog
                .resolve_effect([1; 16], [0xdd; 32], [0xcc; 32])
                .is_err()
        );
        catalog.entries[0].failure = None;
        catalog.entries[0]
            .last_good
            .as_mut()
            .unwrap()
            .discovery
            .as_mut()
            .unwrap()
            .factory_flags = 8;
        assert!(
            catalog
                .resolve_effect([1; 16], [0xdd; 32], [0xcc; 32])
                .is_err()
        );
    }

    #[test]
    fn effect_selection_captures_only_the_explicit_module_and_class() {
        let f = Fixture::new();
        let first = f.module();
        let second = f.0.join("Second.vst3");
        let mut catalog = Catalog::default();
        catalog
            .record(
                first.clone(),
                Ok((effect_report(first.clone(), "Fx|Delay"), "e".repeat(40))),
            )
            .unwrap();
        let mut other = effect_report(second.clone(), "Fx");
        other.executable.as_mut().unwrap().sha256 = "f".repeat(64);
        catalog
            .record(second.clone(), Ok((other, "e".repeat(40))))
            .unwrap();
        let saved = serde_json::to_vec(&catalog).unwrap();
        let selected = catalog.select_effect(&second, [1; 16]).unwrap();
        assert_eq!(selected.module, second);
        assert_eq!(selected.cid, [1; 16]);
        assert_eq!(selected.class_name, "Original Processor");
        assert_eq!(selected.executable_relative_path, "Contents/MacOS/Original");
        assert_eq!(selected.executable_sha256, "f".repeat(64));
        assert_eq!(selected.info_sha256, "c".repeat(64));
        assert_eq!(
            catalog
                .select_effect(&first, [1; 16])
                .unwrap()
                .executable_sha256,
            "d".repeat(64)
        );
        assert!(catalog.select_effect(&first, [2; 16]).is_err());
        assert!(
            catalog
                .select_effect(&f.0.join("Missing.vst3"), [1; 16])
                .is_err()
        );
        assert_eq!(serde_json::to_vec(&catalog).unwrap(), saved);
        // Invalid duplicate paths/classes cannot turn a selection into an
        // arbitrary first-match lookup, even with manually supplied metadata.
        catalog.entries.push(catalog.entries[0].clone());
        assert!(catalog.select_effect(&first, [1; 16]).is_err());
    }

    #[test]
    fn effect_selection_requires_exact_reported_effect_and_cacheable_inventory() {
        let f = Fixture::new();
        let path = f.module();
        for (categories, accepted) in [
            ("Fx", true),
            ("Fx|Instrument", true),
            ("Instrument|Synth", false),
            ("MyFx", false),
            ("fx", false),
            ("", false),
        ] {
            let mut catalog = Catalog::default();
            catalog
                .record(
                    path.clone(),
                    Ok((effect_report(path.clone(), categories), "e".repeat(40))),
                )
                .unwrap();
            assert_eq!(
                catalog.select_effect(&path, [1; 16]).is_ok(),
                accepted,
                "{categories}"
            );
        }
        let mut catalog = Catalog::default();
        let mut dynamic = effect_report(path.clone(), "Fx");
        dynamic.discovery.as_mut().unwrap().factory_flags = 1;
        catalog
            .record(path.clone(), Ok((dynamic, "e".repeat(40))))
            .unwrap();
        assert!(catalog.select_effect(&path, [1; 16]).is_err());
    }

    #[test]
    fn effect_selection_refuses_unavailable_or_last_good_error_without_rewriting_it() {
        let f = Fixture::new();
        let path = f.module();
        let mut catalog = Catalog::default();
        catalog
            .record(
                path.clone(),
                Ok((effect_report(path.clone(), "Fx"), "e".repeat(40))),
            )
            .unwrap();
        catalog
            .source_errors
            .insert(path.clone(), "changed executable".into());
        let saved = serde_json::to_vec(&catalog).unwrap();
        assert!(catalog.select_effect(&path, [1; 16]).is_err());
        assert_eq!(serde_json::to_vec(&catalog).unwrap(), saved);
        catalog.source_errors.clear();
        catalog
            .record(path.clone(), Err("rescan failed".into()))
            .unwrap();
        let saved = serde_json::to_vec(&catalog).unwrap();
        assert!(catalog.select_effect(&path, [1; 16]).is_err());
        assert!(catalog.entries[0].last_good.is_some());
        assert_eq!(serde_json::to_vec(&catalog).unwrap(), saved);
    }

    #[test]
    fn source_refresh_detects_changed_and_moved_duplicates_without_rewriting_last_good() {
        let f = Fixture::new();
        // Structural Mach-O bytes only; this test never creates executable code.
        let mut bytes: Vec<u8> = [0xfeed_facf_u32, 0x0100_000c, 0, 8, 1, 8, 0, 0, 0, 8]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect();
        bytes.push(0);
        let first = f.0.join("A.vst3");
        let second = f.0.join("B.vst3");
        for path in [&first, &second] {
            fs::create_dir_all(path.join("Contents/MacOS")).unwrap();
            fs::write(path.join("Contents/Info.plist"), b"original metadata").unwrap();
            fs::write(path.join("Contents/MacOS/Original"), &bytes).unwrap();
        }
        let snapshot = |path: &Path| {
            let mut value = report(path.to_path_buf());
            value.bundle = Bundle::inspect(path).unwrap();
            value.executable = Some(value.bundle.admit("Contents/MacOS/Original").unwrap());
            value
        };
        storage::update(&f.catalog(), |catalog| {
            for path in [&second, &first] {
                catalog.record(path.clone(), Ok((snapshot(path), "e".repeat(40))))?;
            }
            // A valid input's array order cannot determine duplicate selection.
            catalog.entries.reverse();
            Ok(())
        })
        .unwrap();
        let saved = fs::read(f.catalog()).unwrap();
        let mut catalog = Catalog::load(&f.catalog()).unwrap();
        let original = serde_json::to_vec(&catalog).unwrap();
        catalog.refresh_sources().unwrap();
        let rows = catalog.rows("");
        assert_eq!(
            rows.iter().map(|r| &r.module).collect::<Vec<_>>(),
            vec![&first, &second]
        );
        assert_eq!(rows[0].cid, rows[1].cid);
        assert!(
            rows.iter()
                .all(|r| !r.status.contains("source unavailable"))
        );

        let executable = first.join("Contents/MacOS/Original");
        let mut changed = bytes.clone();
        *changed.last_mut().unwrap() = 1; // Same size, same valid header, different hash.
        fs::write(&executable, changed).unwrap();
        catalog.refresh_sources().unwrap();
        let rows = catalog.rows("");
        assert!(rows[0].status.contains("selected executable changed"));
        assert!(rows[0].status.contains("rescan required"));
        assert!(!rows[1].status.contains("source unavailable"));
        assert_eq!(
            serde_json::to_vec(&catalog).unwrap(),
            original,
            "freshness is not persistent scan state"
        );

        let moved = f.0.join("C.vst3");
        fs::rename(&second, &moved).unwrap();
        catalog.refresh_sources().unwrap();
        assert!(catalog.rows("")[1].status.contains("source unavailable"));
        assert_eq!(
            fs::read(f.catalog()).unwrap(),
            saved,
            "no deletion or implicit rescan on move"
        );
        catalog
            .record(moved.clone(), Ok((snapshot(&moved), "e".repeat(40))))
            .unwrap();
        let rows = catalog.rows("");
        assert_eq!(rows.len(), 3, "historical identity stays recoverable");
        assert_eq!(rows[1].module, second);
        assert_eq!(rows[2].module, moved);
        assert_eq!(rows[1].cid, rows[2].cid);
        assert!(rows[1].status.contains("source unavailable"));
        assert!(!rows[2].status.contains("source unavailable"));

        fs::write(executable, &bytes).unwrap();
        catalog
            .record(first.clone(), Err("explicit rescan refused".into()))
            .unwrap();
        catalog.refresh_sources().unwrap();
        let row = &catalog.rows("")[0];
        assert!(
            !row.status.contains("source unavailable"),
            "freshness recovers when bytes return"
        );
        assert!(
            row.status.contains("last good · explicit rescan refused"),
            "source restoration cannot erase scan failure"
        );
    }

    #[test]
    fn reported_types_use_exact_sdk_tokens_and_survive_restart_without_guesses() {
        let fixture = Fixture::new();
        let path = fixture.module();
        for (categories, expected) in [
            ("Instrument|Synth", "instrument"),
            ("Fx|Delay", "effect"),
            ("Fx|Instrument", "instrument and effect"),
            ("MyInstrument|CustomFx", "processor · type unknown"),
            ("instrument|fx", "processor · type unknown"),
        ] {
            let mut value = report(path.clone());
            let class = &mut value.discovery.as_mut().unwrap().classes[0];
            class.class_flags = Some(0x8000_0000);
            class.subcategories = Some(categories.into());
            class.class_vendor = Some("Class Vendor".into());
            class.class_version = Some("1.2.3".into());
            class.class_sdk_version = Some("VST 3.8".into());
            storage::update(&fixture.catalog(), |catalog| {
                catalog.record(path.clone(), Ok((value, "e".repeat(40))))
            })
            .unwrap();
            let loaded = Catalog::load(&fixture.catalog()).unwrap();
            assert_eq!(
                loaded.rows("")[0].status,
                if matches!(categories, "Fx|Delay" | "Fx|Instrument") {
                    format!("vst3 {expected} · enter to insert · source validation required")
                } else {
                    format!("vst3 {expected} · hosting unavailable")
                }
            );
            assert_eq!(loaded.rows("CLASS VENDOR")[0].vendor, "Class Vendor");
            assert_eq!(loaded.rows(expected).len(), 1);
            assert_eq!(
                loaded.rows("instrument").len(),
                usize::from(expected.contains("instrument"))
            );
        }
        let mut value = report(path.clone());
        value.discovery.as_mut().unwrap().classes[0].subcategories = Some("Instrument".into());
        assert!(
            validate_report(&value).is_err(),
            "partial optional metadata cannot classify"
        );
        let old = Catalog {
            version: 1,
            ..Catalog::default()
        };
        let file = fixture.catalog();
        let bytes = serde_json::to_vec(&old).unwrap();
        fs::write(&file, &bytes).unwrap();
        assert!(Catalog::load(&file).is_err());
        assert_eq!(
            fs::read(file).unwrap(),
            bytes,
            "unsupported alpha catalog remains intact"
        );
    }

    #[test]
    fn representative_standard_library_survives_save_reload_and_search_within_fixed_byte_bound() {
        let fixture = Fixture::new();
        storage::update(&fixture.catalog(), |catalog| {
            for n in 0..320 {
                let path = fixture.0.join(format!("Original{n}.vst3"));
                let mut value = report(path.clone());
                let discovery = value.discovery.as_mut().unwrap();
                let mut controller = discovery.classes[0].clone();
                controller.cid[15] = 2;
                controller.category = "Component Controller Class".into();
                discovery.classes.push(controller);
                catalog.record(path, Ok((value, "e".repeat(40))))?;
            }
            Ok(())
        })
        .unwrap();
        let bytes = fs::read(fixture.catalog()).unwrap();
        assert!(bytes.len() <= MAX_CATALOG_BYTES);
        let saved = Catalog::load(&fixture.catalog()).unwrap();
        assert_eq!(saved.entries.len(), 320);
        assert_eq!(saved.rows("vendor").len(), 320);
        assert_eq!(fs::read(fixture.catalog()).unwrap(), bytes);
    }

    #[test]
    fn restart_restores_actual_identity_search_and_failure_without_execution() {
        let fixture = Fixture::new();
        let path = fixture.module();
        let expected = report(path.clone());
        storage::update(&fixture.catalog(), |c| {
            c.record(path.clone(), Ok((expected.clone(), "e".repeat(40))))
        })
        .expect("publish");
        let loaded = Catalog::load(&fixture.catalog()).expect("reopen");
        let rows = loaded.rows("VENDOR");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].cid, Some([1; 16]));
        assert_eq!(rows[0].name, "Original Processor");
        assert!(
            loaded.entries[0]
                .last_good
                .as_ref()
                .unwrap()
                .stderr
                .is_empty()
        );
        storage::update(&fixture.catalog(), |c| {
            c.record(
                path.clone(),
                Err("scan cancelled; last good retained".into()),
            )
        })
        .expect("record failure");
        let failed = Catalog::load(&fixture.catalog()).expect("failure restart");
        assert_eq!(failed.rows("")[0].cid, Some([1; 16]));
        assert!(failed.rows("")[0].status.contains("cancelled"));
        assert_eq!(
            failed.entries[0].last_good.as_ref().unwrap().executable,
            expected.executable
        );
        let absent = fixture.0.join("Missing.vst3");
        storage::update(&fixture.catalog(), |c| {
            c.record(absent, Err("bundle missing".into()))
        })
        .expect("missing record");
        assert_eq!(
            Catalog::load(&fixture.catalog()).unwrap().rows("Missing")[0].cid,
            None
        );
    }
    #[test]
    fn incomplete_and_duplicate_class_reports_cannot_replace_last_good() {
        let fixture = Fixture::new();
        let path = fixture.module();
        let mut catalog = Catalog::default();
        catalog
            .record(path.clone(), Ok((report(path.clone()), "e".repeat(40))))
            .unwrap();
        let original = serde_json::to_vec(&catalog).unwrap();
        for mode in 0..4 {
            let mut bad = report(path.clone());
            match mode {
                0 => bad.cleanup.pending = true,
                1 => bad.cleanup.custody_lost = true,
                2 => {
                    let duplicate = bad.discovery.as_ref().unwrap().classes[0].clone();
                    bad.discovery.as_mut().unwrap().classes.push(duplicate);
                }
                _ => bad.bundle.supplied_path = fixture.0.join("Other.vst3"),
            };
            assert!(
                catalog
                    .record(path.clone(), Ok((bad, "e".repeat(40))))
                    .is_err()
            );
            assert_eq!(serde_json::to_vec(&catalog).unwrap(), original);
        }
    }
    #[test]
    fn invalid_existing_catalog_and_failed_save_preserve_prior_bytes() {
        let fixture = Fixture::new();
        let file = fixture.catalog();
        fs::write(&file, b"incomplete").unwrap();
        let mut called = false;
        assert!(
            storage::update(&file, |_| {
                called = true;
                Ok(())
            })
            .is_err()
        );
        assert!(!called);
        assert_eq!(fs::read(&file).unwrap(), b"incomplete");
        fs::remove_file(&file).unwrap();
        storage::update(&file, |c| {
            c.record(fixture.module(), Err("first failure".into()))
        })
        .unwrap();
        let before = fs::read(&file).unwrap();
        fs::remove_file(file.with_extension("lock")).unwrap();
        fs::create_dir(file.with_extension("lock")).unwrap();
        assert!(
            storage::update(&file, |c| c
                .record(fixture.module(), Err("new failure".into())))
            .is_err()
        );
        assert_eq!(fs::read(&file).unwrap(), before);
        let mut partial =
            fs::File::create(fixture.0.join(".catalog.json.interrupted.pending")).unwrap();
        partial.write_all(b"{broken").unwrap();
        assert_eq!(
            Catalog::load(&file).unwrap().entries[0].failure.as_deref(),
            Some("first failure")
        );
    }
    #[test]
    fn concurrent_writer_refuses_without_mutating_or_blocking_readers() {
        let fixture = Fixture::new();
        let file = fixture.catalog();
        storage::update(&file, |c| c.record(fixture.module(), Err("old".into()))).unwrap();
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(file.with_extension("lock"))
            .unwrap();
        fs2::FileExt::try_lock_exclusive(&lock).unwrap();
        let mut entered = false;
        assert!(
            storage::update(&file, |_| {
                entered = true;
                Ok(())
            })
            .is_err()
        );
        assert!(!entered);
        assert_eq!(
            Catalog::load(&file).unwrap().entries[0].failure.as_deref(),
            Some("old")
        );
    }
    #[test]
    fn dynamic_discardable_and_unknown_factory_inventory_stays_historical_after_restart() {
        let fixture = Fixture::new();
        let path = fixture.module();
        for flags in [1, 8, 0x8000_0000] {
            let mut value = report(path.clone());
            value.discovery.as_mut().unwrap().factory_flags = flags;
            storage::update(&fixture.catalog(), |catalog| {
                catalog.record(path.clone(), Ok((value, "e".repeat(40))))
            })
            .unwrap();
            let loaded = Catalog::load(&fixture.catalog()).unwrap();
            assert!(loaded.rows("")[0].status.contains("rescan required"));
            assert_eq!(
                loaded.entries[0]
                    .last_good
                    .as_ref()
                    .unwrap()
                    .discovery
                    .as_ref()
                    .unwrap()
                    .factory_flags,
                flags
            );
        }
    }

    fn failed_identity(path: &Path, info: char) -> Bundle {
        Bundle {
            supplied_path: path.to_path_buf(),
            canonical_path: path.to_path_buf(),
            info_sha256: info.to_string().repeat(64),
        }
    }

    #[test]
    fn version_2_catalog_migrates_in_memory_and_rewrites_only_on_update() {
        let fixture = Fixture::new();
        let good = fixture.0.join("Good.vst3");
        let bad = fixture.0.join("Bad.vst3");
        let mut value = report(good.clone());
        value.stdout.clear();
        value.stderr.clear();
        // Historical version 2 layout: entries carry no failed_bundle field.
        let v2 = serde_json::json!({
            "version": 2,
            "entries": [
                {"path": bad, "last_good": null, "source": null, "failure": "scan failed"},
                {"path": good, "last_good": value, "source": "e".repeat(40), "failure": null},
            ],
        });
        let bytes = serde_json::to_vec(&v2).unwrap();
        fs::write(fixture.catalog(), &bytes).unwrap();
        let loaded = Catalog::load(&fixture.catalog()).expect("migrated v2");
        assert_eq!(loaded.version, CATALOG_VERSION);
        assert_eq!(loaded.entries.len(), 2);
        assert!(loaded.entries.iter().all(|e| e.failed_bundle.is_none()));
        assert_eq!(loaded.entries[0].failure.as_deref(), Some("scan failed"));
        assert_eq!(
            loaded
                .entries
                .iter()
                .filter(|e| e.last_good.is_some())
                .count(),
            1
        );
        assert_eq!(
            fs::read(fixture.catalog()).unwrap(),
            bytes,
            "load never rewrites"
        );

        storage::update(&fixture.catalog(), |_| Ok(())).unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&fs::read(fixture.catalog()).unwrap()).unwrap();
        assert_eq!(saved["version"], 3);
        let reopened = Catalog::load(&fixture.catalog()).unwrap();
        assert_eq!(reopened.entries.len(), 2);

        // A version 2 file cannot carry the version 3 field; newer formats are refused.
        for (version, failed) in [(2, true), (4, false)] {
            let mut entry = serde_json::json!(
                {"path": bad, "last_good": null, "source": null, "failure": "scan failed"}
            );
            if failed {
                entry["failed_bundle"] = serde_json::to_value(failed_identity(&bad, 'c')).unwrap();
            }
            let bytes =
                serde_json::to_vec(&serde_json::json!({"version": version, "entries": [entry]}))
                    .unwrap();
            fs::write(fixture.catalog(), &bytes).unwrap();
            assert!(
                Catalog::load(&fixture.catalog()).is_err(),
                "version {version}"
            );
            assert_eq!(
                fs::read(fixture.catalog()).unwrap(),
                bytes,
                "file preserved"
            );
        }
    }

    #[test]
    fn failed_bundle_identity_round_trips_and_matches_only_unchanged_bundles() {
        let fixture = Fixture::new();
        let path = fixture.module();
        let identity = failed_identity(&path, 'c');
        storage::update(&fixture.catalog(), |catalog| {
            catalog.record_failure(
                path.clone(),
                "deadline exceeded".into(),
                Some(identity.clone()),
            )
        })
        .unwrap();
        let saved = Catalog::load(&fixture.catalog()).unwrap();
        assert_eq!(saved.entries[0].failed_bundle.as_ref(), Some(&identity));
        assert!(saved.failed_unchanged(&path, &identity));
        assert!(
            !saved.failed_unchanged(&path, &failed_identity(&path, 'f')),
            "changed plist"
        );
        let other = fixture.0.join("Other.vst3");
        assert!(!saved.failed_unchanged(&other, &failed_identity(&other, 'c')));

        // A failure without identity, or a later success, is not remembered.
        let mut catalog = saved.clone();
        catalog.record(path.clone(), Err("again".into())).unwrap();
        assert!(!catalog.failed_unchanged(&path, &identity));
        let mut catalog = saved;
        catalog
            .record(path.clone(), Ok((report(path.clone()), "e".repeat(40))))
            .unwrap();
        assert!(catalog.entries[0].failed_bundle.is_none());
        assert!(!catalog.failed_unchanged(&path, &identity));

        // The identity must belong to a recorded failure for this exact path.
        let mut invalid = Catalog::default();
        invalid
            .record(path.clone(), Ok((report(path.clone()), "e".repeat(40))))
            .unwrap();
        invalid.validate().expect("valid success entry");
        invalid.entries[0].failed_bundle = Some(identity.clone());
        assert!(invalid.validate().is_err(), "identity without failure");
        assert!(
            Catalog::default()
                .record_failure(path, "x".into(), Some(failed_identity(&other, 'c')))
                .is_err(),
            "identity for another path"
        );
    }
}
