//! Revocable early Candidate-role pins for one actual PendingRoute boundary.
//! These receipts never authorize activation: due rendering uses Transition.
use super::*;
use hex_stream::ProtectionRole;
impl ActiveView<'_> {
    fn scheduled_is_pending(&self, due: u64) -> bool {
        self.state.runtime.as_ref().is_some_and(|runtime| {
            runtime.status().is_some_and(|status| {
                (0..status.route_count).any(|index| {
                    runtime.session_route_status(index).is_some_and(|route| {
                        route
                            .pending
                            .is_some_and(|pending| pending.performance_frame == due)
                    })
                })
            })
        })
    }
    pub(crate) fn revoke_launch_prewarm(
        &mut self,
        domains: &mut [Option<DomainEnvelope>; 2],
    ) -> bool {
        let Some(workspace) = self.state.streaming.as_mut() else {
            return false;
        };
        let CandidateRole::LaunchPrewarm { ticket, due } = workspace.candidate_role else {
            return false;
        };
        // The role holder is private to this actual State and exact command/due.
        if !workspace
            .scheduled
            .is_some_and(|s| s.ticket == ticket && s.due == due)
        {
            return false;
        }
        workspace.candidate_role = CandidateRole::None;
        for token in self.state.domain_refs.into_iter().flatten() {
            if let Some(stamp) = workspace.prewarm_stamps[token.slot].take()
                && let Ok(pages) = super::demand::envelope(domains, token)
                && let Some(loan) = pages.inner.loan.as_mut()
            {
                let _ = loan.cancel(ProtectionRole::Candidate, stamp.request_epoch);
            }
        }
        true
    }
    pub(crate) fn publication_replaces_prewarm(
        &mut self,
        domains: &mut [Option<DomainEnvelope>; 2],
    ) {
        self.revoke_launch_prewarm(domains);
        if let Some(scheduled) = self
            .state
            .streaming
            .as_mut()
            .and_then(|w| w.scheduled.as_mut())
        {
            scheduled.allow_prewarm = false;
        }
    }
    pub(super) fn service_launch_prewarm(
        &mut self,
        domains: &mut [Option<DomainEnvelope>; 2],
        prefix: u64,
        frames: usize,
    ) -> bool {
        let Some(scheduled) = self.state.streaming.as_ref().and_then(|w| w.scheduled) else {
            return false;
        };
        let Some(status) = self.state.runtime.as_ref().and_then(|r| r.status()) else {
            return false;
        };
        if !self.scheduled_is_pending(scheduled.due) {
            self.revoke_launch_prewarm(domains);
            if let Some(w) = self.state.streaming.as_mut() {
                w.scheduled = None;
            }
            return false;
        }
        if !status.playing
            || status.performance_frame >= scheduled.due
            || !scheduled.allow_prewarm
            || scheduled.pressure_blocked
            || frames == 0
        {
            return false;
        }
        if let Some(w) = self.state.streaming.as_mut() {
            w.candidate_role = CandidateRole::LaunchPrewarm {
                ticket: scheduled.ticket,
                due: scheduled.due,
            };
        }
        let mut request = StreamInstall {
            domains,
            pending_play: false,
            command_prefix: prefix,
            frames,
        };
        if self
            .state
            .prepare_stream_window_at(
                None,
                &mut request,
                ProtectionRole::Candidate,
                Some(scheduled.due),
            )
            .is_err()
        {
            self.revoke_launch_prewarm(request.domains);
        }
        true
    }
    pub(super) fn observe_launch_activation(
        &mut self,
        domains: &mut [Option<DomainEnvelope>; 2],
    ) -> Option<u64> {
        let scheduled = self.state.streaming.as_ref()?.scheduled?;
        let status = self.state.runtime.as_ref()?.status()?;
        if status.performance_frame <= scheduled.due || self.scheduled_is_pending(scheduled.due) {
            return None;
        }
        self.revoke_launch_prewarm(domains);
        self.state.streaming.as_mut()?.scheduled = None;
        Some(scheduled.ticket)
    }
    pub(crate) fn inherit_schedule_observation(
        &mut self,
        previous: &mut ActiveView<'_>,
    ) -> Option<u64> {
        let mut scheduled = previous.state.streaming.as_mut()?.scheduled.take()?;
        // Called only after the real checked handoff. The closed constructor
        // starts without pending routes, and the single observational slot
        // forbids competing future schedules; any surviving due route is an
        // actual inherited target (partial cancellation can remove others).
        if self.state.correlation.root != previous.state.correlation.root
            || !self.scheduled_is_pending(scheduled.due)
        {
            return Some(scheduled.ticket);
        }
        let Some(workspace) = self.state.streaming.as_mut() else {
            return Some(scheduled.ticket);
        };
        scheduled.allow_prewarm = true;
        scheduled.pressure_blocked = false;
        workspace.scheduled = Some(scheduled);
        // Never copy old role/stamps/epochs. New context and demand will service
        // the successor's own endpoint view before any subsequent activation.
        None
    }
    pub(crate) fn refresh_scheduled_after_command(
        &mut self,
        domains: &mut [Option<DomainEnvelope>; 2],
    ) -> Option<u64> {
        if let Some(scheduled) = self.state.streaming.as_ref().and_then(|w| w.scheduled)
            && !self.scheduled_is_pending(scheduled.due)
        {
            self.revoke_launch_prewarm(domains);
            if let Some(w) = self.state.streaming.as_mut() {
                w.scheduled = None;
                return Some(scheduled.ticket);
            }
        }
        None
    }
}

#[cfg(all(test, feature = "allocation-probe"))]
impl Active {
    pub(crate) fn test_service_launch_prewarm(
        &mut self,
        domains: &mut [Option<DomainEnvelope>; 2],
        prefix: u64,
    ) -> bool {
        self.service_launch_prewarm(domains, prefix, 1)
    }
    pub(crate) fn test_launch_prewarm_protected(
        &mut self,
        domains: &mut [Option<DomainEnvelope>; 2],
        prefix: u64,
    ) -> bool {
        self.service_launch_prewarm(domains, prefix, 1);
        let Some(w) = self.state.streaming.as_ref() else {
            return false;
        };
        matches!(w.candidate_role, CandidateRole::LaunchPrewarm { .. })
            && self.state.domain_refs.into_iter().flatten().all(|token| {
                w.prewarm_stamps[token.slot].is_some_and(|stamp| {
                    super::demand::envelope(domains, token)
                        .ok()
                        .and_then(|p| p.inner.loan.as_ref())
                        .is_some_and(|loan| {
                            loan.is_resource_protected(
                                ProtectionRole::Candidate,
                                stamp,
                                &w.extents[token.slot],
                            )
                        })
                })
            })
    }
}
