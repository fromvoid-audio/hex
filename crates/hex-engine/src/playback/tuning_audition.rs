use super::*;
use crate::tuning_audition::{Runtime, TuningAuditionInput};

impl Playback {
    /// Control-side monitor attachment. Offline prepared exports never call it.
    pub fn prepare_tuning_audition(
        &mut self,
        project: &hex_project::Project,
        input: Arc<TuningAuditionInput>,
    ) -> Result<(), Invalid> {
        if input.is_cancelled()
            || self.project.shared_contexts().tuning()
                != &project.tuning().table().map_err(|e| Invalid(e.0))?
        {
            return Err(Invalid("tuning audition source is stale"));
        }
        let candidate = Runtime::prepare(project, input)?;
        let bytes = candidate.storage_bytes();
        if bytes > crate::tuning_audition::MAX_TUNING_AUDITION_STORAGE_BYTES {
            return Err(Invalid("tuning audition exceeds its fixed storage budget"));
        }
        self.session_runtime
            .as_ref()
            .ok_or(Invalid("tuning audition requires native playback"))?
            .validate_candidate_runtime_storage(bytes)
            .map_err(|_| Invalid("tuning audition exceeds graph storage budget"))?;
        self.tuning_audition = Some(Box::new(candidate));
        Ok(())
    }
    pub fn cancel_tuning_audition(&mut self) {
        if let Some(runtime) = self.tuning_audition.as_mut() {
            runtime.cancel();
        }
    }
    pub(super) fn mix_tuning_audition(&mut self, output: &mut [Frame]) {
        if let Some(runtime) = self.tuning_audition.as_mut() {
            runtime.mix(output);
        }
    }
}
