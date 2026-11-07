//! Ordered canonical edits prepare changed native instances before publication.
//! Poll only existing Root work/returns; the audio callback never waits. Later
//! FIFO requests retain their ordinary completion order. An accepted document
//! intent or shutdown cancels preparation without consuming another request.
use super::*;
use hex_audio::prepared::PublicationControl;
use hex_session::service::RequestCancellation;

impl Plugins {
    pub(in crate::native_audio) fn preparing_edit(&self, context: AudioSourceContext) -> bool {
        self.edit == Some(context)
    }
    pub(in crate::native_audio) fn finish_edit(
        &mut self,
        context: AudioSourceContext,
        accepted: bool,
    ) {
        if self.edit != Some(context) {
            return;
        }
        self.edit = None;
        for source in &mut self.sources {
            if matches!(source.operation, SourceOperation::Edit(current) if current == context) {
                source.accepted = accepted && !source.retiring;
                source.retiring |= !source.accepted;
            }
        }
        if accepted {
            // A newly restored version supersedes earlier instances of this
            // canonical device. History owns their opaque payloads, not their
            // live DSP state: epoch resume deliberately does not restore those
            // bytes. Keep the newest source as restoration provenance and let
            // Root retire predecessors only after their actual graph/endpoint
            // and outgoing-tail references return. Refusal leaves predecessors
            // untouched; no endpoint is reset or freed here.
            for index in 0..self.sources.len() {
                let superseded = {
                    let old = &self.sources[index];
                    old.accepted
                        && !old.retiring
                        && !matches!(old.operation, SourceOperation::Edit(current) if current == context)
                        && self.sources.iter().any(|new| {
                            new.accepted
                                && !new.retiring
                                && new.project_generation == old.project_generation
                                && new.device == old.device
                                && matches!(new.operation, SourceOperation::Edit(current) if current == context)
                        })
                };
                if superseded {
                    self.sources[index].accepted = false;
                    self.sources[index].retiring = true;
                }
            }
        }
    }
    #[allow(clippy::too_many_arguments)] // Exact target and its existing canonical source.
    pub(super) fn matching_edit_source(
        &self,
        control: &mut PublicationControl,
        input: &ResidentEffectPreparation<'_>,
        destination: PluginEffectDestination,
        device: u64,
        state: &NativeDeviceState,
        desired: Option<&CanonicalAudioSourceSnapshot>,
        cancellation: Option<&RequestCancellation>,
    ) -> Result<Option<Binding>, String> {
        let owner = destination.owner();
        let retain_instrument = destination.is_instrument()
            && control
                .status
                .playing
                .load(std::sync::atomic::Ordering::Acquire);
        let authored = AuthoredSelection::from_targets(input.authored_plugin_targets(device))?;
        let mut changed_state = false;
        if input.preparing_edit()
            && let Some(desired) = desired.filter(|source| {
                source.context().project_generation == input.context().project_generation
            })
        {
            let previous = desired.effect_preparation();
            changed_state = !input.same_authored_plugin_program(
                &previous,
                device,
                control.plugin_callback_quantum_on_control(),
            );
            previous.visit_plugin_devices(|old_owner, old_device, old| {
                changed_state |= old_device == device
                    && (old_owner.owner() != owner || !old.processing_eq(state));
                Ok(())
            })?;
        }
        // The latest accepted preparation for an equal state is the current
        // device's source; older matching instances can still own old tails.
        for source in self.sources.iter().rev() {
            if source.project_generation != input.context().project_generation
                || source.owner != owner
                || source.device != device
                || !match &source.authored {
                    Some(armed) => armed.covers(authored.as_ref()),
                    None => authored.is_none(),
                }
                || source.retiring
                || (input.preparing_edit()
                    && !source.accepted
                    && !matches!(source.operation, SourceOperation::Edit(context) if context == input.context()))
            {
                continue;
            }
            if changed_state
                && !retain_instrument
                && !matches!(source.operation, SourceOperation::Edit(context) if context == input.context())
            {
                // A historical matching payload does not free its physical
                // endpoint from the actual outgoing tail. Prepare this edit's
                // own source instead of introducing a current/tail alias.
                continue;
            }
            if (retain_instrument
                && control.native_source_approves_captured_state_on_control(source.binding, state)
                    == Some(true))
                || original_state(control, source.binding, cancellation)?.processing_eq(state)
            {
                return Ok(Some(source.binding));
            }
        }
        Ok(None)
    }
    #[allow(clippy::too_many_arguments)] // One exact prospective device and its canonical/source contexts.
    pub(super) fn prepare_edit_source(
        &mut self,
        output: &mut WorkerPreparedOutput,
        input: &ResidentEffectPreparation<'_>,
        destination: PluginEffectDestination,
        device: u64,
        state: &NativeDeviceState,
        desired: Option<&CanonicalAudioSourceSnapshot>,
        cancellation: Option<&RequestCancellation>,
    ) -> Result<(), String> {
        let context = input.context();
        let owner = destination.owner();
        let control = output.publication.control().ok_or("native output closed")?;
        if self
            .matching_edit_source(
                control,
                input,
                destination,
                device,
                state,
                desired,
                cancellation,
            )?
            .is_some()
        {
            return Ok(());
        }
        let authored = AuthoredSelection::from_targets(input.authored_plugin_targets(device))?;
        let identity = Hosting::saved_identity_on_control(state)?;
        // Resolve code only: provider identity is class + binary hashes, not a
        // path or another device's instance (hex-plugin-state owns that contract).
        // begin_on_control below always mints the target device's fresh binding
        // and restores its candidate bytes; this never shares the old processor.
        let mut original = None;
        for source in &self.sources {
            if source.project_generation == context.project_generation
                && source.accepted
                && !source.retiring
                && Hosting::saved_identity_on_control(&original_state(
                    control,
                    source.binding,
                    cancellation,
                )?)? == identity
            {
                original = Some(source.binding);
                break;
            }
        }
        let Some(original) = original else {
            // Duplicating an existing unavailable device preserves its exact
            // opaque state, without inventing an empty/default processor.
            let mut preserved = false;
            if let Some(desired) = desired
                .filter(|source| source.context().project_generation == context.project_generation)
            {
                desired
                    .effect_preparation()
                    .visit_plugin_devices(|_, _, old| {
                        preserved |= old.processing_eq(state);
                        Ok(())
                    })?;
            }
            return if preserved {
                Ok(())
            } else {
                Err("original vst3 source unavailable for the changed state".into())
            };
        };
        if self.pending.is_some() || self.restore.is_some() || self.reopen.is_some() {
            return Err("another vst3 source operation is pending".into());
        }
        if !self.configured || self.sources.len() == self.sources.capacity() {
            return Err("vst3 source/history capacity is full".into());
        }
        if self.edit.is_some_and(|current| current != context) {
            return Err("vst3 edit source changed".into());
        }
        self.edit = Some(context);
        let metadata = loop {
            check_cancel(cancellation)?;
            match control.native_source_metadata(original) {
                Ok(metadata) => break metadata,
                Err(error)
                    if matches!(
                        error.as_str(),
                        "native preparation is busy" | "native source is busy"
                    ) =>
                {
                    poll_control(control);
                }
                Err(error) => return Err(error),
            }
        };
        let binding = loop {
            check_cancel(cancellation)?;
            match self
                .host
                .as_ref()
                .ok_or("qualified vst3 helper unavailable")?
                .begin_on_control(
                    control,
                    metadata.selection(),
                    owner,
                    device,
                    context.editor_revision.max(1),
                    output.sample_rate,
                    hosting_profile(destination),
                    Some(state.clone()),
                ) {
                Ok(binding) => break binding,
                Err(error) if error == "native preparation is busy" => poll_control(control),
                Err(error) => return Err(error),
            }
        };
        drop(metadata);
        self.sources.push(Source {
            binding,
            owner,
            device,
            project_generation: context.project_generation,
            operation: SourceOperation::Edit(context),
            authored,
            accepted: false,
            retiring: false,
            queued: false,
            observes_latency: false,
            editor_open: false,
            edit_backlog: false,
            #[cfg(all(test, feature = "allocation-probe"))]
            observed_latency_receipt: false,
            #[cfg(all(test, feature = "allocation-probe"))]
            attempts: 0,
        });
        loop {
            check_cancel(cancellation)?;
            let control = output.publication.control().ok_or("native output closed")?;
            poll_control(control);
            let source = self
                .sources
                .iter_mut()
                .find(|source| source.binding == binding)
                .ok_or("original vst3 edit source unavailable")?;
            #[cfg(not(all(test, feature = "allocation-probe")))]
            let presentation = None;
            #[cfg(all(test, feature = "allocation-probe"))]
            let presentation = self.presentation;
            let ready = advance_source(
                self.host
                    .as_ref()
                    .ok_or("qualified vst3 helper unavailable")?,
                presentation,
                control,
                source,
            )?;
            self.poll(output, desired.map(|source| source.context()));
            if let Some(ready) = ready {
                return if ready.processing_eq(state) {
                    Ok(())
                } else {
                    Err("vst3 restored state differs from the edit".into())
                };
            }
        }
    }
}
pub(super) fn check_cancel(cancellation: Option<&RequestCancellation>) -> Result<(), String> {
    if cancellation.is_some_and(RequestCancellation::is_cancelled) {
        Err("vst3 edit cancelled by document request or shutdown".into())
    } else {
        Ok(())
    }
}
pub(super) fn poll_control(control: &mut PublicationControl) {
    control.reclaim();
    control.poll_resident_publication();
    // Existing control-worker polling cadence. This is not a paint delay,
    // native ACK, timeout-based acceptance or an audio callback wait.
    std::thread::sleep(std::time::Duration::from_millis(5));
}
pub(super) fn original_state(
    control: &mut PublicationControl,
    binding: Binding,
    cancellation: Option<&RequestCancellation>,
) -> Result<NativeDeviceState, String> {
    loop {
        check_cancel(cancellation)?;
        if let Some(state) = control.native_source_prepared_state(binding) {
            return Ok(state);
        }
        if matches!(
            control.native_source_preparation_progress(binding),
            Some(Progress::Failed | Progress::Retiring | Progress::Complete)
        ) {
            return Err(control
                .native_source_preparation_error(binding)
                .unwrap_or_else(|| "original vst3 source is unavailable".into()));
        }
        match control.native_source_metadata(binding) {
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.as_str(),
                    "native preparation is busy" | "native source is busy"
                ) => {}
            Err(error) => return Err(error),
        }
        poll_control(control);
    }
}

/// Consume the private query through disposal even after cancellation.
pub(super) fn canonical_bases(
    host: &Hosting,
    control: &mut PublicationControl,
    binding: Binding,
    state: &NativeDeviceState,
    selection: &AuthoredSelection,
    cancellation: Option<&RequestCancellation>,
) -> Result<hex_audio::plugin_effects::CapturedCanonicalBases, String> {
    let playing = control
        .status
        .playing
        .load(std::sync::atomic::Ordering::Acquire);
    let source = control
        .status
        .native_parameter_source(playing)
        .ok_or("vst3 canonical source is not ready")?
        .0;
    let mut query = loop {
        check_cancel(cancellation)?;
        match host.begin_canonical_bases_capture_on_control(
            control, source, binding, state, selection, playing,
        ) {
            Err(error) if error == "native preparation is busy" => poll_control(control),
            result => break result?,
        }
    };
    loop {
        let mut cancelled = || cancellation.is_some_and(RequestCancellation::is_cancelled);
        if let Some(result) = query.poll_on_control(control, &mut cancelled) {
            return result;
        }
        poll_control(control);
    }
}
