//! Explicit trusted-user-code factory scan; no persistent plugin instances.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "../macos_owner.rs"]
mod owner;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn run() -> i32 {
    use hex_plugin_scan::macos::{
        build_identity::Build,
        identity::Bundle,
        protocol::{Attempt, WAIT_FOR_PLUGIN_ARG},
    };
    use std::{io::Write, path::PathBuf, time::Duration};
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    let wait_for_plugin = arguments
        .get(4)
        .is_some_and(|value| value == WAIT_FOR_PLUGIN_ARG);
    if !(matches!(arguments.len(), 3 | 4) || arguments.len() == 5 && wait_for_plugin)
        || arguments[1] != "--trust-user-code"
    {
        let _ = writeln!(
            std::io::stderr(),
            "usage: hex-vst3-scan-macos <controlled-macos.json|--bundled> --trust-user-code <one.vst3> [deadline-ms [--wait-for-plugin]]; selected module executes as your user"
        );
        return 2;
    }
    let deadline = match arguments.get(3) {
        None => 10000,
        Some(value) => match value.to_str().and_then(|v| v.parse::<u64>().ok()) {
            Some(value) if (1..=30000).contains(&value) => value,
            _ => {
                let _ = writeln!(std::io::stderr(), "deadline must be1..30000 milliseconds");
                return 2;
            }
        },
    };
    let prepared_build = if arguments[0] == "--bundled" {
        std::env::current_exe()
            .map_err(|e| format!("packaged scanner path unavailable: {e}"))
            .and_then(|path| hex_plugin_scan::macos::package::Package::from_scanner(&path))
            .map(|package| package.build)
    } else {
        Build::inspect(&PathBuf::from(&arguments[0]))
    };
    let prepared = prepared_build.and_then(|build| {
        Bundle::inspect(&PathBuf::from(&arguments[2])).map(|bundle| (build, bundle))
    });
    let result = prepared.and_then(|(build, bundle)| {
        owner::run(
            build,
            bundle,
            Attempt {
                request: 1,
                generation: 1,
            },
            Duration::from_millis(deadline),
            wait_for_plugin,
            || owner::stdin_cancelled(wait_for_plugin),
        )
    });
    let (mut report, mut pending) = match result {
        Ok(value) => value,
        Err(error) => {
            let _ = writeln!(std::io::stderr(), "factory scan refused: {error}");
            return 1;
        }
    };
    let had_pending = pending.is_some();
    let output = retire_then_write(
        &mut report,
        |report| {
            // Retire before serialization or any potentially blocked output.
            // A late exit updates cleanup evidence, never the failed outcome.
            if let Some(lease) = pending.as_mut() {
                loop {
                    match lease.poll() {
                        Ok(true) | Err(_) => break,
                        Ok(false) => std::thread::sleep(Duration::from_millis(20)),
                    }
                }
                report.cleanup = lease.cleanup_snapshot();
            }
        },
        &mut std::io::stdout(),
    );
    let success =
        !had_pending && report.error.is_none() && report.discovery.is_some() && output.is_ok();
    if success { 0 } else { 1 }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn retire_then_write<T: serde::Serialize>(
    report: &mut T,
    retire: impl FnOnce(&mut T),
    output: &mut impl std::io::Write,
) -> std::io::Result<()> {
    retire(report);
    let mut bytes = serde_json::to_vec(report).map_err(std::io::Error::other)?;
    bytes.push(b'\n');
    output.write_all(&bytes)
}

#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
mod output_tests {
    #[test]
    fn retirement_finishes_before_failed_output_without_promoting_failure() {
        use std::{cell::Cell, io};
        #[derive(serde::Serialize)]
        struct Outcome {
            error: Option<&'static str>,
            discovery: Option<&'static str>,
            reaped: bool,
        }
        struct FailedOutput<'a>(&'a Cell<bool>);
        impl io::Write for FailedOutput<'_> {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                assert!(self.0.get(), "output must not delay owned retirement");
                Err(io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let retired = Cell::new(false);
        let mut report = Outcome {
            error: Some("cleanup exceeded initial budget"),
            discovery: None,
            reaped: false,
        };
        let result = super::retire_then_write(
            &mut report,
            |report| {
                report.reaped = true;
                retired.set(true);
            },
            &mut FailedOutput(&retired),
        );
        assert!(result.is_err());
        assert!(report.reaped);
        assert!(report.error.is_some() && report.discovery.is_none());
    }
}

fn main() {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    std::process::exit(run());
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        use std::io::Write;
        let _ = writeln!(
            std::io::stderr(),
            "factory scan requires native macOS arm64; no translation fallback"
        );
        std::process::exit(2);
    }
}
