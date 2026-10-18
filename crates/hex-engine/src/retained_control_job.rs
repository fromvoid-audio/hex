//! The one sealed fresh Box constructor used by lifecycle retained-control jobs.
//! Outer layout qualification never declares the payload's children known.
use std::{alloc::Layout, any::Any};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetainedControlJobProgress {
    Pending,
    Disposed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetainedControlJobFault;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetainedControlJobBuildError {
    Reservation,
    MissingSource,
}
trait Job: Send + Sync {
    fn payload(&self) -> &dyn Any;
    fn step(&mut self) -> Result<RetainedControlJobProgress, RetainedControlJobFault>;
}
struct TypedJob<T, E> {
    value: T,
    step: fn(&mut T) -> Result<RetainedControlJobProgress, E>,
}
impl<T: Send + Sync + 'static, E: 'static> Job for TypedJob<T, E> {
    fn payload(&self) -> &dyn Any {
        &self.value
    }
    fn step(&mut self) -> Result<RetainedControlJobProgress, RetainedControlJobFault> {
        (self.step)(&mut self.value).map_err(|_| RetainedControlJobFault)
    }
}
/// A single fresh actual job Box. No Clone/raw-adoption/extraction/replacement
/// API. The owning lifecycle family guard must protect inspection/publication.
///
/// ```compile_fail
/// let raw = Box::new(4_u32);
/// let _: hex_engine::RetainedControlJob = raw.into();
/// ```
/// ```compile_fail
/// fn extract(job: hex_engine::RetainedControlJob) { let _ = job.into_inner(); }
/// ```
pub struct RetainedControlJob {
    job: Box<dyn Job>,
}
impl RetainedControlJob {
    /// Requested body component before source consumption. Input/old job/child
    /// temporaries and the enclosing holder are separate construction charges.
    pub fn body_reservation_quote<T, E>() -> usize {
        std::mem::size_of::<TypedJob<T, E>>()
    }
    pub fn try_from_owned_on_control<T: Send + Sync + 'static, E: 'static>(
        source: &mut Option<T>,
        max_body_bytes: usize,
        step: fn(&mut T) -> Result<RetainedControlJobProgress, E>,
    ) -> Result<Self, RetainedControlJobBuildError> {
        if Self::body_reservation_quote::<T, E>() > max_body_bytes {
            return Err(RetainedControlJobBuildError::Reservation);
        }
        let value = source
            .take()
            .ok_or(RetainedControlJobBuildError::MissingSource)?;
        // The only constructor. Sized Box allocation, then unsize the SAME Box;
        // no caller raw owner, detached receipt or virtual origin assertion.
        let job: Box<TypedJob<T, E>> = Box::new(TypedJob { value, step });
        let job: Box<dyn Job> = job;
        Ok(Self { job })
    }
    /// Actual typed dispatch only. This does not certify T/its owned children.
    /// ```compile_fail
    /// fn escape(job: &hex_engine::RetainedControlJob) {
    ///     let _escaped = job.inspect_payload::<String, _>(|value| value.as_str());
    /// }
    /// ```
    pub fn inspect_payload<T: 'static, R>(
        &self,
        inspect: impl for<'a> FnOnce(&'a T) -> R,
    ) -> Option<R> {
        self.job.payload().downcast_ref::<T>().map(inspect)
    }
    /// Dedicated control cleanup only; no mutable payload or Box escapes.
    /// Caller latches panics/errors and never retries a partial destructor.
    pub fn step_on_control(
        &mut self,
    ) -> Result<RetainedControlJobProgress, RetainedControlJobFault> {
        self.job.step()
    }
    /// Layout of the ACTUAL dynamic boxed value, including padding. Scalar
    /// observation only, not an address/owner/lease or payload-known assertion.
    pub fn actual_body_layout_on_control(&self) -> Layout {
        Layout::for_value(self.job.as_ref())
    }
    pub fn visit_outer_storage_on_control<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), crate::AudioStorageError> {
        self.visit_outer_with_gate(ledger, retained_control_job_box_layout_supported())
    }
    fn visit_outer_with_gate<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
        supported: bool,
    ) -> Result<(), crate::AudioStorageError> {
        ledger.mutate(|ledger| {
            if !supported {
                return Err(crate::AudioStorageError::UnknownStorage);
            }
            let actual = Layout::for_value(self.job.as_ref());
            if actual.size() == 0 {
                return Err(crate::AudioStorageError::UnknownStorage);
            }
            // Holder/fat-pointer fields belong to the actual parent/table once.
            // No invented Box header or allocated vtable is charged.
            ledger.bytes(
                crate::audio_storage::Owner::exclusive(self),
                crate::audio_storage::Location("retained-control-job-box", 0),
                actual.size(),
            )
        })
    }
}
pub fn retained_control_job_box_layout_supported() -> bool {
    let native = match env!("HEX_RETAINED_JOB_BOX_LAYOUT_TARGET") {
        "aarch64-apple-darwin" => cfg!(all(target_arch = "aarch64", target_os = "macos")),
        "x86_64-unknown-linux-gnu" => cfg!(all(
            target_arch = "x86_64",
            target_os = "linux",
            target_env = "gnu"
        )),
        _ => false,
    };
    native
        && env!("HEX_RETAINED_JOB_BOX_LAYOUT_KNOWN") == "1"
        && env!("HEX_RETAINED_JOB_BOX_CONSTRUCTOR_FAMILY")
            == "fresh-retained-control-job-sized-box-new-v1"
        && matches!(
            env!("HEX_RETAINED_JOB_BOX_LAYOUT_TARGET"),
            "aarch64-apple-darwin" | "x86_64-unknown-linux-gnu"
        )
}

#[cfg(test)]
mod tests;
