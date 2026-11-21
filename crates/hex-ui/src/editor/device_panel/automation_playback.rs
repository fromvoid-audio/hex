//! One display sample per device-panel repaint. The acknowledged project
//! owns the envelope and the audio status owns the audible position.
//! Values live only in the UI and grant no edit or processor authority.
use super::{DeviceControlIdentity, DevicePanelOwner, EditorView, ParameterKey};
use crate::editor::device_action_owner::device_in_panel;
use hex_engine::{NativeDeviceClass, NativeDeviceView};
use hex_lattice_contract::{DeviceAutomationTarget, DeviceTypeId, HostGain, ParameterValue};
use hex_project::{BeatTime, Device, DeviceId, Project};
use std::{collections::HashMap, sync::Arc, sync::atomic::Ordering::Acquire};

struct Frame {
    project: Arc<Project>,
    beat: BeatTime,
}

impl Frame {
    fn capture(view: &EditorView) -> Option<Self> {
        let status = view.audio_status()?;
        let source = view.audible_playback.as_ref()?;
        let applied = status.applied_revision.load(Acquire);
        // The callback can apply a preview before the matching project arrives
        // in GPUI. Continue sampling the last acknowledged project meanwhile.
        if !status.playing.load(Acquire)
            || source.project_generation != view.project_generation
            || source.audio_revision > applied
            || status.device_fault.load(Acquire)
            || status.render_fault.load(Acquire)
            || status.shutdown_complete.load(Acquire)
            || (source.audio_revision < applied
                && status.recording_source().is_some_and(|active| {
                    active.context.project_generation != view.project_generation
                }))
        {
            return None;
        }
        let project = &source.state.project;
        let beat = project
            .tempo_map()
            .frame_to_beat(status.audible_position.load(Acquire))
            .ok()?;
        Some(Self {
            project: Arc::clone(project),
            beat,
        })
    }

    fn device(
        &self,
        owner: DevicePanelOwner,
        id: DeviceId,
        class: NativeDeviceClass,
        type_id: DeviceTypeId,
    ) -> Option<&Device> {
        let device = device_in_panel(owner, &self.project, id, class)?;
        (device.state().type_id() == type_id && !device.bypassed()).then_some(device)
    }
}

#[derive(Default)]
pub(super) struct Values {
    frame: Option<Frame>,
    native: HashMap<ParameterKey, ParameterValue>,
    foreign: HashMap<(DeviceId, u32), ([u8; 16], f32)>,
}

pub(super) fn begin(view: &mut EditorView) {
    let frame = Frame::capture(view);
    let values = &mut view.device_panel.playback_values;
    values.frame = frame;
    values.native.clear();
    values.foreign.clear();
}

/// Resolve ownership once per displayed card, then sample only its active
/// lanes. Persistent widget bindings and graph displays reuse these values.
pub(super) fn sample(
    view: &mut EditorView,
    device: &Device,
    class: NativeDeviceClass,
    metadata: Option<&NativeDeviceView>,
) {
    let Some(owner) = view.device_panel.owner else {
        return;
    };
    if view.device_panel.collapsed.contains(&device.id()) {
        return;
    }
    let values = &mut view.device_panel.playback_values;
    let Some(frame) = &values.frame else {
        return;
    };
    let id = device.id();
    let Some(device) = frame.device(owner, id, class, device.state().type_id()) else {
        return;
    };
    for lane in device.reading_automation() {
        let Some(value) = lane.value_at(frame.beat) else {
            continue;
        };
        match lane.target {
            DeviceAutomationTarget::Parameter(parameter) => {
                if let Some(p) = metadata.and_then(|view| {
                    view.parameters
                        .iter()
                        .find(|p| p.descriptor.id == parameter)
                }) && let Some(semantics) = p.semantics
                    && let Ok(value) = semantics
                        .normalization
                        .denormalize(p.descriptor, value.get())
                {
                    values.native.insert(
                        ParameterKey {
                            device: id,
                            parameter,
                        },
                        value,
                    );
                }
            }
            DeviceAutomationTarget::Vst3 { parameter } => {
                if let Some(class) = lane.vst3_class {
                    values.foreign.insert((id, parameter), (class, value.get()));
                }
            }
            DeviceAutomationTarget::HostInputGain | DeviceAutomationTarget::HostOutputGain => {}
        }
    }
}

impl Values {
    pub(super) fn native(&self, key: ParameterKey) -> Option<ParameterValue> {
        self.native.get(&key).copied()
    }

    pub(super) fn trim(
        &self,
        owner: DevicePanelOwner,
        key: ParameterKey,
        identity: DeviceControlIdentity,
        target: DeviceAutomationTarget,
    ) -> Option<ParameterValue> {
        let frame = self.frame.as_ref()?;
        let device = frame.device(owner, key.device, identity.class, identity.type_id)?;
        let lane = device
            .automation_target(target)
            .filter(|lane| lane.is_reading())?;
        let gain = HostGain::from_normalized(lane.value_at(frame.beat)?.get()).ok()?;
        Some(ParameterValue::Float(gain.db()))
    }

    pub(super) fn vst3(
        &self,
        device: DeviceId,
        parameter: u32,
        class: [u8; 16],
        steps: u32,
    ) -> Option<f64> {
        let (source, value) = self.foreign.get(&(device, parameter))?;
        if *source != class {
            return None;
        }
        let value = f64::from(*value);
        let steps = f64::from(steps);
        Some(if steps == 0.0 {
            value
        } else {
            (value * steps).round() / steps
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor, TrackDeviceSlot};

    #[test]
    fn foreign_paint_checks_the_exact_class_and_stepped_values() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 2).unwrap();
        let Created::Track(track) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "fixture".into(),
                before_track: None,
                slot: TrackDeviceSlot::Effect,
                state: hex_engine::default_native_device_state_for_class(
                    NativeDeviceClass::Effect,
                    DeviceTypeId::from_static("hex.rattle"),
                )
                .unwrap(),
            })
            .unwrap()
        else {
            panic!("track");
        };
        let device = editor.project().track(track).unwrap().devices()[0].id();
        let mut values = Values::default();
        values.foreign.insert((device, 17), ([2; 16], 0.625));
        assert_eq!(values.vst3(device, 17, [2; 16], 0), Some(0.625));
        assert_eq!(values.vst3(device, 17, [2; 16], 2), Some(0.5));
        assert_eq!(values.vst3(device, 17, [3; 16], 2), None);
        assert_eq!(values.vst3(device, 18, [2; 16], 2), None);
    }
}
