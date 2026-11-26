//! Worker polling, diagnostics observation, and retired-source reclamation
//! for `Plugins`.
use super::*;

impl Plugins {
    pub fn poll(
        &mut self,
        output: &mut WorkerPreparedOutput,
        desired: Option<&CanonicalAudioSourceSnapshot>,
    ) {
        self.poll_for_context(
            output,
            desired.map(CanonicalAudioSourceSnapshot::context),
            desired,
            None,
        );
    }

    pub(super) fn poll_for_context(
        &mut self,
        output: &mut WorkerPreparedOutput,
        context: Option<AudioSourceContext>,
        desired: Option<&CanonicalAudioSourceSnapshot>,
        insertion: Option<&PluginEffectPreparation<'_>>,
    ) {
        // Preparation pumps this same loop while a successor is starting.
        // Retiring sources must make progress even during rapid history edits.
        self.poll_retiring_edits();
        let Some(control) = output.publication.control() else {
            self.output_closed();
            return;
        };
        if let Some(desired) = desired {
            self.retire_installed_removals(control, desired);
        }
        self.poll_editor_close(control, context.map(|context| context.project_generation));
        self.poll_editor(control, context, desired);
        self.poll_parameters(control, context);
        self.poll_parameter_conversion(control, context);
        self.poll_parameter_override(control, context);
        self.poll_restore(control, context);
        if let Some(output) = control.output_identity()
            && let Some(context) = context
            && !self
                .restore
                .as_ref()
                .is_some_and(restore::Restore::is_returning_epoch)
        {
            let generation = context.project_generation;
            let active_devices: Option<Vec<u64>> = desired
                .filter(|snapshot| snapshot.context().project_generation == generation)
                .map(|snapshot| {
                    let prep = snapshot.effect_preparation();
                    let mut active = Vec::new();
                    let _ = prep.visit_plugin_devices(|_, id, _| {
                        if prep.plugin_device_bypassed(id) == Some(false) {
                            active.push(id);
                        }
                        Ok(())
                    });
                    active
                });
            let endpoints = self
                .sources
                .iter_mut()
                .filter(|source| source.project_generation == generation)
                .map(|source| {
                    let status = control.native_source_endpoint_status(source.binding);
                    if let Some(log) = &self.diagnostics
                        && let Some(sample) = control.take_native_source_slow_exchange(source.binding)
                    {
                        let pickup_interval = sample
                            .empty_poll_to_pickup_ns
                            .map_or_else(|| "unavailable".to_owned(), |ns| ns.to_string());
                        let _ = log.record_typed(
                            hex_diagnostics::Level::Warn,
                            hex_diagnostics::Category::Audio,
                            &format!(
                                "vst3 v=1 stage=slow_exchange generation={generation} output={output} device={} binding={:?} sequence={} exchange_ns={} sdk_process_ns={} quantum_ns={} dropped={} queue_ns=unavailable empty_poll_to_pickup_ns={pickup_interval}",
                                source.device, sample.binding, sample.sequence, sample.exchange_ns,
                                sample.sdk_process_ns, sample.quantum_ns, sample.dropped
                            ),
                        );
                    }
                    if let Some(log) = &self.diagnostics
                        && let Some(sample) = control.take_native_source_concealed_output(source.binding)
                    {
                        let _ = log.record_typed(
                            hex_diagnostics::Level::Warn,
                            hex_diagnostics::Category::Audio,
                            &format!(
                                "vst3 v=1 stage=concealed_output generation={generation} output={output} device={} binding={:?} reply_sequence={} worker_phase={:?} overflowed={}",
                                source.device, sample.binding, sample.miss.reply_sequence,
                                sample.miss.worker_phase, sample.overflowed
                            ),
                        );
                    }
                    let mut failure_diagnostic = None;
                    if status == Some((true, false)) {
                        source.edit_backlog = false;
                        if let Some(diagnostic) =
                            control.take_native_source_failure_diagnostic(source.binding)
                        {
                            #[cfg(all(test, feature = "allocation-probe"))]
                            if let Some(observation) = &self.failure_observation
                                && let Ok(mut observed) = observation.lock()
                            {
                                *observed = Some(diagnostic.clone());
                            }
                            if let Some(log) = &self.diagnostics {
                                let _ = log.record_typed(
                                    hex_diagnostics::Level::Warn,
                                    hex_diagnostics::Category::Audio,
                                    &format!(
                                        "vst3_editor v=1 stage=worker_failure generation={generation} output={output} device={} {diagnostic}",
                                        source.device
                                    ),
                                );
                            }
                            failure_diagnostic = Some(diagnostic);
                        }
                    }
                    (source.binding, source.device, status, failure_diagnostic)
                });
            if let Some(entries) =
                self.degraded
                    .observe(output, endpoints, active_devices.as_deref())
            {
                self.degraded_changed = Some((generation, output, entries));
            }
        }
        #[cfg(all(test, feature = "allocation-probe"))]
        if let Some(observed) = self.completed.as_ref() {
            let count = self
                .sources
                .iter()
                .filter(|s| s.accepted)
                .filter_map(|s| control.native_source_completed_blocks(s.binding))
                .min();
            if let Some(count) = count {
                observed.store(count, std::sync::atomic::Ordering::Release);
            }
        }
        if let Some(pending) = self.pending.as_mut()
            && pending.result.is_none()
        {
            if pending.operation.is_cancelled()
                || context.is_none_or(|current| {
                    current.project_generation != pending.context.project_generation
                })
            {
                pending.result = Some(Err("vst3 insertion cancelled · source changed".into()));
            } else if let Some(source) = self
                .sources
                .iter_mut()
                .find(|s| s.binding == pending.binding)
            {
                // Sample the progress once. Re-reading it after the continuation
                // check could advance newly arrived Facts during background poll.
                let progress = control.native_source_preparation_progress(source.binding);
                let at_facts = matches!(progress, Some(Progress::Facts(_)));
                if let Some(Progress::Facts(facts)) = progress
                    && !pending.facts_observed
                {
                    pending.facts_observed = true;
                    record_insertion_stage(
                        self.diagnostics.as_ref(),
                        source.binding,
                        source.project_generation,
                        "native_facts",
                        format_args!(
                            " tuning_channels={:#06x} brightness_channels={:#06x} unreadable_expression_channels={:#06x}",
                            facts.note_expression.tuning_channels,
                            facts.note_expression.brightness_channels,
                            facts.note_expression.unreadable_channels,
                        ),
                    );
                }
                #[cfg(all(test, feature = "allocation-probe"))]
                let held = if at_facts {
                    let held = self
                        .facts_gate
                        .as_mut()
                        .is_some_and(|gate| gate.held(source.binding));
                    if !held {
                        self.facts_gate = None;
                    }
                    held
                } else {
                    false
                };
                #[cfg(not(all(test, feature = "allocation-probe")))]
                let held = false;

                let matching_insertion = insertion.is_some_and(|input| {
                    input.insertion.context == pending.context
                        && Arc::ptr_eq(&input.insertion.control, &pending.operation)
                        && input.device_id == source.device
                        && input.insertion.destination.owner() == source.owner
                });
                // Only the Session continuation has the validated insertion and
                // its prepared baseline borrow. Background poll wakes that
                // continuation without taking the native source or its Facts.
                pending.facts_ready_for_session = at_facts && !held && insertion.is_none();
                let result = if held || pending.facts_ready_for_session {
                    Ok(None)
                } else if at_facts && !matching_insertion {
                    Err("vst3 preparation differs from the pending insertion".into())
                } else {
                    #[cfg(not(all(test, feature = "allocation-probe")))]
                    let plan = EffectPresentationPlan::AllowUnknown;
                    #[cfg(all(test, feature = "allocation-probe"))]
                    let plan = fixture_presentation_plan(self.presentation);
                    #[cfg(all(test, feature = "allocation-probe"))]
                    {
                        source.attempts = source.attempts.saturating_add(1);
                        pending.facts_attempt_from_session |= at_facts;
                    }
                    self.host
                        .as_ref()
                        .ok_or_else(|| "qualified vst3 helper unavailable".to_owned())
                        .and_then(|host| {
                            advance_source_at_progress(host, plan, control, source, progress)
                        })
                };
                match result {
                    Ok(Some(state)) => {
                        record_insertion_stage(
                            self.diagnostics.as_ref(),
                            source.binding,
                            source.project_generation,
                            "endpoint_ready",
                            format_args!(""),
                        );
                        pending.result = Some(Ok(state));
                    }
                    Ok(None) => {}
                    Err(error) => pending.result = Some(Err(bounded_error(error))),
                }
            } else {
                pending.result = Some(Err("original vst3 source unavailable".into()));
            }
        }
        // Refused operations, deleted sources and superseded versions retain
        // their original bindings until Root's graph and endpoint returns and
        // the editor receipt owner have released them.
        self.reclaim_retired_sources(control, desired, false);
        #[cfg(all(test, feature = "allocation-probe"))]
        {
            if let Some(observed) = &self.progress
                && let Ok(mut observed) = observed.try_lock()
            {
                let source = self.sources.first();
                let restore_refused = observed.restore_refused;
                let editor_receipts = observed.editor_receipts;
                let editor_receipt_count = observed.editor_receipt_count;
                let mut source_bindings = [None; 2];
                for (entry, source) in source_bindings.iter_mut().zip(
                    self.sources
                        .iter()
                        .filter(|source| source.accepted && !source.retiring),
                ) {
                    *entry = Some((
                        source.device,
                        source.binding,
                        control.native_source_completed_blocks(source.binding),
                    ));
                }
                *observed = PreparationObservation {
                    output: control.output_identity(),
                    binding: source.map(|source| source.binding),
                    source_bindings,
                    newest_source: self
                        .sources
                        .last()
                        .map(|source| (source.device, source.binding, source.accepted)),
                    pending: self.pending.is_some(),
                    accepted: source.is_some_and(|source| source.accepted),
                    retiring: source.is_some_and(|source| source.retiring),
                    attempts: source.map_or(0, |source| source.attempts),
                    facts_attempt_from_session: self
                        .pending
                        .as_ref()
                        .is_some_and(|p| p.facts_attempt_from_session),
                    facts_ready_for_session: self
                        .pending
                        .as_ref()
                        .is_some_and(|p| p.facts_ready_for_session),
                    native: source.and_then(|source| {
                        control.native_source_preparation_progress(source.binding)
                    }),
                    restore_refused,
                    observed_latency_receipt: source
                        .is_some_and(|source| source.observed_latency_receipt),
                    recovery_read_binding: observed.recovery_read_binding,
                    editor_receipts,
                    editor_receipt_count,
                    hold_pending_read: observed.hold_pending_read,
                    held_pending_read: observed.held_pending_read,
                };
            }
            if let Some(queued) = &self.queued {
                queued.store(
                    self.sources.iter().any(|s| s.queued && !s.accepted),
                    std::sync::atomic::Ordering::Release,
                );
            }
            if let Some(records) = &self.records {
                records.store(self.sources.len(), std::sync::atomic::Ordering::Release);
            }
        }
    }

