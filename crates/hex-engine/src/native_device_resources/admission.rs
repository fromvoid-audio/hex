//! Borrow actual Session stores, registered resource sets/publication obligations and explicit
//! private offline selections for two finite admission phases. Original banks share one ledger
//! so a current source is not charged again merely because an output also retains it. This is
//! neither a global allocator, whole-graph provenance nor a retirement receipt.
use super::*;
use crate::audio_storage::{Location, Owner};
use crate::{
    Audio, AudioRootStorageLedger, AudioRootStorageSummary, AudioStorageLedgerReservation,
    PreparedRevision,
};

#[derive(Clone, Copy)]
pub struct ControlSamplerSelection<'a> {
    pub sources: Option<&'a ControlSamplerSources>,
    pub prepared: Option<&'a PreparedRevision>,
}
#[derive(Clone, Copy)]
pub struct ControlSamplerOutbound<'a> {
    pub anchor: &'a ControlSamplerSources,
    pub obligation: &'a super::DispatchedSourceObligation,
    pub job: u64,
}
pub struct DeviceResourcePhase<'a> {
    pub stores: &'a [&'a ResidentDeviceAssets],
    /// Actual ordinary resource sets, active/retired books and prospective publication anchors.
    pub registered: &'a [super::DeviceResourceAdmission<'a>],
    pub selections: &'a [ControlSamplerSelection<'a>],
    pub outbound: &'a [ControlSamplerOutbound<'a>],
    /// Actual current/loaded original and derivative owners, not path/size quotes.
    pub legacy: &'a [&'a Audio],
    pub checked_runtime_bytes: usize,
    /// Checked constructor/preparation allowance, never an unlimited fallback.
    pub construction_bytes: usize,
}
pub struct DeviceResourceInventory {
    pub retained: AudioRootStorageSummary,
    pub construction: AudioRootStorageSummary,
    pub retained_frames: u64,
    pub construction_frames: u64,
}
fn legacy<'a>(phase: &'a DeviceResourcePhase<'a>) -> impl Iterator<Item = &'a Audio> + Clone {
    phase
        .legacy
        .iter()
        .copied()
        .chain(
            phase
                .selections
                .iter()
                .filter_map(|s| s.prepared)
                .flat_map(|p| {
                    p.arrangement()
                        .tracks()
                        .iter()
                        .flat_map(|t| t.clips.iter())
                        .filter_map(|c| c.audio.as_ref())
                        .chain(
                            p.session()
                                .audio_clips()
                                .iter()
                                .filter_map(|c| c.resident_audio()),
                        )
                }),
        )
        .chain(
            phase
                .registered
                .iter()
                .flat_map(super::DeviceResourceAdmission::legacy_sources),
        )
}
fn inspect(
    phase: &DeviceResourcePhase<'_>,
    reservation: AudioStorageLedgerReservation,
    staged: Option<&super::resident::StagedNativeAssetView<'_>>,
    construction: bool,
) -> Result<(AudioRootStorageSummary, u64), AudioStorageError> {
    // This local scalar identifies explicit allowances, never PCM provenance.
    let allowance_anchor = 0usize;
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation)?;
    ledger.mutate(|ledger| {
        for store in phase.stores {
            ledger.root(*store, crate::AudioStorageRole::Current)?;
            if let Some(stage) = staged.filter(|s| std::ptr::eq(*store, s.store)) {
                ledger.vec(&stage.staged.destination)?;
                stage.staged.asset.1.visit(ledger)?;
                if construction {
                    ledger.growth(
                        store
                            .assets
                            .capacity()
                            .checked_mul(std::mem::size_of::<(
                                hex_project::AssetId,
                                super::resident::NativeBankLoan,
                            )>())
                            .and_then(|n| {
                                n.checked_add(std::mem::size_of_val(&stage.staged.destination))
                            })
                            .ok_or(AudioStorageError::Overflow)?,
                    )?;
                }
            } else {
                ledger.vec(&store.assets)?;
            }
            for (_, source) in &store.assets {
                source.visit(ledger)?;
            }
        }
        for registered in phase.registered {
            registered.visit(ledger)?;
        }
        for selection in phase.selections {
            if let Some(sources) = selection.sources {
                sources.visit(ledger)?;
                if let Some(prepared) = selection.prepared {
                    if prepared.resource_scope != Some(sources.id()) {
                        return Err(AudioStorageError::SourceMismatch);
                    }
                    prepared
                        .session()
                        .visit_control_sampler_factories(ledger, sources)?;
                } else {
                    // The actual inline scalar table fixes the future factory
                    // allocation before the normal compiler allocates it.
                    ledger.bytes(
                        Owner::exclusive(sources),
                        Location("staged-control-factories", 0),
                        sources
                            .compiler_factory_allowance()
                            .map_err(|_| AudioStorageError::Overflow)?,
                    )?;
                }
            } else if selection
                .prepared
                .is_some_and(PreparedRevision::requires_control_sampler_sources)
            {
                return Err(AudioStorageError::SourceMismatch);
            }
        }
        for outbound in phase.outbound {
            outbound
                .anchor
                .visit_dispatched(ledger, outbound.obligation, outbound.job)?;
        }
        for audio in legacy(phase) {
            ledger.resident_capacity(audio)?;
        }
        ledger.bytes(
            Owner::exclusive(&allowance_anchor),
            Location("checked-control-runtime", 0),
            phase.checked_runtime_bytes,
        )?;
        ledger.growth(phase.construction_bytes)?;
        Ok(())
    })?;
    Ok((ledger.summary()?, ledger.resident_capacity_frames()?))
}
pub fn inspect_device_resource_union(
    retained: &DeviceResourcePhase<'_>,
    construction: &DeviceResourcePhase<'_>,
    reservation: AudioStorageLedgerReservation,
) -> Result<DeviceResourceInventory, ResourcePreparationError> {
    let (kept, retained_frames) = inspect(retained, reservation, None, false)?;
    // Construction selection is a separate borrow after the retained ledger
    // has dropped. Its complete live union is checked against the caller peak.
    let peak_reservation = AudioStorageLedgerReservation {
        max_retained_bytes: reservation.max_construction_peak_bytes,
        ..reservation
    };
    let (peak, construction_frames) = inspect(construction, peak_reservation, None, true)?;
    Ok(DeviceResourceInventory {
        retained: kept,
        construction: peak,
        retained_frames,
        construction_frames,
    })
}

