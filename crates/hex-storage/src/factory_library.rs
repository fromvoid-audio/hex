//! Offline factory library bootstrap, upgrade, precedence, and explicit
//! restore/reinstall through the ordinary `.hexdevice` installer (`#3004`).
//!
//! # Purpose
//! Provision all eleven factory native devices (7 effects + 4 instruments)
//! from bundled offline `.hexdevice` archives through the exact same
//! [`DevicePackageCatalog`] atomic staging, validation, and admission pipeline
//! used for user-dropped packages — with zero network access and zero static
//! DSP or UI panel fallback.
//!
//! # Factory vs User Precedence Contract
//! 1. **Same-ID conflicting user content is never overwritten**: if the user
//!    has already installed a `PackageOrigin::User` generation with the same
//!    `(type_id, major.minor.patch)` and different digest, bootstrap records
//!    [`FactoryItemOutcome::PreservedUserConflict`] and leaves the user's
//!    package intact.
//! 2. **User-selected defaults are never downgraded or silently replaced**:
//!    upgrading the base app adds compatible newer `PackageOrigin::Factory`
//!    generations alongside existing generations without altering any
//!    `selected_defaults` pin or active open project references.
//! 3. **Idempotent startup & explicit offline restore**: repeated launches
//!    report [`FactoryItemOutcome::AlreadyInstalled`] with zero rewrites;
//!    [`FactoryBootstrapMode::ExplicitRestore`] verifies installed factory
//!    generations and reinstalls missing/damaged factory artifacts offline
//!    while still respecting conflicting user-origin packages.
//! 4. **Graceful fault isolation**: missing factory artifacts, corrupted
//!    archives, non-writable/read-only library directories, and cancelled
//!    initialization return structured lowercase diagnostics without force-
//!    quitting or corrupting existing catalog/project data.

use crate::device_packages::{
    AdmissionPolicy, DevicePackageCatalog, InstallOutcome, InstalledPackageRecord,
    PackageInstallError, PackageOrigin,
};
use hex_lattice_contract::package_abi::{DEVICE_KIND_AUDIO_EFFECT, DEVICE_KIND_INSTRUMENT};
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// Total number of bundled factory native devices (7 audio effects + 4 instruments).
pub const FACTORY_LIBRARY_DEVICE_COUNT: usize = 11;

/// Static package inventory metadata describing one offline factory `.hexdevice`
/// artifact. Carries identifiers/filenames only — never DSP code or UI state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FactoryPackageSpec {
    pub type_id: &'static str,
    pub display_name: &'static str,
    pub filename: &'static str,
    pub device_kind: u32,
}

/// Complete eleven-device offline factory inventory reconciled against the
/// current native catalog (`#3004`).
pub const FACTORY_LIBRARY_SPECS: [FactoryPackageSpec; FACTORY_LIBRARY_DEVICE_COUNT] = [
    FactoryPackageSpec {
        type_id: "hex.pkg.utility",
        display_name: "utility",
        filename: "hex.pkg.utility.hexdevice",
        device_kind: DEVICE_KIND_AUDIO_EFFECT,
    },
    FactoryPackageSpec {
        type_id: "hex.pkg.saturator",
        display_name: "saturator",
        filename: "hex.pkg.saturator.hexdevice",
        device_kind: DEVICE_KIND_AUDIO_EFFECT,
    },
    FactoryPackageSpec {
        type_id: "hex.pkg.compressor",
        display_name: "compressor",
        filename: "hex.pkg.compressor.hexdevice",
        device_kind: DEVICE_KIND_AUDIO_EFFECT,
    },
    FactoryPackageSpec {
        type_id: "hex.pkg.gate",
        display_name: "gate",
        filename: "hex.pkg.gate.hexdevice",
        device_kind: DEVICE_KIND_AUDIO_EFFECT,
    },
    FactoryPackageSpec {
        type_id: "hex.pkg.auto_pan",
        display_name: "auto pan",
        filename: "hex.pkg.auto_pan.hexdevice",
        device_kind: DEVICE_KIND_AUDIO_EFFECT,
    },
    FactoryPackageSpec {
        type_id: "hex.pkg.eq",
        display_name: "eq",
        filename: "hex.pkg.eq.hexdevice",
        device_kind: DEVICE_KIND_AUDIO_EFFECT,
    },
    FactoryPackageSpec {
        type_id: "hex.pkg.eq8",
        display_name: "eq8",
        filename: "hex.pkg.eq8.hexdevice",
        device_kind: DEVICE_KIND_AUDIO_EFFECT,
    },
    FactoryPackageSpec {
        type_id: "hex.pkg.sine",
        display_name: "sine",
        filename: "hex.pkg.sine.hexdevice",
        device_kind: DEVICE_KIND_INSTRUMENT,
    },
    FactoryPackageSpec {
        type_id: "hex.pkg.operator",
        display_name: "vector",
        filename: "hex.pkg.operator.hexdevice",
        device_kind: DEVICE_KIND_INSTRUMENT,
    },
    FactoryPackageSpec {
        type_id: "hex.pkg.fm4",
        display_name: "fm4",
        filename: "hex.pkg.fm4.hexdevice",
        device_kind: DEVICE_KIND_INSTRUMENT,
    },
    FactoryPackageSpec {
        type_id: "hex.pkg.sampler",
        display_name: "sampler",
        filename: "hex.pkg.sampler.hexdevice",
        device_kind: DEVICE_KIND_INSTRUMENT,
    },
];

