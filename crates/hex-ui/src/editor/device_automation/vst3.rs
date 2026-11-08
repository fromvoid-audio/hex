//! Copied SDK rows become class-stamped lane entries on the canonical owner.
//! Native admission and metadata refresh stay with Session and its audio sink.
use super::*;

impl Entry {
    pub(super) fn vst3(
        project: &Project,
        generation: u64,
        owner: Owner,
        device: DeviceId,
        row: &PluginParameterRow,
    ) -> Option<Self> {
        let class = row.automation_class?;
        let kind = DeviceAutomationTarget::Vst3 { parameter: row.id };
        kind.validate().ok()?;
        let target = Target {
            generation,
            owner,
            device,
            kind,
            class: devices(project, owner)
                .into_iter()
                .find_map(|(current, class)| (current.id() == device).then_some(class))?,
            provider: Some(hex_lattice_contract::VST3_DEVICE_TYPE_ID),
            vst3_class: Some(class),
        };
        let source = target.current(project, generation)?;
        Some(Self {
            target,
            label: format!("vst3 · {}", hex_session::service::text::verbatim(&row.name)).into(),
            metadata: EntryMetadata::Vst3 { steps: row.steps },
            base: AutomationValue::new(row.normalized as f32).ok()?.get(),
            end: source.automation_target(kind).and_then(automation_extent),
        })
    }
}

impl EditorView {
    /// Keep the touched VST3 target as the main Arrangement preview. This is
    /// transient view state; it never writes authored automation points.
    pub(in crate::editor) fn remember_touched_vst3(
        &mut self,
        owner: Owner,
        device: DeviceId,
        row: &PluginParameterRow,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(entry) = self.state.as_ref().and_then(|state| {
            Entry::vst3(&state.project, self.project_generation, owner, device, row)
        }) else {
            self.automation_view
                .devices
                .main
                .retain(|old| old.target.owner != owner);
            self.automation_view.devices.known.retain(|old| {
                old.target.owner != owner
                    || old.target.device != device
                    || !matches!(old.target.kind, DeviceAutomationTarget::Vst3 { parameter } if parameter == row.id)
            });
            self.automation_view.enabled = true;
            if self.mode == crate::editor::EditorMode::Arrangement {
                self.track_scroll.remeasure();
            }
            cx.notify();
            return false;
        };
        let target = entry.target;
        let lanes = &mut self.automation_view.devices;
        lanes.main.retain(|old| old.target.owner != owner);
        lanes.main.push(entry.clone());
        if let Some(known) = lanes.known.iter_mut().find(|old| old.target == target) {
            *known = entry;
        } else {
            lanes.known.push(entry);
        }
        self.automation_view.enabled = true;
        if self.mode == crate::editor::EditorMode::Arrangement {
            self.track_scroll.remeasure();
        }
        cx.notify();
        true
    }

    #[cfg(feature = "ui-test-support")]
    pub(in crate::editor) fn test_touched_vst3_lane(
        &self,
        owner: Owner,
        device: DeviceId,
        parameter: u32,
    ) -> bool {
        self.automation_view.devices.main.iter().any(|entry| {
            entry.target.owner == owner
                && entry.target.device == device
                && matches!(entry.target.kind, DeviceAutomationTarget::Vst3 { parameter: id } if id == parameter)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice_contract::NativeDeviceState;
    use hex_project::{Command, Created, Editor};

    #[test]
    fn vst3_lane_keeps_the_actual_instrument_or_effect_class_and_raw_zero() {
        // Minimal current envelope from hex-plugin-state's documented fixed header.
        let mut payload = vec![0; 96];
        payload[..4].copy_from_slice(b"HXVS");
        payload[4] = 1;
        payload[8..24].fill(2);
        payload[24..88].fill(3);
        let state =
            NativeDeviceState::new(DeviceTypeId::from_static("hex.vst3"), 1, payload).unwrap();
        let row = PluginParameterRow {
            id: 0,
            name: "gain".into(),
            value: "0.5".into(),
            normalized: 0.5,
            default_normalized: 0.25,
            steps: 0,
            editable: true,
            automation_class: Some([2; 16]),
            page_first: 0,
            metadata_revision: 7,
        };
        for instrument in [false, true] {
            let mut model = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
            let Created::Track(track) = model
                .execute(Command::AddMidiTrack {
                    name: "owner".into(),
                })
                .unwrap()
            else {
                panic!("track")
            };
            let command = if instrument {
                Command::InsertInstrument {
                    track,
                    state: state.clone(),
                }
            } else {
                Command::InsertDevice {
                    track,
                    before: None,
                    state: state.clone(),
                }
            };
            let Created::Device(device) = model.execute(command).unwrap() else {
                panic!("device")
            };
            let entry = Entry::vst3(model.project(), 4, Owner::Track(track), device, &row).unwrap();
            assert_eq!(
                entry.target.kind,
                DeviceAutomationTarget::Vst3 { parameter: 0 }
            );
            assert_eq!(
                entry.target.class,
                if instrument {
                    NativeDeviceClass::Instrument
                } else {
                    NativeDeviceClass::Effect
                }
            );
            assert!(entry.target.current(model.project(), 4).is_some());
            assert!(entry.target.current(model.project(), 5).is_none());
            let mut wrong = row.clone();
            wrong.automation_class = Some([8; 16]);
            assert!(Entry::vst3(model.project(), 4, Owner::Track(track), device, &wrong).is_none());
            wrong.automation_class = None;
            assert!(Entry::vst3(model.project(), 4, Owner::Track(track), device, &wrong).is_none());
            let mut displaced = entry.target;
            displaced.owner = Owner::Main;
            assert!(displaced.current(model.project(), 4).is_none());
            displaced = entry.target;
            displaced.class = if instrument {
                NativeDeviceClass::Effect
            } else {
                NativeDeviceClass::Instrument
            };
            assert!(displaced.current(model.project(), 4).is_none());
        }
    }
}
