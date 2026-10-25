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

/// Start/poll one live main-lane capture through the original serialized reader.
/// The prefix identifies this control exchange, not a sample-exact state boundary.
pub fn live_capture_request(
    config: &Configuration,
    sequence: u64,
    operation: u64,
    poll: bool,
) -> Result<[u8; 72]> {
    let mut out = capture_request(config, sequence, operation)?;
    out[..4].copy_from_slice(if poll { b"HXVP" } else { b"HXVL" });
    Ok(out)
}

/// A live reply is either pending (no payload) or one bounded completed state.
/// Exact poll binding/prefix/ordinal is checked before allocating state storage.
pub fn live_captured_state(
    body: &[u8],
    config: &Configuration,
    sequence: u64,
    operation: u64,
) -> Result<Option<NativeDeviceState>> {
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
        return Err(Error::Protocol("live capture acknowledgement differs"));
    }
    let complete = match reader.byte()? {
        0 => false,
        1 => true,
        _ => return Err(Error::Protocol("invalid live capture disposition")),
    };
    let count = usize::from(u16::from_le_bytes(reader.array()?));
    if count > 4096 || (!complete && count != 0) {
        return Err(Error::Protocol("invalid live capture extent"));
    }
    let payload = reader.take(count)?;
    reader.end()?;
    if !complete {
        return Ok(None);
    }
    copy_captured_state(payload, config).map(Some)
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
    copy_captured_state(payload, config)
}

fn copy_captured_state(payload: &[u8], config: &Configuration) -> Result<NativeDeviceState> {
    let mut copied = buffer(payload.len())?;
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
