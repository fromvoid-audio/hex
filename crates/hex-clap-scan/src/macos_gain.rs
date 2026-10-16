//! Gain executable only; owner-bound enrolled original never-fork Mac profile.
#[path = "macos_cli.rs"]
mod common;
use common::{deadline, finish, supervisor};
use hex_clap_scan::{
    file_bytes,
    offline_protocol::{self, Request},
    receipt_macos::{GAIN_NAMES, OperatorRoot, PROFILE},
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct GainOwner {
    profile: String,
    platform: String,
    module_sha256: String,
    helper_sha256: String,
    source_sha256: String,
    sample_rate: u32,
    next_frame: u64,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct GainInput {
    request: Request,
    state_owner: Option<GainOwner>,
}
fn owner(root: &OperatorRoot, name: &str, rate: u32, frame: u64) -> Result<GainOwner, String> {
    Ok(GainOwner {
        profile: PROFILE.into(),
        platform: "macos-arm64".into(),
        module_sha256: root
            .receipt
            .modules
            .iter()
            .find(|m| m.name == name)
            .ok_or("gain owner missing")?
            .sha256
            .clone(),
        helper_sha256: root
            .receipt
            .helpers
            .get("hex-clap-offline-helper")
            .ok_or("gain helper missing")?
            .clone(),
        source_sha256: root.receipt.offline_source_sha256.clone(),
        sample_rate: rate,
        next_frame: frame,
    })
}
fn validate_gain(input: &GainInput, expected: &GainOwner) -> Result<(), String> {
    input.request.validate()?;
    if input.request.state.is_empty() {
        if input.state_owner.is_some() {
            return Err("empty state has no owner".into());
        }
        return Ok(());
    }
    if input.state_owner.as_ref() != Some(expected) || input.request.state.len() != 32 {
        return Err("gain state owner/length refused".into());
    }
    let s = &input.request.state;
    let frames = u64::from_le_bytes(s[12..20].try_into().map_err(|_| "gain state frame")?);
    if frames != input.request.start_frame || s[..4] != 1u32.to_le_bytes() {
        return Err("gain state version/clock refused".into());
    }
    let gain = f64::from_le_bytes(s[4..12].try_into().map_err(|_| "gain state value")?);
    let remaining = u32::from_le_bytes(s[20..24].try_into().map_err(|_| "gain state tail")?);
    let left = f32::from_le_bytes(s[24..28].try_into().map_err(|_| "gain state left")?);
    let right = f32::from_le_bytes(s[28..32].try_into().map_err(|_| "gain state right")?);
    if !gain.is_finite()
        || !(0.0..=2.0).contains(&gain)
        || remaining > 2
        || !left.is_finite()
        || !right.is_finite()
    {
        return Err("gain state value/tail refused".into());
    }
    Ok(())
}
pub fn gain() {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    let mut output = None;
    let mut result = None;
    let mut state_owner = None;
    let attempt = (|| -> Result<(), String> {
        if args.len() < 3 || args.len() > 4 {
            return Err("usage: hex-clap-offline <enrolled-original-root> <gain-name> <gain-input.json> [deadline-ms]".into());
        }
        let root = OperatorRoot::open(&PathBuf::from(&args[0]))?;
        let name = args[1].to_str().ok_or("gain name encoding")?;
        if !GAIN_NAMES.contains(&name) {
            return Err("foreign/MIDI/delay/forking Mac profile refused".into());
        }
        let helper = root.image("hex-clap-offline-helper", true)?;
        let module = root.image(name, false)?;
        let input_path = PathBuf::from(&args[2]);
        let bytes = file_bytes(&input_path, 1024 * 1024)?;
        let input: GainInput =
            serde_json::from_slice(&bytes).map_err(|_| "strict gain-input syntax refused")?;
        let expected = owner(
            &root,
            name,
            input.request.sample_rate,
            input.request.start_frame,
        )?;
        validate_gain(&input, &expected)?;
        let (actual, pending) = supervisor::run(
            &helper,
            &module,
            input.request.request,
            deadline(&args, 3)?,
            &input.request.encode()?,
        )?;
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
                .unwrap_packet(&actual.packet, "hex-clap-offline-helper")
                .and_then(|p| offline_protocol::decode(&p, &input.request)),
        };
        output = Some(actual);
        root.unchanged()?;
        root.image("hex-clap-offline-helper", true)?;
        root.image(name, false)?;
        if file_bytes(&input_path, 1024 * 1024)? != bytes {
            return Err("gain input changed during attempt".into());
        }
        let decoded = decoded?;
        state_owner = Some(owner(&root, name, decoded.sample_rate, decoded.end_frame)?);
        result = Some(decoded);
        Ok(())
    })();
    if let Some(o) = output.as_mut() {
        o.packet.clear();
    }
    println!(
        "{}",
        serde_json::json!({"scope":PROFILE,"platform":"macos-arm64","result":result,"state_owner":state_owner,"error":attempt.as_ref().err(),"output":output})
    );
    if attempt.is_err() {
        std::process::exit(2);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gain_adapter_is_binary_only_and_type_checked() {
        let _: fn() = gain;
        assert!(OperatorRoot::open(std::path::Path::new("/tmp/caller-root")).is_err());
        assert!(deadline(&["unused".into(), "unused".into(), "0".into()], 2).is_err());
    }
    #[test]
    fn gain_continuation_requires_exact_owner_and_valid_state_before_spawn() {
        use offline_protocol::Block;
        let expected = GainOwner {
            profile: PROFILE.into(),
            platform: "macos-arm64".into(),
            module_sha256: "a".repeat(64),
            helper_sha256: "b".repeat(64),
            source_sha256: "c".repeat(64),
            sample_rate: 48000,
            next_frame: 4,
        };
        let mut state = 1u32.to_le_bytes().to_vec();
        state.extend(1f64.to_le_bytes());
        state.extend(4u64.to_le_bytes());
        state.extend(0u32.to_le_bytes());
        state.extend([0; 8]);
        let mut input = GainInput {
            request: Request {
                version: 1,
                request: 1,
                sample_rate: 48000,
                start_frame: 4,
                initial_gain: 1.0,
                state,
                blocks: vec![Block {
                    left: vec![1.0],
                    right: vec![1.0],
                    points: vec![],
                }],
            },
            state_owner: Some(expected.clone()),
        };
        assert!(validate_gain(&input, &expected).is_ok());
        for field in 0..6 {
            let mut changed = expected.clone();
            match field {
                0 => changed.profile = "midi".into(),
                1 => changed.platform = "linux-x86_64".into(),
                2 => changed.module_sha256 = "d".repeat(64),
                3 => changed.helper_sha256 = "d".repeat(64),
                4 => changed.sample_rate = 44100,
                _ => changed.next_frame = 5,
            }
            assert!(validate_gain(&input, &changed).is_err());
        }
        input.state_owner = None;
        assert!(validate_gain(&input, &expected).is_err());
        input.state_owner = Some(expected.clone());
        input.request.state[4..12].copy_from_slice(&f64::NAN.to_le_bytes());
        assert!(validate_gain(&input, &expected).is_err());
        input.request.state[4..12].copy_from_slice(&1f64.to_le_bytes());
        input.request.state[20..24].copy_from_slice(&3u32.to_le_bytes());
        assert!(validate_gain(&input, &expected).is_err());
        input.request.state[20..24].copy_from_slice(&0u32.to_le_bytes());
        input.request.state[24..28].copy_from_slice(&f32::INFINITY.to_le_bytes());
        assert!(validate_gain(&input, &expected).is_err());
        input.request.state.clear();
        input.request.start_frame = 0;
        assert!(validate_gain(&input, &expected).is_err());
        input.state_owner = None;
        assert!(validate_gain(&input, &expected).is_ok());
    }
}
