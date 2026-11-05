//! Ordinary graph read associations borrow the same retained N2 source domains.
//! The existing ordinary publication/resource book owns each graph and bounds
//! their count. This adds no runtime cell, reader, page endpoint or reaper. Each
//! private association moves with that graph through the existing return ring;
//! only control releases domain uses and destroys its finite demand backing.
use super::*;
use view::Correlation;

fn covers_resource(
    owner: &hex_stream::ConstructorStreamDomain,
    source: &hex_engine::StreamResourceMetadata,
) -> bool {
    match source {
        hex_engine::StreamResourceMetadata::Audio(source) => owner.covers_source(source),
        hex_engine::StreamResourceMetadata::Scalar(source) => owner.covers_scalar_source(source),
    }
}

#[derive(Clone, Copy)]
pub(in crate::candidate_custody) struct OrdinaryReservation {
    pub(in crate::candidate_custody) extents: usize,
    pub(in crate::candidate_custody) metadata_bytes: usize,
}
impl OrdinaryReservation {
    pub(in crate::candidate_custody) fn validate_requested(self) -> Result<Self, &'static str> {
        if self.extents == 0 || self.metadata_bytes == 0 {
            return Err("ordinary stream reservation is empty");
        }
        if self
            .extents
            .checked_mul(2)
            .and_then(|n| n.checked_mul(std::mem::size_of::<StreamResourceExtent>()))
            .and_then(|n| n.checked_add(std::mem::size_of::<OrdinaryStreams>()))
            .is_none_or(|n| n > self.metadata_bytes)
        {
            return Err("ordinary stream metadata reservation exceeded");
        }
        Ok(self)
    }
}
pub(crate) struct OrdinaryStreams {
    root: u64,
    revision: u64,
    context: hex_engine::AudioSourceContext,
    refs: [Option<AudioCandidateDomainToken>; 2],
    workspace: Option<Workspace>,
}
impl OrdinaryStreams {
    pub(crate) fn has_install_pending(&self) -> bool {
        self.workspace.as_ref().is_some_and(|workspace| {
            workspace.role == Some(hex_stream::ProtectionRole::Candidate)
                && workspace.stamps.iter().any(Option::is_some)
        })
    }
    #[cfg(all(test, feature = "stream-test-support", feature = "allocation-probe"))]
    pub(crate) fn test_domain_cached(
        &self,
        domains: &mut [Option<DomainEnvelope>; 2],
        source: hex_engine::StreamResourceKey,
    ) -> bool {
        let Some(workspace) = self.workspace.as_ref() else {
            return false;
        };
        self.refs.iter().flatten().any(|token| {
            let rows = &workspace.extents[token.slot];
            super::demand::envelope(domains, *token)
                .ok()
                .and_then(|pages| pages.inner.loan.as_ref())
                .is_some_and(|loan| {
                    rows.iter()
                        .any(|row| row.source == source && loan.test_has_resource_extent(*row))
                })
        })
    }
    #[cfg(all(test, feature = "stream-test-support", feature = "allocation-probe"))]
    pub(crate) fn test_domain_protected(
        &self,
        domains: &mut [Option<DomainEnvelope>; 2],
        source: hex_engine::StreamResourceKey,
    ) -> bool {
        let Some(workspace) = self.workspace.as_ref() else {
            return false;
        };
        let Some(role) = workspace.role else {
            return false;
        };
        self.refs.iter().flatten().any(|token| {
            let Some(stamp) = workspace.stamps[token.slot] else {
                return false;
            };
            let rows = &workspace.extents[token.slot];
            rows.iter().any(|row| row.source == source)
                && super::demand::envelope(domains, *token)
                    .ok()
                    .and_then(|pages| pages.inner.loan.as_ref())
                    .is_some_and(|loan| loan.is_resource_protected(role, stamp, rows))
        })
    }
    pub(crate) fn take_handoff_diagnostic(&mut self) -> Option<hex_engine::PlaybackHandoffError> {
        self.workspace.as_mut()?.handoff_diagnostic.take()
    }
    pub(crate) fn take_source_changed(&mut self) -> bool {
        self.workspace
            .as_mut()
            .is_some_and(|workspace| std::mem::take(&mut workspace.source_changed))
    }
    pub(crate) fn view<'a>(
        &'a mut self,
        playback: &'a mut hex_engine::playback::Playback,
    ) -> ActiveView<'a> {
        ActiveView {
            state: StateView {
                runtime: Some(playback.stream_playback(self.context)),
                streaming: &mut self.workspace,
                domain_refs: self.refs,
                context_generation: self.context.project_generation,
                correlation: Correlation {
                    root: self.root,
                    owner: hex_stream::ReadinessArtifact::Ordinary {
                        publication: self.revision,
                    },
                },
            },
        }
    }
    #[allow(clippy::too_many_arguments)] // Actual borrowed predecessor and exact finite block/command prefix.
    pub(crate) fn prepare_install(
        &mut self,
        playback: &mut hex_engine::playback::Playback,
        previous: Option<(
            &mut hex_engine::playback::Playback,
            hex_engine::AudioSourceContext,
            u64,
        )>,
        domains: &mut [Option<DomainEnvelope>; 2],
        pending_play: bool,
        prefix: u64,
        frames: usize,
    ) -> Result<Option<hex_engine::StreamChunkWindow>, hex_engine::Invalid> {
        let mut empty = None;
        let mut old = previous.map(|(runtime, context, revision)| StateView {
            runtime: Some(runtime.stream_playback(context)),
            streaming: &mut empty,
            domain_refs: [None; 2],
            context_generation: context.project_generation,
            correlation: Correlation {
                root: self.root,
                owner: hex_stream::ReadinessArtifact::Ordinary {
                    publication: revision,
                },
            },
        });
        self.view(playback).state.prepare_stream_window(
            old.as_mut(),
            &mut StreamInstall {
                domains,
                pending_play,
                command_prefix: prefix,
                frames,
            },
            hex_stream::ProtectionRole::Candidate,
        )
    }
    pub(crate) fn prepare_commit<'a>(
        &mut self,
        domains: &'a mut [Option<DomainEnvelope>; 2],
    ) -> Result<[Option<hex_stream::PreparedRolePromotion<'a>>; 2], hex_engine::Invalid> {
        // Only demand backing and actual domain tokens are needed to reserve
        // promotion. No runtime borrow or musical mutation occurs here.
        StateView {
            runtime: None,
            streaming: &mut self.workspace,
            domain_refs: self.refs,
            context_generation: self.context.project_generation,
            correlation: Correlation {
                root: self.root,
                owner: hex_stream::ReadinessArtifact::Ordinary {
                    publication: self.revision,
                },
            },
        }
        .prepare_stream_commit(domains)
    }
    pub(crate) fn cancel_install(&mut self, domains: &mut [Option<DomainEnvelope>; 2]) {
        StateView {
            runtime: None,
            streaming: &mut self.workspace,
            domain_refs: self.refs,
            context_generation: self.context.project_generation,
            correlation: Correlation {
                root: self.root,
                owner: hex_stream::ReadinessArtifact::Ordinary {
                    publication: self.revision,
                },
            },
        }
        .cancel_stream_pending(domains);
    }
    pub(crate) fn record_install(&mut self) {
        StateView {
            runtime: None,
            streaming: &mut self.workspace,
            domain_refs: self.refs,
            context_generation: self.context.project_generation,
            correlation: Correlation {
                root: self.root,
                owner: hex_stream::ReadinessArtifact::Ordinary {
                    publication: self.revision,
                },
            },
        }
        .record_stream_commit();
    }
    pub(crate) fn has_action(&self) -> bool {
        self.workspace
            .as_ref()
            .is_some_and(|w| w.pending_action.is_some())
    }
}
impl Custody {
    pub(in crate::candidate_custody) fn configure_ordinary_streams(
        &self,
        reservation: OrdinaryReservation,
    ) -> Result<(), &'static str> {
        let reservation = reservation.validate_requested()?;
        let workspace = Workspace::prepare(reservation.extents)?;
        if workspace
            .backing_bytes()
            .and_then(|n| n.checked_add(std::mem::size_of::<OrdinaryStreams>()))
            .is_none_or(|n| n > reservation.metadata_bytes)
        {
            return Err("ordinary stream metadata reservation exceeded");
        }
        let mut bank = lock(&self.bank);
        if !self.enabled()
            || bank.closing
            || bank.ordinary.is_some()
            || bank.publication.reservation.is_some()
            || bank.domains.iter().any(|d| d.phase != Phase::Vacant)
        {
            return Err("ordinary streams require a vacant source root");
        }
        bank.ordinary = Some(reservation);
        Ok(())
    }
    pub(in crate::candidate_custody) fn ordinary_streams_configured(&self) -> bool {
        lock(&self.bank).ordinary.is_some()
    }
    pub(in crate::candidate_custody) fn prepare_ordinary_streams(
        &self,
        playback: &hex_engine::playback::Playback,
        revision: u64,
        context: hex_engine::AudioSourceContext,
    ) -> Result<Box<OrdinaryStreams>, &'static str> {
        if revision == 0 {
            return Err("ordinary stream publication is invalid");
        }
        let reservation = lock(&self.bank)
            .ordinary
            .ok_or("ordinary stream publication is not reserved")?;
        // Allocated on control before taking any actual source association.
        let workspace = Workspace::prepare(reservation.extents)?;
        if workspace
            .backing_bytes()
            .and_then(|n| n.checked_add(std::mem::size_of::<OrdinaryStreams>()))
            .is_none_or(|n| n > reservation.metadata_bytes)
        {
            return Err("ordinary stream metadata reservation exceeded");
        }
        let mut bank = lock(&self.bank);
        if bank.closing || bank.ordinary.is_none() {
            return Err("ordinary source root is closing");
        }
        let mut refs = [None; 2];
        for source in playback
            .stream_resources_on_control()
            .map_err(|_| "ordinary prepared source inventory unavailable")?
        {
            let index = bank
                .domains
                .iter()
                .position(|slot| {
                    slot.phase == Phase::Accepted
                        && !slot.retiring
                        && slot.loan_outstanding
                        && slot
                            .owner
                            .as_ref()
                            .is_some_and(|owner| covers_resource(owner, &source))
                })
                .ok_or("ordinary graph lacks its actual retained source catalog")?;
            let slot = &bank.domains[index];
            if slot.uses.checked_add(1).is_none() {
                return Err("ordinary source association overflow");
            }
            refs[index] = Some(AudioCandidateDomainToken {
                root: bank.root,
                slot: index,
                incarnation: slot.incarnation,
            });
        }
        // Callback demand routes by typed key within these retained domains.
        // An exact match in one domain cannot authorize a conflicting match
        // under the same key in another selected domain.
        for source in playback
            .stream_resources_on_control()
            .map_err(|_| "ordinary prepared source inventory unavailable")?
        {
            for token in refs.iter().flatten() {
                let owner = bank
                    .domains
                    .get(token.slot)
                    .and_then(|slot| slot.owner.as_ref())
                    .ok_or("ordinary retained source catalog is unavailable")?;
                if owner.covers_resource_key(source.key()) && !covers_resource(owner, &source) {
                    return Err("ordinary stream source key has conflicting domain facts");
                }
            }
        }
        // Every fallible check precedes use-count changes; the move-only private
        // carrier is created only here and remains in its graph publication.
        let streams = Box::new(OrdinaryStreams {
            root: bank.root,
            revision,
            context,
            refs,
            workspace: Some(workspace),
        });
        for token in refs.into_iter().flatten() {
            bank.domains[token.slot].uses += 1;
        }
        Ok(streams)
    }
    pub(in crate::candidate_custody) fn release_ordinary_streams(
        &self,
        streams: Box<OrdinaryStreams>,
    ) {
        let mut bank = lock(&self.bank);
        let valid = bank.root == streams.root
            && streams.refs.iter().flatten().all(|token| {
                bank.domains
                    .get(token.slot)
                    .is_some_and(|slot| slot.matches(*token) && slot.uses != 0)
            });
        if !valid {
            // Fail closed on an impossible private-carrier mismatch. Never
            // manufacture freed capacity or reclaim a possibly borrowed pair.
            self.mark_closing(&mut bank);
            return;
        }
        for token in streams.refs.into_iter().flatten() {
            bank.domains[token.slot].uses -= 1;
        }
        // Demand Vecs are destroyed here on control, never on callback return.
    }
}
