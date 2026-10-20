//! Affine continuation for one already-captured Main parent. No operation can
//! construct a runtime, publish playback or change device state after closing.
use super::*;
use hex_engine::{
    AudioCandidateControlBlock as Block, AudioCandidateControlProgress as Progress,
    AudioCandidateIdentity as Identity, AudioStorageError, CapturedMainControl, Frame,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapturedMainFileError {
    Busy,
    Stale,
    Failed,
}

/// The actual Root retains the selected cell. This is deliberately not Clone;
/// a scalar preparation token alone never creates a continuation.
/// ```compile_fail
/// fn duplicate(value: &hex_audio::CapturedMainFile) -> hex_audio::CapturedMainFile { value.clone() }
/// ```
pub struct CapturedMainFile {
    root: Option<CandidateRoot>,
    lineage: Option<hex_engine::sampler_host_fields::NativeCapturedMainRootLease>,
    token: AudioCandidatePreparationToken,
    identity: Identity,
    complete: bool,
}

/// Short, non-Send finite operations under the SAME existing work exclusion.
/// This loan ends before File I/O and exposes no preparation or runtime owner.
/// ```compile_fail
/// fn construct(value: &mut hex_audio::CapturedMainFileLoan<'_>) {
///     value.begin_main_control_work_on_control(100);
/// }
/// ```
/// ```compile_fail
/// fn send<T: Send>(_: T) {}
/// fn escape(value: hex_audio::CapturedMainFileLoan<'_>) { send(value); }
/// ```
pub struct CapturedMainFileLoan<'a> {
    _outer_work: CustodyMutexGuard<'a, crate::candidate_custody::Work>,
    work: CustodyMutexGuard<'a, super::super::super::NativeWork>,
    token: AudioCandidatePreparationToken,
    identity: Identity,
    complete: &'a mut bool,
}

