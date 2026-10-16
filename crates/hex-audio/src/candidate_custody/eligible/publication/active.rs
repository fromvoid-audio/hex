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
    Refused,
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
            || next
                .runtime
                .as_ref()
                .is_none_or(|r| r.contains_streamed_sources())
            || next.policy != crate::prepared::PublicationPolicy::InheritRuntime
        {
            return InstallDecision::Refused;
        }
        let Some(previous) = previous else {
            return InstallDecision::Ready;
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
            return InstallDecision::Refused;
        }
        let Some(successor) = next.spare_successor.as_mut() else {
            return InstallDecision::Refused;
        };
        if successor.stamp != next.stamp
            || successor.successor_dependency
            || successor
                .life
                .successor_dependencies
                .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return InstallDecision::Refused;
        }
        successor.successor_dependency = true;
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
                InstallDecision::Retry
            } else {
                InstallDecision::Refused
            };
        }
        old.runtime = next.retired_runtime.take();
        old.manifest = next.manifest.take();
        old.covering_successor = next.spare_successor.take();
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
