//! Recoverable plugin catalog data model, schema migration, and entry mutations.
use super::*;

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
    pub failed_bundle: Option<Bundle>,
    /// Absent in older catalogs, so the old failure is eligible for retry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_tools_sha256: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClapEntry {
    pub path: PathBuf,
    pub last_good: Option<ClapBundleIdentity>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub descriptors: Vec<ClapDescriptor>,
    pub failure: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct FailedScan {
    pub(crate) bundle: Bundle,
    pub(crate) tools_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub(crate) version: u32,
    pub entries: Vec<Entry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clap_entries: Vec<ClapEntry>,
    #[serde(skip)]
    pub(crate) source_errors: BTreeMap<PathBuf, String>,
}
impl Default for Catalog {
    fn default() -> Self {
        Self {
            version: CATALOG_VERSION,
            entries: Vec::new(),
            clap_entries: Vec::new(),
            source_errors: BTreeMap::new(),
        }
    }
}

impl Catalog {
    /// Upgrade supported older formats in memory, retaining their diagnostics.
    pub(crate) fn migrate(&mut self) -> Result<(), String> {
        if matches!(
            self.version,
            CATALOG_VERSION_2 | CATALOG_VERSION_3 | CATALOG_VERSION_4
        ) {
            if !self.clap_entries.is_empty()
                || self.entries.iter().any(|entry| {
                    (self.version != CATALOG_VERSION_4 && entry.failed_tools_sha256.is_some())
                        || (self.version == CATALOG_VERSION_2 && entry.failed_bundle.is_some())
                })
            {
                return Err("invalid historical plugin catalog; existing file preserved".into());
            }
            self.version = CATALOG_VERSION;
        }
        Ok(())
    }

    /// A remembered failure applies only to the same bundle and scan tools.
    #[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
    pub(crate) fn failed_unchanged(
        &self,
        path: &Path,
        current: &Bundle,
        tools_sha256: &str,
    ) -> bool {
        self.entries.iter().any(|entry| {
            entry.path == path
                && entry.failed_bundle.as_ref() == Some(current)
                && entry.failed_tools_sha256.as_deref() == Some(tools_sha256)
        })
    }

    /// Record a CLAP bundle scan result (static bundle identity + optional descriptors).
    pub fn record_clap(
        &mut self,
        path: PathBuf,
        result: Result<(ClapBundleIdentity, Vec<ClapDescriptor>), String>,
    ) -> Result<(), String> {
        if !path_valid(&path) || path.extension().and_then(|s| s.to_str()) != Some("clap") {
            return Err("absolute bounded .clap plugin path required".into());
        }
        let mut next = self.clone();
        let index = match next.clap_entries.iter().position(|e| e.path == path) {
            Some(index) => index,
            None => {
                if next.entries.len().saturating_add(next.clap_entries.len()) >= MAX_MODULES {
                    return Err("plugin catalog module capacity reached".into());
                }
                next.clap_entries.push(ClapEntry {
                    path: path.clone(),
                    last_good: None,
                    descriptors: Vec::new(),
                    failure: None,
                });
                next.clap_entries.len() - 1
            }
        };
        let entry = &mut next.clap_entries[index];
        match result {
            Ok((identity, descriptors)) => {
                if identity.supplied_path != path {
                    return Err("clap plugin result path mismatch".into());
                }
                entry.last_good = Some(identity);
                entry.descriptors = descriptors;
                entry.failure = None;
                next.source_errors.remove(&path);
            }
            Err(error) => {
                entry.failure = Some(sanitize_failure(&error));
            }
        }
        next.clap_entries.sort_by(|a, b| a.path.cmp(&b.path));
        next.validate()?;
        *self = next;
        Ok(())
    }

    /// Record a scan result. A failure recorded here has no bundle identity and
    /// is rescanned by the next folder scan.
    pub fn record(
        &mut self,
        path: PathBuf,
        result: Result<(Report, String), String>,
    ) -> Result<(), String> {
        self.record_outcome(path, result, None)
    }

    /// Record a failed scan with its observed bundle and discovery tools.
    #[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
    pub(crate) fn record_failure(
        &mut self,
        path: PathBuf,
        error: String,
        scan: Option<FailedScan>,
    ) -> Result<(), String> {
        self.record_outcome(path, Err(error), scan)
    }

