//! Intact resident constructor transfer into a fresh native publication Root.
use super::*;
impl Custody {
    pub(in crate::candidate_custody) fn check_current_sampler_preparation_context(
        &self,
        token: AudioCandidatePreparationToken,
        expected: hex_engine::AudioSourceContext,
    ) -> Result<(), &'static str> {
        let work = self
            .work
            .try_lock()
            .map_err(|_| "candidate preparation is busy")?;
        let execution = work
            .publication
            .preparations
            .get(token.slot)
            .and_then(Option::as_ref)
            .filter(|e| e.token == token && !e.pending && !e.failed && e.refusal.is_none())
            .ok_or("candidate preparation token differs")?;
        if execution
            .cell
            .as_ref()
            .and_then(|c| c.current_sampler_context())
            != Some(expected)
        {
            return Err("current sampler preparation context differs");
        }
        Ok(())
    }

    pub(in crate::candidate_custody) fn adopt_runtime_preparation(
        &self,
        root: &CandidateRoot,
        token: AudioCandidatePreparationToken,
        context: hex_engine::AudioSourceContext,
        rate: u32,
        source: &mut Option<hex_engine::AudioCandidatePreparation>,
    ) -> Result<(), &'static str> {
        let mut work = self
            .work
            .try_lock()
            .map_err(|_| "candidate preparation is busy")?;
        let work = &mut *work;
        let barrier = lock(&root.0.bank);
        if barrier.closing || barrier.retiring {
            return Err("candidate Root is closing");
        }
        let (ledger, pending) = {
            let bank = lock(&self.bank);
            if bank.closing
                || bank.root != token.root
                || bank.root != root.root_id()
                || bank.publication.stream_extents.is_some()
                || bank
                    .publication
                    .slots
                    .iter()
                    .enumerate()
                    .any(|(i, s)| i != token.slot && s.phase != Phase::Vacant)
                || bank.domains.iter().any(|d| d.phase != Phase::Vacant)
            {
                return Err("current sampler adoption requires a fresh resident Root");
            }
            let slot = bank
                .publication
                .slots
                .get(token.slot)
                .filter(|s| {
                    s.incarnation == token.incarnation
                        && s.phase == Phase::Reserved
                        && !s.preparation_retiring
                })
                .ok_or("candidate preparation token differs")?;
            let pending = slot
                .preparation_reservation
                .ok_or("candidate preparation reservation differs")?;
            let limits = bank
                .publication
                .reservation
                .ok_or("candidate publication is not reserved")?;
            let mut ledger = limits.ledger();
            ledger.root_rows = 2;
            ledger.data_rows = pending.storage.allocation_items;
            ledger.handle_rows = pending.handle_rows;
            (ledger, pending)
        };
        let execution = work
            .publication
            .preparations
            .get_mut(token.slot)
            .and_then(Option::as_mut)
            .filter(|e| {
                e.token == token
                    && e.cell.is_none()
                    && !e.pending
                    && !e.failed
                    && e.refusal.is_none()
                    && e.domains.capacity() == 0
            })
            .ok_or("candidate adoption destination is occupied")?;
        let cell = source
            .as_mut()
            .ok_or("current sampler preparation is missing")?;
        let summary = cell
            .preflight_current_sampler_adoption_on_control(
                context,
                rate,
                pending.source_capacity,
                ledger,
                std::mem::size_of::<State>(),
            )
            .map_err(|_| "current sampler constructor context or DATA refused")?;
        if summary.member_rows > pending.member_rows
            || summary.transfer_rows > pending.transfer_rows
            || summary.data.retained_bytes > pending.storage.max_prepared_data_bytes
            || summary.data.construction_peak_bytes > pending.storage.max_construction_data_bytes
        {
            return Err("current sampler destination reservation exceeded");
        }
        let owner = cell
            .owner_on_control()
            .ok_or("candidate preparation is not ready")?;
        self.preflight_stage(
            root,
            owner,
            &execution.domains,
            Header {
                revision: 1,
                context_generation: context.project_generation,
                policy: crate::prepared::PublicationPolicy::InheritRuntime,
            },
            summary,
            Some(token),
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            &work.sources,
        )?;
        // All refusal paths precede this fixed Option move. No engine identity,
        // bank/scope binding or runtime address is relabeled to the new Root.
        execution.cell = source.take();
        Ok(())
    }
}