/// Bootstrap intent distinguishing automatic startup/upgrade from explicit user
/// factory restore/reinstall.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FactoryBootstrapMode {
    StartupOrUpgrade,
    ExplicitRestore,
}

/// Per-device outcome reported by [`bootstrap_factory_library`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FactoryItemOutcome {
    Installed(InstalledPackageRecord),
    AlreadyInstalled(InstalledPackageRecord),
    Restored(InstalledPackageRecord),
    /// Existing `PackageOrigin::User` generation occupies the same `(type_id, version)`
    /// with different content; preserved without overwriting.
    PreservedUserConflict {
        type_id: String,
        reason: String,
    },
    MissingArtifact {
        type_id: String,
        filename: String,
    },
    Canceled {
        type_id: String,
    },
    Failed {
        type_id: String,
        reason: String,
    },
}

/// Deterministic summary of one offline factory library bootstrap/restore pass.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FactoryBootstrapReport {
    pub items: Vec<FactoryItemOutcome>,
    pub catalog_error: Option<String>,
}

impl FactoryBootstrapReport {
    pub fn installed_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| matches!(i, FactoryItemOutcome::Installed(_)))
            .count()
    }

    pub fn already_installed_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| matches!(i, FactoryItemOutcome::AlreadyInstalled(_)))
            .count()
    }

    pub fn restored_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| matches!(i, FactoryItemOutcome::Restored(_)))
            .count()
    }

    pub fn preserved_user_conflicts(&self) -> usize {
        self.items
            .iter()
            .filter(|i| matches!(i, FactoryItemOutcome::PreservedUserConflict { .. }))
            .count()
    }

    pub fn missing_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| matches!(i, FactoryItemOutcome::MissingArtifact { .. }))
            .count()
    }

    pub fn failed_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| matches!(i, FactoryItemOutcome::Failed { .. }))
            .count()
    }

    pub fn canceled_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| matches!(i, FactoryItemOutcome::Canceled { .. }))
            .count()
    }

    /// Lowercase status summary suitable for Hex status banners and logs.
    pub fn summary_message(&self) -> String {
        if let Some(err) = self.catalog_error.as_deref() {
            return format!("factory library unavailable · {}", err.to_lowercase());
        }
        format!(
            "factory library · {} installed · {} existing · {} restored · {} user preserved · {} missing · {} failed",
            self.installed_count(),
            self.already_installed_count(),
            self.restored_count(),
            self.preserved_user_conflicts(),
            self.missing_count(),
            self.failed_count()
        )
    }
}

