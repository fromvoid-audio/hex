//! Descriptor executable only; enrolled original never-fork Mac profile.
#[path = "macos_cli.rs"]
mod common;
use common::{deadline, finish, supervisor};
use hex_clap_scan::{
    Catalog, ValidRecord, file_bytes,
    receipt_macos::{DESCRIPTOR_NAMES, OperatorRoot, PROFILE},
};
use std::path::PathBuf;
pub fn scan() {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    let mut catalog = Catalog::default();
    let mut output = None;
    let mut receipt_hash = String::new();
    let result = (|| -> Result<ValidRecord, String> {
        if args.len() < 2 || args.len() > 4 {
            return Err("usage: hex-clap-scan <enrolled-original-root> <descriptor-name> [deadline-ms] [previous-report.json]".into());
        }
        if let Some(p) = args.get(3) {
            let value: serde_json::Value =
                serde_json::from_slice(&file_bytes(&PathBuf::from(p), 2 * 1024 * 1024)?)
                    .map_err(|_| "previous report syntax")?;
            catalog = serde_json::from_value(
                value
                    .get("catalog")
                    .cloned()
                    .ok_or("previous catalog missing")?,
            )
            .map_err(|_| "previous catalog syntax")?;
        }
        let root = OperatorRoot::open(&PathBuf::from(&args[0]))?;
        receipt_hash = root.receipt_sha256.clone();
        let name = args[1].to_str().ok_or("fixture name encoding")?;
        if !DESCRIPTOR_NAMES.contains(&name) {
            return Err("only enrolled never-fork descriptors admitted".into());
        }
        let helper = root.image("hex-clap-probe", true)?;
        let module = root.image(name, false)?;
        let (actual, pending) = supervisor::run(&helper, &module, 1, deadline(&args, 2)?, &[])?;
        let actual = match finish(actual, pending) {
            Ok(v) => v,
            Err(failure) => {
                let (e, v) = *failure;
                output = Some(v);
                return Err(e);
            }
        };
        let decoded = match &actual.cause {
            Some(e) => Err(e.clone()),
            None => root
                .unwrap_packet(&actual.packet, "hex-clap-probe")
                .and_then(|p| hex_clap_scan::decode_packet(&p, 1)),
        };
        output = Some(actual);
        root.unchanged()?;
        root.image("hex-clap-probe", true)?;
        root.image(name, false)?;
        Ok(ValidRecord {
            module: name.into(),
            binary_sha256: root
                .receipt
                .modules
                .iter()
                .find(|m| m.name == name)
                .ok_or("module owner missing")?
                .sha256
                .clone(),
            helper_sha256: root
                .receipt
                .helpers
                .get("hex-clap-probe")
                .ok_or("probe owner missing")?
                .clone(),
            native_source_sha256: root.receipt.descriptor_source_sha256.clone(),
            compiler: root.receipt.compiler.clone(),
            discovery: decoded?,
        })
    })();
    let ok = result.is_ok();
    catalog.record(result);
    if let Some(o) = output.as_mut() {
        o.packet.clear();
    }
    println!(
        "{}",
        serde_json::json!({"scope":PROFILE,"catalog":catalog,"receipt_sha256":receipt_hash,"output":output})
    );
    if !ok {
        std::process::exit(2);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scan_adapter_is_binary_only_and_type_checked() {
        let _: fn() = scan;
        assert!(OperatorRoot::open(std::path::Path::new("/tmp/caller-root")).is_err());
        assert!(deadline(&["unused".into(), "unused".into(), "0".into()], 2).is_err());
    }
}
