//! Productive control-side Main rendering in the SAME preparation custodian.
//! No runtime owner escapes; a completed block remains until its exact ACK.
use super::*;

/// Nonowning correlation for one actual retained output block, never a receipt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioCandidateControlBlock {
    runtime: AudioCandidateIdentity,
    sequence: u64,
    offset: u64,
    frames: usize,
}
impl AudioCandidateControlBlock {
    pub fn sequence(self) -> u64 {
        self.sequence
    }
    pub fn offset(self) -> u64 {
        self.offset
    }
    pub fn frames(self) -> usize {
        self.frames
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioCandidateControlProgress {
    Block(AudioCandidateControlBlock),
    Complete,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Working,
    Complete,
    Refused,
    StepPending,
    DropPending,
    Proof,
    AnchorsRetiring,
    OriginalInputs,
    OriginalInputsPending,
    Disposed,
    Failed,
}
pub(super) struct ControlWork {
    runtime: ClaimedAudioCandidateRuntime,
    anchors: [Option<AudioCandidateAnchors>; 1],
    proof: Option<AudioCandidateRuntimeDisposed>,
    first_panic: Option<Box<dyn Any + Send>>,
    phase: Phase,
    total: u64,
    offset: u64,
    sequence: u64,
    started: bool,
    output: [crate::Frame; 256],
    pending: Option<AudioCandidateControlBlock>,
}
impl ControlWork {
    fn new(
        runtime: ClaimedAudioCandidateRuntime,
        anchors: AudioCandidateAnchors,
        total: u64,
    ) -> Self {
        Self {
            runtime,
            anchors: [Some(anchors)],
            proof: None,
            first_panic: None,
            phase: Phase::Working,
            total,
            offset: 0,
            sequence: 1,
            started: false,
            output: [[0.0; 2]; 256],
            pending: None,
        }
    }
    pub(super) fn failed_incomplete(&self) -> bool {
        matches!(
            self.phase,
            Phase::StepPending | Phase::DropPending | Phase::OriginalInputsPending | Phase::Failed
        )
    }
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "post-work host inventory consumer in progress")
    )]
    pub(super) fn visit_storage<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        use crate::audio_storage::{Location, Owner};
        if !matches!(
            self.phase,
            Phase::Working | Phase::Complete | Phase::Refused
        ) || self.proof.is_some()
            || self.first_panic.is_some()
        {
            return Err(AudioStorageError::UnknownStorage);
        }
        let anchor = self.anchors[0]
            .as_ref()
            .ok_or(AudioStorageError::UnknownStorage)?;
        if !anchor.matches_current(&self.runtime) {
            return Err(AudioStorageError::SourceMismatch);
        }
        ledger.bytes(
            Owner::exclusive(anchor.owner.inner.as_ref()),
            Location("candidate-state-box", 0),
            std::mem::size_of::<CandidateState>(),
        )?;
        // SAME closed operation has already traversed mandatory custody and
        // external inputs. Re-entering its root visitor would duplicate roots.
        // The runtime below requires those real prior declarations/aliases.

        ledger.vec(&anchor.owner.inner.groups)?;
        ledger.vec(&anchor.owner.inner.journal)?;
        ledger.vec(&anchor.owner.inner.archive.groups)?;
        self.runtime.visit_main_control_storage(ledger)
    }
    fn step(&mut self, maximum: usize) -> Result<AudioCandidateControlProgress, AudioStorageError> {
        if maximum == 0 || maximum > 256 {
            return Err(AudioStorageError::SourceMismatch);
        }
        if let Some(block) = self.pending {
            return Ok(AudioCandidateControlProgress::Block(block));
        }
        if self.phase == Phase::Complete {
            return Ok(AudioCandidateControlProgress::Complete);
        }
        if self.phase != Phase::Working {
            return Err(AudioStorageError::UnknownStorage);
        }
        let remaining = self
            .total
            .checked_sub(self.offset)
            .ok_or(AudioStorageError::SourceMismatch)?;
        if remaining == 0 {
            self.phase = Phase::Complete;
            return Ok(AudioCandidateControlProgress::Complete);
        }
        self.sequence
            .checked_add(1)
            .ok_or(AudioStorageError::Overflow)?;
        let frames = remaining.min(maximum as u64) as usize;
        self.phase = Phase::StepPending;
        let result = catch_unwind(AssertUnwindSafe(|| {
            if !self.started {
                self.runtime
                    .apply_action(crate::PlaybackAction::Transport(
                        crate::PlaybackCommand::Play,
                    ))
                    .map_err(|_| AudioStorageError::SourceMismatch)?;
                self.started = true;
            }
            self.runtime
                .render(&mut self.output[..frames])
                .map_err(|_| AudioStorageError::UnknownStorage)
        }));
        match result {
            Ok(Ok(())) => {
                let block = AudioCandidateControlBlock {
                    runtime: self.runtime.identity(),
                    sequence: self.sequence,
                    offset: self.offset,
                    frames,
                };
                self.pending = Some(block);
                self.phase = Phase::Working;
                Ok(AudioCandidateControlProgress::Block(block))
            }
            Ok(Err(reason)) => {
                self.phase = Phase::Refused;
                Err(reason)
            }
            Err(payload) => {
                self.first_panic = Some(payload);
                self.phase = Phase::Failed;
                Err(AudioStorageError::UnknownStorage)
            }
        }
    }
    fn copy(
        &self,
        expected: AudioCandidateControlBlock,
        output: &mut [crate::Frame; 256],
    ) -> Result<(), AudioStorageError> {
        if self.phase != Phase::Working || self.pending != Some(expected) {
            return Err(AudioStorageError::SourceMismatch);
        }
        output.copy_from_slice(&self.output);
        Ok(())
    }
    fn acknowledge(
        &mut self,
        expected: AudioCandidateControlBlock,
    ) -> Result<(), AudioStorageError> {
        if self.phase != Phase::Working || self.pending != Some(expected) {
            return Err(AudioStorageError::SourceMismatch);
        }
        let offset = self
            .offset
            .checked_add(expected.frames as u64)
            .ok_or(AudioStorageError::Overflow)?;
        let sequence = self
            .sequence
            .checked_add(1)
            .ok_or(AudioStorageError::Overflow)?;
        if offset > self.total {
            return Err(AudioStorageError::SourceMismatch);
        }
        self.offset = offset;
        self.sequence = sequence;
        self.pending = None;
        if offset == self.total {
            self.phase = Phase::Complete;
        }
        Ok(())
    }
    pub(super) fn cleanup_on_control(&mut self) -> Result<(), AudioStorageError> {
        if self.phase == Phase::Disposed {
            return Ok(());
        }
        if self.failed_incomplete() {
            return Err(AudioStorageError::UnknownStorage);
        }
        if matches!(
            self.phase,
            Phase::Working | Phase::Complete | Phase::Refused
        ) {
            self.phase = Phase::DropPending;
            self.proof = match self.runtime.dispose_on_control() {
                Ok(proof) => Some(proof),
                Err(reason) => {
                    self.phase = Phase::Failed;
                    return Err(reason);
                }
            };
            self.phase = Phase::Proof;
        }
        if self.phase == Phase::Proof {
            if let Err(reason) = self
                .proof
                .as_ref()
                .ok_or(AudioStorageError::UnknownStorage)?
                .record_anchor_cells_on_control(&mut self.anchors)
            {
                self.phase = Phase::Failed;
                return Err(reason);
            }
            self.phase = Phase::AnchorsRetiring;
        }
        if self.phase == Phase::AnchorsRetiring {
            self.anchors[0]
                .as_mut()
                .ok_or(AudioStorageError::UnknownStorage)?
                .finish_disposal_on_control()?;
            self.phase = Phase::OriginalInputs;
        }
        if self.phase != Phase::OriginalInputs {
            return Err(AudioStorageError::UnknownStorage);
        }
        self.phase = Phase::OriginalInputsPending;
        if let Err(reason) = self.anchors[0]
            .as_mut()
            .ok_or(AudioStorageError::UnknownStorage)?
            .dispose_original_inputs_on_control()
        {
            self.phase = Phase::Failed;
            return Err(reason);
        }
        self.phase = Phase::Disposed;
        drop(self.anchors[0].take());
        drop(self.proof.take());
        Ok(())
    }
}
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "bounded Main host consumer in progress")
)]
impl AudioCandidatePreparation {
    pub(super) fn begin_main_control_work_on_control(
        &mut self,
        frames: u64,
    ) -> Result<(), AudioStorageError> {
        if self.failed_incomplete()
            || self.cleaning
            || self.complete
            || self.control_work.is_some()
            || self.owner.as_ref().is_none_or(|owner| {
                owner.inner.constructor_data.is_none()
                    || owner
                        .inner
                        .runtime
                        .as_ref()
                        .is_none_or(|r| !r.playback.is_main_control_runtime())
            })
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        // Existing split validates and allocates its actual wrapper while the
        // original owner remains in this persistent cell. Only fixed moves
        // occur between transfer and installing both new destinations here.
        let split = self.preallocate_main_split_on_control()?;
        let (runtime, anchors) = split.transfer_on_control()?;
        self.control_work = Some(ControlWork::new(runtime, anchors, frames));
        Ok(())
    }
    pub(super) fn step_main_control_work_on_control(
        &mut self,
    ) -> Result<AudioCandidateControlProgress, AudioStorageError> {
        self.control_work
            .as_mut()
            .ok_or(AudioStorageError::SourceMismatch)?
            .step(256)
    }
    pub(super) fn step_main_control_work_with_frames_on_control(
        &mut self,
        maximum: usize,
    ) -> Result<AudioCandidateControlProgress, AudioStorageError> {
        self.control_work
            .as_mut()
            .ok_or(AudioStorageError::SourceMismatch)?
            .step(maximum)
    }
    pub(super) fn copy_main_control_block_on_control(
        &self,
        block: AudioCandidateControlBlock,
        output: &mut [crate::Frame; 256],
    ) -> Result<(), AudioStorageError> {
        self.control_work
            .as_ref()
            .ok_or(AudioStorageError::SourceMismatch)?
            .copy(block, output)
    }
    pub(super) fn acknowledge_main_control_block_on_control(
        &mut self,
        block: AudioCandidateControlBlock,
    ) -> Result<(), AudioStorageError> {
        self.control_work
            .as_mut()
            .ok_or(AudioStorageError::SourceMismatch)?
            .acknowledge(block)
    }
}

