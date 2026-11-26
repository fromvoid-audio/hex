//! # Purpose
//!
//! Bounded control-side persistence and selected-bundle discovery coordination for
//! VST3 and CLAP browser metadata. Owns a recoverable current-format catalog,
//! copied records, last-good/error projection, format-neutral discovery traits,
//! and the outer standalone scanner's control lifetime. All filesystem/process
//! work runs on a background control worker.
//!
//! # Dependencies
//!
//! `hex-plugin-scan` supplies copied VST3 metadata, fixed package identity and
//! admission checks. `hex-clap-scan` supplies bounded explicit user-folder CLAP
//! discovery, bundle identity inspection, and CLAP descriptor records. Pinned
//! serde/serde_json encode bounded current-format records. Pinned fs2 supplies
//! nonblocking advisory writer exclusion, with locks held only during catalog
//! read-modify-publication, never across foreign-code execution.
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
//! Default folder scans reuse successful cacheable inventories when the native
//! tools, bundle metadata and executable still match. Full scans run discovery
//! for all eligible bundles. Neither mode changes the admission checks for use.
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
//! Catalog version 5 adds bounded CLAP bundle entries (`clap_entries`) alongside
//! VST3 entries. Catalog version 4 recorded the verified discovery tools
//! alongside the bundle identity for a failed scan. An incremental folder scan
//! skips a failure only when both identities match. A full folder or
//! single-bundle scan always retries. Versions 2, 3, and 4 migrate in memory to
//! version 5 and retain last-good reports and failure diagnostics (rejecting
//! `clap_entries` on versions 2, 3, and 4); versions 2 and 3 failures lack tool
//! provenance and are eligible for retry. Only a catalog update publishes
//! version 5. Version 1 and newer unsupported files (version 6+) remain intact. A binary change with an unchanged Info.plist still requires
//! an explicit retry while the discovery tools remain identical.
//! The native integration tests require an explicit signed original package through
//! `HEX_VST3_CATALOG_TEST_APP` and original fixture directory through
//! `HEX_VST3_CATALOG_FIXTURES`; ordinary runs keep these tests ignored. They do not
//! start the Hex application or establish physical device/listening acceptance.
//!
//! Dynamic/discardable or unknown factory flags retain historical metadata only;
//! rows explicitly require a rescan. Stored inventory never establishes future
//! cacheable class availability or authorizes processing admission.
mod fingerprint;
pub mod format;
mod model;
mod projection;
mod resolve;
pub mod roots;
pub mod storage;
mod validate;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod worker;

pub use format::{
    CatalogFormatBackend, ClapCatalogBackend, PluginFormat, Vst3CatalogBackend, backend,
};
use hex_clap_scan::{Descriptor as ClapDescriptor, user_folder::ClapBundleIdentity};
use hex_plugin_scan::{
    Discovery, PUBLIC_SDK, SDK,
    macos::{identity::Bundle, report::Report},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
    sync::Arc,
};

pub const MAX_MODULES: usize = 512;
pub const MAX_CLASSES: usize = 2048;
pub const MAX_CATALOG_BYTES: usize = 4 * 1024 * 1024;
/// Current saved catalog format; see the crate documentation for migration.
pub const CATALOG_VERSION: u32 = 5;
const CATALOG_VERSION_2: u32 = 2;
const CATALOG_VERSION_3: u32 = 3;
const CATALOG_VERSION_4: u32 = 4;