/// Stage, validate, and install all offline factory `.hexdevice` packages from
/// `factory_bundle_dir` into `library_root` using the exact same
/// [`DevicePackageCatalog`] atomic installer and [`AdmissionPolicy`] as user
/// drops. Never performs network I/O and never force-quits on library errors.
pub fn bootstrap_factory_library(
    library_root: &Path,
    factory_bundle_dir: &Path,
    policy: &AdmissionPolicy,
    mode: FactoryBootstrapMode,
    cancel: Option<&AtomicBool>,
) -> FactoryBootstrapReport {
    let catalog = match DevicePackageCatalog::open_and_recover(library_root) {
        Ok(cat) => cat,
        Err(err) => {
            return FactoryBootstrapReport {
                items: Vec::new(),
                catalog_error: Some(err.to_string().to_lowercase()),
            };
        }
    };

    let existing = catalog.list().unwrap_or_default();
    let mut report = FactoryBootstrapReport::default();

    for spec in FACTORY_LIBRARY_SPECS {
        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            report.items.push(FactoryItemOutcome::Canceled {
                type_id: spec.type_id.to_owned(),
            });
            continue;
        }
        let archive_path = factory_bundle_dir.join(spec.filename);
        let bytes = match fs::read(&archive_path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                report.items.push(FactoryItemOutcome::MissingArtifact {
                    type_id: spec.type_id.to_owned(),
                    filename: spec.filename.to_owned(),
                });
                continue;
            }
            Err(err) => {
                report.items.push(FactoryItemOutcome::Failed {
                    type_id: spec.type_id.to_owned(),
                    reason: format!("unable to read factory artifact: {err}").to_lowercase(),
                });
                continue;
            }
        };

        // In ExplicitRestore mode, if a matching factory generation is already
        // cataloged but damaged on disk, detect and restore it safely.
        let mut had_damaged_factory = false;
        if mode == FactoryBootstrapMode::ExplicitRestore {
            for rec in existing
                .iter()
                .filter(|r| r.type_id == spec.type_id && r.origin == PackageOrigin::Factory)
            {
                let ver = (rec.package_major, rec.package_minor, rec.package_patch);
                if catalog
                    .verify_generation_integrity(&rec.type_id, ver)
                    .is_err()
                {
                    let _ = catalog.request_uninstall(&rec.type_id, ver);
                    let _ = catalog.reclaim_pending_removals(|_| false);
                    had_damaged_factory = true;
                }
            }
        }

        match catalog.install_bytes_with_origin(
            &bytes,
            PackageOrigin::Factory,
            None,
            policy,
            cancel,
        ) {
            Ok(InstallOutcome::Installed(rec)) => {
                if rec.type_id != spec.type_id || rec.manifest.device_kind != spec.device_kind {
                    report.items.push(FactoryItemOutcome::Failed {
                        type_id: spec.type_id.to_owned(),
                        reason: format!(
                            "factory artifact identity mismatch: got {}",
                            rec.type_id.to_lowercase()
                        ),
                    });
                } else if had_damaged_factory {
                    report.items.push(FactoryItemOutcome::Restored(rec));
                } else {
                    report.items.push(FactoryItemOutcome::Installed(rec));
                }
            }
            Ok(InstallOutcome::AlreadyInstalled(rec)) => {
                report.items.push(FactoryItemOutcome::AlreadyInstalled(rec));
            }
            Err(PackageInstallError::VersionHashConflict { type_id, version }) => {
                let is_user_origin = catalog
                    .list()
                    .ok()
                    .and_then(|list| {
                        list.into_iter().find(|r| {
                            r.type_id == type_id
                                && (r.package_major, r.package_minor, r.package_patch) == version
                        })
                    })
                    .is_none_or(|r| r.origin == PackageOrigin::User);
                if is_user_origin {
                    report
                        .items
                        .push(FactoryItemOutcome::PreservedUserConflict {
                            type_id: spec.type_id.to_owned(),
                            reason: "user-installed package generation preserved".to_owned(),
                        });
                } else {
                    report.items.push(FactoryItemOutcome::Failed {
                        type_id: spec.type_id.to_owned(),
                        reason: format!(
                            "conflicting factory generation {}.{}.{} already installed",
                            version.0, version.1, version.2
                        ),
                    });
                }
            }
            Err(PackageInstallError::PublisherTakeover {
                type_id,
                existing_publisher,
                ..
            }) => {
                let is_user_origin = catalog
                    .list()
                    .ok()
                    .and_then(|list| list.into_iter().find(|r| r.type_id == type_id))
                    .is_some_and(|r| r.origin == PackageOrigin::User);
                if is_user_origin {
                    report
                        .items
                        .push(FactoryItemOutcome::PreservedUserConflict {
                            type_id: spec.type_id.to_owned(),
                            reason: format!(
                                "user-installed publisher '{}' preserved",
                                existing_publisher.to_lowercase()
                            ),
                        });
                } else {
                    report.items.push(FactoryItemOutcome::Failed {
                        type_id: spec.type_id.to_owned(),
                        reason: format!(
                            "publisher conflict with '{}'",
                            existing_publisher.to_lowercase()
                        ),
                    });
                }
            }
            Err(PackageInstallError::Cancelled) => {
                report.items.push(FactoryItemOutcome::Canceled {
                    type_id: spec.type_id.to_owned(),
                });
            }
            Err(err) => {
                report.items.push(FactoryItemOutcome::Failed {
                    type_id: spec.type_id.to_owned(),
                    reason: err.to_string().to_lowercase(),
                });
            }
        }
    }

    report
}