    /// Whether any retiring source slot can still make reclamation progress
    /// without violating editor transfer/hold or parked user-disposition custody.
    pub(super) fn has_reclaimable_retiring_source(&self) -> bool {
        self.restore.is_none()
            && self.sources.iter().any(|source| {
                source.retiring
                    && !source.editor_transfer
                    && !self.editor_holds(source.binding)
                    && !source.editor_retirement.parked()
            })
    }

    /// Drive retirement requests, retiring-editor receipt completion, and Root
    /// cell recovery for all eligible retiring sources while preserving Root's
    /// active-graph, tail, endpoint-return, and editor-hold custody.
    pub(super) fn reclaim_retired_sources(
        &mut self,
        control: &mut hex_audio::prepared::PublicationControl,
        desired: Option<&CanonicalAudioSourceSnapshot>,
        pump_reclaim: bool,
    ) -> usize {
        if let Some(desired) = desired {
            self.retire_installed_removals(control, desired);
        }
        // Renewal's bounded indices refer to this original record set. Leave
        // refused records in control custody until the epoch operation retires;
        // removing one here would shift a later source past its renewal stage.
        if self.restore.is_some() {
            return 0;
        }
        let mut requested_any = false;
        for source in &self.sources {
            if source.retiring && !source.editor_transfer && !self.editor_holds(source.binding) {
                let _ = control.retire_native_source_preparation(source.binding);
                requested_any = true;
            }
        }
        if requested_any {
            // Advance retiring-editor leases right after signaling Root so
            // `retiring_edit_reserved` can release in the same turn instead of
            // deferring reclamation to a later poll.
            self.poll_retiring_edits();
            if pump_reclaim {
                control.reclaim();
            }
        }
        let mut reclaimed = 0;
        let mut index = 0;
        while index < self.sources.len() {
            let source = &self.sources[index];
            if source.retiring && !source.editor_transfer && !self.editor_holds(source.binding) {
                let _ = control.retire_native_source_preparation(source.binding);
                if control
                    .recover_native_source_preparation(source.binding)
                    .is_some()
                {
                    self.sources.remove(index);
                    reclaimed += 1;
                    continue;
                }
            }
            index += 1;
        }
        reclaimed
    }