    fn record_outcome(
        &mut self,
        path: PathBuf,
        result: Result<(Report, String), String>,
        failed_scan: Option<FailedScan>,
    ) -> Result<(), String> {
        if !path_valid(&path) || path.extension().and_then(|s| s.to_str()) != Some("vst3") {
            return Err("absolute bounded plugin path required".into());
        }
        let mut next = self.clone();
        let index = match next.entries.iter().position(|e| e.path == path) {
            Some(index) => index,
            None => {
                if next.entries.len().saturating_add(next.clap_entries.len()) >= MAX_MODULES {
                    return Err("plugin catalog module capacity reached".into());
                }
                next.entries.push(Entry {
                    path: path.clone(),
                    last_good: None,
                    source: None,
                    failure: None,
                    failed_bundle: None,
                    failed_tools_sha256: None,
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
                entry.failed_tools_sha256 = None;
                next.source_errors.remove(&path);
            }
            Err(error) => {
                entry.failure = Some(sanitize_failure(&error));
                let (bundle, tools) = failed_scan.map_or((None, None), |scan| {
                    (Some(scan.bundle), Some(scan.tools_sha256))
                });
                entry.failed_bundle = bundle;
                entry.failed_tools_sha256 = tools;
            }
        }
        next.entries.sort_by(|a, b| a.path.cmp(&b.path));
        next.validate()?;
        *self = next;
        Ok(())
    }

    /// Incremental in-place VST3 record update for `scan_batch`, avoiding
    /// per-bundle full-catalog clones while preserving atomic entry validation
    /// and returning `(previous_entry_json_bytes, new_entry_json_bytes)`.
    #[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
    pub(crate) fn record_outcome_incremental(
        &mut self,
        path: PathBuf,
        result: Result<(Report, String), String>,
        failed_scan: Option<FailedScan>,
    ) -> Result<(usize, usize), String> {
        if !path_valid(&path) || path.extension().and_then(|s| s.to_str()) != Some("vst3") {
            return Err("absolute bounded plugin path required".into());
        }
        let existing_index = self.entries.iter().position(|e| e.path == path);
        if existing_index.is_none()
            && self.entries.len().saturating_add(self.clap_entries.len()) >= MAX_MODULES
        {
            return Err("plugin catalog module capacity reached".into());
        }
        let prev_entry = existing_index.map(|i| &self.entries[i]);
        let prev_classes = prev_entry
            .and_then(|e| e.last_good.as_ref())
            .and_then(|r| r.discovery.as_ref())
            .map_or(0, |d| d.classes.len());
        let prev_bytes = match prev_entry {
            Some(entry) => {
                serde_json::to_vec(entry)
                    .map_err(|e| format!("catalog staging failed: {e}"))?
                    .len()
                    + 1
            }
            None => 0,
        };
        let mut candidate = prev_entry.cloned().unwrap_or(Entry {
            path: path.clone(),
            last_good: None,
            source: None,
            failure: None,
            failed_bundle: None,
            failed_tools_sha256: None,
        });
        match result {
            Ok((mut report, source)) => {
                validate_report(&report)?;
                if report.bundle.supplied_path != path || !digest(&source, 40) {
                    return Err("plugin result source/path mismatch".into());
                }
                report.stdout.clear();
                report.stderr.clear();
                candidate.last_good = Some(report);
                candidate.source = Some(source);
                candidate.failure = None;
                candidate.failed_bundle = None;
                candidate.failed_tools_sha256 = None;
            }
            Err(error) => {
                candidate.failure = Some(sanitize_failure(&error));
                let (bundle, tools) = failed_scan.map_or((None, None), |scan| {
                    (Some(scan.bundle), Some(scan.tools_sha256))
                });
                if let Some(ref b) = bundle
                    && (b.supplied_path != path
                        || !path_valid(&b.canonical_path)
                        || !digest(&b.info_sha256, 64))
                {
                    return Err("invalid plugin catalog entry".into());
                }
                if let Some(ref t) = tools
                    && (bundle.is_none() || !digest(t, 64))
                {
                    return Err("invalid plugin catalog entry".into());
                }
                candidate.failed_bundle = bundle;
                candidate.failed_tools_sha256 = tools;
            }
        }
        let new_classes = candidate
            .last_good
            .as_ref()
            .and_then(|r| r.discovery.as_ref())
            .map_or(0, |d| d.classes.len());
        let current_classes: usize = self
            .entries
            .iter()
            .filter_map(|e| e.last_good.as_ref()?.discovery.as_ref())
            .map(|d| d.classes.len())
            .sum::<usize>()
            + self
                .clap_entries
                .iter()
                .filter(|e| e.last_good.is_some())
                .map(|e| e.descriptors.len().max(1))
                .sum::<usize>();
        let next_classes = current_classes
            .saturating_sub(prev_classes)
            .checked_add(new_classes)
            .ok_or("catalog count overflow")?;
        if next_classes > MAX_CLASSES {
            return Err("plugin catalog class bound exceeded".into());
        }
        let new_bytes = serde_json::to_vec(&candidate)
            .map_err(|e| format!("catalog staging failed: {e}"))?
            .len()
            + 1;
        if candidate.failure.is_none() {
            self.source_errors.remove(&path);
        }
        match existing_index {
            Some(index) => self.entries[index] = candidate,
            None => {
                let insert_at = self
                    .entries
                    .partition_point(|entry| entry.path < candidate.path);
                self.entries.insert(insert_at, candidate);
            }
        }
        Ok((prev_bytes, new_bytes))
    }
}
