//! Bounded capability extension through the original serialized SDK reader.
//! Reservation is control-side; an acknowledged extension grants no base value.
use super::*;

#[derive(Clone, Copy)]
pub(in crate::native) struct AuthoredTarget {
    id: u32,
    steps: u32,
}

/// SDK capability acknowledgement only. No normalized canonical base, host
/// source admission, endpoint custody or graph publication is granted.
#[derive(Debug, PartialEq, Eq)]
pub enum AuthoredParameterExtensionProgress {
    Pending,
    Armed { revision: u64, id: u32, steps: u32 },
    Changed { revision: u64, total: u16 },
    Refused(&'static str),
}

impl Prepared {
    /// Prospective steady backing after reserving this metadata capacity. No
    /// allocation or native I/O occurs. This is a quote, not source admission;
    /// reservation must still validate the live epoch before worker transfer.
    pub fn storage_with_authored_capacity_on_control(
        &self,
        limit: usize,
    ) -> Option<crate::native::PreparedStorage> {
        if self.parameter_page.is_some()
            || limit == 0
            || limit > usize::from(MAX_CONTROLLER_PARAMETERS)
        {
            return None;
        }
        let mut storage = self.retained_storage_on_control()?;
        let previous = if let Some(metadata) = &self.authored_parameter_metadata {
            if !metadata.retains_lease(self)
                || metadata.parameters.len() > limit
                || metadata.extension_limit.is_some_and(|old| old > limit)
                || (self.sequence != 0
                    && (metadata.extension_limit != Some(limit)
                        || metadata.parameters.capacity() < limit))
            {
                return None;
            }
            metadata.parameters.capacity()
        } else {
            if self.sequence != 0 {
                return None;
            }
            storage.shared_handle_bound = storage.shared_handle_bound.checked_add(1)?;
            0
        };
        let added = limit
            .saturating_sub(previous)
            .checked_mul(std::mem::size_of::<Parameter>())?;
        storage.data_bytes = storage.data_bytes.checked_add(added)?;
        Some(storage)
    }

    /// Grow only before the first PCM transfer; acknowledged epoch renewal may
    /// reuse the same reservation without allocation. Exact Vec capacity is charged
    /// by the existing source storage inventory. Live extension never allocates
    /// or increases this logical limit. An empty reservation arms no target.
    pub fn reserve_authored_parameter_capacity_on_control(&mut self, limit: usize) -> Result<()> {
        self.require_live_epoch()?;
        if self
            .storage_with_authored_capacity_on_control(limit)
            .is_none()
        {
            return Err(Error::Protocol(
                "authored reservation requires unused or already reserved capacity",
            ));
        }
        if limit == 0 || limit > usize::from(MAX_CONTROLLER_PARAMETERS) {
            return Err(Error::Protocol("invalid authored parameter reservation"));
        }
        if let Some(metadata) = self.authored_parameter_metadata.as_ref()
            && (!metadata.retains_lease(self)
                || metadata.parameters.len() > limit
                || metadata.extension_limit.is_some_and(|old| old > limit))
        {
            return Err(Error::Protocol("authored parameter reservation differs"));
        }
        if self.authored_parameter_metadata.is_none() {
            let mut parameters = Vec::new();
            parameters
                .try_reserve_exact(limit)
                .map_err(|_| Error::Protocol("authored parameter reservation unavailable"))?;
            self.authored_parameter_metadata = Some(AuthoredParameterMetadata {
                binding: self.config.binding,
                source: Arc::downgrade(&self.lease.cancellation.cancelled),
                class: self.config.identity.class,
                revision: 0,
                parameters,
                extension_limit: Some(limit),
            });
            return Ok(());
        }
        let Some(metadata) = self.authored_parameter_metadata.as_mut() else {
            return Err(Error::Protocol(
                "authored parameter reservation disappeared",
            ));
        };
        if metadata.binding != self.config.binding {
            // Resuming the same lease invalidates eligibility, not historical
            // base observations. Only a fresh SDK query can arm the new epoch.
            metadata.binding = self.config.binding;
            metadata.revision = 0;
            for parameter in &mut metadata.parameters {
                parameter.steps = None;
            }
        }
        metadata
            .parameters
            .try_reserve_exact(limit - metadata.parameters.len())
            .map_err(|_| Error::Protocol("authored parameter reservation unavailable"))?;
        metadata.extension_limit = Some(limit);
        Ok(())
    }

    /// Validate one selected target through a fresh page from this exact SDK
    /// reader. The caller supplies the previously observed page/revision/steps.
    /// The selected page is read fresh: PCM value feedback may advance ordinary
    /// enumeration revisions while this operation is pending. Exact target,
    /// flags and steps are revalidated; no current value becomes a base witness.
    /// Shares the ordinary one-query lane; PCM exchanges may continue while pending.
    pub fn begin_authored_parameter_extension_on_worker(
        &mut self,
        class: ClassId,
        id: u32,
        steps: u32,
        query: ParameterPageQuery,
    ) -> Result<()> {
        self.require_live_epoch()?;
        if class != self.config.identity.class
            || id > i32::MAX as u32
            || query.revision == 0
            || query.conversion.is_some()
        {
            return Err(Error::Protocol("authored extension identity differs"));
        }
        let metadata = self
            .authored_parameter_metadata
            .as_ref()
            .filter(|metadata| metadata.matches(self))
            .ok_or(Error::Protocol(
                "authored parameter capacity was not reserved",
            ))?;
        let limit = metadata.extension_limit.ok_or(Error::Protocol(
            "authored parameter capacity was not reserved",
        ))?;
        if metadata
            .parameters
            .binary_search_by_key(&id, |parameter| parameter.id)
            .is_err()
            && metadata.parameters.len() >= limit
        {
            return Err(Error::Protocol("authored parameter capacity is full"));
        }
        query.validate()?;
        self.begin_controller_request_on_worker(ControllerRequest::SelectedParameters {
            first: query.first,
        })?;
        let page = self.parameter_page.as_mut().ok_or(Error::Protocol(
            "authored parameter query admission disappeared",
        ))?;
        page.authored = Some(AuthoredTarget { id, steps });
        Ok(())
    }

