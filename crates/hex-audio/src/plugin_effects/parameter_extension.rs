//! Publish graph-visible targets only from the original worker's one-shot SDK
//! completion and isolated, fully disposed constructor-state restoration.
use super::*;
use crate::plugin_worker::{
    NativeWorkerAuthoredParameterProgress as Progress, NativeWorkerControllerQuery,
};

pub(super) fn extended(
    configuration: &Configuration,
    facts: &CapturedCanonicalBases,
    id: u32,
    steps: u32,
) -> Result<automation::SourceCapabilities, &'static str> {
    if facts.binding() != configuration.binding
        || facts.prepared() != configuration.prepared
        || facts.sample_rate() != configuration.sample_rate
        || !facts.canonical_state().processing_eq(&configuration.state)
    {
        return Err("native extension requires exact isolated constructor bases");
    }
    let class = Hosting::saved_identity_on_control(&configuration.state)
        .map_err(|_| "native extension source identity differs")?
        .class;
    validate_capabilities(
        &configuration.authored,
        &facts.capabilities,
        class,
        id,
        steps,
    )?;
    Ok(facts.capabilities.clone())
}

fn validate_capabilities(
    previous: &automation::SourceCapabilities,
    next: &automation::SourceCapabilities,
    class: hex_plugin_state::ClassId,
    id: u32,
    steps: u32,
) -> Result<(), &'static str> {
    if previous.class() != Some(class)
        || next.class() != Some(class)
        || !next
            .parameters()
            .iter()
            .any(|parameter| parameter.id == id && parameter.steps == steps)
        || next.parameters().iter().any(|parameter| {
            parameter.id != id
                && previous
                    .parameters()
                    .binary_search_by_key(&parameter.id, |old| old.id)
                    .is_err()
        })
        || previous
            .parameters()
            .iter()
            .any(|old| !next.parameters().contains(old))
    {
        return Err("native extension changed existing or unacknowledged capabilities");
    }
    Ok(())
}

pub(super) fn bases(
    capabilities: &automation::SourceCapabilities,
) -> automation::CanonicalParameterBases {
    let mut bases = [None; MAX_FOREIGN_AUTOMATION_TARGETS];
    for (slot, parameter) in bases.iter_mut().zip(capabilities.parameters()) {
        *slot = Some((parameter.id, parameter.normalized_base_bits));
    }
    bases
}

impl NativeEffect {
    pub(crate) fn begin_parameter_extension(
        &mut self,
        class: hex_plugin_state::ClassId,
        id: u32,
        steps: u32,
        query: hex_plugin_host::protocol::ParameterPageQuery,
    ) -> Result<NativeWorkerControllerQuery, &'static str> {
        self.worker
            .begin_authored_parameter_extension_on_control(class, id, steps, query)
    }
    pub(crate) fn complete_parameter_extension(
        &mut self,
        token: &NativeWorkerControllerQuery,
        facts: Option<&CapturedCanonicalBases>,
    ) -> Result<Progress, &'static str> {
        let progress = self
            .worker
            .poll_authored_parameter_extension_on_control(token)?;
        if let (Progress::Armed { id, steps, .. }, Some(facts)) = (progress, facts) {
            let next = extended(&self.callback.configuration, facts, id, steps)?;
            self.callback
                .configuration
                .reserve_program_revision_on_control()?;
            self.source_metadata.authored_bases = bases(&next);
            self.callback.configuration.authored = next;
        }
        Ok(progress)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_acknowledged_target_can_extend_exact_constructor_capabilities() {
        let class = hex_plugin_state::ClassId::new([1; 16]).unwrap();
        let caps = |class, values: &[(u32, f64)], step| {
            let mut bases = [None; MAX_FOREIGN_AUTOMATION_TARGETS];
            for (slot, &(id, value)) in bases.iter_mut().zip(values) {
                *slot = Some((id, value.to_bits()));
            }
            automation::SourceCapabilities::from_canonical(
                class,
                &bases,
                &[step; MAX_FOREIGN_AUTOMATION_TARGETS],
            )
            .unwrap()
        };
        let old = caps(class, &[(7, 0.25)], 0);
        let next = caps(class, &[(7, 0.25), (11, 0.75)], 0);
        validate_capabilities(&old, &next, class, 11, 0).unwrap();
        validate_capabilities(
            &automation::SourceCapabilities::reserved(class),
            &caps(class, &[(0, 0.25)], 0),
            class,
            0,
            0,
        )
        .unwrap();
        for (bad, id, steps) in [
            (caps(class, &[(7, 0.5), (11, 0.75)], 0), 11, 0),
            (caps(class, &[(11, 0.75)], 0), 11, 0),
            (caps(class, &[(7, 0.25), (11, 0.75), (15, 0.5)], 0), 11, 0),
            (next.clone(), 7, 0),
            (next.clone(), 11, 3),
            (
                caps(
                    hex_plugin_state::ClassId::new([2; 16]).unwrap(),
                    &[(7, 0.25), (11, 0.75)],
                    0,
                ),
                11,
                0,
            ),
        ] {
            assert!(validate_capabilities(&old, &bad, class, id, steps).is_err());
        }
    }
}
