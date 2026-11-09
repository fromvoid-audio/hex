//! Canonical Arrangement envelopes owned by stable device/parameter identity.
//! The model retains normalized values independently of provider availability.
//! Providers and playback admission validate current parameter capabilities;
//! this neutral owner never interprets device state or constructs DSP. Empty
//! replacement removes an authored envelope: lane visibility and dashed previews
//! are transient UI state, and never override the device's base parameter value.
use crate::{BeatTime, Created, Device, DeviceId, Invalid, Project};
pub use hex_lattice_contract::DeviceAutomationTarget;
use hex_lattice_contract::ParameterId;

mod replacement;
pub use replacement::DeviceAutomationReplacement;
mod touch;
pub use touch::{
    DeviceAutomationTouchSpan, DeviceAutomationTouchStep, MAX_DEVICE_AUTOMATION_TOUCH_SPANS,
};

pub const MAX_DEVICE_AUTOMATION_LANES: usize = 256;
pub const MAX_DEVICE_AUTOMATION_POINTS_PER_LANE: usize = 1_024;
/// Recorded touches have an independent lane budget. Authored point limits and
/// the total project budget still apply; this is not a callback event budget.
pub const MAX_DEVICE_AUTOMATION_TOUCH_STEPS_PER_LANE: usize = 16_384;
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
    /// Exact class for a VST3 lane. Native and host lanes carry no class.
    pub vst3_class: Option<[u8; 16]>,
    pub points: Vec<AutomationPoint>,
    pub touch: Vec<DeviceAutomationTouchSpan>,
}
impl DeviceAutomation {
    /// Both parts share the existing validated lane and project point budgets.
    pub fn point_count(&self) -> usize {
        self.points.len().saturating_add(
            self.touch
                .iter()
                .fold(0usize, |total, span| total.saturating_add(span.steps.len())),
        )
    }

    pub fn is_reading(&self) -> bool {
        self.read_selection == AutomationReadSelection::Authored
    }

