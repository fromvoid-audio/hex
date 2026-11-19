//! Neutral copied parameter data for one foreign processing segment. Provider
//! metadata/ID validation and actual resource admission stay with its host.

/// Sparse parameter/offset pairs in one processing quantum. One target may
/// need separate endpoints at several SDK slices, so this exceeds the target
/// metadata limit. The native VST3 helper mirrors this wire capacity.
pub const MAX_FOREIGN_PARAMETER_POINTS: usize = 128;
/// Additional SDK timing slices in one hosted block, after its opening slice.
pub const MAX_FOREIGN_TIMING_CHANGES: usize = 7;

/// Distinct parameters armed on one foreign source. The control owner reserves
/// their metadata independently of the PCM point queue.
pub const MAX_FOREIGN_AUTOMATION_TARGETS: usize = 64;

/// Sample-relative normalized value using the provider's raw stable ID. Zero
/// is valid for providers such as VST3; this is not a native-device ParameterId
/// or an authored project edit. Offsets are relative to the supplied segment.
/// The host validates finite0..1 values, ordering, capacity and provider flags
/// before admitting them through the same final callback acceptance as PCM.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ForeignParameterPoint {
    pub frame: u16,
    pub id: u32,
    pub normalized: f64,
}
