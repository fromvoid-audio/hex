use super::*;
use std::sync::{
    Mutex,
    atomic::{AtomicU8, AtomicU64},
};

mod return_removal;

const PENDING: u8 = 0;
const INSTALLED: u8 = 1;
const CANCELLED: u8 = 2;
const REFUSED: u8 = 3;
const EXPIRED: u8 = 4;
const OUTPUT_CLOSED: u8 = 5;
const SOURCE_CHANGED: u8 = 6;

#[derive(Debug)]
struct FixtureReceipt {
    submission: ResidentGraphSubmission,
    state: AtomicU8,
    deadline: Instant,
}

impl DeviceInsertAudioReceipt for Arc<FixtureReceipt> {
    fn submission(&self) -> ResidentGraphSubmission {
        self.submission
    }

    fn state(&self) -> DeviceInsertAudioState {
        if Instant::now() >= self.deadline {
            let _ =
                self.state
                    .compare_exchange(PENDING, EXPIRED, Ordering::AcqRel, Ordering::Acquire);
        }
        match self.state.load(Ordering::Acquire) {
            PENDING => DeviceInsertAudioState::Pending,
            INSTALLED => DeviceInsertAudioState::Installed,
            CANCELLED => DeviceInsertAudioState::Cancelled,
            EXPIRED => DeviceInsertAudioState::Expired,
            OUTPUT_CLOSED => DeviceInsertAudioState::OutputClosed,
            SOURCE_CHANGED => DeviceInsertAudioState::Refused(
                hex_engine::PlaybackHandoffRejection::RuntimeFenceChanged,
            ),
            REFUSED => DeviceInsertAudioState::Refused(
                hex_engine::PlaybackHandoffRejection::DeviceInsertNeedsConfirmation,
            ),
            _ => panic!("unknown fixture receipt state"),
        }
    }

