//! Legacy graph publication advancement on the real-time callback.
use super::*;

impl Callback {
    pub(super) fn advance_legacy_publication(&mut self, frames: Option<usize>) {
        let confirmed_insert = self
            .deferred
            .as_ref()
            .is_some_and(|next| next.approved_device_insert().is_some());
        if confirmed_insert && frames.is_some() {
            return;
        }
        if frames.is_none()
            && !confirmed_insert
            && self
                .deferred
                .as_ref()
                .is_some_and(|next| next.streams.is_some())
        {
            // An already-prepared candidate must release its page role before
            // an admitted command can schedule work on the predecessor.
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            if self.status.admitted_ticket.load(Acquire)
                != self.status.consumed_command.load(Acquire)
                && let Some(mut next) = self.deferred.take()
            {
                if matches!(
                    next.policy,
                    PublicationPolicy::InheritUnderNativeParameter { .. }
                ) {
                    if let Some(streams) = next.streams.as_mut() {
                        streams.cancel_install(&mut self.domain_pages);
                    }
                    self.restore_parameter_predecessor_prewarm(&next);
                }
                self.deferred = Some(next);
            }
            return;
        }
        if !self.scheduled_publication_due() {
            if frames.is_some_and(|frames| frames != 0) {
                self.prewarm_tempo_cut();
            }
            return;
        }
        if self.retired.is_full()
            && (confirmed_insert
                || self.deferred.as_ref().is_some_and(|next| {
                    next.streams.is_some() && next.playback.native_removal_started()
                }))
        {
            self.abort_blocked_device_insert();
            return;
        }
        // A candidate with a transient release-capacity refusal remains owned
        // here while the previous runtime continues to render and drain tails.
        // The queue gate prevents a second graph from being admitted meanwhile.
        if (self.active.is_none() || !self.retired.is_full())
            && let Some(mut next) = self.deferred.take()
        {
            self.trace_audio(
                AudioDiagnosticKind::PublicationAttempt,
                next.revision,
                next.source_context,
                frames.unwrap_or(0),
            );
            if next
                .fresh_runtime_cancellation
                .as_ref()
                .is_some_and(PublicationReceiptOwner::fenced_before_admission)
            {
                if let Some(streams) = next.streams.as_mut() {
                    streams.cancel_install(&mut self.domain_pages);
                }
                self.reject_publication(
                    next.revision,
                    PlaybackHandoffRejection::FreshRuntimeCancelled,
                );
                self.return_rejected_graph(next);
                return;
            }
            let ready = if confirmed_insert {
                self.advance_confirmed_device_insert(&mut next)
            } else {
                self.device_insert_cut_ready(&mut next)
            };
            match ready {
                Ok(true) => {}
                Ok(false) => {
                    self.deferred = Some(next);
                    return;
                }
                Err(reason) => {
                    if let Some(streams) = next.streams.as_mut() {
                        streams.cancel_install(&mut self.domain_pages);
                    }
                    self.reject_publication(next.revision, reason);
                    self.return_rejected_graph(next);
                    return;
                }
            }
            // Do not overtake an accepted transport command or cancel a
            // streamed action while deciding a gesture-bound successor.
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            if matches!(
                next.policy,
                PublicationPolicy::InheritUnderNativeParameter { .. }
            ) && (self.status.admitted_ticket.load(Acquire)
                != self.status.consumed_command.load(Acquire)
                || self.stream_pending_play.is_some()
                || self.stream_action_pending()
                || self.transport_fade.is_some())
            {
                if let Some(streams) = next.streams.as_mut() {
                    streams.cancel_install(&mut self.domain_pages);
                }
                self.restore_parameter_predecessor_prewarm(&next);
                self.deferred = Some(next);
                return;
            }
            #[cfg(test)]
            let forced_refusal = self.test_transient_handoff_refusals > 0;
            #[cfg(test)]
            if forced_refusal {
                self.test_transient_handoff_refusals -= 1;
            }
            #[cfg(not(test))]
            let forced_refusal = false;
            let stale_fence = match next.policy {
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence)
                | PublicationPolicy::InstallQuantizedSeekWhenReady { fence, .. }
                | PublicationPolicy::InheritAtTempoCut(fence)
                | PublicationPolicy::DeviceInsert { fence, .. } => {
                    let active = self.active.as_ref().and_then(ActivePublication::legacy);
                    let fence = if matches!(next.policy, PublicationPolicy::DeviceInsert { .. }) {
                        next.retained_insert_fence(fence)
                    } else {
                        fence
                    };
                    let source_changed =
                        if let PublicationPolicy::DeviceInsert { predecessor, .. } = next.policy {
                            self.recording_source() != Some(predecessor)
                                || active.is_none_or(|active| {
                                    active.playback.session_capability_status()
                                        != SessionCapabilityStatus::Available
                                })
                                || self.stream_action_pending()
                        } else {
                            false
                        };
                    (source_changed || Self::runtime_fence_changed(&self.status, active, fence))
                        .then_some(hex_engine::PlaybackHandoffError::RuntimeFenceChanged)
                }
                PublicationPolicy::InstallUnderStoppedArrangement(token)
                | PublicationPolicy::InheritUnderArrangement(token) => {
                    (self.status.sampler_edit.state(token) != StoppedEditFenceState::Ready)
                        .then_some(hex_engine::PlaybackHandoffError::RuntimeFenceChanged)
                }
                PublicationPolicy::InstallUnderNativeEpoch(token) => {
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    let live = self
                        .native_sources
                        .runtime_transports_are_live(&next.playback);
                    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
                    let live = false;
                    (!native_epoch_accepts_successor(
                        self.status.sampler_edit.native_epoch_context(token),
                        next.source_context,
                    ) || self.active.is_some()
                        || !live)
                        .then_some(hex_engine::PlaybackHandoffError::RuntimeFenceChanged)
                }
                PublicationPolicy::InheritUnderStoppedNative { source, fence } => {
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    let live = self
                        .native_sources
                        .runtime_transports_are_live(&next.playback)
                        && self.current_native_epoch_source() == Some(source);
                    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
                    let live = false;
                    (!live
                        || !self
                            .status
                            .stopped_native_source_fence_matches(source, fence)
                        || self
                            .active
                            .as_ref()
                            .and_then(ActivePublication::legacy)
                            .is_none_or(|active| active.playback.playing())
                        || next.source_context.is_none_or(|context| {
                            context.project_generation != source.context.project_generation
                        }))
                    .then_some(hex_engine::PlaybackHandoffError::RuntimeFenceChanged)
                }
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                PublicationPolicy::InheritUnderNativeParameter {
                    source,
                    transport_epoch,
                    playing,
                } => {
                    let predecessor_matches = !next.playback.has_foreign_continuation_states()
                        || self
                            .active
                            .as_ref()
                            .and_then(ActivePublication::legacy)
                            .is_some_and(|active| {
                                next.playback
                                    .foreign_continuation_matches_predecessor_on_callback(
                                        &active.playback,
                                        next.native_continuation.is_none(),
                                    )
                            });
                    (!predecessor_matches
                        || !self.native_parameter_fence_matches(
                            source,
                            transport_epoch,
                            playing,
                            next.source_context,
                        ))
                    .then_some(hex_engine::PlaybackHandoffError::RuntimeFenceChanged)
                }
                PublicationPolicy::InheritRuntime | PublicationPolicy::InstallFreshRuntime => None,
            };
            let monitor_handoff = stale_fence
                .is_none()
                .then(|| self.paused_input_graph_handoff(&next))
                .flatten();
            if let Some((previous, _, epoch)) = monitor_handoff
                && self
                    .input_monitor
                    .blocks_graph_replacement(previous, epoch, &next.playback)
            {
                if confirmed_insert {
                    self.reject_publication(
                        next.revision,
                        PlaybackHandoffRejection::RuntimeFenceChanged,
                    );
                    self.return_rejected_graph(next);
                    return;
                }
                // The live branch must render its committed transition
                // before publication can try again. The pending candidate
                // stays in the existing publication slot.
                self.deferred = Some(next);
                return;
            }
            #[cfg(test)]
            if stale_fence.is_none()
                && matches!(
                    next.policy,
                    PublicationPolicy::InstallFreshRuntimeIfUnchanged(_)
                        | PublicationPolicy::InstallQuantizedSeekWhenReady { .. }
                )
                && let Some(barrier) = self.test_after_fresh_fence_validation.take()
            {
                barrier.wait();
                barrier.wait();
            }
            let inserting = matches!(next.policy, PublicationPolicy::DeviceInsert { .. });
            let tempo_cut = matches!(next.policy, PublicationPolicy::InheritAtTempoCut(_));
            if inserting && !confirmed_insert {
                // The candidate owns this law so stream preflight and final
                // inheritance both reject the same unproven interruption.
                next.playback.require_continuous_device_insert();
                if stale_fence.is_none() && !forced_refusal {
                    match self.advance_native_device_removal(&mut next) {
                        Ok(true) => {}
                        Ok(false) => {
                            self.deferred = Some(next);
                            return;
                        }
                        Err(reason) => {
                            self.reject_publication(next.revision, reason);
                            self.return_rejected_graph(next);
                            return;
                        }
                    }
                }
            }
            let mut promotions = None;
            if confirmed_insert && stale_fence.is_none() && self.device_insert_uses_streams(&next) {
                let prepared = next
                    .streams
                    .as_mut()
                    .map(|streams| streams.prepare_commit(&mut self.domain_pages));
                match prepared {
                    Some(Ok(prepared)) => promotions = Some(prepared),
                    _ => {
                        self.reject_publication(
                            next.revision,
                            PlaybackHandoffRejection::StreamedPagesNotReady,
                        );
                        self.return_rejected_graph(next);
                        return;
                    }
                }
            } else if !forced_refusal
                && !confirmed_insert
                && stale_fence.is_none()
                && next.streams.is_some()
            {
                let pending_window = self
                    .active
                    .as_mut()
                    .and_then(ActivePublication::stream_view)
                    .is_some_and(|active| active.has_pending_render_window());
                if pending_window {
                    if next.policy.inherits_runtime() {
                        // Give an inheriting successor's predecessor a chance
                        // to commit its exact pending window first, while
                        // bounding a current demand that can never fit.
                        if let Some(frames) = frames {
                            let mut reservation = self.reserve_current_stream_window(frames);
                            if matches!(
                                reservation,
                                Err(crate::candidate_custody::QuantumFailure::Capacity)
                            ) {
                                if let Some(streams) = next.streams.as_mut() {
                                    streams.cancel_install(&mut self.domain_pages);
                                }
                                reservation = self.reserve_current_stream_window(frames);
                            }
                            match reservation {
                                Ok(Some(pins)) => self.current_quantum_pins = Some(pins),
                                Ok(None)
                                | Err(crate::candidate_custody::QuantumFailure::PendingAction) => {}
                                Err(crate::candidate_custody::QuantumFailure::Capacity) => {
                                    if !next
                                        .streams
                                        .as_mut()
                                        .is_some_and(|streams| streams.retry_capacity_conflict())
                                    {
                                        if let Some(streams) = next.streams.as_mut() {
                                            streams.cancel_install(&mut self.domain_pages);
                                        }
                                        self.reject_publication(
                                            next.revision,
                                            PlaybackHandoffRejection::StreamedPageCapacity,
                                        );
                                        self.return_rejected_graph(next);
                                        return;
                                    }
                                }
                                Err(crate::candidate_custody::QuantumFailure::Invalid) => {
                                    if let Some(streams) = next.streams.as_mut() {
                                        streams.cancel_install(&mut self.domain_pages);
                                    }
                                    self.reject_publication(
                                        next.revision,
                                        PlaybackHandoffRejection::StreamedPagesNotReady,
                                    );
                                    self.return_rejected_graph(next);
                                    return;
                                }
                            }
                        }
                        self.trace_audio(
                            AudioDiagnosticKind::PublicationWaiting {
                                reason: PublicationWaitReason::PredecessorWindow,
                            },
                            next.revision,
                            next.source_context,
                            frames.unwrap_or(0),
                        );
                        self.defer_for_stream_pages(next);
                        return;
                    }
                    // A fresh replacement is a recovery path when the current
                    // source cannot complete; it must not wait on that source.
                    if let Some(mut active) = self
                        .active
                        .as_mut()
                        .and_then(ActivePublication::stream_view)
                    {
                        active.cancel_pending_render_window(&mut self.domain_pages);
                    }
                }
                self.cancel_active_stream_action();
                if let Some(mut active) = self
                    .active
                    .as_mut()
                    .and_then(ActivePublication::stream_view)
                {
                    active.publication_replaces_prewarm(&mut self.domain_pages);
                }
                if next.policy.inherits_runtime()
                    && self
                        .active
                        .as_ref()
                        .and_then(ActivePublication::legacy)
                        .is_some_and(|old| {
                            old.source_context.is_none_or(|context| {
                                context.project_generation != next.control_context_generation
                            })
                        })
                {
                    self.reject_publication(
                        next.revision,
                        PlaybackHandoffRejection::RuntimeFenceChanged,
                    );
                    self.return_rejected_graph(next);
                    return;
                }
                if let Some(frames) = frames {
                    let mut reservation = self.reserve_current_stream_window(frames);
                    if matches!(
                        reservation,
                        Err(crate::candidate_custody::QuantumFailure::Capacity)
                    ) {
                        if let Some(streams) = next.streams.as_mut() {
                            streams.cancel_install(&mut self.domain_pages);
                        }
                        reservation = self.reserve_current_stream_window(frames);
                    }
                    let refusal = match reservation {
                        Ok(Some(pins)) => {
                            let complete = pins.complete();
                            self.current_quantum_pins = Some(pins);
                            if !complete && next.policy.inherits_runtime() {
                                if let Some(streams) = next.streams.as_mut() {
                                    streams.cancel_install(&mut self.domain_pages);
                                }
                                self.defer_for_stream_pages(next);
                                return;
                            }
                            None
                        }
                        Ok(None) => None,
                        Err(crate::candidate_custody::QuantumFailure::PendingAction) => {
                            self.defer_for_stream_pages(next);
                            return;
                        }
                        Err(crate::candidate_custody::QuantumFailure::Capacity) => {
                            if next
                                .streams
                                .as_mut()
                                .is_some_and(|streams| streams.retry_capacity_conflict())
                            {
                                self.defer_for_stream_pages(next);
                                return;
                            }
                            Some(PlaybackHandoffRejection::StreamedPageCapacity)
                        }
                        Err(crate::candidate_custody::QuantumFailure::Invalid) => {
                            Some(PlaybackHandoffRejection::StreamedPagesNotReady)
                        }
                    };
                    if let Some(reason) = refusal {
                        if let Some(streams) = next.streams.as_mut() {
                            streams.cancel_install(&mut self.domain_pages);
                        }
                        self.reject_publication(next.revision, reason);
                        self.return_rejected_graph(next);
                        return;
                    }
                }
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                let foreign_continuation = matches!(
                    next.policy,
                    PublicationPolicy::InheritUnderNativeParameter { .. }
                ) && next.playback.has_foreign_continuation_states();
                #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
                let foreign_continuation = false;
                let (prepared, preview_refused) = {
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    let guard = next.native_continuation.as_ref().and_then(|source| {
                        next.playback
                            .foreign_continuation_binding()
                            .and_then(|binding| {
                                native_sources::reserve_parameter_observation(
                                    &self.native_sources,
                                    &self.status,
                                    binding,
                                    source,
                                )
                            })
                    });
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    let preview_refused = next.native_continuation.is_some()
                        && guard.as_ref().is_none_or(|guard| !guard.is_live());
                    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
                    let preview_refused = false;
                    if preview_refused {
                        (None, true)
                    } else {
                        let previous = if next.policy.inherits_runtime() {
                            self.active
                                .as_mut()
                                .and_then(ActivePublication::legacy_mut)
                                .and_then(|old| {
                                    old.source_context
                                        .map(|context| (&mut *old.playback, context, old.revision))
                                })
                        } else {
                            None
                        };
                        let fenced_restart =
                            matches!(next.policy, PublicationPolicy::InheritUnderArrangement(_))
                                && next.playback.live_restart_requested();
                        let stream_frames = if tempo_cut
                            && next.playback.contains_streamed_sources()
                            && frames.is_some_and(|count| count != 0)
                        {
                            TEMPO_STREAM_WINDOW_FRAMES
                        } else {
                            frames.unwrap_or(0)
                        };
                        let prepared = next.streams.as_mut().map(|streams| {
                            streams.prepare_install(
                                &mut next.playback,
                                previous,
                                &mut self.domain_pages,
                                self.stream_pending_play.is_some(),
                                self.stream_prefix,
                                stream_frames,
                                fenced_restart,
                                foreign_continuation,
                                None,
                                next.fresh_runtime_cancellation
                                    .as_ref()
                                    .and_then(|receipt| receipt.0.tempo_cut())
                                    .map(|cut| {
                                        crate::candidate_custody::TempoStreamWindow::Exact(
                                            &cut.anchor,
                                        )
                                    }),
                            )
                        });
                        (prepared, false)
                    }
                };
                if preview_refused {
                    if let Some(streams) = next.streams.as_mut() {
                        streams.cancel_install(&mut self.domain_pages);
                    }
                    self.reject_publication(
                        next.revision,
                        PlaybackHandoffRejection::RuntimeFenceChanged,
                    );
                    self.return_rejected_graph(next);
                    return;
                }
                match prepared {
                    Some(Ok(Some(_))) => {
                        match self.advance_paged_native_removal(&mut next) {
                            Ok(true) => {}
                            Ok(false) => {
                                self.trace_audio(
                                    AudioDiagnosticKind::PublicationWaiting {
                                        reason: PublicationWaitReason::NativeRemovalFade,
                                    },
                                    next.revision,
                                    next.source_context,
                                    frames.unwrap_or(0),
                                );
                                self.deferred = Some(next);
                                return;
                            }
                            Err(reason) => {
                                if let Some(streams) = next.streams.as_mut() {
                                    streams.cancel_install(&mut self.domain_pages);
                                }
                                self.reject_publication(next.revision, reason);
                                self.return_rejected_graph(next);
                                return;
                            }
                        }
                        // Page planning may span callbacks. Revalidate the
                        // actual fence before promoting its protected pages.
                        if let PublicationPolicy::InheritUnderArrangement(token) = next.policy
                            && self.status.sampler_edit.state(token) != StoppedEditFenceState::Ready
                        {
                            if let Some(streams) = next.streams.as_mut() {
                                streams.cancel_install(&mut self.domain_pages);
                            }
                            self.reject_publication(
                                next.revision,
                                PlaybackHandoffRejection::RuntimeFenceChanged,
                            );
                            self.return_rejected_graph(next);
                            return;
                        }
                        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                        if let PublicationPolicy::InheritUnderNativeParameter {
                            source,
                            transport_epoch,
                            playing,
                        } = next.policy
                        {
                            let changed = !self.native_parameter_fence_matches(
                                source,
                                transport_epoch,
                                playing,
                                next.source_context,
                            );
                            let commands_pending = self.status.admitted_ticket.load(Acquire)
                                != self.status.consumed_command.load(Acquire);
                            if changed || commands_pending {
                                if let Some(streams) = next.streams.as_mut() {
                                    streams.cancel_install(&mut self.domain_pages);
                                }
                                if changed {
                                    self.reject_publication(
                                        next.revision,
                                        PlaybackHandoffRejection::RuntimeFenceChanged,
                                    );
                                    self.return_rejected_graph(next);
                                } else {
                                    self.restore_parameter_predecessor_prewarm(&next);
                                    self.defer_for_stream_pages(next);
                                }
                                return;
                            }
                        }
                        self.trace_audio(
                            AudioDiagnosticKind::PublicationPrepared,
                            next.revision,
                            next.source_context,
                            frames.unwrap_or(0),
                        );
                        let committed = next
                            .streams
                            .as_mut()
                            .map(|streams| streams.prepare_commit(&mut self.domain_pages));
                        match committed {
                            Some(Ok(value)) => promotions = Some(value),
                            _ => {
                                if let Some(streams) = next.streams.as_mut() {
                                    streams.cancel_install(&mut self.domain_pages);
                                }
                                self.defer_for_stream_pages(next);
                                return;
                            }
                        }
                    }
                    Some(Ok(None)) => {
                        let reason = next
                            .streams
                            .as_mut()
                            .and_then(|streams| streams.take_handoff_diagnostic())
                            .map_or(
                                PublicationWaitReason::StreamPages,
                                PublicationWaitReason::RuntimeHandoff,
                            );
                        self.trace_audio(
                            AudioDiagnosticKind::PublicationWaiting { reason },
                            next.revision,
                            next.source_context,
                            frames.unwrap_or(0),
                        );
                        self.defer_for_stream_pages(next);
                        return;
                    }
                    refused => {
                        let capacity_conflict = next
                            .streams
                            .as_ref()
                            .is_some_and(|streams| streams.capacity_conflict());
                        if capacity_conflict
                            && next
                                .streams
                                .as_mut()
                                .is_some_and(|streams| streams.retry_candidate_capacity_conflict())
                        {
                            if let Some(streams) = next.streams.as_mut() {
                                streams.cancel_install(&mut self.domain_pages);
                            }
                            self.defer_for_stream_pages(next);
                            return;
                        }
                        let source_changed = next
                            .streams
                            .as_mut()
                            .is_some_and(|streams| streams.take_source_changed());
                        let handoff = next
                            .streams
                            .as_mut()
                            .and_then(|streams| streams.take_handoff_diagnostic());
                        let detail = match refused {
                            Some(Err(error)) => error.0,
                            _ => "stream preparation state unavailable",
                        };
                        self.trace_audio(
                            AudioDiagnosticKind::StreamPreflightFailed { detail, handoff },
                            next.revision,
                            next.source_context,
                            frames.unwrap_or(0),
                        );
                        if let Some(streams) = next.streams.as_mut() {
                            streams.cancel_install(&mut self.domain_pages);
                        }
                        self.reject_publication(
                            next.revision,
                            if source_changed {
                                PlaybackHandoffRejection::StreamSourceChanged
                            } else {
                                handoff.map_or(
                                    if capacity_conflict {
                                        PlaybackHandoffRejection::StreamedPageCapacity
                                    } else {
                                        PlaybackHandoffRejection::StreamedPagesNotReady
                                    },
                                    PlaybackHandoffRejection::from_error,
                                )
                            },
                        );
                        self.return_rejected_graph(next);
                        return;
                    }
                }
            }
            next.playback
                .bind_browser_audition_transport(self.status.transport_epoch.load(Acquire));
            let fenced_refusal = if confirmed_insert && stale_fence.is_none() {
                let active = self.active.as_ref().and_then(ActivePublication::legacy);
                let cut_is_current = || {
                    next.approved_device_insert()
                        .is_some_and(|basis| basis.cut_is_current(&self.status, active))
                };
                if !cut_is_current() {
                    Some(hex_engine::PlaybackHandoffError::RuntimeFenceChanged)
                } else if !next
                    .fresh_runtime_cancellation
                    .as_ref()
                    .is_some_and(PublicationReceiptOwner::begin_confirmed_apply)
                {
                    Some(hex_engine::PlaybackHandoffError::FreshRuntimeCancelled)
                } else {
                    (!cut_is_current())
                        .then_some(hex_engine::PlaybackHandoffError::RuntimeFenceChanged)
                }
            } else if let PublicationPolicy::DeviceInsert { fence, .. }
            | PublicationPolicy::InheritAtTempoCut(fence) = next.policy
                && !confirmed_insert
                && stale_fence.is_none()
                && !forced_refusal
            {
                #[cfg(test)]
                if let Some(barrier) = self.test_before_fresh_runtime_apply.take() {
                    barrier.wait();
                    barrier.wait();
                }
                if !next
                    .fresh_runtime_cancellation
                    .as_ref()
                    .is_some_and(PublicationReceiptOwner::begin_apply)
                {
                    Some(hex_engine::PlaybackHandoffError::FreshRuntimeCancelled)
                } else {
                    #[cfg(test)]
                    if let Some(barrier) = self.test_after_fresh_runtime_apply_decision.take() {
                        barrier.wait();
                        barrier.wait();
                    }
                    let active = self.active.as_ref().and_then(ActivePublication::legacy);
                    let fence = if inserting {
                        next.retained_insert_fence(fence)
                    } else {
                        fence
                    };
                    (Self::runtime_fence_changed(&self.status, active, fence)
                        || self.status.consumed_command.load(Acquire)
                            != fence.admitted_command_ticket
                        || self.status.closing.load(Acquire)
                        || self.transport_fade.is_some()
                        || self.stream_pending_play.is_some())
                    .then_some(hex_engine::PlaybackHandoffError::RuntimeFenceChanged)
                }
            } else {
                None
            };
            let handoff = if let Some(error) = fenced_refusal {
                Some(Err(error))
            } else if forced_refusal {
                Some(Err(hex_engine::PlaybackHandoffError::Session(
                    hex_engine::SessionPlaybackError::ReleaseBankFull,
                )))
            } else if let Some(error) = stale_fence {
                Some(Err(error))
            } else if matches!(
                next.policy,
                PublicationPolicy::InstallFreshRuntime
                    | PublicationPolicy::InstallFreshRuntimeIfUnchanged(_)
                    | PublicationPolicy::InstallQuantizedSeekWhenReady { .. }
                    | PublicationPolicy::InstallUnderStoppedArrangement(_)
                    | PublicationPolicy::InstallUnderNativeEpoch(_)
            ) {
                None
            } else {
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                let foreign_continuation = matches!(
                    next.policy,
                    PublicationPolicy::InheritUnderNativeParameter { .. }
                ) && next.playback.has_foreign_continuation_states();
                #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
                let foreign_continuation = false;
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                let mut guard = next.native_continuation.as_ref().and_then(|source| {
                    next.playback
                        .foreign_continuation_binding()
                        .and_then(|binding| {
                            native_sources::reserve_parameter_observation(
                                &self.native_sources,
                                &self.status,
                                binding,
                                source,
                            )
                        })
                });
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                let isolated_fence_changed = foreign_continuation
                    && next.native_continuation.is_none()
                    && match next.policy {
                        PublicationPolicy::InheritUnderNativeParameter {
                            source,
                            transport_epoch,
                            playing,
                        } => {
                            // The graph pair was already compared. Recheck
                            // the concurrently changing source and transport.
                            self.status.native_parameter_source(playing)
                                != Some((source, transport_epoch))
                                || self.status.device_fault.load(Acquire)
                                || self.status.render_fault.load(Acquire)
                                || self.status.admitted_ticket.load(Acquire)
                                    != self.status.consumed_command.load(Acquire)
                        }
                        _ => true,
                    };
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                let refused = isolated_fence_changed
                    || next.native_continuation.is_some()
                        && guard.as_mut().is_none_or(|guard| {
                            !guard.try_order_before_revocation()
                                || self
                                    .status
                                    .device_fault
                                    .compare_exchange(false, false, AcqRel, Acquire)
                                    .is_err()
                                || self
                                    .status
                                    .render_fault
                                    .compare_exchange(false, false, AcqRel, Acquire)
                                    .is_err()
                                || self
                                    .status
                                    .pending_stop_marker
                                    .compare_exchange(0, 0, AcqRel, Acquire)
                                    .is_err()
                                || {
                                    let consumed = self.status.consumed_command.load(Acquire);
                                    self.status
                                        .admitted_ticket
                                        .compare_exchange(consumed, consumed, AcqRel, Acquire)
                                        .is_err()
                                }
                        });
                #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
                let refused = false;
                if refused {
                    Some(Err(hex_engine::PlaybackHandoffError::RuntimeFenceChanged))
                } else {
                    self.active
                        .as_mut()
                        .and_then(ActivePublication::legacy_mut)
                        .map(|previous| {
                            let generation = next
                                .explicit_control_context
                                .then_some(next.control_context_generation);
                            let previous_generation = previous
                                .explicit_control_context
                                .then_some(previous.control_context_generation);
                            if let Some(cut) = next
                                .fresh_runtime_cancellation
                                .as_ref()
                                .and_then(|receipt| receipt.0.tempo_cut())
                            {
                                next.playback.try_inherit_runtime_with_tempo_anchor(
                                    &mut previous.playback,
                                    generation,
                                    previous_generation,
                                    &cut.anchor,
                                )
                            } else if matches!(
                                next.policy,
                                PublicationPolicy::InheritUnderArrangement(_)
                            ) && next.playback.live_restart_requested()
                            {
                                next.playback.try_restart_runtime_under_arrangement_fence(
                                    &mut previous.playback,
                                    generation,
                                    previous_generation,
                                )
                            } else if foreign_continuation {
                                next.playback
                                    .try_inherit_runtime_with_stored_foreign_continuation(
                                        &mut previous.playback,
                                        generation,
                                        previous_generation,
                                    )
                            } else {
                                next.playback.try_inherit_runtime_in_control_context(
                                    &mut previous.playback,
                                    generation,
                                    previous_generation,
                                )
                            }
                        })
                }
            };
            match handoff {
                Some(Err(error)) if error.is_transient() && !inserting && !tempo_cut => {
                    let _promotions = promotions;
                    self.trace_audio(
                        AudioDiagnosticKind::PublicationWaiting {
                            reason: PublicationWaitReason::RuntimeHandoff(error),
                        },
                        next.revision,
                        next.source_context,
                        frames.unwrap_or(0),
                    );
                    if let Some(streams) = next.streams.as_mut() {
                        streams.cancel_install(&mut self.domain_pages);
                    }
                    self.deferred = Some(next);
                }
                Some(Err(error)) => {
                    let _promotions = promotions;
                    if let Some(streams) = next.streams.as_mut() {
                        streams.cancel_install(&mut self.domain_pages);
                    }
                    if let Some(reason) = error.rejection().or_else(|| {
                        (inserting || tempo_cut)
                            .then(|| PlaybackHandoffRejection::from_error(error))
                    }) {
                        self.reject_publication(next.revision, reason);
                    }
                    self.return_rejected_graph(next);
                }
                _ => {
                    #[cfg(test)]
                    if matches!(
                        next.policy,
                        PublicationPolicy::InstallFreshRuntimeIfUnchanged(_)
                            | PublicationPolicy::InstallQuantizedSeekWhenReady { .. }
                    ) && let Some(barrier) = self.test_before_fresh_runtime_apply.take()
                    {
                        barrier.wait();
                        barrier.wait();
                    }
                    // Device inserts and tempo cuts decide before borrowing
                    // either graph mutably for inheritance.
                    let applying = inserting
                        || tempo_cut
                        || next
                            .fresh_runtime_cancellation
                            .as_ref()
                            .is_none_or(|cancellation| cancellation.begin_apply());
                    if !applying {
                        let _promotions = promotions;
                        if let Some(streams) = next.streams.as_mut() {
                            streams.cancel_install(&mut self.domain_pages);
                        }
                        self.reject_publication(
                            next.revision,
                            PlaybackHandoffRejection::FreshRuntimeCancelled,
                        );
                        self.return_rejected_graph(next);
                        return;
                    }
                    #[cfg(test)]
                    if matches!(
                        next.policy,
                        PublicationPolicy::InstallFreshRuntimeIfUnchanged(_)
                            | PublicationPolicy::InstallQuantizedSeekWhenReady { .. }
                    ) && let Some(barrier) = self.test_after_fresh_runtime_apply_decision.take()
                    {
                        barrier.wait();
                        barrier.wait();
                    }
                    for promotion in promotions.into_iter().flatten().flatten() {
                        promotion.commit();
                    }
                    if let Some(streams) = next.streams.as_mut() {
                        streams.record_install();
                    }
                    if let Some(mut previous) = self
                        .active
                        .as_mut()
                        .and_then(ActivePublication::stream_view)
                    {
                        if let Some(mut successor) = next.stream_view()
                            && let Some(ticket) =
                                successor.inherit_schedule_observation(&mut previous)
                        {
                            self.status
                                .stream_invalidated_schedule
                                .store(ticket, Release);
                        }
                        previous.clear_stream_roles(&mut self.domain_pages);
                    }
                    // A fresh seek keeps its reconstructed transport and
                    // internal DSP. Retained workers still need their gate
                    // identities after the final fence/cancellation decision.
                    if matches!(
                        next.policy,
                        PublicationPolicy::InstallFreshRuntimeIfUnchanged(_)
                            | PublicationPolicy::InstallQuantizedSeekWhenReady { .. }
                    ) && let Some(previous) =
                        self.active.as_mut().and_then(ActivePublication::legacy_mut)
                        && next.control_context_generation == previous.control_context_generation
                    {
                        next.playback
                            .inherit_metronome_for_fresh_seek(&previous.playback);
                        next.playback
                            .cut_retained_foreign_gates_for_fresh_seek(&mut previous.playback);
                    }
                    if let Some(previous) =
                        self.active.as_mut().and_then(ActivePublication::legacy_mut)
                    {
                        next.playback.detach_rebound_inputs_from(
                            &mut previous.playback,
                            next.explicit_control_context
                                .then_some(next.control_context_generation),
                            previous
                                .explicit_control_context
                                .then_some(previous.control_context_generation),
                        );
                        previous.playback.cancel_computer_keyboard();
                        previous.playback.cancel_tuning_audition();
                    }
                    if matches!(
                        next.policy,
                        PublicationPolicy::InstallUnderStoppedArrangement(_)
                    ) && self.status.transport_epoch.load(Acquire)
                        != self.status.sampler_edit.ready_transport_epoch.load(Acquire)
                    {
                        // Priority Stop after the ready witness wins over the
                        // prepared stopped cursor, even during graph construction.
                        next.playback.apply(PlaybackCommand::Stop);
                    }
                    let revision = next.revision;
                    // A held EQ8 selector normally follows the current pair
                    // through certified parameter retargets. If an active
                    // instrument note forces its old processor chain into a
                    // release tail, transfer only the transient selector to
                    // the matching candidate pair at this no-more-refusals
                    // point. The tail retains its ordinary filter history.
                    if let Some(target) = self
                        .processor_audition_lease
                        .filter(|target| self.processor_audition.current() == Some(*target))
                    {
                        let previous_matches_target = self
                            .active
                            .as_ref()
                            .and_then(ActivePublication::legacy)
                            .is_some_and(|active| {
                                active.control_context_generation == target.project_generation
                                    && active.revision >= target.activation_floor
                                    && active.playback.sample_rate() == target.sample_rate_hz
                                    && active.playback.sample_rate() == self.rate
                            });
                        let candidate_matches_target = next.control_context_generation
                            == target.project_generation
                            && next.revision >= target.activation_floor
                            && next.playback.sample_rate() == target.sample_rate_hz
                            && next.playback.sample_rate() == self.rate;
                        if previous_matches_target && candidate_matches_target {
                            let _ = next.playback.transfer_processor_audition_from_release_tail(
                                target.identity,
                                target.selection,
                            );
                        }
                    }
                    // This is beyond every fallible handoff decision. Tail
                    // chains transferred into the candidate have no current
                    // command target, so remove transient auditions before
                    // the candidate's first callback render.
                    next.playback.cancel_retired_processor_auditions();
                    self.status.sampler_console.store(false, Release);
                    self.status
                        .session_capability
                        .store(next.playback.session_capability_status() as u32, Relaxed);
                    // Candidate ownership cannot enter this legacy replacement
                    // path; the queue decision above retains it in the same slot.
                    if matches!(
                        self.active,
                        Some(ActivePublication::Candidate(_) | ActivePublication::Eligible(_))
                    ) {
                        self.return_graph(next);
                        return;
                    }
                    let diagnostic_context = next.source_context;
                    let reset_audible = matches!(
                        next.policy,
                        PublicationPolicy::InstallQuantizedSeekWhenReady { .. }
                    );
                    if let Some((previous, successor, epoch)) = monitor_handoff {
                        self.input_monitor
                            .preserve_graph_release(previous, successor, epoch);
                    }
                    if confirmed_insert {
                        next.playback.release_device_insert_cut();
                        self.release_device_insert_fade_pins();
                    }
                    let previous = self.active.replace(ActivePublication::Legacy(next));
                    if reset_audible {
                        self.audible_clock.clear();
                    }
                    self.release_current_quantum_pins();
                    // Publish actual inherited-tail requirements before the
                    // old graph can be destroyed on control and release its
                    // native references. No lock/allocation/owner drop occurs.
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    self.native_sources.refresh_runtime_requirements(
                        self.active
                            .as_ref()
                            .and_then(ActivePublication::legacy)
                            .map(|active| active.playback.as_ref()),
                    );
                    if let Some(ActivePublication::Legacy(previous)) = previous {
                        self.return_graph(previous);
                    }
                    self.finish_pending_stream_play();
                    self.publish_active_routes(true);
                    self.recovery_seek_pending = None;
                    if self.recovery_position_frozen && !self.status.device_fault.load(Acquire) {
                        self.recovery_position_frozen = false;
                        self.publish_transport();
                    }
                    self.status.render_fault_record.clear();
                    self.status.render_fault.store(false, Release);
                    self.status.stream_source_changed.store(false, Release);
                    self.status.stream_read_failed.store(false, Release);
                    self.status.applied_source.publish(None);
                    self.status.applied_revision.store(revision, Release);
                    self.status.publication_pending.store(false, Release);
                    if let Some(receipt) = self
                        .active
                        .as_ref()
                        .and_then(ActivePublication::legacy)
                        .and_then(|active| active.fresh_runtime_cancellation.as_ref())
                    {
                        receipt.installed();
                    }
                    self.trace_audio(
                        AudioDiagnosticKind::PublicationApplied,
                        revision,
                        diagnostic_context,
                        frames.unwrap_or(0),
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advance_legacy_publication_without_deferred_graph_stays_idle() {
        let (control, mut callback) = channel(48_000).unwrap();
        callback.advance_legacy_publication(None);
        let mut block = [[1.0; 2]; 16];
        callback.render(&mut block).unwrap();
        assert_eq!(block, [[0.0; 2]; 16]);
        assert_eq!(control.status.applied_revision.load(Acquire), 0);
    }
}
