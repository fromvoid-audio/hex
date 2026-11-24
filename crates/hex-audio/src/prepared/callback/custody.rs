//! Candidate installation, custody drain/retirement, and graph return helpers.
use crate::prepared::*;

impl Callback {
    pub(in crate::prepared) fn install_candidate(
        &mut self,
        value: crate::candidate_custody::QueuedCandidate,
    ) -> Result<(), crate::candidate_custody::QueuedCandidate> {
        #[cfg(test)]
        let allowed = self.test_admit_candidate;
        #[cfg(not(test))]
        let allowed = false;
        if !allowed
            || self.active.is_some()
            || self.deferred.is_some()
            || self.status.closing.load(Acquire)
        {
            return Err(value);
        }
        let active =
            crate::candidate_custody::ActiveCandidate::install(value, &self.candidate_root)?;
        self.active = Some(ActivePublication::Candidate(active));
        self.status.publication_pending.store(false, Release);
        Ok(())
    }
    pub(crate) fn native_endpoint_bank_data_bytes(&self) -> Option<usize> {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            self.native_sources
                .data_bytes()?
                .checked_add(self.paused_native_sources.data_bytes()?)
        }
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        {
            Some(0)
        }
    }
    pub(crate) fn matches_native_status(&self, status: &Arc<Status>) -> bool {
        Arc::ptr_eq(&self.status, status)
    }
    pub(crate) fn has_action_writer(&self) -> bool {
        self.action_writer.is_some()
    }
    pub(crate) fn action_writer_identity(&self) -> Option<usize> {
        self.action_writer
            .as_ref()
            .map(action_outcomes::Writer::identity)
    }
    /// Consume the matched producer and writer only under the exact native
    /// root's post-guard, post-stream-handle release authority.
    pub(crate) fn retire_action_outcomes_on_control(
        &mut self,
        client: &mut ClientEndpoints,
        authority: &crate::native_callback_owner::FinalReleaseAuthority,
    ) -> Result<(), ()> {
        if !authority.matches(self.candidate_root.root_id())
            || !client.matches(&self.status)
            || !client.matches_retired_scalar_output(
                Arc::as_ptr(&self.status) as usize,
                self.candidate_root.root_id(),
            )
        {
            return Err(());
        }
        let Some(writer) = self.action_writer.take() else {
            return Err(());
        };
        let Some(admission) = client.take_action_admission() else {
            self.action_writer = Some(writer);
            return Err(());
        };
        match writer.interrupt_after_quiescence(admission) {
            Ok(()) => Ok(()),
            Err((writer, admission)) => {
                self.action_writer = Some(writer);
                client.restore_action_admission(admission);
                Err(())
            }
        }
    }
    pub(crate) fn candidate_root_for_native_boundary(
        &self,
    ) -> crate::candidate_custody::CandidateRoot {
        self.candidate_root.clone()
    }
    pub(crate) fn belongs_to_candidate_root(
        &self,
        root: &crate::candidate_custody::CandidateRoot,
    ) -> bool {
        self.candidate_root.same_root(root)
    }
    pub(crate) fn has_active_ordinary_streams(&self) -> bool {
        matches!(&self.active, Some(ActivePublication::Legacy(publication)) if publication.streams.is_some())
    }
    /// Both callback carriers and the actual stream handle are terminal-owned.
    /// Release only the ordinary graph's source associations here; its actual
    /// Playback/resource-book owners still follow their existing destructors.
    pub(crate) fn release_final_ordinary_streams_on_control(
        &mut self,
        authority: &crate::native_callback_owner::FinalReleaseAuthority,
    ) -> bool {
        if !authority.matches(self.candidate_root.root_id()) {
            return false;
        }
        if let Some(ActivePublication::Legacy(publication)) = self.active.as_mut() {
            publication.release_streams_on_control(&self.candidate_root);
            // Both carriers and the actual backend are terminal here. The
            // remaining graph is inert metadata; releasing native references
            // cannot admit reuse because this Root is already closing. Keep
            // the original callback Box/resources in their terminal custody.
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            drop(publication.native_references.take());
        }
        true
    }
    pub(crate) fn has_active_candidate(&self) -> bool {
        matches!(
            self.active,
            Some(ActivePublication::Candidate(_) | ActivePublication::Eligible(_))
        )
    }
    pub(crate) fn extract_final_candidate_on_control(
        &mut self,
        authority: &crate::native_callback_owner::FinalReleaseAuthority,
        destination: &mut Option<crate::candidate_custody::FinalActiveReturn>,
    ) -> bool {
        if destination.is_some() || !authority.matches(self.candidate_root.root_id()) {
            return false;
        }
        if !self.has_active_candidate() {
            return true;
        }
        // Prevalidated destination and authority; no provider/destructor occurs
        // between taking the active owner and securing its persistent terminal slot.
        match self.active.take() {
            Some(ActivePublication::Candidate(active)) => match active.return_final(authority) {
                Ok(returned) => *destination = Some(returned),
                Err(active) => {
                    self.active = Some(ActivePublication::Candidate(active));
                    return false;
                }
            },
            Some(ActivePublication::Eligible(active)) => match active.return_final(authority) {
                Ok(returned) => {
                    *destination = Some(crate::candidate_custody::FinalActiveReturn::Eligible(
                        returned,
                    ))
                }
                Err(active) => {
                    self.active = Some(ActivePublication::Eligible(active));
                    return false;
                }
            },
            other => {
                self.active = other;
                return false;
            }
        }
        true
    }
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(in crate::prepared) fn apply_candidate_action(&mut self, action: PlaybackAction) {
        if let Some(ActivePublication::Candidate(active)) = self.active.as_mut()
            && active.owner.test_apply_action(action).is_err()
        {
            self.status.device_fault.store(true, Relaxed);
        }
    }
    pub(crate) fn close_candidate_custody_on_control(&mut self) {
        self.status.closing.store(true, Release);
        self.release_device_insert_fade_pins();
        self.candidate_root.close();
    }
    pub(crate) fn has_domain_pages(&self) -> bool {
        self.domain_pages.iter().any(Option::is_some)
    }
    pub(crate) fn extract_domain_pages_on_control(
        &mut self,
        destination: &mut [Option<crate::candidate_custody::DomainEnvelope>; 2],
    ) -> bool {
        if destination.iter().any(Option::is_some)
            || self.domain_pages.iter().enumerate().any(|(index, pages)| {
                pages.as_ref().is_some_and(|pages| {
                    pages.index_for(self.candidate_root.root_id()) != Some(index)
                })
            })
        {
            return false;
        }
        for (from, to) in self.domain_pages.iter_mut().zip(destination) {
            *to = from.take();
            if let Some(pages) = to.as_ref() {
                pages.departed_callback();
            }
        }
        true
    }
    pub(crate) fn drain_candidate_custody_on_control(&mut self) -> bool {
        self.close_candidate_custody_on_control();
        if self.terminal_disposal_pending {
            return false;
        }
        // Persistent held custody precedes any destructor. A failed legacy
        // destructor latches Pending, just like the outer callback carrier.
        // Candidate carriers only move into the exact root/slot association.
        for _ in 0..2 {
            // one existing held return plus the one-slot input ring
            if self.held_retirement.is_none() {
                self.held_retirement = self
                    .deferred_eligible
                    .take()
                    .map(|value| ReturnedPublication::Eligible(value.reject_before_install()))
                    .or_else(|| match self.publications.pop().ok()? {
                        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                        QueuedPublication::NativeEpoch { .. } => None,
                        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                        QueuedPublication::Native(endpoint) => {
                            Some(ReturnedPublication::Native(endpoint))
                        }
                        QueuedPublication::Domain(pages) => {
                            Some(ReturnedPublication::Domain(pages))
                        }
                        QueuedPublication::Eligible(value) => {
                            Some(ReturnedPublication::Eligible(value.reject_before_install()))
                        }
                        QueuedPublication::Legacy(value) => {
                            Some(ReturnedPublication::Legacy(value))
                        }
                        QueuedPublication::Candidate(value) => Some(
                            ReturnedPublication::Candidate(value.reject_before_install()),
                        ),
                    });
            }
            if matches!(
                self.held_retirement.as_ref(),
                Some(ReturnedPublication::Legacy(_))
            ) {
                if let Some(ReturnedPublication::Legacy(publication)) =
                    self.held_retirement.as_mut()
                {
                    publication.release_streams_on_control(&self.candidate_root);
                }
                self.terminal_disposal_pending = true;
                drop(self.held_retirement.take());
                self.terminal_disposal_pending = false;
            } else if let Some(value) = self.held_retirement.take() {
                let refused = match value {
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    ReturnedPublication::Native(endpoint) => self
                        .candidate_root
                        .ingest_native_endpoint(endpoint)
                        .err()
                        .map(ReturnedPublication::Native),
                    ReturnedPublication::Domain(pages) => {
                        if let Some(index) = pages.index_for(self.candidate_root.root_id())
                            && self.domain_pages[index].is_none()
                        {
                            self.domain_pages[index] = Some(pages);
                            None
                        } else {
                            Some(ReturnedPublication::Domain(pages))
                        }
                    }
                    ReturnedPublication::Candidate(value) => self
                        .candidate_root
                        .ingest(value)
                        .err()
                        .map(ReturnedPublication::Candidate),
                    ReturnedPublication::Eligible(value) => self
                        .candidate_root
                        .ingest_eligible(value)
                        .err()
                        .map(ReturnedPublication::Eligible),
                    // Handled above without holding any owner across destruction.
                    ReturnedPublication::Legacy(value) => Some(ReturnedPublication::Legacy(value)),
                };
                if let Some(value) = refused {
                    self.held_retirement = Some(value);
                    return false;
                }
            }
        }
        self.held_retirement.is_none() && self.publications.is_empty()
    }

    pub(in crate::prepared) fn return_graph(&mut self, publication: Publication) {
        self.return_graph_inner(publication, false);
    }

    /// Return a rejected pending graph and clear `publication_pending` before
    /// publishing its terminal receipt state so control threads observing the
    /// receipt never see a stale `publication_in_flight()` window (for example,
    /// when offering device-insert consent). Keeps the receipt owned inside
    /// `publication` so no `Arc` can drop on the audio callback thread.
    pub(in crate::prepared) fn return_rejected_graph(&mut self, publication: Publication) {
        self.return_graph_inner(publication, true);
    }

    fn return_graph_inner(&mut self, mut publication: Publication, clear_pending: bool) {
        if publication.approved_device_insert().is_some() {
            self.release_device_insert_fade_pins();
        }
        if (publication.approved_device_insert().is_some()
            || publication.tempo_cut().is_some()
            || publication.playback.native_removal_pending())
            && let Some(streams) = publication.streams.as_mut()
        {
            streams.cancel_install(&mut self.domain_pages);
        }
        self.release_refused_device_insert_cut(&publication);
        self.restore_refused_native_removal(&mut publication);
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        self.restore_parameter_predecessor_prewarm(&publication);
        if clear_pending {
            self.status.publication_pending.store(false, Release);
        }
        if let Some(receipt) = publication.fresh_runtime_cancellation.as_ref() {
            // The callback is the sole writer of these refusal fields. A
            // refused graph is returned in the same callback branch that
            // publishes the exact revision's reason.
            let refusal = self
                .status
                .rejected_revision()
                .filter(|(revision, _)| *revision == publication.revision)
                .map(|(_, reason)| reason);
            let closing = self.status.closing.load(Acquire);
            let disposition = refusal
                .or_else(|| (!closing).then_some(PlaybackHandoffRejection::RuntimeFenceChanged));
            if receipt.returned(disposition) && closing && refusal.is_none() {
                self.reject_publication(
                    publication.revision,
                    PlaybackHandoffRejection::FreshRuntimeCancelled,
                );
            }
        }
        self.return_publication(ReturnedPublication::Legacy(publication));
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(in crate::prepared) fn restore_parameter_predecessor_prewarm(
        &mut self,
        publication: &Publication,
    ) {
        let PublicationPolicy::InheritUnderNativeParameter { source, .. } = publication.policy
        else {
            return;
        };
        if self.status.closing.load(Acquire) {
            return;
        }
        if let Some(active) = self.active.as_mut().and_then(ActivePublication::legacy_mut)
            && active.revision == source.audio_revision
            && active.source_context == Some(source.context)
            && let Some(mut view) = active.stream_view()
        {
            view.publication_abandoned();
        }
    }
    pub(in crate::prepared) fn return_publication(&mut self, publication: ReturnedPublication) {
        if let ReturnedPublication::Domain(pages) = &publication {
            pages.departed_callback();
        }
        if let Err(PushError::Full(publication)) = self.retired.push(publication) {
            self.held_retirement = Some(publication);
        }
    }
}