    fn cancel(&self) -> bool {
        self.state
            .compare_exchange(PENDING, CANCELLED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
}

#[derive(Default)]
struct Fixture {
    active: Option<ResidentRuntimeLoop>,
    candidate: Option<Arc<FixtureReceipt>>,
    offer_deadline: Option<Instant>,
    revision: u64,
    pending_parameters: Option<PluginParameterResult>,
    pending_conversion: Option<PluginParameterConversionResult>,
    provide_stopped_power_fence: bool,
    observed_power_on: Vec<Option<ResidentPowerOn>>,
}

impl WorkerAudioSink for Arc<Mutex<Fixture>> {
    fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
        Ok(48_000)
    }

    fn canonical_changed(&mut self, _: Arc<CanonicalAudioSourceSnapshot>) {}
    fn close(&mut self) {}
    fn poll(&mut self) {}

    fn resident_publication_enabled(&self) -> bool {
        true
    }

    fn prepare_resident_effects(
        &mut self,
        input: ResidentEffectPreparation<'_>,
        prepare: &mut dyn FnMut(
            &hex_engine::AudioCandidateEffects<'_, '_>,
        ) -> Result<PreparedRevision, hex_engine::Invalid>,
    ) -> Result<Option<PreparedRevision>, String> {
        self.lock()
            .unwrap()
            .observed_power_on
            .push(input.power_on());
        let mut first_vst3 = None;
        let _ = input.visit_plugin_devices(|dest, device_id, _| {
            if first_vst3.is_none() {
                first_vst3 = Some((dest.owner(), device_id));
            }
            Ok(())
        });
        if let Some((owner, device_id)) = first_vst3 {
            hex_engine::AudioCandidateEffects::with_test_foreign_effect_program(
                owner,
                device_id,
                hex_lattice_contract::ForeignProcessorBinding {
                    root: 1,
                    instance: device_id,
                    graph: input.context().editor_revision.max(1),
                    epoch: 1,
                },
                std::num::NonZeroU64::new(1).unwrap(),
                0,
                prepare,
            )
            .map(Some)
            .map_err(|error| error.to_string())
        } else {
            hex_engine::AudioCandidateEffects::with_host_definitions(&[], prepare)
                .map(Some)
                .map_err(|error| error.to_string())
        }
    }

    fn stopped_resident_power_fence(
        &mut self,
        context: hex_engine::AudioSourceContext,
        _: u64,
    ) -> Result<Option<ResidentFreshRuntimeFence>, String> {
        let fixture = self.lock().unwrap();
        if !fixture.provide_stopped_power_fence {
            return Ok(None);
        }
        let active = fixture
            .active
            .filter(|active| active.source_context == Some(context));
        Ok(active.map(|active| ResidentFreshRuntimeFence {
            output_identity: active.output_identity,
            active_audio_revision: active.audio_revision,
            route_generation: 0,
            admitted_command_ticket: 0,
            output_fault_epoch: 0,
            consumed_stop_generation: 1,
        }))
    }

    fn stopped_resident_return_power_on_fence(
        &mut self,
        context: hex_engine::AudioSourceContext,
        _: hex_engine::ProcessorChainOwner,
        _: u64,
    ) -> Result<Option<ResidentFreshRuntimeFence>, String> {
        self.stopped_resident_power_fence(context, 0)
    }

    fn confirms_device_inserts(&self) -> bool {
        true
    }

    fn resident_publication_readiness(&mut self) -> ResidentPublicationReadiness {
        self.lock()
            .unwrap()
            .active
            .map_or(ResidentPublicationReadiness::NoOutput, |submission| {
                ResidentPublicationReadiness::Applied {
                    submission,
                    publication_in_flight: false,
                }
            })
    }

    fn device_insert_receipt(
        &mut self,
        submission: ResidentGraphSubmission,
    ) -> Option<Box<dyn DeviceInsertAudioReceipt>> {
        let fixture = self.lock().unwrap();
        let receipt = fixture.candidate.as_ref()?;
        (receipt.submission == submission).then(|| Box::new(Arc::clone(receipt)) as Box<_>)
    }

    fn offer_device_insert_consent(
        &mut self,
        submission: ResidentGraphSubmission,
    ) -> Option<Instant> {
        let fixture = self.lock().unwrap();
        let receipt = fixture.candidate.as_ref()?;
        let deadline = fixture.offer_deadline?;
        (receipt.submission == submission
            && receipt.state.load(Ordering::Acquire) == REFUSED
            && Instant::now() < deadline)
            .then_some(deadline)
    }

    fn resident_runtime_loop(
        &mut self,
        _: hex_engine::AudioSourceContext,
    ) -> Result<Option<ResidentRuntimeLoop>, String> {
        Ok(self.lock().unwrap().active)
    }

    #[allow(clippy::result_large_err)]
    fn publish_resident_transition(
        &mut self,
        playback: Playback,
        resources: Option<&PreparedDeviceResources>,
        _: Arc<hex_engine::PreparedPlaybackObservation>,
        metadata: ResidentPublicationMetadata,
        policy: ResidentPublicationPolicy,
        metadata_limit: usize,
        admit: &mut dyn FnMut(&[DeviceResourceAdmission<'_>]) -> Result<(), String>,
        inputs: ResidentPublicationInputs<'_>,
    ) -> Result<ResidentPublicationAcceptance, ResidentPublicationRefusal> {
        let result = (|| {
            let captured = resources
                .ok_or("missing fixture resources")?
                .capture_on_control(metadata_limit)
                .map_err(|error| format!("{error:?}"))?;
            admit(&[DeviceResourceAdmission::unpublished_on_control(
                &playback,
                Some(&captured),
            )])
        })();
        if let Err(error) = result {
            return Err(ResidentPublicationRefusal {
                kind: ResidentPublicationFailure::Other,
                playback,
                error,
            });
        }
        if let ResidentPublicationPolicy::DeviceInsert {
            confirmation: Some(refused),
            ..
        } = policy
            && self.offer_device_insert_consent(refused).is_none()
        {
            return Err(ResidentPublicationRefusal {
                kind: ResidentPublicationFailure::Other,
                playback,
                error: "device insert confirmation is no longer current".into(),
            });
        }
        let mut fixture = self.lock().unwrap();
        fixture.revision += 1;
        let observation = ResidentRuntimeLoop {
            output_identity: 51,
            audio_revision: fixture.revision,
            source_context: Some(metadata.context),
            source_origin: metadata.source_origin,
            section_loop: None,
        };
        if let ResidentPublicationPolicy::DeviceInsert { predecessor, .. } = policy {
            assert_eq!(Some(predecessor), fixture.active);
            let receipt = Arc::new(FixtureReceipt {
                submission: ResidentGraphSubmission {
                    output_identity: observation.output_identity,
                    audio_revision: observation.audio_revision,
                    context: metadata.context,
                },
                state: AtomicU8::new(PENDING),
                deadline: Instant::now() + Duration::from_millis(250),
            });
            if let Some(control) = inputs.device_insert_control {
                control.bind_device_insert_receipt(Box::new(Arc::clone(&receipt)));
            }
            fixture.candidate = Some(receipt);
        } else {
            fixture.active = Some(observation);
        }
        Ok(ResidentPublicationAcceptance::Submitted {
            output_identity: observation.output_identity,
            audio_revision: observation.audio_revision,
        })
    }

    fn plugin_parameters_ready(&self) -> bool {
        self.lock().unwrap().pending_parameters.is_some()
    }

    fn take_plugin_parameters(&mut self) -> Option<PluginParameterResult> {
        self.lock().unwrap().pending_parameters.take()
    }

    fn plugin_parameter_conversion_ready(&self) -> bool {
        self.lock().unwrap().pending_conversion.is_some()
    }

    fn take_plugin_parameter_conversion(&mut self) -> Option<PluginParameterConversionResult> {
        self.lock().unwrap().pending_conversion.take()
    }
}

fn service() -> (Service, Arc<Mutex<Fixture>>, Event) {
    let fixture = Arc::new(Mutex::new(Fixture::default()));
    let max = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES;
    let service = Service::with_audio_sink_and_native_reservation(
        16,
        1024,
        None,
        Box::new(Arc::clone(&fixture)),
        crate::ResidentNativeReservation {
            storage: hex_engine::AudioStorageLedgerReservation {
                data_rows: 512,
                handle_rows: 64,
                root_rows: 64,
                max_retained_bytes: max * 2,
                max_construction_peak_bytes: max * 3,
            },
            max_scan_frames: 1024,
        },
    )
    .unwrap();
    let initial = receive(&service);
    (service, fixture, initial)
}

fn receive(service: &Service) -> Event {
    let event = service
        .events
        .recv_timeout(crate::test_wait::HANG_BOUND)
        .unwrap();
    assert!(event.error.is_none(), "{:?}", event.error);
    event
}

fn install_candidate(service: &Service, fixture: &Mutex<Fixture>) -> Event {
    let submission = {
        let mut fixture = fixture.lock().unwrap();
        let receipt = fixture.candidate.as_ref().unwrap();
        assert_eq!(receipt.state.swap(INSTALLED, Ordering::AcqRel), PENDING);
        let submission = receipt.submission;
        fixture.active = Some(ResidentRuntimeLoop {
            output_identity: submission.output_identity,
            audio_revision: submission.audio_revision,
            source_context: Some(submission.context),
            source_origin: ResidentSourceOrigin::CanonicalCandidate,
            section_loop: None,
        });
        submission
    };
    let installed = receive(service);
    assert!(installed.request_complete);
    assert_eq!(
        installed.device_insert_complete,
        Some(DeviceInsertCompletion {
            submission,
            outcome: DeviceInsertOutcome::Installed
        })
    );
    installed
}

fn insert_utility(service: &Service) -> Event {
    let state = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap();
    service
        .submit(Request::Edit(Command::InsertMainDevice {
            before: None,
            state,
        }))
        .unwrap();
    let pending = receive(service);
    assert!(!pending.request_complete);
    assert!(pending.state.project.main_devices().is_empty());
    assert!(!pending.state.can_undo);
    pending
}

fn offer_insert(service: &Service, fixture: &Mutex<Fixture>) -> DeviceInsertConfirmation {
    insert_utility(service);
    let refused = {
        let mut fixture = fixture.lock().unwrap();
        fixture.offer_deadline = Some(Instant::now() + Duration::from_secs(30));
        fixture.candidate.clone().unwrap()
    };
    refused.state.store(REFUSED, Ordering::Release);
    let offered = receive(service);
    assert!(!offered.request_complete);
    assert!(offered.device_insert_complete.is_none());
    assert!(offered.state.project.main_devices().is_empty());
    assert!(!offered.state.can_undo);
    let offer = offered.device_insert_confirmation.unwrap();
    assert_eq!(offer.submission(), refused.submission);
    offer
}

struct SavedFile(PathBuf);

impl SavedFile {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "hex-device-insert-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for SavedFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn builtin_browser_proposal_cancel_fences_the_callback_and_completes_exactly_once() {
    let (service, fixture, initial) = service();
    let initial_runtime = fixture.lock().unwrap().active;
    let insert = |control| Request::DropBrowserDeviceOnArrangement {
        project_generation: 0,
        revision: initial.state.revision,
        class: hex_engine::NativeDeviceClass::Instrument,
        type_id: hex_lattice_contract::DeviceTypeId::from_static("hex.instrument.operator"),
        destination: ArrangementDeviceDestination::NewTrack { before_track: None },
        new_track_gain: None,
        control: Some(control),
    };
    let control = Arc::new(PluginEffectControl::default());
    service.submit(insert(Arc::clone(&control))).unwrap();
    let staged = receive(&service);
    assert!(staged.error.is_none(), "{:?}", staged.error);
    assert!(!staged.request_complete);
    assert!(staged.device_insert_proposal_complete.is_none());
    assert_eq!(staged.state.project, initial.state.project);
    let exact = fixture.lock().unwrap().candidate.clone().unwrap();
    assert!(control.cancel());
    // The callback tries immediately after Cancel, before Session polling.
    assert!(
        exact
            .state
            .compare_exchange(PENDING, INSTALLED, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
    );
    let refused = receive(&service);
    let terminal = refused.device_insert_proposal_complete.unwrap();
    assert!(Arc::ptr_eq(&terminal.control, &control));
    assert!(!terminal.accepted);
    assert_eq!(
        refused.device_insert_complete.unwrap().outcome,
        DeviceInsertOutcome::Cancelled
    );
    assert_eq!(refused.state.project, initial.state.project);
    assert_eq!(fixture.lock().unwrap().active, initial_runtime);

    service.submit(insert(control)).unwrap();
    let replayed = service
        .events
        .recv_timeout(crate::test_wait::HANG_BOUND)
        .unwrap();
    assert!(replayed.error.unwrap().contains("already submitted"));
    assert!(replayed.device_insert_proposal_complete.is_none());
    assert_eq!(replayed.state.project, initial.state.project);

    let committed = Arc::new(PluginEffectControl::default());
    service.submit(insert(Arc::clone(&committed))).unwrap();
    assert!(!receive(&service).request_complete);
    let receipt = fixture.lock().unwrap().candidate.clone().unwrap();
    assert_eq!(
        receipt
            .state
            .compare_exchange(PENDING, INSTALLED, Ordering::AcqRel, Ordering::Acquire),
        Ok(PENDING)
    );
    assert!(!committed.cancel());
    let accepted = receive(&service);
    let terminal = accepted.device_insert_proposal_complete.unwrap();
    assert!(Arc::ptr_eq(&terminal.control, &committed));
    assert!(terminal.accepted);
    assert_eq!(
        accepted.device_insert_complete.unwrap().submission,
        receipt.submission
    );
    assert_eq!(
        accepted.state.project.tracks().len(),
        initial.state.project.tracks().len() + 1
    );
}

#[test]
fn catalog_proposal_keeps_selection_and_the_exact_terminal_for_each_entry_point() {
    use hex_engine::NativeDeviceClass;
    use hex_lattice_contract::DeviceTypeId;

    enum Inbox {
        BrowserEnter,
        Bus,
        TrackRail,
        TrackPicker,
        ReplaceInstrument,
    }
    for inbox in [
        Inbox::BrowserEnter,
        Inbox::Bus,
        Inbox::TrackRail,
        Inbox::TrackPicker,
        Inbox::ReplaceInstrument,
    ] {
        let (service, fixture, _) = service();
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "existing".into(),
            }))
            .unwrap();
        let mut original = receive(&service);
        let track = original.state.project.tracks()[0].id();
        let prior_instrument = if matches!(inbox, Inbox::ReplaceInstrument) {
            let state = hex_engine::default_native_device_state_for_class(
                NativeDeviceClass::Instrument,
                DeviceTypeId::from_static("hex.instrument.sine"),
            )
            .unwrap();
            service
                .submit(Request::Edit(Command::InsertInstrument { track, state }))
                .unwrap();
            let seeded = receive(&service);
            assert!(!seeded.request_complete && seeded.error.is_none());
            original = install_candidate(&service, &fixture);
            assert!(
                original.request_complete && original.error.is_none(),
                "{:?}",
                original.error
            );
            Some(
                original
                    .state
                    .project
                    .track(track)
                    .unwrap()
                    .instrument()
                    .unwrap()
                    .id(),
            )
        } else {
            None
        };
        let original_runtime = fixture.lock().unwrap().active;
        let insert = |control| match inbox {
            Inbox::BrowserEnter => Request::InsertBrowserInstrumentTrack {
                project_generation: 0,
                revision: original.state.revision,
                class: NativeDeviceClass::Instrument,
                type_id: DeviceTypeId::from_static("hex.instrument.operator"),
                new_track_gain: Some(0.5),
                control: Some(control),
            },
            Inbox::Bus => Request::InsertEffectBusDevice {
                project_generation: 0,
                revision: original.state.revision,
                destination: EffectBusDestination::Main,
                class: NativeDeviceClass::Effect,
                type_id: DeviceTypeId::from_static("hex.utility"),
                before: None,
                control: Some(control),
            },
            Inbox::TrackRail => Request::DropNativeDevice {
                project_generation: 0,
                revision: original.state.revision,
                source: NativeDeviceDropSource::Browser {
                    class: NativeDeviceClass::Effect,
                    type_id: DeviceTypeId::from_static("hex.utility"),
                },
                track,
                before: None,
                replace: None,
                control: Some(control),
            },
            Inbox::TrackPicker | Inbox::ReplaceInstrument => {
                let (placement, class, type_id) = match prior_instrument {
                    Some(device) => (
                        TrackCatalogPlacement::ReplaceInstrument(device),
                        NativeDeviceClass::Instrument,
                        DeviceTypeId::from_static("hex.instrument.operator"),
                    ),
                    None => (
                        TrackCatalogPlacement::Insert { before: None },
                        NativeDeviceClass::Effect,
                        DeviceTypeId::from_static("hex.utility"),
                    ),
                };
                Request::LoadTrackCatalogDevice {
                    project_generation: 0,
                    revision: original.state.revision,
                    track,
                    class,
                    type_id,
                    placement,
                    control,
                }
            }
        };
        if matches!(inbox, Inbox::TrackPicker | Inbox::ReplaceInstrument) {
            let control = Arc::new(PluginEffectControl::default());
            let mut stale = insert(Arc::clone(&control));
            if let Request::LoadTrackCatalogDevice { revision, .. } = &mut stale {
                *revision += 1;
            }
            service.submit(stale).unwrap();
            let refused = service
                .events
                .recv_timeout(crate::test_wait::HANG_BOUND)
                .unwrap();
            assert!(
                refused.request_complete
                    && refused.error.as_ref().unwrap().contains("project changed")
            );
            assert_eq!(refused.state.project, original.state.project);
            let completion = refused.device_insert_proposal_complete.unwrap();
            assert!(!completion.accepted && Arc::ptr_eq(&completion.control, &control));
        }
        for cancel in [true, false] {
            let control = Arc::new(PluginEffectControl::default());
            service.submit(insert(Arc::clone(&control))).unwrap();
            let staged = receive(&service);
            assert!(!staged.request_complete);
            assert!(staged.created.is_none() && staged.device_insert_proposal_complete.is_none());
            assert_eq!(staged.state.project, original.state.project);
            let receipt = fixture.lock().unwrap().candidate.clone().unwrap();
            if cancel {
                assert!(control.cancel());
                assert_eq!(receipt.state(), DeviceInsertAudioState::Cancelled);
            } else {
                assert_eq!(
                    receipt.state.compare_exchange(
                        PENDING,
                        INSTALLED,
                        Ordering::AcqRel,
                        Ordering::Acquire
                    ),
                    Ok(PENDING)
                );
                assert!(!control.cancel());
            }
            let terminal = receive(&service);
            let proposal = terminal.device_insert_proposal_complete.unwrap();
            assert!(Arc::ptr_eq(&proposal.control, &control));
            assert_eq!(proposal.accepted, !cancel);
            assert_eq!(
                terminal.device_insert_complete.unwrap().submission,
                receipt.submission
            );
            if cancel {
                assert_eq!(terminal.state.project, original.state.project);
                assert_eq!(fixture.lock().unwrap().active, original_runtime);
            } else {
                match inbox {
                    Inbox::BrowserEnter => {
                        let Created::Track(id) = terminal.created.unwrap() else {
                            panic!("created track")
                        };
                        assert_eq!(terminal.state.project.track(id).unwrap().gain(), 0.5);
                    }
                    Inbox::Bus => {
                        assert_eq!(terminal.state.project.main_devices().len(), 1);
                        assert!(matches!(terminal.created, Some(Created::Device(_))));
                    }
                    Inbox::TrackRail => {
                        assert_eq!(
                            terminal.state.project.track(track).unwrap().devices().len(),
                            1
                        );
                        assert!(terminal.created.is_none());
                    }
                    Inbox::TrackPicker => {
                        let track = terminal.state.project.track(track).unwrap();
                        assert_eq!(track.devices().len(), 1);
                        assert_eq!(
                            terminal.created,
                            Some(Created::Device(track.devices()[0].id()))
                        );
                    }
                    Inbox::ReplaceInstrument => {
                        let device = terminal
                            .state
                            .project
                            .track(track)
                            .unwrap()
                            .instrument()
                            .unwrap();
                        assert_eq!(Some(device.id()), prior_instrument);
                        assert_eq!(
                            device.state().type_id(),
                            DeviceTypeId::from_static("hex.instrument.operator")
                        );
                        assert!(terminal.created.is_none());
                    }
                }
            }
        }
    }
}

#[test]
fn browser_device_insertions_wait_for_the_exact_install_receipt() {
    let (service, fixture, _) = service();
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "audio".into(),
        }))
        .unwrap();
    let original = receive(&service);
    let track = original.state.project.tracks()[0].id();
    let class = hex_engine::NativeDeviceClass::Effect;
    let type_id = hex_lattice_contract::DeviceTypeId::from_static("hex.eq");

    let drop_on_track = |revision| Request::DropNativeDevice {
        project_generation: 0,
        revision,
        source: NativeDeviceDropSource::Browser { class, type_id },
        track,
        before: None,
        replace: None,
        control: None,
    };
    service
        .submit(drop_on_track(original.state.revision))
        .unwrap();
    let staged = receive(&service);
    assert!(!staged.request_complete);
    assert_eq!(staged.state.project, original.state.project);
    assert_eq!(staged.state.revision, original.state.revision);

    let (candidate, predecessor) = {
        let fixture = fixture.lock().unwrap();
        (fixture.candidate.clone().unwrap(), fixture.active)
    };
    candidate.state.store(REFUSED, Ordering::Release);
    let refused = service
        .events
        .recv_timeout(crate::test_wait::HANG_BOUND)
        .unwrap();
    assert!(refused.request_complete);
    assert!(refused.error.is_some());
    assert_eq!(refused.state.project, original.state.project);
    assert_eq!(refused.state.revision, original.state.revision);
    assert_eq!(fixture.lock().unwrap().active, predecessor);

    let mut installed = refused;
    let requests = [
        drop_on_track(original.state.revision),
        Request::AddNativeDevice {
            track,
            class,
            type_id: Box::new(type_id),
            before: None,
        },
        Request::DropBrowserDeviceOnArrangement {
            project_generation: 0,
            revision: original.state.revision + 2,
            class,
            type_id,
            destination: ArrangementDeviceDestination::NewTrack { before_track: None },
            new_track_gain: None,
            control: None,
        },
    ];
    for (index, request) in requests.into_iter().enumerate() {
        service.submit(request).unwrap();
        let staged = receive(&service);
        assert!(!staged.request_complete);
        assert_eq!(staged.state.project, installed.state.project);
        assert_eq!(staged.state.revision, installed.state.revision);

        let submission = {
            let mut fixture = fixture.lock().unwrap();
            let receipt = fixture.candidate.as_ref().unwrap();
            let submission = receipt.submission;
            assert_eq!(
                receipt.state.compare_exchange(
                    PENDING,
                    INSTALLED,
                    Ordering::AcqRel,
                    Ordering::Acquire
                ),
                Ok(PENDING)
            );
            fixture.active = Some(ResidentRuntimeLoop {
                output_identity: submission.output_identity,
                audio_revision: submission.audio_revision,
                source_context: Some(submission.context),
                source_origin: ResidentSourceOrigin::CanonicalCandidate,
                section_loop: None,
            });
            submission
        };
        let prior = installed;
        installed = receive(&service);
        assert!(installed.request_complete);
        assert_eq!(installed.state.revision, prior.state.revision + 1);
        assert_eq!(
            installed.device_insert_complete,
            Some(DeviceInsertCompletion {
                submission,
                outcome: DeviceInsertOutcome::Installed
            })
        );
        if index == 2 {
            assert!(
                installed.created.is_none(),
                "arrangement drop keeps track focus"
            );
            let new_track = installed.state.project.tracks().last().unwrap();
            assert_eq!(new_track.devices()[0].state().type_id(), type_id);
        } else {
            assert_eq!(
                installed
                    .state
                    .project
                    .track(track)
                    .unwrap()
                    .devices()
                    .len(),
                index + 1
            );
        }
    }
}

