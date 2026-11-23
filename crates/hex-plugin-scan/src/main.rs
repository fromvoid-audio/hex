#[cfg(target_os = "linux")]
fn main() -> ! {
    use hex_plugin_scan::{Receipt, ValidRecord, decode_packet};
    use hex_scan_supervisor::cli::{LinuxScanConfig, LoadedReceipt, run_linux_cli};

    const CONFIG: LinuxScanConfig<'static> = LinuxScanConfig {
        bin_name: "hex-plugin-scan",
        receipt_filename: "controlled-fixtures.json",
        scope: "original-controlled-sdk-fixtures",
        helper_name: "hex-vst3-probe",
        max_packet: hex_plugin_scan::MAX_PACKET,
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

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!(
        "controlled scanner currently supports Linux x86_64 only; native Mac/platform acceptance unverified"
    );
    std::process::exit(2);
}
