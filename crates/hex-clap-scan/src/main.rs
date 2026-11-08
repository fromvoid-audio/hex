#[cfg(target_os = "linux")]
use hex_clap_scan::{
    Catalog, Receipt, ValidRecord, binary_identity, controlled_path, decode_packet, file_bytes,
    hash,
};
#[cfg(target_os = "linux")]
use hex_scan_supervisor as supervisor;
#[cfg(target_os = "linux")]
use serde::Serialize;
#[cfg(target_os = "linux")]
use std::{path::PathBuf, time::Duration};
#[cfg(target_os = "linux")]
#[derive(Serialize)]
struct Report {
    scope: &'static str,
    catalog: Catalog,
    receipt_sha256: String,
    #[cfg(target_os = "linux")]
    output: Option<supervisor::Output>,
}
#[cfg(target_os = "linux")]
fn main() {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.len() < 2 || args.len() > 4 {
        eprintln!(
            "usage: hex-clap-scan <controlled-clap-fixtures.json> <fixture-name> [deadline-ms] [previous-report.json]; installed/third-party scanning refused"
        );
        std::process::exit(2);
    }
    let path = PathBuf::from(&args[0]);
    let name = args[1].to_string_lossy();
    let deadline = match args.get(2) {
        None => 10000,
        Some(value) => match value.to_str().and_then(|v| v.parse::<u64>().ok()) {
            Some(value) if (1..=30000).contains(&value) => value,
            _ => {
                eprintln!("deadline must be1..30000 milliseconds");
                std::process::exit(2);
            }
        },
    };
    let mut catalog = Catalog::default();
    if let Some(previous) = args.get(3) {
        match file_bytes(&PathBuf::from(previous), 2 * 1024 * 1024)
            .and_then(|b| {
                serde_json::from_slice::<serde_json::Value>(&b)
                    .map_err(|_| "invalid previous report".into())
            })
            .and_then(|v| {
                serde_json::from_value::<Catalog>(
                    v.get("catalog")
                        .cloned()
                        .ok_or("previous catalog missing")?,
                )
                .map_err(|_| "invalid previous catalog".into())
            }) {
            Ok(value) => catalog = value,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(2);
            }
        }
    }
    let mut report = Report {
        scope: "original-controlled-clap-fixtures",
        catalog,
        receipt_sha256: String::new(),
        #[cfg(target_os = "linux")]
        output: None,
    };
    let result = (|| -> Result<ValidRecord, String> {
        if path.file_name().and_then(|n| n.to_str()) != Some("controlled-clap-fixtures.json") {
            return Err("controlled build receipt required; arbitrary scanning refused".into());
        }
        let root = path.parent().ok_or("fixture root missing")?;
        let receipt_path = controlled_path(root, "controlled-clap-fixtures.json")?;
        let original = file_bytes(&receipt_path, 512 * 1024)?;
        report.receipt_sha256 = hash(&original);
        let receipt = Receipt::load(&receipt_path)?;
        let module = receipt
            .modules
            .iter()
            .find(|m| m.name == name)
            .ok_or("only original controlled fixture names allowed")?;
        let module_path = controlled_path(root, &module.binary)?;
        let helper = controlled_path(root, "hex-clap-probe")?;
        binary_identity(&module_path, &module.sha256)?;
        binary_identity(&helper, &receipt.helper_sha256)?;
        #[cfg(target_os = "linux")]
        {
            let (mut output, mut pending) = supervisor::run(
                &helper,
                &module_path,
                1,
                Duration::from_millis(deadline),
                &[],
                hex_clap_scan::MAX_PACKET,
            )?;
            if let Some(lease) = pending.as_mut() {
                output.packet.clear();
                report.output = Some(output);
                report.catalog.record(Err(
                    "cleanup incomplete: supervisor retains owned lease".into()
                ));
                println!(
                    "{}",
                    serde_json::to_string(&report).expect("bounded report")
                );
                // Never silently drop an unreaped native child. Keep this dedicated owner
                // alive to drive cleanup; total termination is not promised for OS-stuck
                // processes. No further signals target an already-reaped group identity.
                loop {
                    match lease.poll() {
                        Ok(true) => break,
                        Ok(false) => {}
                        Err(error) => eprintln!("cleanup retained: {error}"),
                    };
                    std::thread::sleep(Duration::from_millis(20));
                }
                return Err("cleanup completed after retained failure".into());
            }
            let result = if let Some(error) = output.cause.clone() {
                Err(error)
            } else {
                decode_packet(&output.packet, 1)
            };
            report.output = Some(output);
            if file_bytes(&receipt_path, 512 * 1024)? != original {
                return Err("controlled receipt changed during probe".into());
            }
            binary_identity(&module_path, &module.sha256)?;
            binary_identity(&helper, &receipt.helper_sha256)?;
            let discovery = result?;
            Ok(ValidRecord {
                module: module.name.clone(),
                binary_sha256: module.sha256.clone(),
                helper_sha256: receipt.helper_sha256,
                native_source_sha256: receipt.native_source_sha256,
                compiler: receipt.compiler,
                discovery,
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err("unsupported platform; native Mac CLAP scanner unverified".into())
        }
    })();
    let successful = result.is_ok();
    report.catalog.record(result);
    // Packet bytes are internal evidence; omit them from persistent results to
    // keep metadata/log ownership separate and bounded.
    #[cfg(target_os = "linux")]
    if let Some(output) = report.output.as_mut() {
        output.packet.clear();
    }
    println!(
        "{}",
        serde_json::to_string(&report).expect("bounded report")
    );
    if !successful {
        std::process::exit(2);
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn main() {
    macos_scan::scan();
}
#[cfg(any(all(target_os = "macos", target_arch = "aarch64"), test))]
mod macos_scan;
#[cfg(not(any(target_os = "linux", all(target_os = "macos", target_arch = "aarch64"))))]
fn main() {
    eprintln!(
        "controlled scanner requires linux x86_64 or enrolled original arm64 mac; native mac acceptance pending"
    );
    std::process::exit(2);
}
