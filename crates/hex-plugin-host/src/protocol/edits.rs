//! Ordered native controller callback receipts. These are source observations,
//! not authored automation, a canonical edit, or final host audio acceptance.
use super::*;
use std::num::NonZeroU64;
pub const MAX_PARAMETER_EDITS: usize = 8;
// Native main-thread FIFO plus the eight worker handoff slots. Completed
// receipts are tracked separately and do not count toward pending callbacks.
const MAX_PENDING_PARAMETER_EDITS: u16 = 72;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ParameterEditKind {
    #[default]
    Begin,
    Perform,
    End,
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ParameterEdit {
    pub sequence: u64,
    pub gesture: u64,
    pub normalized: f64,
    pub id: u32,
    pub kind: ParameterEditKind,
    /// Actual cumulative input frame at which this callback was consumed.
    /// Perform is reported only after successful SDK processing/flush.
    pub first_frame: u64,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct ParameterEdits {
    values: [ParameterEdit; MAX_PARAMETER_EDITS],
    len: usize,
    /// Remaining callbacks across main admission and the worker handoff.
    /// The main producer can admit more immediately after this observation.
    pub pending: u16,
    /// Remaining handoff records at the observation. This is a count, not a
    /// promise that the next record is Perform: main can publish Begin after
    /// this poll has finished draining non-processing records.
    pub ready: u16,
}
impl ParameterEdits {
    pub fn as_slice(&self) -> &[ParameterEdit] {
        &self.values[..self.len]
    }
}
pub fn edit_request(config: &Configuration, sequence: u64, operation: u64) -> Result<[u8; 72]> {
    let mut request = capture_request(config, sequence, operation)?;
    request[..4].copy_from_slice(b"HXVG");
    Ok(request)
}
/// A source-bound edit batch with the latest uncovered latency restart.
/// Repeated reads can return the same ordinal until reactivation covers it.
#[derive(Clone, Copy, Debug)]
pub struct RecoveryEdits {
    pub edits: ParameterEdits,
    pub pending_latency_restart: Option<NonZeroU64>,
}
pub fn recovery_edit_request(
    config: &Configuration,
    sequence: u64,
    operation: u64,
) -> Result<[u8; 72]> {
    let mut request = edit_request(config, sequence, operation)?;
    request[4..6].copy_from_slice(&2_u16.to_le_bytes());
    Ok(request)
}
pub fn recovery_edits(
    body: &[u8],
    config: &Configuration,
    sequence: u64,
    operation: u64,
    previous: (u64, u64),
) -> Result<RecoveryEdits> {
    let (edits, ordinal) = body
        .split_last_chunk::<8>()
        .ok_or(Error::Protocol("missing latency restart ordinal"))?;
    Ok(RecoveryEdits {
        edits: parameter_edits(edits, config, sequence, operation, previous)?,
        pending_latency_restart: NonZeroU64::new(u64::from_le_bytes(*ordinal)),
    })
}
pub fn parameter_edits(
    body: &[u8],
    config: &Configuration,
    sequence: u64,
    operation: u64,
    previous: (u64, u64),
) -> Result<ParameterEdits> {
    config.validate()?;
    let frames = sequence
        .checked_mul(u64::from(config.quantum))
        .ok_or(Error::Protocol("edit position exhausted"))?;
    let mut reader = Reader::new(body);
    if operation == 0
        || reader.binding()? != config.binding
        || u64::from_le_bytes(reader.array()?) != sequence
        || u64::from_le_bytes(reader.array()?) != frames
        || u64::from_le_bytes(reader.array()?) != operation
    {
        return Err(Error::Protocol("parameter edit acknowledgement differs"));
    }
    let pending = u16::from_le_bytes(reader.array()?);
    let ready = u16::from_le_bytes(reader.array()?);
    let len = usize::from(u16::from_le_bytes(reader.array()?));
    if pending > MAX_PENDING_PARAMETER_EDITS
        || usize::from(ready) > MAX_PARAMETER_EDITS
        || ready > pending
        || len > MAX_PARAMETER_EDITS
    {
        return Err(Error::Protocol("parameter edit capacity differs"));
    }
    let mut result = ParameterEdits {
        pending,
        ready,
        len,
        ..ParameterEdits::default()
    };
    let mut last = previous.0;
    let mut last_frame = previous.1;
    for index in 0..len {
        let sequence = u64::from_le_bytes(reader.array()?);
        let gesture = u64::from_le_bytes(reader.array()?);
        let normalized = f64::from_le_bytes(reader.array()?);
        let id = u32::from_le_bytes(reader.array()?);
        let kind = match u32::from_le_bytes(reader.array()?) {
            1 => ParameterEditKind::Begin,
            2 => ParameterEditKind::Perform,
            3 => ParameterEditKind::End,
            _ => return Err(Error::Protocol("invalid parameter edit kind")),
        };
        let first_frame = u64::from_le_bytes(reader.array()?);
        if sequence <= last
            || gesture == 0
            || gesture > sequence
            || (kind == ParameterEditKind::Begin) != (gesture == sequence)
            || id > i32::MAX as u32
            || !normalized.is_finite()
            || !(0.0..=1.0).contains(&normalized)
            || (kind != ParameterEditKind::Perform && normalized != 0.0)
            || first_frame > frames
            || first_frame < last_frame
            || (index > 0 && first_frame < result.values[index - 1].first_frame)
        {
            return Err(Error::Protocol("invalid parameter edit receipt"));
        }
        result.values[index] = ParameterEdit {
            sequence,
            gesture,
            normalized,
            id,
            kind,
            first_frame,
        };
        last = sequence;
        last_frame = first_frame;
    }
    reader.end()?;
    Ok(result)
}
