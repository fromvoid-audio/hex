//! Selected-plugin browser records and control work. Never called by audio.
pub mod storage;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod worker;

use hex_plugin_scan::{Discovery, PUBLIC_SDK, SDK, macos::report::Report};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::{Component, Path, PathBuf},
};

pub const MAX_MODULES: usize = 64;
pub const MAX_CLASSES: usize = 2048;
pub const MAX_CATALOG_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub path: PathBuf,
    pub last_good: Option<Report>,
    pub source: Option<String>,
    pub failure: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    version: u32,
    pub entries: Vec<Entry>,
}
impl Default for Catalog {
    fn default() -> Self {
        Self {
            version: 2,
            entries: Vec::new(),
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
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 2 || self.entries.len() > MAX_MODULES {
            return Err("unsupported or oversized plugin catalog".into());
        }
        let mut paths = BTreeSet::new();
        let mut classes = 0_usize;
        for entry in &self.entries {
            if !path_valid(&entry.path)
                || entry.path.extension().and_then(|s| s.to_str()) != Some("vst3")
                || !paths.insert(&entry.path)
                || entry.failure.as_ref().is_some_and(|s| !bounded(s, 512))
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

    pub fn record(
        &mut self,
        path: PathBuf,
        result: Result<(Report, String), String>,
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
            }
            Err(error) => {
                entry.failure = Some(error.chars().take(128).collect());
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
            if let Some(report) = &entry.last_good {
                if let Some(discovery) = &report.discovery {
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
                                format!("{state}vst3 {kind} · hosting unavailable")
                            },
                        });
                    }
                }
            }
        }
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
    fn report(path: PathBuf) -> Report {
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
                format!("vst3 {expected} · hosting unavailable")
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
}
