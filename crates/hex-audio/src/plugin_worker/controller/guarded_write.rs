//! Successor replay runs on the original native reader. The control result
//! distinguishes refusal from a completed SDK write whose readback has changed.
use super::*;
use hex_plugin_host::protocol::{FlushDisposition, MAX_EDITOR_FLUSH_PARAMETERS};

#[derive(Clone, Copy)]
pub(super) struct GuardedWrite {
    expected_editor_admission: u64,
    parameters: Parameters,
}

impl GuardedWrite {
    fn new(expected_editor_admission: u64, points: &[(u32, f64)]) -> Result<Self, &'static str> {
        // The SDK zero-frame flush accepts at most 64 distinct parameters.
        if points.is_empty() || points.len() > MAX_EDITOR_FLUSH_PARAMETERS {
            return Err("invalid native guarded editor write extent");
        }
        let mut parameters = Parameters::default();
        for &(id, normalized) in points {
            parameters
                .push(ParameterPoint {
                    frame: 0,
                    id,
                    normalized,
                })
                .map_err(|_| "invalid native guarded editor value")?;
        }
        if parameters.as_slice().len() != points.len() {
            return Err("duplicate native guarded editor target");
        }
        Ok(Self {
            expected_editor_admission,
            parameters,
        })
    }

    pub(super) fn apply(
        self,
        instance: &mut hex_plugin_host::native::Prepared,
    ) -> Result<NativeWorkerGuardedEditorWriteProgress, Error> {
        use NativeWorkerGuardedEditorWriteProgress as Progress;
        let flushed = instance
            .flush_editor_parameters_on_worker(&self.parameters, self.expected_editor_admission)?;
        match flushed.disposition {
            FlushDisposition::EditorConflict => Ok(Progress::Conflict),
            FlushDisposition::Applied => {
                let matches = self.parameters.as_slice().iter().all(|point| {
                    flushed
                        .as_slice()
                        .iter()
                        .any(|value| value.id == point.id && value.normalized == point.normalized)
                });
                Ok(if matches {
                    Progress::Applied
                } else {
                    Progress::ReadbackChanged
                })
            }
            FlushDisposition::Empty => {
                Err(Error::Protocol("native guarded editor write was empty"))
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeWorkerGuardedEditorWriteProgress {
    Pending,
    /// The exact SDK flush and controller readback accepted all values.
    Applied,
    /// The worker refused the write before changing any parameter.
    Conflict,
    /// The SDK processed the write but the controller returned different values.
    ReadbackChanged,
    /// The original source failed or was revoked. Application is unknown.
    Failed,
}

impl Reply {
    pub(super) fn guarded(result: NativeWorkerGuardedEditorWriteProgress) -> Option<Self> {
        Some(Self {
            guarded_write: Some(result),
            ..Self::default()
        })
    }
}

impl NativeWorker {
    /// Serialize one bounded write on the exact source's existing worker.
    /// The token grants no authority to capture or modify canonical state.
    pub fn begin_guarded_editor_write_on_control(
        &mut self,
        expected_editor_admission: u64,
        points: &[(u32, f64)],
    ) -> Result<NativeWorkerControllerQuery, &'static str> {
        self.begin_query(Operation::Guarded(GuardedWrite::new(
            expected_editor_admission,
            points,
        )?))
    }

    /// Only the exact guarded write token consumes its terminal.
    pub fn poll_guarded_editor_write_on_control(
        &mut self,
        request: &NativeWorkerControllerQuery,
    ) -> Result<NativeWorkerGuardedEditorWriteProgress, &'static str> {
        use NativeWorkerGuardedEditorWriteProgress as Progress;
        if request.kind != Kind::GuardedWrite {
            return Err("native controller operation kind differs");
        }
        match self.poll_reply(request)? {
            Poll::Pending => Ok(Progress::Pending),
            Poll::Failed => Ok(Progress::Failed),
            Poll::Ready(reply) => reply
                .guarded_write
                .ok_or("native guarded editor write terminal differs"),
        }
    }
}
