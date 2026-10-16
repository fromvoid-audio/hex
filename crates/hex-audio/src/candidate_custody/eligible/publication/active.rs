//! Same actual runtime/State Box in the single callback active slot. A held
//! successor claim anchors the actual retained successor until return metadata
//! normally drops on control. No callback lock, clone or destructor is needed.
use super::*;
pub(crate) struct Active {
    state: Box<State>,
}
pub(crate) struct FinalReturn(Returned);
pub(crate) enum InstallDecision {
    Ready,
    Retry,
    Refused(hex_engine::PlaybackHandoffRejection),
}
impl Claims {
    fn matches_state(&self, state: &State, root: &CandidateRoot) -> bool {
        self.stamp == state.stamp
            && state
                .runtime
                .as_ref()
                .is_some_and(|r| self.matches(root, r))
            && self.life.revision.load(Ordering::Acquire) == state.revision
    }
}
impl Drop for Claims {
    fn drop(&mut self) {
        if self.successor_dependency {
            // One private move-only paired dependency was acquired before the
            // real handoff. No public clone or counter-only producer exists.
            self.life.successor_dependencies.store(0, Ordering::Release);
        }
    }
}
impl Queued {
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(crate) fn test_set_transfer_allowance(&mut self, bytes: usize) -> Option<usize> {
        self.state
            .runtime
            .as_mut()?
            .test_set_owned_runtime_reservation(bytes)
    }
    pub(crate) fn revision(&self) -> u64 {
        self.state.revision
    }
    pub(crate) fn prepare_install(
        &mut self,
        previous: Option<&mut Active>,
        root: &CandidateRoot,
    ) -> InstallDecision {
        let next = &mut self.state;
        if !next
            .claims
            .as_ref()
            .is_some_and(|c| c.matches_state(next, root))
            || next.retired_runtime.is_some()
            || next.manifest.is_some()
            || next.covering_successor.is_some()
            || next.policy != crate::prepared::PublicationPolicy::InheritRuntime
        {
            return InstallDecision::Refused(
                hex_engine::PlaybackHandoffRejection::ConstructorClaimsUnavailable,
            );
        }
        if next
            .runtime
            .as_ref()
            .is_none_or(|r| r.contains_streamed_sources())
        {
            return InstallDecision::Refused(
                hex_engine::PlaybackHandoffRejection::StreamedPagesNotReady,
            );
        }
        let decision = next.claims.as_ref().map(|claims| &claims.life.cancellation);
        let Some(decision) = decision else {
            return InstallDecision::Refused(
                hex_engine::PlaybackHandoffRejection::ConstructorClaimsUnavailable,
            );
        };
        if decision.load(Ordering::Acquire) == CANCELLED {
            return InstallDecision::Refused(
                hex_engine::PlaybackHandoffRejection::FreshRuntimeCancelled,
            );
        }
        let Some(previous) = previous else {
            // Admission linearizes at this final open observation for an apply
            // that wins its CAS. Closure observed before it refuses; a decision
            // already in flight may finish under ordinary shutdown semantics.
            if !root.eligible_claims_open() {
                return InstallDecision::Refused(
                    hex_engine::PlaybackHandoffRejection::ConstructorClaimsUnavailable,
                );
            }
            return match decision.compare_exchange(
                PENDING,
                APPLYING,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) | Err(APPLYING) => {
                    decision.store(FINISHED, Ordering::Release);
                    InstallDecision::Ready
                }
                Err(CANCELLED) => InstallDecision::Refused(
                    hex_engine::PlaybackHandoffRejection::FreshRuntimeCancelled,
                ),
                _ => InstallDecision::Refused(
                    hex_engine::PlaybackHandoffRejection::ConstructorClaimsUnavailable,
                ),
            };
        };
        let old = &mut previous.state;
        if !old
            .claims
            .as_ref()
            .is_some_and(|c| c.matches_state(old, root))
            || old.manifest.is_some()
            || old.covering_successor.is_some()
            || old.retired_runtime.is_some()
        {
            return InstallDecision::Refused(
                hex_engine::PlaybackHandoffRejection::ConstructorClaimsUnavailable,
            );
        }
        let Some(successor) = next.spare_successor.as_mut() else {
            return InstallDecision::Refused(
                hex_engine::PlaybackHandoffRejection::ConstructorClaimsUnavailable,
            );
        };
        if successor.stamp != next.stamp
            || successor.successor_dependency
            || successor
                .life
                .successor_dependencies
                .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return InstallDecision::Refused(
                hex_engine::PlaybackHandoffRejection::ConstructorClaimsUnavailable,
            );
        }
        successor.successor_dependency = true;
        if !root.eligible_claims_open() {
            successor.successor_dependency = false;
            successor
                .life
                .successor_dependencies
                .store(0, Ordering::Release);
            return InstallDecision::Refused(
                hex_engine::PlaybackHandoffRejection::ConstructorClaimsUnavailable,
            );
        }
        if !matches!(
            decision.compare_exchange(PENDING, APPLYING, Ordering::AcqRel, Ordering::Acquire),
            Ok(_) | Err(APPLYING)
        ) {
            successor.successor_dependency = false;
            successor
                .life
                .successor_dependencies
                .store(0, Ordering::Release);
            return InstallDecision::Refused(
                hex_engine::PlaybackHandoffRejection::FreshRuntimeCancelled,
            );
        }
        let result = next.runtime.as_mut().map(|runtime| {
            runtime.try_inherit_and_retire_in_place(
                &mut old.runtime,
                &mut next.retired_runtime,
                &mut next.manifest,
            )
        });
        // The engine preflights every fallible branch before actual ownership
        // swaps. Its successful origin commit leaves only fixed Option moves.
        if !matches!(result, Some(Ok(()))) {
            successor.successor_dependency = false;
            successor
                .life
                .successor_dependencies
                .store(0, Ordering::Release);
            return if result.is_some_and(|r| r.is_err_and(|e| e.is_transient())) {
                // No owner/history/origin move occurred. Restore cancellable
                // deferred custody; a racing cancellation can now win Pending.
                let _ = decision.compare_exchange(
                    APPLYING,
                    PENDING,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
                InstallDecision::Retry
            } else {
                InstallDecision::Refused(
                    hex_engine::PlaybackHandoffRejection::ConstructorClaimsUnavailable,
                )
            };
        }
        old.runtime = next.retired_runtime.take();
        old.manifest = next.manifest.take();
        old.covering_successor = next.spare_successor.take();
        decision.store(FINISHED, Ordering::Release);
        InstallDecision::Ready
    }
    /// Only called after prepare_install succeeds while both owners stay in
    /// persistent callback slots; moving this Box preserves the actual runtime.
    pub(crate) fn into_active(self) -> Active {
        Active { state: self.state }
    }
}
impl Active {
    pub(crate) fn revision(&self) -> u64 {
        self.state.revision
    }
    pub(crate) fn context_generation(&self) -> u64 {
        self.state.context_generation
    }
    pub(crate) fn runtime(&self) -> Option<&ClaimedAudioCandidateRuntime> {
        self.state.runtime.as_ref()
    }
    pub(crate) fn runtime_mut(&mut self) -> Option<&mut ClaimedAudioCandidateRuntime> {
        self.state.runtime.as_mut()
    }
    pub(crate) fn return_replaced(self) -> Returned {
        Returned {
            stamp: self.state.stamp,
            state: Some(self.state),
            kind: AudioCandidateReturnKind::Replaced,
        }
    }
    pub(crate) fn return_final(
        self,
        authority: &crate::native_callback_owner::FinalReleaseAuthority,
    ) -> Result<FinalReturn, Self> {
        if !authority.matches(self.state.stamp.root) {
            return Err(self);
        }
        Ok(FinalReturn(Returned {
            stamp: self.state.stamp,
            state: Some(self.state),
            kind: AudioCandidateReturnKind::FinalActive,
        }))
    }
}
impl FinalReturn {
    pub(crate) fn from_returned(returned: Returned) -> Self {
        Self(returned)
    }
    pub(crate) fn into_returned(self) -> Returned {
        self.0
    }
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(crate) fn identity(&self) -> AudioCandidateIdentity {
        self.0.stamp.owner
    }
}
