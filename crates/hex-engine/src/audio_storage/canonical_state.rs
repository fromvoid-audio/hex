//! Selected sealed canonical state slice bodies and actual physical fields.
//! Existing payload/reference domains and inline-state locations are reused.
use super::*;
use hex_lattice_contract::{NativeDeviceState, NativeStateStorageObservation};
use std::{alloc::Layout, sync::atomic::AtomicUsize};

/// The separate three-constructor native requested-layout declaration. Neither
/// counts nor this environmental selector establish custody or allocator RSS.
pub fn canonical_native_state_storage_layout_supported() -> bool {
    let target = env!("HEX_CANONICAL_STATE_ARC_LAYOUT_TARGET");
    env!("HEX_CANONICAL_STATE_ARC_LAYOUT_KNOWN") == "1"
        && env!("HEX_CANONICAL_STATE_ARC_CONSTRUCTOR_FAMILY")
            == "canonical-native-state-vec-slices-empty-array-v1"
        && ((cfg!(all(
            target_arch = "x86_64",
            target_os = "linux",
            target_env = "gnu"
        )) && target == "x86_64-unknown-linux-gnu")
            || (cfg!(all(target_arch = "aarch64", target_os = "macos"))
                && target == "aarch64-apple-darwin"))
}
fn requested(view: &NativeStateStorageObservation<'_>) -> Result<usize, AudioStorageError> {
    let element = view.element_layout();
    let bytes = view
        .len()
        .checked_mul(element.size())
        .ok_or(AudioStorageError::Overflow)?;
    let body =
        Layout::from_size_align(bytes, element.align()).map_err(|_| AudioStorageError::Overflow)?;
    Ok(Layout::array::<AtomicUsize>(2)
        .map_err(|_| AudioStorageError::Overflow)?
        .extend(body)
        .map_err(|_| AudioStorageError::Overflow)?
        .0
        .pad_to_align()
        .size())
}
impl<'a> AudioRootStorageLedger<'a> {
    /// Actual immutable payload/reference allocations, including the fresh empty
    /// payload header. Inline state/Arc fields belong to enclosing storage once.
    /// No input Vec capacity, historical construction peak or root closure is
    /// inferred. Call for every ACTUAL canonical/prepared/history owner field.
    pub fn borrow_canonical_native_state_on_control(
        &mut self,
        state: &'a NativeDeviceState,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| ledger.canonical_native_state(state))
    }
    pub(crate) fn canonical_native_state(
        &mut self,
        state: &'a NativeDeviceState,
    ) -> Result<(), AudioStorageError> {
        if !canonical_native_state_storage_layout_supported() {
            return Err(AudioStorageError::UnknownStorage);
        }
        self.state_field(state, state.payload_field_on_control(), 0)?;
        if let Some(view) = state.asset_reference_field_on_control() {
            self.state_field(state, view, 1)?;
        }
        Ok(())
    }
    /// Actual model current/candidate state fields only. Enclosing Project,
    /// track/device buffers and unrelated children need their own traversal.
    pub fn borrow_project_native_states_on_control(
        &mut self,
        project: &'a hex_project::Project,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| ledger.project_native_states(project))
    }
    pub(crate) fn project_native_states(
        &mut self,
        project: &'a hex_project::Project,
    ) -> Result<(), AudioStorageError> {
        for track in project.tracks() {
            for device in track.devices().iter().chain(track.instrument()) {
                for state in device.authored_states() {
                    self.canonical_native_state(state)?;
                }
            }
        }
        for track in project.return_tracks() {
            for device in track.devices() {
                for state in device.authored_states() {
                    self.canonical_native_state(state)?;
                }
            }
        }
        for device in project.main_devices() {
            for state in device.authored_states() {
                self.canonical_native_state(state)?;
            }
        }
        Ok(())
    }
    /// Visit actual current/Undo/Redo states without synthesizing identities or
    /// fresh prepared copies. This borrow alone does not freeze other roots.
    pub fn borrow_editor_native_states_on_control(
        &mut self,
        editor: &'a hex_project::Editor,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| {
            editor
                .require_no_original_sampler_pending_on_control()
                .map_err(|_| AudioStorageError::UnknownStorage)?;
            for project in editor.retained_projects_on_control() {
                ledger.project_native_states(project)?;
            }
            Ok(())
        })
    }
    pub(crate) fn skip_observed_canonical_state_field(
        &self,
        state: &'a NativeDeviceState,
        index: usize,
    ) -> bool {
        if self.arithmetic.is_some() {
            return false; // state payload/references remain mandatory observations
        }
        let field = Owner::exclusive(state);
        self.handles.iter().any(|row| {
            row.field.same(field) && row.location == Location("inline-state-field", index)
        })
    }
    fn state_field(
        &mut self,
        state: &'a NativeDeviceState,
        view: NativeStateStorageObservation<'a>,
        index: usize,
    ) -> Result<(), AudioStorageError> {
        if view.owning_counts().1 != 0 {
            return Err(AudioStorageError::UnknownStorage);
        }
        let owner = if index == 0 {
            Owner::device_payload(state)
        } else {
            Owner::device_asset_references(state)
        };
        let field = Owner::exclusive(state);
        let location = Location("inline-state-field", index);
        // The sealed fixed struct maps this borrowed state + index to its actual
        // private field; no synthesized state or temporary Arc observation.
        self.bytes(
            owner,
            Location(
                if index == 0 {
                    "inline-state-payload"
                } else {
                    "inline-state-references"
                },
                0,
            ),
            requested(&view)?,
        )?;
        if self.skip_observed_canonical_state_field(state, index) {
            return Ok(());
        }
        self.observe_handle_at(owner, field, location)
    }
}
#[cfg(test)]
use crate::pcm_arc::build_gate_tests as pcm_gate;
#[cfg(test)]
#[path = "../../build_support/state_gate.rs"]
mod state_gate;
#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice_contract::{
        DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, DeviceTypeId,
    };
    fn ledger<'a>(limit: usize) -> AudioRootStorageLedger<'a> {
        AudioRootStorageLedger::try_new_on_control(AudioStorageLedgerReservation {
            data_rows: 32,
            handle_rows: 32,
            root_rows: 4,
            max_retained_bytes: limit,
            max_construction_peak_bytes: limit,
        })
        .unwrap()
    }
    #[test]
    fn canonical_state_header_padding_aliases_and_independent_equal_allocations() {
        assert_ne!(state_gate::FAMILY, "arc-audiodata-retained-vec-v1");
        for (len, expected) in [(0, 48), (1, 56), (7, 56), (8, 56), (9, 64), (4096, 4144)] {
            let make = || {
                NativeDeviceState::new(
                    DeviceTypeId::from_static("test.state-storage"),
                    1,
                    vec![3; len],
                )
                .unwrap()
                .with_asset_references(vec![DeviceAssetReference {
                    slot: DeviceAssetSlot::new(1).unwrap(),
                    asset: DeviceAssetKey::new(1).unwrap(),
                }])
                .unwrap()
            };
            let state = make();
            let alias = state.clone();
            let equal = make();
            let mut ledger = self::ledger(100_000);
            if !canonical_native_state_storage_layout_supported() {
                assert_eq!(
                    ledger.borrow_canonical_native_state_on_control(&state),
                    Err(AudioStorageError::UnknownStorage)
                );
                continue;
            }
            eprintln!(
                "executing supported canonical state three-constructor layout path len={len}"
            );
            // Independent pinned-64-bit requested sizes include both headers
            // and padding, rather than deriving the oracle from the visitor.
            assert_eq!(
                requested(&state.payload_field_on_control()).unwrap()
                    + requested(&state.asset_reference_field_on_control().unwrap()).unwrap(),
                expected
            );
            assert!(expected > len); // headers and padding, including fresh empty payload
            ledger
                .borrow_canonical_native_state_on_control(&state)
                .unwrap();
            assert_eq!(ledger.summary().unwrap().data_bytes, expected);
            assert!(
                ledger
                    .validate_selected_shared_handles_on_control()
                    .is_err()
            );
            ledger
                .borrow_canonical_native_state_on_control(&state)
                .unwrap(); // no alias substitute
            assert!(
                ledger
                    .validate_selected_shared_handles_on_control()
                    .is_err()
            );
            ledger
                .borrow_canonical_native_state_on_control(&alias)
                .unwrap();
            ledger
                .validate_selected_shared_handles_on_control()
                .unwrap();
            ledger
                .borrow_canonical_native_state_on_control(&equal)
                .unwrap();
            assert_eq!(ledger.summary().unwrap().data_bytes, expected * 2);
            assert_eq!(ledger.selected_shared_counts_on_control(), (4, 6));
            assert!(ledger.summary().unwrap().require_complete().is_err());
            // The reservation covers retained DATA plus the ACTUAL ledger
            // backing/header, not just the slice bodies. Set the test budget
            // only after observing this already reserved ledger's capacity.
            let mut exact = self::ledger(100_000);
            let exact_total = exact.summary().unwrap().ledger_bytes + expected;
            exact.reservation.max_retained_bytes = exact_total;
            exact.reservation.max_construction_peak_bytes = exact_total;
            exact
                .borrow_canonical_native_state_on_control(&state)
                .unwrap();
            exact
                .borrow_canonical_native_state_on_control(&alias)
                .unwrap();
            exact.validate_selected_shared_handles_on_control().unwrap();
            assert_eq!(exact.summary().unwrap().data_bytes, expected);
            assert_eq!(exact.summary().unwrap().retained_bytes, exact_total);
            let mut short = self::ledger(100_000);
            let short_total = short.summary().unwrap().ledger_bytes + expected - 1;
            short.reservation.max_retained_bytes = short_total;
            short.reservation.max_construction_peak_bytes = short_total;
            assert!(
                short
                    .borrow_canonical_native_state_on_control(&state)
                    .is_err()
            );
        }
    }
    #[test]
    fn actual_canonical_history_and_candidate_fields_are_required_without_synthetic_owners() {
        use hex_project::{Command, Created, Editor, Project};
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "source".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let device = editor.project().tracks()[0].instrument().unwrap().id();
        editor
            .execute(Command::ReplaceDeviceState {
                device,
                state: hex_project::default_instrument_state()
                    .unwrap()
                    .with_output_gain_millidb(-3000)
                    .unwrap(),
            })
            .unwrap();
        editor
            .execute(Command::RenameTrack {
                track,
                name: "renamed".into(),
            })
            .unwrap();
        assert!(editor.undo()); // actual Redo and Undo both retain canonical aliases
        let candidate = editor.project().clone();
        let mut ledger = self::ledger(100_000);
        if !canonical_native_state_storage_layout_supported() {
            assert_eq!(
                ledger.borrow_editor_native_states_on_control(&editor),
                Err(AudioStorageError::UnknownStorage)
            );
            return;
        }
        eprintln!("executing actual canonical model/history/candidate state fields");
        ledger
            .borrow_project_native_states_on_control(editor.project())
            .unwrap();
        assert!(
            ledger
                .validate_selected_shared_handles_on_control()
                .is_err()
        );
        ledger
            .borrow_editor_native_states_on_control(&editor)
            .unwrap();
        assert!(
            ledger
                .validate_selected_shared_handles_on_control()
                .is_err()
        ); // candidate alias omitted
        ledger
            .borrow_project_native_states_on_control(&candidate)
            .unwrap();
        ledger
            .validate_selected_shared_handles_on_control()
            .unwrap();
        assert_eq!(ledger.summary().unwrap().data_bytes, 32); // two independently born empty payload headers
        assert_eq!(ledger.selected_shared_counts_on_control(), (2, 4));
        assert!(ledger.summary().unwrap().require_complete().is_err());
        drop(ledger);
        drop(editor);
        let mut ledger = self::ledger(100_000);
        let retained = ledger.summary().unwrap().ledger_bytes + 16;
        ledger.reservation.max_retained_bytes = retained;
        ledger.reservation.max_construction_peak_bytes = retained;
        ledger
            .borrow_project_native_states_on_control(&candidate)
            .unwrap();
        ledger
            .validate_selected_shared_handles_on_control()
            .unwrap();
        assert_eq!(ledger.selected_shared_counts_on_control(), (1, 1));
    }
}
