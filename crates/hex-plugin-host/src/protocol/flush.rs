//! Parameter-only transactions at an acknowledged stopped PCM prefix.
use super::*;
use crate::transport::{MAX_PARAMETER_POINTS, Parameters};

pub const MAX_FLUSHED_PARAMETERS: usize = 128;
/// Native zero-frame processing accepts at most this many distinct queues.
pub const MAX_EDITOR_FLUSH_PARAMETERS: usize = 64;
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FlushedParameter {
    pub id: u32,
    pub normalized: f64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlushDisposition {
    Empty,
    Applied,
    /// No host value was written. The successor's editor admission, pending
    /// input, or current writable target set prevented the guarded operation.
    EditorConflict,
}
/// Actual controller readback after the same stopped processor's successful
/// flush. Data only; no canonical edit, resume or graph publication authority.
#[derive(Clone, Copy, Debug)]
pub struct FlushedParameters {
    pub disposition: FlushDisposition,
    values: [FlushedParameter; MAX_FLUSHED_PARAMETERS],
    len: usize,
}
impl FlushedParameters {
    pub fn as_slice(&self) -> &[FlushedParameter] {
        &self.values[..self.len]
    }
}
pub fn flush_request(
    config: &Configuration,
    sequence: u64,
    operation: u64,
    points: &Parameters,
) -> Result<Vec<u8>> {
    encode_flush(config, sequence, operation, points, None)
}

pub fn guarded_flush_request(
    config: &Configuration,
    sequence: u64,
    operation: u64,
    points: &Parameters,
    expected_editor_admission: u64,
) -> Result<Vec<u8>> {
    encode_flush(
        config,
        sequence,
        operation,
        points,
        Some(expected_editor_admission),
    )
}

fn encode_flush(
    config: &Configuration,
    sequence: u64,
    operation: u64,
    points: &Parameters,
    expected_editor_admission: Option<u64>,
) -> Result<Vec<u8>> {
    if operation == 0
        || (expected_editor_admission.is_some() && !valid_editor_flush_extent(points))
        || points.as_slice().len() > MAX_PARAMETER_POINTS
        || points.as_slice().iter().any(|point| point.frame != 0)
    {
        return Err(Error::Protocol("invalid stopped parameter flush"));
    }
    let guarded_extent = if expected_editor_admission.is_some() {
        8
    } else {
        0
    };
    let mut out = buffer(72 + guarded_extent + points.as_slice().len() * 16)?;
    out.extend(quiesce(config, sequence)?);
    out[..4].copy_from_slice(b"HXVF");
    let version: u16 = if expected_editor_admission.is_some() {
        3
    } else {
        2
    };
    out[4..6].copy_from_slice(&version.to_le_bytes());
    out[10..12].copy_from_slice(&(points.as_slice().len() as u16).to_le_bytes());
    out.extend(operation.to_le_bytes());
    if let Some(expected) = expected_editor_admission {
        out.extend(expected.to_le_bytes());
    }
    for point in points.as_slice() {
        out.extend([0; 4]);
        out.extend(point.id.to_le_bytes());
        out.extend(point.normalized.to_le_bytes());
    }
    Ok(out)
}
pub fn flushed_parameters(
    body: &[u8],
    config: &Configuration,
    sequence: u64,
    operation: u64,
    requested: &Parameters,
) -> Result<FlushedParameters> {
    decode_flushed_parameters(body, config, sequence, operation, requested, false)
}

pub(crate) fn valid_editor_flush_extent(parameters: &Parameters) -> bool {
    (1..=MAX_EDITOR_FLUSH_PARAMETERS).contains(&parameters.as_slice().len())
}

pub fn guarded_flushed_parameters(
    body: &[u8],
    config: &Configuration,
    sequence: u64,
    operation: u64,
    requested: &Parameters,
) -> Result<FlushedParameters> {
    if !valid_editor_flush_extent(requested) {
        return Err(Error::Protocol(
            "invalid guarded editor parameter flush extent",
        ));
    }
    decode_flushed_parameters(body, config, sequence, operation, requested, true)
}

fn decode_flushed_parameters(
    body: &[u8],
    config: &Configuration,
    sequence: u64,
    operation: u64,
    requested: &Parameters,
    guarded: bool,
) -> Result<FlushedParameters> {
    config.validate()?;
    let frames = sequence
        .checked_mul(u64::from(config.quantum))
        .ok_or(Error::Protocol("flush position exhausted"))?;
    let mut reader = Reader::new(body);
    if operation == 0
        || reader.binding()? != config.binding
        || u64::from_le_bytes(reader.array()?) != sequence
        || u64::from_le_bytes(reader.array()?) != frames
        || u64::from_le_bytes(reader.array()?) != operation
    {
        return Err(Error::Protocol("stopped flush acknowledgement differs"));
    }
    let disposition = match reader.byte()? {
        0 if requested.as_slice().is_empty() => FlushDisposition::Empty,
        1 => FlushDisposition::Applied,
        2 if guarded && !requested.as_slice().is_empty() => FlushDisposition::EditorConflict,
        _ => return Err(Error::Protocol("invalid stopped flush disposition")),
    };
    let len = usize::from(u16::from_le_bytes(reader.array()?));
    if len > MAX_FLUSHED_PARAMETERS
        || (disposition != FlushDisposition::Applied && len != 0)
        || (disposition == FlushDisposition::Applied && len == 0)
    {
        return Err(Error::Protocol("invalid stopped flush extent"));
    }
    let mut result = FlushedParameters {
        disposition,
        values: [FlushedParameter::default(); MAX_FLUSHED_PARAMETERS],
        len,
    };
    for index in 0..len {
        let id = u32::from_le_bytes(reader.array()?);
        let normalized = f64::from_le_bytes(reader.array()?);
        if id > i32::MAX as u32
            || !normalized.is_finite()
            || !(0.0..=1.0).contains(&normalized)
            || result.values[..index].iter().any(|value| value.id == id)
        {
            return Err(Error::Protocol("invalid stopped flush value"));
        }
        result.values[index] = FlushedParameter { id, normalized };
    }
    reader.end()?;
    if disposition != FlushDisposition::EditorConflict
        && requested
            .as_slice()
            .iter()
            .any(|point| !result.as_slice().iter().any(|value| value.id == point.id))
    {
        return Err(Error::Protocol(
            "stopped flush omitted a requested parameter",
        ));
    }
    Ok(result)
}
