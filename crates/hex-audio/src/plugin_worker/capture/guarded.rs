//! A capture sequenced after the original worker accepted every guarded value.
use super::*;
use crate::plugin_worker::NativeWorkerControllerQuery;
use hex_plugin_host::transport::ParameterInputSource;

/// Captured state linked to a completed guarded write on the original worker.
/// A later host parameter write admitted through this worker or transport
/// revocation invalidates its input permit. The capture can also contain
/// SDK-side changes admitted before the actual state snapshot.
pub struct NativeWorkerStateAfterGuardedWrite {
    state: NativeDeviceState,
    source: Weak<Signal>,
    binding: Binding,
    parameter_ordinal: NonZeroU64,
}

impl NativeWorkerStateAfterGuardedWrite {
    pub fn state(&self) -> &NativeDeviceState {
        &self.state
    }
}

#[allow(clippy::large_enum_variant)] // Transfer the original capture without allocating.
pub enum NativeWorkerGuardedCaptureProgress {
    Pending,
    Captured(NativeWorkerStateAfterGuardedWrite),
    Failed,
    Refused(StateCaptureRefusal),
}

impl NativeWorker {
    /// Capture after the exact guarded editor write returned Applied.
    pub fn begin_state_capture_after_guarded_write_on_control(
        &mut self,
        input: &NativeWorkerControllerQuery,
    ) -> Result<NativeWorkerCapture, &'static str> {
        let ordinal = self.guarded_write_ordinal(input)?;
        self.begin_state_capture(Some(CaptureAfter::GuardedWrite(ordinal)))
    }

    /// Only the sequenced capture's exact terminal yields the guarded fact.
    pub fn poll_state_capture_after_guarded_write_on_control(
        &mut self,
        request: &NativeWorkerCapture,
    ) -> Result<NativeWorkerGuardedCaptureProgress, &'static str> {
        use NativeWorkerGuardedCaptureProgress as Progress;
        let Some(CapturedInput::GuardedWrite(parameter_ordinal)) = request.after else {
            return Err("native guarded editor capture differs");
        };
        Ok(match self.poll_state_capture(request)? {
            NativeWorkerCaptureProgress::Pending => Progress::Pending,
            NativeWorkerCaptureProgress::Failed => Progress::Failed,
            NativeWorkerCaptureProgress::Refused(reason) => Progress::Refused(reason),
            NativeWorkerCaptureProgress::Captured(state) => {
                Progress::Captured(NativeWorkerStateAfterGuardedWrite {
                    state,
                    source: Arc::downgrade(&self.reaper.completed),
                    binding: self.binding,
                    parameter_ordinal,
                })
            }
        })
    }

    /// Observe the generation of the captured input on the exact paired
    /// transport. The callback must reserve that generation before using it.
    pub fn observe_guarded_capture_input_on_control(
        &self,
        fact: &NativeWorkerStateAfterGuardedWrite,
    ) -> Option<ParameterInputSource> {
        if fact.binding != self.binding
            || fact.source.as_ptr() != Arc::as_ptr(&self.reaper.completed)
            || self.retirement_requested
            || !self.transport_is_live_on_control()
            || !self.retains_original_source_on_control()
        {
            return None;
        }
        self.reaper
            .control
            .observe_parameter_input_on_control(fact.parameter_ordinal)
    }
}