#[test]
fn native_moves_wait_for_the_exact_audio_receipt_and_preserve_noops() {
    let (service, fixture, _) = service();
    let run = |request| {
        service.submit(request).unwrap();
        receive(&service)
    };
    let add_track = |name: &str| {
        let Created::Track(track) = run(Request::Edit(Command::AddTrack { name: name.into() }))
            .created
            .unwrap()
        else {
            panic!("track");
        };
        track
    };
    let source = add_track("source");
    let destination = add_track("destination");
    let class = hex_engine::NativeDeviceClass::Effect;
    let type_id = hex_lattice_contract::DeviceTypeId::from_static("hex.utility");
    let add = || {
        assert!(
            !run(Request::AddNativeDevice {
                track: source,
                class,
                type_id: Box::new(type_id),
                before: None,
            })
            .request_complete
        );
        install_candidate(&service, &fixture)
    };
    add();
    let original = add().state;
    let devices = original.project.track(source).unwrap().devices();
    let first = devices[0].id();
    let second = devices[1].id();
    let reorder = |before| Request::ReorderDevices {
        project_generation: 0,
        revision: original.revision,
        owner: hex_project::DeviceChainOwner::Track(source),
        selected: hex_project::DeviceSelectionIds::new(&[first]).unwrap(),
        before,
    };
    let pending = run(reorder(None));
    assert!(!pending.request_complete);
    assert_eq!(pending.state.project, original.project);
    let predecessor = {
        let mut fixture = fixture.lock().unwrap();
        fixture.offer_deadline = Some(Instant::now() + Duration::from_secs(3));
        fixture
            .candidate
            .as_ref()
            .unwrap()
            .state
            .store(REFUSED, Ordering::Release);
        fixture.active
    };
    let offer = receive(&service).device_insert_confirmation.unwrap();
    assert!(offer.cancel());
    let cancelled = receive(&service);
    assert_eq!(
        cancelled.device_insert_complete.unwrap().outcome,
        DeviceInsertOutcome::Cancelled
    );
    assert_eq!(cancelled.state.revision, original.revision);
    assert_eq!(cancelled.state.project, original.project);
    assert_eq!(fixture.lock().unwrap().active, predecessor);

    let unchanged = run(reorder(Some(second)));
    assert!(unchanged.request_complete);
    assert_eq!(unchanged.state.revision, original.revision);
    assert_eq!(unchanged.state.project, original.project);
    assert_eq!(fixture.lock().unwrap().active, predecessor);

    let pending = run(Request::MoveTrackDevices {
        project_generation: 0,
        revision: original.revision,
        source,
        selected: hex_project::DeviceSelectionIds::new(&[second, first]).unwrap(),
        destination,
    });
    assert!(!pending.request_complete);
    assert_eq!(pending.state.project, original.project);
    let moved = install_candidate(&service, &fixture).state;
    assert_eq!(moved.project.track(destination).unwrap().devices(), devices);
    assert_eq!(moved.revision, original.revision + 1);
    assert_eq!(run(Request::Undo).state.project, original.project);
    assert_eq!(run(Request::Redo).state.project, moved.project);

    let pending = run(Request::DropNativeDevice {
        project_generation: 0,
        revision: moved.revision + 2,
        source: NativeDeviceDropSource::Existing {
            device: first,
            class,
            type_id,
        },
        track: source,
        before: None,
        replace: None,
        control: None,
    });
    assert!(!pending.request_complete);
    assert_eq!(pending.state.project, moved.project);
    let dropped = install_candidate(&service, &fixture);
    assert!(dropped.created.is_none());
    assert_eq!(
        dropped.state.project.track(source).unwrap().devices(),
        &devices[..1]
    );
    assert_eq!(run(Request::Undo).state.project, moved.project);
}

