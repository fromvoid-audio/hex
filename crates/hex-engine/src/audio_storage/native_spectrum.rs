//! Constructor-bound native analyzer capture. Its frame and primitive buffers
//! remain DATA; the std worker bookkeeping remains an opaque RESOURCE.
use super::*;
use crate::{
    SharedMetadata,
    sampler_host_fields::{BodyRef, NativeSpectrumCoreFields, NativeSpectrumProjectCoreFields},
};
use std::sync::atomic::{AtomicU8, Ordering};
const LIVE: u8 = 0;
const DROP_PENDING: u8 = 1;
const BUFFERS_DISPOSED: u8 = 2;
const DISPOSED: u8 = 3;
const FAILED: u8 = 4;

/// Actual private Audio wrapper AND its actual contained Core, before escape.
/// Footprint alone never certifies other worker/capture/result descendants.
pub struct NativeSpectrumCaptureFields<'a> {
    pub body: BodyRef<'a>,
    pub core: NativeSpectrumCoreFields<'a>,
    pub project: NativeSpectrumProjectCoreFields<'a>,
}
impl NativeSpectrumCaptureFields<'_> {
    fn dimensions(&self) -> Result<(usize, [usize; 15]), AudioStorageError> {
        if !self.body.contains(self.core.body) {
            return Err(AudioStorageError::SourceMismatch);
        }
        self.core.data_bytes()?;
        self.project.validate_under(self.body, &self.core)?;
        let core = self.core.buffer_bytes();
        let project = self.project.buffer_bytes();
        Ok((
            self.body.layout_bytes(),
            std::array::from_fn(|i| if i < 8 { core[i] } else { project[i - 8] }),
        ))
    }
}
struct CapturedStorage {
    frame: usize,
    buffers: [usize; 15],
    lineages: [u64; 2],
    phase: AtomicU8,
}
/// Existing NativeBank retains this metadata allocation through actual cleanup.
pub struct NativeSpectrumCapturedRegistration {
    state: SharedMetadata<CapturedStorage>,
}
/// Affine field in the actual captured payload. Disappearance is not disposal.
/// ```compile_fail
/// fn duplicate(value: hex_engine::sampler_host_fields::NativeSpectrumCaptureLease) {
///     let _other = value.clone();
/// }
/// ```
pub struct NativeSpectrumCaptureLease {
    state: SharedMetadata<CapturedStorage>,
}
/// Affine counterpart in the SAME constructor's actual Reaper. No Clone, raw
/// owner access or Copy identity can manufacture this association.
/// ```compile_fail
/// fn duplicate(value: hex_engine::sampler_host_fields::NativeSpectrumJoinLease) {
///     let _other = value.clone();
/// }
/// ```
pub struct NativeSpectrumJoinLease {
    state: SharedMetadata<CapturedStorage>,
}
impl NativeSpectrumCapturedRegistration {
    pub fn metadata_requested_bytes_on_control() -> Result<usize, AudioStorageError> {
        let (body, header) = crate::owned_shared::OwnedShared::<CapturedStorage>::declared_parts()
            .ok_or(AudioStorageError::UnknownStorage)?;
        body.checked_add(header).ok_or(AudioStorageError::Overflow)
    }
    pub fn construction_scratch_bytes_on_control() -> Result<usize, AudioStorageError> {
        size_of::<CapturedStorage>()
            .checked_add(size_of::<(usize, [usize; 15])>())
            .and_then(|n| n.checked_add(size_of::<[u64; 2]>()))
            .and_then(|n| {
                n.checked_add(size_of::<(
                    Self,
                    NativeSpectrumCaptureLease,
                    NativeSpectrumJoinLease,
                )>())
            })
            .ok_or(AudioStorageError::Overflow)
    }
    /// Sole audited producer: actual Audio captured wrapper, after combined
    /// same-reservation preflight and before spawn/endpoint escape.
    pub fn pair_actual_capture_on_control(
        fields: NativeSpectrumCaptureFields<'_>,
    ) -> Result<(Self, NativeSpectrumCaptureLease, NativeSpectrumJoinLease), AudioStorageError>
    {
        Self::metadata_requested_bytes_on_control()?;
        let (frame, buffers) = fields.dimensions()?;
        let lineages = [
            crate::audio_only_inputs::handle()?,
            crate::audio_only_inputs::handle()?,
        ];
        let state = SharedMetadata::new_on_control(CapturedStorage {
            frame,
            buffers,
            lineages,
            phase: AtomicU8::new(LIVE),
        });
        let capture = NativeSpectrumCaptureLease {
            state: state.clone(),
        };
        let join = NativeSpectrumJoinLease {
            state: state.clone(),
        };
        Ok((Self { state }, capture, join))
    }
    fn finalize(&self, joined: &mut NativeSpectrumJoinLease) -> Result<(), AudioStorageError> {
        if !self.state.shares_owner_with(&joined.state) {
            return Err(AudioStorageError::SourceMismatch);
        }
        if self.is_normally_disposed_on_control() {
            return Ok(());
        }
        let arc = self.state.paired_owner_on_control().arc();
        if self.state.phase.load(Ordering::Acquire) != BUFFERS_DISPOSED
            || Arc::strong_count(arc) != 2
            || Arc::weak_count(arc) != 0
        {
            self.state.phase.store(FAILED, Ordering::Release);
            return Err(AudioStorageError::UnknownStorage);
        }
        self.state.phase.store(DISPOSED, Ordering::Release);
        Ok(())
    }
    /// Only Audio's actual owning-parent normal-join adapter may call this.
    /// The affine counterpart proves pairing; actual join remains an audited
    /// producer obligation, never a boolean/counter/metadata observation.
    pub fn finalize_actual_join_on_control(
        &self,
        joined: &mut NativeSpectrumJoinLease,
    ) -> Result<(), AudioStorageError> {
        self.finalize(joined)
    }
    /// Distinct constructor abort path: actual spawn returned Err AFTER normal
    /// destruction of its closure. An unwind/Preparing phase cannot call this.
    pub fn finalize_failed_spawn_on_control(
        &self,
        aborted: &mut NativeSpectrumJoinLease,
    ) -> Result<(), AudioStorageError> {
        self.finalize(aborted)
    }
    pub fn is_normally_disposed_on_control(&self) -> bool {
        self.state.phase.load(Ordering::Acquire) == DISPOSED
    }
    pub(crate) fn visit_fields<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        let phase = self.state.phase.load(Ordering::Acquire);
        if !matches!(phase, LIVE | BUFFERS_DISPOSED | DISPOSED) {
            return Err(AudioStorageError::UnknownStorage);
        }
        let paired = self.state.paired_owner_on_control();
        if !ledger.skip_observed_shared_field(&self.state) {
            ledger.observe_shared(paired.arc(), paired.parts(), Owner::exclusive(&self.state))?;
        }
        if phase == DISPOSED {
            return Ok(());
        }
        let mut owner = Owner::exclusive(self.state.as_ref());
        owner.domain = OwnerDomain::RetainedNative;
        ledger.bytes(
            owner,
            Location("native-spectrum-captured-frame", 0),
            self.state.frame,
        )?;
        if phase == LIVE {
            for (index, bytes) in self.state.buffers.iter().enumerate() {
                ledger.bytes(
                    owner,
                    Location("native-spectrum-captured-buffer", index),
                    *bytes,
                )?;
            }
        }
        let shared = Owner::shared(paired.arc());
        if let Some(arithmetic) = &mut ledger.arithmetic {
            arithmetic.existing(shared)?;
        }
        for index in if phase == LIVE { 0..2 } else { 1..2 } {
            ledger.handle_at(
                shared,
                Owner::exclusive(&self.state.lineages[index]),
                Location("native-spectrum-capture-lease", index),
                self.state.lineages[index],
            )?;
        }
        Ok(())
    }
    pub fn visit_storage_on_control<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        ledger.mutate(|ledger| self.visit_fields(ledger))
    }
}
impl NativeSpectrumCaptureLease {
    /// Actual owning payload could not prove its supported retained shape.
    /// Failure preserves charge and never retries a destructive take.
    pub fn record_actual_capture_failure_on_control(&self) {
        self.state.phase.store(FAILED, Ordering::Release);
    }
    pub fn begin_actual_buffers_drop_on_control(
        &self,
        fields: NativeSpectrumCaptureFields<'_>,
    ) -> Result<(), AudioStorageError> {
        let (frame, buffers) = fields.dimensions()?;
        if self.state.phase.load(Ordering::Acquire) != LIVE
            || self.state.frame != frame
            || self.state.buffers != buffers
        {
            self.state.phase.store(FAILED, Ordering::Release);
            return Err(AudioStorageError::SourceMismatch);
        }
        self.state.phase.store(DROP_PENDING, Ordering::Release);
        Ok(())
    }
    pub fn complete_actual_buffers_drop_on_control(&mut self) -> Result<(), AudioStorageError> {
        if self.state.phase.load(Ordering::Acquire) != DROP_PENDING {
            self.state.phase.store(FAILED, Ordering::Release);
            return Err(AudioStorageError::UnknownStorage);
        }
        self.state.phase.store(BUFFERS_DISPOSED, Ordering::Release);
        Ok(())
    }
}
impl Drop for NativeSpectrumCaptureLease {
    fn drop(&mut self) {
        if self.state.phase.load(Ordering::Acquire) != BUFFERS_DISPOSED {
            self.state.phase.store(FAILED, Ordering::Release);
        }
    }
}
impl NativeSpectrumJoinLease {
    pub fn record_actual_worker_failure_on_control(&mut self) {
        self.state.phase.store(FAILED, Ordering::Release);
    }
}
impl Drop for NativeSpectrumJoinLease {
    fn drop(&mut self) {
        if self.state.phase.load(Ordering::Acquire) != DISPOSED {
            self.state.phase.store(FAILED, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests;

mod packet;
pub use packet::{
    NativeSpectrumPacketFields, NativeSpectrumPacketLease, NativeSpectrumPacketRegistration,
};

mod result;
pub use result::{
    NativeSpectrumResultFields, NativeSpectrumResultHome, NativeSpectrumResultLease,
    NativeSpectrumResultRegistration,
};
