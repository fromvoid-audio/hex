//! Read-only browser row projection and explicit plugin selection queries.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub module: PathBuf,
    pub cid: Option<[u8; 16]>,
    pub name: String,
    pub vendor: String,
    pub status: String,
    /// Current metadata for a draggable class. Native admission is separate.
    pub selection: Option<Arc<PluginSelection>>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginRole {
    Effect,
    Instrument,
}

impl PluginRole {
    fn reported_by(self, class: &hex_plugin_scan::Class) -> bool {
        let token = match self {
            Self::Effect => "Fx",
            Self::Instrument => "Instrument",
        };
        class
            .subcategories
            .as_deref()
            .is_some_and(|categories| categories.split('|').any(|category| category == token))
    }
}

/// Exact copied metadata for an explicit plug-in activation request. This is
/// neither source admission nor permission to reuse a cached executable: the
/// processing owner must inspect the selected module again, compare both
/// digests/class and retain its actual native preparation through the terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginSelection {
    pub role: PluginRole,
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
    pub fn select_effect(&self, module: &Path, cid: [u8; 16]) -> Result<PluginSelection, String> {
        self.select_with_role(module, cid, Some(PluginRole::Effect))
    }

    pub fn select_plugin(&self, module: &Path, cid: [u8; 16]) -> Result<PluginSelection, String> {
        self.select_with_role(module, cid, None)
    }

    pub(crate) fn select_with_role(
        &self,
        module: &Path,
        cid: [u8; 16],
        role: Option<PluginRole>,
    ) -> Result<PluginSelection, String> {
        self.validate()?;
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.path == module)
            .ok_or("selected vst3 module is no longer in the catalog")?;
        let (report, discovery) = self.plugin_inventory(entry)?;
        let class = discovery
            .classes
            .iter()
            .find(|class| class.cid == cid)
            .ok_or("selected vst3 class is no longer in the catalog")?;
        Self::plugin_selection(entry, report, class, role)
    }

    fn plugin_inventory<'a>(
        &self,
        entry: &'a Entry,
    ) -> Result<(&'a Report, &'a Discovery), String> {
        if entry.failure.is_some() || self.source_errors.contains_key(&entry.path) {
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
        Ok((report, discovery))
    }

    fn plugin_selection(
        entry: &Entry,
        report: &Report,
        class: &hex_plugin_scan::Class,
        role: Option<PluginRole>,
    ) -> Result<PluginSelection, String> {
        // Preserve the existing effect choice for dual-role classes.
        let role = role.or_else(|| {
            [PluginRole::Effect, PluginRole::Instrument]
                .into_iter()
                .find(|role| role.reported_by(class))
        });
        let Some(role) =
            role.filter(|role| class.category == "Audio Module Class" && role.reported_by(class))
        else {
            return Err("selected vst3 class does not report the requested role".into());
        };
        let executable = report
            .executable
            .as_ref()
            .ok_or("selected vst3 executable is unavailable")?;
        Ok(PluginSelection {
            role,
            module: entry.path.clone(),
            cid: class.cid,
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
        for entry in &self.clap_entries {
            let Some(identity) = &entry.last_good else {
                continue;
            };
            if let Err(error) = identity.unchanged() {
                self.source_errors
                    .insert(entry.path.clone(), error.chars().take(128).collect());
            }
        }
        Ok(())
    }

    /// Pure metadata projection for one specific external plugin format.
    pub fn rows_for(&self, format: PluginFormat, query: &str) -> Vec<Row> {
        match format {
            PluginFormat::Vst3 => self.vst3_rows(query),
            PluginFormat::Clap => self.clap_rows(query),
        }
    }

    /// Pure metadata projection across all cataloged formats (VST3 and CLAP).
    pub fn rows(&self, query: &str) -> Vec<Row> {
        let mut rows = self.vst3_rows(query);
        if !self.clap_entries.is_empty() {
            rows.extend(self.clap_rows(query));
            rows.sort_by(|a, b| (&a.module, a.cid).cmp(&(&b.module, b.cid)));
        }
        rows
    }

    /// Pure metadata projection for discovered CLAP bundles.
    pub fn clap_rows(&self, query: &str) -> Vec<Row> {
        let query = query.to_lowercase();
        let mut rows = Vec::new();
        for entry in &self.clap_entries {
            let source = self
                .source_errors
                .get(&entry.path)
                .map_or_else(String::new, |error| {
                    format!("source unavailable · rescan required · {error} · ")
                });
            let Some(identity) = &entry.last_good else {
                let name = entry.path.file_stem().map_or_else(
                    || "unavailable clap plug-in".into(),
                    |name| name.to_string_lossy().into_owned(),
                );
                if query.is_empty()
                    || name.to_lowercase().contains(&query)
                    || "clap".contains(&query)
                {
                    rows.push(Row {
                        module: entry.path.clone(),
                        cid: None,
                        selection: None,
                        name,
                        vendor: String::new(),
                        status: entry
                            .failure
                            .clone()
                            .unwrap_or_else(|| "clap unavailable".into()),
                    });
                }
                continue;
            };
            let state = entry
                .failure
                .as_ref()
                .map_or_else(String::new, |error| format!("last good · {error} · "));
            if entry.descriptors.is_empty() {
                let vendor = identity.bundle_identifier.clone().unwrap_or_default();
                let arch = if identity.thin_arm64_module {
                    "arm64 bundle"
                } else {
                    "mach-o bundle"
                };
                if query.is_empty()
                    || identity.bundle_name.to_lowercase().contains(&query)
                    || vendor.to_lowercase().contains(&query)
                    || "clap".contains(&query)
                {
                    rows.push(Row {
                        module: entry.path.clone(),
                        cid: None,
                        selection: None,
                        name: identity.bundle_name.clone(),
                        vendor,
                        status: format!("{source}{state}clap {arch} · discovered"),
                    });
                }
            } else {
                for descriptor in &entry.descriptors {
                    let kind = if descriptor
                        .features
                        .iter()
                        .any(|f| f.eq_ignore_ascii_case("instrument"))
                    {
                        "instrument"
                    } else if descriptor
                        .features
                        .iter()
                        .any(|f| f.eq_ignore_ascii_case("audio-effect"))
                    {
                        "effect"
                    } else {
                        "processor"
                    };
                    if !query.is_empty()
                        && !descriptor.name.to_lowercase().contains(&query)
                        && !descriptor.vendor.to_lowercase().contains(&query)
                        && !kind.contains(&query)
                        && !"clap".contains(&query)
                    {
                        continue;
                    }
                    rows.push(Row {
                        module: entry.path.clone(),
                        cid: None,
                        selection: None,
                        name: descriptor.name.clone(),
                        vendor: descriptor.vendor.clone(),
                        status: format!("{source}{state}clap {kind} · discovered"),
                    });
                }
            }
        }
        rows.sort_by(|a, b| (&a.module, &a.name).cmp(&(&b.module, &b.name)));
        rows
    }

    fn vst3_rows(&self, query: &str) -> Vec<Row> {
        let query = query.to_lowercase();
        let mut rows = Vec::new();
        for entry in &self.entries {
            if entry.last_good.is_none() {
                let name = entry.path.file_stem().map_or_else(
                    || "unavailable plug-in".into(),
                    |name| name.to_string_lossy().into_owned(),
                );
                if query.is_empty() || name.to_lowercase().contains(&query) {
                    rows.push(Row {
                        module: entry.path.clone(),
                        cid: None,
                        selection: None,
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
                let selectable_report = self.plugin_inventory(entry).ok().map(|(report, _)| report);
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
                    let selection = selectable_report
                        .and_then(|report| Self::plugin_selection(entry, report, class, None).ok())
                        .map(Arc::new);
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
                            let action = if selection.is_some() {
                                "drag or enter to insert"
                            } else {
                                "hosting unavailable"
                            };
                            format!("{source}{state}vst3 {kind} · {action}")
                        },
                        selection,
                    });
                }
            }
        }
        rows.sort_by(|a, b| (&a.module, a.cid).cmp(&(&b.module, b.cid)));
        rows
    }
}
