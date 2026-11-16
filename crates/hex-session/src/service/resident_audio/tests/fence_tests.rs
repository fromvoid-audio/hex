//! A stalled callback fence parks its section edit without stalling the worker
//! (#4544): unrelated requests run, later mutations wait in order, and the edit
//! completes or refuses exactly as the former synchronous poll did.
use super::*;
use std::time::Instant;

/// Answers the stopped Arrangement fence only after `remaining` polls;
/// `usize::MAX` models a callback that never runs.
struct StalledFence {
    inner: OfflineSink,
    remaining: Arc<AtomicUsize>,
    begun: Arc<AtomicUsize>,
    cancelled: Arc<AtomicUsize>,
}
impl WorkerAudioSink for StalledFence {
    fn resident_publication_enabled(&self) -> bool {
        true
    }
    fn resident_admission(&self) -> [DeviceResourceAdmission<'_>; 3] {
        self.inner.resident_admission()
    }
    fn native_sampler_hold(&self) -> NativeSamplerReservationHold {
        self.inner.native_sampler_hold()
    }
    fn initialize(&mut self, s: &AtomicBool) -> Result<u32, String> {
        self.inner.initialize(s)
    }
    fn canonical_changed(&mut self, s: Arc<CanonicalAudioSourceSnapshot>) {
        self.inner.canonical_changed(s);
    }
    fn poll(&mut self) {
        self.inner.poll();
    }
    fn close(&mut self) {
        self.inner.close();
    }
    fn begin_section_edit(
        &mut self,
        _: hex_engine::AudioSourceContext,
        _: bool,
    ) -> Result<(), String> {
        self.begun.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
    fn section_edit_ready(&mut self) -> Option<Result<ArrangementEditPosition, String>> {
        let remaining = self.remaining.load(Ordering::Acquire);
        if remaining == usize::MAX {
            return None;
        }
        if remaining > 0 {
            self.remaining.store(remaining - 1, Ordering::Release);
            return None;
        }
        Some(ArrangementEditPosition::from_frames(0, false, None))
    }
    fn cancel_section_edit(&mut self) {
        self.cancelled.fetch_add(1, Ordering::AcqRel);
    }
    #[allow(clippy::too_many_arguments, clippy::result_large_err)]
    fn publish_resident(
        &mut self,
        playback: Playback,
        resources: Option<&PreparedDeviceResources>,
        observation: Arc<hex_engine::PreparedPlaybackObservation>,
        metadata: ResidentPublicationMetadata,
        policy: ResidentPublicationPolicy,
        limit: usize,
        admit: &mut dyn FnMut(&[DeviceResourceAdmission<'_>]) -> Result<(), String>,
    ) -> Result<ResidentPublicationAcceptance, ResidentPublicationRefusal> {
        self.inner.publish_resident(
            playback,
            resources,
            observation,
            metadata,
            policy,
            limit,
            admit,
        )
    }
}

struct Fixture {
    service: Service,
    remaining: Arc<AtomicUsize>,
    begun: Arc<AtomicUsize>,
    cancelled: Arc<AtomicUsize>,
    section: hex_project::SectionId,
    device: hex_project::DeviceId,
    revision: u64,
    preset: std::path::PathBuf,
    _files: sampler_export::tests::Files,
}
impl Fixture {
    fn new() -> Self {
        let files = sampler_export::tests::Files::new(true);
        let root = files.source().parent().unwrap().to_owned();
        let remaining = Arc::new(AtomicUsize::new(0));
        let begun = Arc::new(AtomicUsize::new(0));
        let cancelled = Arc::new(AtomicUsize::new(0));
        let retained = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES * 2;
        let service = Service::with_audio_sink_and_native_reservation(
            16,
            2_000_000,
            Some(root.join("recovery/untitled.hex-journal")),
            Box::new(StalledFence {
                inner: OfflineSink {
                    observed: Arc::default(),
                    pending: None,
                },
                remaining: remaining.clone(),
                begun: begun.clone(),
                cancelled: cancelled.clone(),
            }),
            crate::ResidentNativeReservation {
                storage: hex_engine::AudioStorageLedgerReservation {
                    data_rows: 512,
                    handle_rows: 128,
                    root_rows: 128,
                    max_retained_bytes: retained,
                    max_construction_peak_bytes: retained
                        + hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
                },
                max_scan_frames: 2_000_000,
            },
        )
        .unwrap();
        let receive = || {
            service
                .events
                .recv_timeout(crate::test_wait::HANG_BOUND)
                .unwrap()
        };
        let _ = receive();
        service
            .submit(Request::Import(files.source().to_owned()))
            .unwrap();
        let imported = receive();
        assert!(imported.error.is_none(), "{:?}", imported.error);
        let track = imported.state.project.tracks()[0].id();
        service
            .submit(Request::Edit(Command::InsertDevice {
                track,
                before: None,
                state: hex_engine::default_native_device_state_for_class(
                    hex_engine::NativeDeviceClass::Effect,
                    hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
                )
                .unwrap(),
            }))
            .unwrap();
        let inserted = receive();
        assert!(inserted.error.is_none(), "{:?}", inserted.error);
        let device = inserted.state.project.tracks()[0].devices()[0].id();
        // A marker-only section edit takes no lease.
        service
            .submit(Request::EditArrangementSection {
                project_generation: 0,
                revision: inserted.state.revision,
                command: Command::AddArrangementSection {
                    name: "verse".into(),
                    color: hex_project::ElementColor::Rose,
                    start: hex_project::BeatTime::ZERO,
                    end: hex_project::BeatTime::from_quarters(4).unwrap(),
                },
            })
            .unwrap();
        let added = receive();
        assert!(added.error.is_none(), "{:?}", added.error);
        let Some(Created::ArrangementSection(section)) = added.created else {
            panic!("section")
        };
        assert_eq!(begun.load(Ordering::Acquire), 0);
        Self {
            service,
            remaining,
            begun,
            cancelled,
            section,
            device,
            revision: added.state.revision,
            preset: root.join("unrelated.hex-preset"),
            _files: files,
        }
    }
    fn receive(&self) -> Event {
        self.service
            .events
            .recv_timeout(crate::test_wait::HANG_BOUND)
            .unwrap()
    }
    /// A structural section edit (which fences), an unrelated device query
    /// and a later project mutation, submitted back to back.
    fn submit_edit_query_and_mutation(&self) {
        self.service
            .submit(Request::EditArrangementSection {
                project_generation: 0,
                revision: self.revision,
                command: Command::DuplicateArrangementSection(self.section),
            })
            .unwrap();
        self.service
            .submit(Request::SaveDevicePreset {
                owner: preset::Owner {
                    project_generation: 0,
                    revision: self.revision,
                    device: self.device,
                    class: hex_engine::NativeDeviceClass::Effect,
                    type_id: hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
                },
                path: self.preset.clone(),
            })
            .unwrap();
        self.service
            .submit(Request::Edit(Command::AddScene {
                name: "later".into(),
            }))
            .unwrap();
    }
}

#[test]
fn stalled_fence_serves_an_unrelated_query_and_refuses_after_the_bound() {
    let fixture = Fixture::new();
    fixture.remaining.store(usize::MAX, Ordering::Release);
    let started = Instant::now();
    fixture.submit_edit_query_and_mutation();
    // The query can finish before or after the real deadline if this worker
    // is descheduled. Both events must preserve the original revision.
    let first = fixture.receive();
    let second = fixture.receive();
    let (query, edit) = if first.error.is_none() {
        (first, second)
    } else {
        (second, first)
    };
    assert!(query.error.is_none(), "{:?}", query.error);
    assert!(fixture.preset.exists());
    assert_eq!(query.state.revision, fixture.revision);
    assert_eq!(fixture.begun.load(Ordering::Acquire), 1);
    assert_eq!(
        edit.error.as_deref(),
        Some("audio output did not acknowledge the arrangement edit; try again")
    );
    assert!(started.elapsed() >= Duration::from_millis(100));
    assert_eq!(edit.state.revision, fixture.revision);
    assert_eq!(fixture.cancelled.load(Ordering::Acquire), 1);
    // The held mutation runs only after the lease resolved.
    let later = fixture.receive();
    assert!(later.error.is_none(), "{:?}", later.error);
    assert_eq!(later.state.revision, fixture.revision + 1);
    assert_eq!(later.state.project.scenes().len(), 1);
}

#[test]
fn late_fence_answer_completes_or_refuses_before_later_mutations() {
    let fixture = Fixture::new();
    // The callback answers after a few worker polls.
    fixture.remaining.store(5, Ordering::Release);
    fixture
        .service
        .submit(Request::EditArrangementSection {
            project_generation: 0,
            revision: fixture.revision,
            command: Command::DuplicateArrangementSection(fixture.section),
        })
        .unwrap();
    let edit = fixture.receive();
    let completed = edit.error.is_none();
    if completed {
        assert_eq!(edit.state.revision, fixture.revision + 1);
        assert_eq!(edit.state.project.arrangement_sections().len(), 2);
    } else {
        assert_eq!(
            edit.error.as_deref(),
            Some("audio output did not acknowledge the arrangement edit; try again")
        );
        assert_eq!(edit.state.revision, fixture.revision);
        assert_eq!(fixture.cancelled.load(Ordering::Acquire), 1);
    }
    assert_eq!(fixture.begun.load(Ordering::Acquire), 1, "begun once");
    fixture
        .service
        .submit(Request::Edit(Command::AddScene {
            name: "later".into(),
        }))
        .unwrap();
    let later = fixture.receive();
    assert!(later.error.is_none(), "{:?}", later.error);
    assert_eq!(
        later.state.revision,
        fixture.revision + u64::from(completed) + 1
    );
    assert_eq!(
        later.state.project.arrangement_sections().len(),
        if completed { 2 } else { 1 }
    );
}

/// #4568: parked time is its own phase; the dispatched section phases stay
/// within `worker_us`, which starts only when the parked edit dispatches.
#[test]
fn parked_wait_is_reported_apart_from_the_dispatched_section_phases() {
    use crate::service::diagnostics::{DiagnosticObserver, DiagnosticStage};
    let fixture = Fixture::new();
    let outcomes = Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed = Arc::clone(&outcomes);
    fixture
        .service
        .set_diagnostic_observer(Arc::new(DiagnosticObserver::new(move |event| {
            if matches!(event.stage, DiagnosticStage::OutcomeReady { .. }) {
                observed.lock().unwrap().push(event.details.clone());
            }
        })))
        .unwrap();
    // The callback answers after enough worker polls for a measurable park.
    fixture.remaining.store(20, Ordering::Release);
    fixture
        .service
        .submit(Request::EditArrangementSection {
            project_generation: 0,
            revision: fixture.revision,
            command: Command::DuplicateArrangementSection(fixture.section),
        })
        .unwrap();
    let edit = fixture.receive();
    let completed = edit.error.is_none();
    if !completed {
        assert_eq!(
            edit.error.as_deref(),
            Some("audio output did not acknowledge the arrangement edit; try again")
        );
    }
    let details = outcomes
        .lock()
        .unwrap()
        .iter()
        .rfind(|details| details.contains("section_fence_us="))
        .cloned()
        .expect("section outcome");
    let field = |name: &str| -> u64 {
        details
            .split_whitespace()
            .find_map(|field| field.strip_prefix(name)?.strip_prefix('=')?.parse().ok())
            .unwrap_or_else(|| panic!("{name} missing from {details:?}"))
    };
    let parked = field("section_parked_us");
    let worker = field("worker_us");
    if completed {
        assert!(
            field("section_view_us") > 0,
            "view construction is measured: {details}"
        );
    } else {
        assert_eq!(
            field("section_view_us"),
            0,
            "refusal has no view: {details}"
        );
    }
    let dispatched: u64 = [
        "section_fence_us",
        "section_edit_us",
        "section_prepare_us",
        "section_publish_us",
        "section_finish_us",
        "section_view_us",
    ]
    .iter()
    .map(|name| field(name))
    .sum();
    assert!(parked > 0, "parked wait is observable: {details}");
    assert!(
        dispatched <= worker,
        "phases {dispatched} within worker {worker}: {details}"
    );
    // A marker-only edit does not need a lease, so even a valid timeout above
    // cannot remove coverage of successful view-construction diagnostics.
    outcomes.lock().unwrap().clear();
    fixture
        .service
        .submit(Request::EditArrangementSection {
            project_generation: 0,
            revision: edit.state.revision,
            command: Command::AddArrangementSection {
                name: "marker".into(),
                color: hex_project::ElementColor::Rose,
                start: hex_project::BeatTime::from_quarters(8).unwrap(),
                end: hex_project::BeatTime::from_quarters(12).unwrap(),
            },
        })
        .unwrap();
    let marker = fixture.receive();
    assert!(marker.error.is_none(), "{:?}", marker.error);
    let marker_details = outcomes
        .lock()
        .unwrap()
        .last()
        .cloned()
        .expect("marker outcome");
    let marker_view = marker_details
        .split_whitespace()
        .find_map(|field| field.strip_prefix("section_view_us=")?.parse::<u64>().ok())
        .expect("marker view timing");
    assert!(
        marker_view > 0,
        "view construction is measured: {marker_details}"
    );
    outcomes.lock().unwrap().clear();
    fixture
        .service
        .submit(Request::EditArrangementSection {
            project_generation: 0,
            revision: marker.state.revision,
            command: Command::AddArrangementSection {
                name: "invalid".into(),
                color: hex_project::ElementColor::Rose,
                start: hex_project::BeatTime::ZERO,
                end: hex_project::BeatTime::ZERO,
            },
        })
        .unwrap();
    let refused = fixture.receive();
    assert!(refused.error.is_some());
    let details = outcomes
        .lock()
        .unwrap()
        .last()
        .cloned()
        .expect("refused outcome");
    assert!(
        details
            .split_whitespace()
            .any(|field| field == "section_view_us=0"),
        "{details}"
    );
}