    /// Poll on the same serialized reader through its exact terminal. Refusal
    /// is an ordinary capability outcome, not a processor fault. New targets
    /// deliberately have no initial canonical value witness.
    pub fn poll_authored_parameter_extension_on_worker(
        &mut self,
    ) -> Result<AuthoredParameterExtensionProgress> {
        use AuthoredParameterExtensionProgress as Progress;
        let target = self
            .parameter_page
            .and_then(|page| page.authored)
            .ok_or(Error::Protocol(
                "authored parameter extension is not pending",
            ))?;
        match self.poll_parameter_page()? {
            ParameterPageProgress::Pending => Ok(Progress::Pending),
            ParameterPageProgress::Changed { revision, total } => {
                Ok(Progress::Changed { revision, total })
            }
            ParameterPageProgress::Page(page) => {
                let mut rows = page.as_slice().iter().filter(|row| row.id == target.id);
                let Some(row) = rows.next().filter(|_| rows.next().is_none()) else {
                    return Ok(Progress::Refused(
                        "authored parameter is absent or duplicated",
                    ));
                };
                let Some(metadata) = self.authored_parameter_metadata.as_mut() else {
                    return Ok(Progress::Refused(
                        "authored parameter reservation disappeared",
                    ));
                };
                if let Err(reason) = metadata.extend(target, row) {
                    return Ok(Progress::Refused(reason));
                }
                metadata.revision = page.revision;
                Ok(Progress::Armed {
                    revision: page.revision,
                    id: target.id,
                    steps: target.steps,
                })
            }
            ParameterPageProgress::Conversion(_)
            | ParameterPageProgress::Editor(_)
            | ParameterPageProgress::EditorSeal(_) => Ok(Progress::Refused(
                "expected authored parameter metadata page",
            )),
        }
    }
}

impl AuthoredParameterMetadata {
    fn extend(
        &mut self,
        target: AuthoredTarget,
        row: &protocol::ControllerParameter,
    ) -> std::result::Result<(), &'static str> {
        if row.id != target.id
            || row.step_count != target.steps
            || !row.supports_authored_automation()
        {
            return Err("authored parameter capability changed");
        }
        match self
            .parameters
            .binary_search_by_key(&target.id, |parameter| parameter.id)
        {
            Ok(index) if self.parameters[index].steps.is_none() => {
                self.parameters[index].steps = Some(target.steps);
                Ok(())
            }
            Ok(index) if self.parameters[index].steps == Some(target.steps) => Ok(()),
            Ok(_) => Err("armed authored parameter steps changed"),
            Err(index) => {
                if self
                    .extension_limit
                    .is_none_or(|limit| self.parameters.len() >= limit)
                    || self.parameters.len() == self.parameters.capacity()
                {
                    return Err("authored parameter capacity is full");
                }
                self.parameters.insert(
                    index,
                    Parameter {
                        id: target.id,
                        steps: Some(target.steps),
                        normalized: None,
                    },
                );
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_preserves_bases_and_storage_and_refuses_changed_or_full_metadata() {
        let mut metadata = AuthoredParameterMetadata {
            binding: Binding {
                root: 1,
                instance: 2,
                graph: 3,
                epoch: 4,
            },
            source: std::sync::Weak::new(),
            class: ClassId::new([1; 16]).unwrap(),
            revision: 1,
            parameters: Vec::with_capacity(2),
            extension_limit: Some(2),
        };
        metadata.parameters.push(Parameter {
            id: 7,
            steps: Some(0),
            normalized: Some(0.25_f64.to_bits()),
        });
        let backing = metadata.backing_data_bytes();
        let target = AuthoredTarget { id: 0, steps: 3 };
        let row = protocol::ControllerParameter {
            id: 0,
            step_count: 3,
            flags: 1,
            normalized: 0.75,
            ..Default::default()
        };
        for changed in [
            protocol::ControllerParameter { flags: 3, ..row },
            protocol::ControllerParameter {
                step_count: 4,
                ..row
            },
            protocol::ControllerParameter { id: 1, ..row },
        ] {
            assert!(metadata.extend(target, &changed).is_err());
            assert_eq!(metadata.parameters.len(), 1);
        }
        let mut result = Ok(());
        let measured = allocation_counter::measure(|| result = metadata.extend(target, &row));
        result.unwrap();
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(metadata.initial_normalized(7), Some(0.25));
        assert_eq!(metadata.initial_normalized(0), None);
        assert_eq!(metadata.step_count(0), Some(3));
        assert_eq!(
            metadata.parameters.iter().map(|p| p.id).collect::<Vec<_>>(),
            [0, 7]
        );
        metadata.extend(target, &row).unwrap();
        assert!(
            metadata
                .extend(
                    AuthoredTarget { id: 9, steps: 3 },
                    &protocol::ControllerParameter { id: 9, ..row }
                )
                .is_err()
        );
        assert_eq!(metadata.backing_data_bytes(), backing);
        assert_eq!(metadata.parameters.len(), 2);
    }
}
