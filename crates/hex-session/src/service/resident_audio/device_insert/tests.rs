use super::*;
use std::sync::{
    Mutex,
    atomic::{AtomicU8, AtomicU64},
};

const PENDING: u8 = 0;
const INSTALLED: u8 = 1;
const CANCELLED: u8 = 2;
const REFUSED: u8 = 3;
const EXPIRED: u8 = 4;

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
    revision: u64,
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
        _: ResidentEffectPreparation<'_>,
        prepare: &mut dyn FnMut(
            &hex_engine::AudioCandidateEffects<'_, '_>,
        ) -> Result<PreparedRevision, hex_engine::Invalid>,
    ) -> Result<Option<PreparedRevision>, String> {
        hex_engine::AudioCandidateEffects::with_host_definitions(&[], prepare)
            .map(Some)
            .map_err(|error| error.to_string())
    }

    fn confirms_device_inserts(&self) -> bool {
        true
    }

    fn device_insert_receipt(
        &mut self,
        submission: ResidentGraphSubmission,
    ) -> Option<Box<dyn DeviceInsertAudioReceipt>> {
        let fixture = self.lock().unwrap();
        let receipt = fixture.candidate.as_ref()?;
        (receipt.submission == submission).then(|| Box::new(Arc::clone(receipt)) as Box<_>)
    }

    fn resident_runtime_loop(
        &mut self,
        _: hex_engine::AudioSourceContext,
    ) -> Result<Option<ResidentRuntimeLoop>, String> {
        Ok(self.lock().unwrap().active)
    }

    #[allow(clippy::result_large_err)]
    fn publish_resident(
        &mut self,
        playback: Playback,
        resources: Option<&PreparedDeviceResources>,
        _: Arc<hex_engine::PreparedPlaybackObservation>,
        metadata: ResidentPublicationMetadata,
        policy: ResidentPublicationPolicy,
        metadata_limit: usize,
        admit: &mut dyn FnMut(&[DeviceResourceAdmission<'_>]) -> Result<(), String>,
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
            fixture.candidate = Some(Arc::new(FixtureReceipt {
                submission: ResidentGraphSubmission {
                    output_identity: observation.output_identity,
                    audio_revision: observation.audio_revision,
                    context: metadata.context,
                },
                state: AtomicU8::new(PENDING),
                deadline: Instant::now() + Duration::from_millis(250),
            }));
        } else {
            fixture.active = Some(observation);
        }
        Ok(ResidentPublicationAcceptance::Submitted {
            output_identity: observation.output_identity,
            audio_revision: observation.audio_revision,
        })
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
