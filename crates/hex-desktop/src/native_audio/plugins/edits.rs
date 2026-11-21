//! Ordered canonical edits prepare changed native instances before publication.
//! Poll only existing Root work/returns; the audio callback never waits. Later
//! FIFO requests retain their ordinary completion order. An accepted document
//! intent or shutdown cancels preparation without consuming another request.
use super::*;
use hex_audio::prepared::{AppliedAudioSource, PublicationControl};
use hex_engine::playback::{HostedMoveRequest, HostedTrackEffectMove};
use hex_session::service::RequestCancellation;
mod authored;

impl Plugins {
    pub(super) fn retained_instrument_move(
        &self,
        output: Option<&mut WorkerPreparedOutput>,
        input: &ResidentEffectPreparation<'_>,
        desired: Option<&CanonicalAudioSourceSnapshot>,
    ) -> Result<Option<hosted_move::MoveRoute>, String> {
        let Some(route) = input.hosted_instrument_move() else {
            return Ok(None);
        };
        let control = output
            .and_then(|output| output.publication.control())
            .ok_or("native output closed")?;
        if !control
            .status
            .playing
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Ok(None);
        }
        let predecessor = retained_move_predecessor(control, route.predecessor, input, desired)?;
        let device = route.device.value();
        let donor = ProcessorChainOwner::Track(route.donor.value());
        let mut sources = self.sources.iter().filter(|source| {
            source.owner == donor
                && source.device == device
                && source.project_generation == predecessor.context.project_generation
                && source.accepted
                && !source.retiring
                && source.binding.root == predecessor.output_identity
        });
        let source = sources
            .next()
            .ok_or("original vst3 move worker is unavailable")?;
        if sources.next().is_some() {
            return Err("original vst3 move worker is ambiguous".into());
        }
        // The read mode also covers ordinary Resident polling. A read at
        // the exact predecessor stays with the same physical worker. An
        // observed latency restart or a recovery read from another source
        // still needs to settle before the move.
        if source.editor_open
            || source.open_editor_group.is_some()
            || source.edit_backlog
            || source
                .edit_recovery
                .is_some_and(|read| read.recovery && read.source != predecessor)
            || self
                .latency_notice
                .is_some_and(|(source, _)| source == predecessor)
            || source.pending_touch.is_some()
        {
            return Err("running vst3 move requires one settled instrument route".into());
        }
        let physical_owner = if let Some(hosted_move::AdoptedRoute {
            request: HostedMoveRequest::Instrument(installed),
            ..
        }) = self.adopted_move(input.context())
        {
            if installed.receiver != route.donor
                || installed.physical_owner != route.receiver
                || installed.device != route.device
                || installed.binding != source.binding
            {
                return Err(
                    "running vst3 move must return the original worker to its first track".into(),
                );
            }
            installed.physical_owner
        } else {
            route.donor
        };
        Ok(Some(hosted_move::MoveRoute::prepare(
            input.context(),
            route.predecessor,
            HostedMoveRequest::Instrument(hex_engine::playback::HostedInstrumentMove {
                donor: route.donor,
                receiver: route.receiver,
                physical_owner,
                device: route.device,
                binding: source.binding,
            }),
            predecessor,
        )))
    }

    /// Correlate a fresh source selected by the ordinary compiler with the
    /// exact staged move. The donor remains owned by the applied graph.
    pub(super) fn fresh_track_effect_move(
        &self,
        control: &mut PublicationControl,
        input: &ResidentEffectPreparation<'_>,
        desired: Option<&CanonicalAudioSourceSnapshot>,
        bindings: &[Binding],
        cancellation: Option<&RequestCancellation>,
    ) -> Result<Option<hosted_move::MoveRoute>, String> {
        let Some(route) = input.hosted_track_effect_move() else {
            return Ok(None);
        };
        if !control
            .status
            .playing
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Ok(None);
        }
        let predecessor = retained_move_predecessor(control, route.predecessor, input, desired)?;
        if control.status.native_effect_source() != Some(predecessor) {
            return Ok(None);
        }
        let device = route.device.value();
        let donor = ProcessorChainOwner::Track(route.donor.value());
        let receiver = ProcessorChainOwner::Track(route.receiver.value());
        let mut donors = self.sources.iter().filter(|source| {
            source.owner == donor
                && source.device == device
                && source.accepted
                && !source.retiring
                && source.project_generation == predecessor.context.project_generation
                && source.binding.root == predecessor.output_identity
        });
        let Some(original) = donors.next() else {
            return Ok(None);
        };
        if donors.next().is_some() {
            return Err("original vst3 move worker is ambiguous".into());
        }
        if original.editor_open
            || original.open_editor_group.is_some()
            || original.edit_backlog
            || original
                .edit_recovery
                .is_some_and(|read| read.recovery && read.source != predecessor)
            || self
                .latency_notice
                .is_some_and(|(installed, _)| installed == predecessor)
            || original.pending_touch.is_some()
        {
            return Ok(None);
        }
        let mut prepared = self.sources.iter().filter(|source| {
            source.owner == receiver
                && source.device == device
                && !source.accepted
                && !source.retiring
                && source.project_generation == input.context().project_generation
                && source.operation.edit_context() == Some(input.context())
                && source.binding.root == predecessor.output_identity
                && source.binding != original.binding
                && bindings.contains(&source.binding)
        });
        let Some(fresh) = prepared.next() else {
            return Ok(None);
        };
        if prepared.next().is_some() {
            return Err("fresh vst3 move worker is ambiguous".into());
        }
        let identity =
            NativeProcessorIdentity::new(receiver, device).map_err(|error| error.to_string())?;
        let binding = fresh.binding;
        let mut state = None;
        input.visit_plugin_devices(|destination, id, authored| {
            if !destination.is_instrument() && destination.owner() == receiver && id == device {
                state = Some(authored);
            }
            Ok(())
        })?;
        let state = state.ok_or("fresh vst3 effect receiver changed")?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(150);
        let supported = loop {
            check_cancel(cancellation)?;
            match control.with_native_effect_definition(&[binding], |definition| {
                Ok(definition.matches_track_effect_move_on_control(
                    identity,
                    binding,
                    state,
                    input.sample_rate(),
                ))
            }) {
                Err(reason)
                    if matches!(
                        reason.as_str(),
                        "native effect preparation is busy" | "native effect selection is busy"
                    ) && std::time::Instant::now() < deadline =>
                {
                    poll_control(control)
                }
                result => break result?,
            }
        };
        Ok(supported.then(|| {
            hosted_move::MoveRoute::prepare(
                input.context(),
                route.predecessor,
                HostedMoveRequest::Effect(HostedTrackEffectMove {
                    donor: route.donor,
                    receiver: route.receiver,
                    device: route.device,
                    binding,
                }),
                predecessor,
            )
        }))
    }

    pub(in crate::native_audio) fn received_foreign_program_on_control(
        control: &mut PublicationControl,
        binding: Binding,
        cancellation: Option<&RequestCancellation>,
    ) -> hex_engine::ForeignProgramFrontier {
        use hex_engine::ForeignProgramFrontier;
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(150);
        loop {
            if check_cancel(cancellation).is_err() {
                return ForeignProgramFrontier::Unknown;
            }
            match control.try_native_source_program_frontier_on_control(binding) {
                Ok(frontier) if frontier != ForeignProgramFrontier::Unknown => return frontier,
                Ok(ForeignProgramFrontier::Unknown)
                | Err("native preparation is busy" | "native source is busy")
                    if std::time::Instant::now() < deadline =>
                {
                    poll_control(control);
                }
                _ => return ForeignProgramFrontier::Unknown,
            }
        }
    }
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
            if source.operation.edit_context() == Some(context) {
                source.accepted = accepted && !source.retiring;
                source.editor_retirement.ever_accepted |= source.accepted;
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
                let successor = {
                    let old = &self.sources[index];
                    if !old.accepted
                        || old.retiring
                        || old.operation.edit_context() == Some(context)
                    {
                        None
                    } else {
                        self.sources
                            .iter()
                            .rev()
                            .find(|new| {
                                new.accepted
                                    && !new.retiring
                                    && new.project_generation == old.project_generation
                                    && new.device == old.device
                                    && new.operation.edit_context() == Some(context)
                            })
                            .map(|new| {
                                let predecessor_admitted_through = match new.operation {
                                    SourceOperation::Edit {
                                        predecessor: Some(cut),
                                        ..
                                    } if cut.binding == old.binding => Some(cut.admitted_through),
                                    _ => None,
                                };
                                RetiringSuccessor {
                                    binding: new.binding,
                                    context,
                                    predecessor_admitted_through,
                                }
                            })
                    }
                };
                if let Some(successor) = successor {
                    let old = &mut self.sources[index];
                    old.accepted = false;
                    old.retiring = true;
                    old.editor_retirement.successor = Some(successor);
                    // The original helper must detach without cutting an
                    // active gesture before its editor moves to the successor.
                    old.editor_transfer = old.editor_open;
                }
            }
        }
    }

    /// Validate the exact successor selected by the accepted transition. A
    /// later replacement must not inherit an earlier editor's pending input.
    /// Session must separately validate that its canonical context still matches.
    pub(super) fn retiring_successor(
        &self,
        index: usize,
    ) -> Result<RetiringSuccessor, &'static str> {
        let old = self
            .sources
            .get(index)
            .ok_or("vst3 retiring source changed")?;
        let successor = old
            .editor_retirement
            .successor
            .ok_or("vst3 editor replacement is unavailable")?;
        let provider = old
            .provider
            .ok_or("vst3 retiring editor provider is unavailable")?;
        self.sources
            .iter()
            .find(|source| {
                source.binding == successor.binding
                    && source.provider == Some(provider)
                    && successor.binding.root == old.binding.root
                    && source.accepted
                    && !source.retiring
                    && source.project_generation == old.project_generation
                    && source.device == old.device
                    && source.operation.edit_context() == Some(successor.context)
            })
            .ok_or("vst3 editor replacement changed before its edit was transferred")?;
        Ok(successor)
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
        let mut retain_instrument = destination.is_instrument()
            && (self.restore.is_some()
                || control
                    .status
                    .playing
                    .load(std::sync::atomic::Ordering::Acquire));
        let authored = requested_selection(input, device)?;
        let replay = self.retiring_replay_capture.as_ref().filter(|replay| {
            replay.publication == input.context()
                && replay.identity.owner() == owner
                && replay.identity.device_id() == device
        });
        let mut changed_state = false;
        let mut reintroduced = false;
        if input.preparing_edit()
            && let Some(desired) = desired.filter(|source| {
                source.context().project_generation == input.context().project_generation
            })
        {
            let previous = desired.effect_preparation();
            let retimed_effect = !destination.is_instrument()
                && control
                    .status
                    .playing
                    .load(std::sync::atomic::Ordering::Acquire)
                && input.retimes_existing_authored_effect(&previous, device);
            changed_state = !retimed_effect
                && !input.same_authored_plugin_program(
                    &previous,
                    device,
                    control.plugin_callback_quantum_on_control(),
                );
            reintroduced =
                self.catalog.is_some() && previous.plugin_device_bypassed(device).is_none();
            previous.visit_plugin_devices(|old_owner, old_device, old| {
                if old_device == device {
                    let same_processor = old_owner.owner() == owner && old.processing_eq(state);
                    changed_state |= !same_processor;
                    // Play can reach the callback after preparation has begun.
                    // A lane-only edit can keep the existing instrument in either
                    // transport state because its processor configuration is equal.
                    retain_instrument |=
                        destination.is_instrument() && old_owner == destination && same_processor;
                }
                Ok(())
            })?;
        }
        // The latest accepted preparation for an equal state is the current
        // device's source; older matching instances can still own old tails.
        for source in self.sources.iter().rev() {
            if source.project_generation != input.context().project_generation
                || source.owner != owner
                || source.device != device
                || !source_covers_selection(source.authored.as_ref(), authored.as_ref())
                || source.retiring
                || (input.preparing_edit()
                    && !source.accepted
                    && source.operation.edit_context() != Some(input.context()))
            {
                continue;
            }
            if let Some(replay) = replay {
                if replay.binding == source.binding && source.accepted {
                    return if replay.matches_candidate(control, state) {
                        Ok(Some(source.binding))
                    } else {
                        Err("vst3 replay candidate source or state changed".into())
                    };
                }
                continue;
            }
            if changed_state
                && !destination.is_instrument()
                && input.preparing_edit()
                && let Some(gesture) = self.parameter_gesture.as_ref()
                && gesture.sequenced_candidate_source(
                    control,
                    input.context(),
                    desired.map(|old| old.context()),
                    source.binding,
                    device,
                    state,
                    cancellation,
                )?
            {
                return Ok(Some(source.binding));
            }
            if (reintroduced || (changed_state && !retain_instrument))
                && source.operation.edit_context() != Some(input.context())
            {
                // A historical matching payload does not free its physical
                // endpoint from the actual outgoing tail. Prepare this edit's
                // own source instead of introducing a current/tail alias.
                continue;
            }
            // A fenced stopped restore may reuse the exact source whose
            // sequenced capture became canonical during a playing gesture.
            // The original prepared state can predate that capture.
            if ((retain_instrument || self.restored_canonical_source(input.context()).is_some())
                && captured_state_approved(control, source.binding, state, cancellation)?)
                || original_state(control, source.binding, cancellation)?.processing_eq(state)
            {
                return Ok(Some(source.binding));
            }
        }
        if replay.is_some() {
            return Err("vst3 replay candidate lost its original helper".into());
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
        fresh_source: bool,
        powering_on: bool,
    ) -> Result<(), String> {
        let context = input.context();
        let owner = destination.owner();
        let control = output.publication.control().ok_or("native output closed")?;
        self.extend_edit_targets(control, input, destination, device, state, cancellation)?;
        if !fresh_source
            && !powering_on
            && self
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
        let authored = requested_selection(input, device)?;
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
        let metadata;
        let catalog;
        let selection = if let Some(original) = original {
            metadata = loop {
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
            metadata.selection()
        } else {
            // History can reintroduce the last device from a provider whose
            // worker has already retired. Use the same exact lookup as Open.
            let previous = desired
                .filter(|source| source.context().project_generation == context.project_generation)
                .map(CanonicalAudioSourceSnapshot::effect_preparation);
            let reintroduced = powering_on
                || previous
                    .as_ref()
                    .is_some_and(|old| old.plugin_device_bypassed(device).is_none());
            let selected = if reintroduced {
                self.saved_catalog_selection(state, destination)
            } else {
                Err("existing vst3 source unavailable".into())
            };
            catalog = match selected {
                Ok(selected) => selected,
                Err(reason) => {
                    if powering_on {
                        return Err(reason);
                    }
                    let mut preserved = false;
                    if let Some(previous) = previous {
                        previous.visit_plugin_devices(|_, _, old| {
                            preserved |= old.processing_eq(state);
                            Ok(())
                        })?;
                    }
                    return if preserved {
                        // Unavailable devices still duplicate their authored state.
                        Ok(())
                    } else {
                        Err(format!(
                            "original vst3 source unavailable for the changed state · {reason}"
                        ))
                    };
                }
            };
            reopen::catalog_selection(&catalog)
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
        let provider = provider_identity(state)?;
        let predecessor = self
            .sources
            .iter()
            .rev()
            .find(|source| {
                source.project_generation == context.project_generation
                    && source.device == device
                    && source.accepted
                    && !source.retiring
            })
            .filter(|source| source.provider == Some(provider))
            .map(|source| source.binding);
        let predecessor = if let Some(binding) = predecessor {
            self.observe_editor_preparation_cut(output, desired, binding, cancellation)?
        } else {
            None
        };
        let control = output.publication.control().ok_or("native output closed")?;
        let binding = loop {
            check_cancel(cancellation)?;
            match self
                .host
                .as_ref()
                .ok_or("qualified vst3 helper unavailable")?
                .begin_on_control(
                    control,
                    EffectSelection { ..selection },
                    owner,
                    device,
                    context.editor_revision.max(1),
                    output.sample_rate,
                    hosting_profile(destination),
                    Some(state.clone()),
                    None,
                ) {
                Ok(binding) => break binding,
                Err(error) if error == "native preparation is busy" => poll_control(control),
                Err(error) => return Err(error),
            }
        };
        self.sources.push(Source {
            binding,
            owner,
            device,
            project_generation: context.project_generation,
            operation: SourceOperation::Edit {
                context,
                predecessor,
            },
            authored,
            accepted: false,
            retiring: false,
            queued: false,
            observes_latency: false,
            editor_open: false,
            editor_transfer: false,
            editor_open_on_insert: false,
            provider: None,
            edit_backlog: false,
            edit_recovery: None,
            open_editor_group: None,
            editor_retirement: Default::default(),
            pending_touch: None,
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
            self.poll(output, desired);
            if let Some(ready) = ready {
                return if ready.processing_eq(state) {
                    Ok(())
                } else {
                    Err("vst3 restored state differs from the edit".into())
                };
            }
        }
    }

    /// Reuse the exact original controller token. A source which cannot be
    /// observed has no ordering proof. Continue servicing existing controller
    /// clients while trying to admit, and drain an admitted token before
    /// honoring cancellation.
    fn observe_editor_preparation_cut(
        &mut self,
        output: &mut WorkerPreparedOutput,
        desired: Option<&CanonicalAudioSourceSnapshot>,
        binding: Binding,
        cancellation: Option<&RequestCancellation>,
    ) -> Result<Option<EditorPreparationCut>, String> {
        use hex_audio::plugin_worker::{ControllerRequest, NativeWorkerControllerProgress};
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(150);
        let request = loop {
            check_cancel(cancellation)?;
            self.poll(output, desired);
            let control = output.publication.control().ok_or("native output closed")?;
            match control.begin_native_source_controller_query_on_control(
                binding,
                ControllerRequest::ReadEditorFrontier,
            ) {
                Ok(request) => break request,
                Err(
                    "native preparation is busy"
                    | "native source is busy"
                    | "native controller mailbox is busy"
                    | "native controller query is already pending",
                ) if std::time::Instant::now() < deadline => {
                    poll_control(control);
                }
                Err(_) => return Ok(None),
            }
        };
        let control = output.publication.control().ok_or("native output closed")?;
        let cut = loop {
            match control.poll_native_source_controller_query_on_control(binding, &request) {
                Ok(NativeWorkerControllerProgress::EditorFrontier(admitted_through)) => {
                    break Some(EditorPreparationCut {
                        binding,
                        admitted_through,
                    });
                }
                Ok(NativeWorkerControllerProgress::Pending)
                | Err("native preparation is busy" | "native source is busy") => {
                    poll_control(control);
                }
                _ => break None,
            }
        };
        check_cancel(cancellation)?;
        Ok(cut)
    }
}
fn retained_move_predecessor(
    control: &PublicationControl,
    predecessor: hex_session::service::ResidentRuntimeLoop,
    input: &ResidentEffectPreparation<'_>,
    desired: Option<&CanonicalAudioSourceSnapshot>,
) -> Result<AppliedAudioSource, String> {
    control
        .status
        .recording_source()
        .filter(|installed| {
            installed.output_identity == predecessor.output_identity
                && installed.audio_revision == predecessor.audio_revision
                && Some(installed.context) == predecessor.source_context
                && desired.is_some_and(|desired| desired.context() == installed.context)
                && installed.context.project_generation == input.context().project_generation
                && installed.context.editor_revision.checked_add(1)
                    == Some(input.context().editor_revision)
        })
        .ok_or_else(|| "original vst3 move publication changed".into())
}

pub(super) fn check_cancel(cancellation: Option<&RequestCancellation>) -> Result<(), String> {
    if cancellation.is_some_and(RequestCancellation::preparation_expired) {
        Err("verified vst3 preparation timed out".into())
    } else if cancellation.is_some_and(RequestCancellation::is_cancelled) {
        Err("vst3 edit cancelled by document request or shutdown".into())
    } else {
        Ok(())
    }
}
pub(super) fn captured_state_approved(
    control: &mut PublicationControl,
    binding: Binding,
    state: &NativeDeviceState,
    cancellation: Option<&RequestCancellation>,
) -> Result<bool, String> {
    loop {
        check_cancel(cancellation)?;
        match control.try_native_source_approves_captured_state_on_control(binding, state) {
            Err("native preparation is busy" | "native source is busy") => poll_control(control),
            result => return result.map_err(str::to_owned),
        }
    }
}
pub(super) fn poll_control(control: &mut PublicationControl) {
    control.reclaim();
    control.poll_resident_publication();
    // Existing control-worker polling cadence. This is not a paint delay,
    // native ACK, timeout-based acceptance or an audio callback wait.
    std::thread::sleep(std::time::Duration::from_millis(5));
}
pub(super) fn source_metadata(
    control: &mut PublicationControl,
    binding: Binding,
    cancellation: Option<&RequestCancellation>,
) -> Result<hex_audio::plugin_effects::EffectSourceMetadata, String> {
    loop {
        check_cancel(cancellation)?;
        match control.native_source_metadata(binding) {
            Err(reason)
                if matches!(
                    reason.as_str(),
                    "native preparation is busy" | "native source is busy"
                ) =>
            {
                poll_control(control)
            }
            result => return result,
        }
    }
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

pub(super) fn requested_selection(
    input: &ResidentEffectPreparation<'_>,
    device: u64,
) -> Result<Option<AuthoredSelection>, String> {
    AuthoredSelection::from_targets_and_dense(
        input.authored_plugin_targets(device),
        input.modulated_plugin_target(device)?,
    )
    .map_err(str::to_owned)
}

pub(super) fn source_covers_selection(
    armed: Option<&AuthoredSelection>,
    requested: Option<&AuthoredSelection>,
) -> bool {
    armed.and_then(AuthoredSelection::dense_parameter)
        == requested.and_then(AuthoredSelection::dense_parameter)
        && armed.map_or(requested.is_none(), |armed| armed.covers(requested))
}

/// Root's complete validated target set, including live metadata extensions.
pub(super) fn armed_selection(
    control: &mut PublicationControl,
    binding: Binding,
    cancellation: Option<&RequestCancellation>,
) -> Result<Option<AuthoredSelection>, String> {
    let metadata = source_metadata(control, binding, cancellation)?;
    let class = metadata.selection().cid;
    AuthoredSelection::from_targets_and_dense(
        metadata
            .authored_base_values()
            .map(|(id, _)| (id, Some(class))),
        metadata.dense_parameter().map(|id| (id, class)),
    )
    .map_err(str::to_owned)
}

/// Consume the private query through disposal even after cancellation.
pub(super) fn canonical_bases(
    host: &Hosting,
    control: &mut PublicationControl,
    binding: Binding,
    state: &NativeDeviceState,
    selection: &AuthoredSelection,
    cancellation: Option<&RequestCancellation>,
    stopped: Option<(
        hex_audio::prepared::AppliedAudioSource,
        hex_audio::prepared::StoppedEditFence,
    )>,
) -> Result<hex_audio::plugin_effects::CapturedCanonicalBases, String> {
    let playing = control
        .status
        .playing
        .load(std::sync::atomic::Ordering::Acquire);
    let source = match stopped {
        Some((source, _)) => source,
        None => {
            control
                .status
                .native_parameter_source(playing)
                .ok_or("vst3 canonical source is not ready")?
                .0
        }
    };
    let mut query = loop {
        check_cancel(cancellation)?;
        let result = match stopped {
            Some((_, fence)) => host.begin_stopped_canonical_bases_capture_on_control(
                control, source, fence, binding, state, selection,
            ),
            None => host.begin_canonical_bases_capture_on_control(
                control, source, binding, state, selection, playing,
            ),
        };
        match result {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_reuse_requires_the_same_dense_reservation_independently_of_sparse_coverage() {
        let class = [1; 16];
        let ordinary = AuthoredSelection::from_targets([(0, Some(class)), (7, Some(class))])
            .unwrap()
            .unwrap();
        let selected =
            AuthoredSelection::from_targets_and_dense([(7, Some(class))], Some((0, class)))
                .unwrap()
                .unwrap();
        let same_dense = AuthoredSelection::from_targets_and_dense([], Some((0, class)))
            .unwrap()
            .unwrap();
        let changed_dense = AuthoredSelection::from_targets_and_dense([], Some((7, class)))
            .unwrap()
            .unwrap();
        assert!(source_covers_selection(Some(&ordinary), None));
        assert!(!source_covers_selection(Some(&ordinary), Some(&same_dense)));
        assert!(!source_covers_selection(Some(&selected), None));
        assert!(source_covers_selection(Some(&selected), Some(&same_dense)));
        assert!(!source_covers_selection(
            Some(&selected),
            Some(&changed_dense)
        ));
    }
}
