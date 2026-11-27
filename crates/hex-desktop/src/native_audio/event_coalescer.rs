//! Coalesced bounded UI event queue for `NativeAudioSink`.
//!
//! Each event class holds at most one pending slot so backpressure on the UI
//! channel never drops release/open/observation ordering or accumulates queues.
use hex_engine::AudioSourceContext;
use hex_ui::{EditorAudioEvent, EditorPublicationState};
use std::sync::mpsc::{SyncSender, TrySendError};

pub(crate) struct EventCoalescer {
    sender: SyncSender<EditorAudioEvent>,
    // A release is terminal custody for the old endpoint. While it is pending,
    // no new output can open, so one bounded slot suffices across replacements.
    pub(super) pending_release: Option<EditorAudioEvent>,
    pub(super) pending_open: Option<EditorAudioEvent>,
    pub(crate) pending_observation: Option<EditorAudioEvent>,
    // One coalesced status may wait behind an accepted ordinary submission. Status changes
    // never overwrite the exact tuple needed to complete keyboard/restore/gain/seek tracking.
    pub(super) pending_status_observation: Option<EditorAudioEvent>,
    /// One route-change notice for the delivered output (#4131).
    pub(super) pending_route: Option<EditorAudioEvent>,
    pub(super) pending_degraded: Option<EditorAudioEvent>,
    pub(super) pending_editor_windows: Option<EditorAudioEvent>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) editor_windows: Option<(u64, u64, Vec<u64>)>,
    pub(super) pending_latency: Option<EditorAudioEvent>,
    pub(super) pending_parameter: Option<EditorAudioEvent>,
    pub(super) pending_plugin_transport: Option<EditorAudioEvent>,
    pub(crate) last_state: Option<(AudioSourceContext, &'static str, Option<u64>)>,
}

impl EventCoalescer {
    pub(super) fn new(sender: SyncSender<EditorAudioEvent>) -> Self {
        Self {
            sender,
            pending_release: None,
            pending_open: None,
            pending_observation: None,
            pending_status_observation: None,
            pending_route: None,
            pending_degraded: None,
            pending_editor_windows: None,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            editor_windows: None,
            pending_latency: None,
            pending_parameter: None,
            pending_plugin_transport: None,
            last_state: None,
        }
    }