#[test]
fn native_move_history_waits_for_each_exact_install_and_can_retry_after_cancel() {
    for layout in [
        NativeMoveLayout::Delay,
        NativeMoveLayout::UtilityDelay,
        NativeMoveLayout::Sine,
        NativeMoveLayout::Vector,
        NativeMoveLayout::Sampler,
    ] {
        assert_native_move_history(layout);
    }
}

enum NativeMoveLayout {
    Delay,
    UtilityDelay,
    Sine,
    Vector,
    Sampler,
}

fn assert_native_move_history(layout: NativeMoveLayout) {
    let (service, fixture, _) = service();
    let run = |request| {
        service.submit(request).unwrap();
        receive(&service)
    };
    let add_track = |name: &str| {
        let added = run(Request::Edit(Command::AddTrack { name: name.into() }));
        let Created::Track(track) = added.created.unwrap() else {
            panic!("track");
        };
        (track, added.state.revision)
    };
    let (source, _) = add_track("source");
    let (destination, setup_revision) = add_track("destination");
    if matches!(layout, NativeMoveLayout::UtilityDelay) {
        assert!(
            !run(Request::AddNativeDevice {
                track: destination,
                class: hex_engine::NativeDeviceClass::Effect,
                type_id: Box::new(hex_lattice_contract::DeviceTypeId::from_static(
                    "hex.utility"
                )),
                before: None,
            })
            .request_complete
        );
        install_candidate(&service, &fixture);
    }
    let (class, type_id) = match layout {
        NativeMoveLayout::Sine => (
            hex_engine::NativeDeviceClass::Instrument,
            "hex.instrument.sine",
        ),
        NativeMoveLayout::Vector => (
            hex_engine::NativeDeviceClass::Instrument,
            "hex.instrument.operator",
        ),
        NativeMoveLayout::Sampler => (
            hex_engine::NativeDeviceClass::Instrument,
            "hex.instrument.sampler",
        ),
        NativeMoveLayout::Delay | NativeMoveLayout::UtilityDelay => {
            (hex_engine::NativeDeviceClass::Effect, "hex.delay")
        }
    };
    let files = matches!(layout, NativeMoveLayout::Sampler)
        .then(|| crate::service::sampler_export::tests::Files::new(false));
    let original = if let Some(files) = files.as_ref() {
        let loaded = run(Request::LoadFirstSampler {
            request_id: 1,
            project_generation: 0,
            revision: setup_revision,
            target: crate::service::SamplerLoadTarget::Track {
                track: source,
                slot: crate::service::SamplerLoadSlot::New(
                    hex_project::InstrumentLoadPlacement::First,
                ),
            },
            path: files.source().to_owned(),
            cancel: Arc::new(AtomicBool::new(false)),
        });
        assert!(loaded.request_complete);
        loaded.state
    } else {
        assert!(
            !run(Request::AddNativeDevice {
                track: source,
                class,
                type_id: Box::new(hex_lattice_contract::DeviceTypeId::from_static(type_id)),
                before: None,
            })
            .request_complete
        );
        install_candidate(&service, &fixture).state
    };
    let moved_device = |project: &hex_project::Project, track| {
        let track = project.track(track).unwrap();
        if class == hex_engine::NativeDeviceClass::Instrument {
            track.instrument().unwrap().id()
        } else {
            track.devices().last().unwrap().id()
        }
    };
    let device = moved_device(&original.project, source);
    assert!(
        !run(Request::MoveTrackDevices {
            project_generation: 0,
            revision: original.revision,
            source,
            selected: hex_project::DeviceSelectionIds::new(&[device]).unwrap(),
            destination,
        })
        .request_complete
    );
    let moved = install_candidate(&service, &fixture).state;
    assert_eq!(moved_device(&moved.project, destination), device);

    let pending = run(Request::Undo);
    assert!(!pending.request_complete);
    assert_eq!(pending.state.project, moved.project);
    assert_eq!(pending.state.revision, moved.revision);
    {
        let fixture = fixture.lock().unwrap();
        let receipt = fixture.candidate.as_ref().unwrap();
        assert_eq!(receipt.state.swap(CANCELLED, Ordering::AcqRel), PENDING);
    }
    let cancelled = service
        .events
        .recv_timeout(crate::test_wait::HANG_BOUND)
        .unwrap();
    assert!(cancelled.request_complete);
    assert_eq!(
        cancelled.error.as_deref(),
        Some("device edit cancelled before installation")
    );
    assert_eq!(
        cancelled.device_insert_complete.unwrap().outcome,
        DeviceInsertOutcome::Cancelled
    );
    assert_eq!(cancelled.state.project, moved.project);
    assert_eq!(cancelled.state.revision, moved.revision);
    assert!(cancelled.state.can_undo);
    assert!(!cancelled.state.can_redo);

    let pending = run(Request::Undo);
    assert!(!pending.request_complete);
    assert_eq!(pending.state.project, moved.project);
    let undone = install_candidate(&service, &fixture).state;
    assert_eq!(undone.project, original.project);
    assert_eq!(undone.revision, moved.revision + 1);
    assert!(undone.can_redo);

    let pending = run(Request::Redo);
    assert!(!pending.request_complete);
    assert_eq!(pending.state.project, original.project);
    let redone = install_candidate(&service, &fixture).state;
    assert_eq!(redone.project, moved.project);
    assert_eq!(redone.revision, undone.revision + 1);
    assert!(!redone.can_redo);

    let mut compound = hex_project::Editor::new(original.project.as_ref().clone(), 4).unwrap();
    compound
        .execute_many_prepared(
            vec![
                Command::MoveTrackDevices {
                    source,
                    destination,
                    selected: hex_project::DeviceSelectionIds::new(&[device]).unwrap(),
                },
                Command::Gain {
                    track: destination,
                    gain: 0.5,
                },
            ],
            |_, _| Ok::<_, hex_project::Invalid>(()),
        )
        .unwrap();
    let compound_project = compound.project().clone();
    assert!(
        move_history::stage(&mut compound, false, false)
            .unwrap()
            .is_none()
    );
    assert_eq!(compound.project(), &compound_project);
    assert!(compound.can_undo());
    assert!(!compound.can_redo());
}

#[test]
fn cancelled_resident_paste_keeps_the_clipboard_for_one_confirmed_history_step() {
    use crate::service::DeviceClipboardAction;
    use hex_project::{DeviceChainOwner, DeviceSelectionIds};

    let (service, fixture, _) = service();
    let run = |request| {
        service.submit(request).unwrap();
        receive(&service)
    };
    insert_utility(&service);
    let original = install_candidate(&service, &fixture).state;
    let owner = DeviceChainOwner::Main;
    let device = &original.project.main_devices()[0];
    let clipboard = |action| Request::DeviceClipboard {
        project_generation: 0,
        revision: original.revision,
        action,
    };
    run(clipboard(DeviceClipboardAction::Copy {
        owner,
        selected: DeviceSelectionIds::new(&[device.id()]).unwrap(),
    }));
    let paste = || {
        clipboard(DeviceClipboardAction::Paste {
            owner,
            before: None,
        })
    };
    let pending = run(paste());
    assert!(!pending.request_complete);
    assert_eq!(pending.state.project, original.project);
    {
        let mut fixture = fixture.lock().unwrap();
        fixture.offer_deadline = Some(Instant::now() + Duration::from_secs(3));
        fixture
            .candidate
            .as_ref()
            .unwrap()
            .state
            .store(REFUSED, Ordering::Release);
    }
    let offer = receive(&service).device_insert_confirmation.unwrap();
    assert!(offer.cancel());
    let cancelled = receive(&service);
    assert_eq!(
        cancelled.device_insert_complete.unwrap().outcome,
        DeviceInsertOutcome::Cancelled
    );
    assert_eq!(cancelled.state.project, original.project);
    assert_eq!(cancelled.state.revision, original.revision);

    assert!(!run(paste()).request_complete);
    let pasted = install_candidate(&service, &fixture).state;
    assert_eq!(pasted.revision, original.revision + 1);
    let [retained, duplicate] = pasted.project.main_devices() else {
        panic!("original and pasted devices");
    };
    assert_eq!(retained, device);
    assert_ne!(duplicate.id(), device.id());
    assert_eq!(duplicate.state(), device.state());
    let undone = run(Request::Undo).state;
    assert_eq!(undone.project.main_devices(), std::slice::from_ref(device));
    assert!(undone.can_redo);
    assert_eq!(run(Request::Redo).state.project, pasted.project);
}

