//! Source-bound SDK metadata for control-side authored automation preparation.
use super::*;
use protocol::{MAX_CONTROLLER_PARAMETERS, ParameterPage};

/// Capability observed during one coherent enumeration of the exact source.
/// The processing worker rechecks the selected capability; the page revision
/// may advance independently as normalized values change.
#[derive(Debug)]
pub struct AuthoredParameterMetadata {
    binding: Binding,
    source: std::sync::Weak<AtomicBool>,
    class: ClassId,
    revision: u64,
    parameters: Vec<Parameter>,
}

impl AuthoredParameterMetadata {
    pub(in crate::native) fn backing_data_bytes(&self) -> Option<usize> {
        self.parameters
            .capacity()
            .checked_mul(std::mem::size_of::<Parameter>())
    }

    pub fn binding(&self) -> Binding {
        self.binding
    }

    pub fn class(&self) -> ClassId {
        self.class
    }

    pub(in crate::native) fn matches(&self, instance: &Prepared) -> bool {
        self.binding == instance.config.binding && self.retains_lease(instance)
    }

    fn retains_lease(&self, instance: &Prepared) -> bool {
        self.class == instance.config.identity.class
            && self.source.as_ptr() == Arc::as_ptr(&instance.lease.cancellation.cancelled)
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Exact controller base first observed before worker transfer. Renewal
    /// on the same lease preserves it. IDs first selected after the initial
    /// arm have no base witness until a fresh canonical restoration.
    pub fn initial_normalized(&self, id: u32) -> Option<f64> {
        let index = self.parameters.binary_search_by_key(&id, |p| p.id).ok()?;
        self.parameters[index].normalized.map(f64::from_bits)
    }

    /// Zero steps means continuous. Raw ParamID zero is a valid key.
    pub fn step_count(&self, id: u32) -> Option<u32> {
        let index = self.parameters.binary_search_by_key(&id, |p| p.id).ok()?;
        self.parameters[index].steps
    }
}

impl Prepared {
    /// Collect only the requested raw IDs from this exact loaded instance.
    /// Call on the preparation owner before handing this reader to a running
    /// PCM worker. All pages share one deadline and one SDK metadata revision.
    /// Missing, duplicate, ineligible and changed metadata refuse the set.
    pub fn read_authored_parameter_metadata_on_control(
        &mut self,
        class: ClassId,
        ids: &[u32],
    ) -> Result<AuthoredParameterMetadata> {
        if self.config.identity.class != class {
            return Err(Error::Protocol(
                "authored parameter class differs from loaded source",
            ));
        }
        let mut selection = Selection::new(ids)?;
        let until = end_after(self.limits.startup)?;
        let mut query = ParameterPageQuery::default();
        loop {
            self.begin_controller_request_until(query.into(), until)?;
            let page = loop {
                match self.poll_parameter_page_on_worker()? {
                    ParameterPageProgress::Page(page) => break page,
                    ParameterPageProgress::Pending => {
                        if Instant::now() >= until {
                            return self.refuse_parameter_page(Error::Timeout);
                        }
                        pause(until);
                    }
                    ParameterPageProgress::Changed { .. } => {
                        return Err(Error::Protocol("authored parameter metadata changed"));
                    }
                    ParameterPageProgress::Conversion(_) | ParameterPageProgress::Editor(_) => {
                        return Err(Error::Protocol("expected authored parameter metadata page"));
                    }
                }
            };
            let Some(next) = selection.accept(&page)? else {
                return selection.finish(self, page.revision);
            };
            query = next;
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Parameter {
    id: u32,
    steps: Option<u32>,
    normalized: Option<u64>,
}

struct Selection {
    parameters: Vec<Parameter>,
    total: Option<u16>,
}

impl Selection {
    fn new(ids: &[u32]) -> Result<Self> {
        if ids.is_empty()
            || ids.len() > usize::from(MAX_CONTROLLER_PARAMETERS)
            || ids.windows(2).any(|ids| ids[0] >= ids[1])
            || ids.last().is_some_and(|&id| id > i32::MAX as u32)
        {
            return Err(Error::Protocol("invalid authored parameter ID set"));
        }
        let mut parameters = Vec::new();
        parameters
            .try_reserve_exact(ids.len())
            .map_err(|_| Error::Protocol("authored parameter metadata allocation refused"))?;
        parameters.extend(ids.iter().map(|&id| Parameter {
            id,
            steps: None,
            normalized: None,
        }));
        Ok(Self {
            parameters,
            total: None,
        })
    }

    fn accept(&mut self, page: &ParameterPage) -> Result<Option<ParameterPageQuery>> {
        if *self.total.get_or_insert(page.total) != page.total {
            return Err(Error::Protocol("authored parameter inventory changed"));
        }
        for parameter in page.as_slice() {
            self.accept_parameter(parameter)?;
        }
        // The decoder bounds the whole extent to MAX_CONTROLLER_PARAMETERS.
        let first = page.first + page.as_slice().len() as u16;
        Ok((first < page.total).then_some(ParameterPageQuery {
            first,
            revision: page.revision,
            conversion: None,
        }))
    }

    fn accept_parameter(&mut self, parameter: &protocol::ControllerParameter) -> Result<()> {
        let Ok(index) = self
            .parameters
            .binary_search_by_key(&parameter.id, |p| p.id)
        else {
            return Ok(());
        };
        let selection = &mut self.parameters[index];
        if !parameter.supports_authored_automation() || selection.steps.is_some() {
            return Err(Error::Protocol(
                "authored parameter is ineligible or duplicated",
            ));
        }
        selection.steps = Some(parameter.step_count);
        selection.normalized = Some(parameter.normalized.to_bits());
        Ok(())
    }

    fn retain_initial_values(&mut self, previous: &AuthoredParameterMetadata) {
        for parameter in &mut self.parameters {
            parameter.normalized = previous.initial_normalized(parameter.id).map(f64::to_bits);
        }
    }

    fn finish(mut self, instance: &Prepared, revision: u64) -> Result<AuthoredParameterMetadata> {
        if self.parameters.iter().any(|p| p.steps.is_none()) {
            return Err(Error::Protocol("authored parameter is missing"));
        }
        // Epoch renewal refreshes capabilities on the same loaded lease. Its
        // current controller values may already be authored or manually held.
        // Carry the first canonical observations across that renewal. A newly
        // selected ID has no base witness until a fresh canonical restoration.
        if let Some(previous) = instance
            .authored_parameter_metadata
            .as_ref()
            .filter(|previous| previous.retains_lease(instance))
        {
            self.retain_initial_values(previous);
        }
        Ok(AuthoredParameterMetadata {
            binding: instance.config.binding,
            source: Arc::downgrade(&instance.lease.cancellation.cancelled),
            class: instance.config.identity.class,
            revision,
            parameters: self.parameters,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_selected_controller_values_are_lossless_and_refused_rows_cannot_replace_them() {
        let mut selection = Selection::new(&[0, 7]).unwrap();
        let value = 0.12345678901234566;
        let parameter = protocol::ControllerParameter {
            id: 0,
            flags: 1,
            normalized: value,
            ..Default::default()
        };
        selection.accept_parameter(&parameter).unwrap();
        assert!(
            selection
                .accept_parameter(&protocol::ControllerParameter {
                    normalized: 0.75,
                    ..parameter
                })
                .is_err()
        );
        assert!(
            selection
                .accept_parameter(&protocol::ControllerParameter {
                    id: 7,
                    flags: 3,
                    ..parameter
                })
                .is_err()
        );
        selection
            .accept_parameter(&protocol::ControllerParameter {
                id: 100,
                ..parameter
            })
            .unwrap();
        let class = ClassId::new([1; 16]).unwrap();
        let metadata = AuthoredParameterMetadata {
            binding: Binding {
                root: 1,
                instance: 2,
                graph: 3,
                epoch: 4,
            },
            source: std::sync::Weak::new(),
            class,
            revision: 1,
            parameters: selection.parameters,
        };
        assert_eq!(metadata.initial_normalized(0), Some(value));
        assert_eq!(metadata.initial_normalized(7), None);
        assert_eq!(metadata.initial_normalized(100), None);
        assert_eq!(metadata.step_count(0), Some(0));
        let mut renewed = Selection::new(&[0, 7]).unwrap();
        renewed
            .accept_parameter(&protocol::ControllerParameter {
                normalized: 0.75,
                ..parameter
            })
            .unwrap();
        renewed
            .accept_parameter(&protocol::ControllerParameter {
                id: 7,
                normalized: 0.5,
                ..parameter
            })
            .unwrap();
        renewed.retain_initial_values(&metadata);
        assert_eq!(renewed.parameters[0].normalized, Some(value.to_bits()));
        assert_eq!(renewed.parameters[1].normalized, None);
    }
}
