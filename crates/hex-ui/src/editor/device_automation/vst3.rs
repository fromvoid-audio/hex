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
            class: NativeDeviceClass::Effect,
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

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice_contract::NativeDeviceState;
    use hex_project::{Command, Created, Editor};

    #[test]
    fn exact_vst3_row_keeps_raw_zero_and_refuses_stale_identity() {
        // Current fixed provider envelope documented by hex-plugin-state.
        let mut payload = vec![0; 96];
        payload[..4].copy_from_slice(b"HXVS");
        payload[4] = 1;
        payload[8..24].fill(2);
        payload[24..88].fill(3);
        let state =
            NativeDeviceState::new(DeviceTypeId::from_static("hex.vst3"), 1, payload).unwrap();
        let mut model = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = model
            .execute(Command::AddTrack {
                name: "owner".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Device(device) = model
            .execute(Command::InsertDevice {
                track,
                before: None,
                state,
            })
            .unwrap()
        else {
            panic!("device")
        };
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
        let entry = Entry::vst3(model.project(), 4, Owner::Track(track), device, &row).unwrap();
        assert_eq!(
            entry.target.kind,
            DeviceAutomationTarget::Vst3 { parameter: 0 }
        );
        assert_eq!(entry.target.class, NativeDeviceClass::Effect);
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
    }
}