    /// Evaluate a validated normalized envelope on canonical Arrangement time.
    /// Endpoint values hold outside the authored range; a preview has no points
    /// and returns None. This read-only query never allocates or patches DSP.
    pub fn value_at(&self, beat: BeatTime) -> Option<AutomationValue> {
        if let Some(value) = touch::value_at(&self.touch, beat) {
            return Some(value);
        }
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
        if matches!(lane.target, DeviceAutomationTarget::Vst3 { .. }) != lane.vst3_class.is_some() {
            return Err(Invalid("invalid vst3 automation class stamp"));
        }
        if previous.is_some_and(|target| target >= lane.target) {
            return Err(Invalid(
                "device automation parameters must strictly increase",
            ));
        }
        previous = Some(lane.target);
        if lane.touch.is_empty() || !lane.points.is_empty() {
            validate_points(&lane.points)?;
        }
        touch::validate(&lane.touch)?;
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
pub(crate) fn automation_class(
    target: DeviceAutomationTarget,
    state: &hex_lattice_contract::NativeDeviceState,
) -> Result<Option<[u8; 16]>, Invalid> {
    if !matches!(target, DeviceAutomationTarget::Vst3 { .. }) {
        return Ok(None);
    }
    hex_plugin_state::decode(state)
        .map(|plugin| Some(plugin.identity.class.bytes()))
        .map_err(|_| Invalid("vst3 automation requires a valid vst3 device state"))
}
fn validate_automation_class(
    lane: &DeviceAutomation,
    state: &hex_lattice_contract::NativeDeviceState,
) -> Result<(), Invalid> {
    if lane.vst3_class != automation_class(lane.target, state)? {
        return Err(Invalid("vst3 automation target differs from device class"));
    }
    Ok(())
}
impl Device {
    /// Canonical class stamp for this target, independent of SDK availability.
    /// Native/host targets need no class; VST3 targets require a valid VST3
    /// state. This identity query grants no live capability or capture authority.
    pub fn automation_target_class(
        &self,
        target: DeviceAutomationTarget,
    ) -> Result<Option<[u8; 16]>, Invalid> {
        target
            .validate()
            .map_err(|_| Invalid("invalid automation target identity"))?;
        automation_class(target, self.state())
    }

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
        for lane in &self.automation {
            if !lane.touch.is_empty()
                && state.type_id() == hex_plugin_state::TYPE_ID
                && !matches!(lane.target, DeviceAutomationTarget::Vst3 { .. })
            {
                return Err(Invalid("recorded touch requires a native effect target"));
            }
            validate_automation_class(lane, state)?;
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
// Provider grants are checked by Session and the prepared instrument factory.
fn valid_source_touch(
    device: &Device,
    instrument: DeviceId,
    target: DeviceAutomationTarget,
) -> bool {
    matches!(target, DeviceAutomationTarget::Vst3 { .. })
        || (device.id() == instrument
            && device.state().type_id() != hex_plugin_state::TYPE_ID
            && matches!(target, DeviceAutomationTarget::Parameter(_)))
}

fn splice_touch_lane(
    entry: &mut Device,
    target: DeviceAutomationTarget,
    vst3_class: Option<[u8; 16]>,
    spans: Vec<DeviceAutomationTouchSpan>,
) -> Result<(), Invalid> {
    if automation_class(target, entry.state())? != vst3_class {
        return Err(Invalid("recorded touch differs from device class"));
    }
    match entry
        .automation
        .binary_search_by_key(&target, |lane| lane.target)
    {
        Ok(index) => {
            let lane = &mut entry.automation[index];
            lane.touch = touch::splice(&lane.touch, spans)?;
            lane.read_selection = AutomationReadSelection::Authored;
        }
        Err(index) => {
            if entry.automation.len() == MAX_DEVICE_AUTOMATION_LANES {
                return Err(Invalid("device automation lane limit reached"));
            }
            entry.automation.insert(
                index,
                DeviceAutomation {
                    target,
                    vst3_class,
                    points: Vec::new(),
                    touch: touch::splice(&[], spans)?,
                    read_selection: AutomationReadSelection::Authored,
                },
            );
        }
    }
    Ok(())
}

impl Project {
    /// Each canonical device is visited once; immutable track source bindings
    /// are views and do not add duplicate authored envelopes to this inventory.
    pub fn automation_devices(&self) -> impl Iterator<Item = &Device> {
        self.devices()
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
        if self.native_sources.iter().any(|source| {
            source.devices().any(|device| {
                device.automation().iter().any(|lane| {
                    !lane.touch.is_empty()
                        && !valid_source_touch(device, source.instrument().id(), lane.target)
                })
            })
        }) {
            return Err(Invalid("recorded touch requires a native effect target"));
        }
        let mut total = 0usize;
        for device in self.automation_devices() {
            validate_device_automation(&device.automation)?;
            for lane in &device.automation {
                if !lane.touch.is_empty()
                    && device.state().type_id() == hex_plugin_state::TYPE_ID
                    && !matches!(lane.target, DeviceAutomationTarget::Vst3 { .. })
                {
                    return Err(Invalid("recorded touch requires a native effect target"));
                }
                validate_automation_class(lane, device.state())?;
                total = total
                    .checked_add(lane.points.len())
                    .and_then(|n| {
                        lane.touch
                            .iter()
                            .try_fold(n, |n, span| n.checked_add(span.steps.len()))
                    })
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
    pub(crate) fn splice_device_automation_touch(
        &mut self,
        device: DeviceId,
        target: DeviceAutomationTarget,
        vst3_class: Option<[u8; 16]>,
        spans: Vec<DeviceAutomationTouchSpan>,
    ) -> Result<Created, Invalid> {
        target
            .validate()
            .map_err(|_| Invalid("invalid automation target identity"))?;
        if self.native_sources.iter().any(|source| {
            source.devices().any(|entry| {
                entry.id() == device && !valid_source_touch(entry, source.instrument().id(), target)
            })
        }) {
            return Err(Invalid("recorded touch requires a native effect target"));
        }
        self.edit_device(device, |entry| {
            splice_touch_lane(entry, target, vst3_class, spans)
        })?;
        self.validate_device_automation()?;
        Ok(Created::None)
    }

    /// Use the same candidate for every lane so its preparation and history
    /// can succeed only for the complete group. The wire/worker admits and
    /// clocks the group before Project sees this edit.
    pub(crate) fn splice_vst3_automation_touch_group(
        &mut self,
        device: DeviceId,
        class: [u8; 16],
        lanes: Vec<(u32, Vec<DeviceAutomationTouchSpan>)>,
    ) -> Result<Created, Invalid> {
        const MAX_GROUP_LANES: usize = 8;
        if !(2..=MAX_GROUP_LANES).contains(&lanes.len())
            || lanes.windows(2).any(|pair| pair[0].0 >= pair[1].0)
        {
            return Err(Invalid(
                "vst3 touch group needs distinct ordered parameters",
            ));
        }
        self.edit_device(device, |entry| {
            for (parameter, spans) in lanes {
                let target = DeviceAutomationTarget::Vst3 { parameter };
                target
                    .validate()
                    .map_err(|_| Invalid("invalid grouped vst3 target"))?;
                splice_touch_lane(entry, target, Some(class), spans)?;
            }
            Ok(())
        })?;
        self.validate_device_automation()?;
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
            let vst3_class = automation_class(target, device.state())?;
            match device
                .automation
                .binary_search_by_key(&target, |lane| lane.target)
            {
                Ok(index) if points.is_empty() => {
                    device.automation.remove(index);
                }
                Ok(index) => {
                    if !device.automation[index].touch.is_empty() {
                        return Err(Invalid(
                            "clear recorded touch spans before editing the base curve",
                        ));
                    }
                    device.automation[index].points = points;
                }
                Err(index) if !points.is_empty() => {
                    if device.automation.len() == MAX_DEVICE_AUTOMATION_LANES {
                        return Err(Invalid("device automation lane limit reached"));
                    }
                    device.automation.insert(
                        index,
                        DeviceAutomation {
                            touch: Vec::new(),
                            read_selection: crate::AutomationReadSelection::Authored,
                            target,
                            vst3_class,
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
