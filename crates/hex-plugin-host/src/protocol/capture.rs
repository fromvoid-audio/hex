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
    let count = u32::from_le_bytes(reader.array()?) as usize;
    if count > hex_plugin_state::MAX_ENVELOPE_BYTES || (!complete && count != 0) {
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
    let count = u32::from_le_bytes(reader.array()?) as usize;
    if count > hex_plugin_state::MAX_ENVELOPE_BYTES {
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

/// Reply magic for a bounded, non-terminal refusal of one stopped or live
/// capture. Version 02 adds reason 2 (state still changing); version 01
/// carried only reason 1 and is no longer accepted. Body: binding, PCM
/// sequence, frames, operation, reason byte, then the reason's fields.
pub const CAPTURE_REFUSAL_MAGIC: [u8; 8] = *b"HXVICR02";
const STATE_EXCEEDS_LIMIT: u8 = 1;
const STATE_STILL_CHANGING: u8 = 2;

/// The helper refused one exact capture. Nothing was captured, the processor
/// keeps running and its epoch stays usable; only that operation is retired.
/// Callers keep their last confirmed state; this never permits saving
/// truncated, stale or default state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StateCaptureRefusal {
    /// The plugin's state does not fit the `hex_plugin_state::MAX_OPAQUE_BYTES`
    /// profile (reason 1, followed by a little-endian `u32`).
    ExceedsLimit {
        /// Opaque state bytes the helper observed: always above the limit, and a
        /// lower bound when the helper stopped reading at its capture buffer.
        observed_bytes: u32,
    },
    /// Parameter edits kept arriving, so no consistent component/controller
    /// cut could be read within the helper's bound (reason 2, no fields).
    /// A later capture may succeed once the plugin stops changing.
    StillChanging,
}

impl std::fmt::Display for StateCaptureRefusal {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ExceedsLimit { .. } => write!(
                out,
                "plugin state exceeds the {}-byte limit · not saved · plugin keeps running",
                hex_plugin_state::MAX_OPAQUE_BYTES
            ),
            Self::StillChanging => out.write_str(
                "plugin state kept changing during capture · not saved · plugin keeps running",
            ),
        }
    }
}

/// One live capture start/poll reply: a worker error, a non-terminal refusal
/// ([`Error::CaptureRefused`]), or a pending/completed acknowledgement bound
/// to the exact operation. Any other frame is malformed.
pub fn live_capture_reply(
    magic: &[u8],
    body: &[u8],
    config: &Configuration,
    sequence: u64,
    operation: u64,
) -> Result<Option<NativeDeviceState>> {
    if magic == b"HXVIER01" {
        return Err(Error::Worker(error(body, config.binding)?));
    }
    if magic == CAPTURE_REFUSAL_MAGIC {
        return Err(Error::CaptureRefused(capture_refusal(
            body, config, sequence, operation,
        )?));
    }
    if magic != b"HXVLCA02" {
        return Err(Error::Protocol("missing live capture acknowledgement"));
    }
    live_captured_state(body, config, sequence, operation)
}

/// Exact original binding/prefix/operation and a known reason with valid
/// fields; anything else is malformed and the caller treats it as terminal.
pub fn capture_refusal(
    body: &[u8],
    config: &Configuration,
    sequence: u64,
    operation: u64,
) -> Result<StateCaptureRefusal> {
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
            "capture refusal differs from original operation",
        ));
    }
    match reader.byte()? {
        STATE_EXCEEDS_LIMIT => {
            let observed_bytes = u32::from_le_bytes(reader.array()?);
            reader.end()?;
            if usize::try_from(observed_bytes)
                .is_ok_and(|n| n <= hex_plugin_state::MAX_OPAQUE_BYTES)
            {
                return Err(Error::Protocol(
                    "capture refusal names a state within the limit",
                ));
            }
            Ok(StateCaptureRefusal::ExceedsLimit { observed_bytes })
        }
        STATE_STILL_CHANGING => {
            reader.end()?;
            Ok(StateCaptureRefusal::StillChanging)
        }
        _ => Err(Error::Protocol("unknown capture refusal reason")),
    }
}
