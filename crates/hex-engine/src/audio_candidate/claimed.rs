//! Physical runtime/anchor split. It grants no native publication or cost
//! completeness by itself. Hosts must retain exact covering anchors and their
//! own actual catalog/resource custody until every report is consumed.
use super::*;
mod playback;
pub use playback::AudioCandidateRuntimeStatus;

pub struct AudioCandidateSplitRefusal {
    pub reason: AudioStorageError,
    pub owner: AudioCandidateOwner,
}
/// The original preparation wrapper/bank/token stays intact in control custody.
pub struct AudioCandidateAnchors {
    pub(super) owner: AudioCandidateOwner,
}
struct ClaimedState {
    identity: AudioCandidateIdentity,
    source_context: Option<crate::AudioSourceContext>,
    runtime: Option<Box<RuntimeOwner>>,
    groups: Vec<Group>,
    journal: Vec<groups::TransferRow>,
    phase: ClaimedPhase,
    first_panic: Option<Box<dyn Any + Send>>,
    owned_runtime_bytes: Option<usize>,
}
impl ClaimedState {
    fn wrapper_storage_bytes(&self) -> usize {
        let Self {
            identity: _,
            source_context: _,
            runtime: _,
            groups: _,
            journal: _,
            phase: _,
            first_panic: _,
            owned_runtime_bytes: _,
        } = self;
        // Existing Runtime/Vec allocations move unchanged from the constructor;
        // only this wrapper is a new allocation at the split frontier.
        std::mem::size_of::<Self>()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClaimedPhase {
    Current,
    Retired,
    Pending,
    Disposed,
    Failed,
}
/// No raw Playback/Arc, mutable-root, Clone or legacy reconstruction escape.
/// Its exact nested Runtime Box and group/journal backings come from the same
/// original constructor; a split never reruns the compiler or adopts a runtime.
pub struct ClaimedAudioCandidateRuntime {
    inner: Box<ClaimedState>,
}
/// A borrow of both actual halves, never a detached scalar capability. Native
/// hosts must retain these exact anchors before transferring their runtime.
pub struct AudioCandidateConstructorClaims<'a> {
    anchors: &'a AudioCandidateAnchors,
    runtime: &'a ClaimedAudioCandidateRuntime,
}
impl AudioCandidateConstructorClaims<'_> {
    pub fn identity(&self) -> AudioCandidateIdentity {
        self.runtime.identity()
    }
    pub fn summary(&self) -> Option<AudioCandidateConstructorSummary> {
        self.anchors.owner.inner.constructor_data
    }
}
pub(super) fn reserved_state_bytes() -> usize {
    std::mem::size_of::<ClaimedState>()
}
impl AudioCandidateOwner {
    /// Configure existing metering flags while the actual constructor is still
    /// rooted and unpublished. No runtime/owner escapes and no storage is
    /// constructed here. Native hosts perform this BEFORE final inspection;
    /// this is not a publication or resource-admission proof.
    pub fn prepare_device_metering_on_control(
        &mut self,
        enabled: bool,
    ) -> Result<(), AudioStorageError> {
        if self.inner.phase != AudioCandidateDisposalPhase::Owned
            || self.inner.runtime_receipt.is_some()
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        let runtime = self
            .inner
            .runtime
            .as_mut()
            .ok_or(AudioStorageError::SourceMismatch)?;
        if !runtime.playback.candidate_constructor_is_fresh() {
            return Err(AudioStorageError::SourceMismatch);
        }
        runtime.playback.set_device_metering(enabled);
        Ok(())
    }
    /// All reported refusals precede owner movement. Box allocation follows the
    /// existing control-side fatal-OOM convention. The host must preflight and
    /// charge this wrapper plus all simultaneous anchor/runtime storage first.
    pub fn split_constructor_on_control(
        self,
    ) -> Result<
        (ClaimedAudioCandidateRuntime, AudioCandidateAnchors),
        Box<AudioCandidateSplitRefusal>,
    > {
        match self.prepare_constructor_split() {
            Ok(state) => Ok(self.commit_constructor_split(state)),
            Err(reason) => Err(Box::new(AudioCandidateSplitRefusal {
                reason,
                owner: self,
            })),
        }
    }
    fn prepare_constructor_split(&self) -> Result<Box<ClaimedState>, AudioStorageError> {
        if self.inner.phase != AudioCandidateDisposalPhase::Owned
            || self.inner.runtime_receipt.is_some()
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        self.inner.validate_groups()?;
        self.inner.validate_fresh_origins()?;
        let state = Box::new(ClaimedState {
            identity: self.identity(),
            source_context: self.source_context(),
            runtime: None,
            groups: Vec::new(),
            journal: Vec::new(),
            phase: ClaimedPhase::Current,
            first_panic: None,
            owned_runtime_bytes: self.inner.constructor_data.map(|s| s.owned_runtime_bytes),
        });
        let _actual_wrapper_bytes = state.wrapper_storage_bytes();
        Ok(state)
    }
    fn commit_constructor_split(
        mut self,
        mut state: Box<ClaimedState>,
    ) -> (ClaimedAudioCandidateRuntime, AudioCandidateAnchors) {
        // Only fixed moves after every check/allocation. The actual Runtime Box
        // and the original outer preparation wrapper both keep their addresses.
        state.owned_runtime_bytes = self.inner.constructor_data.map(|s| s.owned_runtime_bytes);
        state.runtime = self.inner.runtime.take();
        state.groups = std::mem::take(&mut self.inner.groups);
        state.journal = std::mem::take(&mut self.inner.journal);
        self.inner.phase = AudioCandidateDisposalPhase::Leased;
        (
            ClaimedAudioCandidateRuntime { inner: state },
            AudioCandidateAnchors { owner: self },
        )
    }
}
/// Lifetime-bound preallocated split storage for the ACTUAL construction cell.
/// No publication permission, owner extraction or static/Clone escape. Dropping
/// this preparation leaves the original constructor in its original cell; only
/// the empty, scalar/empty-Vec wrapper is reclaimed on control.
/// The host must reserve its actual overlap BEFORE calling and retain all real
/// external guards/custody through final inspection and reserved queue commit.
/// ```compile_fail
/// let _ = hex_engine::AudioCandidatePreparedSplit {};
/// ```
/// ```compile_fail
/// fn escape<'a>(p: hex_engine::AudioCandidatePreparedSplit<'a>)
///     -> hex_engine::AudioCandidatePreparedSplit<'static> { p }
/// ```
/// ```compile_fail
/// fn mutate(cell: &mut hex_engine::AudioCandidatePreparation) {
///     let split = cell.preallocate_publication_split_on_control().unwrap();
///     let _ = cell.cleanup_on_control();
///     std::hint::black_box(split);
/// }
/// ```
pub struct AudioCandidatePreparedSplit<'a> {
    preparation: &'a mut AudioCandidatePreparation,
    state: Box<ClaimedState>,
}
impl AudioCandidatePreparedSplit<'_> {
    /// Actual requested allocated wrapper layout, not generic provenance,
    /// unseen payload accounting, RSS or a whole-graph peak declaration.
    pub fn wrapper_layout_on_control(&self) -> std::alloc::Layout {
        std::alloc::Layout::for_value(self.state.as_ref())
    }
    /// Immutable constructor facts. The mutable cell remains exclusively
    /// borrowed by this preparation; no owner or PCM is moved/cloned out.
    pub fn owner_on_control(&self) -> Option<&AudioCandidateOwner> {
        self.preparation.owner_ref_on_control()
    }
    /// The existing constructor seal is still ONLY its own storage proof.
    /// A native host must join actual external roots under current custody;
    /// this summary is not a native/publication permit or joint ledger.
    pub fn seal_constructor_on_control(
        &mut self,
        reservation: crate::AudioStorageLedgerReservation,
        external_runtime_bytes: usize,
    ) -> Result<AudioCandidateConstructorSummary, AudioStorageError> {
        self.preparation
            .owner_on_control()
            .ok_or(AudioStorageError::SourceMismatch)?
            .seal_constructor_with_runtime_storage_on_control(reservation, external_runtime_bytes)
    }
    /// All refusal precedes the irreversible ownership moves. The same native
    /// host exclusion MUST continue from final inspection through this call and
    /// queue visibility; no guard is supplied or manufactured by this wrapper.
    /// The successful transfer itself allocates/clones no owner and invokes no
    /// user code; the actual runtime Box/groups/journal move one-to-one.
    pub fn transfer_on_control(
        self,
    ) -> Result<(ClaimedAudioCandidateRuntime, AudioCandidateAnchors), AudioStorageError> {
        let owner = self
            .preparation
            .owner_ref_on_control()
            .ok_or(AudioStorageError::SourceMismatch)?;
        // Before the frontier: keep the original rooted on every refusal.
        // No mutable owner access is exposed while this split borrow exists.
        if owner.identity() != self.state.identity
            || owner.source_context() != self.state.source_context
            || owner.inner.phase != AudioCandidateDisposalPhase::Owned
            || owner.inner.runtime_receipt.is_some()
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        let Self { preparation, state } = self;
        let owner = preparation
            .take_preflighted_owner()
            .ok_or(AudioStorageError::SourceMismatch)?;
        // Irreversible frontier: only closed, fixed ownership/scalar moves.
        Ok(owner.commit_constructor_split(state))
    }
}
impl AudioCandidatePreparation {
    /// Allocate/validate split metadata BEFORE the final native inspection
    /// scope. All source/runtime owners stay in this exact construction cell.
    /// Its exclusive borrow prevents intervening mutation, without supplying
    /// external family/native guards or candidate/source cancellation fences.
    pub fn preallocate_publication_split_on_control(
        &mut self,
    ) -> Result<AudioCandidatePreparedSplit<'_>, AudioStorageError> {
        let state = self
            .owner_ref_on_control()
            .ok_or(AudioStorageError::SourceMismatch)?
            .prepare_constructor_split()?;
        Ok(AudioCandidatePreparedSplit {
            preparation: self,
            state,
        })
    }
    /// Existing convenience path. Native joint publication must preallocate
    /// separately and retain its actual guards through the audited transfer.
    pub fn split_for_publication_on_control(
        &mut self,
    ) -> Result<(ClaimedAudioCandidateRuntime, AudioCandidateAnchors), AudioStorageError> {
        self.preallocate_publication_split_on_control()?
            .transfer_on_control()
    }
}
impl AudioCandidateAnchors {
    pub fn constructor_claims<'a>(
        &'a self,
        runtime: &'a ClaimedAudioCandidateRuntime,
    ) -> Option<AudioCandidateConstructorClaims<'a>> {
        if !self.matches_current(runtime)
            || self.owner.inner.constructor_data.is_none()
            || runtime.inner.phase != ClaimedPhase::Current
            || !runtime
                .inner
                .runtime
                .as_ref()?
                .playback
                .candidate_constructor_is_fresh()
            || runtime
                .inner
                .groups
                .iter()
                .enumerate()
                .any(|(member, group)| {
                    group.closed
                        || group.origin
                            != group.present.then_some(groups::Origin {
                                runtime: self.identity(),
                                member,
                            })
                })
        {
            return None;
        }
        Some(AudioCandidateConstructorClaims {
            anchors: self,
            runtime,
        })
    }
    pub fn identity(&self) -> AudioCandidateIdentity {
        self.owner.identity()
    }
    pub fn preparation(&self) -> &AudioPreparationCustody {
        self.owner.preparation()
    }
    pub fn matches_current(&self, runtime: &ClaimedAudioCandidateRuntime) -> bool {
        if self.identity() != runtime.identity()
            || self.owner.source_context() != runtime.source_context()
            || self.owner.inner.phase != AudioCandidateDisposalPhase::Leased
        {
            return false;
        }
        let Ok(root) = self
            .owner
            .inner
            .custody
            .candidate_root_view(&self.owner.inner.token)
        else {
            return false;
        };
        runtime
            .inner
            .runtime
            .as_ref()
            .and_then(|r| r.playback.prepared_revision())
            .is_some_and(|r| std::sync::Arc::ptr_eq(r, root))
    }
    pub fn original_runtime_disposed(&self) -> bool {
        self.owner.inner.archive.original_disposed
    }
    pub fn all_origins_closed(&self) -> bool {
        self.owner.inner.archive.all_closed()
    }
    /// Normal runtime proof must already have been atomically ingested into
    /// every actual origin anchor. No scalar identity can authorize this step.
    pub fn finish_disposal_on_control(
        &mut self,
    ) -> Result<AudioCandidateDisposed, AudioStorageError> {
        if self.owner.inner.phase == AudioCandidateDisposalPhase::Leased
            && self.owner.inner.archive.all_closed()
        {
            self.owner.inner.phase = AudioCandidateDisposalPhase::RootsCleaning;
        }
        self.owner.finish_disposal_on_control()
    }
    pub fn recover_inputs_on_control(&mut self) -> Option<OwnedAudioInputs> {
        self.owner.recover_inputs_on_control()
    }
    pub fn dispose_original_inputs_on_control(&mut self) -> Result<(), AudioStorageError> {
        self.owner.dispose_original_inputs_on_control()
    }
}
impl ClaimedAudioCandidateRuntime {
    pub fn source_context(&self) -> Option<crate::AudioSourceContext> {
        self.inner.source_context
    }