pub(crate) use model::FailedScan;
pub use model::{Catalog, ClapEntry, Entry};
pub use projection::{PluginRole, PluginSelection, Row};
pub(crate) use validate::{digest, path_valid, sanitize_failure};
pub use validate::{validate_discovery, validate_report};

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
    pub(super) struct Fixture(pub(super) PathBuf);
    impl Fixture {
        pub(super) fn new() -> Self {
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
    fn saved_plugin_label_uses_the_exact_class_and_binary_and_failures_use_the_stem() {
        let fixture = Fixture::new();
        let path = fixture.module();
        let mut catalog = Catalog::default();
        catalog
            .record(path.clone(), Err("unavailable".into()))
            .unwrap();
        assert_eq!(catalog.rows("")[0].name, "Original");

        let report = effect_report(path.clone(), "Fx");
        catalog
            .record(path.clone(), Ok((report, "e".repeat(40))))
            .unwrap();
        assert_eq!(
            catalog.saved_class_name([1; 16], [0xdd; 32], [0xcc; 32]),
            Some("Original Processor"),
        );
        assert_eq!(
            catalog.saved_class_name([2; 16], [0xdd; 32], [0xcc; 32]),
            None
        );
        assert_eq!(
            catalog.saved_class_name([1; 16], [0xde; 32], [0xcc; 32]),
            None
        );
        assert_eq!(
            catalog.saved_class_name([1; 16], [0xdd; 32], [0xcd; 32]),
            None
        );
        catalog.record(path, Err("rescan failed".into())).unwrap();
        assert_eq!(
            catalog.saved_class_name([1; 16], [0xdd; 32], [0xcc; 32]),
            Some("Original Processor"),
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
        let rows = catalog.rows("");
        assert_eq!(rows[1].selection.as_deref(), Some(&selected));
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
            let role = match categories {
                "Instrument|Synth" => Some(PluginRole::Instrument),
                "Fx|Delay" | "Fx|Instrument" => Some(PluginRole::Effect),
                _ => None,
            };
            let rows = loaded.rows("");
            assert_eq!(
                rows[0].selection.as_ref().map(|selected| selected.role),
                role
            );
            assert_eq!(
                rows[0].status,
                if role.is_some() {
                    format!("vst3 {expected} · drag or enter to insert")
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
    fn transient_lock_contention_retries_and_succeeds() {
        let fixture = Fixture::new();
        let file = fixture.catalog();
        storage::update(&file, |c| c.record(fixture.module(), Err("initial".into()))).unwrap();
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(file.with_extension("lock"))
            .unwrap();
        fs2::FileExt::try_lock_exclusive(&lock).unwrap();
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(15));
            drop(lock);
        });
        let result = storage::update(&file, |c| {
            c.record(fixture.module(), Err("after-retry".into()))
        });
        releaser.join().unwrap();
        assert!(
            result.is_ok(),
            "transient lock contention should retry: {:?}",
            result.err()
        );
        assert_eq!(
            Catalog::load(&file).unwrap().entries[0].failure.as_deref(),
            Some("after-retry")
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
        assert_eq!(saved["version"], CATALOG_VERSION);
        let reopened = Catalog::load(&fixture.catalog()).unwrap();
        assert_eq!(reopened.entries.len(), 2);

        // A version 2 file cannot carry the version 3 field; newer formats (v6+) are refused.
        for (version, failed) in [(2, true), (6, false)] {
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
        let tools = "a".repeat(64);
        storage::update(&fixture.catalog(), |catalog| {
            catalog.record_failure(
                path.clone(),
                "deadline exceeded".into(),
                Some(FailedScan {
                    bundle: identity.clone(),
                    tools_sha256: tools.clone(),
                }),
            )
        })
        .unwrap();
        let saved = Catalog::load(&fixture.catalog()).unwrap();
        assert_eq!(saved.entries[0].failed_bundle.as_ref(), Some(&identity));
        assert!(saved.failed_unchanged(&path, &identity, &tools));
        assert!(
            !saved.failed_unchanged(&path, &failed_identity(&path, 'f'), &tools),
            "changed plist"
        );
        let other = fixture.0.join("Other.vst3");
        assert!(!saved.failed_unchanged(&other, &failed_identity(&other, 'c'), &tools));

        // A failure without identity, or a later success, is not remembered.
        let mut catalog = saved.clone();
        catalog.record(path.clone(), Err("again".into())).unwrap();
        assert!(!catalog.failed_unchanged(&path, &identity, &tools));
        let mut catalog = saved;
        catalog
            .record(path.clone(), Ok((report(path.clone()), "e".repeat(40))))
            .unwrap();
        assert!(catalog.entries[0].failed_bundle.is_none());
        assert!(!catalog.failed_unchanged(&path, &identity, &tools));

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
                .record_failure(
                    path,
                    "x".into(),
                    Some(FailedScan {
                        bundle: failed_identity(&other, 'c'),
                        tools_sha256: tools,
                    }),
                )
                .is_err(),
            "identity for another path"
        );
    }

    #[test]
    fn clap_entries_persist_refresh_project_rows_and_coexist_with_vst3() {
        let fixture = Fixture::new();
        let vst3 = fixture.module();
        let clap = fixture.0.join("HexClapLead.clap");
        fs::create_dir_all(clap.join("Contents/MacOS")).unwrap();
        let mut macho = vec![0u8; 56];
        for (at, n) in [
            (0, 0xfeedfacf_u32),
            (4, 0x0100000c),
            (12, 6),
            (16, 1),
            (20, 24),
            (32, 0x1b),
            (36, 24),
        ] {
            macho[at..at + 4].copy_from_slice(&n.to_le_bytes());
        }
        macho[40..56].fill(3);
        fs::write(
            clap.join("Contents/Info.plist"),
            "<plist><dict><key>CFBundleIdentifier</key><string>org.hex.clap.lead</string><key>CFBundleExecutable</key><string>HexClapLead</string></dict></plist>",
        )
        .unwrap();
        fs::write(clap.join("Contents/MacOS/HexClapLead"), &macho).unwrap();

        let bad_clap = fixture.0.join("Broken.clap");
        fs::create_dir_all(bad_clap.join("Contents/MacOS")).unwrap();
        fs::write(bad_clap.join("Contents/MacOS/Broken"), b"corrupt").unwrap();

        // Populate VST3 entry first via recoverable storage update.
        storage::update(&fixture.catalog(), |catalog| {
            catalog.record(
                vst3.clone(),
                Ok((effect_report(vst3.clone(), "Fx"), "e".repeat(40))),
            )
        })
        .unwrap();

        // Run CLAP folder scan through the shared worker cancellation/publication boundary.
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            let cancel = crate::worker::Cancellation::default();
            let update = crate::worker::scan_clap_folders(
                &fixture.catalog(),
                std::slice::from_ref(&fixture.0),
                cancel.token(),
            );
            assert!(
                update.status.contains("1 discovered · 1 failed"),
                "{}",
                update.status
            );
            let mut catalog = update.catalog.expect("published catalog");
            assert_eq!(catalog.entries.len(), 1);
            assert_eq!(catalog.clap_entries.len(), 2);

            // Reopen from disk and confirm format-neutral backends project cleanly.
            let reopened = Catalog::load(&fixture.catalog()).unwrap();
            assert_eq!(backend(PluginFormat::Vst3).rows(&reopened, "").len(), 1);
            let clap_rows = backend(PluginFormat::Clap).rows(&reopened, "");
            assert_eq!(clap_rows.len(), 2);
            assert!(
                clap_rows.iter().any(
                    |row| row.name == "HexClapLead" && row.status.contains("clap arm64 bundle")
                )
            );
            assert!(
                clap_rows
                    .iter()
                    .any(|row| row.name == "Broken" && row.status.contains("Mach-O"))
            );
            assert_eq!(reopened.rows("").len(), 3);

            // Mutating the CLAP binary triggers a transient source_error without deleting last_good.
            macho[42] = 9;
            fs::write(clap.join("Contents/MacOS/HexClapLead"), &macho).unwrap();
            catalog.refresh_sources().unwrap();
            let refreshed = catalog.clap_rows("HexClapLead");
            assert_eq!(refreshed.len(), 1);
            assert!(
                refreshed[0].status.contains("source unavailable"),
                "{}",
                refreshed[0].status
            );
        }
    }

    #[test]
    fn version_4_catalog_migrates_to_v5_and_historical_versions_reject_clap_entries() {
        let fixture = Fixture::new();
        let bad = fixture.0.join("Bad.vst3");
        let clap = fixture.0.join("Lead.clap");
        let v4 = serde_json::json!({
            "version": 4,
            "entries": [{
                "path": bad,
                "last_good": null,
                "source": null,
                "failure": "scan failed",
                "failed_bundle": failed_identity(&bad, 'c'),
                "failed_tools_sha256": "a".repeat(64),
            }],
        });
        let bytes = serde_json::to_vec(&v4).unwrap();
        fs::write(fixture.catalog(), &bytes).unwrap();

        let loaded = Catalog::load(&fixture.catalog()).expect("v4 migrates to v5");
        assert_eq!(loaded.version, CATALOG_VERSION);
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(
            loaded.entries[0].failed_tools_sha256.as_deref(),
            Some("a".repeat(64).as_str())
        );
        assert_eq!(
            fs::read(fixture.catalog()).unwrap(),
            bytes,
            "load preserves file"
        );

        storage::update(&fixture.catalog(), |_| Ok(())).unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&fs::read(fixture.catalog()).unwrap()).unwrap();
        assert_eq!(saved["version"], CATALOG_VERSION);

        // Historical v2, v3, v4 catalogs carrying `clap_entries` must be rejected without modifying the file.
        for version in [2_u32, 3, 4] {
            let malformed = serde_json::json!({
                "version": version,
                "entries": [],
                "clap_entries": [{
                    "path": clap,
                    "last_good": null,
                    "failure": "boom",
                }],
            });
            let raw = serde_json::to_vec(&malformed).unwrap();
            fs::write(fixture.catalog(), &raw).unwrap();
            assert!(
                Catalog::load(&fixture.catalog()).is_err(),
                "version {version} must reject clap_entries"
            );
            assert_eq!(fs::read(fixture.catalog()).unwrap(), raw, "file preserved");
        }
    }

    #[test]
    fn clap_validation_rejects_traversal_and_nul_and_failure_strings_sanitize_controls() {
        let fixture = Fixture::new();
        let clap = fixture.0.join("Synth.clap");
        let vst3 = fixture.module();

        // NUL and control characters in failure strings are stripped cleanly and keep catalog valid.
        let mut catalog = Catalog::default();
        catalog
            .record(vst3, Err("bad\0error\nwith\x1bcontrols".into()))
            .expect("vst3 failure with NUL sanitized");
        assert_eq!(
            catalog.entries[0].failure.as_deref(),
            Some("baderrorwithcontrols")
        );
        catalog
            .record_clap(clap.clone(), Err("clap\0fail\r\n".into()))
            .expect("clap failure with NUL sanitized");
        assert_eq!(catalog.clap_entries[0].failure.as_deref(), Some("clapfail"));
        catalog.validate().expect("sanitized catalog remains valid");

        // Relative path traversal, `.`, empty segments, NUL, and backslash are rejected on CLAP entries.
        for bad_rel in [
            "Contents/../MacOS/Synth",
            "Contents/./MacOS/Synth",
            "Contents//MacOS/Synth",
            "Contents/MacOS/Synth\0bad",
            "Contents\\MacOS\\Synth",
            "/Contents/MacOS/Synth",
        ] {
            let mut bad_cat = Catalog::default();
            bad_cat.clap_entries.push(ClapEntry {
                path: clap.clone(),
                last_good: Some(ClapBundleIdentity {
                    supplied_path: clap.clone(),
                    canonical_path: clap.clone(),
                    bundle_name: "Synth".into(),
                    bundle_identifier: Some("org.hex.synth".into()),
                    info_plist_sha256: Some("a".repeat(64)),
                    executable_relative_path: bad_rel.into(),
                    executable_sha256: "b".repeat(64),
                    thin_arm64_module: true,
                }),
                descriptors: Vec::new(),
                failure: None,
            });
            assert!(
                bad_cat.validate().is_err(),
                "bad CLAP executable_relative_path `{bad_rel}` must be rejected"
            );
        }
    }
}
