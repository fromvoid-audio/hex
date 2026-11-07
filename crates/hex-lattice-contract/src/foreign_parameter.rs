//! Neutral copied parameter data for one foreign processing segment. Provider
//! metadata/ID validation and actual resource admission stay with its host.

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