pub(super) fn inspect_staged_union(
    retained: &DeviceResourcePhase<'_>,
    construction: &DeviceResourcePhase<'_>,
    reservation: AudioStorageLedgerReservation,
    staged: &super::resident::StagedNativeAssetView<'_>,
) -> Result<DeviceResourceInventory, ResourcePreparationError> {
    let mut retained_stores = retained.stores.to_vec();
    if !retained_stores
        .iter()
        .any(|store| std::ptr::eq(*store, staged.store))
    {
        retained_stores.push(staged.store);
    }
    let mut construction_stores = construction.stores.to_vec();
    if !construction_stores
        .iter()
        .any(|store| std::ptr::eq(*store, staged.store))
    {
        construction_stores.push(staged.store);
    }
    let retained = DeviceResourcePhase {
        stores: &retained_stores,
        registered: retained.registered,
        selections: retained.selections,
        outbound: retained.outbound,
        legacy: retained.legacy,
        checked_runtime_bytes: retained.checked_runtime_bytes,
        construction_bytes: retained.construction_bytes,
    };
    let construction = DeviceResourcePhase {
        stores: &construction_stores,
        registered: construction.registered,
        selections: construction.selections,
        outbound: construction.outbound,
        legacy: construction.legacy,
        checked_runtime_bytes: construction.checked_runtime_bytes,
        construction_bytes: construction.construction_bytes,
    };
    let (kept, retained_frames) = inspect(&retained, reservation, Some(staged), false)?;
    let peak_reservation = AudioStorageLedgerReservation {
        max_retained_bytes: reservation.max_construction_peak_bytes,
        ..reservation
    };
    let (peak, construction_frames) = inspect(&construction, peak_reservation, Some(staged), true)?;
    Ok(DeviceResourceInventory {
        retained: kept,
        construction: peak,
        retained_frames,
        construction_frames,
    })
}
