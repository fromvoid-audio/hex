//! Ordered native controller callback receipts. These are source observations,
//! not authored automation, a canonical edit, or final host audio acceptance.
use super::*;
use std::num::NonZeroU64;
pub const MAX_PARAMETER_EDITS: usize = 8;

/// Effective editor input at frame zero of one successful SDK quantum.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EditorPerform {
    pub sequence: u64,
    pub gesture: u64,
    pub id: u32,
    pub normalized: f64,
}

/// The bounded, ordered batch copied out of that same native reply.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EditorPerforms {
    points: [EditorPerform; MAX_PARAMETER_EDITS],
    len: usize,
}

impl EditorPerforms {
    pub fn as_slice(&self) -> &[EditorPerform] {
        &self.points[..self.len]
    }

    #[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
    pub(crate) fn push(&mut self, point: EditorPerform) -> Result<()> {
        let entry = self
            .points
            .get_mut(self.len)
            .ok_or(Error::Protocol("editor PCM input capacity exceeded"))?;
        *entry = point;
        self.len += 1;
        Ok(())
    }
}

/// One successful SDK reply, including the exact original binding and quantum.
/// Multiple staged editor callbacks to the same parameter at offset zero
/// report only the final one that actually entered the processor.
pub struct EditorPcm {
    pub block: Block,
    pub performs: EditorPerforms,
    pub restorations: EditorRestorations,
    /// Sum of the helper's SDK process calls in this quantum, when requested.
    pub sdk_process_ns: Option<u64>,
}

/// The canonical restoration choice for one original worker target.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EditorTouchPlan {
    pub scope: NonZeroU64,
    pub parameter: u32,
    pub base: f64,
    pub reading: bool,
}

impl EditorTouchPlan {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(crate) fn wire(self) -> Result<[u8; 24]> {
        if self.parameter > i32::MAX as u32 || !(0.0..=1.0).contains(&self.base) {
            return Err(Error::Protocol("invalid editor touch target or base"));
        }
        let mut bytes = [0; 24];
        bytes[..8].copy_from_slice(&self.scope.get().to_le_bytes());
        bytes[8..12].copy_from_slice(&self.parameter.to_le_bytes());
        bytes[12] = u8::from(self.reading);
        bytes[16..].copy_from_slice(&self.base.to_bits().to_le_bytes());
        Ok(bytes)
    }
}

/// One raw ParamID and its prepared read/base restoration choice.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EditorTouchTarget {
    pub parameter: u32,
    pub base: f64,
    pub reading: bool,
}

/// All declared targets of one source-bound group scope. Construction
/// refuses duplicate, unsorted or invalid targets before sending any input.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EditorTouchPlanSet {
    scope: NonZeroU64,
    targets: [EditorTouchTarget; MAX_PARAMETER_EDITS],
    len: usize,
}
impl EditorTouchPlanSet {
    pub fn new(scope: NonZeroU64, targets: &[EditorTouchTarget]) -> Result<Self> {
        if !(2..=MAX_PARAMETER_EDITS).contains(&targets.len())
            || targets
                .windows(2)
                .any(|rows| rows[0].parameter >= rows[1].parameter)
            || targets.iter().any(|target| {
                target.parameter > i32::MAX as u32 || !(0.0..=1.0).contains(&target.base)
            })
        {
            return Err(Error::Protocol("invalid editor touch group"));
        }
        let mut plan = Self {
            scope,
            targets: Default::default(),
            len: targets.len(),
        };
        plan.targets[..plan.len].copy_from_slice(targets);
        Ok(plan)
    }

    pub fn scope(&self) -> NonZeroU64 {
        self.scope
    }

    pub fn targets(&self) -> &[EditorTouchTarget] {
        &self.targets[..self.len]
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(crate) const WIRE_BYTES: usize = 16 + 16 * MAX_PARAMETER_EDITS;

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(crate) fn wire(&self) -> [u8; Self::WIRE_BYTES] {
        let mut bytes = [0; Self::WIRE_BYTES];
        bytes[..8].copy_from_slice(&self.scope.get().to_le_bytes());
        bytes[8] = self.len as u8;
        for (row, target) in bytes[16..].chunks_exact_mut(16).zip(self.targets()) {
            row[..4].copy_from_slice(&target.parameter.to_le_bytes());
            row[4] = u8::from(target.reading);
            row[8..].copy_from_slice(&target.base.to_bits().to_le_bytes());
        }
        bytes
    }
}

/// An actual restored SDK sample following the exact ordered gesture End.
/// The subsequent compensated Main delivery remains a separate witness.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EditorRestoration {
    pub sequence: u64,
    pub gesture: u64,
    pub parameter: u32,
}

/// Effective restorations in the same physical quantum, ordered by the
/// original SDK End sequences, with at most one row for each raw target.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EditorRestorations {
    rows: [EditorRestoration; MAX_PARAMETER_EDITS],
    len: usize,
}
impl EditorRestorations {
    pub fn as_slice(&self) -> &[EditorRestoration] {
        &self.rows[..self.len]
    }

    #[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
    pub(crate) fn push(&mut self, row: EditorRestoration) -> Result<()> {
        *self
            .rows
            .get_mut(self.len)
            .ok_or(Error::Protocol("editor restoration capacity exceeded"))? = row;
        self.len += 1;
        Ok(())
    }
}

// Native main-thread FIFO plus the eight worker handoff slots. Completed
// receipts are tracked separately and do not count toward pending callbacks.
const MAX_PENDING_PARAMETER_EDITS: u16 = 72;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ParameterEditKind {
    #[default]
    Begin,
    Perform,
    End,
    /// The SDK began a timestamp cohort. This marker has no ParamID.
    GroupBegin,
    /// The SDK finished its cohort; per-parameter holds can outlive it.
    GroupEnd,
    /// The cohort was refused; callers must preserve any delivered prefix.
    GroupAborted,
}
impl ParameterEditKind {
    pub fn is_group_marker(self) -> bool {
        matches!(self, Self::GroupBegin | Self::GroupEnd | Self::GroupAborted)
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ParameterEdit {
    pub sequence: u64,
    /// Exact Begin sequence, or GroupBegin sequence for a group marker.
    pub gesture: u64,
    pub normalized: f64,
    /// Raw SDK ParamID including zero. Group markers carry no parameter.
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
/// Request one parameter-only exchange while the transport is idle. The
/// active helper keeps its PCM prefix and returns the usual recovery receipts.
pub fn idle_editor_edit_request(
    config: &Configuration,
    sequence: u64,
    operation: u64,
) -> Result<[u8; 72]> {
    let mut request = edit_request(config, sequence, operation)?;
    request[4..6].copy_from_slice(&3_u16.to_le_bytes());
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
            4 => ParameterEditKind::GroupBegin,
            5 => ParameterEditKind::GroupEnd,
            6 => ParameterEditKind::GroupAborted,
            _ => return Err(Error::Protocol("invalid parameter edit kind")),
        };
        let first_frame = u64::from_le_bytes(reader.array()?);
        if sequence <= last
            || gesture == 0
            || gesture > sequence
            || matches!(
                kind,
                ParameterEditKind::Begin | ParameterEditKind::GroupBegin
            ) != (gesture == sequence)
            || id > i32::MAX as u32
            || (kind.is_group_marker() && id != 0)
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
