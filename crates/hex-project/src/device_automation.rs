//! Canonical Arrangement envelopes owned by stable device/parameter identity.
//! The model retains normalized values independently of provider availability.
//! Providers and playback admission validate current parameter capabilities;
//! this neutral owner never interprets device state or constructs DSP. Empty
//! replacement removes an authored envelope: lane visibility and dashed previews
//! are transient UI state, and never override the device's base parameter value.
use crate::{BeatTime, Created, Device, DeviceId, Invalid, Project};
pub use hex_lattice_contract::DeviceAutomationTarget;
use hex_lattice_contract::ParameterId;

pub const MAX_DEVICE_AUTOMATION_LANES: usize = 256;
pub const MAX_DEVICE_AUTOMATION_POINTS_PER_LANE: usize = 1_024;
pub const MAX_DEVICE_AUTOMATION_POINTS: usize = 65_536;

/// A finite normalized parameter value, including a canonical positive zero.
/// Private representation makes Eq sound and prevents NaN entering authored state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AutomationValue(u32);
impl AutomationValue {
    pub fn new(value: f32) -> Result<Self, Invalid> {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(Invalid("automation value must be finite and within 0..1"));
        }
        Ok(Self(if value == 0.0 { 0 } else { value.to_bits() }))
    }
    pub fn get(self) -> f32 {
        f32::from_bits(self.0)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutomationInterpolation {
    Step,
    Linear,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AutomationPoint {
    pub beat: BeatTime,
    pub value: AutomationValue,
    pub outgoing: AutomationInterpolation,
}
/// Persisted read selection, independent of the retained authored points.
/// Base selects the ordinary device value; Authored resumes the envelope at the
/// current content origin. It never stores an effective DSP/modulation value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AutomationReadSelection {
    #[default]
    Authored,
    Base,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceAutomation {
    pub read_selection: AutomationReadSelection,
    pub target: DeviceAutomationTarget,
    pub points: Vec<AutomationPoint>,
}
impl DeviceAutomation {
    pub fn is_reading(&self) -> bool {
        self.read_selection == AutomationReadSelection::Authored
    }

    /// Evaluate a validated normalized envelope on canonical Arrangement time.
    /// Endpoint values hold outside the authored range; a preview has no points
    /// and returns None. This read-only query never allocates or patches DSP.
    pub fn value_at(&self, beat: BeatTime) -> Option<AutomationValue> {
        let first = self.points.first()?;
        let next = self.points.partition_point(|point| point.beat <= beat);
        if next == 0 {
            return Some(first.value);
        }
        let left = self.points.get(next - 1)?;
        let Some(right) = self.points.get(next) else {
            return Some(left.value);
        };
        if left.outgoing == AutomationInterpolation::Step || beat == left.beat {
            return Some(left.value);
        }
        let span = right.beat.raw().checked_sub(left.beat.raw())?;
        if span == 0 {
            return None;
        }
        let elapsed = beat.raw().checked_sub(left.beat.raw())?;
        let fraction = elapsed as f64 / span as f64;
        AutomationValue::new(
            (f64::from(left.value.get())
                + (f64::from(right.value.get()) - f64::from(left.value.get())) * fraction)
                as f32,
        )
        .ok()
    }
}
pub fn validate_device_automation(lanes: &[DeviceAutomation]) -> Result<(), Invalid> {
    if lanes.len() > MAX_DEVICE_AUTOMATION_LANES {
        return Err(Invalid("device automation lane limit reached"));
    }
    let mut previous = None;
    for lane in lanes {
        lane.target
            .validate()
            .map_err(|_| Invalid("invalid automation parameter identity"))?;
        if previous.is_some_and(|target| target >= lane.target) {
            return Err(Invalid(
                "device automation parameters must strictly increase",
            ));
        }
        previous = Some(lane.target);
        validate_points(&lane.points)?;
    }
    Ok(())
}
fn validate_points(points: &[AutomationPoint]) -> Result<(), Invalid> {
    if points.is_empty() || points.len() > MAX_DEVICE_AUTOMATION_POINTS_PER_LANE {
        return Err(Invalid("device automation envelope needs 1..1024 points"));
    }
    if points.windows(2).any(|pair| pair[0].beat >= pair[1].beat) {
        return Err(Invalid("device automation beats must strictly increase"));
    }
    Ok(())
}
impl Device {
    pub(crate) fn validate_automation_provider(
        &self,
        state: &hex_lattice_contract::NativeDeviceState,
    ) -> Result<(), Invalid> {
        if self
            .automation
            .iter()
            .any(|lane| matches!(lane.target, DeviceAutomationTarget::Parameter(_)))
            && self.state.type_id() != state.type_id()
        {
            return Err(Invalid(
                "clear automation before replacing a device provider",
            ));
        }
        Ok(())
    }
    /// Only the selected playback envelopes. Authored inventory and budgets
    /// still include overridden lanes, whose points remain editable and saved.
    pub fn reading_automation(&self) -> impl Iterator<Item = &DeviceAutomation> + Clone {
        self.automation.iter().filter(|lane| lane.is_reading())
    }
    pub fn automation(&self) -> &[DeviceAutomation] {
        &self.automation
    }
    pub fn automation_lane(&self, parameter: ParameterId) -> Option<&DeviceAutomation> {
        self.automation_target(DeviceAutomationTarget::Parameter(parameter))
    }
    pub fn automation_target(&self, target: DeviceAutomationTarget) -> Option<&DeviceAutomation> {
        self.automation
            .binary_search_by_key(&target, |lane| lane.target)
            .ok()
            .map(|index| &self.automation[index])
    }
}
impl Project {
    /// Each canonical device is visited once; immutable track source bindings
    /// are views and do not add duplicate authored envelopes to this inventory.
    pub fn automation_devices(&self) -> impl Iterator<Item = &Device> {
        self.tracks
            .iter()
            .flat_map(|track| track.devices())
            .chain(self.returns.iter().flat_map(|track| track.devices()))
            .chain(self.main_devices())
            .chain(
                self.native_sources
                    .iter()
                    .flat_map(|source| source.devices()),
            )
    }
    pub fn has_overridden_device_automation(&self) -> bool {
        self.automation_devices()
            .any(|device| device.automation().iter().any(|lane| !lane.is_reading()))
    }
    pub fn has_device_automation(&self) -> bool {
        self.automation_devices()
            .any(|device| !device.automation.is_empty())
    }
    pub(crate) fn validate_device_automation(&self) -> Result<(), Invalid> {
        let mut total = 0usize;
        for device in self.automation_devices() {
            validate_device_automation(&device.automation)?;
            for lane in &device.automation {
                total = total
                    .checked_add(lane.points.len())
                    .ok_or(Invalid("automation point count overflow"))?;
                if total > MAX_DEVICE_AUTOMATION_POINTS {
                    return Err(Invalid("project device automation point limit reached"));
                }
            }
        }
        Ok(())
    }
    pub(crate) fn manual_device_edit(
        &mut self,
        device: DeviceId,
        state: hex_lattice_contract::NativeDeviceState,
        targets: Vec<DeviceAutomationTarget>,
    ) -> Result<Created, Invalid> {
        if targets.is_empty()
            || targets.len() > hex_lattice_contract::MAX_PARAMETERS_PER_DEVICE + 2
            || targets.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(Invalid(
                "manual parameter targets must be bounded and strictly ordered",
            ));
        }
        for target in &targets {
            target
                .validate()
                .map_err(|_| Invalid("invalid manual parameter target"))?;
        }
        self.edit_device(device, move |entry| {
            entry.replace_state(state)?;
            for lane in &mut entry.automation {
                if targets.binary_search(&lane.target).is_ok() {
                    lane.read_selection = AutomationReadSelection::Base;
                }
            }
            Ok(())
        })?;
        Ok(Created::None)
    }
    pub(crate) fn set_device_automation_read_selection(
        &mut self,
        device: DeviceId,
        target: DeviceAutomationTarget,
        selection: AutomationReadSelection,
    ) -> Result<Created, Invalid> {
        target
            .validate()
            .map_err(|_| Invalid("invalid automation target identity"))?;
        self.edit_device(device, |entry| {
            let index = entry
                .automation
                .binary_search_by_key(&target, |lane| lane.target)
                .map_err(|_| Invalid("automation lane no longer exists"))?;
            entry.automation[index].read_selection = selection;
            Ok(())
        })?;
        Ok(Created::None)
    }
    pub(crate) fn reenable_device_automation(&mut self) -> Result<Created, Invalid> {
        let count = self
            .automation_devices()
            .filter(|device| device.automation().iter().any(|lane| !lane.is_reading()))
            .count();
        let mut devices = Vec::new();
        devices
            .try_reserve_exact(count)
            .map_err(|_| Invalid("could not reserve automation selection edit"))?;
        devices.extend(
            self.automation_devices()
                .filter(|device| device.automation().iter().any(|lane| !lane.is_reading()))
                .map(Device::id),
        );
        for device in devices {
            self.edit_device(device, |entry| {
                for lane in &mut entry.automation {
                    lane.read_selection = AutomationReadSelection::Authored;
                }
                Ok(())
            })?;
        }
        Ok(Created::None)
    }
    pub(crate) fn replace_device_automation(
        &mut self,
        device: DeviceId,
        parameter: ParameterId,
        points: Vec<AutomationPoint>,
    ) -> Result<Created, Invalid> {
        self.replace_device_automation_target(
            device,
            DeviceAutomationTarget::Parameter(parameter),
            points,
        )
    }
    pub(crate) fn replace_device_automation_target(
        &mut self,
        device: DeviceId,
        target: DeviceAutomationTarget,
        points: Vec<AutomationPoint>,
    ) -> Result<Created, Invalid> {
        target
            .validate()
            .map_err(|_| Invalid("invalid automation target identity"))?;
        if !points.is_empty() {
            validate_points(&points)?;
        }
        self.edit_device(device, move |device| {
            match device
                .automation
                .binary_search_by_key(&target, |lane| lane.target)
            {
                Ok(index) if points.is_empty() => {
                    device.automation.remove(index);
                }
                Ok(index) => device.automation[index].points = points,
                Err(index) if !points.is_empty() => {
                    if device.automation.len() == MAX_DEVICE_AUTOMATION_LANES {
                        return Err(Invalid("device automation lane limit reached"));
                    }
                    device.automation.insert(
                        index,
                        DeviceAutomation {
                            read_selection: crate::AutomationReadSelection::Authored,
                            target,
                            points,
                        },
                    );
                }
                Err(_) => {}
            }
            Ok(())
        })?;
        self.validate_device_automation()?;
        Ok(Created::None)
    }
}

#[cfg(test)]
mod tests;
