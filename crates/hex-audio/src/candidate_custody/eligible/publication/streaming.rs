//! Caller-finite exact demand backing follows its actual runtime State and
//! returns to the reserved Root slot before normal metadata destruction.
use super::*;
use hex_engine::StreamPageExtent;
mod actions;
mod demand;
mod prewarm;
pub(crate) use actions::StageAction;
#[derive(Clone, Copy)]
struct ScheduledAction {
    ticket: u64,
    due: u64,
    allow_prewarm: bool,
    pressure_blocked: bool,
}
#[derive(Clone, Copy)]
enum CandidateRole {
    None,
    Publication,
    LaunchPrewarm { ticket: u64, due: u64 },
}
pub(crate) use demand::{StreamInstall, StreamOutcome, StreamProgress};
struct PendingAction {
    prepared: hex_engine::PreparedStreamAction,
    ticket: u64,
}
pub(super) struct Workspace {
    pub(super) extents: [Vec<StreamPageExtent>; 2],
    pub(super) limit: usize,
    pub(super) stamps: [Option<hex_stream::ReadinessStamp>; 2],
    pub(super) active: [Option<hex_stream::ReadinessStamp>; 2],
    pub(super) role: Option<hex_stream::ProtectionRole>,
    pub(super) predecessor: Option<AudioCandidateIdentity>,
    pending_action: Option<PendingAction>,
    scheduled: Option<ScheduledAction>,
    prewarm_stamps: [Option<hex_stream::ReadinessStamp>; 2],
    candidate_role: CandidateRole,
}
impl Workspace {
    fn prepare(limit: usize) -> Result<Self, &'static str> {
        let mut extents: [Vec<StreamPageExtent>; 2] = std::array::from_fn(|_| Vec::new());
        for rows in &mut extents {
            rows.try_reserve_exact(limit)
                .map_err(|_| "stream demand reservation failed")?;
        }
        Ok(Self {
            extents,
            limit,
            stamps: [None; 2],
            active: [None; 2],
            role: None,
            predecessor: None,
            pending_action: None,
            scheduled: None,
            prewarm_stamps: [None; 2],
            candidate_role: CandidateRole::None,
        })
    }
    pub(super) fn backing_bytes(&self) -> Option<usize> {
        self.extents.iter().try_fold(0usize, |bytes, rows| {
            rows.capacity()
                .checked_mul(std::mem::size_of::<StreamPageExtent>())?
                .checked_add(bytes)
        })
    }
    pub(super) fn valid(&self) -> bool {
        self.limit != 0
            && self
                .extents
                .iter()
                .all(|rows| rows.len() <= self.limit && rows.capacity() >= self.limit)
    }
    pub(super) fn clear(&mut self) {
        for rows in &mut self.extents {
            rows.clear();
        }
        self.stamps = [None; 2];
        self.active = [None; 2];
        self.role = None;
        self.predecessor = None;
        self.pending_action = None;
        self.scheduled = None;
        self.prewarm_stamps = [None; 2];
        self.candidate_role = CandidateRole::None;
    }
}
impl Custody {
    pub(in crate::candidate_custody) fn configure_streaming(
        &self,
        limit: usize,
    ) -> Result<(), &'static str> {
        let (count, max_retained, max_peak, current) = {
            let bank = lock(&self.bank);
            let reservation = bank
                .publication
                .reservation
                .ok_or("constructor publication is not reserved")?;
            if limit == 0
                || bank.closing
                || bank.publication.stream_extents.is_some()
                || bank
                    .publication
                    .slots
                    .iter()
                    .any(|slot| slot.phase != Phase::Vacant)
                || bank.domains.iter().any(|slot| slot.phase != Phase::Vacant)
            {
                return Err("stream demand requires a vacant constructor Root");
            }
            (
                bank.publication.slots.len(),
                reservation.max_retained_data_bytes,
                reservation.max_construction_peak_data_bytes,
                bank.metadata_bytes,
            )
        };
        let requested = count
            .checked_mul(2)
            .and_then(|n| n.checked_mul(limit))
            .and_then(|n| n.checked_mul(std::mem::size_of::<StreamPageExtent>()))
            .ok_or("stream demand storage overflow")?;
        let temporary = count
            .checked_mul(std::mem::size_of::<Workspace>())
            .ok_or("stream demand construction overflow")?;
        if current
            .checked_add(requested)
            .is_none_or(|n| n > max_retained)
            || current
                .checked_add(requested)
                .and_then(|n| n.checked_add(temporary))
                .is_none_or(|n| n > max_peak)
        {
            return Err("stream demand DATA reservation exceeded");
        }
        let mut workspaces = Vec::new();
        workspaces
            .try_reserve_exact(count)
            .map_err(|_| "stream workspace reservation failed")?;
        for _ in 0..count {
            workspaces.push(Workspace::prepare(limit)?);
        }
        let actual = workspaces
            .iter()
            .try_fold(0usize, |bytes, rows| {
                rows.backing_bytes()?.checked_add(bytes)
            })
            .ok_or("stream demand storage overflow")?;
        let overlap = workspaces
            .capacity()
            .checked_mul(std::mem::size_of::<Workspace>())
            .ok_or("stream demand construction overflow")?;
        let mut bank = lock(&self.bank);
        if bank.closing
            || bank.metadata_bytes != current
            || bank.publication.stream_extents.is_some()
            || bank
                .publication
                .slots
                .iter()
                .any(|slot| slot.phase != Phase::Vacant)
            || bank.domains.iter().any(|slot| slot.phase != Phase::Vacant)
        {
            return Err("stream demand constructor changed during reservation");
        }
        let retained = current
            .checked_add(actual)
            .ok_or("stream demand storage overflow")?;
        if retained > max_retained || retained.checked_add(overlap).is_none_or(|n| n > max_peak) {
            return Err("stream demand actual DATA reservation exceeded");
        }
        // No allocation or destructor between validated vacancies and moves.
        for (slot, workspace) in bank.publication.slots.iter_mut().zip(workspaces.drain(..)) {
            slot.streaming = Some(workspace);
        }
        bank.metadata_bytes = retained;
        bank.publication.stream_extents = Some(limit);
        Ok(())
    }
}