impl CandidateRoot {
    /// The lifecycle owning parent calls this only after actual stream/native
    /// disposal and normal analyzer join/drop. Its SAME retirement actor moves
    /// into existing Root control Work; no extra owner pool or Root role.
    pub(crate) fn retain_captured_main_native_tail(
        &self,
        native: &mut Option<crate::native_callback_owner::NativeRetirement>,
    ) -> Result<bool, ()> {
        let Some(tail) = native.as_ref() else {
            return Ok(false);
        };
        if !tail.captured_data_tail_matches_on_control(self) {
            return Err(());
        }
        macro_rules! acquire {
            ($lock:expr) => {
                match $lock.try_lock() {
                    Ok(guard) => guard,
                    Err(crate::candidate_custody::CustodyTryLockError::Contended) => {
                        return Ok(false)
                    }
                    Err(crate::candidate_custody::CustodyTryLockError::Poisoned) => return Err(()),
                }
            };
        }
        let outer_bank = acquire!(self.0.bank);
        let mut outer_work = acquire!(self.0.execution);
        let returns = acquire!(self.0.returns);
        let work = acquire!(self.0.eligible.work);
        let bank = acquire!(self.0.eligible.bank);
        // Inspection-only Resident exclusion remains SIXTH and last. A live
        // opaque request/error/cancellation is not inferred DATA-only.
        let resident = match self
            .0
            .resident
            .as_ref()
            .map(|r| r.try_read_on_control())
            .transpose()
        {
            Ok(value) => value,
            Err(AudioStorageError::ExternalReadersPending) => return Ok(false),
            Err(_) => return Err(()),
        };
        if let Some(resident) = resident.as_ref() {
            let fields = resident.fields().map_err(|_| ())?;
            if fields.request.is_some()
                || fields.failed_response.is_some()
                || fields.cancellation.is_some()
            {
                return Ok(false);
            }
        }
        if !outer_bank.closing
            || !bank.closing
            || self.0.root_fields.is_none()
            || outer_bank.slots.iter().any(|s| s.phase != Phase::Vacant)
            || outer_work.current.is_some()
            || outer_work.bank_drop.is_some()
            || outer_work.bank_drop_pending
            || outer_work.metadata_drop_pending
            || outer_work.first_panic.is_some()
            || outer_work.captured_native_tail.is_some()
            || outer_work.captured_native_tail_pending
            || returns.as_ref().is_some_and(|r| {
                r.pending || r.first_panic.is_some() || r.held.is_some() || !r.receiver.is_empty()
            })
            || work.domains.iter().any(Option::is_some)
            || bank.domains.iter().any(|s| s.phase != Phase::Vacant)
            || work.publication.anchors.iter().any(Option::is_some)
            || work.publication.executions.iter().any(Option::is_some)
            || work.publication.banks.iter().any(Option::is_some)
            || work.publication.failed_payloads.iter().any(Option::is_some)
            || bank
                .spectrum_capture
                .as_ref()
                .is_some_and(|s| !s.is_normally_disposed_on_control())
        {
            return Ok(false);
        }
        let mut captured = None;
        for (index, slot) in bank.publication.slots.iter().enumerate() {
            if slot.phase == Phase::Vacant {
                continue;
            }
            let Some(execution) = work
                .publication
                .preparations
                .get(index)
                .and_then(Option::as_ref)
            else {
                return Ok(false);
            };
            let Some(identity) = execution.captured_main else {
                return Ok(false);
            };
            if captured.is_some()
                || execution.pending
                || execution.failed
                || execution.refusal.is_some()
                || slot.phase != Phase::Reserved
                || slot.incarnation != execution.token.incarnation
                || !execution.domains.is_empty()
            {
                return Ok(false);
            }
            let data_only = execution
                .cell
                .as_ref()
                .ok_or(())?
                .require_captured_main_data_only_on_control(identity);
            data_only.map_err(|_| ())?;
            captured = Some(identity);
        }
        if captured.is_none() {
            return Ok(false);
        }
        outer_work.captured_native_tail = native.take();
        Ok(true)
    }
    pub(crate) fn begin_captured_main_file(
        &self,
        token: AudioCandidatePreparationToken,
        identity: Identity,
        frames: u64,
    ) -> Result<CapturedMainFile, CapturedMainFileError> {
        let mut work = self
            .0
            .eligible
            .work
            .try_lock()
            .map_err(|_| CapturedMainFileError::Busy)?;
        let bank = self
            .0
            .eligible
            .bank
            .try_lock()
            .map_err(|_| CapturedMainFileError::Busy)?;
        let slot = bank
            .publication
            .slots
            .get(token.slot)
            .ok_or(CapturedMainFileError::Stale)?;
        if bank.root != token.root
            || bank.closing
            || slot.incarnation != token.incarnation
            || slot.phase != Phase::Reserved
            || slot.preparation_retiring
        {
            return Err(CapturedMainFileError::Stale);
        }
        drop(bank);
        let execution = work
            .publication
            .preparations
            .get_mut(token.slot)
            .and_then(Option::as_mut)
            .filter(|p| p.token == token)
            .ok_or(CapturedMainFileError::Stale)?;
        if execution.pending
            || execution.failed
            || execution.refusal.is_some()
            || execution.captured_main.is_some()
            || !execution.domains.is_empty()
        {
            return Err(CapturedMainFileError::Stale);
        }
        let cell = execution
            .cell
            .as_mut()
            .ok_or(CapturedMainFileError::Stale)?;
        if cell.operations_on_control().prepared_identity() != Some(identity) {
            return Err(CapturedMainFileError::Stale);
        }
        let lineage = self
            .0
            .root_fields
            .as_ref()
            .map(|fields| fields.reserve_captured_main_field_on_control())
            .transpose()
            .map_err(|_| CapturedMainFileError::Failed)?;
        // Pair the actual fields immediately. The Root/metadata alias is
        // concrete, nonallocating, and retained in this constructor destination.
        let mut captured = CapturedMainFile {
            root: Some(self.clone()),
            lineage,
            token,
            identity,
            complete: true,
        };
        execution.pending = true;
        let result = catch_unwind(AssertUnwindSafe(|| {
            cell.operations_on_control()
                .begin_main_control_work_on_control(frames)
        }));
        match result {
            Ok(Ok(())) => {
                execution.captured_main = Some(identity);
                execution.pending = false;
            }
            Ok(Err(_)) => {
                execution.failed = cell.failed_incomplete();
                execution.pending = false;
                execution.refusal = Some("captured Main construction refused");
                return Err(CapturedMainFileError::Failed);
            }
            Err(payload) => {
                execution.failed = true;
                work.publication.failed_payloads[token.slot] = Some(payload);
                return Err(CapturedMainFileError::Failed);
            }
        }
        captured.complete = false;
        Ok(captured)
    }
}