#[test]
fn native_insert_is_saved_and_undone_only_after_its_install_receipt() {
    let (service, fixture, initial) = service();
    let pending = insert_utility(&service);
    assert_eq!(pending.state.revision, initial.state.revision);

    let files = SavedFile::new();
    let path = files.0.join("session.hex");
    service.submit(Request::SaveNew(path.clone())).unwrap();
    assert!(receive(&service).save_complete);

    let mut fixture = fixture.lock().unwrap();
    let receipt = fixture.candidate.clone().unwrap();
    let submission = receipt.submission;
    assert_ne!(
        submission.audio_revision,
        fixture.active.unwrap().audio_revision
    );
    assert_eq!(
        receipt
            .state
            .compare_exchange(PENDING, INSTALLED, Ordering::AcqRel, Ordering::Acquire),
        Ok(PENDING)
    );
    fixture.active = Some(ResidentRuntimeLoop {
        output_identity: submission.output_identity,
        audio_revision: submission.audio_revision,
        source_context: Some(submission.context),
        source_origin: ResidentSourceOrigin::CanonicalCandidate,
        section_loop: None,
    });
    drop(fixture);

    let installed = receive(&service);
    assert!(installed.request_complete && installed.state.can_undo && installed.state.unsaved);
    assert_eq!(
        installed.device_insert_complete,
        Some(DeviceInsertCompletion {
            submission,
            outcome: DeviceInsertOutcome::Installed,
        }),
    );
    assert_eq!(installed.state.revision, initial.state.revision + 1);
    assert_eq!(installed.state.project.main_devices().len(), 1);
    assert!(hex_storage::load(&path).unwrap().main_devices().is_empty());

    service.submit(Request::SaveCurrent).unwrap();
    assert!(receive(&service).save_complete);
    assert_eq!(hex_storage::load(&path).unwrap().main_devices().len(), 1);

    service.submit(Request::Undo).unwrap();
    let undone = receive(&service);
    assert!(!undone.state.can_undo && undone.state.can_redo);
    assert!(undone.state.project.main_devices().is_empty());
    service.submit(Request::Redo).unwrap();
    assert_eq!(receive(&service).state.project, installed.state.project);
}

#[test]
fn refusal_and_expiry_keep_the_predecessor_and_leave_no_undo() {
    let (service, fixture, initial) = service();
    for refusal in [true, false] {
        insert_utility(&service);
        let (predecessor, candidate) = {
            let fixture = fixture.lock().unwrap();
            (fixture.active, fixture.candidate.clone().unwrap())
        };
        if refusal {
            candidate.state.store(REFUSED, Ordering::Release);
        }
        let terminal = service
            .events
            .recv_timeout(crate::test_wait::HANG_BOUND)
            .unwrap();
        assert!(terminal.request_complete && terminal.error.is_some());
        assert_eq!(
            terminal.device_insert_complete,
            Some(DeviceInsertCompletion {
                submission: candidate.submission,
                outcome: if refusal {
                    DeviceInsertOutcome::ConfirmationRequired
                } else {
                    DeviceInsertOutcome::Expired
                },
            }),
        );
        assert_eq!(terminal.state.revision, initial.state.revision);
        assert_eq!(terminal.state.project, initial.state.project);
        assert!(!terminal.state.can_undo);
        assert_eq!(fixture.lock().unwrap().active, predecessor);
        if !refusal {
            assert_eq!(
                candidate.state.compare_exchange(
                    PENDING,
                    INSTALLED,
                    Ordering::AcqRel,
                    Ordering::Acquire
                ),
                Err(EXPIRED),
            );
        }
    }
}

#[test]
fn confirmed_device_insert_commits_only_the_second_exact_receipt() {
    let (service, fixture, initial) = service();
    let offer = offer_insert(&service, &fixture);
    let files = SavedFile::new();
    let path = files.0.join("session.hex");
    service.submit(Request::SaveNew(path.clone())).unwrap();
    let saved = receive(&service);
    assert!(saved.save_complete && !saved.state.can_undo);
    assert!(hex_storage::load(&path).unwrap().main_devices().is_empty());
    assert!(offer.proceed());
    assert!(!offer.cancel());

    let deadline = Instant::now() + crate::test_wait::HANG_BOUND;
    let submission = loop {
        let mut fixture = fixture.lock().unwrap();
        let candidate = fixture.candidate.as_ref().unwrap();
        if candidate.submission != offer.submission() {
            let submission = candidate.submission;
            assert_eq!(submission.context, offer.submission().context);
            assert_eq!(
                submission.output_identity,
                offer.submission().output_identity
            );
            assert_eq!(
                candidate.state.compare_exchange(
                    PENDING,
                    INSTALLED,
                    Ordering::AcqRel,
                    Ordering::Acquire
                ),
                Ok(PENDING)
            );
            fixture.active = Some(ResidentRuntimeLoop {
                output_identity: submission.output_identity,
                audio_revision: submission.audio_revision,
                source_context: Some(submission.context),
                source_origin: ResidentSourceOrigin::CanonicalCandidate,
                section_loop: None,
            });
            break submission;
        }
        drop(fixture);
        assert!(
            Instant::now() < deadline,
            "confirmed candidate was not submitted"
        );
        thread::sleep(Duration::from_millis(1));
    };
    let committed = receive(&service);
    assert_eq!(
        committed.device_insert_complete,
        Some(DeviceInsertCompletion {
            submission,
            outcome: DeviceInsertOutcome::Installed
        }),
    );
    assert_eq!(committed.state.revision, initial.state.revision + 1);
    assert!(committed.request_complete && committed.state.can_undo);
    assert_eq!(committed.state.project.main_devices().len(), 1);
    service.submit(Request::Undo).unwrap();
    assert!(receive(&service).state.project.main_devices().is_empty());
}

#[test]
fn cancelled_or_stale_consent_leaves_audio_and_history_at_the_predecessor() {
    let (service, fixture, initial) = service();
    for stale in [false, true] {
        let offer = offer_insert(&service, &fixture);
        let predecessor = fixture.lock().unwrap().active;
        let outcome = if stale {
            fixture.lock().unwrap().offer_deadline = None;
            assert!(offer.proceed());
            DeviceInsertOutcome::Refused(hex_engine::PlaybackHandoffRejection::RuntimeFenceChanged)
        } else {
            assert!(offer.cancel());
            assert!(!offer.proceed());
            DeviceInsertOutcome::Cancelled
        };
        let terminal = service
            .events
            .recv_timeout(crate::test_wait::HANG_BOUND)
            .unwrap();
        assert_eq!(
            terminal.device_insert_complete,
            Some(DeviceInsertCompletion {
                submission: offer.submission(),
                outcome,
            })
        );
        assert!(terminal.request_complete && !terminal.state.can_undo);
        assert_eq!(terminal.state.revision, initial.state.revision);
        assert_eq!(terminal.state.project, initial.state.project);
        assert_eq!(fixture.lock().unwrap().active, predecessor);
    }
}

fn confirm_and_install(
    service: &Service,
    fixture: &Mutex<Fixture>,
    offer: &DeviceInsertConfirmation,
) -> Event {
    assert!(offer.is_pending());
    assert!(offer.proceed());
    let deadline = Instant::now() + crate::test_wait::HANG_BOUND;
    let submission = loop {
        let mut fixture = fixture.lock().unwrap();
        let candidate = fixture.candidate.as_ref().unwrap();
        if candidate.submission != offer.submission() {
            let submission = candidate.submission;
            assert_eq!(
                candidate.state.compare_exchange(
                    PENDING,
                    INSTALLED,
                    Ordering::AcqRel,
                    Ordering::Acquire
                ),
                Ok(PENDING)
            );
            fixture.active = Some(ResidentRuntimeLoop {
                output_identity: submission.output_identity,
                audio_revision: submission.audio_revision,
                source_context: Some(submission.context),
                source_origin: ResidentSourceOrigin::CanonicalCandidate,
                section_loop: None,
            });
            break submission;
        }
        drop(fixture);
        assert!(
            Instant::now() < deadline,
            "confirmed candidate was not submitted"
        );
        thread::sleep(Duration::from_millis(1));
    };
    let committed = receive(service);
    assert_eq!(
        committed.device_insert_complete,
        Some(DeviceInsertCompletion {
            submission,
            outcome: DeviceInsertOutcome::Installed,
        })
    );
    committed
}

