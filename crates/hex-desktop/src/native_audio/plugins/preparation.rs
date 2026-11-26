//! Insertion/edit source preparation, graph compilation, and shared native
//! preparation state-machine helpers for `Plugins`.
use super::*;

impl Plugins {
    pub fn ready(&self) -> bool {
        // Session queries this only for its held insertion. No Root binding yet
        // means a contended admission must retry, not wait for nonexistent facts.
        self.pending
            .as_ref()
            .is_none_or(|p| p.result.is_some() || p.facts_ready_for_session)
    }

    pub fn prepare(
        &mut self,
        output: &mut WorkerPreparedOutput,
        input: PluginEffectPreparation<'_>,
        desired: Option<&CanonicalAudioSourceSnapshot>,
    ) -> Result<PluginEffectPreparationState, String> {
        if let Some(pending) = self.pending.as_mut() {
            if !Arc::ptr_eq(&pending.operation, &input.insertion.control)
                || pending.context != input.insertion.context
            {
                return Err("another vst3 source operation is pending".into());
            }
            pending.canonical_context = input.canonical_context();
        } else {
            if self.restore.is_some() || self.reopen.is_some() {
                return Err("vst3 restoration is pending".into());
            }
            if !self.configured {
                return Err(self
                    .unavailable
                    .clone()
                    .unwrap_or_else(|| "vst3 hosting unavailable for this output".into()));
            }
            if input.insertion.control.is_cancelled() {
                return Err("vst3 insertion cancelled".into());
            }
            let control = output.publication.control().ok_or("native output closed")?;
            if self.sources.len() >= self.sources.capacity() {
                self.reclaim_retired_sources(control, desired, true);
                if self.sources.len() >= self.sources.capacity() {
                    if self.has_reclaimable_retiring_source() {
                        edits::poll_control(control);
                        return Ok(PluginEffectPreparationState::Preparing);
                    }
                    return Err(
                        "vst3 source/history capacity is full · close the project before loading more"
                            .into(),
                    );
                }
            }
            let selection = &input.insertion.selection;
            let owner = input.insertion.destination.owner();
            let graph = input
                .insertion
                .context
                .editor_revision
                .checked_add(1)
                .ok_or("vst3 graph revision exhausted")?;
            let mut timing = self
                .diagnostics
                .as_ref()
                .map(|_| (std::time::Instant::now(), EffectStartupTimings::default()));
            let begin = self
                .host
                .as_ref()
                .ok_or("qualified vst3 helper unavailable")?
                .begin_on_control(
                    control,
                    EffectSelection {
                        module: &selection.module,
                        executable: &selection.executable,
                        cid: selection.cid,
                        executable_sha256: &selection.executable_sha256,
                        info_sha256: &selection.info_sha256,
                    },
                    owner,
                    input.device_id,
                    graph,
                    output.sample_rate,
                    hosting_profile(input.insertion.destination),
                    None,
                    timing.as_mut().map(|(_, timings)| timings),
                );
            let binding = match begin {
                Ok(binding) => binding,
                // Root's contended Work mutex refuses before reserving a slot
                // or native owner. Retain the existing ordered request and retry
                // through its normal continuation, as project reopen does.
                Err(reason) if reason == "native preparation is busy" => {
                    edits::poll_control(control);
                    return Ok(PluginEffectPreparationState::Preparing);
                }
                Err(reason) if reason == "native preparation cells are full" => {
                    let reclaimed = self.reclaim_retired_sources(control, desired, true);
                    if reclaimed > 0 || self.has_reclaimable_retiring_source() {
                        edits::poll_control(control);
                        return Ok(PluginEffectPreparationState::Preparing);
                    }
                    return Err(reason);
                }
                Err(reason) => return Err(reason),
            };
            if let Some((started, timing)) = timing {
                record_insertion_stage(
                    self.diagnostics.as_ref(),
                    binding,
                    input.insertion.context.project_generation,
                    "native_prepare_returned",
                    format_args!(
                        " elapsed_ns={} selection_inspection_ns={} revalidation_ns={} waitable_probe_ns={:?} instance_to_facts_ns={:?}",
                        started.elapsed().as_nanos(),
                        timing.selection_inspection.as_nanos(),
                        timing.revalidation.as_nanos(),
                        timing.waitable_probe.map(|elapsed| elapsed.as_nanos()),
                        timing.instance_to_facts.map(|elapsed| elapsed.as_nanos()),
                    ),
                );
            }
            #[cfg(all(test, feature = "allocation-probe"))]
            if let Some(gate) = self.facts_gate.as_mut() {
                gate.binding = Some(binding);
            }
            self.sources.push(Source {
                binding,
                owner,
                device: input.device_id,
                project_generation: input.insertion.context.project_generation,
                operation: SourceOperation::Insertion(Arc::clone(&input.insertion.control)),
                authored: None,
                accepted: false,
                retiring: false,
                queued: false,
                observes_latency: false,
                editor_open: false,
                editor_transfer: false,
                editor_open_on_insert: false,
                edit_backlog: false,
                edit_recovery: None,
                open_editor_group: None,
                provider: None,
                editor_retirement: Default::default(),
                pending_touch: None,
                #[cfg(all(test, feature = "allocation-probe"))]
                observed_latency_receipt: false,
                #[cfg(all(test, feature = "allocation-probe"))]
                attempts: 0,
            });
            self.pending = Some(Pending {
                operation: Arc::clone(&input.insertion.control),
                binding,
                context: input.insertion.context,
                canonical_context: input.canonical_context(),
                result: None,
                facts_ready_for_session: false,
                facts_observed: false,
                #[cfg(all(test, feature = "allocation-probe"))]
                facts_attempt_from_session: false,
            });
        }
        self.poll_for_context(output, Some(input.canonical_context()), None, Some(&input));
        match self.pending.as_ref().and_then(|p| p.result.as_ref()) {
            Some(Ok(state)) => Ok(PluginEffectPreparationState::Ready(state.clone())),
            Some(Err(error)) => Err(error.clone()),
            None => Ok(PluginEffectPreparationState::Preparing),
        }
    }

