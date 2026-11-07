//! Final source ACK stays in the same reserved Root cell. Neither preparation
//! completion nor this read-only state copy seals endpoints for publication.
use super::*;
use hex_plugin_host::protocol::PresentationPlan;

impl Custody {
    pub(in crate::candidate_custody) fn native_source_metadata(
        &self,
        binding: Binding,
    ) -> Result<crate::plugin_effects::EffectSourceMetadata, String> {
        let work = self
            .work
            .try_lock()
            .map_err(|_| "native preparation is busy")?;
        let bank = self.bank.try_lock().map_err(|_| "native source is busy")?;
        let index = bank.sources.index(binding).ok_or("native source differs")?;
        let slot = &bank.sources.slots[index];
        if bank.closing || slot.retiring || slot.starting || slot.phase != Phase::Accepted {
            return Err("native source metadata is unavailable".into());
        }
        let source = match work.sources.cells[index].owner.as_ref() {
            Some(Owner::Effect(owner)) => NativeEffectReference::Unsplit(owner),
            Some(Owner::Loaned(owner)) => NativeEffectReference::Loaned(owner),
            _ => return Err("native source is not constructed".into()),
        };
        if source.binding() != binding || slot.identity != Some(source.identity()) {
            return Err("native source metadata binding differs".into());
        }
        // Work retains the actual source. Keep the ordinary bank available
        // for close/cancellation while allocating the bounded copied strings.
        drop(bank);
        source
            .source_metadata()
            .try_copy_on_control()
            .map_err(|error| error.to_string())
    }
    pub(in crate::candidate_custody) fn native_source_for_base_capture(
        &self,
        binding: Binding,
        canonical: &NativeDeviceState,
        require_installed: bool,
    ) -> Result<crate::plugin_effects::BaseCaptureSource, String> {
        let work = self
            .work
            .try_lock()
            .map_err(|_| "native preparation is busy")?;
        let bank = self.bank.try_lock().map_err(|_| "native source is busy")?;
        let index = bank.sources.index(binding).ok_or("native source differs")?;
        let slot = &bank.sources.slots[index];
        let source = match work.sources.cells[index].owner.as_ref() {
            Some(Owner::Effect(owner)) => NativeEffectReference::Unsplit(owner),
            Some(Owner::Loaned(owner)) => NativeEffectReference::Loaned(owner),
            _ => return Err("native base capture requires the original processor".into()),
        };
        if bank.closing
            || slot.phase != Phase::Accepted
            || slot.starting
            || slot.retiring
            || (require_installed
                && (!slot.endpoint_loaned
                    || !slot.endpoint_return.as_ref().is_some_and(|signal| {
                        signal.installed.load(Ordering::Acquire)
                            && signal.runtime_required.load(Ordering::Acquire)
                    })))
            || source.binding() != binding
            || slot.identity != Some(source.identity())
            || !(source.matches_canonical_state(canonical)
                || source.approves_captured_state_on_control(canonical))
        {
            return Err("native base capture source differs from the canonical processor".into());
        }
        let (_, presentation) = slot
            .presentation
            .ok_or("native presentation is unavailable")?;
        drop(bank);
        Ok(crate::plugin_effects::BaseCaptureSource {
            metadata: source
                .source_metadata()
                .try_copy_on_control()
                .map_err(|error| error.to_string())?,
            identity: source.identity(),
            profile: source.profile(),
            presentation,
            prepared: source.prepared(),
            canonical_is_constructor_state: source.matches_canonical_state(canonical),
        })
    }