#[test]
fn read_only_observations_do_not_cancel_pending_or_confirmed_device_insert() {
    let (service, fixture, _) = service();
    let stage_utility = || {
        let state = hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Effect,
            hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
        )
        .unwrap();
        service
            .submit(Request::Edit(Command::InsertMainDevice {
                before: None,
                state,
            }))
            .unwrap();
        let pending = receive(&service);
        assert!(!pending.request_complete);
        pending
    };
    stage_utility();
    let baseline = install_candidate(&service, &fixture).state;
    let existing_device = baseline.project.main_devices()[0].id();
    let sample_query = |revision, request_id| PluginParameterQuery {
        context: hex_engine::AudioSourceContext {
            project_generation: 0,
            editor_revision: revision,
        },
        device: existing_device,
        output_identity: 51,
        request_id,
        page_first: Some(0),
    };
    let sample_conversion = |revision, request_id| PluginParameterConversionQuery {
        context: hex_engine::AudioSourceContext {
            project_generation: 0,
            editor_revision: revision,
        },
        device: existing_device,
        output_identity: 51,
        request_id,
        source: hex_lattice_contract::ForeignProcessorBinding {
            root: 1,
            instance: 1,
            graph: 1,
            epoch: 1,
        },
        parameter: 7,
        operation: PluginParameterConversion::Format(0.5),
    };

    // Part A: Periodic VST3 parameter queries, their continuations, and other
    // read-only observations arriving while awaiting confirmation must not
    // cancel the offer.
    stage_utility();
    let refused = {
        let mut fixture = fixture.lock().unwrap();
        fixture.offer_deadline = Some(Instant::now() + Duration::from_secs(30));
        fixture.candidate.clone().unwrap()
    };
    refused.state.store(REFUSED, Ordering::Release);
    let offer = receive(&service).device_insert_confirmation.unwrap();

    let query = sample_query(baseline.revision, 1);
    service
        .submit(Request::QueryPluginParameters(query))
        .unwrap();
    let observed = receive(&service);
    assert!(observed.plugin_parameter_observation);
    assert_eq!(observed.plugin_parameters.unwrap().query, query);
    assert!(offer.is_pending());

    let async_query = sample_query(baseline.revision, 2);
    fixture.lock().unwrap().pending_parameters = Some(PluginParameterResult {
        query: async_query,
        source: None,
        total: 0,
        descriptor_signature: None,
        rows: Ok(Vec::new()),
    });
    let continued = receive(&service);
    assert!(continued.plugin_parameter_observation);
    assert_eq!(continued.plugin_parameters.unwrap().query, async_query);
    assert!(offer.is_pending());

    let conversion = sample_conversion(baseline.revision, 3);
    service
        .submit(Request::QueryPluginParameterConversion(conversion.clone()))
        .unwrap();
    let observed = receive(&service);
    assert!(observed.plugin_parameter_observation);
    assert_eq!(
        observed.plugin_parameter_conversion.unwrap().query,
        conversion
    );
    assert!(offer.is_pending());

    let async_conversion = sample_conversion(baseline.revision, 4);
    fixture.lock().unwrap().pending_conversion = Some(PluginParameterConversionResult {
        query: async_conversion.clone(),
        value: Ok(PluginParameterConvertedValue {
            normalized: 0.5,
            display: Some("0.5".into()),
        }),
    });
    let continued = receive(&service);
    assert!(continued.plugin_parameter_observation);
    assert_eq!(
        continued.plugin_parameter_conversion.unwrap().query,
        async_conversion
    );
    assert!(offer.is_pending());

    service
        .submit(Request::QueryFailedPluginAutomation { request_id: 5 })
        .unwrap();
    let automation = receive(&service);
    assert!(automation.plugin_parameter_observation);
    assert!(automation.device_automation_take_complete.is_some());
    assert!(offer.is_pending());

    let committed = confirm_and_install(&service, &fixture, &offer);
    assert_eq!(committed.state.project.main_devices().len(), 2);

    // Part B: A read-only query arriving before audio refuses for confirmation
    // must be served immediately rather than parked in device_insert_deferred
    // (which would cancel the confirmation as soon as it is offered).
    stage_utility();
    let pre_offer_query = sample_query(committed.state.revision, 6);
    service
        .submit(Request::QueryPluginParameters(pre_offer_query))
        .unwrap();
    let observed = receive(&service);
    assert!(observed.plugin_parameter_observation);
    assert_eq!(observed.plugin_parameters.unwrap().query, pre_offer_query);

    let refused = {
        let mut fixture = fixture.lock().unwrap();
        fixture.offer_deadline = Some(Instant::now() + Duration::from_secs(30));
        fixture.candidate.clone().unwrap()
    };
    refused.state.store(REFUSED, Ordering::Release);
    let offered = receive(&service);
    let offer = offered.device_insert_confirmation.unwrap();
    let committed = confirm_and_install(&service, &fixture, &offer);
    assert_eq!(committed.state.project.main_devices().len(), 3);

    // Part C: A competing mutating edit still cancels an unanswered
    // confirmation offer and then executes in FIFO order.
    stage_utility();
    let refused = {
        let mut fixture = fixture.lock().unwrap();
        fixture.offer_deadline = Some(Instant::now() + Duration::from_secs(30));
        fixture.candidate.clone().unwrap()
    };
    refused.state.store(REFUSED, Ordering::Release);
    let offer = receive(&service).device_insert_confirmation.unwrap();
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "after".into(),
        }))
        .unwrap();
    let cancelled = receive(&service);
    assert_eq!(
        cancelled.device_insert_complete,
        Some(DeviceInsertCompletion {
            submission: offer.submission(),
            outcome: DeviceInsertOutcome::Cancelled,
        })
    );
    assert!(!offer.is_pending());
    let added = receive(&service);
    assert!(added.request_complete);
    assert_eq!(added.state.project.tracks().len(), 1);
    assert_eq!(added.state.project.tracks()[0].name(), "after");
}

#[test]
fn undo_of_powered_off_main_and_return_vst3_routes_through_power_on_preparation() {
    let vst3 = hex_lattice_contract::NativeDeviceState::new(
        hex_lattice_contract::VST3_DEVICE_TYPE_ID,
        1,
        vec![9],
    )
    .unwrap();

    for on_return in [false, true] {
        let (service, fixture, _) = service();
        let device = if on_return {
            service
                .submit(Request::Edit(Command::AddReturnTrack { name: "fx".into() }))
                .unwrap();
            let bus = match receive(&service).created {
                Some(Created::ReturnTrack(bus)) => bus,
                other => panic!("expected return track, got {other:?}"),
            };
            service
                .submit(Request::Edit(Command::InsertReturnDevice {
                    return_track: bus,
                    before: None,
                    state: vst3.clone(),
                }))
                .unwrap();
            let event = receive(&service);
            let event = if event.request_complete {
                event
            } else {
                install_candidate(&service, &fixture)
            };
            match event.created {
                Some(Created::Device(device)) => device,
                other => panic!("expected return device, got {other:?}"),
            }
        } else {
            service
                .submit(Request::Edit(Command::InsertMainDevice {
                    before: None,
                    state: vst3.clone(),
                }))
                .unwrap();
            let event = receive(&service);
            let event = if event.request_complete {
                event
            } else {
                install_candidate(&service, &fixture)
            };
            match event.created {
                Some(Created::Device(device)) => device,
                other => panic!("expected main device, got {other:?}"),
            }
        };

        fixture.lock().unwrap().provide_stopped_power_fence = true;

        // Power off the sole Main/Return VST3.
        service
            .submit(Request::Edit(Command::SetDeviceBypass {
                device,
                bypassed: true,
            }))
            .unwrap();
        let off = receive(&service);
        let off = if off.request_complete {
            off
        } else {
            install_candidate(&service, &fixture)
        };
        assert!(off.error.is_none(), "{:?}", off.error);
        assert!(off.state.project.device(device).unwrap().bypassed());

        fixture.lock().unwrap().observed_power_on.clear();

        // Undo the power-off: must route through staged power-on preparation
        // with an explicit ResidentPowerOn intent, never falling through with
        // power_on: None.
        service.submit(Request::Undo).unwrap();
        let pending = receive(&service);
        assert!(
            !pending.request_complete,
            "Undo power-on must stage via DeviceInsert receipt"
        );
        let undone = install_candidate(&service, &fixture);
        assert!(undone.error.is_none(), "{:?}", undone.error);
        assert!(!undone.state.project.device(device).unwrap().bypassed());
        let power_ons = fixture.lock().unwrap().observed_power_on.clone();
        assert!(
            power_ons
                .iter()
                .flatten()
                .any(|intent| intent.device() == device.value()),
            "on_return={on_return}: Undo of powered-off VST3 must supply ResidentPowerOn, got {power_ons:?}"
        );
    }
}