    /// An installed canonical revision ends the device's project membership.
    /// Root still controls return of every graph, endpoint and editor receipt.
    fn retire_installed_removals(
        &mut self,
        control: &hex_audio::prepared::PublicationControl,
        desired: &CanonicalAudioSourceSnapshot,
    ) {
        let context = desired.context();
        if self.installed_removals_checked == Some(context)
            || control
                .status
                .recording_source()
                .is_none_or(|installed| installed.context != context)
        {
            return;
        }
        self.retire_absent_catalog_sources(desired);
        self.installed_removals_checked = Some(context);
    }

    /// Called only after the exact canonical graph installs. Source absence
    /// ends editor admission while Root retains actual runtime custody.
    pub(in crate::native_audio) fn retire_absent_catalog_sources(
        &mut self,
        desired: &CanonicalAudioSourceSnapshot,
    ) {
        let context = desired.context();
        let preparation = desired.effect_preparation();
        self.retire_absent_sources_for_context(context, |visit| {
            preparation.visit_plugin_devices(|destination, device, _| {
                visit(destination.owner(), device);
                Ok(())
            })
        });
    }

    pub(super) fn retire_absent_sources_for_context(
        &mut self,
        context: AudioSourceContext,
        mut visit_live: impl FnMut(&mut dyn FnMut(ProcessorChainOwner, u64)) -> Result<(), String>,
    ) {
        let has_catalog = self.catalog.is_some();
        for index in 0..self.sources.len() {
            let source = &self.sources[index];
            if !source.accepted || source.retiring {
                continue;
            }
            if source.project_generation < context.project_generation {
                self.sources[index].accepted = false;
                self.sources[index].retiring = true;
                continue;
            }
            if source.project_generation != context.project_generation {
                continue;
            }
            let target_owner = source.owner;
            let target_device = source.device;
            let mut present_on_owner = false;
            let mut present_anywhere = false;
            if visit_live(&mut |owner, device| {
                if device == target_device {
                    present_anywhere = true;
                    present_on_owner |= owner == target_owner;
                }
            })
            .is_err()
            {
                continue;
            }
            let superseded_by_newer = self.sources[index + 1..].iter().any(|newer| {
                newer.accepted
                    && !newer.retiring
                    && newer.project_generation == context.project_generation
                    && newer.device == target_device
            });
            let retire = if present_on_owner {
                superseded_by_newer
            } else if has_catalog || present_anywhere || superseded_by_newer {
                true
            } else {
                // Catalog-less test fixtures cannot reload a provider from disk
                // after the last source of that provider retires; still retire
                // extra absent sources whenever another live source for the
                // same provider remains.
                self.sources[index].provider.is_some_and(|provider| {
                    self.sources.iter().enumerate().any(|(other_index, other)| {
                        other_index != index
                            && other.accepted
                            && !other.retiring
                            && other.project_generation == context.project_generation
                            && other.provider == Some(provider)
                    })
                })
            };
            if retire {
                self.sources[index].accepted = false;
                self.sources[index].retiring = true;
            }
        }
    }
}
