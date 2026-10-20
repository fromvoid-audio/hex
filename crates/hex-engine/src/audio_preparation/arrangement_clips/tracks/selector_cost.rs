//! Actual persistent selector rows plus genuinely additional constructor cost.
//! No context/provider construction, owning clone, heap scratch or union permit.
use super::*;

#[derive(Clone, Copy, Debug)]
pub(in crate::audio_preparation::arrangement_clips) struct SelectorCost {
    pub(in crate::audio_preparation::arrangement_clips) additional_retained_bytes: usize,
    pub(in crate::audio_preparation::arrangement_clips) additional_peak_bytes: usize,
}
fn add(total: &mut usize, value: usize) -> Result<(), AudioStorageError> {
    *total = total
        .checked_add(value)
        .ok_or(AudioStorageError::Overflow)?;
    Ok(())
}
fn replacement<T>(values: Option<&Vec<T>>, wanted: usize) -> Result<usize, AudioStorageError> {
    if wanted <= values.map_or(0, Vec::capacity) {
        return Ok(0);
    }
    std::alloc::Layout::array::<T>(wanted).map_err(|_| AudioStorageError::Overflow)?;
    wanted
        .checked_mul(size_of::<T>())
        .ok_or(AudioStorageError::Overflow)
}
impl TrackAssembly {
    pub(in crate::audio_preparation::arrangement_clips) fn compiler_selector_cost(
        &self,
        project: &hex_project::Project,
    ) -> Result<SelectorCost, AudioStorageError> {
        self.require_vacant_rows()?;
        crate::gain_envelope::validate_project(project).map_err(invalid)?;
        let mut retained = replacement(Some(&self.rows), project.tracks().len())?;
        let mut peak = retained;
        let (body, header) = OwnedShared::<hex_lattice::DeviceMeter>::declared_parts()
            .ok_or(AudioStorageError::UnsupportedConstructor)?;
        let meter = body
            .checked_add(header)
            .ok_or(AudioStorageError::Overflow)?;
        for (index, track) in project.tracks().iter().enumerate() {
            if !track.clips().is_empty()
                || track
                    .devices()
                    .iter()
                    .any(|device| device.state().type_id() != hex_devices::utility::UTILITY_TYPE_ID)
            {
                return Err(AudioStorageError::UnsupportedGraph);
            }
            let existing = self.rows.get(index);
            let sends = replacement(existing.map(|row| &row.sends), track.sends().len())?;
            let fresh = utility::UtilityCustody::default();
            let utility = existing
                .map_or(&fresh, |row| &row.utility)
                .construction_allowance(
                    track.devices(),
                    hex_lattice::ProcessorChainOwner::Track(track.id().value()),
                )?;
            add(&mut retained, sends)?;
            add(&mut peak, sends)?;
            add(&mut retained, meter)?;
            add(&mut peak, meter)?;
            add(&mut retained, utility.additional_retained_bytes)?;
            add(&mut peak, utility.additional_construction_peak_bytes)?;
        }
        Ok(SelectorCost {
            additional_retained_bytes: retained,
            additional_peak_bytes: peak,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    use hex_project::{Command, Created, Editor};
    fn project(filtered: bool) -> hex_project::Project {
        let mut editor = Editor::new(hex_project::Project::new(48_000).unwrap(), 16).unwrap();
        for _ in 0..3 {
            let Created::Track(track) = editor
                .execute(Command::InsertTrackWithDevice {
                    name: "track".into(),
                    before_track: None,
                    slot: hex_project::TrackDeviceSlot::Instrument,
                    state: hex_project::default_instrument_state()
                        .expect("fixture instrument state"),
                })
                .unwrap()
            else {
                panic!("track")
            };
            editor.execute(Command::RemoveInstrument { track }).unwrap();
            let definition = &hex_devices::utility::UTILITY_DEFINITION;
            let state = definition
                .patch_parameter(
                    &definition.default_state().unwrap(),
                    hex_devices::utility::parameter::DC_REMOVE,
                    ParameterValue::Bool(filtered),
                )
                .unwrap();
            editor
                .execute(Command::InsertDevice {
                    track,
                    before: None,
                    state,
                })
                .unwrap();
        }
        editor.project().clone()
    }
    #[test]
    fn compiler_selector_cost_bounds_actual_rows_meters_and_utility_with_spare_backing() {
        assert!(crate::audio_storage_constructor_layout_supported());
        for filtered in [false, true] {
            for capacity in [0, 1, 7] {
                let project = project(filtered);
                let mut rows = TrackAssembly::default();
                rows.rows.reserve_exact(capacity);
                while rows.rows.len() < capacity {
                    rows.rows.push(Row::default());
                }
                for row in &mut rows.rows {
                    row.sends.reserve_exact(9);
                }
                let before = rows.capacity_at(false, &[]).unwrap().bytes().unwrap();
                let pointer = rows.rows.as_ptr();
                crate::owned_shared::reset_birth_count();
                crate::owned_shared::reset_clone_count();
                let cost = rows.compiler_selector_cost(&project).unwrap();
                assert_eq!(rows.rows.as_ptr(), pointer);
                assert!(rows.phase == Phase::Vacant);
                assert_eq!(crate::owned_shared::birth_count(), 0);
                assert_eq!(crate::owned_shared::clone_count(), 0);
                rows.reserve_compiler_children(&project, None).unwrap();
                let after = rows.capacity_at(true, &[]).unwrap();
                let retained = after.bytes().unwrap();
                let peak = retained + after.previous_growth_backing_bytes;
                assert!(retained <= before + cost.additional_retained_bytes);
                assert!(peak <= before + cost.additional_peak_bytes);
                assert_eq!(rows.rows.capacity(), capacity.max(3));
                assert!(matches!(
                    rows.compiler_selector_cost(&project),
                    Err(AudioStorageError::Capacity)
                ));
                eprintln!(
                    "SUPPORTED selector rows capacity={capacity} filtered={filtered} before={before} retained={retained} peak={peak}"
                );
            }
        }
    }
}
