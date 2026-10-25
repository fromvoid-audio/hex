//! Exact quiescent-source capture; no source/graph/endpoint authority escapes.
use super::*;

/// Serialized control operation after actual quiescence, never a PCM request.
/// An epoch-local ordinal rejects a prior capture reply at the same PCM prefix.
pub fn capture_request(config: &Configuration, sequence: u64, operation: u64) -> Result<[u8; 72]> {
    if operation == 0 {
        return Err(Error::Protocol("capture operation is missing"));
    }
    let mut out = [0; 72];
    out[..64].copy_from_slice(&quiesce(config, sequence)?);
    out[..4].copy_from_slice(b"HXVC");
    out[64..].copy_from_slice(&operation.to_le_bytes());
    Ok(out)
}

/// Bounds and exact original binding/prefix/operation precede the state copy.
/// Startup capture and canonical state remain unchanged. An absent component
/// is an unsupported capture, never permission to overwrite confirmed state.
pub fn captured_state(
    body: &[u8],
    config: &Configuration,
    sequence: u64,
    operation: u64,
) -> Result<NativeDeviceState> {
    config.validate()?;
    let frames = sequence
        .checked_mul(u64::from(config.quantum))
        .ok_or(Error::Protocol("capture position exhausted"))?;
    let mut reader = Reader::new(body);
    if operation == 0
        || reader.binding()? != config.binding
        || u64::from_le_bytes(reader.array()?) != sequence
        || u64::from_le_bytes(reader.array()?) != frames
        || u64::from_le_bytes(reader.array()?) != operation
    {
        return Err(Error::Protocol(
            "capture acknowledgement differs from original operation",
        ));
    }
    let count = usize::from(u16::from_le_bytes(reader.array()?));
    if count > 4096 {
        return Err(Error::Protocol("captured state exceeds envelope bound"));
    }
    let payload = reader.take(count)?;
    reader.end()?;
    let mut copied = buffer(count)?;
    copied.extend_from_slice(payload);
    let captured = NativeDeviceState::new(TYPE_ID, STATE_VERSION, copied)
        .map_err(|_| Error::Protocol("invalid captured state envelope"))?;
    let state = hex_plugin_state::decode(&captured).map_err(Error::State)?;
    if state.identity != config.identity || state.component.is_none() {
        return Err(Error::Protocol(
            "captured component source identity is unavailable or differs",
        ));
    }
    Ok(captured)
}