#[cfg(test)]
impl AudioCandidatePreparation {
    pub(super) fn test_work_native_claim_refuses(&self) -> bool {
        let work = self.control_work.as_ref().unwrap();
        work.anchors[0]
            .as_ref()
            .unwrap()
            .constructor_claims(&work.runtime)
            .is_none()
    }
    pub(super) fn test_work_original_drop(&mut self, call: Box<dyn FnOnce() + Send + Sync>) {
        self.control_work.as_mut().unwrap().anchors[0]
            .as_mut()
            .unwrap()
            .owner
            .inner
            .custody
            .test_original_input_drop(crate::audio_only_inputs::OriginalInputDropProbe(Some(call)));
    }
    pub(super) fn test_work_kept_original_drop_failure(&self) -> bool {
        let work = self.control_work.as_ref().unwrap();
        let anchor = work.anchors[0].as_ref().unwrap();
        work.phase == Phase::Failed
            && anchor.owner.inner.inputs_drop_pending
            && !anchor.owner.inner.inputs_disposed
            && anchor.owner.inner.first_panic.as_ref().is_some_and(|p| {
                p.downcast_ref::<&str>() == Some(&"injected original input destruction")
            })
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "test actual bound target, producer, editor and retained owners together"
    )]
    pub(super) fn test_main_work_inventory<'a>(
        &'a self,
        target: &'a crate::SharedMetadata<hex_project::Project>,
        producer: &'a crate::native_device_resources::OriginalResourceBankPlan<'a>,
        editor: &'a hex_project::Editor,
        original: &'a crate::AudioRootStorageLedger<'a>,
        transforms: &crate::audio_transform::AudioTransformPreparation,
        reader: &'a crate::AudioCompiledRevision,
        reservation: crate::AudioStorageLedgerReservation,
    ) -> Result<crate::AudioRootStorageSummary, AudioStorageError> {
        transforms.with_pcm_inventory_on_control(|inventory| {
            let mut ledger = crate::AudioRootStorageLedger::try_new_on_control(reservation)?;
            ledger.root(self, crate::AudioStorageRole::Current)?;
            let work = self
                .control_work
                .as_ref()
                .ok_or(AudioStorageError::SourceMismatch)?;
            work.anchors[0]
                .as_ref()
                .unwrap()
                .owner
                .inner
                .custody
                .visit_sampler_candidate_inputs(
                    target,
                    Some(producer),
                    editor,
                    None,
                    original,
                    inventory,
                    &mut ledger,
                )?;
            reader.visit_shared_storage_on_control(&mut ledger)?;
            work.visit_storage(&mut ledger)?;
            ledger.validate_closed_shared_handles()?;
            ledger.summary()
        })
    }
}
