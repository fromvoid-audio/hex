//! Borrowed access to the shared readiness/transport kernel. These views contain
//! no runtime, source, page endpoint or native-owner moves. Constructor custody,
//! cancellation and final destruction remain in the actual State/Root.
use super::*;
use hex_stream::ProtectionRole;

#[derive(Clone, Copy)]
pub(super) struct Correlation {
    pub(super) root: u64,
    pub(super) owner: hex_stream::ReadinessArtifact,
}
pub(super) struct StateView<'a> {
    pub(super) runtime: Option<hex_engine::StreamPlayback<'a>>,
    pub(super) streaming: &'a mut Option<Workspace>,
    pub(super) domain_refs: [Option<AudioCandidateDomainToken>; 2],
    pub(super) context_generation: u64,
    pub(super) correlation: Correlation,
}
pub(crate) struct ActiveView<'a> {
    pub(super) state: StateView<'a>,
}
pub(crate) struct QuantumPins {
    complete: bool,
    entries: [Option<(
        AudioCandidateDomainToken,
        hex_stream::CurrentQuantumReservation,
    )>; 2],
}
impl QuantumPins {
    pub(crate) fn complete(&self) -> bool {
        self.complete
    }
    pub(crate) fn release(self, domains: &mut [Option<DomainEnvelope>; 2]) {
        for (token, reservation) in self.entries.into_iter().flatten() {
            if let Ok(pages) = super::demand::envelope(domains, token)
                && let Some(loan) = pages.inner.loan.as_mut()
            {
                let _ = loan.release_current_quantum(&reservation);
            }
        }
    }
}
impl ActiveView<'_> {
    /// Protect the exact already-resident demand for the complete output buffer.
    pub(crate) fn reserve_current_quantum(
        &mut self,
        domains: &mut [Option<DomainEnvelope>; 2],
        frames: usize,
        pending_play: bool,
    ) -> Result<QuantumPins, hex_engine::Invalid> {
        let refs = self.state.domain_refs;
        let workspace = self
            .state
            .streaming
            .as_mut()
            .filter(|workspace| workspace.valid())
            .ok_or(hex_engine::Invalid("stream demand workspace unavailable"))?;
        if workspace.pending_action.is_some() {
            return Err(hex_engine::Invalid("stream action is pending"));
        }
        for rows in &mut workspace.quantum_extents {
            rows.clear();
        }
        let runtime = self
            .state
            .runtime
            .as_mut()
            .ok_or(hex_engine::Invalid("stream runtime unavailable"))?;
        let mut offset = 0usize;
        while offset < frames {
            let rows = &mut workspace.quantum_extents;
            let covered = runtime.visit_current_stream_quantum_at(
                u64::try_from(offset).map_err(|_| hex_engine::Invalid("stream offset overflow"))?,
                frames - offset,
                pending_play.then_some(true),
                &mut |extent| {
                    let index = refs
                        .iter()
                        .flatten()
                        .find_map(|token| {
                            domains
                                .get(token.slot)
                                .and_then(Option::as_ref)
                                .filter(|pages| pages.inner.token == *token)
                                .and_then(|pages| pages.inner.loan.as_ref())
                                .is_some_and(|loan| loan.covers_resource_key(extent.source))
                                .then_some(token.slot)
                        })
                        .ok_or(hex_engine::Invalid(
                            "stream demand source lacks actual domain",
                        ))?;
                    Workspace::insert(rows, workspace.limit, index, extent)
                },
            )?;
            if covered == 0 {
                break;
            }
            offset = offset
                .checked_add(covered)
                .ok_or(hex_engine::Invalid("stream offset overflow"))?;
        }
        let mut pins = QuantumPins {
            complete: true,
            entries: std::array::from_fn(|_| None),
        };
        for token in refs.into_iter().flatten() {
            let result = super::demand::envelope(domains, token)
                .and_then(|pages| {
                    pages
                        .inner
                        .loan
                        .as_mut()
                        .ok_or(hex_engine::Invalid("stream loan unavailable"))
                })
                .and_then(|loan| {
                    loan.reserve_resident_current_quantum(&workspace.quantum_extents[token.slot])
                        .map_err(|_| hex_engine::Invalid("current stream reservation refused"))
                });
            match result {
                Ok((reservation, complete)) => {
                    pins.complete &= complete;
                    pins.entries[token.slot] = Some((token, reservation));
                }
                Err(error) => {
                    pins.release(domains);
                    return Err(error);
                }
            }
        }
        Ok(pins)
    }
    pub(crate) fn capacity_conflict(&self) -> bool {
        self.state
            .streaming
            .as_ref()
            .is_some_and(|workspace| workspace.capacity_conflict)
    }
    /// A current render window has reserved readiness but has not committed.
    pub(crate) fn has_pending_render_window(&self) -> bool {
        self.state.streaming.as_ref().is_some_and(|workspace| {
            workspace.pending_action.is_none()
                && workspace.role == Some(ProtectionRole::Transition)
                && workspace.stamps.iter().any(Option::is_some)
        })
    }
    pub(crate) fn cancel_pending_render_window(
        &mut self,
        domains: &mut [Option<DomainEnvelope>; 2],
    ) {
        self.state.cancel_stream_pending(domains);
    }
}
impl State {
    fn stream_view(&mut self) -> StateView<'_> {
        StateView {
            runtime: self.runtime.as_mut().map(|r| r.stream_playback()),
            streaming: &mut self.streaming,
            domain_refs: self.domain_refs,
            context_generation: self.context_generation,
            correlation: Correlation {
                root: self.stamp.root,
                owner: self.stamp.owner.into(),
            },
        }
    }
    pub(in crate::candidate_custody::eligible::publication) fn cancel_stream_pending(
        &mut self,
        domains: &mut [Option<DomainEnvelope>; 2],
    ) {
        self.stream_view().cancel_stream_pending(domains);
    }
    pub(in crate::candidate_custody::eligible::publication) fn prepare_stream_window(
        &mut self,
        previous: Option<&mut State>,
        request: &mut StreamInstall<'_>,
        role: ProtectionRole,
    ) -> Result<Option<hex_engine::StreamChunkWindow>, hex_engine::Invalid> {
        let mut previous = previous.map(State::stream_view);
        self.stream_view()
            .prepare_stream_window(previous.as_mut(), request, role)
    }
    pub(in crate::candidate_custody::eligible::publication) fn prepare_stream_commit<'a>(
        &mut self,
        domains: &'a mut [Option<DomainEnvelope>; 2],
    ) -> Result<[Option<hex_stream::PreparedRolePromotion<'a>>; 2], hex_engine::Invalid> {
        self.stream_view().prepare_stream_commit(domains)
    }
    pub(in crate::candidate_custody::eligible::publication) fn record_stream_commit(&mut self) {
        self.stream_view().record_stream_commit();
    }
}
impl Active {
    pub(crate) fn stream_view(&mut self) -> ActiveView<'_> {
        ActiveView {
            state: self.state.stream_view(),
        }
    }
    pub(crate) fn streaming_enabled(&self) -> bool {
        self.state.streaming.is_some()
    }
    pub(crate) fn has_stream_action(&self) -> bool {
        self.state
            .streaming
            .as_ref()
            .is_some_and(|w| w.pending_action.is_some())
    }
    pub(crate) fn clear_stream_roles(&mut self, domains: &mut [Option<DomainEnvelope>; 2]) {
        self.stream_view().clear_stream_roles(domains);
    }
    pub(crate) fn publication_replaces_prewarm(
        &mut self,
        domains: &mut [Option<DomainEnvelope>; 2],
    ) {
        self.stream_view().publication_replaces_prewarm(domains);
    }
    pub(crate) fn inherit_schedule_observation(&mut self, previous: &mut Active) -> Option<u64> {
        self.stream_view()
            .inherit_schedule_observation(&mut previous.stream_view())
    }
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(super) fn service_launch_prewarm(
        &mut self,
        domains: &mut [Option<DomainEnvelope>; 2],
        prefix: u64,
        frames: usize,
    ) -> bool {
        self.stream_view()
            .service_launch_prewarm(domains, prefix, frames)
    }
}
