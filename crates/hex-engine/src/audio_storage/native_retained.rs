//! Constructor-frozen native allocation records. These are a distinct domain:
//! a record is born from the real closed constructor walk, never a mutable
//! owner's saved address or a caller-supplied byte declaration. Pair/lifecycle
//! checks in the birth archive remain mandatory before using these records.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativeRuntimePartition {
    Shell,
    Group(u8, usize),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativeSharedKey {
    Project,
    Revision,
    EffectMeter {
        kind: u8,
        chain: usize,
        device: usize,
    },
    EffectConfiguration {
        kind: u8,
        chain: usize,
        device: usize,
    },
    ContextResource(u16),
    RuntimeRegistration,
    GroupRegistration(usize),
}
// Audited Engine producer only: the real birth archive supplies its actual
// retained registration fields, never bytes, addresses or an approval callback.
pub(crate) trait NativeArchiveAnchorFields {
    fn registration_owner(&self, key: NativeSharedKey) -> Result<Owner<'_>, AudioStorageError>;
    fn registration_key(&self, owner: Owner<'_>) -> Result<NativeSharedKey, AudioStorageError>;
}
pub(crate) fn native_shared_revision_owner(
    revision: &Arc<crate::PreparedRevision>,
    key: NativeSharedKey,
) -> Result<Owner<'_>, AudioStorageError> {
    match key {
        NativeSharedKey::Project => Ok(Owner::shared(revision.arrangement())),
        NativeSharedKey::Revision => Ok(Owner::shared(revision)),
        NativeSharedKey::EffectMeter {
            kind,
            chain,
            device,
        } => {
            let entries = match kind {
                0 => {
                    &revision
                        .arrangement()
                        .tracks()
                        .get(chain)
                        .ok_or(AudioStorageError::SourceMismatch)?
                        .device_chain
                }
                4 => {
                    &revision
                        .arrangement()
                        .return_buses()
                        .get(chain)
                        .ok_or(AudioStorageError::SourceMismatch)?
                        .device_chain
                }
                _ => return Err(AudioStorageError::SourceMismatch),
            };
            Ok(Owner::shared(
                &entries
                    .get(device)
                    .ok_or(AudioStorageError::SourceMismatch)?
                    .meter,
            ))
        }
        NativeSharedKey::EffectConfiguration {
            kind,
            chain,
            device,
        } => {
            let entries = match kind {
                0 => {
                    &revision
                        .arrangement()
                        .tracks()
                        .get(chain)
                        .ok_or(AudioStorageError::SourceMismatch)?
                        .device_chain
                }
                4 => {
                    &revision
                        .arrangement()
                        .return_buses()
                        .get(chain)
                        .ok_or(AudioStorageError::SourceMismatch)?
                        .device_chain
                }
                _ => return Err(AudioStorageError::SourceMismatch),
            };
            Ok(Owner::shared(
                entries
                    .get(device)
                    .ok_or(AudioStorageError::SourceMismatch)?
                    .configuration
                    .as_ref()
                    .ok_or(AudioStorageError::UnknownStorage)?,
            ))
        }
        NativeSharedKey::ContextResource(index) => {
            let context = revision
                .arrangement()
                .shared_contexts()
                .owners()
                .find(|(candidate, _)| *candidate == index)
                .map(|(_, owner)| owner)
                .ok_or(AudioStorageError::SourceMismatch)?;
            Ok(context
                .resource_configuration()
                .ok_or(AudioStorageError::SourceMismatch)?
                .native_constructor_owner())
        }
        _ => Err(AudioStorageError::SourceMismatch),
    }
}
fn native_shared_key(
    revision: &Arc<crate::PreparedRevision>,
    registrations: &dyn NativeArchiveAnchorFields,
    row: &Handle<'_>,
) -> Result<NativeSharedKey, AudioStorageError> {
    for key in [NativeSharedKey::Project, NativeSharedKey::Revision] {
        if row.owner.same(native_shared_revision_owner(revision, key)?) {
            return Ok(key);
        }
    }
    if let Some(NativeRuntimePartition::Group(kind @ (0 | 4), chain)) = row.native_partition {
        let key = match row.location.0 {
            "runtime-meter" => Some(NativeSharedKey::EffectMeter {
                kind,
                chain,
                device: row.location.1,
            }),
            "runtime-configuration" => Some(NativeSharedKey::EffectConfiguration {
                kind,
                chain,
                device: row.location.1,
            }),
            _ => None,
        };
        if let Some(key) = key {
            if row.owner.same(native_shared_revision_owner(revision, key)?) {
                return Ok(key);
            }
            return Err(AudioStorageError::SourceMismatch);
        }
    }
    for (index, context) in revision.arrangement().shared_contexts().owners() {
        if let Some(configuration) = context.resource_configuration() {
            if row.owner.same(configuration.native_constructor_owner()) {
                return Ok(NativeSharedKey::ContextResource(index));
            }
        }
    }
    registrations.registration_key(row.owner)
}
struct NativeSharedLineage {
    partition: NativeRuntimePartition,
    key: NativeSharedKey,
    id: u64,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum AnchorPhase {
    Live,
    DropPending,
    Retired,
    Failed,
}
struct NativePhysicalAllocation {
    partition: NativeRuntimePartition,
    declaration: Declaration,
}
/// The unresolved handle count is deliberate: exclusive DATA alone cannot
/// qualify an active runtime. Individual shared lineages and their actual
/// immutable anchors must be bound before the retained traversal can complete.
pub(crate) struct NativePhysicalCapture {
    allocations: Vec<NativePhysicalAllocation>,
    handles: Vec<NativeSharedLineage>,
    anchor: Option<crate::AudioCompiledRevision>,
    anchor_phase: AnchorPhase,
}
impl NativePhysicalCapture {
    pub(crate) fn visit_metadata<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        ledger.vec(&self.allocations)?;
        ledger.vec(&self.handles)?;
        if matches!(
            self.anchor_phase,
            AnchorPhase::DropPending | AnchorPhase::Failed
        ) {
            return Err(AudioStorageError::UnknownStorage);
        }
        if let Some(anchor) = &self.anchor {
            anchor.visit_shared_storage_on_control(ledger)?;
        }
        Ok(())
    }
    pub(crate) fn visit_exclusive_partition<'a>(
        &'a self,
        partition: NativeRuntimePartition,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        for record in self
            .allocations
            .iter()
            .filter(|row| row.partition == partition)
        {
            // Identity is the ACTUAL lifetime-bound record in its constructor
            // archive. It is neither the old mutable owner's address nor a
            // public copied declaration/approval token.
            let mut owner = Owner::exclusive(record);
            owner.domain = OwnerDomain::RetainedNative;
            ledger.data(
                owner,
                Location("retained-native-allocation", 0),
                record.declaration,
                None,
            )?;
        }
        Ok(())
    }
    pub(crate) fn visit_partition<'a>(
        &'a self,
        partition: NativeRuntimePartition,
        registrations: &'a dyn NativeArchiveAnchorFields,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        if self.anchor_phase != AnchorPhase::Live {
            return Err(AudioStorageError::UnknownStorage);
        }
        let anchor = self
            .anchor
            .as_ref()
            .ok_or(AudioStorageError::UnknownStorage)?;
        self.visit_exclusive_partition(partition, ledger)?;
        for lineage in self.handles.iter().filter(|row| row.partition == partition) {
            let owner = match lineage.key {
                NativeSharedKey::RuntimeRegistration | NativeSharedKey::GroupRegistration(_) => {
                    registrations.registration_owner(lineage.key)?
                }
                key => anchor.native_constructor_owner(key)?,
            };
            if let Some(arithmetic) = &mut ledger.arithmetic {
                arithmetic.existing(owner)?;
            } else if !ledger.data.iter().any(|row| row.owner.same(owner)) {
                return Err(AudioStorageError::SourceMismatch);
            }
            ledger.handle_at(
                owner,
                Owner::exclusive(lineage),
                Location("retained-native-handle", 0),
                lineage.id,
            )?;
        }
        Ok(())
    }
    pub(crate) fn retire_anchor_on_control(
        &mut self,
        first_panic: &mut Option<Box<dyn Any + Send>>,
    ) -> Result<(), AudioStorageError> {
        match self.anchor_phase {
            AnchorPhase::Retired => return Ok(()),
            AnchorPhase::DropPending | AnchorPhase::Failed => {
                return Err(AudioStorageError::UnknownStorage);
            }
            AnchorPhase::Live => {}
        }
        self.anchor_phase = AnchorPhase::DropPending;
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(self.anchor.take()))) {
            Ok(()) => {
                self.anchor_phase = AnchorPhase::Retired;
                Ok(())
            }
            Err(payload) => {
                if first_panic.is_none() {
                    *first_panic = Some(payload);
                }
                self.anchor_phase = AnchorPhase::Failed;
                Err(AudioStorageError::UnknownStorage)
            }
        }
    }
}
impl<'a> AudioRootStorageLedger<'a> {
    /// Engine-internal finite constructor walk only. An ordinary Session
    /// inspection outside this operation retains its existing direct domains.
    pub(crate) fn with_native_constructor_partition<T>(
        &mut self,
        partition: NativeRuntimePartition,
        visit: impl FnOnce(&mut Self) -> Result<T, AudioStorageError>,
    ) -> Result<T, AudioStorageError> {
        let previous = self.native_partition;
        if previous.is_none() && partition != NativeRuntimePartition::Shell {
            return visit(self);
        }
        self.native_partition = Some(partition);
        let result = visit(self);
        self.native_partition = previous;
        result
    }
    /// Copy ONLY concrete declarations from tagged actual constructor rows.
    /// The closed constructor has already checked its real mandatory inputs
    /// and shared closure. No arbitrary ledger can supply an exported permit.
    pub(crate) fn capture_native_physical_on_control(
        &self,
        reserved_split_bytes: usize,
        remaining_runtime_bytes: usize,
        custody: &crate::AudioPreparationCustody,
        token: &crate::AudioPreparationToken,
        registrations: &dyn NativeArchiveAnchorFields,
    ) -> Result<NativePhysicalCapture, AudioStorageError> {
        if self.arithmetic.is_some() || self.native_partition.is_some() {
            return Err(AudioStorageError::UnknownStorage);
        }
        let summary = self.summary()?;
        self.validate_closed_shared_handles()?;
        if !crate::audio_storage_constructor_layout_supported() {
            return Err(AudioStorageError::UnknownStorage);
        }
        let revision = custody.candidate_root_view(token)?;
        let mut count = 0_usize;
        let mut unbound_data = 0_usize;
        for row in self
            .data
            .iter()
            .filter(|row| row.native_partition.is_some())
        {
            match row.owner.domain {
                OwnerDomain::Exclusive | OwnerDomain::ExclusiveVec if row.streamed.is_none() => {
                    count = count.checked_add(1).ok_or(AudioStorageError::Overflow)?;
                }
                _ => {
                    unbound_data = unbound_data
                        .checked_add(1)
                        .ok_or(AudioStorageError::Overflow)?
                }
            }
        }
        let unbound_handles = self
            .handles
            .iter()
            .filter(|row| row.native_partition.is_some())
            .count();
        if count == 0 {
            return Err(AudioStorageError::SourceMismatch);
        }
        if unbound_data != 0 {
            return Err(AudioStorageError::UnknownStorage);
        }
        for row in self
            .handles
            .iter()
            .filter(|row| row.native_partition.is_some())
        {
            native_shared_key(revision, registrations, row)?;
        }
        // The two new physical reader handles must fit before registration.
        if self
            .handles
            .len()
            .checked_add(2)
            .ok_or(AudioStorageError::Overflow)?
            > self.reservation.handle_rows
        {
            return Err(AudioStorageError::Capacity);
        }
        let backing = count
            .checked_mul(size_of::<NativePhysicalAllocation>())
            .and_then(|n| {
                n.checked_add(unbound_handles.checked_mul(size_of::<NativeSharedLineage>())?)
            })
            .ok_or(AudioStorageError::Overflow)?;
        let peak_extra = backing
            .checked_add(size_of::<NativePhysicalCapture>())
            .ok_or(AudioStorageError::Overflow)?;
        // The pinned grow_exact layout is the same supported basis as the
        // configured ledger backing. Check BOTH ceilings before this allocation.
        if backing > remaining_runtime_bytes
            || summary
                .retained_bytes
                .checked_add(reserved_split_bytes)
                .and_then(|n| n.checked_add(backing))
                .ok_or(AudioStorageError::Overflow)?
                > self.reservation.max_retained_bytes
            || summary
                .construction_peak_bytes
                .checked_add(reserved_split_bytes)
                .and_then(|n| n.checked_add(peak_extra))
                .ok_or(AudioStorageError::Overflow)?
                > self.reservation.max_construction_peak_bytes
        {
            return Err(AudioStorageError::ReservationExceeded);
        }
        let mut allocations = Vec::new();
        allocations
            .try_reserve_exact(count)
            .map_err(|_| AudioStorageError::Allocation)?;
        if allocations.capacity() != count {
            return Err(AudioStorageError::UnknownStorage);
        }
        for row in self
            .data
            .iter()
            .filter(|row| row.native_partition.is_some())
        {
            if matches!(
                row.owner.domain,
                OwnerDomain::Exclusive | OwnerDomain::ExclusiveVec
            ) && row.streamed.is_none()
            {
                allocations.push(NativePhysicalAllocation {
                    partition: row
                        .native_partition
                        .ok_or(AudioStorageError::SourceMismatch)?,
                    declaration: row.declaration,
                });
            }
        }
        let mut handles = Vec::new();
        handles
            .try_reserve_exact(unbound_handles)
            .map_err(|_| AudioStorageError::Allocation)?;
        if handles.capacity() != unbound_handles {
            return Err(AudioStorageError::UnknownStorage);
        }
        for row in self
            .handles
            .iter()
            .filter(|row| row.native_partition.is_some())
        {
            handles.push(NativeSharedLineage {
                partition: row
                    .native_partition
                    .ok_or(AudioStorageError::SourceMismatch)?,
                key: native_shared_key(revision, registrations, row)?,
                id: row.id,
            });
        }
        // The actual constructor supplies the reader directly; no raw Arc is
        // adopted and no source inferred from counts or a saved address.
        let anchor = custody.register_compiled_reader_on_control(token)?;
        Ok(NativePhysicalCapture {
            allocations,
            handles,
            anchor: Some(anchor),
            anchor_phase: AnchorPhase::Live,
        })
    }
}
