//! Bounded new metadata for the actual closed Sampler table constructor.
//! Existing resources/previous contexts and the full old/new union are separate.
use super::*;
use crate::owned_shared::OwnedShared;
use std::mem::size_of;

#[derive(Clone, Copy, Debug)]
pub(super) struct Quote {
    retained_bytes: usize,
    construction_peak_bytes: usize,
}
impl Quote {
    pub(super) fn limits(self) -> SamplerContextConstructionLimits {
        SamplerContextConstructionLimits {
            retained_bytes: self.retained_bytes,
            construction_peak_bytes: self.construction_peak_bytes,
        }
    }
}

/// Reads canonical states and constructor ceilings only; no owner clone, codec
/// allocation, provider call, resource lease or context identity is acquired.
pub(super) fn quote(
    project: &Project,
    resources: Option<crate::native_device_resources::DeviceResourcePreparation<'_>>,
) -> Result<Quote, Invalid> {
    let overflow = || Invalid("sampler context construction allowance overflow");
    if project.tracks().len() > MAX_SHARED_CONTEXTS_PER_REVISION {
        return Err(Invalid("shared instrument context count limit reached"));
    }
    let mut count = 0usize;
    for slot in project
        .tracks()
        .iter()
        .filter_map(|track| track.instrument())
    {
        // The opt-in route is closed to the actual built-in Sampler branch.
        // The common legacy constructor keeps its existing provider behavior.
        if slot.state().type_id() != hex_devices::sampler::state::TYPE_ID {
            return Err(Invalid("sampler context construction class is unsupported"));
        }
        hex_devices::sampler::state::DeviceStateRead::from_native(slot.state())
            .map_err(|_| Invalid("sampler state is not a current configuration"))?;
        if !slot.state().asset_references().is_empty() && resources.is_none() {
            return Err(Invalid("instrument resource source plan is unavailable"));
        }
        count = count.checked_add(1).ok_or_else(overflow)?;
    }
    let (body, header) = OwnedShared::<SharedAnalyticContextTable>::declared_parts()
        .ok_or(Invalid("sampler context constructor layout is unsupported"))?;
    let row = size_of::<SharedContextLease>()
        .checked_add(size_of::<(SharedContextId, u16)>())
        .and_then(|n| n.checked_add(size_of::<(TrackId, DeviceId)>()))
        .ok_or_else(overflow)?;
    let table = project
        .tracks()
        .len()
        .checked_mul(row)
        .and_then(|n| n.checked_add(body))
        .and_then(|n| n.checked_add(header))
        .ok_or_else(overflow)?;
    // Existing concrete leaf constructors check their entire new owner,
    // factory, state and resource-configuration metadata against this ceiling.
    // It excludes original PCM, whose existing bank must join the caller union.
    let scope = resources.map_or(MAX_SHARED_ANALYTIC_CONTEXT_BYTES, |r| {
        r.max_scope_metadata_bytes
    });
    let retained_bytes = count
        .checked_mul(scope)
        .and_then(|n| n.checked_add(table))
        .ok_or_else(overflow)?;
    // Keep one additional full scope for the current configuration/codec input
    // overlap, and the existing conservative marker scratch. This is a ceiling,
    // not a measurement or inference that previous/shared owners are absent.
    let scratch = if count == 0 {
        0
    } else {
        scope
            .checked_add(
                hex_devices::sampler::MAX_SLICES
                    .checked_mul(size_of::<usize>())
                    .ok_or_else(overflow)?,
            )
            .ok_or_else(overflow)?
    };
    Ok(Quote {
        retained_bytes,
        construction_peak_bytes: retained_bytes.checked_add(scratch).ok_or_else(overflow)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AudioRootStorageLedger, AudioStorageLedgerReservation, AudioStorageRole};

    #[test]
    fn sampler_context_preconstruction_limits_refuse_before_birth_and_bound_actual_new_metadata() {
        let Some((bank, identity, editor)) = super::super::sampler_tests::canonical_fixture()
        else {
            eprintln!("unsupported sampler context preconstruction");
            return;
        };
        drop(identity);
        eprintln!("SUPPORTED sampler context preconstruction loaded original-resource route");
        let registry = hex_devices::native_instrument_registry().unwrap();
        let resources = Some(crate::native_device_resources::DeviceResourcePreparation {
            bank: &bank,
            max_scope_metadata_bytes: 65_536,
        });
        let quote = quote(editor.project(), resources).unwrap();
        for peak in [false, true] {
            let mut limits = SamplerContextConstructionLimits {
                retained_bytes: quote.retained_bytes,
                construction_peak_bytes: quote.construction_peak_bytes,
            };
            if peak {
                limits.construction_peak_bytes -= 1;
            } else {
                limits.retained_bytes -= 1;
            }
            crate::owned_shared::reset_birth_count();
            crate::owned_shared::reset_clone_count();
            assert!(matches!(
                SharedAnalyticContextTable::prepare_with_sampler_construction_limits(
                    editor.project(),
                    &registry,
                    None,
                    resources,
                    Some(limits),
                    None,
                ),
                Err(Invalid("sampler context construction allowance exceeded"))
            ));
            assert_eq!(crate::owned_shared::birth_count(), 0);
            assert_eq!(crate::owned_shared::clone_count(), 0);
        }
        let collect = |table: &Option<OwnedShared<SharedAnalyticContextTable>>| {
            let mut ledger =
                AudioRootStorageLedger::try_new_on_control(AudioStorageLedgerReservation {
                    data_rows: 1024,
                    handle_rows: 1024,
                    root_rows: 128,
                    max_retained_bytes: 4 * 1024 * 1024,
                    max_construction_peak_bytes: 4 * 1024 * 1024,
                })
                .unwrap();
            ledger
                .borrow_device_resource_bank_on_control(&bank, AudioStorageRole::Caller)
                .unwrap();
            ledger
                .borrow_editor_native_states_on_control(&editor)
                .unwrap();
            // The same real caller slot is charged before and after construction.
            ledger.root(table, AudioStorageRole::Current).unwrap();
            if let Some(table) = table {
                super::super::resource_storage::visit_table_contents(table, &mut ledger).unwrap();
            }
            ledger
                .validate_selected_shared_handles_on_control()
                .unwrap();
            let summary = ledger.summary().unwrap();
            summary.require_known_storage().unwrap();
            summary
        };
        let mut table = None;
        let before = collect(&table);
        crate::owned_shared::reset_birth_count();
        table = Some(
            SharedAnalyticContextTable::prepare_with_sampler_construction_limits(
                editor.project(),
                &registry,
                None,
                resources,
                Some(SamplerContextConstructionLimits {
                    retained_bytes: quote.retained_bytes,
                    construction_peak_bytes: quote.construction_peak_bytes,
                }),
                None,
            )
            .unwrap(),
        );
        assert_eq!(crate::owned_shared::birth_count(), 4); // lease, factory, owner, table
        let after = collect(&table);
        assert!(after.data_bytes > before.data_bytes);
        assert!(after.data_bytes - before.data_bytes <= quote.retained_bytes);
        let table = table.as_ref().unwrap();
        assert_eq!(table.owners.capacity(), editor.project().tracks().len());
        assert_eq!(table.ids.capacity(), editor.project().tracks().len());
        assert_eq!(
            table.mutable_continuations.capacity(),
            editor.project().tracks().len()
        );
        table.refuse_opaque_control_drop().unwrap();
        let reused = SharedAnalyticContextTable::prepare_with_sampler_construction_limits(
            editor.project(),
            &registry,
            Some(table.as_ref()),
            resources,
            Some(SamplerContextConstructionLimits {
                retained_bytes: quote.retained_bytes,
                construction_peak_bytes: quote.construction_peak_bytes,
            }),
            None,
        )
        .unwrap();
        assert!(Arc::ptr_eq(table.owners[0].arc(), reused.owners[0].arc()));
        assert!(!Arc::ptr_eq(table.arc(), reused.arc()));
        assert_eq!(
            table.owners[0].sampler_factory.as_ref().unwrap().0.parts(),
            reused.owners[0].sampler_factory.as_ref().unwrap().0.parts()
        );
    }
    #[test]
    fn sampler_context_selected_union_reservation_keeps_old_aliases_and_refuses_before_birth() {
        let Some((bank, identity, editor)) = super::super::sampler_tests::canonical_fixture()
        else {
            eprintln!("unsupported sampler context selected union");
            return;
        };
        drop(identity);
        eprintln!(
            "SUPPORTED sampler context selected union old table aliases and prebirth refusal"
        );
        let registry = hex_devices::native_instrument_registry().unwrap();
        let resources = Some(crate::native_device_resources::DeviceResourcePreparation {
            bank: &bank,
            max_scope_metadata_bytes: 65_536,
        });
        let old = SharedAnalyticContextTable::prepare_with_sampler_construction_limits(
            editor.project(),
            &registry,
            None,
            resources,
            Some(quote(editor.project(), resources).unwrap().limits()),
            None,
        )
        .unwrap();
        let retained_alias = old.clone_on_control();
        let reservation = AudioStorageLedgerReservation {
            data_rows: 1024,
            handle_rows: 1024,
            root_rows: 128,
            max_retained_bytes: 4 * 1024 * 1024,
            max_construction_peak_bytes: 4 * 1024 * 1024,
        };
        let snapshot = editor.project().snapshot();
        let collect = |reservation, include_alias| {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation).unwrap();
            ledger
                .borrow_device_resource_bank_on_control(&bank, AudioStorageRole::Caller)
                .unwrap();
            ledger
                .borrow_editor_native_states_on_control(&editor)
                .unwrap();
            // This independent equality oracle also owns real state aliases;
            // visit those fields rather than weakening closure to ignore them.
            for device in snapshot
                .tracks
                .iter()
                .flat_map(|track| track.devices.iter().chain(track.instrument.iter()))
                .chain(
                    snapshot
                        .return_tracks
                        .iter()
                        .flat_map(|track| &track.devices),
                )
            {
                ledger
                    .borrow_canonical_native_state_on_control(&device.state)
                    .unwrap();
            }
            super::super::resource_storage::visit_table(
                &old,
                &mut ledger,
                AudioStorageRole::Current,
            )
            .unwrap();
            if include_alias {
                super::super::resource_storage::visit_table(
                    &retained_alias,
                    &mut ledger,
                    AudioStorageRole::Archive,
                )
                .unwrap();
            }
            ledger
        };
        let before = collect(reservation, true).summary().unwrap();
        let required = quote(editor.project(), resources).unwrap().limits();
        let mut exact = reservation;
        exact.max_retained_bytes = before.retained_bytes + required.retained_bytes;
        exact.max_construction_peak_bytes =
            before.construction_peak_bytes + required.construction_peak_bytes;
        for peak in [false, true] {
            let mut short = exact;
            if peak {
                short.max_construction_peak_bytes -= 1;
            } else {
                short.max_retained_bytes -= 1;
            }
            let ledger = collect(short, true);
            crate::owned_shared::reset_birth_count();
            crate::owned_shared::reset_clone_count();
            assert_eq!(
                ledger
                    .preflight_sampler_context_construction_on_control(editor.project(), resources)
                    .unwrap_err(),
                crate::AudioStorageError::ReservationExceeded
            );
            assert!(matches!(
                SharedAnalyticContextTable::prepare_with_sampler_construction_limits(
                    editor.project(),
                    &registry,
                    Some(&old),
                    resources,
                    Some(required),
                    Some(&ledger),
                ),
                Err(Invalid(
                    "sampler context selected union reservation was refused"
                ))
            ));
            assert_eq!(crate::owned_shared::birth_count(), 0);
            assert_eq!(crate::owned_shared::clone_count(), 0);
            assert_eq!(ledger.summary().unwrap(), before);
        }
        let omitted = collect(exact, false);
        assert!(
            omitted
                .preflight_sampler_context_construction_on_control(editor.project(), resources)
                .is_err()
        );
        let ledger = collect(exact, true);
        let limits = ledger
            .preflight_sampler_context_construction_on_control(editor.project(), resources)
            .unwrap();
        let candidate = SharedAnalyticContextTable::prepare_with_sampler_construction_limits(
            editor.project(),
            &registry,
            Some(&old),
            resources,
            Some(limits),
            Some(&ledger),
        )
        .unwrap();
        assert!(!Arc::ptr_eq(old.arc(), candidate.arc()));
        assert!(Arc::ptr_eq(old.owners[0].arc(), candidate.owners[0].arc()));
        // The selected old/input ledger remains borrowed during construction;
        // it is not relabelled as the now enlarged candidate union.
        assert_eq!(ledger.summary().unwrap(), before);
        assert_eq!(editor.project().snapshot(), snapshot);
        assert!(Arc::ptr_eq(old.arc(), retained_alias.arc()));
        assert!(before.require_complete().is_err());
    }

    #[test]
    fn sampler_context_preconstruction_empty_class_and_overflow_preserve_originals() {
        let Some((bank, identity, mut editor)) = super::super::sampler_tests::canonical_fixture()
        else {
            eprintln!("unsupported sampler context preconstruction empty/class");
            return;
        };
        drop(identity);
        eprintln!("SUPPORTED sampler context preconstruction empty/class/overflow");
        let registry = hex_devices::native_instrument_registry().unwrap();
        let device = editor.project().tracks()[0].instrument().unwrap().id();
        let original = editor.project().snapshot();
        let overflow_resources = Some(crate::native_device_resources::DeviceResourcePreparation {
            bank: &bank,
            max_scope_metadata_bytes: usize::MAX,
        });
        crate::owned_shared::reset_birth_count();
        assert!(matches!(
            quote(editor.project(), overflow_resources),
            Err(Invalid("sampler context construction allowance overflow"))
        ));
        assert_eq!(crate::owned_shared::birth_count(), 0);
        assert_eq!(editor.project().snapshot(), original);
        editor
            .execute(hex_project::Command::ReplaceDeviceState {
                device,
                state: hex_devices::sampler::state::DeviceState::Empty
                    .to_native()
                    .unwrap(),
            })
            .unwrap();
        let empty_quote = quote(editor.project(), None).unwrap();
        crate::owned_shared::reset_birth_count();
        let table = SharedAnalyticContextTable::prepare_with_sampler_construction_limits(
            editor.project(),
            &registry,
            None,
            None,
            Some(SamplerContextConstructionLimits {
                retained_bytes: empty_quote.retained_bytes,
                construction_peak_bytes: empty_quote.construction_peak_bytes,
            }),
            None,
        )
        .unwrap();
        assert_eq!(crate::owned_shared::birth_count(), 3); // factory, owner, table
        assert!(table.owners[0].resources.is_none());
        table.refuse_opaque_control_drop().unwrap();
        drop(table);
        editor
            .execute(hex_project::Command::ReplaceDeviceState {
                device,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap();
        crate::owned_shared::reset_birth_count();
        crate::owned_shared::reset_clone_count();
        assert!(matches!(
            SharedAnalyticContextTable::prepare_with_sampler_construction_limits(
                editor.project(),
                &registry,
                None,
                None,
                Some(SamplerContextConstructionLimits {
                    retained_bytes: usize::MAX,
                    construction_peak_bytes: usize::MAX,
                }),
                None,
            ),
            Err(Invalid("sampler context construction class is unsupported"))
        ));
        assert_eq!(crate::owned_shared::birth_count(), 0);
        assert_eq!(crate::owned_shared::clone_count(), 0);
    }
}
