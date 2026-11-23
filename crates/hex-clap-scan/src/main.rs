#[cfg(target_os = "linux")]
fn main() -> ! {
    use hex_clap_scan::{Receipt, ValidRecord, decode_packet};
    use hex_scan_supervisor::cli::{LinuxScanConfig, LoadedReceipt, run_linux_cli};

    const CONFIG: LinuxScanConfig<'static> = LinuxScanConfig {
        bin_name: "hex-clap-scan",
        receipt_filename: "controlled-clap-fixtures.json",
        scope: "original-controlled-clap-fixtures",
        helper_name: "hex-clap-probe",
        max_packet: hex_clap_scan::MAX_PACKET,
    };

    run_linux_cli(
        &CONFIG,
        |path| {
            let receipt = Receipt::load(path)?;
            Ok(LoadedReceipt {
                helper_sha256: receipt.helper_sha256,
                native_source_sha256: receipt.native_source_sha256,
                compiler: receipt.compiler,
                modules: receipt.modules,
            })
        },
        decode_packet,
        |module, receipt, discovery| ValidRecord {
            module: module.name.clone(),
            binary_sha256: module.sha256.clone(),
            helper_sha256: receipt.helper_sha256,
            native_source_sha256: receipt.native_source_sha256,
            compiler: receipt.compiler,
            discovery,
        },
    )
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