    pub(super) fn clear_for_relinquish(&mut self) {
        self.pending_open = None; // Client close only: endpoint Drop is terminal-owned
        self.pending_observation = None;
        self.pending_status_observation = None;
        self.pending_route = None;
        self.pending_degraded = None;
        self.pending_editor_windows = None;
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            self.editor_windows = None;
        }
        self.pending_latency = None;
        if !matches!(
            self.pending_parameter,
            Some(EditorAudioEvent::PluginEditorRetirementIssue { .. })
        ) {
            self.pending_parameter = None;
        }
        self.pending_plugin_transport = None;
        self.last_state = None;
    }

    pub(super) fn observe(
        &mut self,
        output_identity: Option<u64>,
        context: AudioSourceContext,
        tag: &'static str,
        state: EditorPublicationState,
    ) {
        let revision = match &state {
            EditorPublicationState::Submitted { audio_revision }
            | EditorPublicationState::VerifiedPreviewSubmitted { audio_revision, .. }
            | EditorPublicationState::VerifiedArrangementGainSubmitted { audio_revision }
            | EditorPublicationState::VerifiedArrangementPlayStartSubmitted { audio_revision }
            | EditorPublicationState::VerifiedClipMoveSubmitted { audio_revision, .. }
            | EditorPublicationState::ResidentSubmitted { audio_revision, .. } => {
                Some(*audio_revision)
            }
            _ => None,
        };
        if self.last_state == Some((context, tag, revision)) {
            return;
        }
        let Some(output_identity) = output_identity else {
            return;
        };
        self.last_state = Some((context, tag, revision));
        let event = EditorAudioEvent::Publication {
            context,
            output_identity,
            state,
        };
        if matches!(
            self.pending_observation.as_ref(),
            Some(EditorAudioEvent::Publication {
                state: EditorPublicationState::ResidentSubmitted { .. },
                ..
            })
        ) {
            // Resident enqueue admission refuses while this exact submission is pending, so
            // only coalescible control status can arrive here. Keep it in the fixed status slot.
            self.pending_status_observation = Some(event);
        } else {
            self.pending_observation = Some(event);
        }
    }

    /// Attempts to flush pending slots in strict custody order. Returns `true`
    /// when the UI receiver has disconnected.
    pub(super) fn flush(&mut self) -> bool {
        for pending in [
            &mut self.pending_release,
            &mut self.pending_open,
            &mut self.pending_observation,
            &mut self.pending_status_observation,
            &mut self.pending_route,
            &mut self.pending_degraded,
            &mut self.pending_editor_windows,
            &mut self.pending_latency,
            &mut self.pending_plugin_transport,
            &mut self.pending_parameter,
        ] {
            let Some(event) = pending.take() else {
                continue;
            };
            match self.sender.try_send(event) {
                Ok(()) => {}
                Err(TrySendError::Full(event)) => {
                    *pending = Some(event);
                    return false;
                }
                Err(TrySendError::Disconnected(_)) => {
                    return true;
                }
            }
        }
        false
    }

    pub(super) fn is_quiescent(&self) -> bool {
        self.pending_open.is_none()
            && self.pending_observation.is_none()
            && self.pending_status_observation.is_none()
            && self.pending_route.is_none()
            && self.pending_degraded.is_none()
            && self.pending_editor_windows.is_none()
            && self.pending_latency.is_none()
            && self.pending_parameter.is_none()
            && self.pending_plugin_transport.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn ctx(generation: u64, revision: u64) -> AudioSourceContext {
        AudioSourceContext {
            project_generation: generation,
            editor_revision: revision,
        }
    }

    #[test]
    fn observe_deduplicates_and_protects_resident_submission_with_status_slot() {
        let (tx, rx) = mpsc::sync_channel(4);
        let mut coalescer = EventCoalescer::new(tx);
        let context = ctx(1, 2);

        // No output identity: observation is ignored without recording last_state.
        coalescer.observe(None, context, "busy", EditorPublicationState::Busy);
        assert!(coalescer.pending_observation.is_none());
        assert!(coalescer.last_state.is_none());

        let prepared = std::sync::Arc::new(
            hex_engine::PreparedRevision::prepare(
                &hex_project::Project::new(48_000).unwrap(),
                &std::collections::BTreeMap::new(),
                std::sync::Arc::new(hex_engine::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let observation = std::sync::Arc::new(
            hex_engine::PreparedPlaybackObservation::prepare_on_control(&prepared).unwrap(),
        );
        let metadata = hex_session::service::ResidentPublicationMetadata {
            source_origin: hex_session::service::ResidentSourceOrigin::CanonicalCandidate,
            clip_preview: None,
            context,
            device_preview: None,
            tuning_preview: None,
            seek_request: None,
            stopped_restore: false,
            computer_keyboard_request: None,
            gain_recording_track: None,
            requires_preroll: false,
        };

        // Resident submission lands in primary slot; a subsequent status notice
        // coalesces into pending_status_observation without overwriting it.
        coalescer.observe(
            Some(9),
            context,
            "submitted",
            EditorPublicationState::ResidentSubmitted {
                audio_revision: 42,
                metadata,
                observation,
            },
        );
        coalescer.observe(Some(9), context, "busy", EditorPublicationState::Busy);
        assert!(matches!(
            coalescer.pending_observation,
            Some(EditorAudioEvent::Publication {
                output_identity: 9,
                state: EditorPublicationState::ResidentSubmitted {
                    audio_revision: 42,
                    ..
                },
                ..
            })
        ));
        assert!(matches!(
            coalescer.pending_status_observation,
            Some(EditorAudioEvent::Publication {
                state: EditorPublicationState::Busy,
                ..
            })
        ));

        // Repeating the same state is a no-op.
        coalescer.observe(Some(9), context, "busy", EditorPublicationState::Busy);
        assert!(!coalescer.flush());
        assert!(matches!(
            rx.try_recv().unwrap(),
            EditorAudioEvent::Publication {
                state: EditorPublicationState::ResidentSubmitted {
                    audio_revision: 42,
                    ..
                },
                ..
            }
        ));
        assert!(matches!(
            rx.try_recv().unwrap(),
            EditorAudioEvent::Publication {
                state: EditorPublicationState::Busy,
                ..
            }
        ));
        assert!(coalescer.is_quiescent());
    }

    #[test]
    fn flush_respects_priority_order_backpressure_and_disconnect() {
        let (tx, rx) = mpsc::sync_channel(1);
        let mut coalescer = EventCoalescer::new(tx);
        coalescer.pending_route = Some(EditorAudioEvent::OutputRouteChanged { output_identity: 3 });
        coalescer.pending_release = Some(EditorAudioEvent::Unavailable {
            project_generation: 1,
            restart_request: None,
            error: "release".into(),
        });

        // Channel capacity is 1: release flushes first, route stays queued.
        assert!(!coalescer.flush());
        assert!(coalescer.pending_release.is_none());
        assert!(coalescer.pending_route.is_some());
        assert!(matches!(
            rx.try_recv().unwrap(),
            EditorAudioEvent::Unavailable { .. }
        ));

        // Second flush delivers route.
        assert!(!coalescer.flush());
        assert!(coalescer.is_quiescent());
        assert!(matches!(
            rx.try_recv().unwrap(),
            EditorAudioEvent::OutputRouteChanged { output_identity: 3 }
        ));

        // Disconnected receiver reports true on flush.
        drop(rx);
        coalescer.pending_route = Some(EditorAudioEvent::OutputRouteChanged { output_identity: 4 });
        assert!(coalescer.flush());
    }

    #[test]
    fn clear_for_relinquish_preserves_editor_retirement_issue_and_clears_others() {
        let (tx, _rx) = mpsc::sync_channel(2);
        let mut coalescer = EventCoalescer::new(tx);
        coalescer.pending_route = Some(EditorAudioEvent::OutputRouteChanged { output_identity: 7 });
        coalescer.pending_parameter = Some(EditorAudioEvent::PluginEditorRetirementIssue {
            project_generation: 1,
            device: 7,
            issue: hex_ui::PluginEditorRetirementIssue::NeedsDisposition,
        });
        coalescer.clear_for_relinquish();
        assert!(coalescer.pending_route.is_none());
        assert!(matches!(
            coalescer.pending_parameter,
            Some(EditorAudioEvent::PluginEditorRetirementIssue { .. })
        ));
    }
}
