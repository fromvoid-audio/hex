//! Two-phase metadata/admission wire; only a terminal frame can carry discovery.
use super::identity::{Bundle, Executable, MAX_PATH_BYTES};
use super::macho::Image;
use crate::{Decoder, Discovery, MAX_PACKET, decode_packet_for_platform};

/// Explicit interactive scanner mode. The supervisor channel must stay open.
pub const WAIT_FOR_PLUGIN_ARG: &str = "--wait-for-plugin";
pub const REQUEST: &[u8; 8] = b"HXMACRQ1";
pub const PREFLIGHT: &[u8; 8] = b"HXMACPF1";
pub const ADMIT: &[u8; 8] = b"HXMACAK1";
pub const RESULT: &[u8; 8] = b"HXMACRS1";
pub const FRAME_LIMIT: usize = MAX_PACKET + 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Attempt {
    pub request: u64,
    pub generation: u64,
}

#[derive(Clone, Debug)]
pub struct Preflight {
    pub executable: String,
    pub identifier: String,
}

fn text(body: &mut Vec<u8>, value: &str, bound: usize) -> Result<(), String> {
    if value.len() > bound || value.contains('\0') {
        return Err("wire text bound or encoding refused".into());
    }
    body.extend_from_slice(&(value.len() as u32).to_le_bytes());
    body.extend_from_slice(value.as_bytes());
    Ok(())
}

fn identity(body: &mut Vec<u8>, attempt: Attempt) -> Result<(), String> {
    if attempt.request == 0 || attempt.generation == 0 {
        return Err("nonzero scan request and generation required".into());
    }
    body.extend_from_slice(&1_u16.to_le_bytes());
    body.extend_from_slice(&attempt.request.to_le_bytes());
    body.extend_from_slice(&attempt.generation.to_le_bytes());
    Ok(())
}

fn check_identity(decoder: &mut Decoder<'_>, expected: Attempt) -> Result<(), String> {
    if decoder.take(2)? != 1_u16.to_le_bytes() {
        return Err("unknown Mac scan protocol version".into());
    }
    let request = u64::from_le_bytes(decoder.take(8)?.try_into().map_err(|_| "request width")?);
    let generation = u64::from_le_bytes(
        decoder
            .take(8)?
            .try_into()
            .map_err(|_| "generation width")?,
    );
    if expected.request == 0
        || expected.generation == 0
        || request != expected.request
        || generation != expected.generation
    {
        return Err("stale Mac scan request/generation".into());
    }
    Ok(())
}

fn frame(magic: &[u8; 8], body: Vec<u8>) -> Result<Vec<u8>, String> {
    if body.len() > MAX_PACKET {
        return Err("Mac scan packet bound refused".into());
    }
    let mut result = Vec::with_capacity(12 + body.len());
    result.extend_from_slice(magic);
    result.extend_from_slice(&(body.len() as u32).to_le_bytes());
    result.extend(body);
    Ok(result)
}

pub fn frame_length(bytes: &[u8]) -> Result<Option<usize>, String> {
    if bytes.len() < 12 {
        return Ok(None);
    }
    let size = u32::from_le_bytes(bytes[8..12].try_into().map_err(|_| "frame width")?) as usize;
    if size > MAX_PACKET {
        return Err("Mac scan frame bound refused".into());
    }
    Ok(Some(size + 12))
}

fn payload<'a>(bytes: &'a [u8], magic: &[u8; 8], attempt: Attempt) -> Result<Decoder<'a>, String> {
    if frame_length(bytes)? != Some(bytes.len()) || bytes.get(..8) != Some(magic.as_slice()) {
        return Err("unexpected, incomplete or trailing Mac scan frame".into());
    }
    let mut decoder = Decoder { bytes, offset: 12 };
    check_identity(&mut decoder, attempt)?;
    Ok(decoder)
}

pub fn request(bundle: &Bundle, attempt: Attempt) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    identity(&mut body, attempt)?;
    text(
        &mut body,
        bundle
            .canonical_path
            .to_str()
            .ok_or("bundle path encoding")?,
        MAX_PATH_BYTES,
    )?;
    text(&mut body, &bundle.info_sha256, 64)?;
    frame(REQUEST, body)
}

pub fn preflight(bytes: &[u8], attempt: Attempt, bundle: &Bundle) -> Result<Preflight, String> {
    let mut decoder = payload(bytes, PREFLIGHT, attempt)?;
    let executable = decoder.text(MAX_PATH_BYTES)?;
    let identifier = decoder.text(256)?;
    let info_sha256 = decoder.text(64)?;
    if executable.is_empty()
        || executable.contains('\0')
        || identifier.is_empty()
        || identifier.contains('\0')
        || info_sha256 != bundle.info_sha256
        || decoder.offset != bytes.len()
    {
        return Err("Mac preflight metadata/identity refused".into());
    }
    Ok(Preflight {
        executable,
        identifier,
    })
}

pub fn admission(
    bundle: &Bundle,
    executable: &Executable,
    attempt: Attempt,
) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    identity(&mut body, attempt)?;
    text(&mut body, &executable.relative_path, MAX_PATH_BYTES)?;
    text(&mut body, &bundle.info_sha256, 64)?;
    text(&mut body, &executable.sha256, 64)?;
    body.extend_from_slice(
        &(match executable.image {
            Image::ThinArm64 => 1_u32,
            Image::UniversalArm64 => 2,
        })
        .to_le_bytes(),
    );
    frame(ADMIT, body)
}

pub fn terminal(bytes: &[u8], attempt: Attempt) -> Result<Discovery, String> {
    let mut decoder = payload(bytes, RESULT, attempt)?;
    if decoder.u32()? != 7 {
        return Err("factory release, bundleExit and unload attempt required".into());
    }
    decode_packet_for_platform(&bytes[decoder.offset..], attempt.request, "darwin-arm64")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn admission_identity_and_ordering_are_not_interchangeable() {
        let attempt = Attempt {
            request: 7,
            generation: 2,
        };
        let bundle = Bundle {
            supplied_path: PathBuf::from("original.vst3"),
            canonical_path: PathBuf::from("/original.vst3"),
            info_sha256: "a".repeat(64),
        };
        let mut body = Vec::new();
        identity(&mut body, attempt).expect("fixture identity");
        text(&mut body, "Contents/MacOS/original", MAX_PATH_BYTES).expect("path");
        text(&mut body, "org.hex.original", 256).expect("identifier");
        text(&mut body, &bundle.info_sha256, 64).expect("hash");
        let valid = frame(PREFLIGHT, body).expect("preflight fixture");
        assert!(preflight(&valid, attempt, &bundle).is_ok());
        assert!(
            preflight(
                &valid,
                Attempt {
                    generation: 3,
                    ..attempt
                },
                &bundle
            )
            .is_err()
        );
        assert!(terminal(&valid, attempt).is_err());
        let mut duplicate = valid.clone();
        duplicate.extend_from_slice(&valid);
        assert!(preflight(&duplicate, attempt, &bundle).is_err());
        let mut wrong_info = bundle;
        wrong_info.info_sha256 = "b".repeat(64);
        assert!(preflight(&valid, attempt, &wrong_info).is_err());
    }

    #[test]
    fn frame_lengths_refuse_unbounded_or_truncated_inputs() {
        assert_eq!(frame_length(b"short"), Ok(None));
        let mut bytes = Vec::from(PREFLIGHT.as_slice());
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(frame_length(&bytes).is_err());
        assert!(
            terminal(
                &bytes,
                Attempt {
                    request: 1,
                    generation: 1
                }
            )
            .is_err()
        );
    }
}