    /// Non-default fixture pressure on the real transfer preflight allowance.
    /// This changes no owner/location/activity flag and exposes no runtime.
    #[cfg(any(test, feature = "instrument-test-support"))]
    pub fn test_set_owned_runtime_reservation(&mut self, bytes: usize) -> Option<usize> {
        self.inner.owned_runtime_bytes.replace(bytes)
    }
    #[cfg(any(test, feature = "instrument-test-support"))]
    pub fn test_track_boxes(
        &self,
        id: hex_project::TrackId,
    ) -> Option<(Option<usize>, Option<usize>)> {
        self.inner.runtime.as_ref()?.playback.test_track_boxes(id)
    }
    #[cfg(any(test, feature = "instrument-test-support"))]
    pub fn test_tail_boxes(
        &self,
        id: hex_project::TrackId,
    ) -> Option<(Option<usize>, Option<usize>)> {
        self.inner.runtime.as_ref()?.playback.test_tail_boxes(id)
    }
    #[cfg(any(test, feature = "instrument-test-support"))]
    pub fn test_tail_activity(&self, id: hex_project::TrackId) -> Option<(bool, bool)> {
        self.inner.runtime.as_ref()?.playback.test_tail_activity(id)
    }
    pub fn identity(&self) -> AudioCandidateIdentity {
        self.inner.identity
    }
    #[cfg(any(test, feature = "instrument-test-support"))]
    pub fn test_prepared_strong_count(&self) -> Option<usize> {
        self.inner
            .runtime
            .as_ref()?
            .playback
            .prepared_revision()
            .map(std::sync::Arc::strong_count)
    }
    #[cfg(any(test, feature = "instrument-test-support"))]
    pub fn test_runtime_address(&self) -> Option<usize> {
        self.inner
            .runtime
            .as_ref()
            .map(|r| r.as_ref() as *const RuntimeOwner as usize)
    }
    pub fn sample_rate(&self) -> Option<u32> {
        self.inner
            .runtime
            .as_ref()
            .map(|r| r.playback.sample_rate())
    }
    pub fn apply_action(
        &mut self,
        action: crate::PlaybackAction,
    ) -> Result<(), crate::PlaybackActionError> {
        if self.inner.phase != ClaimedPhase::Current {
            return Err(crate::PlaybackActionError::SessionNotPrepared);
        }
        self.inner
            .runtime
            .as_mut()
            .ok_or(crate::PlaybackActionError::SessionNotPrepared)?
            .playback
            .try_apply_action(action)
    }
    pub fn render(&mut self, output: &mut [crate::Frame]) -> Result<(), crate::Invalid> {
        if self.inner.phase != ClaimedPhase::Current {
            return Err(crate::Invalid("candidate runtime is not current"));
        }
        self.inner
            .runtime
            .as_mut()
            .ok_or(crate::Invalid("candidate runtime unavailable"))?
            .playback
            .render(output)
    }
    /// Pure exact next-chunk demand. A proposed Play/Pause flag is speculative;
    /// neither transport nor page readiness is committed by this observation.
    pub fn visit_next_stream_chunk(
        &self,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(crate::StreamPageExtent) -> Result<(), crate::Invalid>,
    ) -> Result<crate::StreamChunkWindow, crate::Invalid> {
        if self.inner.phase != ClaimedPhase::Current {
            return Err(crate::Invalid("candidate runtime is not current"));
        }
        self.inner
            .runtime
            .as_ref()
            .ok_or(crate::Invalid("candidate runtime unavailable"))?
            .playback
            .visit_next_stream_chunk(maximum_frames, playing, visit)
    }
    /// Runs the real handoff's pure preflight against the actual predecessor.
    /// Scratch planning may change; committed owners/history/routes do not.
    pub fn preview_stream_handoff(
        &mut self,
        previous: &mut Self,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(crate::StreamPageExtent) -> Result<(), crate::Invalid>,
    ) -> Result<crate::StreamChunkWindow, crate::PlaybackHandoffError> {
        let invalid =
            || crate::PlaybackHandoffError::Session(crate::SessionPlaybackError::InvalidCommand);
        if self.inner.phase != ClaimedPhase::Current
            || previous.inner.phase != ClaimedPhase::Current
            || self.identity() == previous.identity()
        {
            return Err(invalid());
        }
        let next = self.inner.runtime.as_mut().ok_or_else(invalid)?;
        let old = previous.inner.runtime.as_mut().ok_or_else(invalid)?;
        next.playback
            .preview_stream_handoff(&mut old.playback, maximum_frames, playing, visit)
    }
    /// The current covering graph is actually borrowed through this runtime's
    /// own original prepared-root handle. There is no detached report accessor.
    pub fn contains_streamed_sources(&self) -> bool {
        self.inner
            .runtime
            .as_ref()
            .is_none_or(|r| r.playback.contains_streamed_sources())
    }
    pub fn try_inherit_and_retire_in_place(
        &mut self,
        previous: &mut Option<Self>,
        retired: &mut Option<Self>,
        manifest: &mut Option<AudioCandidateTransfer>,
    ) -> Result<(), crate::PlaybackHandoffError> {
        let invalid =
            || crate::PlaybackHandoffError::Session(crate::SessionPlaybackError::InvalidCommand);
        if self.inner.phase != ClaimedPhase::Current || retired.is_some() || manifest.is_some() {
            return Err(invalid());
        }
        let old = previous.as_mut().ok_or_else(invalid)?;
        if old.inner.phase != ClaimedPhase::Current || old.identity() == self.identity() {
            return Err(invalid());
        }
        let next_runtime = self.inner.runtime.as_mut().ok_or_else(invalid)?;
        let old_runtime = old.inner.runtime.as_mut().ok_or_else(invalid)?;
        let transfer = groups::Transfer::new(
            &mut self.inner.groups,
            &mut old.inner.groups,
            &mut self.inner.journal,
            self.inner
                .owned_runtime_bytes
                .as_mut()
                .ok_or_else(invalid)?,
        );
        next_runtime
            .playback
            .inherit_candidate_groups(&mut old_runtime.playback, transfer)?;
        *manifest = Some(AudioCandidateTransfer {
            predecessor: old.identity(),
            successor: self.identity(),
            rows: std::mem::take(&mut self.inner.journal),
        });
        old.inner.phase = ClaimedPhase::Retired;
        *retired = previous.take();
        Ok(())
    }
    /// A successful Drop closes only the actual remaining origin book. Neither
    /// anchor cleanup, catalog release nor original-bank recovery is implied.
    pub fn dispose_on_control(
        &mut self,
    ) -> Result<AudioCandidateRuntimeDisposed, AudioStorageError> {
        if !matches!(
            self.inner.phase,
            ClaimedPhase::Current | ClaimedPhase::Retired
        ) {
            return Err(AudioStorageError::UnknownStorage);
        }
        let runtime = self
            .inner
            .runtime
            .as_ref()
            .ok_or(AudioStorageError::UnknownStorage)?;
        validate_runtime_groups(runtime, &self.inner.groups)?;
        self.inner.phase = ClaimedPhase::Pending;
        if let Err(payload) = catch_unwind(AssertUnwindSafe(|| drop(self.inner.runtime.take()))) {
            self.inner.first_panic = Some(payload);
            self.inner.phase = ClaimedPhase::Failed;
            return Err(AudioStorageError::UnknownStorage);
        }
        self.inner.phase = ClaimedPhase::Disposed;
        for group in &mut self.inner.groups {
            group.closed = group.present;
        }
        Ok(AudioCandidateRuntimeDisposed {
            runtime: self.inner.identity,
            groups: std::mem::take(&mut self.inner.groups),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn constructor_metering_setup_preserves_pcm_storage_and_actual_device_peaks() {
        use hex_lattice_contract::NativeDeviceMetadata;
        if !crate::audio_storage_constructor_layout_supported() {
            eprintln!("unsupported constructor metering; no positive setup proof");
            return;
        }
        eprintln!("SUPPORTED actual rooted constructor metering and Utility output peaks");
        let mut snapshot = crate::audio_preparation::candidate_test_project().snapshot();
        let device = snapshot.next_id;
        snapshot.tracks[0]
            .devices
            .push(hex_project::DeviceSnapshot {
                id: device,
                bypassed: false,
                state: hex_devices::utility::UTILITY_DEFINITION
                    .default_state()
                    .unwrap(),
            });
        snapshot.next_id += 1;
        let project = hex_project::Project::from_snapshot(snapshot).unwrap();
        let asset = project.assets().next().unwrap().id();
        let mut results = Vec::new();
        for enabled in [false, true] {
            let quote = AudioCandidateReservationQuote::for_project(&project, 1).unwrap();
            let mut cell = AudioCandidatePreparation::try_new_on_control(
                1,
                quote.storage_reservation(1024).unwrap(),
            )
            .unwrap();
            let audio = crate::Audio::new(48_000, vec![[0.25, -0.5]; 8]).unwrap();
            cell.copy_resident_source_on_control(asset, &audio).unwrap();
            cell.prepare_on_control(
                &project,
                11,
                crate::AudioSourceContext {
                    project_generation: 2,
                    editor_revision: 7,
                },
            )
            .unwrap();
            let owner = cell.owner_on_control().unwrap();
            let address = owner.inner.runtime.as_deref().unwrap() as *const RuntimeOwner;
            owner.prepare_device_metering_on_control(enabled).unwrap();
            assert_eq!(
                owner.inner.runtime.as_deref().unwrap() as *const RuntimeOwner,
                address
            );
            let summary = owner
                .seal_constructor_on_control(crate::AudioStorageLedgerReservation {
                    data_rows: 4096,
                    handle_rows: 2048,
                    root_rows: 4,
                    max_retained_bytes: 64 * 1024 * 1024,
                    max_construction_peak_bytes: 64 * 1024 * 1024,
                })
                .unwrap();
            let (mut runtime, mut anchors) = cell.split_for_publication_on_control().unwrap();
            assert_eq!(
                anchors.constructor_claims(&runtime).unwrap().summary(),
                Some(summary)
            );
            assert!(
                anchors
                    .owner
                    .prepare_device_metering_on_control(!enabled)
                    .is_err()
            );
            runtime
                .apply_action(crate::PlaybackAction::Transport(
                    crate::PlaybackCommand::Play,
                ))
                .unwrap();
            let mut pcm = [[0.0; 2]; 8];
            runtime
                .render_with_post_node_observer(&mut pcm, None)
                .unwrap();
            let root = anchors
                .owner
                .inner
                .custody
                .candidate_root_view(&anchors.owner.inner.token)
                .unwrap();
            let peaks = root
                .arrangement()
                .device_meter(device)
                .unwrap()
                .take_peaks();
            if enabled {
                // DeviceMeter reports aggregate input/output peaks, rather
                // than separate left/right channels. Unity Utility preserves
                // the independent 0.5 absolute stereo input maximum.
                assert_eq!(peaks, [0.5, 0.5]);
            } else {
                assert_eq!(peaks, [0.0, 0.0]);
            }
            results.push(pcm);
            let proof = runtime.dispose_on_control().unwrap();
            let mut cells = [Some(anchors)];
            proof.record_anchor_cells_on_control(&mut cells).unwrap();
            let anchors = cells[0].as_mut().unwrap();
            anchors.finish_disposal_on_control().unwrap();
            let _bank = anchors.recover_inputs_on_control().unwrap();
            cell.cleanup_on_control().unwrap();
        }
        assert_eq!(results[0], results[1]);
        assert_eq!(results[1], [[0.25, -0.5]; 8]);
    }
    #[test]
    fn preallocated_split_drop_and_transfer_preserve_actual_cell_and_allocation_ownership() {
        if !crate::audio_storage_constructor_layout_supported() {
            eprintln!("unsupported preallocated split constructor; no positive transfer proof");
            return;
        }
        eprintln!(
            "SUPPORTED actual preallocated split drop/retry and fixed constructor ownership transfer"
        );
        let reservation = crate::AudioStorageReservation {
            allocation_items: 256,
            handle_obligations: 4096,
            max_prepared_data_bytes: 64 * 1024 * 1024,
            max_construction_data_bytes: 64 * 1024 * 1024,
            caller_retained_bytes: 0,
            caller_construction_bytes: 0,
        };
        let mut cell = AudioCandidatePreparation::try_new_on_control(1, reservation).unwrap();
        assert!(cell.preallocate_publication_split_on_control().is_err());
        let project = crate::audio_preparation::candidate_test_project();
        let asset = project.assets().next().unwrap().id();
        // Existing audio-clip constructor fixture only; this is not Sampler
        // original-resource admission or a new production PCM-copy route.
        let audio = crate::Audio::new(48_000, vec![[0.25; 2]; 8]).unwrap();
        cell.copy_resident_source_on_control(asset, &audio).unwrap();
        cell.prepare_on_control(
            &project,
            11,
            crate::AudioSourceContext {
                project_generation: 2,
                editor_revision: 7,
            },
        )
        .unwrap();
        let facts = |owner: &AudioCandidateOwner| {
            (
                owner.identity(),
                owner.inner.as_ref() as *const CandidateState as usize,
                owner.inner.runtime.as_deref().unwrap() as *const RuntimeOwner as usize,
                owner.inner.groups.as_ptr() as usize,
                owner.inner.journal.as_ptr() as usize,
            )
        };
        let before = facts(cell.owner_on_control().unwrap());
        {
            let prepared = cell.preallocate_publication_split_on_control().unwrap();
            assert_eq!(facts(prepared.owner_on_control().unwrap()), before);
            assert!(prepared.state.runtime.is_none());
            assert_eq!(prepared.state.groups.capacity(), 0);
            assert_eq!(prepared.state.journal.capacity(), 0);
            // Drop before final admission leaves the original owner rooted.
        }
        assert_eq!(facts(cell.owner_on_control().unwrap()), before);
        let mut prepared = cell.preallocate_publication_split_on_control().unwrap();
        let allocated_wrapper = prepared.state.as_ref() as *const ClaimedState as usize;
        // Final constructor sealing follows allocation; the transferred wrapper
        // must receive the NEW seal rather than its earlier absent declaration.
        assert!(prepared.state.owned_runtime_bytes.is_none());
        assert!(
            prepared
                .seal_constructor_on_control(
                    crate::AudioStorageLedgerReservation {
                        data_rows: 4096,
                        handle_rows: 2048,
                        root_rows: 4,
                        max_retained_bytes: 1,
                        max_construction_peak_bytes: 1,
                    },
                    0
                )
                .is_err()
        );
        assert_eq!(facts(prepared.owner_on_control().unwrap()), before);
        assert_eq!(
            prepared.state.as_ref() as *const ClaimedState as usize,
            allocated_wrapper
        );
        let summary = prepared
            .seal_constructor_on_control(
                crate::AudioStorageLedgerReservation {
                    data_rows: 4096,
                    handle_rows: 2048,
                    root_rows: 4,
                    max_retained_bytes: 64 * 1024 * 1024,
                    max_construction_peak_bytes: 64 * 1024 * 1024,
                },
                0,
            )
            .unwrap();
        let (mut runtime, mut anchors) = prepared.transfer_on_control().unwrap();
        assert!(cell.owner_on_control().is_none());
        assert_eq!(
            runtime.inner.as_ref() as *const ClaimedState as usize,
            allocated_wrapper
        );
        assert_eq!(anchors.owner.identity(), before.0);
        assert_eq!(
            anchors.owner.inner.as_ref() as *const CandidateState as usize,
            before.1
        );
        assert_eq!(
            runtime.inner.runtime.as_deref().unwrap() as *const RuntimeOwner as usize,
            before.2
        );
        assert_eq!(runtime.inner.groups.as_ptr() as usize, before.3);
        assert_eq!(runtime.inner.journal.as_ptr() as usize, before.4);
        assert_eq!(
            runtime.inner.owned_runtime_bytes,
            Some(summary.owned_runtime_bytes)
        );
        assert!(anchors.matches_current(&runtime));
        assert!(anchors.recover_inputs_on_control().is_none());
        runtime
            .apply_action(crate::PlaybackAction::Transport(
                crate::PlaybackCommand::Play,
            ))
            .unwrap();
        let mut actual = [[0.0; 2]; 8];
        runtime
            .render_with_post_node_observer(&mut actual, None)
            .unwrap();
        assert!(
            actual
                .iter()
                .any(|frame| frame.iter().any(|sample| *sample != 0.0))
        );
        let proof = runtime.dispose_on_control().unwrap();
        let mut anchor_cells = [Some(anchors)];
        proof
            .record_anchor_cells_on_control(&mut anchor_cells)
            .unwrap();
        let anchors = anchor_cells[0].as_mut().unwrap();
        anchors.finish_disposal_on_control().unwrap();
        let _bank = anchors.recover_inputs_on_control().unwrap();
        cell.cleanup_on_control().unwrap();
    }
    #[test]
    fn split_keeps_actual_runtime_box_and_original_custodian_until_normal_bound_disposal() {
        let Some((custody, token, pointer, capacity)) =
            crate::audio_preparation::candidate_test_fixture(false)
        else {
            return;
        };
        let mut owner = AudioCandidateOwner::try_new_on_control(custody, token)
            .ok()
            .unwrap();
        let summary = owner
            .seal_constructor_on_control(crate::AudioStorageLedgerReservation {
                data_rows: 4096,
                handle_rows: 2048,
                root_rows: 4,
                max_retained_bytes: 64 * 1024 * 1024,
                max_construction_peak_bytes: 64 * 1024 * 1024,
            })
            .unwrap();
        let box_address = owner.inner.runtime.as_deref().unwrap() as *const RuntimeOwner as usize;
        let wrapper_address = owner.inner.as_ref() as *const CandidateState as usize;
        let groups_address = owner.inner.groups.as_ptr() as usize;
        let journal_address = owner.inner.journal.as_ptr() as usize;
        let identity = owner.identity();
        let (mut runtime, mut anchor) = owner.split_constructor_on_control().ok().unwrap();
        assert_eq!(
            summary.member_rows,
            runtime.inner.groups.capacity() + anchor.owner.inner.archive.groups.capacity()
        );
        assert_eq!(summary.transfer_rows, runtime.inner.journal.capacity());
        assert_eq!(runtime.identity(), identity);
        assert!(anchor.matches_current(&runtime));
        assert_eq!(
            anchor.constructor_claims(&runtime).unwrap().summary(),
            Some(summary)
        );
        assert_eq!(
            runtime.inner.runtime.as_deref().unwrap() as *const RuntimeOwner as usize,
            box_address
        );
        assert_eq!(
            anchor.owner.inner.as_ref() as *const CandidateState as usize,
            wrapper_address
        );
        assert_eq!(runtime.inner.groups.as_ptr() as usize, groups_address);
        assert_eq!(runtime.inner.journal.as_ptr() as usize, journal_address);
        assert!(anchor.recover_inputs_on_control().is_none());
        assert!(anchor.finish_disposal_on_control().is_err());
        runtime
            .apply_action(crate::PlaybackAction::Transport(
                crate::PlaybackCommand::Play,
            ))
            .unwrap();
        assert!(runtime.status().unwrap().playing);
        assert!(runtime.set_device_metering(true));
        let mut output = [[0.0; 2]; 8];
        runtime
            .render_with_post_node_observer(&mut output, None)
            .unwrap();
        assert!(anchor.constructor_claims(&runtime).is_none());
        let (runtime, proof) = std::thread::spawn(move || {
            let proof = runtime.dispose_on_control().unwrap();
            (runtime, proof)
        })
        .join()
        .unwrap();
        assert_eq!(runtime.inner.phase, ClaimedPhase::Disposed);
        let mut runtime = runtime;
        assert!(runtime.status().is_none());
        assert!(runtime.session_route_status(0).is_none());
        assert!(!runtime.set_device_metering(true));
        assert!(
            runtime
                .render_with_post_node_observer(&mut [[0.0; 2]; 1], None)
                .is_err()
        );
        assert!(runtime.set_gain_recording_override(1, 1.0).is_err());
        assert_eq!(proof.identity(), identity);
        let mut anchors = [Some(anchor)];
        proof.record_anchor_cells_on_control(&mut anchors).unwrap();
        assert!(proof.record_anchor_cells_on_control(&mut anchors).is_err());
        let anchor = anchors[0].as_mut().unwrap();
        anchor.finish_disposal_on_control().unwrap();
        let bank = anchor.recover_inputs_on_control().unwrap();
        assert_eq!(
            (bank.sources.as_ptr() as usize, bank.sources.capacity()),
            (pointer, capacity)
        );
    }
}
