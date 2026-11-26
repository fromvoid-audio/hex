//! Strict schema, path, digest, and bound validation for plugin catalog records.
use super::*;

pub(crate) fn bounded(value: &str, maximum: usize) -> bool {
    value.len() <= maximum && !value.contains('\0')
}
pub(crate) fn sanitize_failure(error: &str) -> String {
    let cleaned: String = error
        .chars()
        .filter(|c| !c.is_control())
        .take(128)
        .collect();
    if cleaned.is_empty() {
        "scan failed".into()
    } else {
        cleaned
    }
}
pub(crate) fn digest(value: &str, size: usize) -> bool {
    value.len() == size
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
pub(crate) fn path_valid(path: &Path) -> bool {
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
        let total_modules = self
            .entries
            .len()
            .checked_add(self.clap_entries.len())
            .ok_or("catalog module count overflow")?;
        if self.version != CATALOG_VERSION || total_modules > MAX_MODULES {
            return Err("unsupported or oversized plugin catalog".into());
        }
        let mut paths = BTreeSet::new();
        let mut classes = 0_usize;
        for entry in &self.entries {
            if !path_valid(&entry.path)
                || entry.path.extension().and_then(|s| s.to_str()) != Some("vst3")
                || !paths.insert(&entry.path)
                || entry.failure.as_ref().is_some_and(|s| !bounded(s, 512))
                || entry
                    .failed_tools_sha256
                    .as_ref()
                    .is_some_and(|tools| entry.failed_bundle.is_none() || !digest(tools, 64))
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
        let mut clap_paths = BTreeSet::new();
        for entry in &self.clap_entries {
            if !path_valid(&entry.path)
                || entry.path.extension().and_then(|s| s.to_str()) != Some("clap")
                || !clap_paths.insert(&entry.path)
                || entry.failure.as_ref().is_some_and(|s| !bounded(s, 512))
            {
                return Err("invalid clap plugin catalog entry".into());
            }
            if let Some(identity) = &entry.last_good {
                if identity.supplied_path != entry.path
                    || !path_valid(&identity.canonical_path)
                    || identity.bundle_name.is_empty()
                    || !bounded(&identity.bundle_name, 256)
                    || identity
                        .bundle_identifier
                        .as_ref()
                        .is_some_and(|s| s.is_empty() || !bounded(s, 256))
                    || identity
                        .info_plist_sha256
                        .as_ref()
                        .is_some_and(|s| !digest(s, 64))
                    || !digest(&identity.executable_sha256, 64)
                    || identity.executable_relative_path.is_empty()
                    || !bounded(&identity.executable_relative_path, 4096)
                    || identity
                        .executable_relative_path
                        .split('/')
                        .any(|s| s.is_empty() || s == "." || s == "..")
                    || identity.executable_relative_path.contains('\\')
                    || Path::new(&identity.executable_relative_path).is_absolute()
                {
                    return Err("invalid retained clap plugin snapshot".into());
                }
                let mut descriptor_ids = BTreeSet::new();
                for descriptor in &entry.descriptors {
                    if descriptor.api[0] != 1
                        || descriptor.id.trim().is_empty()
                        || !bounded(&descriptor.id, 512)
                        || descriptor.name.trim().is_empty()
                        || !bounded(&descriptor.name, 512)
                        || !bounded(&descriptor.vendor, 512)
                        || !descriptor_ids.insert(&descriptor.id)
                    {
                        return Err("invalid clap descriptor metadata".into());
                    }
                }
                classes = classes
                    .checked_add(entry.descriptors.len().max(1))
                    .ok_or("catalog count overflow")?;
            } else if !entry.descriptors.is_empty() || entry.failure.is_none() {
                return Err("clap catalog entry has no scan result".into());
            }
        }
        if classes > MAX_CLASSES {
            return Err("plugin catalog class bound exceeded".into());
        }
        Ok(())
    }
}
