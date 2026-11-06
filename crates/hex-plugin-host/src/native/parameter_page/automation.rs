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
    pub fn binding(&self) -> Binding {
        self.binding
    }

    pub fn class(&self) -> ClassId {
        self.class
    }

    pub(in crate::native) fn matches(&self, instance: &Prepared) -> bool {
        self.binding == instance.config.binding
            && self.class == instance.config.identity.class
            && self.source.as_ptr() == Arc::as_ptr(&instance.lease.cancellation.cancelled)
    }

    pub fn revision(&self) -> u64 {
        self.revision
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
        parameters.extend(ids.iter().map(|&id| Parameter { id, steps: None }));
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
            let Ok(index) = self
                .parameters
                .binary_search_by_key(&parameter.id, |p| p.id)
            else {
                continue;
            };
            let selection = &mut self.parameters[index];
            if !parameter.supports_authored_automation() || selection.steps.is_some() {
                return Err(Error::Protocol(
                    "authored parameter is ineligible or duplicated",
                ));
            }
            selection.steps = Some(parameter.step_count);
        }
        // The decoder bounds the whole extent to MAX_CONTROLLER_PARAMETERS.
        let first = page.first + page.as_slice().len() as u16;
        Ok((first < page.total).then_some(ParameterPageQuery {
            first,
            revision: page.revision,
            conversion: None,
        }))
    }

    fn finish(self, instance: &Prepared, revision: u64) -> Result<AuthoredParameterMetadata> {
        if self.parameters.iter().any(|p| p.steps.is_none()) {
            return Err(Error::Protocol("authored parameter is missing"));
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
