//! Worker-owned, source-fact-free native-effect parameter projection. Derive the
//! audio Project from the actual canonical source through the ordinary validated
//! device command; never promote an arbitrary frontend/preview Project to source
//! authority. Native constructor cells, readers, graphs and leases stay external.
use super::*;
use crate::device_gesture::{GestureKey, GesturePhase, GestureTarget};

pub struct VerifiedDeviceParameterSource {
    canonical: Arc<CanonicalAudioSourceSnapshot>,
    preview: Option<(DevicePreviewIdentity, Arc<Project>)>,
    terminal: Option<(GestureKey, u64)>,
}
impl VerifiedDeviceParameterSource {
    pub(super) fn capture(
        event: &Event,
        previous: Option<(GestureKey, u64)>,
    ) -> Result<Option<Self>, SessionError> {
        let Some(canonical) = event
            .state
            .canonical_audio_source
            .as_ref()
            .filter(|source| source.audio_mode() == DocumentAudioMode::VerifiedStreaming)
        else {
            return Ok(None);
        };
        if let Some(active) = event.active_device_gesture
            && let GestureTarget::DeviceParameter { device, parameter } = active.target
        {
            let Some(preview) = event.device_preview.as_ref() else {
                return Ok(None);
            };
            let identity = preview.identity;
            if identity.project_generation != canonical.context().project_generation
                || identity.key.generation != canonical.context().project_generation
                || identity.key.token == 0
                || preview.state.revision != event.state.revision
                || preview.state.canonical_audio_source.is_some()
                || identity.canonical_revision != event.state.revision
                || event.state.audio_authoring_revision(canonical.context())
                    != Some(event.state.revision)
                || identity.key != active.key
                || identity.sequence != active.sequence
            {
                return Err(SessionError::Invalid(
                    "device preview no longer matches the current audio",
                ));
            }
            let original = canonical.project_metadata();
            if Session::native_device_class_for_project(original, device)?
                != hex_engine::NativeDeviceClass::Effect
            {
                return Err(SessionError::Invalid(
                    "live parameter preview is unavailable for this device",
                ));
            }
            let command = crate::device_gesture::device_parameter_command(
                original,
                device,
                parameter,
                active.value,
                active.manual_edit,
            )?;
            let project = original.preview_device_edit(&command)?;
            let before = original
                .device(device)
                .ok_or(SessionError::Invalid("device no longer exists"))?;
            let after = project
                .device(device)
                .ok_or(SessionError::Invalid("device no longer exists"))?;
            if before.state().type_id() != after.state().type_id()
                || before.state().asset_references() != after.state().asset_references()
                || preview.state.project.device(device).is_none_or(|shown| {
                    shown.state() != after.state() || shown.automation() != after.automation()
                })
            {
                return Err(SessionError::Invalid(
                    "device preview no longer matches the current parameter",
                ));
            }
            let source = Self {
                canonical: Arc::clone(canonical),
                preview: Some((identity, Arc::new(project))),
                terminal: None,
            };
            source.candidate_reservation_quote().map_err(|_| {
                SessionError::Invalid("live parameter preview is unavailable for this device")
            })?;
            return Ok(Some(source));
        }
        if event.device_preview.is_some()
            && event.active_device_gesture.is_some_and(|active| {
                matches!(
                    active.target,
                    GestureTarget::DeviceParameterPair { .. }
                        | GestureTarget::DeviceParameterSnapshot { .. }
                        | GestureTarget::EqNodeCreation { .. }
                )
            })
        {
            return Err(SessionError::Invalid(
                "live preview is unavailable for this control in streaming documents",
            ));
        }
        let Some((key, sequence)) = previous else {
            return Ok(None);
        };
        // A refused commit can retire the owned gesture too. Active-key absence
        // plus the worker's delivered predecessor proves that terminal; a future
        // refused key does not. Restore canonical audio in either retired case.
        let Some(ack) = event.gesture_ack.as_ref().filter(|ack| {
            matches!(ack.phase, GesturePhase::Commit | GesturePhase::Cancel)
                && ack.key == key
                && key.token != 0
                && key.generation == canonical.context().project_generation
                && !event
                    .active_device_gesture
                    .is_some_and(|active| active.key == key)
        }) else {
            return Ok(None);
        };
        Ok(Some(Self {
            canonical: Arc::clone(canonical),
            preview: None,
            terminal: Some((ack.key, sequence)),
        }))
    }
    pub fn canonical(&self) -> &Arc<CanonicalAudioSourceSnapshot> {
        &self.canonical
    }
    pub fn preview_identity(&self) -> Option<DevicePreviewIdentity> {
        self.preview.as_ref().map(|(identity, _)| *identity)
    }
    pub fn terminal_key(&self) -> Option<GestureKey> {
        self.terminal.map(|(key, _)| key)
    }
    pub fn terminal_sequence(&self) -> Option<u64> {
        self.terminal.map(|(_, sequence)| sequence)
    }
    fn project(&self) -> &Project {
        self.preview.as_ref().map_or_else(
            || self.canonical.project_metadata(),
            |(_, project)| project.as_ref(),
        )
    }
    /// Reuse the canonical finite constructor/class admission. An overlay adds
    /// no source ID, asset reference, private sampler or source-owner capability.
    pub fn candidate_reservation_quote(
        &self,
    ) -> Result<hex_engine::AudioCandidateReservationQuote, hex_engine::AudioStorageError> {
        if hex_engine::native_device_resources::requires_private_sampler_sources(self.project()) {
            return Err(hex_engine::AudioStorageError::UnsupportedGraph);
        }
        hex_engine::AudioCandidateReservationQuote::for_project(
            self.project(),
            self.canonical.required_sources().count(),
        )
    }
    /// Only the existing reserved cell constructs a graph. Context and catalog
    /// remain anchored to the genuine canonical source; no resident fallback.
    pub fn prepare_candidate_on_control(
        &self,
        catalog: u64,
        destination: &mut hex_engine::AudioCandidatePreparation,
    ) -> Result<(), hex_engine::AudioStorageError> {
        destination.prepare_on_control(self.project(), catalog, self.canonical.context())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device_gesture::{GestureRequest, ParameterId, ParameterValue};
    struct Sink(Arc<std::sync::atomic::AtomicUsize>);
    impl WorkerAudioSink for Sink {
        fn canonical_changed(&mut self, _: Arc<CanonicalAudioSourceSnapshot>) {}
        fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
            Ok(48_000)
        }
        fn verified_device_parameter_source(&mut self, _: Arc<VerifiedDeviceParameterSource>) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::Release);
        }
        fn poll(&mut self) {}
        fn close(&mut self) {}
    }
    fn display_state(old: &ViewState, project: Arc<Project>) -> Arc<ViewState> {
        Arc::new(ViewState {
            canonical_audio_source: old.canonical_audio_source.clone(),
            comparison_audio: old.comparison_audio.clone(),
            project,
            waveforms: old.waveforms.clone(),
            source_overviews: old.source_overviews.clone(),
            audio_clip_waveforms: old.audio_clip_waveforms.clone(),
            midi_preview: old.midi_preview.clone(),
            gain_preview: old.gain_preview.clone(),
            timeline_frames: old.timeline_frames,
            can_undo: old.can_undo,
            can_redo: old.can_redo,
            revision: old.revision,
            unsaved: old.unsaved,
            document_path: old.document_path.clone(),
        })
    }
    fn send(service: &Service, request: Request) -> Event {
        service.submit(request).unwrap();
        let event = service
            .events
            .recv_timeout(crate::test_wait::HANG_BOUND)
            .unwrap();
        assert!(event.error.is_none(), "{:?}", event.error);
        event
    }
    #[test]
    fn canonical_delta_identity_and_owned_terminal_are_independent_proofs() {
        let service = Service::new(48_000, 16, 0).unwrap();
        service
            .events
            .recv_timeout(crate::test_wait::HANG_BOUND)
            .unwrap();
        send(
            &service,
            Request::NewWithAudioMode {
                mode: DocumentAudioMode::VerifiedStreaming,
                reservation: Some(crate::SourcePlanReservation {
                    reader: hex_media::VerifiedWavReaderLimits::new(
                        hex_media::WavReaderLimits::new(64 * 1024 * 1024, 8_000_000, 32, 4096),
                        1024 * 1024,
                    ),
                    max_retained_sources: 1,
                    max_path_bytes: 1024 * 1024,
                    max_manifest_bytes: 1024 * 1024,
                }),
            },
        );
        let added = send(
            &service,
            Request::Edit(Command::AddTrack {
                name: "width".into(),
            }),
        );
        let track = added.state.project.tracks()[0].id();
        let mut inserted = send(
            &service,
            Request::Edit(Command::InsertDevice {
                track,
                before: None,
                state: hex_engine::default_native_device_state(
                    hex_engine::DeviceTypeId::from_static("hex.utility"),
                )
                .unwrap(),
            }),
        );
        let Some(hex_project::Created::Device(device)) = inserted.created else {
            panic!("utility")
        };
        for id in [2, 3] {
            inserted = send(
                &service,
                Request::Edit(Command::ReplaceDeviceAutomationTarget {
                    device,
                    target: hex_lattice_contract::DeviceAutomationTarget::Parameter(
                        ParameterId::from_static(id),
                    ),
                    points: vec![hex_project::AutomationPoint {
                        beat: hex_project::BeatTime::ZERO,
                        value: hex_project::AutomationValue::new(0.4).unwrap(),
                        outgoing: hex_project::AutomationInterpolation::Step,
                    }],
                }),
            );
        }
        let key = GestureKey {
            generation: inserted
                .state
                .canonical_audio_source
                .as_ref()
                .unwrap()
                .context()
                .project_generation,
            token: 1,
        };
        send(
            &service,
            Request::DeviceGesture(GestureRequest::Begin {
                key,
                target: GestureTarget::DeviceParameter {
                    device,
                    parameter: ParameterId::from_static(3),
                },
            }),
        );
        let mut changed = send(
            &service,
            Request::DeviceGesture(GestureRequest::Change {
                key,
                sequence: 1,
                value: ParameterValue::Float(2.0),
            }),
        );
        let source = VerifiedDeviceParameterSource::capture(&changed, None)
            .unwrap()
            .unwrap();
        assert!(Arc::ptr_eq(
            source.canonical(),
            inserted.state.canonical_audio_source.as_ref().unwrap()
        ));
        assert_eq!(
            source.project(),
            changed
                .device_preview
                .as_ref()
                .unwrap()
                .state
                .project
                .as_ref()
        );
        assert_eq!(changed.state.project, inserted.state.project);
        let lanes = source.project().device(device).unwrap().automation();
        assert_eq!(
            lanes
                .iter()
                .find(|l| l.target
                    == hex_lattice_contract::DeviceAutomationTarget::Parameter(
                        ParameterId::from_static(3)
                    ))
                .unwrap()
                .read_selection,
            hex_project::AutomationReadSelection::Base
        );
        assert_eq!(
            lanes
                .iter()
                .find(|l| l.target
                    == hex_lattice_contract::DeviceAutomationTarget::Parameter(
                        ParameterId::from_static(2)
                    ))
                .unwrap()
                .read_selection,
            hex_project::AutomationReadSelection::Authored
        );
        let target = changed.active_device_gesture.unwrap().target;
        changed.active_device_gesture.as_mut().unwrap().target =
            GestureTarget::DeviceParameterPair {
                device,
                first: ParameterId::from_static(3),
                second: ParameterId::from_static(2),
            };
        assert!(VerifiedDeviceParameterSource::capture(&changed, None).is_err());
        changed.active_device_gesture.as_mut().unwrap().target = target;
        let valid_state = changed.device_preview.as_ref().unwrap().state.clone();
        let started = std::time::Instant::now();
        for _ in 0..1024 {
            std::hint::black_box(
                VerifiedDeviceParameterSource::capture(&changed, None)
                    .unwrap()
                    .unwrap(),
            );
        }
        eprintln!(
            "verified device canonical projection: 1024 one-track/two-lane captures {:?} (unoptimized control-only)",
            started.elapsed()
        );
        // Display metadata cannot smuggle a second audio edit into this projection.
        let shown = changed.device_preview.as_mut().unwrap();
        let mut display = hex_project::Editor::new((*shown.state.project).clone(), 8).unwrap();
        display
            .execute(Command::Gain { track, gain: 0.25 })
            .unwrap();
        shown.state = display_state(&shown.state, Arc::new(display.project().clone()));
        let isolated = VerifiedDeviceParameterSource::capture(&changed, None)
            .unwrap()
            .unwrap();
        assert_eq!(isolated.project().track(track).unwrap().gain(), 1.0);
        changed.device_preview.as_mut().unwrap().identity.sequence += 1;
        assert!(VerifiedDeviceParameterSource::capture(&changed, None).is_err());
        changed.device_preview.as_mut().unwrap().identity.sequence -= 1;
        let shown = changed.device_preview.as_mut().unwrap();
        shown.state = display_state(&shown.state, inserted.state.project.clone());
        assert!(VerifiedDeviceParameterSource::capture(&changed, None).is_err());
        assert!(
            inserted
                .state
                .project
                .preview_device_edit(&Command::Gain { track, gain: 0.5 })
                .is_err()
        );
        let cancelled = send(
            &service,
            Request::DeviceGesture(GestureRequest::Cancel { key }),
        );
        assert!(
            VerifiedDeviceParameterSource::capture(&cancelled, None)
                .unwrap()
                .is_none()
        );
        let terminal = VerifiedDeviceParameterSource::capture(&cancelled, Some((key, 1)))
            .unwrap()
            .unwrap();
        assert_eq!(terminal.terminal_key(), Some(key));
        assert_eq!(terminal.project(), inserted.state.project.as_ref());
        service
            .submit(Request::DeviceGesture(GestureRequest::Cancel {
                key: GestureKey { token: 99, ..key },
            }))
            .unwrap();
        let future = service
            .events
            .recv_timeout(crate::test_wait::HANG_BOUND)
            .unwrap();
        assert!(!future.gesture_ack.as_ref().unwrap().accepted);
        assert!(
            VerifiedDeviceParameterSource::capture(&future, Some((key, 1)))
                .unwrap()
                .is_none()
        );
        // The worker projection reaches its sink before a full UI event queue
        // can accept the matching display event.
        changed.device_preview.as_mut().unwrap().state = valid_state;
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        sender.send(future).unwrap();
        let delivered = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut worker = WorkerEvents::new(
            sender,
            Arc::new(AtomicBool::new(false)),
            Some(Box::new(Sink(delivered.clone()))),
        );
        let thread = std::thread::spawn(move || worker.send_with_control_poll(changed, |_| {}));
        let deadline = std::time::Instant::now() + crate::test_wait::HANG_BOUND;
        while delivered.load(std::sync::atomic::Ordering::Acquire) == 0 {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        receiver.recv_timeout(crate::test_wait::HANG_BOUND).unwrap();
        assert!(thread.join().unwrap());
        assert!(
            receiver
                .recv_timeout(crate::test_wait::HANG_BOUND)
                .unwrap()
                .device_preview
                .is_some()
        );
    }
}
