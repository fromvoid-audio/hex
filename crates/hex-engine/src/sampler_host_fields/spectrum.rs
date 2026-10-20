//! Actual fixed float Box fields in the existing native analyzer constructor.
//! Borrowed DATA only: no worker lifetime, endpoint or RESOURCE certificate.
use super::BodyRef;
use crate::{AudioRootStorageLedger, AudioStorageError, audio_storage::Location};

/// Only the analyzer's two primitive allocation shapes. No arbitrary payload,
/// supplied byte count, raw pointer or recursive child callback is accepted.
/// The real owning Box field and its real allocation stay borrowed together.
/// ```compile_fail
/// fn escape(value: &Box<[f64; 16]>)
///     -> hex_engine::sampler_host_fields::SpectrumFloatBoxRef<'static> {
///     hex_engine::sampler_host_fields::SpectrumFloatBoxRef::fft_values(value)
/// }
/// ```
#[derive(Clone, Copy)]
pub struct SpectrumFloatBoxRef<'a> {
    field: BodyRef<'a>,
    allocation: BodyRef<'a>,
}
impl<'a> SpectrumFloatBoxRef<'a> {
    #[expect(
        clippy::borrowed_box,
        reason = "actual owning field identity is mandatory"
    )]
    pub fn stereo<const N: usize>(field: &'a Box<[[f32; 2]; N]>) -> Self {
        Self {
            field: BodyRef::of(field),
            allocation: BodyRef::of(field.as_ref()),
        }
    }
    #[expect(
        clippy::borrowed_box,
        reason = "actual owning field identity is mandatory"
    )]
    pub fn fft_values<const N: usize>(field: &'a Box<[f64; N]>) -> Self {
        Self {
            field: BodyRef::of(field),
            allocation: BodyRef::of(field.as_ref()),
        }
    }
}
/// Exhaustive Audio producer supplies these eight actual fields in fixed order:
/// four stereo windows, then kernel window/real/imaginary/positive power.
/// Its body is the ACTUAL enclosing Core, including the inline kernel. This
/// certifies neither absent future children nor a running worker's storage.
pub struct NativeSpectrumCoreFields<'a> {
    pub body: BodyRef<'a>,
    pub buffers: [SpectrumFloatBoxRef<'a>; 8],
}
impl NativeSpectrumCoreFields<'_> {
    pub(crate) fn buffer_bytes(&self) -> [usize; 8] {
        self.buffers.map(|field| field.allocation.layout_bytes())
    }
    /// Informational constructor footprint, never an admission/retirement token.
    pub fn data_bytes(&self) -> Result<usize, AudioStorageError> {
        let mut bytes = self.body.layout_bytes();
        for (index, buffer) in self.buffers.iter().enumerate() {
            if !self.body.contains(buffer.field) || buffer.allocation.layout_bytes() == 0 {
                return Err(AudioStorageError::SourceMismatch);
            }
            if self.buffers[..index].iter().any(|earlier| {
                earlier.field.same(buffer.field) || earlier.allocation.same(buffer.allocation)
            }) {
                return Err(AudioStorageError::SourceMismatch);
            }
            bytes = bytes
                .checked_add(buffer.allocation.layout_bytes())
                .ok_or(AudioStorageError::Overflow)?;
        }
        Ok(bytes)
    }
}
pub(crate) fn visit_spectrum_core<'a>(
    fields: &NativeSpectrumCoreFields<'a>,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    fields.data_bytes()?;
    ledger.bytes(
        fields.body.owner(),
        Location("native-spectrum-core-inline", 0),
        fields.body.layout_bytes(),
    )?;
    for buffer in fields.buffers {
        ledger.bytes(
            buffer.allocation.owner(),
            Location("native-spectrum-float-box", 0),
            buffer.allocation.layout_bytes(),
        )?;
    }
    Ok(())
}
impl<'a> AudioRootStorageLedger<'a> {
    /// Actual constructor-side Core only. The worker, stop/ring ownership,
    /// Project buffers, queued/latest result owners and physical retirement
    /// still require their independent mandatory Root bindings.
    pub fn borrow_sampler_spectrum_core_fields_on_control(
        &mut self,
        fields: &NativeSpectrumCoreFields<'a>,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| visit_spectrum_core(fields, ledger))
    }
}

/// The owning Audio producer exhaustively checks the actual inline envelope
/// frame fields. BodyRef here certifies ONLY the real fixed Box footprint;
/// it never certifies absent owning descendants or any pending/result Box.
pub struct NativeSpectrumProjectCoreFields<'a> {
    pub body: BodyRef<'a>,
    pub buffers: [SpectrumFloatBoxRef<'a>; 6],
    pub envelope_field: BodyRef<'a>,
    pub envelopes: BodyRef<'a>,
}
impl NativeSpectrumProjectCoreFields<'_> {
    pub(crate) fn buffer_bytes(&self) -> [usize; 7] {
        std::array::from_fn(|i| {
            if i < 6 {
                self.buffers[i].allocation.layout_bytes()
            } else {
                self.envelopes.layout_bytes()
            }
        })
    }
    pub(crate) fn validate_under(
        &self,
        frame: BodyRef<'_>,
        core: &NativeSpectrumCoreFields<'_>,
    ) -> Result<(), AudioStorageError> {
        if !frame.contains(self.body)
            || !self.body.contains(self.envelope_field)
            || self.envelopes.layout_bytes() == 0
            || self.body.same(core.body)
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        for (i, buffer) in self.buffers.iter().enumerate() {
            if !self.body.contains(buffer.field)
                || buffer.allocation.layout_bytes() == 0
                || buffer.allocation.same(self.envelopes)
                || self.buffers[..i].iter().any(|earlier| {
                    earlier.field.same(buffer.field) || earlier.allocation.same(buffer.allocation)
                })
                || core.buffers.iter().any(|other| {
                    other.field.same(buffer.field) || other.allocation.same(buffer.allocation)
                })
            {
                return Err(AudioStorageError::SourceMismatch);
            }
        }
        if core
            .buffers
            .iter()
            .any(|other| other.allocation.same(self.envelopes))
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        Ok(())
    }
}
