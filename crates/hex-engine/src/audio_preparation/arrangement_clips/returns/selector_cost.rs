//! Actual persistent Return selector cost; no fresh active compiler plan.
use super::*;
#[derive(Clone, Copy, Debug)]
pub(in crate::audio_preparation::arrangement_clips) struct SelectorCost {
    pub(in crate::audio_preparation::arrangement_clips) additional_retained_bytes: usize,
    pub(in crate::audio_preparation::arrangement_clips) additional_peak_bytes: usize,
}
impl ReturnAssembly {
    pub(in crate::audio_preparation::arrangement_clips) fn selector_construction_cost(
        &self,
        project: &hex_project::Project,
    ) -> Result<SelectorCost, AudioStorageError> {
        self.require_vacant_rows()?;
        let wanted = project.return_tracks().len();
        if wanted > hex_project::MAX_RETURN_TRACKS {
            return Err(AudioStorageError::UnsupportedGraph);
        }
        let outer = if wanted > self.rows.capacity() {
            std::alloc::Layout::array::<Row>(wanted).map_err(|_| AudioStorageError::Overflow)?;
            wanted
                .checked_mul(size_of::<Row>())
                .ok_or(AudioStorageError::Overflow)?
        } else {
            0
        };
        let mut retained = outer
            .checked_add(self.feed.construction_cost(project)?)
            .ok_or(AudioStorageError::Overflow)?;
        let mut peak = retained;
        for (index, bus) in project.return_tracks().iter().enumerate() {
            let fresh = UtilityCustody::default();
            let utility = self
                .rows
                .get(index)
                .map_or(&fresh, |row| &row.utility)
                .construction_allowance(
                    bus.devices(),
                    hex_lattice::ProcessorChainOwner::Return(bus.id().value()),
                )?;
            retained = retained
                .checked_add(utility.additional_retained_bytes)
                .ok_or(AudioStorageError::Overflow)?;
            peak = peak
                .checked_add(utility.additional_construction_peak_bytes)
                .ok_or(AudioStorageError::Overflow)?;
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
    fn project(filtered: bool, routed: bool) -> hex_project::Project {
        let mut editor = Editor::new(hex_project::Project::new(48_000).unwrap(), 16).unwrap();
        let mut ids = [None; 3];
        for id in &mut ids {
            let Created::ReturnTrack(bus) = editor
                .execute(Command::AddReturnTrack { name: "bus".into() })
                .unwrap()
            else {
                panic!("return")
            };
            *id = Some(bus);
            let definition = &hex_devices::utility::UTILITY_DEFINITION;
            let state = definition
                .patch_parameter(
                    &definition.default_state().unwrap(),
                    hex_devices::utility::parameter::DC_REMOVE,
                    ParameterValue::Bool(filtered),
                )
                .unwrap();
            editor
                .execute(Command::InsertReturnDevice {
                    return_track: bus,
                    before: None,
                    state,
                })
                .unwrap();
        }
        if routed {
            hex_project::Project::from_snapshot(
                editor
                    .project()
                    .snapshot()
                    .with_return_feed(ids[0].unwrap(), ids[1].unwrap(), 0.5)
                    .unwrap()
                    .with_return_feed(ids[1].unwrap(), ids[2].unwrap(), 0.3)
                    .unwrap(),
            )
            .unwrap()
        } else {
            editor.project().clone()
        }
    }
    #[test]
    fn return_selector_cost_bounds_actual_rows_utility_and_paired_feed_separately() {
        assert!(crate::audio_storage_constructor_layout_supported());
        for filtered in [false, true] {
            for routed in [false, true] {
                for capacity in [0, 1, 7] {
                    let project = project(filtered, routed);
                    let mut rows = ReturnAssembly::default();
                    rows.rows.reserve_exact(capacity);
                    while rows.rows.len() < capacity {
                        rows.rows.push(Row::default());
                    }
                    let before = rows.capacity_at(false, &[], None).unwrap().bytes().unwrap();
                    let pointer = rows.rows.as_ptr();
                    crate::owned_shared::reset_birth_count();
                    crate::owned_shared::reset_clone_count();
                    let cost = rows.selector_construction_cost(&project).unwrap();
                    assert_eq!(rows.rows.as_ptr(), pointer);
                    assert!(rows.phase == Phase::Vacant);
                    assert_eq!(crate::owned_shared::birth_count(), 0);
                    assert_eq!(crate::owned_shared::clone_count(), 0);
                    rows.reserve(&project).unwrap();
                    rows.construct().unwrap();
                    let actual = rows.capacity_at(true, &[], None).unwrap();
                    let retained = actual.bytes().unwrap();
                    let peak = retained + actual.previous_growth_backing_bytes;
                    assert!(retained <= before + cost.additional_retained_bytes);
                    assert!(peak <= before + cost.additional_peak_bytes);
                    assert_eq!(rows.rows.capacity(), capacity.max(3));
                    let fresh = crate::return_feed_plan::ReturnFeedPlan::prepare(&project).unwrap();
                    assert_eq!(fresh.edges().count(), if routed { 2 } else { 0 });
                    // Distinct equal-content plans coexist: neither quote can
                    // stand in for the other physical plan's retained backing.
                    let fresh_bytes = fresh.backing_bytes().unwrap();
                    assert_eq!(routed, fresh_bytes > 0);
                    assert!(matches!(
                        rows.selector_construction_cost(&project),
                        Err(AudioStorageError::Capacity)
                    ));
                    eprintln!(
                        "SUPPORTED Return selector capacity={capacity} filtered={filtered} routed={routed} retained={retained} peak={peak} separatefresh={fresh_bytes}"
                    );
                }
            }
        }
    }
}
