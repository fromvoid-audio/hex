//! Standalone controlled Linux offline child owner; never an engine host.
#[cfg(target_os = "linux")]
use hex_scan_supervisor as supervisor;
#[cfg(target_os = "linux")]
fn main() {
    use hex_clap_scan::{
        Receipt, binary_identity, controlled_path, file_bytes, hash,
        offline_protocol::{
            self, BuildReceipt, DELAY_MODES, DelayRequest, MIDI_MODES, MODES, MidiRequest, Request,
        },
    };
    use std::{path::PathBuf, time::Duration};
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.len() < 3 || args.len() > 4 {
        eprintln!(
            "usage: hex-clap-offline <controlled-offline.json> <original-fixture-name> <request.json> [deadline-ms]; third-party/realtime processing refused"
        );
        std::process::exit(2);
    }
    let mut result = None;
    let mut output = None;
    let mut receipt_hash = String::new();
    let mut helper_hash = String::new();
    let mut module_hash = String::new();
    let mut request_hash = String::new();
    let attempt = (|| -> Result<(), String> {
        let path = PathBuf::from(&args[0]);
        if path.file_name().and_then(|s| s.to_str()) != Some("controlled-offline.json") {
            return Err("controlled offline receipt required".into());
        }
        let deadline = match args.get(3) {
            None => 10000,
            Some(v) => v
                .to_str()
                .and_then(|s| s.parse::<u64>().ok())
                .filter(|n| (1..=30000).contains(n))
                .ok_or("deadline must be1..30000 milliseconds")?,
        };
        let root = path.parent().ok_or("fixture root missing")?;
        let manifest = controlled_path(root, "controlled-offline.json")?;
        let scanner_manifest = controlled_path(root, "controlled-clap-fixtures.json")?;
        let scanner_bytes = file_bytes(&scanner_manifest, 512 * 1024)?;
        Receipt::load(&scanner_manifest)?;
        let bytes = file_bytes(&manifest, 64 * 1024)?;
        receipt_hash = hash(&bytes);
        let receipt: BuildReceipt =
            serde_json::from_slice(&bytes).map_err(|_| "invalid offline receipt")?;
        receipt.validate(&scanner_bytes)?;
        let name = args[1].to_str().ok_or("invalid fixture name")?;
        let module = receipt
            .modules
            .iter()
            .find(|m| m.name == name)
            .ok_or("only original controlled offline fixture names allowed")?;
        let module_path = controlled_path(root, &module.binary)?;
        let helper = controlled_path(root, "hex-clap-offline-helper")?;
        binary_identity(&module_path, &module.sha256)?;
        binary_identity(&helper, &receipt.helper_sha256)?;
        helper_hash = receipt.helper_sha256;
        module_hash = module.sha256.clone();
        let request_bytes = file_bytes(&PathBuf::from(&args[2]), 1024 * 1024)?;
        request_hash = hash(&request_bytes);
        let json: serde_json::Value =
            serde_json::from_slice(&request_bytes).map_err(|_| "invalid offline request JSON")?;
        let midi: Option<MidiRequest> = if json.get("version").and_then(|v| v.as_u64()) == Some(2) {
            Some(serde_json::from_value(json.clone()).map_err(|_| "invalid MIDI request JSON")?)
        } else {
            None
        };
        let delay: Option<DelayRequest> = if json.get("version").and_then(|v| v.as_u64()) == Some(3)
        {
            Some(
                serde_json::from_value(json.clone())
                    .map_err(|_| "invalid finite-delay request JSON")?,
            )
        } else {
            None
        };
        let legacy: Option<Request> = if midi.is_none() && delay.is_none() {
            Some(serde_json::from_value(json).map_err(|_| "invalid offline request JSON")?)
        } else {
            None
        };
        let (input, request_id) = if let Some(request) = &delay {
            if !DELAY_MODES.contains(&name) {
                return Err("finite-delay fixture/profile mismatch".into());
            }
            (request.encode_bound(&module_hash)?, request.request)
        } else if let Some(request) = &midi {
            if !MIDI_MODES.contains(&name) {
                return Err("MIDI fixture/profile mismatch".into());
            }
            (request.encode_bound(&module_hash)?, request.request)
        } else {
            if !MODES.contains(&name) {
                return Err("legacy fixture/profile mismatch".into());
            }
            let request = legacy.as_ref().ok_or("request missing")?;
            (request.encode()?, request.request)
        };
        let (mut actual, mut pending) = supervisor::run(
            &helper,
            &module_path,
            request_id,
            Duration::from_millis(deadline),
            &input,
            hex_clap_scan::MAX_PACKET,
        )?;
        if let Some(lease) = pending.as_mut() {
            actual.packet.clear();
            output = Some(actual);
            println!(
                "{}",
                serde_json::json!({"scope":"original-controlled-offline-fixtures","error":"cleanup incomplete: owned lease retained","output":output})
            );
            loop {
                match lease.poll() {
                    Ok(true) => break,
                    Ok(false) => {}
                    Err(e) => eprintln!("cleanup retained: {e}"),
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            return Err("cleanup completed after retained failure".into());
        }
        let decoded = match actual.cause.as_ref() {
            Some(e) => Err(e.clone()),
            None => {
                if let Some(request) = &delay {
                    offline_protocol::decode_delay(&actual.packet, request, &module_hash).and_then(
                        |r| {
                            serde_json::to_value(r)
                                .map_err(|_| "finite-delay result encoding".into())
                        },
                    )
                } else if let Some(request) = &midi {
                    offline_protocol::decode_midi(&actual.packet, request, &module_hash).and_then(
                        |r| serde_json::to_value(r).map_err(|_| "MIDI result encoding".into()),
                    )
                } else {
                    offline_protocol::decode(
                        &actual.packet,
                        legacy.as_ref().ok_or("request missing")?,
                    )
                    .and_then(|r| serde_json::to_value(r).map_err(|_| "result encoding".into()))
                }
            }
        };
        output = Some(actual);
        if file_bytes(&manifest, 64 * 1024)? != bytes
            || file_bytes(&scanner_manifest, 512 * 1024)? != scanner_bytes
            || file_bytes(&PathBuf::from(&args[2]), 1024 * 1024)? != request_bytes
        {
            return Err("offline input/build receipt changed during processing".into());
        }
        binary_identity(&module_path, &module_hash)?;
        binary_identity(&helper, &helper_hash)?;
        result = Some(decoded?);
        Ok(())
    })();
    if let Some(o) = output.as_mut() {
        o.packet.clear();
    }
    println!(
        "{}",
        serde_json::json!({"scope":"original-controlled-offline-fixtures","receipt_sha256":receipt_hash,"helper_sha256":helper_hash,"binary_sha256":module_hash,"request_sha256":request_hash,"source_sha256":offline_protocol::source_hash(),"result":result,"error":attempt.as_ref().err(),"output":output})
    );
    if attempt.is_err() {
        std::process::exit(2);
    }
}
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn main() {
    macos_gain::gain();
}
#[cfg(any(all(target_os = "macos", target_arch = "aarch64"), test))]
#[path = "../macos_gain.rs"]
mod macos_gain;
#[cfg(not(any(target_os = "linux", all(target_os = "macos", target_arch = "aarch64"))))]
fn main() {
    eprintln!(
        "controlled offline fixture requires linux x86_64 or enrolled original arm64 mac; native mac acceptance pending"
    );
    std::process::exit(2);
}
