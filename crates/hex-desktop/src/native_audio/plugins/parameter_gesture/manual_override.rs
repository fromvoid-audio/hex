//! Actual applied-worker hold and isolated canonical capture for one exact
//! processor. Release drains on the ordinary control poll even after UI refusal.
use super::*;

pub(super) struct LiveOverride {
    pub(super) source: AppliedAudioSource,
    pub(super) transport_epoch: u64,
    pub(super) class: [u8; 16],
    pub(super) reading: bool,
    pub(super) uses_hold: bool,
    pub(super) playing: bool,
    pub(super) hold: Option<NativeWorkerParameterChange>,
    release: Option<NativeWorkerParameterChange>,
    scratch: Option<ManualBaseCapture>,
    pub(super) invalidated: bool,
    pub(super) discard: bool,
}

impl LiveOverride {
    pub(super) fn new(
        source: AppliedAudioSource,
        transport_epoch: u64,
        class: [u8; 16],
        reading: bool,
        uses_hold: bool,
        playing: bool,
    ) -> Self {
        Self {
            source,
            transport_epoch,
            class,
            reading,
            uses_hold,
            playing,
            hold: None,
            release: None,
            scratch: None,
            invalidated: false,
            discard: false,
        }
    }

    fn poll_release(&mut self, control: &mut PublicationControl, binding: Binding) -> bool {
        if let Some(token) = self.release.as_ref() {
            match poll_input(control, binding, token) {
                Ok(NativeWorkerParameterProgress::Pending) | Err("native preparation is busy") => {
                    return false;
                }
                Ok(_) => {}
                Err(reason) if source_gone(reason) => {}
                Err(_) => return false,
            }
            self.release = None;
            self.hold = None;
        }
        let Some(hold) = self.hold.as_ref() else {
            return true;
        };
        match control.native_source_parameter_hold_on_control(
            self.source,
            binding,
            NativeParameterHold::Release { hold },
        ) {
            Ok(token) => self.release = Some(token),
            Err(reason) if source_gone(reason) => {
                self.hold = None;
                return true;
            }
            Err(_) => {}
        }
        false
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AcceptedCommitRelease {
    Wait,
    ReleaseHold,
    ClearDirectly,
}

fn accepted_commit_release(
    binding_active: bool,
    endpoint: Option<(bool, bool)>,
    processed: bool,
    playing: bool,
) -> AcceptedCommitRelease {
    if !binding_active || endpoint.is_none() {
        return AcceptedCommitRelease::ClearDirectly;
    }
    let quiescent = endpoint.is_some_and(|(_, live)| !live);
    if processed || quiescent || !playing {
        AcceptedCommitRelease::ReleaseHold
    } else {
        AcceptedCommitRelease::Wait
    }
}

impl Plugins {
    pub(in crate::native_audio) fn override_publication_source(
        &self,
        successor: AudioSourceContext,
    ) -> Option<(AppliedAudioSource, u64, bool)> {
        if let Some(replay) = &self.retiring_replay_capture
            && let Some(source) = replay.isolated_source(successor)
        {
            return Some(source);
        }
        let gesture = self.parameter_gesture.as_ref()?;
        let (predecessor, _) = gesture.commit?;
        let live = gesture.live.as_ref()?;
        (successor.project_generation == predecessor.project_generation
            && predecessor.editor_revision.checked_add(1) == Some(successor.editor_revision))
        .then_some((live.source, live.transport_epoch, live.playing))
    }

    pub(in crate::native_audio::plugins) fn parameter_override_cleanup_pending(&self) -> bool {
        self.parameter_gesture.as_ref().is_some_and(|gesture| {
            gesture.live.as_ref().is_some_and(|live| {
                (live.invalidated || gesture.commit_accepted)
                    && (gesture.pending.is_some() || live.hold.is_some() || live.scratch.is_some())
            })
        })
    }

    pub(in crate::native_audio::plugins) fn poll_parameter_override(
        &mut self,
        control: &mut PublicationControl,
        context: Option<AudioSourceContext>,
    ) {
        self.collect_parameter_observations();
        let binding_active = self.parameter_gesture.as_ref().is_some_and(|gesture| {
            self.sources.iter().any(|source| {
                source.binding == gesture.binding && source.accepted && !source.retiring
            })
        });
        let Some(gesture) = self.parameter_gesture.as_mut() else {
            return;
        };
        if gesture.has_retired_delivery() {
            return;
        }
        if gesture
            .pending
            .as_ref()
            .is_some_and(|pending| pending.delivery.is_some())
            && gesture.poll_point(control).is_err()
            && let Some(live) = gesture.live.as_mut()
        {
            live.invalidated = true;
            live.discard = true;
        }
        let Some(live) = gesture.live.as_mut() else {
            return;
        };
        if gesture.commit_accepted {
            let processed = gesture.next_program.is_some_and(|expected| {
                control
                    .native_source_processed_program_on_control(gesture.binding)
                    .filter(|received| received.binding == gesture.binding)
                    .and_then(|received| received.program)
                    .is_some_and(|actual| actual >= expected)
            });
            let endpoint = control.native_source_endpoint_status(gesture.binding);
            let playing = control
                .status
                .playing
                .load(std::sync::atomic::Ordering::Acquire);
            match accepted_commit_release(binding_active, endpoint, processed, playing) {
                AcceptedCommitRelease::ClearDirectly => {
                    self.parameter_gesture = None;
                }
                AcceptedCommitRelease::ReleaseHold => {
                    if live.poll_release(control, gesture.binding) {
                        self.parameter_gesture = None;
                    }
                }
                AcceptedCommitRelease::Wait => {}
            }
            return;
        }
        if gesture.commit.is_none()
            && let Some(scratch) = live.scratch.as_mut()
            && scratch.poll_on_control(control, &mut || true).is_some()
        {
            // Capture cleanup is independent of the retained audible hold.
            live.scratch = None;
        }
        if gesture.commit.is_none() {
            let source_changed = context != Some(live.source.context)
                || control.status.recording_source() != Some(live.source);
            live.discard |= source_changed
                || (!live.playing
                    && live.reading
                    && control
                        .status
                        .playing
                        .load(std::sync::atomic::Ordering::Acquire));
            live.invalidated |= control.status.native_parameter_source(live.playing)
                != Some((live.source, live.transport_epoch));
        }
        if gesture.recording && live.invalidated {
            // The original recording scope ended even when the output itself
            // remains open (Stop, seek, source replacement or removal).
            live.discard = true;
        }
        let discard = live.discard;
        let invalidated = live.invalidated;
        if gesture.recording && invalidated {
            gesture.recording_output_closed();
        }
        let finished = invalidated && gesture.poll_override_cleanup(control) && discard;
        self.collect_parameter_observations();
        if finished {
            self.parameter_gesture = None;
        }
    }
}

impl Gesture {
    pub(super) fn restore_override(
        &mut self,
        control: &mut PublicationControl,
    ) -> Result<Prepared, Error> {
        if let Some(live) = self.live.as_mut() {
            live.invalidated = true;
        }
        let deadline = Instant::now() + Duration::from_millis(150);
        loop {
            if self.poll_override_cleanup(control) {
                return Ok(Prepared::Applied);
            }
            if Instant::now() >= deadline {
                return Err(refused("vst3 automation read is still being restored"));
            }
            edits::poll_control(control);
        }
    }

    fn poll_override_cleanup(&mut self, control: &mut PublicationControl) -> bool {
        if let Some(pending) = self.pending.as_ref() {
            if pending.value != self.origin.initial {
                let _ = control.withdraw_native_source_parameter_change_on_control(
                    self.binding,
                    &pending.token,
                );
            }
            let _ = self.poll_point(control);
        }
        let initial = self.sdk_value(self.origin.initial);
        let Some(live) = self.live.as_mut() else {
            return true;
        };
        if let Some(scratch) = live.scratch.as_mut()
            && scratch.poll_on_control(control, &mut || true).is_some()
        {
            live.scratch = None;
        }
        // Cancel explicitly restores a target without a reading lane, including
        // a newly armed target still absent from the actual incoming programme.
        // Keep that restoration inside its hold. Idle targets use ordinary input.
        // A running reading target releases its authored hold.
        if !(live.discard || live.reading && live.playing)
            && self.pending.is_none()
            && self.last != self.origin.initial
        {
            let restored = if let Some(hold) = live.hold.as_ref() {
                control.native_source_parameter_hold_on_control(
                    live.source,
                    self.binding,
                    NativeParameterHold::Change {
                        hold,
                        normalized: initial,
                    },
                )
            } else {
                control.begin_native_source_parameter_change_on_control(
                    self.binding,
                    self.origin.parameter,
                    initial,
                )
            };
            match restored {
                Ok(token) => {
                    self.pending = Some(PendingPoint {
                        token,
                        value: self.origin.initial,
                        sequence: self.sequence,
                        opening_hold: false,
                        delivery: None,
                        processed: false,
                        sdk_finished: false,
                        #[cfg(all(test, feature = "allocation-probe"))]
                        hold_sdk_ack: false,
                    })
                }
                Err(reason) if source_gone(reason) => live.discard = true,
                Err(_) => {}
            }
            return false;
        }
        let released = self.pending.is_none() && live.poll_release(control, self.binding);
        released && live.scratch.is_none()
    }

    pub(super) fn capture_override(
        &mut self,
        control: &mut PublicationControl,
        host: &Hosting,
        canonical: &NativeDeviceState,
        cancellation: Option<&RequestCancellation>,
    ) -> Result<Prepared, Error> {
        let actual = self.sdk_value(self.last);
        let live = self.live.as_mut().ok_or_else(lost)?;
        if (live.playing && live.uses_hold && live.hold.is_none()) || live.invalidated {
            return Err(refused("vst3 automation hold is unavailable"));
        }
        let status = std::sync::Arc::clone(&control.status);
        let expected = (live.source, live.transport_epoch);
        let mut cancelled = || {
            cancellation.is_some_and(RequestCancellation::is_cancelled)
                || status.native_parameter_source(live.playing) != Some(expected)
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if cancelled() {
                return Err(lost());
            }
            if live.scratch.is_none() {
                let beginning = if live.playing {
                    host.begin_manual_base_capture_on_control(
                        control,
                        live.source,
                        self.binding,
                        canonical,
                        self.origin.parameter,
                        actual,
                    )
                } else {
                    host.begin_idle_manual_base_capture_on_control(
                        control,
                        live.source,
                        self.binding,
                        canonical,
                        self.origin.parameter,
                        actual,
                    )
                };
                match beginning {
                    Ok(scratch) => live.scratch = Some(scratch),
                    Err(reason)
                        if matches!(
                            reason.as_str(),
                            "native preparation is busy" | "native source is busy"
                        ) => {}
                    Err(reason) => return Err(refused(reason)),
                }
            }
            if let Some(scratch) = live.scratch.as_mut()
                && let Some(terminal) = scratch.poll_on_control(control, &mut cancelled)
            {
                live.scratch = None;
                return terminal
                    .map_err(|error| refused(error.to_string()))
                    .map(|base| {
                        Prepared::OverrideCaptured(Box::new(PluginParameterOverride {
                            source: live.source.context,
                            class: live.class,
                            parameter: self.origin.parameter,
                            base,
                        }))
                    });
            }
            if Instant::now() >= deadline {
                return Err(refused("vst3 manual base capture is still settling"));
            }
            edits::poll_control(control);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepted_commit_release_clears_missing_retired_superseded_and_releases_on_pause() {
        // Playing with active live endpoint and unprocessed revision waits.
        assert_eq!(
            accepted_commit_release(true, Some((true, true)), false, true),
            AcceptedCommitRelease::Wait
        );
        // Once the audio quantum processes the new program revision, release hold.
        assert_eq!(
            accepted_commit_release(true, Some((true, true)), true, true),
            AcceptedCommitRelease::ReleaseHold
        );
        // Endpoint no longer live (quiescent/degraded) releases hold.
        assert_eq!(
            accepted_commit_release(true, Some((true, false)), false, true),
            AcceptedCommitRelease::ReleaseHold
        );
        // Paused transport (!playing) releases hold instead of stalling until resume.
        assert_eq!(
            accepted_commit_release(true, Some((true, true)), false, false),
            AcceptedCommitRelease::ReleaseHold
        );
        // Stop (epoch renewal) leaves endpoint None -> clear directly (no deadlock).
        assert_eq!(
            accepted_commit_release(true, None, false, false),
            AcceptedCommitRelease::ClearDirectly
        );
        assert_eq!(
            accepted_commit_release(false, None, false, true),
            AcceptedCommitRelease::ClearDirectly
        );
        // Device removal / superseding marks source retiring or removes it -> clear directly.
        assert_eq!(
            accepted_commit_release(false, Some((true, true)), false, true),
            AcceptedCommitRelease::ClearDirectly
        );
    }

    #[cfg(all(
        target_os = "macos",
        target_arch = "aarch64",
        feature = "allocation-probe"
    ))]
    #[test]
    fn poll_parameter_override_clears_accepted_commit_on_stop_removal_and_pause() {
        let _serial = hex_audio::worker_fixture::serial_output();
        let (mut output, backend) =
            hex_audio::worker_fixture::open_after_retirement(48_000, false).unwrap();
        let control = output.publication.control().unwrap();
        let context = AudioSourceContext {
            project_generation: 1,
            editor_revision: 2,
        };
        let mut editor =
            hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 4).unwrap();
        let hex_project::Created::Device(device_id) = editor
            .execute(hex_project::Command::InsertMainDevice {
                before: None,
                state: hex_engine::default_native_device_state(
                    hex_engine::DeviceTypeId::from_static("hex.utility"),
                )
                .unwrap(),
            })
            .unwrap()
        else {
            panic!("device");
        };
        let binding = Binding {
            root: control.output_identity().unwrap_or(1),
            instance: device_id.value(),
            graph: 2,
            epoch: 1,
        };
        let applied = AppliedAudioSource {
            context,
            audio_revision: 2,
            output_identity: binding.root,
        };
        let make_gesture = || Gesture {
            key: GestureKey {
                generation: 1,
                token: 1,
            },
            origin: PluginParameterOrigin {
                context,
                device: device_id,
                parameter: 0,
                page_first: 0,
                metadata_revision: 1,
                initial: PluginParameterValue::new(0.25).unwrap(),
            },
            binding,
            output: binding.root,
            steps: 0,
            last: PluginParameterValue::new(0.5).unwrap(),
            sequence: 1,
            pending: None,
            live: Some(LiveOverride::new(applied, 1, [1; 16], true, true, true)),
            capture: None,
            capture_sequenced: false,
            last_input: None,
            last_input_idle: false,
            sequenced_capture: None,
            before: None,
            commit: Some((context, 1)),
            next_program: Some(std::num::NonZeroU64::new(5).unwrap()),
            commit_accepted: true,
            capture_class: None,
            deliveries: Vec::new(),
            retired_delivery: None,
            recording: false,
            recording_base: None,
            recorded_release: false,
            recording_context: None,
            recording_closed: false,
            recording_interrupted: false,
        };

        // Case 1: Stop/epoch renewal or device removal (no active matching source / endpoint None).
        let mut plugins = Plugins {
            parameter_gesture: Some(make_gesture()),
            ..Plugins::default()
        };
        plugins.poll_parameter_override(control, Some(context));
        assert!(plugins.parameter_gesture.is_none());
        assert!(plugins.idle());

        // Case 2: Source still present in self.sources but marked retiring (removed/superseded).
        plugins.sources.push(Source {
            binding,
            provider: None,
            owner: ProcessorChainOwner::Main,
            device: device_id.value(),
            project_generation: 1,
            operation: SourceOperation::Reopen,
            authored: None,
            accepted: false,
            retiring: true,
            queued: true,
            observes_latency: false,
            editor_open: false,
            editor_transfer: false,
            editor_open_on_insert: false,
            edit_backlog: false,
            edit_recovery: None,
            open_editor_group: None,
            editor_retirement: Default::default(),
            pending_touch: None,
            observed_latency_receipt: false,
            attempts: 0,
        });
        plugins.parameter_gesture = Some(make_gesture());
        plugins.poll_parameter_override(control, Some(context));
        assert!(plugins.parameter_gesture.is_none());

        // Case 3: Paused transport (!playing) with accepted source clears without stalling.
        plugins.sources[0].accepted = true;
        plugins.sources[0].retiring = false;
        plugins.parameter_gesture = Some(make_gesture());
        control
            .status
            .playing
            .store(false, std::sync::atomic::Ordering::Release);
        plugins.poll_parameter_override(control, Some(context));
        assert!(plugins.parameter_gesture.is_none());
        assert!(plugins.idle());

        drop(backend);
        output.publication.close();
    }
}