#[test]
fn save_requests_defer_during_pending_device_insert_while_a_gesture_is_active() {
    use crate::device_gesture::{GestureKey, GestureRequest, GestureTarget};
    use crate::service::audio_sink::WorkerEvents;

    // Unit-level contract: all four Save* variants defer when a gesture is active,
    // and still serve immediately when no gesture is active.
    let save_requests = [
        Request::SaveCurrent,
        Request::SaveNew(std::path::PathBuf::from("new.hex")),
        Request::SaveProjectAs {
            path: std::path::PathBuf::from("as.hex"),
            extension_added: false,
        },
        Request::SaveCurrentWithFormatUpgrade {
            project_generation: 0,
            path: std::path::PathBuf::from("cur.hex"),
        },
    ];
    for request in &save_requests {
        assert!(
            WorkerEvents::served_during_device_insert(request, false),
            "save without active gesture must remain servable"
        );
        assert!(
            !WorkerEvents::served_during_device_insert(request, true),
            "save with active gesture must defer behind pending_device_insert"
        );
    }

    // End-to-end regression: SaveCurrent arriving while pending_device_insert is
    // pending AND a device_gesture is active must defer behind the staged edit,
    // never committing the gesture underneath it (no ProjectCommitFailed desync).
    let (service, fixture, _initial) = service();
    let utility = hex_engine::default_native_device_state(
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap();
    service
        .submit(Request::Edit(Command::InsertMainDevice {
            before: None,
            state: utility.clone(),
        }))
        .unwrap();
    let first_pending = receive(&service);
    assert!(!first_pending.request_complete);
    let inserted = install_candidate(&service, &fixture);
    let Some(Created::Device(first_device)) = inserted.created else {
        panic!("expected first utility device");
    };

    // Save once so SaveCurrent has a backing file path.
    let temp_dir =
        std::env::temp_dir().join(format!("hex-save-during-insert-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&temp_dir);
    let save_path = temp_dir.join("project.hex");
    let _ = std::fs::remove_file(&save_path);
    service.submit(Request::SaveNew(save_path.clone())).unwrap();
    let saved_initial = receive(&service);
    assert!(saved_initial.save_complete);

    // Start an active parameter gesture on first_device (uncommitted!).
    let gain_param = hex_lattice_contract::ParameterId::from_static(1);
    let key = GestureKey {
        generation: 0,
        token: 1,
    };
    service
        .submit(Request::DeviceGesture(GestureRequest::Begin {
            key,
            target: GestureTarget::DeviceParameter {
                device: first_device,
                parameter: gain_param,
            },
        }))
        .unwrap();
    let began = receive(&service);
    assert!(began.gesture_ack.as_ref().is_some_and(|ack| ack.accepted));

    let edited_gain = hex_lattice_contract::ParameterValue::Float(6.0);
    service
        .submit(Request::DeviceGesture(GestureRequest::Change {
            key,
            sequence: 1,
            value: edited_gain,
        }))
        .unwrap();
    let changed = receive(&service);
    assert!(changed.gesture_ack.as_ref().is_some_and(|ack| ack.accepted));

    // Real Resident audio reports Unknown source_origin for in-place parameter
    // gestures (e.g. VST3 or non-graph-replacing previews), allowing a staged
    // Request::Edit to begin while device_gesture remains active.
    if let Some(active) = fixture.lock().unwrap().active.as_mut() {
        active.source_origin = ResidentSourceOrigin::Unknown;
    }

    // Stage a second device insert while the device_gesture is STILL active.
    service
        .submit(Request::Edit(Command::InsertMainDevice {
            before: None,
            state: utility,
        }))
        .unwrap();
    let pending_insert = receive(&service);
    assert!(!pending_insert.request_complete);
    assert_eq!(pending_insert.state.project.main_devices().len(), 1);

    // Trigger SaveCurrent while pending_device_insert is STILL awaiting audio!
    service.submit(Request::SaveCurrent).unwrap();
    std::thread::sleep(Duration::from_millis(30));
    assert!(
        service.events.try_recv().is_err(),
        "SaveCurrent must stay deferred while pending_device_insert and device_gesture are active"
    );

    // Now the audio callback installs the staged device insert: must commit
    // with DeviceInsertOutcome::Installed (never ProjectCommitFailed!).
    let committed = install_candidate(&service, &fixture);
    assert_eq!(
        committed
            .device_insert_complete
            .as_ref()
            .map(|completion| completion.outcome),
        Some(DeviceInsertOutcome::Installed)
    );
    assert_eq!(committed.state.project.main_devices().len(), 2);

    // Immediately after commit, the deferred SaveCurrent finalizes the active
    // gesture and saves the consistent project containing BOTH edits.
    let saved = receive(&service);
    assert!(saved.save_complete);
    assert_eq!(saved.state.project.main_devices().len(), 2);
    let loaded = hex_storage::load(&save_path).unwrap();
    assert_eq!(loaded.main_devices().len(), 2);
    let saved_gain = hex_engine::native_device_parameter(
        loaded.device(first_device).unwrap().state(),
        gain_param,
    )
    .unwrap();
    assert_eq!(saved_gain, edited_gain);

    let _ = std::fs::remove_file(&save_path);
    let _ = std::fs::remove_dir(&temp_dir);
}

#[test]
fn hosted_membership_operation_detects_same_id_vst3_native_replacement() {
    let mut editor = hex_project::Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let hex_project::Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "track".into(),
        })
        .unwrap()
    else {
        panic!("expected track");
    };
    let vst3 = hex_plugin_state::encode(hex_plugin_state::State {
        identity: hex_plugin_state::Identity {
            class: hex_plugin_state::ClassId::new([0x33; 16]).unwrap(),
            binary: hex_plugin_state::BinaryIdentity {
                executable_sha256: [0x44; 32],
                info_sha256: [0x55; 32],
            },
        },
        component: Some(b"vst3"),
        controller: None,
    })
    .unwrap();
    let hex_project::Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: vst3,
        })
        .unwrap()
    else {
        panic!("expected vst3 device");
    };
    let with_vst3 = editor.project().clone();
    let native_utility = hex_engine::default_native_device_state(
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: native_utility,
        })
        .unwrap();
    let with_native = editor.project().clone();
    assert_eq!(
        hosted_membership_operation(&with_vst3, &with_native),
        Some(GraphEditOperation::RemoveDevice)
    );
    assert_eq!(
        hosted_membership_operation(&with_native, &with_vst3),
        Some(GraphEditOperation::InsertDevice)
    );
}

