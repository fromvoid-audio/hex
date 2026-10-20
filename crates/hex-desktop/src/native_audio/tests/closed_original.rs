//! Real Service dispatch -> SAME Session writers -> private Audio consumer.
//! This desired-positive discriminator does not waive legacy worker roots.
use super::*;
use hex_engine::{AudioRootStorageLedger, AudioStorageLedgerReservation, AudioStorageReservation};
use hex_session::service::{WorkerRootView, sampler_captured_source_fields_on_control};

fn storage() -> AudioStorageReservation {
    AudioStorageReservation {
        allocation_items: 256,
        handle_obligations: 1024,
        max_prepared_data_bytes: 32 * 1024 * 1024,
        max_construction_data_bytes: 32 * 1024 * 1024,
        caller_retained_bytes: 0,
        caller_construction_bytes: 0,
    }
}
fn ledger_limits() -> AudioStorageLedgerReservation {
    AudioStorageLedgerReservation {
        data_rows: 4096,
        handle_rows: 4096,
        root_rows: 256,
        max_retained_bytes: 32 * 1024 * 1024,
        max_construction_peak_bytes: 32 * 1024 * 1024,
    }
}
fn publication() -> hex_audio::AudioCandidatePublicationReservation {
    AudioPolicy::new().unwrap().publication
}
struct ActualConsumer {
    source: Option<SharedCanonicalAudioSource>,
    output: Option<WorkerPreparedOutput>,
    backend: Option<HeadlessBackend>,
    files: Vec<hex_session::service::MainControlFileJob>,
    result: Option<mpsc::SyncSender<Result<(), String>>>,
}
impl ActualConsumer {
    fn construct(&mut self, roots: &WorkerRootView<'_>) -> Result<(), String> {
        let source = self.source.as_ref().ok_or("missing real source")?;
        if !roots.matches_current_source_on_control(source) {
            return Err("actual source differs".into());
        }
        let output = self.output.as_mut().ok_or("missing real output")?;
        let mut control = output.publication.control().ok_or("missing real worker")?;
        // This is the distinct original-class entry. The legacy per-project
        // class still refuses instruments/MIDI. Existing supported-limit rows
        // are a conservative ceiling only; actual compiler/host walks remain
        // mandatory and may refuse, without changing the legacy class guard.
        let quote = hex_engine::AudioCandidateReservationQuote::for_supported_limits(0)
            .map_err(|e| format!("original-class ceiling: {e:?}"))?;
        let selected_storage = quote
            .storage_reservation(0)
            .map_err(|e| format!("actual selected storage: {e:?}"))?;
        let mut selected_limits = ledger_limits();
        selected_limits.max_retained_bytes = selected_storage.max_prepared_data_bytes;
        selected_limits.max_construction_peak_bytes = selected_storage.max_construction_data_bytes;
        let token = control
            .reserve_audio_candidate_preparation(
                hex_audio::AudioCandidatePreparationReservation {
                    source_capacity: 0,
                    member_rows: quote.member_rows,
                    transfer_rows: quote.transfer_rows,
                    handle_rows: quote.handle_rows,
                    storage: selected_storage,
                },
                Vec::new(),
            )
            .map_err(|error| error.reason.to_owned())?;
        crate::snapshot_transport::with_snapshot_family_inventory_on_control(
            65_536, |families| {
                let worker = roots.sampler_fields_on_control();
                let captured = sampler_captured_source_fields_on_control(source);
                let files = hex_session::service::MainControlFileJob::reserved_slots_fields_on_control(&self.files);
                let mut loan = roots.original_sampler_session_loan_on_control().map_err(|e| format!("Session loan: {e:?}"))?;
                let (resident, writers) = loan.resident_fields_and_writer_on_control();
                let plan = hex_engine::native_device_resources::OriginalResourceBankPlan::borrow_resident_fields_on_control(
                    &resident, source.project_owner_on_control(),
                ).map_err(|e| format!("actual producer: {e:?}"))?;
                use hex_engine::sampler_host_fields::OriginalSamplerResidentFields;
                let cancel = resident.cancellation().map_err(|e| format!("actual cancel: {e:?}"))?;
                let limits = hex_engine::native_device_resources::OriginalResourcePlanReservation {
                    inputs: AudioStorageReservation {
                        max_prepared_data_bytes: 1_000_000,
                        max_construction_data_bytes: 1_000_000,
                        ..storage()
                    },
                    catalog: hex_engine::native_device_resources::ResourceCatalogReservation {
                        metadata_bytes: 65_536, hash_frames: 8,
                    },
                    max_preparation_metadata_bytes: 65_536,
                    max_retained_data_bytes: 1_000_000,
                    max_construction_peak_bytes: 1_000_000,
                };
                let progress = control.prepare_audio_candidate_on_control(token, |mut operations| {
                    operations.install_original_resources_on_control(&plan, limits, cancel)
                        .map_err(|_| hex_engine::AudioStorageError::UnknownStorage)
                }).map_err(str::to_owned)?;
                if !matches!(progress, hex_audio::AudioCandidatePreparationProgress::Collecting) {
                    return Err(format!("actual producer did not install resources: {progress:?}"));
                }
                // Actual surrounding borrowed inputs, not an approved scalar or
                // fabricated resource bank. Mandatory constructor walks repeat
                // these operands and reject ANY unexplained original rows.
                let mut original = AudioRootStorageLedger::try_new_on_control(ledger_limits()).map_err(|e| format!("original ledger: {e:?}"))?;
                original.borrow_sampler_worker_fields_on_control(&worker).map_err(|e| format!("worker fields: {e:?}"))?;
                original.borrow_sampler_captured_source_fields_on_control(&captured).map_err(|e| format!("captured fields: {e:?}"))?;
                cancel.visit_storage_on_control(&mut original).map_err(|e| format!("actual cancel storage: {e:?}"))?;
                control.construct_original_sampler_with_host_on_control(
                    token, &worker, &captured, &files, &families, &resident, &plan,
                    None, &original, writers, selected_limits,
                ).map_err(|e| format!("closed actual host: {e:?}"))
            },
        ).map_err(|e| format!("actual families: {e:?}"))?
    }
}
impl WorkerAudioSink for ActualConsumer {
    fn initialize(&mut self, _: &dyn hex_engine::CancellationQuery) -> Result<u32, String> {
        let (mut output, backend) =
            hex_audio::worker_fixture::open_with_reservation(48_000, publication())?;
        output
            .publication
            .control()
            .ok_or("missing worker")?
            .configure_audio_candidate_publication(publication())
            .map_err(str::to_owned)?;
        self.output = Some(output);
        self.backend = Some(backend);
        Ok(48_000)
    }
    fn canonical_changed(&mut self, source: SharedCanonicalAudioSource) {
        self.source = Some(source);
    }
    fn poll(&mut self) {}
    fn poll_with_worker_roots(&mut self, roots: &WorkerRootView<'_>) {
        let ready = self.source.as_ref().is_some_and(|s| {
            s.project_owner_on_control()
                .tracks()
                .iter()
                .flat_map(|track| track.midi_clips())
                .any(|clip| !clip.notes().is_empty())
        });
        if ready && let Some(sender) = self.result.take() {
            let result = self.construct(roots);
            let _ = sender.try_send(result);
        }
    }
    fn close(&mut self) {
        self.source.take();
        self.backend.take();
        self.output.take();
    }
}
#[test]
fn real_service_nonempty_original_graph_reaches_closed_native_host_consumer() {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    let _families = crate::snapshot_transport::TABLE_TEST_GATE.lock().unwrap();
    crate::snapshot_transport::initialize_test_table();
    let (send, receive) = mpsc::sync_channel(1);
    let (_family, worker_control) =
        crate::snapshot_transport::worker_from_existing_table_on_control().unwrap();
    let service = Service::with_audio_sink_and_worker_control(
        8,
        1024,
        None,
        Box::new(ActualConsumer {
            source: None,
            output: None,
            backend: None,
            files: Vec::with_capacity(2),
            result: Some(send),
        }),
        worker_control,
    )
    .unwrap();
    fn event(service: &Service, predicate: impl Fn(&Event) -> bool) -> Event {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(event) = service.try_event()
                && predicate(&event)
            {
                return event;
            }
            assert!(Instant::now() < deadline, "actual Service event timed out");
            std::thread::yield_now();
        }
    }
    event(&service, |_| true);
    service
        .submit(Request::Edit(hex_project::Command::AddMidiTrack {
            name: "actual original graph".into(),
        }))
        .unwrap();
    let added = event(&service, |e| !e.state.project.tracks().is_empty());
    let track = added.state.project.tracks()[0].id();
    let instrument = added.state.project.tracks()[0].instrument().unwrap().id();
    drop(added);
    service
        .submit(Request::Edit(hex_project::Command::ReplaceDeviceState {
            device: instrument,
            state: hex_engine::empty_sampler_state_on_control().unwrap(),
        }))
        .unwrap();
    event(&service, |e| {
        e.state.project.device(instrument).is_some_and(|d| {
            d.state().type_id()
                == hex_engine::empty_sampler_state_on_control()
                    .unwrap()
                    .type_id()
        })
    });
    service
        .submit(Request::Edit(hex_project::Command::PlaceMidiClip {
            track,
            start: hex_project::BeatTime::ZERO,
            length: hex_project::BeatTime::ONE,
        }))
        .unwrap();
    let clip = event(&service, |e| {
        e.state
            .project
            .tracks()
            .iter()
            .flat_map(|track| track.midi_clips())
            .next()
            .is_some()
    })
    .state
    .project
    .tracks()
    .iter()
    .flat_map(|track| track.midi_clips())
    .next()
    .unwrap()
    .id();
    service
        .submit(Request::Edit(hex_project::Command::AddMidiNote {
            clip,
            start: hex_project::BeatTime::ZERO,
            duration: hex_project::BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 127,
        }))
        .unwrap();
    event(&service, |e| {
        e.state
            .project
            .tracks()
            .iter()
            .flat_map(|track| track.midi_clips())
            .any(|c| !c.notes().is_empty())
    });
    let result = receive.recv_timeout(Duration::from_secs(10)).unwrap();
    drop(service);
    assert_eq!(result, Ok(()), "desired actual closed-consumer integration");
}