impl CapturedMainFile {
    /// Progress DATA-only endpoint/root reclamation on the existing worker,
    /// outside every Main loan and File write. The actual handle stays owned
    /// until true normal retirement; Pending never becomes a caller receipt.
    pub fn poll_retirement_on_control(&self) -> bool {
        let Some(root) = self.root.as_ref() else {
            return false;
        };
        root.advance_on_control();
        self.complete && root.finish_on_control()
    }
    pub fn try_loan_on_control(
        &mut self,
    ) -> Result<CapturedMainFileLoan<'_>, CapturedMainFileError> {
        if self.complete {
            return Err(CapturedMainFileError::Stale);
        }
        let root = self.root.as_ref().ok_or(CapturedMainFileError::Stale)?;
        let outer_work = root
            .0
            .execution
            .try_lock()
            .map_err(|_| CapturedMainFileError::Busy)?;
        if outer_work.first_panic.is_some() || outer_work.captured_native_tail_pending {
            return Err(CapturedMainFileError::Failed);
        }
        let work = root
            .0
            .eligible
            .work
            .try_lock()
            .map_err(|_| CapturedMainFileError::Busy)?;
        let bank = root
            .0
            .eligible
            .bank
            .try_lock()
            .map_err(|_| CapturedMainFileError::Busy)?;
        let slot = bank
            .publication
            .slots
            .get(self.token.slot)
            .ok_or(CapturedMainFileError::Stale)?;
        if bank.root != self.token.root
            || slot.incarnation != self.token.incarnation
            || slot.phase != Phase::Reserved
        {
            return Err(CapturedMainFileError::Stale);
        }
        drop(bank);
        let execution = work
            .publication
            .preparations
            .get(self.token.slot)
            .and_then(Option::as_ref)
            .filter(|p| p.token == self.token && p.captured_main == Some(self.identity))
            .ok_or(CapturedMainFileError::Stale)?;
        if execution.pending || execution.failed || execution.refusal.is_some() {
            return Err(CapturedMainFileError::Failed);
        }
        Ok(CapturedMainFileLoan {
            _outer_work: outer_work,
            work,
            token: self.token,
            identity: self.identity,
            complete: &mut self.complete,
        })
    }
    /// Explicit cancellation is retryable only while the SAME handle remains.
    /// It marks cleanup; actual destruction still belongs to Root/control.
    pub fn cancel_on_control(&mut self) -> Result<(), CapturedMainFileError> {
        if self.complete {
            return Err(CapturedMainFileError::Stale);
        }
        let root = self.root.as_ref().ok_or(CapturedMainFileError::Stale)?;
        let mut work = root
            .0
            .eligible
            .work
            .try_lock()
            .map_err(|_| CapturedMainFileError::Busy)?;
        let mut bank = root
            .0
            .eligible
            .bank
            .try_lock()
            .map_err(|_| CapturedMainFileError::Busy)?;
        let slot = bank
            .publication
            .slots
            .get_mut(self.token.slot)
            .filter(|slot| slot.incarnation == self.token.incarnation)
            .ok_or(CapturedMainFileError::Stale)?;
        let execution = work
            .publication
            .preparations
            .get_mut(self.token.slot)
            .and_then(Option::as_mut)
            .filter(|p| p.token == self.token && p.captured_main == Some(self.identity))
            .ok_or(CapturedMainFileError::Stale)?;
        if execution.pending || execution.failed {
            return Err(CapturedMainFileError::Failed);
        }
        execution.captured_main = None;
        slot.preparation_retiring = true;
        self.complete = true;
        Ok(())
    }
}
impl Drop for CapturedMainFile {
    fn drop(&mut self) {
        if !self.complete {
            // On contention/failure retain the actual cell and its charges.
            // No blocking lock, forced take or false normal completion.
            let _ = self.cancel_on_control();
        }
        if let Some(lineage) = self.lineage.as_mut() {
            if lineage.begin_actual_field_drop_on_control().is_err() {
                return;
            }
            drop(self.root.take());
            let _ = lineage.complete_actual_field_drop_on_control();
        }
    }
}
impl CapturedMainFileLoan<'_> {
    fn perform<R>(
        &mut self,
        action: impl FnOnce(
            &mut hex_engine::AudioCandidatePreparationOperations<'_>,
        ) -> Result<R, AudioStorageError>,
    ) -> Result<R, AudioStorageError> {
        let execution = self
            .work
            .publication
            .preparations
            .get_mut(self.token.slot)
            .and_then(Option::as_mut)
            .filter(|p| p.token == self.token && p.captured_main == Some(self.identity))
            .ok_or(AudioStorageError::SourceMismatch)?;
        if execution.pending || execution.failed || execution.refusal.is_some() {
            return Err(AudioStorageError::UnknownStorage);
        }
        execution.pending = true;
        let result = catch_unwind(AssertUnwindSafe(|| {
            action(
                &mut execution
                    .cell
                    .as_mut()
                    .ok_or(AudioStorageError::SourceMismatch)?
                    .operations_on_control(),
            )
        }));
        match result {
            Ok(result) => {
                execution.failed = execution
                    .cell
                    .as_ref()
                    .is_some_and(|c| c.failed_incomplete());
                execution.pending = false;
                result
            }
            Err(payload) => {
                execution.failed = true;
                self.work.publication.failed_payloads[self.token.slot] = Some(payload);
                Err(AudioStorageError::UnknownStorage)
            }
        }
    }
}
impl CapturedMainControl for CapturedMainFileLoan<'_> {
    fn step_main_control_work_for_identity_on_control(
        &mut self,
        expected: Identity,
        maximum: usize,
    ) -> Result<Progress, AudioStorageError> {
        if expected != self.identity {
            return Err(AudioStorageError::SourceMismatch);
        }
        self.perform(|ops| ops.step_main_control_work_for_identity_on_control(expected, maximum))
    }
    fn copy_main_control_block_on_control(
        &self,
        block: Block,
        output: &mut [Frame; 256],
    ) -> Result<(), AudioStorageError> {
        if block.runtime_identity() != self.identity {
            return Err(AudioStorageError::SourceMismatch);
        }
        let cell = self
            .work
            .publication
            .preparations
            .get(self.token.slot)
            .and_then(Option::as_ref)
            .and_then(|p| p.cell.as_ref())
            .ok_or(AudioStorageError::SourceMismatch)?;
        cell.copy_captured_main_block_on_control(block, output)
    }
    fn acknowledge_main_control_block_on_control(
        &mut self,
        block: Block,
    ) -> Result<(), AudioStorageError> {
        if block.runtime_identity() != self.identity {
            return Err(AudioStorageError::SourceMismatch);
        }
        self.perform(|ops| ops.acknowledge_main_control_block_on_control(block))
    }
    fn finish_main_control_work_on_control(
        &mut self,
        expected: Identity,
    ) -> Result<(), AudioStorageError> {
        if expected != self.identity {
            return Err(AudioStorageError::SourceMismatch);
        }
        self.perform(|ops| ops.finish_main_control_work_on_control(expected))?;
        if let Some(execution) = self.work.publication.preparations[self.token.slot].as_mut() {
            execution.captured_main = None;
        }
        *self.complete = true;
        Ok(())
    }
}