    /// The ready hosted operation alone may bind the successor's receipt.
    pub(in crate::native_audio) fn device_insert_control(
        &self,
        context: AudioSourceContext,
    ) -> Option<Arc<PluginEffectControl>> {
        let pending = self.pending.as_ref()?;
        (pending.canonical_context.project_generation == context.project_generation
            && pending.canonical_context.editor_revision.checked_add(1)
                == Some(context.editor_revision)
            && matches!(pending.result, Some(Ok(_))))
        .then(|| Arc::clone(&pending.operation))
    }

    /// Select installed sources and workers prepared for this exact proposal
    /// or private verified graph edit. Root still validates the actual state.
    pub(in crate::native_audio) fn verified_effect_bindings(
        &self,
        input: ResidentEffectPreparation<'_>,
        proposal: Option<&Arc<PluginEffectControl>>,
        staged_sources: bool,
    ) -> Result<Vec<Binding>, String> {
        let context = input.context();
        let staged = staged_sources && self.edit == Some(context);
        let pending = self.pending.as_ref().filter(|pending| {
            proposal.is_some_and(|proposal| Arc::ptr_eq(proposal, &pending.operation))
                && pending.canonical_context.project_generation
                    == input.context().project_generation
                && pending.canonical_context.editor_revision.checked_add(1)
                    == Some(input.context().editor_revision)
                && matches!(pending.result, Some(Ok(_)))
                && !pending.operation.is_cancelled()
        });
        let mut bindings = Vec::new();
        input.visit_plugin_devices(|destination, device, _| {
            let authored = edits::requested_selection(&input, device)?;
            let source = self
                .sources
                .iter()
                .rev()
                .find(|source| {
                    source.project_generation == input.context().project_generation
                        && source.owner == destination.owner()
                        && source.device == device
                        && !source.retiring
                        && (source.accepted
                            || pending.is_some_and(|pending| source.binding == pending.binding)
                            || (staged && source.operation.edit_context() == Some(context)))
                        && edits::source_covers_selection(
                            source.authored.as_ref(),
                            authored.as_ref(),
                        )
                })
                .ok_or("verified graph has no exact prepared vst3 source")?;
            bindings
                .try_reserve(1)
                .map_err(|_| "verified vst3 bindings allocation refused")?;
            bindings.push(source.binding);
            Ok(())
        })?;
        Ok(bindings)
    }

