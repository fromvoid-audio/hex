//! Format-neutral plugin catalog abstractions shared by VST3 and CLAP.
//!
//! Each supported external format implements [`CatalogFormatBackend`] so folder
//! resolution, bounded enumeration, and row projection share one recoverable
//! catalog boundary without duplicating traversal or persistence logic.

use crate::{Catalog, Row, roots};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// External plugin format tracked by the recoverable plugin catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginFormat {
    Vst3,
    Clap,
}

impl PluginFormat {
    /// Lowercase canonical identifier for UI labels and diagnostics.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Vst3 => "vst3",
            Self::Clap => "clap",
        }
    }

    /// Bundle directory extension (without leading dot).
    pub const fn bundle_extension(self) -> &'static str {
        match self {
            Self::Vst3 => "vst3",
            Self::Clap => "clap",
        }
    }

    /// Standard macOS Audio Plug-Ins subdirectory name.
    pub const fn macos_folder_name(self) -> &'static str {
        match self {
            Self::Vst3 => "VST3",
            Self::Clap => "CLAP",
        }
    }
}

/// Format-neutral discovery and projection contract over [`Catalog`].
pub trait CatalogFormatBackend: Send + Sync {
    fn format(&self) -> PluginFormat;

    fn standard_roots(&self) -> Result<Vec<PathBuf>, String> {
        roots::macos_standard_paths_for(self.format())
    }

    fn scan_roots(
        &self,
        system: bool,
        user: bool,
        custom: Option<PathBuf>,
    ) -> Result<Vec<PathBuf>, String> {
        roots::macos_scan_paths_for(self.format(), system, user, custom)
    }

    fn enumerate(
        &self,
        roots: &[PathBuf],
        cancelled: &mut dyn FnMut() -> bool,
    ) -> Result<roots::Enumeration, String> {
        roots::enumerate_for(self.format(), roots, cancelled)
    }

    fn rows(&self, catalog: &Catalog, query: &str) -> Vec<Row>;
}

/// VST3 catalog backend delegating to the existing VST3 discovery and projection.
#[derive(Clone, Copy, Debug, Default)]
pub struct Vst3CatalogBackend;

impl CatalogFormatBackend for Vst3CatalogBackend {
    fn format(&self) -> PluginFormat {
        PluginFormat::Vst3
    }

    fn rows(&self, catalog: &Catalog, query: &str) -> Vec<Row> {
        catalog.rows_for(PluginFormat::Vst3, query)
    }
}

/// CLAP catalog backend delegating to bounded user-folder CLAP discovery and projection.
#[derive(Clone, Copy, Debug, Default)]
pub struct ClapCatalogBackend;

impl CatalogFormatBackend for ClapCatalogBackend {
    fn format(&self) -> PluginFormat {
        PluginFormat::Clap
    }

    fn rows(&self, catalog: &Catalog, query: &str) -> Vec<Row> {
        catalog.rows_for(PluginFormat::Clap, query)
    }
}

/// Return a static backend handle for the requested plugin format.
pub fn backend(format: PluginFormat) -> &'static dyn CatalogFormatBackend {
    static VST3: Vst3CatalogBackend = Vst3CatalogBackend;
    static CLAP: ClapCatalogBackend = ClapCatalogBackend;
    match format {
        PluginFormat::Vst3 => &VST3,
        PluginFormat::Clap => &CLAP,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backends_resolve_distinct_macos_standard_roots() {
        let vst3 = backend(PluginFormat::Vst3);
        let clap = backend(PluginFormat::Clap);
        assert_eq!(vst3.format(), PluginFormat::Vst3);
        assert_eq!(clap.format(), PluginFormat::Clap);

        let vst3_roots = vst3.scan_roots(true, true, None).unwrap();
        let clap_roots = clap.scan_roots(true, true, None).unwrap();
        assert_eq!(vst3_roots[0], PathBuf::from("/Library/Audio/Plug-Ins/VST3"));
        assert!(vst3_roots[1].ends_with("Library/Audio/Plug-Ins/VST3"));
        assert_eq!(clap_roots[0], PathBuf::from("/Library/Audio/Plug-Ins/CLAP"));
        assert!(clap_roots[1].ends_with("Library/Audio/Plug-Ins/CLAP"));
    }
}
