//! Explicit trusted-user-code factory scan; no persistent plugin instances.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "../macos_owner.rs"]
mod owner;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn run() -> i32 {
    use hex_plugin_scan::macos::{build_identity::Build, identity::Bundle, protocol::Attempt};
    use std::{io::Write, path::PathBuf, time::Duration};
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    if !matches!(arguments.len(), 3 | 4) || arguments[1] != "--trust-user-code" {
        let _ = writeln!(
            std::io::stderr(),
            "usage: hex-vst3-scan-macos <controlled-macos.json> --trust-user-code <one.vst3> [deadline-ms]; selected module executes as your user"
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
    let prepared = Build::inspect(&PathBuf::from(&arguments[0])).and_then(|build| {
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
            owner::stdin_cancelled,
        )
    });
    let (report, mut pending) = match result {
        Ok(value) => value,
        Err(error) => {
            let _ = writeln!(std::io::stderr(), "factory scan refused: {error}");
            return 1;
        }
    };
    let mut success = report.error.is_none() && report.discovery.is_some();
    match serde_json::to_vec(&report) {
        Ok(mut bytes) => {
            bytes.push(b'\n');
            if std::io::stdout().write_all(&bytes).is_err() {
                success = false;
            }
        }
        Err(_) => {
            let _ = writeln!(std::io::stderr(), "scan report encoding failed");
            success = false;
        }
    }
    // Only an actually owned, unreaped child is retained here. LostCustody is
    // terminal failure and drops a noncustodial handle without any signals.
    if let Some(lease) = pending.as_mut() {
        loop {
            match lease.poll() {
                Ok(true) => break,
                Ok(false) => {}
                Err(error) => {
                    let _ = writeln!(std::io::stderr(), "retained cleanup failed: {error}");
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        success = false;
    }
    if success { 0 } else { 1 }
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
