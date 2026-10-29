//! Checked host-owned trim value. The integer millidB domain is independent of
//! provider ParameterId/descriptor space and is shared by state, UI and runtime.
use crate::DeviceContractError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostGain(pub(super) i32);
impl HostGain {
    pub const MIN_MILLIDB: i32 = -60_000;
    pub const MAX_MILLIDB: i32 = 24_000;
    pub const MIN_DB: f32 = Self::MIN_MILLIDB as f32 / 1000.0;
    pub const MAX_DB: f32 = Self::MAX_MILLIDB as f32 / 1000.0;
    pub const ZERO: Self = Self(0);
    pub fn from_millidb(value: i32) -> Result<Self, DeviceContractError> {
        if !(Self::MIN_MILLIDB..=Self::MAX_MILLIDB).contains(&value) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        Ok(Self(value))
    }
    pub fn from_db(value: f32) -> Result<Self, DeviceContractError> {
        if !value.is_finite() || !(Self::MIN_DB..=Self::MAX_DB).contains(&value) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        Self::from_millidb((value * 1000.0).round() as i32)
    }
    /// Refuse malformed normalized input; round once in the exact integer
    /// domain rather than stepping a floating-point approximation of 0.001 dB.
    pub fn from_normalized(value: f32) -> Result<Self, DeviceContractError> {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let millidb = (f64::from(Self::MIN_MILLIDB)
            + f64::from(value) * f64::from(Self::MAX_MILLIDB - Self::MIN_MILLIDB))
        .round() as i32;
        Self::from_millidb(millidb)
    }
    pub const fn millidb(self) -> i32 {
        self.0
    }
    pub fn db(self) -> f32 {
        self.0 as f32 / 1000.0
    }
    pub fn normalized(self) -> f32 {
        (self.0 - Self::MIN_MILLIDB) as f32 / (Self::MAX_MILLIDB - Self::MIN_MILLIDB) as f32
    }
    pub fn linear(self) -> f32 {
        10.0_f32.powf(self.db() / 20.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_canonical_millidb_round_trips_and_zero_is_exact_unity() {
        for millidb in HostGain::MIN_MILLIDB..=HostGain::MAX_MILLIDB {
            let gain = HostGain::from_millidb(millidb).unwrap();
            assert_eq!(HostGain::from_normalized(gain.normalized()).unwrap(), gain);
            assert_eq!(HostGain::from_db(gain.db()).unwrap(), gain);
        }
        assert_eq!(
            HostGain::from_normalized(HostGain::ZERO.normalized())
                .unwrap()
                .linear(),
            1.0
        );
        for millidb in [-60_000, -6_000, 0, 3_000, 24_000] {
            let gain = HostGain::from_millidb(millidb).unwrap();
            let state = crate::NativeDeviceState::new(
                crate::DeviceTypeId::from_static("hex.test"),
                1,
                Vec::new(),
            )
            .unwrap()
            .with_input_gain_millidb(millidb)
            .unwrap()
            .with_output_gain_millidb(millidb)
            .unwrap();
            assert_eq!(state.input_gain_linear(), gain.linear());
            assert_eq!(state.output_gain_linear(), gain.linear());
        }
    }
    #[test]
    fn malformed_host_values_refuse_without_provider_id_exceptions() {
        for normalized in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.01, 1.01] {
            assert!(HostGain::from_normalized(normalized).is_err());
        }
        for db in [f32::NAN, f32::INFINITY, -60.001, 24.001] {
            assert!(HostGain::from_db(db).is_err());
        }
        assert!(HostGain::from_millidb(-60_001).is_err());
        assert!(HostGain::from_millidb(24_001).is_err());
        for id in [u32::MAX - 1, u32::MAX] {
            assert!(crate::ParameterId::from_static(id).validate().is_err());
        }
    }
}
