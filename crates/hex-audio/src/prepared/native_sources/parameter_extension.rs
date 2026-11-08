//! Bounded original-source capability publication, independent of canonical
//! edits and graph admission. Cancellation may drain without publishing.
use super::*;
use crate::{
    plugin_effects::CapturedCanonicalBases,
    plugin_worker::{
        NativeWorkerAuthoredParameterProgress as Progress, NativeWorkerControllerQuery,
    },
};

impl PublicationControl {
    /// One source-bound SDK query on the existing controller slot. The token
    /// survives a close racing admission; Root retains original source custody.
    pub fn begin_native_source_parameter_extension_on_control(
        &mut self,
        binding: Binding,
        class: hex_plugin_state::ClassId,
        id: u32,
        steps: u32,
        query: hex_plugin_host::protocol::ParameterPageQuery,
    ) -> Result<NativeWorkerControllerQuery, &'static str> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native extension output is closing");
        }
        self.candidate_root
            .begin_native_parameter_extension(binding, class, id, steps, query)
    }

    /// Armed publishes one graph-visible target only after the exact worker
    /// completed and disposed isolated constructor bases match every prior
    /// capability. Neither completion nor this cache edit admits an audio graph.
    pub fn complete_native_source_parameter_extension_on_control(
        &mut self,
        binding: Binding,
        token: &NativeWorkerControllerQuery,
        constructor: &CapturedCanonicalBases,
    ) -> Result<Progress, &'static str> {
        let (source, epoch, playing) = constructor.source();
        let matches = || {
            self.native_custody
                && !self.status.closing.load(Acquire)
                && self.output_identity() == Some(source.output_identity)
                && self.status.native_parameter_source(playing) == Some((source, epoch))
        };
        if binding != constructor.binding() || !matches() {
            return Err("native extension source changed");
        }
        let result = self.candidate_root.complete_native_parameter_extension(
            binding,
            token,
            Some(constructor),
        )?;
        if !matches() {
            return Err("native extension source changed");
        }
        Ok(result)
    }

    /// Consume an abandoned SDK operation without publishing graph-visible
    /// capabilities. Native's bounded validation cache may already contain the
    /// target. After output close, original Root disposal owns its late work.
    pub fn drain_native_source_parameter_extension_on_control(
        &mut self,
        binding: Binding,
        token: &NativeWorkerControllerQuery,
    ) -> Result<Progress, &'static str> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native extension output is closing");
        }
        let result = self
            .candidate_root
            .complete_native_parameter_extension(binding, token, None)?;
        if self.status.closing.load(Acquire) {
            return Err("native extension output is closing");
        }
        Ok(result)
    }
}