    pub fn finish(&mut self, operation: &Arc<PluginEffectControl>, accepted: bool) {
        if self
            .pending
            .as_ref()
            .is_none_or(|p| !Arc::ptr_eq(&p.operation, operation))
        {
            return;
        }
        let pending = self.pending.take();
        #[cfg(all(test, feature = "allocation-probe"))]
        if self.facts_gate.as_ref().and_then(|gate| gate.binding)
            == pending.as_ref().map(|pending| pending.binding)
        {
            self.facts_gate = None;
        }
        if let Some(accepted_index) = self.sources.iter().position(|s| {
            s.operation
                .insertion()
                .is_some_and(|current| Arc::ptr_eq(current, operation))
        }) {
            let source = &mut self.sources[accepted_index];
            source.accepted = accepted
                && pending
                    .as_ref()
                    .is_some_and(|p| matches!(p.result, Some(Ok(_))));
            source.editor_retirement.ever_accepted |= source.accepted;
            source.retiring |= !source.accepted;
            source.editor_open_on_insert = source.accepted && operation.opens_editor();
            let accepted_now = source.accepted;
            let accepted_binding = source.binding;
            let accepted_generation = source.project_generation;
            let accepted_device = source.device;
            record_insertion_stage(
                self.diagnostics.as_ref(),
                accepted_binding,
                accepted_generation,
                "graph_terminal",
                format_args!(" committed={accepted_now}"),
            );
            if accepted_now {
                self.installed_removals_checked = None;
                for prior in &mut self.sources {
                    if prior.binding != accepted_binding
                        && prior.accepted
                        && !prior.retiring
                        && prior.project_generation == accepted_generation
                        && prior.device == accepted_device
                    {
                        prior.accepted = false;
                        prior.retiring = true;
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)] // Publication context selects fresh versus retained sources.
    pub(in crate::native_audio) fn prepare_edit_sources(
        &mut self,
        mut output: Option<&mut WorkerPreparedOutput>,
        input: &ResidentEffectPreparation<'_>,
        desired: Option<&CanonicalAudioSourceSnapshot>,
        cancellation: Option<&hex_session::service::RequestCancellation>,
        fresh_hosted_sources: bool,
        power_on: Option<NativeProcessorIdentity>,
    ) -> Result<(), String> {
        let previous = desired
            .filter(|_| fresh_hosted_sources)
            .map(CanonicalAudioSourceSnapshot::effect_preparation);
        let reload_device = input.reload_plugin_device();
        input.visit_plugin_devices(|destination, device, state| {
            // History may carry the identity and state of a deleted device.
            // The predecessor's membership decides whether DSP can survive.
            let reloading = reload_device == Some(device);
            let powering_on = reloading
                || power_on.is_some_and(|identity| {
                    identity.owner() == destination.owner() && identity.device_id() == device
                });
            let fresh_source = reloading
                || previous
                    .as_ref()
                    .is_some_and(|source| source.plugin_device_bypassed(device).is_none());
            self.prepare_edit_source(
                output.as_deref_mut().ok_or("native output closed")?,
                input,
                destination,
                device,
                state,
                desired,
                cancellation,
                fresh_source,
                powering_on,
            )
            .map_err(bounded_error)
        })
    }

    #[allow(clippy::too_many_arguments)] // Exact compile-time source and bypass transition inputs.
    pub(super) fn refuse_unavailable_fallback(
        &self,
        has_control: bool,
        context: AudioSourceContext,
        owner: ProcessorChainOwner,
        device: u64,
        preparing_edit: bool,
        next_bypassed: Option<bool>,
        previously_powered_off: Option<NativeProcessorIdentity>,
        previous_bypassed: Option<bool>,
    ) -> bool {
        (has_control && self.has_accepted_effect_source(context, owner, device))
            || Self::re_enabling_without_worker(
                preparing_edit,
                owner,
                device,
                next_bypassed,
                previously_powered_off,
                previous_bypassed,
            )
    }

    pub(super) fn re_enabling_without_worker(
        preparing_edit: bool,
        owner: ProcessorChainOwner,
        device: u64,
        next_bypassed: Option<bool>,
        previously_powered_off: Option<NativeProcessorIdentity>,
        previous_bypassed: Option<bool>,
    ) -> bool {
        preparing_edit
            && next_bypassed == Some(false)
            && (previously_powered_off.is_some_and(|identity| {
                identity.owner() == owner && identity.device_id() == device
            }) || previous_bypassed == Some(true))
    }

    pub(super) fn effective_powering_on(
        &self,
        context: AudioSourceContext,
        preparing_edit: bool,
        previously_powered_off: Option<NativeProcessorIdentity>,
        explicit_power_on: Option<(ProcessorChainOwner, u64)>,
        device_bypassed: impl FnOnce(u64) -> Option<bool>,
    ) -> Option<NativeProcessorIdentity> {
        previously_powered_off.filter(|identity| {
            explicit_power_on.is_some_and(|(owner, device)| {
                identity.owner() == owner && identity.device_id() == device
            }) || (preparing_edit
                && device_bypassed(identity.device_id()) == Some(false)
                && !self.has_accepted_effect_source(
                    context,
                    identity.owner(),
                    identity.device_id(),
                ))
        })
    }

    pub fn compile(
        &mut self,
        output: Option<&mut WorkerPreparedOutput>,
        input: ResidentEffectPreparation<'_>,
        desired: Option<&CanonicalAudioSourceSnapshot>,
        cancellation: Option<&hex_session::service::RequestCancellation>,
        powered_off: Option<NativeProcessorIdentity>,
        prepare: &mut dyn FnMut(
            &hex_engine::AudioCandidateEffects<'_, '_>,
        ) -> Result<PreparedRevision, hex_engine::Invalid>,
    ) -> Result<Option<PreparedRevision>, String> {
        if self.reopen.as_ref().is_some_and(|restore| {
            restore.context.project_generation == input.context().project_generation
                && (!restore.ready
                    || restore.context != input.context()
                    || !input.restoring_plugin_state())
        }) {
            return Err("vst3 state restoration is pending".into());
        }
        if input.running_power_off().is_some() {
            return self
                .prepare_running_power_off(output, &input, prepare)
                .map(Some);
        }
        // Document preparation precedes source_changed/request_reopen. Its
        // next-generation declaration can select resource-free Off directly;
        // this grants no SDK source or fresh-On permission.
        let incoming_document = off_document_preparation(
            input.preparing_edit(),
            input.context(),
            desired.map(|source| source.context()),
        );
        let saved_off = |owner, device| {
            self.saved_off_document
                .is_some_and(|(generation, identity)| {
                    generation == input.context().project_generation
                        && identity.owner() == owner
                        && identity.device_id() == device
                })
        };
        let main_off = powered_off
            .filter(|identity| identity.owner() == ProcessorChainOwner::Main)
            .map(NativeProcessorIdentity::device_id);
        if let Some((device, state)) = input.powered_off_main()
            && (incoming_document
                || saved_off(ProcessorChainOwner::Main, device)
                || main_off == Some(device))
            && !self.has_accepted_main_source(input.context(), device)
        {
            return Hosting::prepare_powered_off_on_control(
                hex_audio::plugin_effects::PoweredOffEffect {
                    owner: ProcessorChainOwner::Main,
                    device_id: device,
                    sample_rate: input.sample_rate(),
                    state,
                },
                prepare,
            )
            .map(Some);
        }
        if let Some((owner, device, state)) = input.powered_off_return()
            && (powered_off.is_some_and(|off| off.owner() == owner && off.device_id() == device)
                || ((incoming_document || saved_off(owner, device))
                    && !self.has_accepted_effect_source(input.context(), owner, device)))
        {
            return Hosting::prepare_powered_off_on_control(
                hex_audio::plugin_effects::PoweredOffEffect {
                    owner,
                    device_id: device,
                    sample_rate: input.sample_rate(),
                    state,
                },
                prepare,
            )
            .map(Some);
        }
        let previously_powered_off = powered_off.or_else(|| {
            self.saved_off_document
                .filter(|(generation, _)| *generation == input.context().project_generation)
                .map(|(_, identity)| identity)
        });
        let powering_on = self.effective_powering_on(
            input.context(),
            input.preparing_edit(),
            previously_powered_off,
            input
                .power_on()
                .map(|intent| (intent.owner(), intent.device())),
            |device| input.plugin_device_bypassed(device),
        );
        let mut output = output;
        let mut reorder = None;
        if input.preparing_edit() {
            if let Some(output) = output.as_deref_mut() {
                reorder =
                    self.prepare_hosted_effect_reorder(output, &input, desired, cancellation)?;
            }
            if reorder.is_none() {
                self.prepare_edit_sources(
                    output.as_deref_mut(),
                    &input,
                    desired,
                    cancellation,
                    false,
                    powering_on,
                )?;
            }
        }
        let mut bindings = Vec::new();
        bindings
            .try_reserve_exact(self.sources.len())
            .map_err(|_| "vst3 compiler selection unavailable")?;
        let context = input.context();
        let mut unavailable = Vec::new();
        input.visit_plugin_devices(|destination, device, state| {
            let owner = destination.owner();
            if let Some((_, binding)) = reorder
                && input.hosted_track_effect_reorder().is_some_and(|route| {
                    destination == PluginEffectDestination::Track(route.track)
                        && device == route.device.value()
                })
            {
                bindings.push(binding);
                return Ok(());
            }
            let (has_control, degraded_device, source) = match output
                .as_deref_mut()
                .and_then(|output| output.publication.control())
            {
                Some(control) => {
                    let degraded = input.reload_plugin_device() != Some(device)
                        && self.is_device_degraded_on_control(
                            control,
                            context.project_generation,
                            device,
                        );
                    let matched = self.matching_edit_source(
                        control,
                        &input,
                        destination,
                        device,
                        state,
                        desired,
                        cancellation,
                    )?;
                    (true, degraded, matched)
                }
                None => (false, false, None),
            };
            if let Some(binding) = source {
                bindings.push(binding);
                return Ok(());
            }
            let previous_bypassed = desired
                .filter(|source| source.context().project_generation == context.project_generation)
                .and_then(|source| source.effect_preparation().plugin_device_bypassed(device));
            if !degraded_device
                && self.refuse_unavailable_fallback(
                    has_control,
                    context,
                    owner,
                    device,
                    input.preparing_edit(),
                    input.plugin_device_bypassed(device),
                    previously_powered_off,
                    previous_bypassed,
                )
            {
                return Err("original vst3 source unavailable for this device".into());
            }
            // A staged document can preserve unresolved state, as can later
            // edits retaining that same canonical device. A degraded device also
            // falls back to an isolated pass-through placeholder on unrelated
            // edits until explicitly reloaded. This is never an admission path
            // for a new unsupported same-document effect edit.
            let mut preserved = degraded_device
                || desired.is_some_and(|source| {
                    source.context().project_generation.checked_add(1)
                        == Some(context.project_generation)
                });
            if let Some(source) = desired
                .filter(|source| source.context().project_generation == context.project_generation)
            {
                source.effect_preparation().visit_plugin_devices(
                    |old_destination, old_device, old_state| {
                        preserved |= old_destination == destination
                            && old_device == device
                            && old_state.processing_eq(state);
                        preserved |= input.preparing_edit() && old_state.processing_eq(state);
                        Ok(())
                    },
                )?;
            }
            if !preserved {
                return Err("original vst3 source unavailable for this device".into());
            }
            unavailable
                .try_reserve(1)
                .map_err(|_| "unavailable vst3 metadata allocation refused")?;
            unavailable.push(hex_audio::plugin_effects::UnavailableEffect {
                owner,
                device_id: device,
                state,
            });
            Ok(())
        })?;
        if bindings.is_empty() && unavailable.is_empty() {
            return Ok(None);
        }
        if bindings.is_empty() {
            return Hosting::prepare_unavailable_on_control(&unavailable, prepare).map(Some);
        }
        let control = output
            .and_then(|output| output.publication.control())
            .ok_or("native output closed")?;
        let movement = if let Some((route, _)) = reorder {
            Some(route)
        } else {
            match self.fresh_instrument_move(control, &input, desired, &bindings)? {
                Some(route) => Some(route),
                None => {
                    self.fresh_track_effect_move(control, &input, desired, &bindings, cancellation)?
                }
            }
        };
        let mut canonical = Vec::new();
        canonical
            .try_reserve_exact(bindings.len())
            .map_err(|_| "vst3 canonical facts capacity unavailable")?;
        input.visit_plugin_devices(|destination, device, state| {
            let Some(source) = self.sources.iter().rev().find(|source| {
                bindings.contains(&source.binding)
                    && source.owner == destination.owner()
                    && source.device == device
            }) else {
                return Ok(());
            };
            if !edits::original_state(control, source.binding, cancellation)?.processing_eq(state) {
                let selection = match edits::armed_selection(control, source.binding, cancellation)?
                {
                    Some(selection) => selection,
                    None => AuthoredSelection::for_no_parameters(
                        Hosting::saved_identity_on_control(state)?.class.bytes(),
                    )?,
                };
                canonical.push(edits::canonical_bases(
                    self.host
                        .as_ref()
                        .ok_or("qualified vst3 helper unavailable")?,
                    control,
                    source.binding,
                    state,
                    &selection,
                    cancellation,
                    self.restored_canonical_source(input.context()),
                )?);
            }
            Ok(())
        })?;
        for &binding in &bindings {
            let revision = loop {
                edits::check_cancel(cancellation)?;
                match control.reserve_native_source_program_revision_on_control(binding) {
                    Err(reason)
                        if reason == "native preparation is busy"
                            || reason == "native source is busy" =>
                    {
                        edits::poll_control(control)
                    }
                    result => break result?,
                }
            };
            if let Some(gesture) = self
                .parameter_gesture
                .as_mut()
                .filter(|gesture| gesture.binding == binding && gesture.commit.is_some())
            {
                gesture.next_program = Some(revision);
            }
        }
        loop {
            if movement
                .is_some_and(|route| control.status.recording_source() != Some(route.applied))
            {
                return Err("original vst3 move source is no longer installed".into());
            }
            match Hosting::prepare_with_canonical_bases_on_control(
                control,
                &bindings,
                &unavailable,
                &canonical,
                None,
                prepare,
            ) {
                Err(error)
                    if input.preparing_edit()
                        && matches!(
                            error.as_str(),
                            "native effect preparation is busy" | "native effect selection is busy"
                        ) =>
                {
                    // Root has not borrowed/called the compiler while its Work
                    // or bank is contended by original-source reclamation.
                    edits::check_cancel(cancellation)?;
                    edits::poll_control(control);
                }
                Ok(prepared) => {
                    if let Some(route) = movement {
                        self.hosted_move = Some(route);
                    }
                    return Ok(Some(prepared));
                }
                Err(error) => return Err(error),
            }
        }
    }
}

pub(super) fn hosting_profile(destination: PluginEffectDestination) -> PluginProfile {
    if destination.is_instrument() {
        PluginProfile::Instrument
    } else {
        PluginProfile::Effect
    }
}

/// Bounded worker-side correlation for one native insertion. The binding
/// identifies the output, planned device, successor revision and lease.
pub(super) fn record_insertion_stage(
    log: Option<&hex_diagnostics::DiagnosticsHandle>,
    binding: Binding,
    generation: u64,
    stage: &'static str,
    detail: std::fmt::Arguments<'_>,
) {
    if let Some(log) = log {
        let _ = log.record_typed(
            hex_diagnostics::Level::Debug,
            hex_diagnostics::Category::Audio,
            &format!(
                "vst3_insert v=1 stage={stage} generation={generation} binding={binding:?}{detail}"
            ),
        );
    }
}

pub(super) fn bounded_error(error: String) -> String {
    if error.capacity() <= 8192 {
        return error;
    }
    // Root retains the original complete diagnostic; UI correlation keeps only
    // a bounded explanation while its actual owner reaches disposal/recovery.
    error.chars().take(1024).collect()
}

/// The same original preparation and endpoint ACK protocol serves insertion and
/// stopped renewal. Returning captured state never admits a canonical graph.
pub(super) fn advance_source(
    host: &Hosting,
    presentation: Option<EffectPresentation>,
    control: &mut hex_audio::prepared::PublicationControl,
    source: &mut Source,
) -> Result<Option<NativeDeviceState>, String> {
    let progress = control.native_source_preparation_progress(source.binding);
    advance_source_at_progress(
        host,
        fixture_presentation_plan(presentation),
        control,
        source,
        progress,
    )
}

pub(super) fn fixture_presentation_plan(
    presentation: Option<EffectPresentation>,
) -> EffectPresentationPlan {
    presentation.map_or(
        EffectPresentationPlan::AllowUnknown,
        EffectPresentation::plan,
    )
}

pub(super) fn advance_source_at_progress(
    host: &Hosting,
    presentation: EffectPresentationPlan,
    control: &mut hex_audio::prepared::PublicationControl,
    source: &mut Source,
    progress: Option<Progress>,
) -> Result<Option<NativeDeviceState>, String> {
    match progress {
        Some(Progress::Facts(_)) => {
            match host.complete_on_control(control, source.binding, presentation) {
                Ok(()) => {}
                // Root has not taken or changed this source when Work is busy.
                // Retain its exact operation and re-poll through the existing
                // worker loop; contention is neither native ACK nor refusal.
                Err(reason) if reason == "native preparation is busy" => return Ok(None),
                Err(reason) => return Err(reason),
            }
        }
        Some(Progress::Prepared { .. }) => match control
            .start_native_effect_with_authored_selection(source.binding, source.authored.as_ref())
        {
            Ok(()) => {}
            Err("native preparation is busy") => return Ok(None),
            Err(reason) => return Err(reason.to_owned()),
        },
        Some(Progress::Effect { .. }) => {
            if source.editor_retirement.lease.is_none() {
                match control.reserve_native_retiring_edits_on_control(source.binding) {
                    Ok(lease) => source.editor_retirement.lease = Some(lease),
                    Err("native preparation is busy") => return Ok(None),
                    Err(reason) => return Err(reason.to_owned()),
                }
            }
            if let Some((loaned, live)) = control.native_source_endpoint_status(source.binding)
                && (!live || (source.queued && !loaned))
            {
                return Err("vst3 endpoint returned or stopped before publication".into());
            }
            if !source.queued {
                match control.publish_native_source_endpoint(source.binding) {
                    Ok(()) => source.queued = true,
                    Err("native endpoint publication is busy" | "native preparation is busy") => {
                        return Ok(None);
                    }
                    Err(reason) => return Err(reason.to_owned()),
                }
            }
            if control.native_source_endpoint_installed(source.binding) == Some(true) {
                let Some(state) = control.native_source_prepared_state(source.binding) else {
                    return Ok(None);
                };
                source.provider = Some(provider_identity(&state)?);
                return Ok(Some(state));
            }
        }
        Some(Progress::Failed | Progress::Complete | Progress::Retiring) => {
            return match control.try_native_source_preparation_error(source.binding) {
                Ok(Some(reason)) => Err(reason),
                Ok(None) => Err("vst3 preparation refused".into()),
                Err("native preparation is busy") => Ok(None),
                Err(reason) => Err(reason.to_owned()),
            };
        }
        Some(Progress::Preparing) | None => {}
    }
    Ok(None)
}

/// Pure document-generation eligibility, never source or SDK authority.
pub(super) fn off_document_preparation(
    preparing_edit: bool,
    candidate: AudioSourceContext,
    previous: Option<AudioSourceContext>,
) -> bool {
    !preparing_edit
        && match previous {
            Some(previous) => {
                previous.project_generation.checked_add(1) == Some(candidate.project_generation)
            }
            None => candidate.project_generation == 0 && candidate.editor_revision == 0,
        }
}