#[cfg(all(test, feature = "allocation-probe"))]
mod tests {
    use super::*;
    use crate::prepared::candidate_tests::{anchored, candidate, finish, recover, serial};
    fn backing(root: &CandidateRoot) -> Vec<[(usize, usize); 2]> {
        let bank = lock(&root.0.eligible.bank);
        bank.publication
            .slots
            .iter()
            .map(|slot| {
                let workspace = slot.streaming.as_ref().unwrap();
                assert!(workspace.valid());
                std::array::from_fn(|index| {
                    (
                        workspace.extents[index].as_ptr() as usize,
                        workspace.extents[index].capacity(),
                    )
                })
            })
            .collect()
    }
    #[test]
    fn finite_actual_workspace_moves_with_runtime_and_returns_before_report_slot_reuse() {
        let _serial = serial();
        let Some(mut owner) = candidate() else { return };
        let ledger = AudioStorageLedgerReservation {
            data_rows: 4096,
            handle_rows: 2048,
            root_rows: 1,
            max_retained_bytes: 96 * 1024 * 1024,
            max_construction_peak_bytes: 96 * 1024 * 1024,
        };
        let summary = owner.seal_constructor_on_control(ledger).unwrap();
        let reservation = AudioCandidatePublicationReservation {
            runtime_slots: 2,
            archive_slots: 2,
            member_rows: summary.member_rows * 2,
            transfer_rows: summary.transfer_rows * 2,
            receipt_rows: 3,
            data_rows: ledger.data_rows,
            handle_rows: ledger.handle_rows,
            max_retained_data_bytes: ledger.max_retained_bytes,
            max_construction_peak_data_bytes: ledger.max_construction_peak_bytes,
        };
        let (mut control, mut data, errors, mut retirement) = anchored();
        control
            .configure_audio_candidate_publication(reservation)
            .unwrap();
        let measured = allocation_counter::measure(|| {
            assert!(
                control
                    .configure_audio_candidate_streaming(usize::MAX)
                    .is_err()
            )
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        control.configure_audio_candidate_streaming(8).unwrap();
        let root = control.native_candidate_root();
        let original = backing(&root);
        assert_eq!(original.len(), 2);
        assert!(
            original
                .iter()
                .flatten()
                .all(|(ptr, capacity)| *ptr != 0 && *capacity >= 8)
        );
        let ticket = control
            .stage_eligible_audio_candidate(
                owner,
                vec![],
                crate::prepared::PublicationPolicy::InheritRuntime,
                0,
            )
            .ok()
            .unwrap();
        assert_eq!(
            control.cancel_audio_candidate(&ticket),
            AudioCandidateCancellationResult::Cancelled
        );
        assert!(control.configure_audio_candidate_streaming(8).is_err());
        data.owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block());
        let recovered = recover(&mut control, &ticket);
        assert_eq!(backing(&root), original);
        assert_eq!(
            recovered.return_kind,
            AudioCandidateReturnKind::NeverInstalled
        );
        drop(recovered);
        drop(control);
        drop(data);
        drop(errors);
        finish(&mut retirement);
    }
}
