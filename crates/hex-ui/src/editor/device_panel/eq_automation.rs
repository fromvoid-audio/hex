//! EQ parameter presentation and axis-aware automation touches.

use super::{EditorView, ParameterKey, bindings::parameter_display_override};
use gpui::{Bounds, Pixels};
use hex_engine::NativeDeviceParameterView;
use hex_lattice_contract::{DeviceAutomationTarget, DeviceTypeId, ParameterId, ParameterValue};
use std::sync::atomic::Ordering::Acquire;

pub(super) fn effective_value(
    view: &mut EditorView,
    key: ParameterKey,
    device_type: DeviceTypeId,
    parameter: &NativeDeviceParameterView,
) -> ParameterValue {
    parameter_display_override(view, key)
        .or_else(|| playback_value(view, key, device_type, parameter))
        .unwrap_or(parameter.value)
}

fn playback_value(
    view: &EditorView,
    key: ParameterKey,
    device_type: DeviceTypeId,
    parameter: &NativeDeviceParameterView,
) -> Option<ParameterValue> {
    let status = view.audio_status()?;
    let source = view.audible_playback.as_ref()?;
    if !status.playing.load(Acquire)
        || source.project_generation != view.project_generation
        || source.audio_revision != status.applied_revision.load(Acquire)
    {
        return None;
    }
    let project = &source.state.project;
    let device = project.device(key.device)?;
    if device.state().type_id() != device_type {
        return None;
    }
    let lane = device.automation_target(DeviceAutomationTarget::Parameter(key.parameter))?;
    if !lane.is_reading() {
        return None;
    }
    let beat = project
        .tempo_map()
        .frame_to_beat(status.position.load(Acquire))
        .ok()?;
    let value = lane.value_at(beat)?;
    parameter
        .semantics?
        .normalization
        .denormalize(parameter.descriptor, value.get())
        .ok()
}

#[derive(Clone, Copy)]
pub(super) struct DragTouch {
    previous: [f32; 2],
    size: [f32; 2],
    last: Option<ParameterId>,
}

impl DragTouch {
    pub(super) fn new(position: [f32; 2], bounds: Option<Bounds<Pixels>>) -> Self {
        Self {
            previous: position,
            size: bounds.map_or([1.0; 2], |bounds| {
                [f32::from(bounds.size.width), f32::from(bounds.size.height)]
            }),
            last: None,
        }
    }

    /// Compare the latest sample in pixels. A stationary mouse-up keeps the
    /// last axis, and a vertical-only drag cannot claim a frequency touch.
    pub(super) fn moved(
        &mut self,
        position: [f32; 2],
        horizontal: Option<ParameterId>,
        vertical: ParameterId,
    ) -> Option<DeviceAutomationTarget> {
        let previous = std::mem::replace(&mut self.previous, position);
        let dx = (position[0] - previous[0]).abs() * self.size[0];
        let dy = (position[1] - previous[1]).abs() * self.size[1];
        let parameter = match horizontal {
            Some(horizontal) if dx > 0.0 && dx >= dy => horizontal,
            _ if dy > 0.0 => vertical,
            _ => return None,
        };
        (self.last.replace(parameter) != Some(parameter))
            .then_some(DeviceAutomationTarget::Parameter(parameter))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_latest_moving_axis_wins_and_stationary_release_keeps_it() {
        let frequency = ParameterId::new(10).unwrap();
        let gain = ParameterId::new(11).unwrap();
        let mut touch = DragTouch::new([0.5, 0.5], None);
        assert_eq!(
            touch.moved([0.6, 0.51], Some(frequency), gain),
            Some(DeviceAutomationTarget::Parameter(frequency))
        );
        assert_eq!(
            touch.moved([0.61, 0.6], Some(frequency), gain),
            Some(DeviceAutomationTarget::Parameter(gain))
        );
        assert_eq!(touch.moved([0.61, 0.6], Some(frequency), gain), None);
        assert_eq!(touch.moved([0.62, 0.6], None, gain), None);
    }
}
