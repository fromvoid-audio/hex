//! Shared CLI deadline/report parsing and Linux controlled-scanner runner.
use crate::common::{Catalog, Module, file_bytes};
use serde::de::DeserializeOwned;
use std::{ffi::OsStr, path::Path, time::Duration};

pub const DEFAULT_DEADLINE_MS: u64 = 10_000;
pub const MAX_DEADLINE_MS: u64 = 30_000;

pub fn parse_deadline_ms(arg: Option<&OsStr>) -> Result<u64, String> {
    match arg {
        None => Ok(DEFAULT_DEADLINE_MS),
        Some(value) => value
            .to_str()
            .and_then(|text| text.parse::<u64>().ok())
            .filter(|ms| (1..=MAX_DEADLINE_MS).contains(ms))
            .ok_or_else(|| "deadline must be 1..30000 milliseconds".into()),
    }
}

pub fn parse_deadline_duration(arg: Option<&OsStr>) -> Result<Duration, String> {
    parse_deadline_ms(arg).map(Duration::from_millis)
}

pub fn load_previous_catalog<R: DeserializeOwned>(path: &Path) -> Result<Catalog<R>, String> {
    let bytes = file_bytes(path, 2 * 1024 * 1024)?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| "invalid previous report")?;
    let catalog = value
        .get("catalog")
        .cloned()
        .ok_or("previous catalog missing")?;
    serde_json::from_value::<Catalog<R>>(catalog).map_err(|_| "invalid previous catalog".into())
}

pub struct LinuxScanConfig<'a> {
    pub bin_name: &'a str,
    pub receipt_filename: &'a str,
    pub scope: &'a str,
    pub helper_name: &'a str,
    pub max_packet: usize,
}

pub struct LoadedReceipt {
    pub helper_sha256: String,
    pub native_source_sha256: String,
    pub compiler: String,
    pub modules: Vec<Module>,
}

#[cfg(target_os = "linux")]
pub fn run_linux_cli<D, R>(
    config: &LinuxScanConfig<'_>,
    load_receipt: impl FnOnce(&Path) -> Result<LoadedReceipt, String>,
    decode_packet: impl FnOnce(&[u8], u64) -> Result<D, String>,
    build_record: impl FnOnce(&Module, LoadedReceipt, D) -> R,
) -> !
where
    R: serde::Serialize + DeserializeOwned,
{
    use crate::common::{binary_identity, controlled_path, hash};
    use serde::Serialize;
    use std::path::PathBuf;

    #[derive(Serialize)]
    struct Report<'a, R> {
        scope: &'a str,
        catalog: Catalog<R>,
        receipt_sha256: String,
        output: Option<crate::Output>,
    }

    fn print_report<R: Serialize>(report: &Report<'_, R>) {
        match serde_json::to_string(report) {
            Ok(json) => println!("{json}"),
            Err(error) => {
                eprintln!("report serialization failed: {error}");
                std::process::exit(2);
            }
        }
    }

    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.len() < 2 || args.len() > 4 {
        eprintln!(
            "usage: {} <{}> <fixture-name> [deadline-ms] [previous-report.json]; installed/third-party scanning refused",
            config.bin_name, config.receipt_filename
        );
        std::process::exit(2);
    }
    let path = PathBuf::from(&args[0]);
    let name = args[1].to_string_lossy();
    let deadline = match parse_deadline_ms(args.get(2).map(AsRef::as_ref)) {
        Ok(ms) => ms,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    let catalog = match args.get(3) {
        None => Catalog::default(),
        Some(previous) => match load_previous_catalog::<R>(&PathBuf::from(previous)) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(2);
            }
        },
    };
    let mut report = Report {
        scope: config.scope,
        catalog,
        receipt_sha256: String::new(),
        output: None,
    };
    let result = (|| -> Result<R, String> {
        if path.file_name().and_then(|n| n.to_str()) != Some(config.receipt_filename) {
            return Err("controlled build receipt required; arbitrary scanning refused".into());
        }
        let root = path.parent().ok_or("fixture root missing")?;
        let receipt_path = controlled_path(root, config.receipt_filename)?;
        let original = file_bytes(&receipt_path, 512 * 1024)?;
        report.receipt_sha256 = hash(&original);
        let receipt = load_receipt(&receipt_path)?;
        let module = receipt
            .modules
            .iter()
            .find(|m| m.name == name)
            .cloned()
            .ok_or("only original controlled fixture names allowed")?;
        let module_path = controlled_path(root, &module.binary)?;
        let helper = controlled_path(root, config.helper_name)?;
        binary_identity(&module_path, &module.sha256)?;
        binary_identity(&helper, &receipt.helper_sha256)?;
        let (mut output, mut pending) = crate::run(
            &helper,
            &module_path,
            1,
            Duration::from_millis(deadline),
            &[],
            config.max_packet,
        )?;
        if let Some(lease) = pending.as_mut() {
            output.packet.clear();
            report.output = Some(output);
            report.catalog.record(Err(
                "cleanup incomplete: supervisor retains owned lease".into()
            ));
            print_report(&report);
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
        Ok(build_record(&module, receipt, discovery))
    })();
    let successful = result.is_ok();
    report.catalog.record(result);
    // Packet bytes are internal evidence; omit them from persistent results to
    // keep metadata/log ownership separate and bounded.
    if let Some(output) = report.output.as_mut() {
        output.packet.clear();
    }
    print_report(&report);
    if !successful {
        std::process::exit(2);
    }
    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        ffi::OsString,
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn parse_deadline_ms_enforces_range_and_clean_error_text() {
        assert_eq!(parse_deadline_ms(None).unwrap(), 10_000);
        assert_eq!(parse_deadline_ms(Some(OsStr::new("1"))).unwrap(), 1);
        assert_eq!(
            parse_deadline_ms(Some(OsStr::new("30000"))).unwrap(),
            30_000
        );
        for bad in ["0", "30001", "abc", "-5"] {
            assert_eq!(
                parse_deadline_ms(Some(OsStr::new(bad))).unwrap_err(),
                "deadline must be 1..30000 milliseconds"
            );
        }
        let raw = OsString::from("1500");
        assert_eq!(
            parse_deadline_duration(Some(raw.as_os_str())).unwrap(),
            Duration::from_millis(1500)
        );
    }

    #[test]
    fn load_previous_catalog_reads_valid_and_rejects_malformed() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!(
            "hex-scan-supervisor-cli-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();

        let valid = dir.join("prev.json");
        fs::write(&valid, r#"{"catalog":{"last_valid":42,"last_error":null}}"#).unwrap();
        let catalog = load_previous_catalog::<u32>(&valid).unwrap();
        assert_eq!(catalog.last_valid, Some(42));
        assert_eq!(catalog.last_error, None);

        let missing = dir.join("missing.json");
        fs::write(&missing, r#"{"other":1}"#).unwrap();
        assert_eq!(
            load_previous_catalog::<u32>(&missing).unwrap_err(),
            "previous catalog missing"
        );

        let invalid = dir.join("invalid.json");
        fs::write(&invalid, b"not json").unwrap();
        assert_eq!(
            load_previous_catalog::<u32>(&invalid).unwrap_err(),
            "invalid previous report"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
