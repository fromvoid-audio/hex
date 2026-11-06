//! Saved identity resolution uses copied metadata only. Actual files and native
//! instances must still pass the processing host's original admission path.
use super::*;

impl Catalog {
    /// Read the last-known label for a saved class and exact binary. Discovery
    /// errors can disable loading while the copied name remains useful.
    pub fn saved_class_name(
        &self,
        cid: [u8; 16],
        executable_sha256: [u8; 32],
        info_sha256: [u8; 32],
    ) -> Option<&str> {
        let mut names = self.entries.iter().filter_map(|entry| {
            let report = entry.last_good.as_ref()?;
            if !binary_matches(report, executable_sha256, info_sha256) {
                return None;
            }
            report.discovery.as_ref()?.classes.iter().find_map(|class| {
                (class.cid == cid && class.category == "Audio Module Class")
                    .then_some(class.name.as_str())
            })
        });
        let name = names.next()?;
        names.all(|candidate| candidate == name).then_some(name)
    }

    /// Resolve one saved class and both binary digests without executing code,
    /// inspecting files, mutating the catalog or choosing among duplicate copies.
    /// Paths are lookup results, never part of the saved instance identity.
    pub fn resolve_effect(
        &self,
        cid: [u8; 16],
        executable_sha256: [u8; 32],
        info_sha256: [u8; 32],
    ) -> Result<PluginSelection, String> {
        self.resolve_plugin(cid, executable_sha256, info_sha256, PluginRole::Effect)
    }

    pub fn resolve_plugin(
        &self,
        cid: [u8; 16],
        executable_sha256: [u8; 32],
        info_sha256: [u8; 32],
        role: PluginRole,
    ) -> Result<PluginSelection, String> {
        self.validate()?;
        let mut selected = None;
        for entry in &self.entries {
            let Some(report) = &entry.last_good else {
                continue;
            };
            if !binary_matches(report, executable_sha256, info_sha256)
                || !report
                    .discovery
                    .as_ref()
                    .is_some_and(|discovery| discovery.classes.iter().any(|class| class.cid == cid))
            {
                continue;
            }
            let candidate = self.select_with_role(&entry.path, cid, Some(role))?;
            if selected.replace(candidate).is_some() {
                return Err("saved vst3 source is ambiguous · select and rescan one module".into());
            }
        }
        selected.ok_or_else(|| "saved vst3 binary or class is unavailable".into())
    }
}

fn binary_matches(report: &Report, executable_sha256: [u8; 32], info_sha256: [u8; 32]) -> bool {
    report
        .executable
        .as_ref()
        .is_some_and(|executable| digest_matches(&executable.sha256, executable_sha256))
        && digest_matches(&report.bundle.info_sha256, info_sha256)
}

fn digest_matches(encoded: &str, expected: [u8; 32]) -> bool {
    encoded.len() == 64
        && encoded
            .as_bytes()
            .chunks_exact(2)
            .zip(expected)
            .all(|(pair, byte)| {
                let hex = b"0123456789abcdef";
                pair == [hex[usize::from(byte >> 4)], hex[usize::from(byte & 15)]]
            })
}
