//! One pre-publication phase in the already reserved runtime cells. The worker
//! borrows rooted construction storage; the adapter retains only this token.
use super::*;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioCandidatePreparationToken {
    root: u64,
    slot: usize,
    incarnation: u64,
}
#[derive(Clone, Copy, Debug)]
pub struct AudioCandidatePreparationReservation {
    pub source_capacity: usize,
    pub storage: hex_engine::AudioStorageReservation,
    pub member_rows: usize,
    pub transfer_rows: usize,
    pub handle_rows: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioCandidatePreparationProgress {
    Collecting,
    Ready,
    Retiring,
    Refused(&'static str),
}
pub struct AudioCandidatePreparationRefusal {
    pub reason: &'static str,
    pub domains: Vec<AudioCandidateDomainToken>,
}
pub(super) struct PreparationExecution {
    token: AudioCandidatePreparationToken,
    cell: Option<hex_engine::AudioCandidatePreparation>,
    domains: Vec<AudioCandidateDomainToken>,
    pending: bool,
    failed: bool,
    refusal: Option<&'static str>,
}
impl Custody {
    pub(in crate::candidate_custody) fn reserve_runtime_preparation(
        &self,
        reservation: AudioCandidatePreparationReservation,
        domains: Vec<AudioCandidateDomainToken>,
    ) -> Result<AudioCandidatePreparationToken, AudioCandidatePreparationRefusal> {
        let Ok(mut work) = self.work.try_lock() else {
            return Err(AudioCandidatePreparationRefusal {
                reason: "candidate preparation is busy",
                domains,
            });
        };
        let preflight = (|| {
            let mut bank = lock(&self.bank);
            let limits = bank
                .publication
                .reservation
                .ok_or("candidate publication is not reserved")?;
            if bank.closing
                || domains.len() > 2
                || reservation.storage.handle_obligations > reservation.handle_rows
                || reservation.storage.max_construction_data_bytes
                    < reservation.storage.max_prepared_data_bytes
                || bank
                    .publication
                    .slots
                    .iter()
                    .any(|s| s.preparation_reservation.is_some())
            {
                return Err("candidate preparation is unavailable");
            }
            let index = bank
                .publication
                .slots
                .iter()
                .position(|s| s.phase == Phase::Vacant)
                .ok_or("candidate runtime cells are full")?;
            let archive = bank
                .publication
                .archives
                .iter()
                .position(|occupied| !occupied)
                .ok_or("candidate archive cells are full")?;
            let incarnation = bank.publication.slots[index]
                .incarnation
                .checked_add(1)
                .ok_or("candidate preparation incarnation exhausted")?;
            if work.publication.preparations[index].is_some() {
                return Err("candidate preparation destination is occupied");
            }
            let mut retained = bank
                .domains
                .iter()
                .try_fold(bank.metadata_bytes, |n, d| n.checked_add(d.data))
                .ok_or("candidate preparation DATA overflow")?;
            let mut peak = bank
                .domains
                .iter()
                .try_fold(bank.metadata_bytes, |n, d| n.checked_add(d.peak))
                .ok_or("candidate preparation peak overflow")?;
            let mut members = reservation.member_rows;
            let mut transfers = reservation.transfer_rows;
            for slot in &bank.publication.slots {
                retained = retained
                    .checked_add(slot.retained)
                    .ok_or("candidate preparation DATA overflow")?;
                peak = peak
                    .checked_add(slot.peak)
                    .ok_or("candidate preparation peak overflow")?;
                if let Some(summary) = slot.summary {
                    members = members
                        .checked_add(summary.member_rows)
                        .ok_or("candidate member overflow")?;
                    transfers = transfers
                        .checked_add(summary.transfer_rows)
                        .ok_or("candidate transfer overflow")?;
                }
            }
            let metadata = domains
                .capacity()
                .checked_mul(std::mem::size_of::<AudioCandidateDomainToken>())
                .ok_or("candidate domain metadata overflow")?;
            let retained_charge = reservation
                .storage
                .max_prepared_data_bytes
                .checked_add(metadata)
                .ok_or("candidate preparation DATA overflow")?;
            let peak_charge = reservation
                .storage
                .max_construction_data_bytes
                .checked_add(metadata)
                .ok_or("candidate preparation peak overflow")?;
            if retained
                .checked_add(retained_charge)
                .is_none_or(|n| n > limits.max_retained_data_bytes)
                || peak
                    .checked_add(peak_charge)
                    .is_none_or(|n| n > limits.max_construction_peak_data_bytes)
                || members > limits.member_rows
                || transfers > limits.transfer_rows
                || reservation.handle_rows
                    > remaining_handle_reservation(&bank)
                        .ok_or("candidate handle reservation exceeded")?
            {
                return Err("candidate preparation reservation exceeded");
            }
            for (i, token) in domains.iter().enumerate() {
                if token.root != bank.root
                    || domains[..i].contains(token)
                    || bank.domains.get(token.slot).is_none_or(|s| {
                        !s.matches(*token)
                            || s.phase != Phase::Accepted
                            || s.retiring
                            || s.owner.is_none()
                            || s.uses == usize::MAX
                    })
                {
                    return Err("candidate preparation domain is unavailable");
                }
            }
            let token = AudioCandidatePreparationToken {
                root: bank.root,
                slot: index,
                incarnation,
            };
            for domain in &domains {
                bank.domains[domain.slot].uses += 1;
            }
            bank.publication.archives[archive] = true;
            let slot = &mut bank.publication.slots[index];
            slot.incarnation = incarnation;
            slot.archive = archive;
            slot.phase = Phase::Reserved;
            slot.preparation_reservation = Some(reservation);
            slot.preparation_retiring = false;
            slot.retained = retained_charge;
            slot.peak = peak_charge;
            Ok(token)
        })();
        let token = match preflight {
            Ok(token) => token,
            Err(reason) => return Err(AudioCandidatePreparationRefusal { reason, domains }),
        };
        // Dedicated work custody spans construction, never an ordinary bank or
        // terminal lock. This exact destination exists before any allocation.
        work.publication.preparations[token.slot] = Some(PreparationExecution {
            token,
            cell: None,
            domains,
            pending: false,
            failed: false,
            refusal: None,
        });
        let Some(execution) = work.publication.preparations[token.slot].as_mut() else {
            let mut bank = lock(&self.bank);
            bank.publication.slots[token.slot].phase = Phase::Failed;
            self.mark_closing(&mut bank);
            return Ok(token);
        };
        execution.pending = true;
        match catch_unwind(AssertUnwindSafe(|| {
            hex_engine::AudioCandidatePreparation::try_new_on_control(
                reservation.source_capacity,
                reservation.storage,
            )
        })) {
            Ok(Ok(cell)) => {
                execution.cell = Some(cell);
                execution.pending = false;
            }
            Ok(Err(_)) => {
                execution.refusal = Some("candidate preparation reservation refused");
                execution.pending = false;
                lock(&self.bank).publication.slots[token.slot].preparation_retiring = true;
            }
            Err(payload) => {
                execution.failed = true;
                work.publication.failed_payloads[token.slot] = Some(payload);
                let mut bank = lock(&self.bank);
                bank.publication.slots[token.slot].phase = Phase::Failed;
                self.mark_closing(&mut bank);
            }
        }
        Ok(token)
    }
    pub(in crate::candidate_custody) fn with_runtime_preparation(
        &self,
        token: AudioCandidatePreparationToken,
        prepare: impl FnOnce(
            &mut hex_engine::AudioCandidatePreparation,
        ) -> Result<(), hex_engine::AudioStorageError>,
    ) -> Result<AudioCandidatePreparationProgress, &'static str> {
        let mut work = self
            .work
            .try_lock()
            .map_err(|_| "candidate preparation is busy")?;
        {
            let bank = lock(&self.bank);
            let slot = bank
                .publication
                .slots
                .get(token.slot)
                .ok_or("candidate preparation token differs")?;
            if bank.root != token.root
                || slot.incarnation != token.incarnation
                || slot.phase != Phase::Reserved
                || slot.preparation_reservation.is_none()
            {
                return Err("candidate preparation token differs");
            }
            if bank.closing || slot.preparation_retiring {
                return Ok(AudioCandidatePreparationProgress::Retiring);
            }
        }
        let execution = work
            .publication
            .preparations
            .get_mut(token.slot)
            .and_then(Option::as_mut)
            .filter(|p| p.token == token)
            .ok_or("candidate preparation storage differs")?;
        if let Some(reason) = execution.refusal {
            return Ok(AudioCandidatePreparationProgress::Refused(reason));
        }
        if execution.pending || execution.failed {
            return Ok(AudioCandidatePreparationProgress::Refused(
                "candidate preparation is incomplete",
            ));
        }
        execution.pending = true;
        let result = catch_unwind(AssertUnwindSafe(|| {
            prepare(
                execution
                    .cell
                    .as_mut()
                    .ok_or(hex_engine::AudioStorageError::SourceMismatch)?,
            )
        }));
        match result {
            Ok(Ok(())) => execution.pending = false,
            Ok(Err(_)) => {
                execution.pending = false;
                execution.refusal = Some("candidate preparation refused");
                let mut bank = lock(&self.bank);
                bank.publication.slots[token.slot].preparation_retiring = true;
                if execution
                    .cell
                    .as_ref()
                    .is_some_and(|cell| cell.failed_incomplete())
                {
                    execution.failed = true;
                    bank.publication.slots[token.slot].phase = Phase::Failed;
                    self.mark_closing(&mut bank);
                }
            }
            Err(payload) => {
                execution.failed = true;
                work.publication.failed_payloads[token.slot] = Some(payload);
                let mut bank = lock(&self.bank);
                bank.publication.slots[token.slot].phase = Phase::Failed;
                self.mark_closing(&mut bank);
                return Ok(AudioCandidatePreparationProgress::Refused(
                    "candidate preparation is incomplete",
                ));
            }
        }
        let bank = lock(&self.bank);
        if bank.closing || bank.publication.slots[token.slot].preparation_retiring {
            return Ok(AudioCandidatePreparationProgress::Retiring);
        }
        if let Some(reason) = execution.refusal {
            return Ok(AudioCandidatePreparationProgress::Refused(reason));
        }
        Ok(
            if execution
                .cell
                .as_mut()
                .and_then(|cell| cell.owner_on_control())
                .is_some()
            {
                AudioCandidatePreparationProgress::Ready
            } else {
                AudioCandidatePreparationProgress::Collecting
            },
        )
    }
    pub(in crate::candidate_custody) fn cancel_runtime_preparation(
        &self,
        token: AudioCandidatePreparationToken,
    ) -> bool {
        let mut bank = lock(&self.bank);
        if bank.root != token.root {
            return false;
        }
        let Some(slot) = bank.publication.slots.get_mut(token.slot).filter(|s| {
            s.incarnation == token.incarnation
                && s.phase == Phase::Reserved
                && s.preparation_reservation.is_some()
        }) else {
            return false;
        };
        slot.preparation_retiring = true;
        true
    }
    pub(super) fn advance_runtime_preparations(&self, work: &mut Work, abandoned: bool) {
        for index in 0..work.preparations.len() {
            let Some(execution) = work.preparations[index].as_mut() else {
                continue;
            };
            if execution.pending || execution.failed || work.failed_payloads[index].is_some() {
                continue;
            }
            {
                let mut bank = lock(&self.bank);
                let closing = bank.closing;
                let slot = &mut bank.publication.slots[index];
                if slot.phase != Phase::Reserved
                    || (!slot.preparation_retiring
                        && !closing
                        && !abandoned
                        && execution.refusal.is_none())
                {
                    continue;
                }
                slot.phase = Phase::Cleaning;
            }
            execution.pending = true;
            let result = catch_unwind(AssertUnwindSafe(|| {
                if let Some(cell) = execution.cell.as_mut() {
                    cell.cleanup_on_control()?;
                }
                drop(execution.cell.take());
                Ok::<_, hex_engine::AudioStorageError>(())
            }));
            match result {
                Ok(Ok(())) => {}
                result => {
                    execution.failed = true;
                    let mut bank = lock(&self.bank);
                    bank.publication.slots[index].phase = Phase::Failed;
                    self.mark_closing(&mut bank);
                    if let Err(payload) = result {
                        work.failed_payloads[index] = Some(payload);
                    }
                    continue;
                }
            }
            // Domains are scalar associations; remove uses under the bank, then
            // destroy the actual metadata backing outside it before vacancy.
            {
                let mut bank = lock(&self.bank);
                if execution.domains.iter().any(|token| {
                    bank.domains
                        .get(token.slot)
                        .is_none_or(|d| !d.matches(*token) || d.uses == 0)
                }) {
                    execution.failed = true;
                } else {
                    for token in &execution.domains {
                        bank.domains[token.slot].uses -= 1;
                    }
                }
                if execution.failed {
                    bank.publication.slots[index].phase = Phase::Failed;
                    self.mark_closing(&mut bank);
                    continue;
                }
            }
            let archive = { lock(&self.bank).publication.slots[index].archive };
            let removed = work.preparations[index].take();
            if let Err(payload) = catch_unwind(AssertUnwindSafe(|| drop(removed))) {
                work.failed_payloads[index] = Some(payload);
                let mut bank = lock(&self.bank);
                bank.publication.slots[index].phase = Phase::Failed;
                self.mark_closing(&mut bank);
                continue;
            }
            let mut bank = lock(&self.bank);
            bank.publication.archives[archive] = false;
            let slot = &mut bank.publication.slots[index];
            slot.preparation_reservation = None;
            slot.preparation_retiring = false;
            slot.phase = Phase::Vacant;
            slot.retained = 0;
            slot.peak = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn root() -> CandidateRoot {
        let root = CandidateRoot::new(2).unwrap();
        root.0
            .eligible
            .configure_publications(
                root.root_id(),
                AudioCandidatePublicationReservation {
                    runtime_slots: 2,
                    archive_slots: 2,
                    member_rows: 4096,
                    transfer_rows: 4096,
                    receipt_rows: 2,
                    data_rows: 4096,
                    handle_rows: 4096,
                    max_retained_data_bytes: 64 * 1024 * 1024,
                    max_construction_peak_data_bytes: 64 * 1024 * 1024,
                },
                std::mem::size_of::<Inner>() + 2 * std::mem::size_of::<Slot>(),
            )
            .unwrap();
        root
    }
    fn reservation() -> AudioCandidatePreparationReservation {
        AudioCandidatePreparationReservation {
            source_capacity: 1,
            member_rows: 1024,
            transfer_rows: 1024,
            handle_rows: 512,
            storage: hex_engine::AudioStorageReservation {
                allocation_items: 256,
                handle_obligations: 512,
                max_prepared_data_bytes: 16 * 1024 * 1024,
                max_construction_data_bytes: 16 * 1024 * 1024,
                caller_retained_bytes: 0,
                caller_construction_bytes: 0,
            },
        }
    }
    fn project() -> (
        hex_project::Project,
        hex_project::AssetId,
        hex_engine::Audio,
    ) {
        use hex_project::{Command, Created, Editor, Project};
        let mut editor = Editor::new(Project::new(48000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "audio".into(),
            })
            .unwrap()
        else {
            panic!("track");
        };
        editor.execute(Command::RemoveInstrument { track }).unwrap();
        let Created::Asset(asset) = editor
            .execute(Command::AddAsset {
                path: "audio.wav".into(),
                sample_rate: 48000,
                frames: 8,
            })
            .unwrap()
        else {
            panic!("asset");
        };
        editor
            .execute(Command::PlaceClip {
                track,
                asset,
                start: 0,
                duration: 8,
                source_offset: 0,
                source_length: 8,
                looping: false,
            })
            .unwrap();
        (
            editor.project().clone(),
            asset,
            hex_engine::Audio::new(48000, vec![[0.25; 2]; 8]).unwrap(),
        )
    }
    #[test]
    fn reserved_slot_incarnation_survives_actual_preparation_close_and_normal_cleanup() {
        if !hex_engine::audio_storage_constructor_layout_supported() {
            return;
        }
        let root = root();
        let custody = &root.0.eligible;
        let token = custody
            .reserve_runtime_preparation(reservation(), Vec::new())
            .ok()
            .unwrap();
        let held = lock(&custody.bank).publication.retained_bytes().unwrap();
        assert!(held >= reservation().storage.max_prepared_data_bytes);
        assert!(
            custody
                .reserve_runtime_preparation(reservation(), Vec::new())
                .is_err()
        );
        let (project, asset, audio) = project();
        assert!(
            custody
                .with_runtime_preparation(
                    AudioCandidatePreparationToken {
                        incarnation: token.incarnation + 1,
                        ..token
                    },
                    |_| panic!("stale closure ran")
                )
                .is_err()
        );
        assert_eq!(
            custody
                .with_runtime_preparation(token, |cell| {
                    cell.copy_resident_source_on_control(asset, &audio)?;
                    cell.prepare_on_control(
                        &project,
                        11,
                        hex_engine::AudioSourceContext {
                            project_generation: 2,
                            editor_revision: 8,
                        },
                    )?;
                    custody.close(); // closure wins after constructing the actual owner
                    Ok(())
                })
                .unwrap(),
            AudioCandidatePreparationProgress::Retiring
        );
        {
            let bank = lock(&custody.bank);
            assert_eq!(
                bank.publication.slots[token.slot].incarnation,
                token.incarnation
            );
            assert_eq!(bank.publication.slots[token.slot].phase, Phase::Reserved);
            assert_eq!(bank.publication.retained_bytes(), Some(held));
        }
        std::thread::scope(|scope| {
            scope
                .spawn(|| custody.advance_publications_on_control(&root, false))
                .join()
                .unwrap();
        });
        assert!(custody.complete());
        assert_eq!(lock(&custody.bank).publication.retained_bytes(), Some(0));
        assert!(lock(&custody.work).publication.preparations[token.slot].is_none());
        assert!(custody.with_runtime_preparation(token, |_| Ok(())).is_err());
    }
    #[test]
    fn cancellation_and_pressure_preserve_slot_until_partial_sources_and_wrappers_are_destroyed() {
        if !hex_engine::audio_storage_constructor_layout_supported() {
            return;
        }
        let root = root();
        let custody = &root.0.eligible;
        let mut excessive = reservation();
        excessive.storage.max_prepared_data_bytes = usize::MAX;
        excessive.storage.max_construction_data_bytes = usize::MAX;
        assert!(
            custody
                .reserve_runtime_preparation(excessive, Vec::new())
                .is_err()
        );
        assert_eq!(lock(&custody.bank).publication.occupied(), Some(0));
        let token = custody
            .reserve_runtime_preparation(reservation(), Vec::new())
            .ok()
            .unwrap();
        let (_, asset, audio) = project();
        assert_eq!(
            custody
                .with_runtime_preparation(token, |cell| cell
                    .copy_resident_source_on_control(asset, &audio))
                .unwrap(),
            AudioCandidatePreparationProgress::Collecting
        );
        assert!(custody.cancel_runtime_preparation(token));
        assert_eq!(lock(&custody.bank).publication.occupied(), Some(1));
        assert_eq!(
            custody
                .with_runtime_preparation(token, |_| panic!("cancelled closure ran"))
                .unwrap(),
            AudioCandidatePreparationProgress::Retiring
        );
        custody.advance_publications_on_control(&root, false);
        assert_eq!(lock(&custody.bank).publication.occupied(), Some(0));
        let reused = custody
            .reserve_runtime_preparation(reservation(), Vec::new())
            .ok()
            .unwrap();
        assert_eq!(reused.slot, token.slot);
        assert!(reused.incarnation > token.incarnation);
        assert!(!custody.cancel_runtime_preparation(token));
        assert!(custody.cancel_runtime_preparation(reused));
        custody.advance_publications_on_control(&root, false);
        assert!(custody.complete());
    }
    #[test]
    fn partial_construction_unwind_keeps_actual_cell_charge_and_first_payload_without_retry() {
        if !hex_engine::audio_storage_constructor_layout_supported() {
            return;
        }
        use std::sync::atomic::AtomicUsize;
        struct Payload(Arc<AtomicUsize>);
        impl Drop for Payload {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let root = root();
        let custody = &root.0.eligible;
        let token = custody
            .reserve_runtime_preparation(reservation(), Vec::new())
            .ok()
            .unwrap();
        let (_, asset, audio) = project();
        let drops = Arc::new(AtomicUsize::new(0));
        let payload = drops.clone();
        assert!(matches!(
            custody
                .with_runtime_preparation(token, |cell| {
                    cell.copy_resident_source_on_control(asset, &audio)?;
                    std::panic::panic_any(Payload(payload));
                })
                .unwrap(),
            AudioCandidatePreparationProgress::Refused(_)
        ));
        let held = lock(&custody.bank).publication.retained_bytes().unwrap();
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        for _ in 0..2 {
            custody.advance_publications_on_control(&root, true);
        }
        assert_eq!(lock(&custody.bank).publication.retained_bytes(), Some(held));
        assert_eq!(
            lock(&custody.bank).publication.slots[token.slot].phase,
            Phase::Failed
        );
        assert!(
            lock(&custody.work).publication.preparations[token.slot]
                .as_ref()
                .unwrap()
                .cell
                .is_some()
        );
        assert!(
            custody
                .with_runtime_preparation(token, |_| panic!("failed closure retried"))
                .is_err()
        );
        assert_eq!(drops.load(Ordering::SeqCst), 0);
    }
}
