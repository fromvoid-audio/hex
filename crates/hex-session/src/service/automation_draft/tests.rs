//! Real disk lane, bounded loan pressure and existing recovery/Save As consumers.
use super::*;
use hex_project::{AutomationValue, Command, Created, Editor, Project};
use hex_storage::automation_draft::{DraftClock, DraftOutcome, DraftReadLimits, DraftReader};
use std::{
    fs,
    sync::{
        Barrier,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hex-session-automation-draft-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn fixture() -> (Project, DraftTarget, DraftProvenance) {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "original source".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    (
        editor.project().clone(),
        DraftTarget::TrackGain { track },
        DraftProvenance {
            sample_rate: 48000,
            output_identity: 9,
            project_generation: 0,
            editor_revision: 2,
            audio_revision: 7,
            transport_epoch: 1,
            gesture_serial: 1,
        },
    )
}
fn event(sequence: u64) -> AppliedEvent {
    AppliedEvent {
        sequence,
        performance_frame: sequence * 3,
        content_frame: (sequence % 97) * 3,
        loop_pass: sequence / 97,
        value: AutomationValue::new((sequence % 10) as f32 / 10.0).unwrap(),
    }
}
fn reader(path: &Path) -> DraftReader {
    DraftReader::open(
        path,
        DraftReadLimits {
            max_source_bytes: 64 * 1024,
        },
    )
    .unwrap()
}

fn failed_outcome(
    path: PathBuf,
    acknowledged: u64,
    durable_prefix: DraftPrefixIdentity,
    events: &[AppliedEvent],
) -> DraftWorkerOutcome {
    let retained = if events.is_empty() {
        Vec::new()
    } else {
        let mut batch = Box::new(Batch {
            slot: 0,
            ticket: 1,
            events: [event(1); EVENTS_PER_BATCH],
            len: events.len(),
        });
        batch.events[..events.len()].copy_from_slice(events);
        vec![RetainedBatch(batch)]
    };
    DraftWorkerOutcome {
        path,
        admitted_events: acknowledged + events.len() as u64,
        progress: DraftProgress {
            durable_events: acknowledged,
            ..Default::default()
        },
        durable_prefix,
        requested_terminal: DraftTerminal::Interrupted,
        failure: Some("test failure".into()),
        retained,
    }
}

fn copy_test(
    outcome: &DraftWorkerOutcome,
    destination: &Path,
    limits: DraftReadLimits,
    max_events: u64,
) -> Result<(), String> {
    let captured = reader(&outcome.path);
    let project = captured.source().clone();
    let target = captured.target();
    let provenance = captured.provenance();
    let clock = captured.capture_clock();
    drop(captured);
    copy_failed_on_control(
        outcome,
        RecoverySource {
            project: &project,
            target,
            provenance,
            clock,
        },
        destination,
        limits,
        max_events,
    )
}

#[test]
fn failed_copy_reconciles_partial_and_complete_overlaps() {
    for visible in 1..=3 {
        let files = Files::new();
        let source = files.path("source");
        let output = files.path("copy");
        let (project, target, stamp) = fixture();
        let mut writer = DraftWriter::create_new(&source, &project, target, stamp).unwrap();
        writer.append(&[event(1)]).unwrap();
        let identity = writer.durable_prefix_identity();
        writer
            .append(&(2..=visible).map(event).collect::<Vec<_>>())
            .unwrap();
        drop(writer);
        let original = fs::read(&source).unwrap();
        let outcome = failed_outcome(source.clone(), 1, identity, &[event(2), event(3)]);
        copy_test(
            &outcome,
            &output,
            DraftReadLimits {
                max_source_bytes: 65536,
            },
            3,
        )
        .unwrap();
        let mut recovered = reader(&output);
        let mut page = [None; 4];
        assert_eq!(recovered.read_events(0, &mut page).unwrap(), 3);
        assert_eq!(
            &page[..3],
            &[Some(event(1)), Some(event(2)), Some(event(3))]
        );
        assert_eq!(recovered.outcome(), DraftOutcome::Interrupted);
        assert_eq!(fs::read(&source).unwrap(), original);
        assert_eq!(outcome.retained[0].events(), &[event(2), event(3)]);
    }
}

#[test]
fn failed_copy_refuses_inconsistent_custody_and_keeps_original() {
    let files = Files::new();
    let source = files.path("source");
    let (project, target, stamp) = fixture();
    let mut writer = DraftWriter::create_new(&source, &project, target, stamp).unwrap();
    writer.append(&[event(1)]).unwrap();
    let identity1 = writer.durable_prefix_identity();
    writer.append(&[event(2)]).unwrap();
    let identity2 = writer.durable_prefix_identity();
    let original = fs::read(&source).unwrap();
    writer.append(&[event(3)]).unwrap();
    let identity3 = writer.durable_prefix_identity();
    drop(writer);
    fs::write(&source, &original).unwrap();
    let limits = DraftReadLimits {
        max_source_bytes: 65536,
    };
    let mismatch = failed_outcome(source.clone(), 1, identity1, &[event(3)]);
    assert!(copy_test(&mismatch, &files.path("mismatch"), limits, 4).is_err());
    let missing = failed_outcome(source.clone(), 3, identity3, &[]);
    assert!(copy_test(&missing, &files.path("missing"), limits, 4).is_err());
    let duplicate = failed_outcome(source.clone(), 2, identity2, &[event(2)]);
    assert!(copy_test(&duplicate, &files.path("duplicate"), limits, 4).is_err());
    assert!(copy_test(&duplicate, &files.path("budget"), limits, 1).is_err());
    let collision = files.path("collision");
    fs::write(&collision, b"keep").unwrap();
    let valid = failed_outcome(source.clone(), 2, identity2, &[]);
    let mut wrong_stamp = stamp;
    wrong_stamp.gesture_serial += 1;
    assert!(
        copy_failed_on_control(
            &valid,
            RecoverySource {
                project: &project,
                target,
                provenance: wrong_stamp,
                clock: None
            },
            &files.path("wrong-identity"),
            limits,
            4
        )
        .is_err()
    );
    assert!(!files.path("wrong-identity").exists());
    assert!(copy_test(&valid, &collision, limits, 4).is_err());
    assert_eq!(fs::read(&collision).unwrap(), b"keep");
    let wrong_count = failed_outcome(source.clone(), 2, identity1, &[]);
    assert!(copy_test(&wrong_count, &files.path("wrong-count"), limits, 4).is_err());
    let replacement = files.path("replacement");
    let mut other = DraftWriter::create_new(&replacement, &project, target, stamp).unwrap();
    other.append(&[event(2), event(3)]).unwrap();
    drop(other);
    fs::write(&source, fs::read(replacement).unwrap()).unwrap();
    assert!(copy_test(&valid, &files.path("replaced"), limits, 4).is_err());
    fs::write(&source, &original).unwrap();
    assert_eq!(fs::read(&source).unwrap(), original);
}

#[test]
fn failed_copy_retains_valid_prefix_after_damaged_tail_and_reports_output_error() {
    let files = Files::new();
    let source = files.path("source");
    let (project, target, stamp) = fixture();
    let mut writer = DraftWriter::create_new(&source, &project, target, stamp).unwrap();
    writer.append(&[event(1)]).unwrap();
    let identity = writer.durable_prefix_identity();
    drop(writer);
    let mut bytes = fs::read(&source).unwrap();
    bytes.push(3);
    fs::write(&source, &bytes).unwrap();
    let outcome = failed_outcome(source.clone(), 1, identity, &[event(2)]);
    let limits = DraftReadLimits {
        max_source_bytes: 65536,
    };
    assert!(copy_test(&outcome, &files.path("missing-dir").join("copy"), limits, 2).is_err());
    let output = files.path("copy");
    copy_test(&outcome, &output, limits, 2).unwrap();
    let mut recovered = reader(&output);
    let mut page = [None; 2];
    assert_eq!(recovered.read_events(0, &mut page).unwrap(), 2);
    assert_eq!(page, [Some(event(1)), Some(event(2))]);
    assert_eq!(recovered.outcome(), DraftOutcome::Interrupted);
    assert_eq!(fs::read(source).unwrap(), bytes);
}

#[test]
fn failed_copy_preserves_capture_clock_with_no_retained_events() {
    let files = Files::new();
    let source = files.path("clocked-source");
    let output = files.path("clocked-copy");
    let (project, target, stamp) = fixture();
    let clock = hex_storage::automation_draft::CaptureClock {
        sample_rate: 48000,
        loop_region: None,
    };
    let mut writer =
        DraftWriter::create_with_clock(&source, &project, target, stamp, clock).unwrap();
    writer.append(&[event(1)]).unwrap();
    let identity = writer.durable_prefix_identity();
    drop(writer);
    let outcome = failed_outcome(source, 1, identity, &[]);
    copy_test(
        &outcome,
        &output,
        DraftReadLimits {
            max_source_bytes: 65536,
        },
        1,
    )
    .unwrap();
    let recovered = reader(&output);
    assert_eq!(recovered.capture_clock(), Some(clock));
    assert_eq!(recovered.progress().durable_events, 1);
    assert_eq!(
        recovered.progress().durable_terminal,
        Some(DraftTerminal::Interrupted)
    );
}
fn send(input: &mut DraftInput, events: &[AppliedEvent]) {
    let mut batch = input.reserve_batch().unwrap().unwrap();
    for event in events {
        batch.push(*event).unwrap();
    }
    batch.submit().unwrap();
}
#[test]
fn three_loan_backpressure_and_independent_terminal_preserve_fifo_and_exact_file() {
    let files = Files::new();
    let path = files.path("take.hex-automation-draft");
    let (project, target, stamp) = fixture();
    let writer = DraftWriter::create_new(&path, &project, target, stamp).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let held = Arc::clone(&barrier);
    let (mut input, worker) = spawn_with_start(writer, move || {
        held.wait();
    })
    .unwrap();
    for sequence in 1..=BATCH_SLOTS as u64 {
        send(&mut input, &[event(sequence)]);
    }
    assert!(
        input.reserve_batch().unwrap().is_none(),
        "leave receipts unconsumed under pressure"
    );
    assert_eq!(input.progress().accepted_events, 3);
    assert_eq!(input.progress().durable_events, 0);
    let end = DraftClock {
        performance_frame: 12,
        content_frame: 12,
        loop_pass: 0,
    };
    input.finish(DraftTerminal::Finished(end)).unwrap();
    assert!(input.reserve_batch().is_err());
    barrier.wait();
    let outcome = worker.join_on_control().unwrap();
    assert!(outcome.failure.is_none());
    assert!(outcome.retained.is_empty());
    assert_eq!(outcome.admitted_events, 3);
    assert_eq!(outcome.progress.durable_events, 3);
    assert_eq!(
        outcome.progress.durable_terminal,
        Some(DraftTerminal::Finished(end))
    );
    assert!(input.progress().finished);
    let mut observed = reader(&path);
    assert_eq!(observed.outcome(), DraftOutcome::Finished(end));
    assert!(observed.source_matches(&project).unwrap());
    let mut page = [None; 5];
    assert_eq!(observed.read_events(0, &mut page).unwrap(), 3);
    assert_eq!(
        page,
        [Some(event(1)), Some(event(2)), Some(event(3)), None, None]
    );
    let mut durable = 0;
    let mut terminal = 0;
    while let Some(ack) = input.poll() {
        match ack {
            DraftWriteAck::Durable { .. } => durable += 1,
            DraftWriteAck::Terminal { failure, .. } => {
                assert!(failure.is_none());
                terminal += 1;
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!((durable, terminal), (3, 1));
}
#[test]
fn repeated_write_read_turnovers_reuse_the_same_three_capture_buffers() {
    let files = Files::new();
    let path = files.path("long.hex-automation-draft");
    let (project, target, stamp) = fixture();
    let writer = DraftWriter::create_new(&path, &project, target, stamp).unwrap();
    let (mut input, worker) = spawn_writer_on_control(writer).unwrap();
    let addresses = input
        .free
        .iter()
        .flatten()
        .map(|batch| batch.as_ref() as *const Batch as usize)
        .collect::<std::collections::BTreeSet<_>>();
    let deadline = Instant::now() + crate::test_wait::HANG_BOUND;
    for batch_index in 0..80 {
        while input.free.iter().all(Option::is_none) {
            while input.poll().is_some() {}
            assert!(Instant::now() < deadline);
            crate::test_wait::pause();
        }
        let mut batch = input.reserve_batch().unwrap().unwrap();
        assert!(
            addresses.contains(&(batch.batch.as_ref().unwrap().as_ref() as *const Batch as usize))
        );
        for offset in 1..=EVENTS_PER_BATCH {
            batch
                .push(event((batch_index * EVENTS_PER_BATCH + offset) as u64))
                .unwrap();
        }
        assert_eq!(batch.remaining(), 0);
        batch.submit().unwrap();
    }
    input.finish(DraftTerminal::Interrupted).unwrap();
    let outcome = worker.join_on_control().unwrap();
    assert!(outcome.failure.is_none());
    let total = 80 * EVENTS_PER_BATCH;
    assert_eq!(outcome.progress.durable_events, total as u64);
    let mut observed = reader(&path);
    let mut page = [None; 13];
    let mut ordinal = 0;
    while ordinal < total {
        let count = observed.read_events(ordinal as u64, &mut page).unwrap();
        assert!(count > 0);
        for (offset, record) in page[..count].iter().enumerate() {
            assert_eq!(*record, Some(event((ordinal + offset + 1) as u64)));
        }
        ordinal += count;
    }
    assert_eq!(observed.outcome(), DraftOutcome::Interrupted);
}
#[test]
fn writer_refusal_retains_all_unsynced_batches_and_never_invents_completion() {
    for explicit_join in [false, true] {
        let files = Files::new();
        let path = files.path("refused.hex-automation-draft");
        let (project, target, stamp) = fixture();
        let writer = DraftWriter::create_new(&path, &project, target, stamp).unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let held = Arc::clone(&barrier);
        let (mut input, worker) = spawn_with_start(writer, move || {
            held.wait();
        })
        .unwrap();
        send(&mut input, &[event(1)]);
        send(&mut input, &[event(1)]); // malformed sequence: owning storage validates before any write
        send(&mut input, &[event(3)]);
        input.finish(DraftTerminal::Interrupted).unwrap();
        barrier.wait();
        let outcome = if explicit_join {
            worker.join_on_control().unwrap()
        } else {
            drop(worker);
            input.try_take_outcome().unwrap()
        };
        assert!(input.try_take_outcome().is_none());
        assert!(outcome.failure.is_some());
        assert_eq!(outcome.admitted_events, 3);
        assert_eq!(outcome.progress.durable_events, 1);
        assert_eq!(outcome.progress.durable_terminal, None);
        assert_eq!(outcome.retained.len(), 2);
        assert_eq!(outcome.retained[0].events(), &[event(1)]);
        assert_eq!(outcome.retained[1].events(), &[event(3)]);
        assert_eq!(reader(&path).outcome(), DraftOutcome::Interrupted);
        assert!(
            reader(&path)
                .matches_prefix(outcome.durable_prefix)
                .unwrap()
        );
        assert!(input.progress().failed);
    }
}
#[test]
fn a_refused_loan_keeps_its_original_source_and_cannot_retry_into_another_writer() {
    let files = Files::new();
    let (project, target, stamp) = fixture();
    let first_path = files.path("first.hex-automation-draft");
    let writer = DraftWriter::create_new(&first_path, &project, target, stamp).unwrap();
    let (mut input, worker) = spawn_writer_on_control(writer).unwrap();
    let mut batch = input.reserve_batch().unwrap().unwrap();
    batch.push(event(1)).unwrap();
    worker.request_interruption_on_control().unwrap();
    let DraftSubmitError::Refused(refused) = batch.submit().unwrap_err() else {
        panic!("owned refusal")
    };
    let second_path = files.path("second.hex-automation-draft");
    let writer = DraftWriter::create_new(&second_path, &project, target, stamp).unwrap();
    let (mut other, other_worker) = spawn_writer_on_control(writer).unwrap();
    let refused = refused.retry(&mut other).unwrap_err();
    assert_eq!(refused.path(), first_path);
    assert_eq!(refused.events(), &[event(1)]);
    assert_eq!(other.progress().accepted_events, 0);
    assert!(worker.join_on_control().unwrap().failure.is_none());
    assert!(other_worker.join_on_control().unwrap().failure.is_none());
    assert_eq!(reader(&first_path).progress().durable_events, 0);
    assert_eq!(reader(&second_path).progress().durable_events, 0);
}
#[test]
fn recovery_reuses_source_preparation_without_saved_association_and_save_as_preserves_draft() {
    use crate::service::{ProjectSource, Request, Service};
    let files = Files::new();
    let (project, target, stamp) = fixture();
    let path = files.path("recover.hex-automation-draft");
    let session = Session::from_loaded(project.clone(), 8, 0, Vec::new()).unwrap();
    let (mut input, worker) =
        start_on_control(&session, None, path.clone(), target, stamp).unwrap();
    send(&mut input, &[event(1)]);
    input.finish(DraftTerminal::Interrupted).unwrap(); // quiescence is not a fabricated native end
    assert!(worker.join_on_control().unwrap().failure.is_none());
    let original = fs::read(&path).unwrap();
    let service = Service::with_recovery(48000, 8, 0, files.path("untitled.hex-journal")).unwrap();
    service
        .events
        .recv_timeout(crate::test_wait::HANG_BOUND)
        .unwrap();
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "later journal".into(),
        }))
        .unwrap();
    service
        .events
        .recv_timeout(crate::test_wait::HANG_BOUND)
        .unwrap();
    service
        .submit(Request::OpenWith {
            source: ProjectSource::AutomationDraft {
                path: path.clone(),
                limits: DraftReadLimits {
                    max_source_bytes: 64 * 1024,
                },
            },
            relinks: Default::default(),
        })
        .unwrap();
    let recovered = service
        .events
        .recv_timeout(crate::test_wait::HANG_BOUND)
        .unwrap();
    assert!(recovered.error.is_none(), "{:?}", recovered.error);
    assert!(recovered.project_replaced);
    assert_eq!(recovered.state.project.as_ref(), &project);
    assert!(recovered.state.document_path.is_none());
    assert!(recovered.state.unsaved);
    let saved = files.path("recovered-copy.hex");
    service.submit(Request::SaveNew(saved.clone())).unwrap();
    let saved_event = service
        .events
        .recv_timeout(crate::test_wait::HANG_BOUND)
        .unwrap();
    assert!(saved_event.error.is_none(), "{:?}", saved_event.error);
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(hex_storage::load(saved).unwrap(), project);
    assert_eq!(reader(&path).outcome(), DraftOutcome::Interrupted);
}
#[test]
fn discovery_is_bounded_and_never_treats_candidate_names_as_valid_files() {
    let files = Files::new();
    fs::write(files.path("unknown.txt"), b"keep").unwrap();
    fs::write(files.path("bad.hex-automation-draft"), b"not a valid draft").unwrap();
    let discovery = discover_on_control(&files.0, 32).unwrap();
    assert!(!discovery.truncated);
    assert_eq!(
        discovery.candidates,
        vec![files.path("bad.hex-automation-draft")]
    );
    assert!(
        DraftReader::open(
            &discovery.candidates[0],
            DraftReadLimits {
                max_source_bytes: 64 * 1024
            }
        )
        .is_err()
    );
    assert_eq!(fs::read(files.path("unknown.txt")).unwrap(), b"keep");
    assert!(discover_on_control(&files.0, 1).unwrap().truncated);
    assert!(discover_on_control(&files.0, 0).is_err());
    for index in 0..7 {
        fs::write(
            files.path(&format!("{index}.hex-automation-draft")),
            b"candidate only",
        )
        .unwrap();
    }
    let mut cursor = DraftDiscoveryCursor::open_on_control(&files.0).unwrap();
    let mut observed = std::collections::BTreeSet::new();
    loop {
        let page = cursor.next_page_on_control(1).unwrap();
        assert!(page.candidates.len() <= 1);
        observed.extend(page.candidates);
        if !page.truncated {
            break;
        }
    }
    assert_eq!(
        observed.len(),
        8,
        "paging does not discard the prefetched entry or impose a total-file ceiling"
    );
}

#[test]
fn failed_control_gate_keeps_admitted_batches_available_without_a_false_terminal() {
    let files = Files::new();
    let path = files.path("control-failure.hex-automation-draft");
    let (project, target, stamp) = fixture();
    let writer = DraftWriter::create_new(&path, &project, target, stamp).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let held = Arc::clone(&barrier);
    let (mut input, worker) = spawn_with_start(writer, move || {
        held.wait();
    })
    .unwrap();
    send(&mut input, &[event(1)]);
    send(&mut input, &[event(2)]);
    let shared = Arc::clone(&input.shared);
    assert!(
        std::thread::spawn(move || {
            let _guard = shared.state.lock().unwrap();
            panic!("controlled gate failure");
        })
        .join()
        .is_err()
    );
    assert!(input.progress().failed);
    barrier.wait();
    let outcome = worker.join_on_control().unwrap();
    assert!(outcome.failure.is_some());
    assert_eq!(outcome.progress.durable_events, 0);
    assert_eq!(outcome.progress.durable_terminal, None);
    assert_eq!(outcome.retained.len(), 2);
    assert_eq!(outcome.retained[0].events(), &[event(1)]);
    assert_eq!(outcome.retained[1].events(), &[event(2)]);
    assert_eq!(reader(&path).outcome(), DraftOutcome::Interrupted);
}

#[test]
fn unexpected_worker_failure_is_visible_after_join_and_leaves_the_source_file() {
    let files = Files::new();
    let path = files.path("worker-failure.hex-automation-draft");
    let (project, target, stamp) = fixture();
    let writer = DraftWriter::create_new(&path, &project, target, stamp).unwrap();
    let (input, worker) =
        spawn_with_start(writer, || panic!("controlled startup failure")).unwrap();
    assert!(worker.join_on_control().is_err());
    assert!(input.progress().failed);
    assert!(input.progress().finished);
    assert_eq!(input.progress().terminal_durable, None);
    assert_eq!(reader(&path).outcome(), DraftOutcome::Interrupted);
}

#[test]
fn native_deliveries_seal_only_the_exact_target_values_and_restoration() {
    use hex_engine::{
        NativeParameterApplication, NativeTransientOperation, NativeTransientValue,
        default_native_device_state, native_parameter_input_target,
    };
    use hex_lattice_contract::{DeviceTypeId, HostGain, ParameterKind, ParameterValue};
    use hex_project::DeviceAutomationTarget;
    use hex_storage::automation_draft::{CaptureClock, CompletedAutomationTake};

    let files = Files::new();
    let mut session = Session::new(48_000, 8, 4096).unwrap();
    let state = default_native_device_state(DeviceTypeId::from_static("hex.utility")).unwrap();
    let view = hex_engine::native_device_view(&state).unwrap();
    let mute = view
        .parameters
        .iter()
        .find(|p| p.descriptor.label == "mute")
        .unwrap()
        .descriptor
        .id;
    let gain = view
        .parameters
        .iter()
        .find(|p| p.descriptor.label == "gain")
        .unwrap()
        .descriptor;
    let ParameterKind::Float { max, .. } = gain.kind else {
        panic!("utility gain domain");
    };
    let Created::Device(device) = session
        .command(Command::InsertMainDevice {
            before: None,
            state,
        })
        .unwrap()
    else {
        panic!("main effect");
    };
    let (_, _, stamp) = fixture();
    let clock = CaptureClock {
        sample_rate: 48_000,
        loop_region: None,
    };
    for (index, (target, value, normalized)) in [
        (
            DeviceAutomationTarget::Parameter(mute),
            NativeTransientValue::Parameter {
                id: mute,
                value: ParameterValue::Bool(true),
            },
            1.0,
        ),
        (
            DeviceAutomationTarget::Parameter(gain.id),
            NativeTransientValue::Parameter {
                id: gain.id,
                value: ParameterValue::Float(max * 0.5),
            },
            0.5,
        ),
        (
            DeviceAutomationTarget::HostOutputGain,
            NativeTransientValue::HostOutputGain(HostGain::from_normalized(0.25).unwrap()),
            0.25,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let path = files.path(&format!("native-{index}.hex-automation-draft"));
        let typed = native_parameter_input_target(session.project(), device, target).unwrap();
        let (mut input, worker) =
            DeviceDraftInput::start_on_control(&session, None, path.clone(), typed, stamp, clock)
                .unwrap();
        let application = NativeParameterApplication {
            identity: typed.identity(),
            operation: NativeTransientOperation::Set(value),
            arrangement_frame: 8,
            loop_pass: 0,
            performance_frame: 16,
            remaining_latency_frames: 4,
        };
        let stale = DraftProvenance {
            audio_revision: stamp.audio_revision + 1,
            ..stamp
        };
        assert!(
            input
                .append_delivered(typed, stale, &[(1, application)])
                .is_err()
        );
        assert!(
            input
                .append_delivered(typed, stamp, &[(1, application), (1, application)])
                .is_err()
        );
        let wrong_target = NativeParameterApplication {
            operation: NativeTransientOperation::Set(NativeTransientValue::HostInputGain(
                HostGain::ZERO,
            )),
            ..application
        };
        assert!(
            input
                .append_delivered(typed, stamp, &[(1, wrong_target)])
                .is_err()
        );
        assert_eq!(input.progress().accepted_events, 0);
        assert!(input.finish_delivered(typed, stamp, application).is_err());
        assert_eq!(
            input
                .append_delivered(typed, stamp, &[(2, application)])
                .unwrap(),
            Some(3)
        );
        let restoration = NativeParameterApplication {
            operation: NativeTransientOperation::Clear(target),
            arrangement_frame: 12,
            performance_frame: 20,
            ..application
        };
        input.finish_delivered(typed, stamp, restoration).unwrap();
        let outcome = worker.join_on_control().unwrap();
        assert_eq!(outcome.progress.durable_events, 1);
        assert!(outcome.failure.is_none());
        let mut take = CompletedAutomationTake::open_on_control(
            &path,
            DraftReadLimits {
                max_source_bytes: 64 * 1024,
            },
        )
        .unwrap();
        assert_eq!(take.target(), DraftTarget::Device { device, target });
        assert_eq!(
            take.end_clock(),
            DraftClock {
                content_frame: 12,
                performance_frame: 20,
                loop_pass: 0
            }
        );
        let mut file = reader(&path);
        let mut records = [None; 2];
        assert_eq!(file.read_events(0, &mut records).unwrap(), 1);
        assert_eq!(
            records[0],
            Some(AppliedEvent {
                sequence: 2,
                performance_frame: 16,
                content_frame: 8,
                loop_pass: 0,
                value: AutomationValue::new(normalized).unwrap(),
            })
        );
        if index == 2 {
            let prepare = super::super::automation_take::prepare_device_take_adoption_on_control;
            assert!(prepare(&session, None, &mut take, stale).is_err());
            let before = session.project().clone();
            let command = prepare(&session, None, &mut take, stamp).unwrap();
            session.command(command).unwrap();
            let lane = session
                .project()
                .device(device)
                .unwrap()
                .automation_target(target)
                .unwrap();
            let beat = |frame| session.project().tempo_map().frame_to_beat(frame).unwrap();
            assert_eq!(lane.value_at(beat(7)), None);
            assert_eq!(
                lane.value_at(beat(8)),
                Some(AutomationValue::new(normalized).unwrap())
            );
            assert_eq!(lane.value_at(beat(12)), None);
            assert!(prepare(&session, None, &mut take, stamp).is_err());
            let recorded = session.project().clone();
            assert!(session.undo());
            assert_eq!(session.project(), &before);
            assert!(session.redo());
            assert_eq!(session.project(), &recorded);
            assert!(path.exists(), "the sealed source remains available");
        }
    }
}