    pub(in crate::candidate_custody) fn complete_native_source(
        &self,
        binding: Binding,
        presentation: impl Into<PresentationPlan>,
    ) -> Result<(), &'static str> {
        self.complete_native_source_between(binding, presentation, || {}, || {})
    }

    pub(super) fn complete_native_source_between(
        &self,
        binding: Binding,
        presentation: impl Into<PresentationPlan>,
        after_reservation: impl FnOnce(),
        after_completion: impl FnOnce(),
    ) -> Result<(), &'static str> {
        let presentation = presentation.into();
        let Ok(mut work) = self.work.try_lock() else {
            return Err("native preparation is busy");
        };
        let index = {
            let mut bank = lock(&self.bank);
            let index = bank.sources.index(binding).ok_or("native source differs")?;
            let slot = &mut bank.sources.slots[index];
            if slot.phase != Phase::Accepted
                || slot.retiring
                || slot.starting
                || slot.presentation.is_some()
            {
                return Err("native source is not awaiting completion");
            }
            if !matches!(
                work.sources.cells[index].owner,
                Some(Owner::Pending(_) | Owner::Quiescent(_))
            ) {
                return Err("native preparation owner differs");
            }
            slot.starting = true;
            index
        };
        let cell = &mut work.sources.cells[index];
        // Set the persistent pending marker before extracting the unique owner.
        // Only the dedicated Work lock spans native I/O; close can still cancel
        // the exact source through its separately retained token.
        cell.pending = true;
        after_reservation();
        let result = catch_unwind(AssertUnwindSafe(|| match cell.owner.take() {
            Some(Owner::Pending(owner)) => match owner.complete_retained_on_control(presentation) {
                Ok(owner) => cell.owner = Some(Owner::Prepared(owner)),
                Err(owner) => {
                    cell.constructor_refused = true;
                    cell.owner = Some(Owner::Refused(owner));
                }
            },
            Some(Owner::Quiescent(owner)) => {
                match owner.resume_retained_on_control(binding, presentation) {
                    Ok(owner) => cell.owner = Some(Owner::Prepared(owner)),
                    Err(owner) => {
                        cell.constructor_refused = true;
                        cell.owner = Some(Owner::Refused(owner));
                    }
                }
            }
            other => cell.owner = other,
        }));
        if let Err(payload) = result {
            cell.first_panic = Some(payload);
            self.fail_native_source(index);
            return Ok(());
        }
        after_completion();
        cell.pending = false;
        let completed = match cell.owner.as_ref() {
            Some(Owner::Prepared(owner)) => Some((owner.ready().presentation, presentation)),
            _ => None,
        };
        let mut bank = lock(&self.bank);
        let closing = bank.closing;
        let slot = &mut bank.sources.slots[index];
        slot.starting = false;
        slot.presentation = completed;
        slot.retiring |= closing || completed.is_none();
        if completed.is_none() {
            slot.phase = Phase::Cleaning;
        }
        Ok(()) // Custody accepted, including native refusal or concurrent close.
    }

    pub(in crate::candidate_custody) fn native_source_prepared_state(
        &self,
        binding: Binding,
    ) -> Option<NativeDeviceState> {
        let work = self.work.try_lock().ok()?;
        let bank = self.bank.try_lock().ok()?;
        let index = bank.sources.index(binding)?;
        let slot = &bank.sources.slots[index];
        if slot.retiring || slot.starting || slot.phase != Phase::Accepted {
            return None;
        }
        if let Some(Owner::Effect(owner)) = work.sources.cells[index].owner.as_ref() {
            return Some(owner.configuration().clone());
        }
        if let Some(Owner::Loaned(owner)) = work.sources.cells[index].owner.as_ref() {
            return Some(owner.configuration().clone());
        }
        let Some(Owner::Prepared(owner)) = work.sources.cells[index].owner.as_ref() else {
            return None;
        };
        // Preserve the original restore envelope, even if the plugin normalizes
        // its capture. A copy is a query, never a source/publication witness.
        Some(
            owner
                .configuration()
                .restore
                .as_ref()
                .unwrap_or(&owner.ready().initial_state)
                .clone(),
        )
    }
}

impl CandidateRoot {
    pub(crate) fn native_source_metadata(
        &self,
        binding: Binding,
    ) -> Result<crate::plugin_effects::EffectSourceMetadata, String> {
        self.require_native_source_control_open()
            .map_err(str::to_owned)?;
        self.0.eligible.native_source_metadata(binding)
    }
    pub(crate) fn native_source_for_base_capture(
        &self,
        binding: Binding,
        canonical: &NativeDeviceState,
        require_installed: bool,
    ) -> Result<crate::plugin_effects::BaseCaptureSource, String> {
        self.require_native_source_control_open()
            .map_err(str::to_owned)?;
        self.0
            .eligible
            .native_source_for_base_capture(binding, canonical, require_installed)
    }

    pub(crate) fn complete_native_source_preparation(
        &self,
        binding: Binding,
        presentation: impl Into<PresentationPlan>,
    ) -> Result<(), &'static str> {
        self.require_native_source_control_open()?;
        let result = self
            .0
            .eligible
            .complete_native_source(binding, presentation);
        crate::lifecycle::request_candidate_cleanup(self);
        result
    }

    pub(crate) fn native_source_prepared_state(
        &self,
        binding: Binding,
    ) -> Option<NativeDeviceState> {
        self.0.eligible.native_source_prepared_state(binding)
    }
}
