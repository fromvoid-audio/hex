//! Original worker completion and isolated facts meet under existing Root
//! source custody. Fixed capability publication grants no endpoint or graph.
use super::*;
use crate::{
    plugin_effects::CapturedCanonicalBases,
    plugin_worker::{
        NativeWorkerAuthoredParameterProgress as Progress, NativeWorkerControllerQuery,
    },
};

impl Custody {
    pub(in crate::candidate_custody) fn begin_parameter_extension(
        &self,
        binding: Binding,
        class: hex_plugin_state::ClassId,
        id: u32,
        steps: u32,
        query: hex_plugin_host::protocol::ParameterPageQuery,
    ) -> Result<NativeWorkerControllerQuery, &'static str> {
        self.admit_live_operation(
            binding,
            |owner| match owner {
                Owner::Effect(owner) => owner.begin_parameter_extension(class, id, steps, query),
                Owner::Loaned(owner) => owner.begin_parameter_extension(class, id, steps, query),
                _ => Err("native extension owner differs"),
            },
            || {},
            "native extension was not admitted",
        )
    }
    pub(in crate::candidate_custody) fn complete_parameter_extension(
        &self,
        binding: Binding,
        token: &NativeWorkerControllerQuery,
        facts: Option<&CapturedCanonicalBases>,
    ) -> Result<Progress, &'static str> {
        self.with_live_capture(binding, |owner| match owner {
            Owner::Effect(owner) => owner.complete_parameter_extension(token, facts),
            Owner::Loaned(owner) => owner.complete_parameter_extension(token, facts),
            _ => Err("native extension owner differs"),
        })
    }
}
impl CandidateRoot {
    pub(crate) fn begin_native_parameter_extension(
        &self,
        binding: Binding,
        class: hex_plugin_state::ClassId,
        id: u32,
        steps: u32,
        query: hex_plugin_host::protocol::ParameterPageQuery,
    ) -> Result<NativeWorkerControllerQuery, &'static str> {
        self.require_native_source_control_open()?;
        self.0
            .eligible
            .begin_parameter_extension(binding, class, id, steps, query)
    }
    pub(crate) fn complete_native_parameter_extension(
        &self,
        binding: Binding,
        token: &NativeWorkerControllerQuery,
        facts: Option<&CapturedCanonicalBases>,
    ) -> Result<Progress, &'static str> {
        self.require_native_source_control_open()?;
        let result = self
            .0
            .eligible
            .complete_parameter_extension(binding, token, facts)?;
        self.require_native_source_control_open()?;
        Ok(result)
    }
}
