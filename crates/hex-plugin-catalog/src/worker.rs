//! Background control work. The standalone scanner alone owns native children.
use crate::{Catalog, FailedScan, validate_report};
use hex_plugin_scan::macos::{
    identity::Bundle,
    package::Package,
    protocol::WAIT_FOR_PLUGIN_ARG,
    report::{Cleanup, Report},
};
use std::{
    io::Write,
    os::{fd::OwnedFd, unix::net::UnixStream},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};

const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
const ERROR_LIMIT: usize = 64 * 1024;
const RUNNING: u8 = 0;
const CANCELLED: u8 = 1;
const COMMITTING: u8 = 2;
const FINISHED: u8 = 3;

mod cancellation;
mod clap_worker;
mod vst3_worker;

use cancellation::CurrentModuleGuard;
pub use cancellation::{Cancellation, ScanDeadline, ScanDiagnostics, ScanMode, Token, Update};
pub use clap_worker::{scan_clap_folders, scan_clap_standard};
#[cfg(test)]
use vst3_worker::{
    CachedResult, DiscoveryOutcome, ScanTools, cached_result, discover, discovery_app_path, drain,
    poll_and_drain, scan_batch, scanner_command,
};
pub use vst3_worker::{
    available_app, scan, scan_folders, scan_folders_with_deadline, scan_folders_with_mode,
    scan_standard, scan_standard_with_deadline, scan_with_deadline,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_launch_can_select_absolute_tools_and_packaged_apps_keep_their_own() {
        let tools = Path::new("/checkout/artifacts/tools/hex.app");
        let raw = Path::new("/checkout/target/release/hex-desktop");
        assert_eq!(discovery_app_path(raw, Some(tools)).unwrap(), tools);
        assert!(discovery_app_path(raw, Some(Path::new("hex.app"))).is_err());
        assert!(discovery_app_path(raw, None).is_err());
        let packaged = Path::new("/Applications/hex.app/Contents/MacOS/hex");
        assert_eq!(
            discovery_app_path(packaged, Some(tools)).unwrap(),
            Path::new("/Applications/hex.app")
        );
    }
    #[test]
    fn scan_deadline_is_bounded_and_applies_to_scanner_and_outer_guard() {
        assert!(ScanDeadline::from_millis(0).is_err());
        assert!(ScanDeadline::from_millis(30_001).is_err());
        assert!(ScanDeadline::from_millis(1).is_ok());
        assert!(ScanDeadline::from_millis(30_000).is_ok());
        let default = ScanDeadline::default();
        assert_eq!(default.millis, 10_000);
        assert_eq!(default.outer_limit(), Duration::from_secs(12));
        let deadline = ScanDeadline::from_millis(23_456).unwrap();
        let command = scanner_command(Path::new("/scanner"), Path::new("/test.vst3"), deadline);
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["--bundled", "--trust-user-code", "/test.vst3", "23456"]
        );
        assert_eq!(deadline.outer_limit(), Duration::from_millis(25_456));
        assert!(!deadline.expired(Duration::from_millis(25_456)));
        assert!(deadline.expired(Duration::from_millis(25_457)));
    }
    #[test]
    fn pre_spawn_refusal_reports_no_invented_native_cleanup() {
        let owner = Cancellation::default();
        let module = Path::new("/original-fixture.vst3");
        let outcome = discover(
            Path::new(""),
            module,
            None,
            &owner.token(),
            ScanDeadline::default(),
        );
        assert!(outcome.result.is_err());
        assert!(outcome.chain_retired, "no scanner was started");
        let observed = outcome.diagnostics;
        assert_eq!(observed.module, module);
        assert!(!observed.scanner_started);
        assert!(observed.scanner_exit.is_none());
        assert!(observed.native_cleanup.is_none());
        assert!(observed.report.is_empty() && observed.stderr.is_empty());
    }
    #[test]
    fn final_report_published_at_reap_is_drained_without_waiting_for_eof() {
        let (mut output, mut writer) = UnixStream::pair().unwrap();
        let (mut diagnostic, mut diagnostic_writer) = UnixStream::pair().unwrap();
        output.set_nonblocking(true).unwrap();
        diagnostic.set_nonblocking(true).unwrap();
        let payload = vec![b'x'; 8_000];
        let mut out = Vec::new();
        let mut err = Vec::new();
        // The old loop observed empty input here, then reaped and broke,
        // discarding the report published immediately before successful reap.
        assert!(!drain(&mut output, &mut out, OUTPUT_LIMIT).unwrap());
        let mut exit = None;
        let mut overflowed = [false; 2];
        let (received, error) = poll_and_drain(
            &mut exit,
            || {
                writer.write_all(&payload).unwrap();
                diagnostic_writer.write_all(b"final diagnostic").unwrap();
                Ok(Some(()))
            },
            [
                (&mut output, &mut out, OUTPUT_LIMIT),
                (&mut diagnostic, &mut err, ERROR_LIMIT),
            ],
            &mut overflowed,
        )
        .unwrap();
        assert!(received);
        assert!(error.is_none());
        assert_eq!(out, payload, "the report includes multiple read chunks");
        for _ in 0..4 {
            let (received, error) = poll_and_drain(
                &mut exit,
                || panic!("retired child must never be polled again"),
                [
                    (&mut output, &mut out, OUTPUT_LIMIT),
                    (&mut diagnostic, &mut err, ERROR_LIMIT),
                ],
                &mut overflowed,
            )
            .unwrap();
            assert!(error.is_none());
            if !received {
                // Writers deliberately remain open: termination is not EOF-based.
                assert_eq!(out, payload);
                assert_eq!(err, b"final diagnostic");
                return;
            }
        }
        panic!("bounded final report did not finish draining");
    }

    #[test]
    fn overflowed_stream_discards_flood_without_growing_buffer() {
        let (mut output, mut writer) = UnixStream::pair().unwrap();
        let (mut diagnostic, _diag_writer) = UnixStream::pair().unwrap();
        output.set_nonblocking(true).unwrap();
        diagnostic.set_nonblocking(true).unwrap();

        let flood_thread = std::thread::spawn(move || {
            let chunk = vec![b'A'; 32 * 1024];
            for _ in 0..8 {
                writer.write_all(&chunk).unwrap();
            }
        });

        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut exit: Option<()> = None;
        let mut overflowed = [false; 2];
        let started = Instant::now();
        let mut saw_error = false;
        while started.elapsed() < Duration::from_secs(2) {
            let (received, error) = poll_and_drain(
                &mut exit,
                || Ok(None),
                [
                    (&mut output, &mut out, 8 * 1024),
                    (&mut diagnostic, &mut err, ERROR_LIMIT),
                ],
                &mut overflowed,
            )
            .unwrap();
            if error.is_some() {
                saw_error = true;
            }
            if !received && flood_thread.is_finished() {
                break;
            }
            if !received {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        assert!(
            flood_thread.is_finished(),
            "overflow drain must unblock flooded writer promptly"
        );
        flood_thread.join().unwrap();
        assert!(saw_error, "overflow must be reported");
        assert!(overflowed[0], "overflowed stream flag latched");
        assert!(
            out.len() <= 8 * 1024,
            "retained bytes must stay within limit: {}",
            out.len()
        );
    }

    #[test]
    fn vst3_capacity_check_counts_existing_clap_entries() {
        let root = std::env::temp_dir().join(format!(
            "hex-catalog-capacity-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let catalog_path = root.join("catalog.json");
        crate::storage::update(&catalog_path, |catalog| {
            for i in 0..crate::MAX_MODULES {
                catalog.record_clap(root.join(format!("P{i}.clap")), Err("x".into()))?;
            }
            Ok(())
        })
        .unwrap();

        let cancel = Cancellation::default();
        let update = scan_with_deadline(
            &catalog_path,
            &root.join("hex.app"),
            root.join("OneMore.vst3"),
            cancel.token(),
            ScanDeadline::default(),
        );
        assert!(
            update.status.contains("capacity reached"),
            "unexpected status: {}",
            update.status
        );
        assert!(update.catalog.is_none());

        let folder = root.join("folder");
        std::fs::create_dir_all(folder.join("OneMore.vst3")).unwrap();
        let folder_update = scan_folders_with_mode(
            &catalog_path,
            &root.join("hex.app"),
            &[folder],
            cancel.token(),
            ScanDeadline::default(),
            ScanMode::Incremental,
        );
        assert!(
            folder_update.status.contains("capacity reached"),
            "unexpected folder status: {}",
            folder_update.status
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cancellation_wins_until_commit_and_never_claims_a_late_cancel() {
        let owner = Cancellation::default();
        let token = owner.token();
        assert!(owner.cancel());
        assert!(
            owner.cancel(),
            "repeated pending cancellation remains accepted"
        );
        assert!(!token.begin_publication().unwrap());
        assert!(!owner.cancel());
        let owner = Cancellation::default();
        let token = owner.token();
        assert!(token.begin_publication().unwrap());
        assert!(!owner.cancel());
    }

    fn identity(path: &Path, info: char) -> Bundle {
        Bundle {
            supplied_path: path.to_path_buf(),
            canonical_path: path.to_path_buf(),
            info_sha256: info.to_string().repeat(64),
        }
    }

    fn remembered_failure(
        catalog: &Catalog,
        path: &Path,
        info: char,
        tools: &str,
    ) -> Option<CachedResult> {
        catalog
            .failed_unchanged(path, &identity(path, info), tools)
            .then_some(CachedResult::Failure)
    }

    /// Fake discovery outcomes keyed by bundle name; counts native launches.
    const TOOLS: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn fake_discovery(path: &Path, launches: &mut Vec<PathBuf>) -> DiscoveryOutcome {
        launches.push(path.to_path_buf());
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let (result, chain_retired) = match name {
            "Hanging" => (
                Err("scanner deadline exceeded; cleanup retained".into()),
                true,
            ),
            "Licensing" => (Err("native cleanup unconfirmed".into()), false),
            "Setup" => (Err("scanner start failed".into()), true),
            _ => (
                Ok((crate::tests::report(path.to_path_buf()), "e".repeat(40))),
                true,
            ),
        };
        DiscoveryOutcome {
            result,
            chain_retired,
            diagnostics: ScanDiagnostics {
                scanner_started: name != "Setup",
                ..ScanDiagnostics::default()
            },
            failed_scan: Some(FailedScan {
                bundle: identity(path, 'c'),
                tools_sha256: TOOLS.into(),
            }),
        }
    }

    #[test]
    fn folder_scan_reuses_exact_sources_and_full_scan_retries_failures() {
        use std::fs;

        let fixture = crate::tests::Fixture::new();
        let [unchanged, changed, new, failed, dynamic] =
            ["Unchanged", "Changed", "New", "Hanging", "Dynamic"]
                .map(|name| fixture.0.join(format!("{name}.vst3")));
        let mut binary: Vec<_> = [0xfeed_facf_u32, 0x0100_000c, 0, 8, 1, 8, 0, 0, 0, 8]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect();
        binary.push(0);
        let executable = "Contents/MacOS/Original";
        for bundle in [&unchanged, &changed, &new, &failed, &dynamic] {
            fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
            fs::write(bundle.join("Contents/Info.plist"), b"original metadata").unwrap();
            fs::write(bundle.join(executable), &binary).unwrap();
        }
        let mut catalog = Catalog::default();
        for path in [&unchanged, &changed, &dynamic] {
            let mut report = crate::tests::report(path.clone());
            report.bundle = Bundle::inspect(path).unwrap();
            report.executable = Some(report.bundle.admit(executable).unwrap());
            if path == &dynamic {
                report.discovery.as_mut().unwrap().factory_flags = 8;
            }
            catalog
                .record(path.clone(), Ok((report, "e".repeat(40))))
                .unwrap();
        }
        catalog
            .record_failure(
                failed.clone(),
                "prior failure".into(),
                Some(FailedScan {
                    bundle: Bundle::inspect(&failed).unwrap(),
                    tools_sha256: TOOLS.into(),
                }),
            )
            .unwrap();
        *binary.last_mut().unwrap() = 1;
        fs::write(changed.join(executable), &binary).unwrap();

        let all = [
            unchanged.clone(),
            changed.clone(),
            new.clone(),
            failed.clone(),
            dynamic.clone(),
        ];
        let receipt = "a".repeat(64);
        let helper = "b".repeat(64);
        let tools = ScanTools {
            scan: TOOLS,
            receipt: &receipt,
            helper: &helper,
        };
        let owner = Cancellation::default();
        let mut launches = Vec::new();
        let fast = scan_batch(
            &mut catalog.clone(),
            all.to_vec(),
            &owner.token(),
            &mut None,
            |catalog, path| cached_result(ScanMode::Incremental, catalog, path, tools),
            |path| fake_discovery(path, &mut launches),
        )
        .unwrap();
        assert_eq!(launches, [changed, new, dynamic]);
        assert_eq!((fast.reused, fast.skipped), (1, 1));

        launches.clear();
        let full = scan_batch(
            &mut catalog,
            all.to_vec(),
            &owner.token(),
            &mut None,
            |catalog, path| cached_result(ScanMode::Full, catalog, path, tools),
            |path| fake_discovery(path, &mut launches),
        )
        .unwrap();
        assert_eq!(launches, all);
        assert_eq!((full.reused, full.skipped), (0, 0));
    }

    #[test]
    fn unretired_bundle_is_recorded_without_discarding_results_and_failures_are_skipped() {
        let root = Path::new("/hex-batch-fixture");
        let [good, hanging, licensing, later] =
            ["Good", "Hanging", "Licensing", "Later"].map(|n| root.join(format!("{n}.vst3")));
        let owner = Cancellation::default();
        let token = owner.token();
        let mut staged = Catalog::default();
        let mut diagnostics = None;
        let mut launches = Vec::new();
        let all = vec![
            good.clone(),
            hanging.clone(),
            licensing.clone(),
            later.clone(),
        ];
        let first = scan_batch(
            &mut staged,
            all.clone(),
            &token,
            &mut diagnostics,
            |catalog, path| remembered_failure(catalog, path, 'c', TOOLS),
            |path| fake_discovery(path, &mut launches),
        )
        .expect("batch publishes despite an unretired chain");
        // No launch after the chain that did not retire.
        assert_eq!(launches, [good.clone(), hanging.clone(), licensing.clone()]);
        assert_eq!((first.successful, first.failed, first.skipped), (1, 2, 0));
        assert_eq!(
            first.stopped,
            Some((1, "native cleanup unconfirmed".to_owned()))
        );
        let recorded: Vec<_> = first
            .results
            .iter()
            .map(|r| (&r.path, r.result.is_ok()))
            .collect();
        assert_eq!(
            recorded,
            [(&good, true), (&hanging, false), (&licensing, false)]
        );
        let mut published = Catalog::default();
        for result in &first.results {
            result.apply(&mut published).unwrap();
        }
        let good_rows: Vec<_> = published
            .entries
            .iter()
            .filter(|e| e.last_good.is_some())
            .map(|e| &e.path)
            .collect();
        assert_eq!(good_rows, [&good], "the good bundle is published");
        assert!(published.failed_unchanged(&hanging, &identity(&hanging, 'c'), TOOLS));
        assert!(published.failed_unchanged(&licensing, &identity(&licensing, 'c'), TOOLS));

        // The next scan skips both unchanged failures and reaches the rest.
        launches.clear();
        let second = scan_batch(
            &mut published,
            all.clone(),
            &token,
            &mut diagnostics,
            |catalog, path| remembered_failure(catalog, path, 'c', TOOLS),
            |path| fake_discovery(path, &mut launches),
        )
        .unwrap();
        assert_eq!(launches, [good.clone(), later.clone()]);
        assert_eq!(
            (second.successful, second.failed, second.skipped),
            (2, 0, 2)
        );
        assert!(second.stopped.is_none());

        // A scanner upgrade retries an unchanged bundle.
        launches.clear();
        let new_tools = "b".repeat(64);
        let retried = scan_batch(
            &mut published,
            vec![hanging.clone()],
            &token,
            &mut diagnostics,
            |catalog, path| remembered_failure(catalog, path, 'c', &new_tools),
            |path| {
                let mut result = fake_discovery(path, &mut launches);
                result.failed_scan.as_mut().unwrap().tools_sha256 = new_tools.clone();
                result
            },
        )
        .unwrap();
        assert_eq!(retried.skipped, 0);
        assert_eq!(launches.as_slice(), std::slice::from_ref(&hanging));
        assert!(published.failed_unchanged(&hanging, &identity(&hanging, 'c'), &new_tools));

        // A changed bundle identity is scanned again.
        launches.clear();
        scan_batch(
            &mut published,
            vec![hanging.clone()],
            &token,
            &mut diagnostics,
            |catalog, path| remembered_failure(catalog, path, 'f', &new_tools),
            |path| fake_discovery(path, &mut launches),
        )
        .unwrap();
        assert_eq!(launches, [hanging]);
    }

    #[test]
    fn failure_before_the_scanner_launch_is_not_remembered() {
        let owner = Cancellation::default();
        let token = owner.token();
        let setup = PathBuf::from("/hex-batch-fixture/Setup.vst3");
        let mut staged = Catalog::default();
        let mut launches = Vec::new();
        for _ in 0..2 {
            let batch = scan_batch(
                &mut staged,
                vec![setup.clone()],
                &token,
                &mut None,
                |catalog, path| remembered_failure(catalog, path, 'c', TOOLS),
                |path| fake_discovery(path, &mut launches),
            )
            .unwrap();
            assert_eq!((batch.failed, batch.skipped), (1, 0));
        }
        assert_eq!(launches.len(), 2, "tool failures are retried");
        assert!(staged.entries[0].failure.is_some());
        assert!(staged.entries[0].failed_bundle.is_none());
    }

    #[test]
    fn cancelled_batch_promotes_nothing() {
        let owner = Cancellation::default();
        let token = owner.token();
        assert!(owner.cancel());
        let mut launches = Vec::new();
        let result = scan_batch(
            &mut Catalog::default(),
            vec![PathBuf::from("/hex-batch-fixture/Good.vst3")],
            &token,
            &mut None,
            |_, _| None,
            |path| fake_discovery(path, &mut launches),
        );
        assert!(result.is_err());
        assert!(launches.is_empty());
    }
}