#[test]
fn verified_streaming_with_vst3_supports_edit_compare_gesture_and_replacement_undo_redo() {
    #[derive(Debug)]
    struct InstalledReceipt(ResidentGraphSubmission);
    impl DeviceInsertAudioReceipt for InstalledReceipt {
        fn submission(&self) -> ResidentGraphSubmission {
            self.0
        }
        fn state(&self) -> DeviceInsertAudioState {
            DeviceInsertAudioState::Installed
        }
        fn cancel(&self) -> bool {
            false
        }
    }

    struct VerifiedVst3Sink {
        next_revision: u64,
        prepare_verified_calls: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl WorkerAudioSink for VerifiedVst3Sink {
        fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
            Ok(48_000)
        }
        fn canonical_changed(&mut self, _: Arc<CanonicalAudioSourceSnapshot>) {}
        fn poll(&mut self) {}
        fn close(&mut self) {}
        fn resident_publication_enabled(&self) -> bool {
            true
        }
        fn publish_resident(
            &mut self,
            _playback: crate::service::Playback,
            _resources: Option<&hex_engine::native_device_resources::PreparedDeviceResources>,
            _observation: Arc<hex_engine::PreparedPlaybackObservation>,
            _metadata: ResidentPublicationMetadata,
            _policy: ResidentPublicationPolicy,
            _metadata_limit: usize,
            _admit: &mut dyn FnMut(
                &[hex_engine::native_device_resources::DeviceResourceAdmission<'_>],
            ) -> Result<(), String>,
        ) -> Result<ResidentPublicationAcceptance, ResidentPublicationRefusal> {
            self.next_revision += 1;
            Ok(ResidentPublicationAcceptance::Submitted {
                output_identity: 1,
                audio_revision: self.next_revision,
            })
        }
        fn confirms_verified_device_inserts(&self) -> bool {
            true
        }
        fn begin_verified_graph_edit(
            &mut self,
            attempt: VerifiedGraphEditAttempt,
        ) -> Result<(), String> {
            self.next_revision += 1;
            let submission = ResidentGraphSubmission {
                output_identity: 1,
                audio_revision: self.next_revision,
                context: attempt.source().context(),
            };
            attempt.submitted(Box::new(InstalledReceipt(submission)));
            Ok(())
        }
        fn prepare_verified_effects(
            &mut self,
            _input: ResidentEffectPreparation<'_>,
            _attempt: Option<&VerifiedGraphEditAttempt>,
            prepare: &mut dyn FnMut(
                &hex_engine::AudioCandidateEffects<'_, '_>,
            ) -> Result<PreparedRevision, hex_engine::Invalid>,
        ) -> Result<PreparedRevision, String> {
            self.prepare_verified_calls.fetch_add(1, Ordering::AcqRel);
            hex_engine::AudioCandidateEffects::with_test_foreign_effect_program(
                hex_engine::ProcessorChainOwner::Main,
                u64::MAX,
                hex_lattice_contract::ForeignProcessorBinding {
                    root: 1,
                    instance: 1,
                    graph: 1,
                    epoch: 1,
                },
                std::num::NonZeroU64::new(1).unwrap(),
                0,
                prepare,
            )
            .map_err(|error| error.to_string())
        }
    }

    let prepare_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let retained = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES * 2;
    let service = Service::with_audio_sink_and_native_reservation(
        32,
        1_000_000,
        None,
        Box::new(VerifiedVst3Sink {
            next_revision: 0,
            prepare_verified_calls: Arc::clone(&prepare_calls),
        }),
        crate::ResidentNativeReservation {
            storage: hex_engine::AudioStorageLedgerReservation {
                data_rows: 512,
                handle_rows: 64,
                root_rows: 64,
                max_retained_bytes: retained,
                max_construction_peak_bytes: retained
                    + hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
            },
            max_scan_frames: 1024,
        },
    )
    .unwrap();
    let _ = receive(&service);

    // Switch to VerifiedStreaming mode.
    service
        .submit(Request::NewWithAudioMode {
            reservation: Some(crate::SourcePlanReservation {
                reader: hex_media::VerifiedWavReaderLimits {
                    reader: hex_media::WavReaderLimits {
                        max_file_bytes: 64 * 1024 * 1024,
                        max_source_frames: 8_000_000,
                        max_header_count: 32,
                        max_window_frames: 4096,
                    },
                    max_manifest_bytes: 1024 * 1024,
                },
                max_retained_sources: 64,
                max_path_bytes: 1024 * 1024,
                max_manifest_bytes: 4 * 1024 * 1024,
            }),
            mode: DocumentAudioMode::VerifiedStreaming,
        })
        .unwrap();
    let mode_event = receive(&service);
    assert_eq!(
        mode_event
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .audio_mode(),
        DocumentAudioMode::VerifiedStreaming
    );
    let project_generation = mode_event
        .state
        .canonical_audio_source
        .as_ref()
        .unwrap()
        .context()
        .project_generation;

    let run = |request: Request| -> Event {
        service.submit(request).unwrap();
        let mut last = receive(&service);
        while !last.request_complete {
            last = receive(&service);
        }
        assert!(last.error.is_none(), "unexpected error: {:?}", last.error);
        last
    };

    let added = run(Request::Edit(Command::AddTrack {
        name: "vst3 track".into(),
    }));
    let track = added.state.project.tracks()[0].id();
    let vst3_state = hex_plugin_state::encode(hex_plugin_state::State {
        identity: hex_plugin_state::Identity {
            class: hex_plugin_state::ClassId::new([0x33; 16]).unwrap(),
            binary: hex_plugin_state::BinaryIdentity {
                executable_sha256: [0x44; 32],
                info_sha256: [0x55; 32],
            },
        },
        component: Some(b"vst3"),
        controller: None,
    })
    .unwrap();
    let inserted_vst3 = run(Request::Edit(Command::InsertDevice {
        track,
        before: None,
        state: vst3_state,
    }));
    let vst3_device = inserted_vst3.state.project.track(track).unwrap().devices()[0].id();
    let inserted_native = run(Request::Edit(Command::InsertDevice {
        track,
        before: None,
        state: hex_engine::default_native_device_state(
            hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
        )
        .unwrap(),
    }));
    let native_device = inserted_native
        .state
        .project
        .track(track)
        .unwrap()
        .devices()[1]
        .id();
    assert!(prepare_calls.load(Ordering::Acquire) > 0);

    // 1. Request::Edit paths (bypass, track gain, mute, add track) succeed in VerifiedStreaming + VST3.
    let bypassed = run(Request::Edit(Command::ToggleDeviceBypass {
        device: vst3_device,
    }));
    assert!(
        bypassed
            .state
            .project
            .device(vst3_device)
            .unwrap()
            .bypassed()
    );
    let unbypassed = run(Request::Edit(Command::SetDeviceBypass {
        device: vst3_device,
        bypassed: false,
    }));
    assert!(
        !unbypassed
            .state
            .project
            .device(vst3_device)
            .unwrap()
            .bypassed()
    );
    let gained = run(Request::Edit(Command::Gain { track, gain: 0.75 }));
    assert!((gained.state.project.track(track).unwrap().gain() - 0.75).abs() < 1e-6);
    let muted = run(Request::Edit(Command::ToggleMute(track)));
    assert!(muted.state.project.track(track).unwrap().muted());
    let extra = run(Request::Edit(Command::AddTrack {
        name: "second".into(),
    }));
    assert_eq!(extra.state.project.tracks().len(), 2);

    // 2. Request::DeviceGesture (native device parameter + track gain + barrier finalize) succeeds.
    let gain_param = hex_lattice_contract::ParameterId::from_static(1);
    let edited_gain = hex_lattice_contract::ParameterValue::Float(6.0);
    let key = crate::device_gesture::GestureKey {
        generation: project_generation,
        token: 1,
    };
    let begin = run(Request::DeviceGesture(
        crate::device_gesture::GestureRequest::Begin {
            key,
            target: crate::device_gesture::GestureTarget::DeviceParameter {
                device: native_device,
                parameter: gain_param,
            },
        },
    ));
    assert!(begin.gesture_ack.as_ref().unwrap().accepted);
    let change = run(Request::DeviceGesture(
        crate::device_gesture::GestureRequest::Change {
            key,
            sequence: 1,
            value: edited_gain,
        },
    ));
    assert!(
        change.gesture_ack.as_ref().unwrap().accepted,
        "{:?}",
        change.gesture_ack
    );
    let commit = run(Request::DeviceGesture(
        crate::device_gesture::GestureRequest::Commit {
            key,
            sequence: 2,
            value: edited_gain,
            manual_edit: false,
        },
    ));
    assert!(commit.gesture_ack.as_ref().unwrap().accepted);
    assert_eq!(
        hex_engine::native_device_parameter(
            commit.state.project.device(native_device).unwrap().state(),
            gain_param,
        )
        .unwrap(),
        edited_gain
    );

    // Also exercise gesture rebase (ToggleMute) + finalization barrier (Undo -> finish_verified).
    let key2 = crate::device_gesture::GestureKey {
        generation: project_generation,
        token: 2,
    };
    let _ = run(Request::DeviceGesture(
        crate::device_gesture::GestureRequest::Begin {
            key: key2,
            target: crate::device_gesture::GestureTarget::TrackGain { track },
        },
    ));
    let _ = run(Request::DeviceGesture(
        crate::device_gesture::GestureRequest::Change {
            key: key2,
            sequence: 1,
            value: hex_lattice_contract::ParameterValue::Float(0.5),
        },
    ));
    let rebased = run(Request::Edit(Command::ToggleMute(track)));
    assert!(rebased.active_device_gesture.is_some());
    let undone_gesture = run(Request::Undo);
    assert!(undone_gesture.active_device_gesture.is_none());
    let finalized = run(Request::Redo);
    assert!((finalized.state.project.track(track).unwrap().gain() - 0.5).abs() < 1e-6);

    // 3. Request::CompareDevice (Switch to B, edit B, audible Switch back to A) succeeds.
    let copied_ab = run(Request::CompareDevice {
        owner: crate::service::preset::Owner {
            project_generation,
            revision: finalized.state.revision,
            device: native_device,
            class: hex_engine::NativeDeviceClass::Effect,
            type_id: hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
        },
        expected: hex_project::DeviceVariant::A,
        action: hex_project::DeviceComparisonAction::Switch,
    });
    assert_eq!(copied_ab.error, None);
    let patched_for_ab = run(Request::SetDeviceParameter {
        device: native_device,
        parameter: gain_param,
        value: hex_lattice_contract::ParameterValue::Float(12.0),
    });
    let switched_ab = run(Request::CompareDevice {
        owner: crate::service::preset::Owner {
            project_generation,
            revision: patched_for_ab.state.revision,
            device: native_device,
            class: hex_engine::NativeDeviceClass::Effect,
            type_id: hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
        },
        expected: hex_project::DeviceVariant::B,
        action: hex_project::DeviceComparisonAction::Switch,
    });
    assert_eq!(
        hex_engine::native_device_parameter(
            switched_ab
                .state
                .project
                .device(native_device)
                .unwrap()
                .state(),
            gain_param,
        )
        .unwrap(),
        edited_gain
    );

    // 4. Ordinary Undo/Redo in VerifiedStreaming + VST3 succeeds.
    let undone_ab = run(Request::Undo);
    assert_eq!(
        hex_engine::native_device_parameter(
            undone_ab
                .state
                .project
                .device(native_device)
                .unwrap()
                .state(),
            gain_param,
        )
        .unwrap(),
        hex_lattice_contract::ParameterValue::Float(12.0)
    );
    let redone_ab = run(Request::Redo);
    assert_eq!(
        hex_engine::native_device_parameter(
            redone_ab
                .state
                .project
                .device(native_device)
                .unwrap()
                .state(),
            gain_param,
        )
        .unwrap(),
        edited_gain
    );

    // 5. Replacing the VST3 effect with a native effect via ReplaceNativeDevice (same DeviceId)
    //    and pressing Undo/Redo succeeds in VerifiedStreaming mode!
    let replaced = run(Request::ReplaceNativeDevice {
        device: vst3_device,
        class: hex_engine::NativeDeviceClass::Effect,
        type_id: Box::new(hex_lattice_contract::DeviceTypeId::from_static(
            "hex.utility",
        )),
    });
    assert_eq!(
        replaced
            .state
            .project
            .device(vst3_device)
            .unwrap()
            .state()
            .type_id(),
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility")
    );
    let undone_replace = run(Request::Undo);
    assert_eq!(
        undone_replace
            .state
            .project
            .device(vst3_device)
            .unwrap()
            .state()
            .type_id(),
        hex_lattice_contract::DeviceTypeId::from_static("hex.vst3")
    );
    let redone_replace = run(Request::Redo);
    assert_eq!(
        redone_replace
            .state
            .project
            .device(vst3_device)
            .unwrap()
            .state()
            .type_id(),
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility")
    );
}

mod track_routes;
