//! Unit tests for `audio_sink` contracts and `WorkerEvents` scheduling.
use super::worker_events::{BUSY_POLL_INTERVAL, IDLE_GRACE_POLLS, IDLE_POLL_INTERVAL};
use super::*;
use std::sync::{Mutex, atomic::AtomicUsize};
use std::time::Duration;
use std::time::Instant;

#[test]
fn running_main_off_requires_its_explicit_exact_command_input() {
    let mut editor = hex_project::Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: NativeDeviceState::new(hex_lattice_contract::VST3_DEVICE_TYPE_ID, 1, vec![1])
                .unwrap(),
        })
        .unwrap()
    else {
        panic!("Main fixture");
    };
    let previous = hex_engine::AudioSourceContext {
        project_generation: 1,
        editor_revision: 2,
    };
    let candidate = hex_engine::AudioSourceContext {
        editor_revision: 3,
        ..previous
    };
    let intent = ResidentRunningPowerOff {
        owner: hex_engine::ProcessorChainOwner::Main,
        device: device.value(),
        source: ResidentGraphSubmission {
            context: previous,
            output_identity: 1,
            audio_revision: 4,
        },
        binding: hex_lattice_contract::ForeignProcessorBinding {
            root: 1,
            instance: 2,
            graph: 3,
            epoch: 4,
        },
    };
    editor
        .execute(Command::SetDeviceBypass {
            device,
            bypassed: true,
        })
        .unwrap();
    let selected = |project: &Project, context, preparing_edit, running_power_off| {
        ResidentEffectPreparation {
            project,
            context,
            preparing_edit,
            running_power_off,
            hosted_move: None,
            reload_plugin_device: None,
            restoring_plugin_state: false,
            power_on: None,
        }
        .running_power_off()
    };
    assert_eq!(
        selected(editor.project(), candidate, true, Some(intent)),
        Some(intent)
    );
    assert_eq!(selected(editor.project(), candidate, true, None), None);
    assert_eq!(
        selected(editor.project(), previous, true, Some(intent)),
        None
    );
    assert_eq!(
        selected(editor.project(), candidate, false, Some(intent)),
        None
    );
    assert_eq!(
        selected(
            editor.project(),
            candidate,
            true,
            Some(ResidentRunningPowerOff {
                device: device.value() + 1,
                ..intent
            })
        ),
        None
    );
    editor
        .execute(Command::SetDeviceBypass {
            device,
            bypassed: false,
        })
        .unwrap();
    assert_eq!(
        selected(editor.project(), candidate, true, Some(intent)),
        None
    );
}

#[test]
fn fresh_main_on_requires_the_explicit_power_input_and_unchanged_target() {
    let mut editor = hex_project::Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: NativeDeviceState::new(hex_lattice_contract::VST3_DEVICE_TYPE_ID, 1, vec![1])
                .unwrap(),
        })
        .unwrap()
    else {
        panic!("main");
    };
    let source_context = hex_engine::AudioSourceContext {
        project_generation: 1,
        editor_revision: 2,
    };
    let candidate = hex_engine::AudioSourceContext {
        editor_revision: 3,
        ..source_context
    };
    let intent = ResidentPowerOn {
        device: device.value(),
        interval: ResidentPowerOnInterval::Stopped {
            owner: hex_engine::ProcessorChainOwner::Main,
            context: source_context,
            fence: ResidentFreshRuntimeFence {
                output_identity: 1,
                active_audio_revision: 2,
                route_generation: 3,
                admitted_command_ticket: 4,
                output_fault_epoch: 5,
                consumed_stop_generation: 6,
            },
        },
    };
    let selected = |project: &Project, context, power_on| {
        ResidentEffectPreparation {
            project,
            context,
            restoring_plugin_state: false,
            preparing_edit: true,
            power_on,
            running_power_off: None,
            hosted_move: None,
            reload_plugin_device: None,
        }
        .power_on()
    };
    assert_eq!(selected(editor.project(), candidate, None), None);
    assert_eq!(
        selected(editor.project(), candidate, Some(intent)),
        Some(intent)
    );
    let running = ResidentPowerOn {
        interval: ResidentPowerOnInterval::Running {
            owner: hex_engine::ProcessorChainOwner::Main,
            source: ResidentGraphSubmission {
                context: source_context,
                audio_revision: 2,
                output_identity: 1,
            },
        },
        ..intent
    };
    assert_eq!(
        selected(editor.project(), candidate, Some(running)),
        Some(running)
    );
    assert_eq!(
        selected(editor.project(), source_context, Some(running)),
        None
    );
    assert_eq!(
        selected(editor.project(), source_context, Some(intent)),
        None
    );
    assert_eq!(
        selected(
            editor.project(),
            candidate,
            Some(ResidentPowerOn {
                device: device.value() + 1,
                ..intent
            })
        ),
        None
    );
    editor
        .execute(Command::SetDeviceBypass {
            device,
            bypassed: true,
        })
        .unwrap();
    assert_eq!(selected(editor.project(), candidate, Some(intent)), None);
    editor.execute(Command::RemoveDevice(device)).unwrap();
    assert_eq!(selected(editor.project(), candidate, Some(intent)), None);
}

#[test]
fn only_the_sole_played_bypassed_main_is_a_saved_off_declaration() {
    let mut editor = hex_project::Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let state =
        NativeDeviceState::new(hex_lattice_contract::VST3_DEVICE_TYPE_ID, 1, vec![1]).unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: state.clone(),
        })
        .unwrap()
    else {
        panic!("main");
    };
    let selected = |project: &Project| {
        ResidentEffectPreparation {
            project,
            context: hex_engine::AudioSourceContext {
                project_generation: 1,
                editor_revision: 0,
            },
            restoring_plugin_state: false,
            preparing_edit: false,
            power_on: None,
            running_power_off: None,
            hosted_move: None,
            reload_plugin_device: None,
        }
        .powered_off_main()
        .map(|(id, _)| id)
    };
    assert_eq!(selected(editor.project()), None);
    editor
        .execute(Command::SetDeviceBypass {
            device,
            bypassed: true,
        })
        .unwrap();
    assert_eq!(selected(editor.project()), Some(device.value()));
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "detached".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    editor
        .execute(Command::InsertInstrument { track, state })
        .unwrap();
    assert_eq!(selected(editor.project()), None);
    editor.execute(Command::RemoveInstrument { track }).unwrap();
    assert_eq!(selected(editor.project()), Some(device.value()));
}

#[test]
fn detached_hosted_instrument_keeps_authored_state_without_playback_membership() {
    let mut editor = hex_project::Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "midi".into(),
        })
        .unwrap()
    else {
        panic!("midi track");
    };
    let state =
        NativeDeviceState::new(hex_lattice_contract::VST3_DEVICE_TYPE_ID, 1, vec![1]).unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertInstrument {
            track,
            state: state.clone(),
        })
        .unwrap()
    else {
        panic!("instrument");
    };
    let playback_bypass = |project: &Project| {
        ResidentEffectPreparation {
            project,
            context: hex_engine::AudioSourceContext {
                project_generation: 0,
                editor_revision: 0,
            },
            restoring_plugin_state: false,
            preparing_edit: false,
            power_on: None,
            running_power_off: None,
            hosted_move: None,
            reload_plugin_device: None,
        }
        .plugin_device_bypassed(device.value())
    };
    assert_eq!(playback_bypass(editor.project()), Some(false));
    editor.execute(Command::RemoveInstrument { track }).unwrap();
    assert_eq!(editor.project().device(device).unwrap().state(), &state);
    assert_eq!(playback_bypass(editor.project()), None);
    assert!(editor.undo());
    assert_eq!(playback_bypass(editor.project()), Some(false));
}

#[test]
fn retained_outcomes_keep_their_request_identity_and_order() {
    use super::super::diagnostics::{DiagnosticOrigin, DiagnosticRequest, Hooks};
    use std::sync::atomic::AtomicU64;

    let mut service = Service::new(48_000, 8, 1024).unwrap();
    let first = service
        .events
        .recv_timeout(crate::test_wait::HANG_BOUND)
        .unwrap();
    service
        .submit(Request::Edit(Command::AddTrack { name: "one".into() }))
        .unwrap();
    let second = service
        .events
        .recv_timeout(crate::test_wait::HANG_BOUND)
        .unwrap();
    service.shutdown(crate::test_wait::HANG_BOUND).unwrap();
    let first_context = first
        .state
        .canonical_audio_source
        .as_ref()
        .unwrap()
        .context();
    let second_context = second
        .state
        .canonical_audio_source
        .as_ref()
        .unwrap()
        .context();

    let (sender, receiver) = std::sync::mpsc::sync_channel(2);
    let observed = Arc::new(Observation::default());
    let mut events = WorkerEvents::new_with_diagnostics(
        sender,
        Arc::new(AtomicBool::new(false)),
        Some(Box::new(Sink {
            observed: Arc::clone(&observed),
            fail: false,
            idle: true,
        })),
        Arc::new(Hooks::default()),
    );
    let intent = AtomicU64::new(0);
    let mut queued = QueuedRequest::continuation(Request::CopyClips(Vec::new()), &intent);
    queued.diagnostic = Some(DiagnosticRequest {
        request_id: 1,
        origin: DiagnosticOrigin::Submitted,
        ui_action_id: None,
        kind: "first",
        operation: None,
    });
    events.begin_diagnostics(&queued, 0, first.state.revision);
    let first_revision = first.state.revision;
    let first = events.capture_outcome(first);
    queued.diagnostic.as_mut().unwrap().request_id = 2;
    events.begin_diagnostics(&queued, 0, second.state.revision);
    let second_revision = second.state.revision;
    let second = events.capture_outcome(second);
    {
        let sources = observed.sources.lock().unwrap();
        assert_eq!(
            sources
                .iter()
                .map(|(generation, revision, _)| (*generation, *revision))
                .collect::<Vec<_>>(),
            vec![
                (
                    first_context.project_generation,
                    first_context.editor_revision
                ),
                (
                    second_context.project_generation,
                    second_context.editor_revision
                ),
            ]
        );
    }
    assert!(matches!(
        receiver.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    assert!(events.send_pending_with_control_poll(first, |_| ()));
    assert!(events.send_pending_with_control_poll(second, |_| ()));
    let first = receiver.try_recv().unwrap();
    let second = receiver.try_recv().unwrap();
    assert_eq!(
        (
            first.request.unwrap().request_id,
            first.event.state.revision
        ),
        (1, first_revision)
    );
    assert_eq!(
        (
            second.request.unwrap().request_id,
            second.event.state.revision
        ),
        (2, second_revision)
    );
    let sources = observed.sources.lock().unwrap();
    assert_eq!(
        sources
            .iter()
            .map(|(generation, revision, _)| (*generation, *revision))
            .collect::<Vec<_>>(),
        vec![
            (
                first_context.project_generation,
                first_context.editor_revision
            ),
            (
                second_context.project_generation,
                second_context.editor_revision
            ),
        ]
    );
}
#[derive(Default)]
struct Observation {
    sources: Mutex<Vec<(u64, u64, thread::ThreadId)>>,
    polls: AtomicUsize,
    closed: Mutex<Option<thread::ThreadId>>,
    restarts: Mutex<Vec<(u64, u64, thread::ThreadId)>>,
    waits: Mutex<Vec<Duration>>,
    wait_changed: std::sync::Condvar,
    accelerate_polls: AtomicBool,
    idle_timeout_once: AtomicBool,
}
impl Observation {
    fn wait_for_waits(&self, mut ready: impl FnMut(&[Duration]) -> bool) -> Vec<Duration> {
        let deadline = Instant::now() + crate::test_wait::HANG_BOUND;
        let mut waits = self.waits.lock().unwrap();
        while !ready(&waits) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "timed out waiting for poll waits: {waits:?}"
            );
            let (next, _) = self.wait_changed.wait_timeout(waits, remaining).unwrap();
            waits = next;
        }
        waits.clone()
    }
}
struct Sink {
    observed: Arc<Observation>,
    fail: bool,
    idle: bool,
}
impl WorkerAudioSink for Sink {
    fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
        if self.fail {
            Err("device unavailable".into())
        } else {
            Ok(44100)
        }
    }
    fn canonical_changed(&mut self, source: Arc<CanonicalAudioSourceSnapshot>) {
        let context = source.context();
        self.observed.sources.lock().unwrap().push((
            context.project_generation,
            context.editor_revision,
            thread::current().id(),
        ));
    }
    fn restart_audio(&mut self, generation: u64, request_id: u64) -> Result<(), String> {
        self.observed.restarts.lock().unwrap().push((
            generation,
            request_id,
            thread::current().id(),
        ));
        Ok(())
    }
    fn poll(&mut self) {
        self.observed.polls.fetch_add(1, Ordering::SeqCst);
    }
    fn worker_idle(&mut self) -> bool {
        self.idle
    }
    fn test_poll_wait(&mut self, wait: Duration) -> Duration {
        {
            let mut waits = self.observed.waits.lock().unwrap();
            waits.push(wait);
            self.observed.wait_changed.notify_all();
        }
        if !self.observed.accelerate_polls.load(Ordering::Acquire) {
            return wait;
        }
        if wait == BUSY_POLL_INTERVAL
            || self
                .observed
                .idle_timeout_once
                .swap(false, Ordering::AcqRel)
        {
            Duration::ZERO
        } else {
            wait
        }
    }
    fn close(&mut self) {
        *self.observed.closed.lock().unwrap() = Some(thread::current().id());
    }
}
fn wait(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + crate::test_wait::HANG_BOUND;
    while !ready() {
        assert!(Instant::now() < deadline);
        crate::test_wait::pause();
    }
}
fn saved_effect_state(class: [u8; 16]) -> NativeDeviceState {
    hex_plugin_state::encode(hex_plugin_state::State {
        identity: hex_plugin_state::Identity {
            class: hex_plugin_state::ClassId::new(class).unwrap(),
            binary: hex_plugin_state::BinaryIdentity {
                executable_sha256: [2; 32],
                info_sha256: [3; 32],
            },
        },
        component: Some(&[4]),
        controller: None,
    })
    .unwrap()
}

#[test]
fn toggling_a_compatible_loop_keeps_the_authored_effect_worker() {
    use hex_project::{
        AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, Command, Created,
        DeviceAutomationTarget, Editor,
    };

    fn prepare(project: &Project) -> ResidentEffectPreparation<'_> {
        ResidentEffectPreparation {
            project,
            context: hex_engine::AudioSourceContext {
                project_generation: 1,
                editor_revision: 1,
            },
            restoring_plugin_state: false,
            preparing_edit: true,
            power_on: None,
            running_power_off: None,
            hosted_move: None,
            reload_plugin_device: None,
        }
    }

    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: saved_effect_state([1; 16]),
        })
        .unwrap()
    else {
        panic!("plugin fixture");
    };
    let automation = |value| Command::ReplaceDeviceAutomationTarget {
        device,
        target: DeviceAutomationTarget::Vst3 { parameter: 7 },
        points: vec![AutomationPoint {
            beat: BeatTime::ZERO,
            value: AutomationValue::new(value).unwrap(),
            outgoing: AutomationInterpolation::Step,
        }],
    };
    editor.execute(automation(0.5)).unwrap();
    editor
        .execute(Command::SetLoopRegion {
            start: 1024,
            end: 4096,
            enabled: true,
        })
        .unwrap();
    let before = editor.project().clone();

    editor.execute(Command::ToggleLoopRegion).unwrap();
    let keeps_worker = |old: &Project, new: &Project| {
        prepare(new).same_authored_plugin_program(&prepare(old), device.value(), Some(256))
    };
    let disabled = editor.project().clone();
    assert!(keeps_worker(&before, &disabled));
    assert!(keeps_worker(&disabled, &before));

    editor
        .execute(Command::SetLoopRegion {
            start: 0,
            end: 1024,
            enabled: true,
        })
        .unwrap();
    assert!(keeps_worker(&disabled, editor.project()));

    editor.execute(automation(0.75)).unwrap();
    assert!(!keeps_worker(&disabled, editor.project()));
    assert!(editor.undo());
    editor
        .execute(Command::SetLoopRegion {
            start: 0,
            end: 1025,
            enabled: true,
        })
        .unwrap();
    assert!(!keeps_worker(&disabled, editor.project()));
}

#[test]
fn plugin_source_selection_retains_base_lanes_as_well_as_reading_lanes() {
    use hex_project::{
        AutomationInterpolation, AutomationPoint, AutomationReadSelection, AutomationValue,
        BeatTime, Command, Created, DeviceAutomationTarget, Editor,
    };
    let class = [1; 16];
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: saved_effect_state(class),
        })
        .unwrap()
    else {
        panic!("plugin fixture");
    };
    for parameter in [7, 11] {
        editor
            .execute(Command::ReplaceDeviceAutomationTarget {
                device,
                target: DeviceAutomationTarget::Vst3 { parameter },
                points: vec![AutomationPoint {
                    beat: BeatTime::ZERO,
                    value: AutomationValue::new(0.5).unwrap(),
                    outgoing: AutomationInterpolation::Step,
                }],
            })
            .unwrap();
    }
    editor
        .execute(Command::SetDeviceAutomationReadSelection {
            device,
            target: DeviceAutomationTarget::Vst3 { parameter: 7 },
            selection: AutomationReadSelection::Base,
        })
        .unwrap();
    let input = ResidentEffectPreparation {
        project: editor.project(),
        context: hex_engine::AudioSourceContext {
            project_generation: 1,
            editor_revision: 1,
        },
        restoring_plugin_state: false,
        preparing_edit: true,
        power_on: None,
        running_power_off: None,
        hosted_move: None,
        reload_plugin_device: None,
    };
    assert_eq!(
        input
            .authored_plugin_targets(device.value())
            .collect::<Vec<_>>(),
        vec![(7, Some(class)), (11, Some(class))]
    );
    assert_eq!(input.modulated_plugin_target(device.value()).unwrap(), None);
    use hex_project::{
        DeviceModulation, DeviceModulationRoute, ModulationAmount, ModulationDestination,
        ModulationOutputId, ModulationRouteId, ModulationSourceId, Modulator,
    };
    let first = editor.project().next_identity();
    let source = ModulationSourceId::new(first).unwrap();
    editor
        .execute(Command::ReplaceDeviceModulation {
            device,
            modulation: DeviceModulation::new(
                vec![Modulator {
                    id: source,
                    color: Default::default(),
                    enabled: false,
                    state: hex_engine::lfo_modulator::Settings::default()
                        .encode()
                        .unwrap(),
                }],
                vec![DeviceModulationRoute {
                    id: ModulationRouteId::new(first + 1).unwrap(),
                    source,
                    output: ModulationOutputId::MAIN,
                    destination: ModulationDestination::Device {
                        device_type: hex_plugin_state::TYPE_ID,
                        target: DeviceAutomationTarget::Vst3 { parameter: 0 },
                        vst3_class: Some(class),
                    },
                    amount: ModulationAmount::new(0.25).unwrap(),
                    range: Default::default(),
                    enabled: false,
                }],
            )
            .unwrap(),
        })
        .unwrap();
    let projected = |project: &Project, reopen| {
        ResidentEffectPreparation {
            project,
            context: hex_engine::AudioSourceContext {
                project_generation: 1,
                editor_revision: 2,
            },
            restoring_plugin_state: reopen,
            preparing_edit: !reopen,
            power_on: None,
            running_power_off: None,
            hosted_move: None,
            reload_plugin_device: None,
        }
        .modulated_plugin_target(device.value())
        .unwrap()
    };
    assert_eq!(projected(editor.project(), false), Some((0, class)));
    assert_eq!(projected(editor.project(), true), Some((0, class)));
    assert!(editor.undo());
    assert_eq!(projected(editor.project(), false), None);
    assert!(editor.redo());
    assert_eq!(projected(editor.project(), false), Some((0, class)));
}

#[test]
fn worker_uses_negotiated_rate_and_keeps_cleanup_live_under_event_backpressure_and_close() {
    let observed = Arc::new(Observation::default());
    let service = Service::with_audio_sink(
        16,
        0,
        None,
        Box::new(Sink {
            observed: observed.clone(),
            fail: false,
            idle: false,
        }),
    )
    .unwrap();
    let caller = thread::current().id();
    // Initial + first edit fill the two event slots; the second edit must
    // notify native custody before waiting for its canonical event delivery.
    for name in ["one", "two"] {
        service
            .submit(Request::Edit(Command::AddTrack { name: name.into() }))
            .unwrap();
    }
    wait(|| observed.sources.lock().unwrap().len() == 3);
    let polls = observed.polls.load(Ordering::SeqCst);
    wait(|| observed.polls.load(Ordering::SeqCst) > polls + 2);
    for revision in 0..=2 {
        let event = service
            .events
            .recv_timeout(crate::test_wait::HANG_BOUND)
            .unwrap();
        assert_eq!(event.state.project.sample_rate(), 44100);
        assert_eq!(event.state.revision, revision);
    }
    let sources = observed.sources.lock().unwrap();
    assert_eq!(
        sources.iter().map(|s| (s.0, s.1)).collect::<Vec<_>>(),
        [(0, 0), (0, 1), (0, 2)]
    );
    assert!(sources.iter().all(|s| s.2 != caller));
    drop(sources);
    // Close while the event queue is full again. No UI receive or progress
    // poll is needed to relinquish the worker's publication/output bundle.
    for _ in 0..3 {
        service.submit(Request::Undo).unwrap();
    }
    wait(|| observed.sources.lock().unwrap().len() >= 5);
    drop(service);
    wait(|| observed.closed.lock().unwrap().is_some());
    assert_ne!(observed.closed.lock().unwrap().unwrap(), caller);
}
#[test]
fn restart_is_generation_checked_worker_only_and_does_not_edit_canonical_state() {
    let observed = Arc::new(Observation::default());
    let service = Service::with_audio_sink(
        16,
        0,
        None,
        Box::new(Sink {
            observed: observed.clone(),
            fail: false,
            idle: false,
        }),
    )
    .unwrap();
    let initial = service
        .events
        .recv_timeout(crate::test_wait::HANG_BOUND)
        .unwrap();
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "track".into(),
        }))
        .unwrap();
    let edited = service
        .events
        .recv_timeout(crate::test_wait::HANG_BOUND)
        .unwrap();
    assert!(edited.error.is_none());
    assert!(edited.state.can_undo);
    for (generation, id, valid) in [(1, 1, false), (0, 0, false), (0, 2, true)] {
        service
            .submit(Request::RestartAudio {
                project_generation: generation,
                request_id: id,
            })
            .unwrap();
        let event = service
            .events
            .recv_timeout(crate::test_wait::HANG_BOUND)
            .unwrap();
        assert_eq!(event.state.revision, edited.state.revision);
        assert_eq!(event.state.project, edited.state.project);
        assert_eq!(event.state.can_undo, edited.state.can_undo);
        assert_eq!(event.state.can_redo, edited.state.can_redo);
        assert_eq!(
            event
                .state
                .canonical_audio_source
                .as_ref()
                .unwrap()
                .context()
                .project_generation,
            initial
                .state
                .canonical_audio_source
                .as_ref()
                .unwrap()
                .context()
                .project_generation
        );
        assert_eq!(event.error.is_none(), valid);
        assert!(event.playback.is_none());
    }
    let rows = observed.restarts.lock().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].0, rows[0].1), (0, 2));
    assert_ne!(rows[0].2, thread::current().id());
    drop(rows);
    service.submit(Request::Undo).unwrap();
    let undone = service
        .events
        .recv_timeout(crate::test_wait::HANG_BOUND)
        .unwrap();
    assert!(undone.error.is_none());
    assert!(undone.state.project.tracks().is_empty());
    service.submit(Request::Redo).unwrap();
    let redone = service
        .events
        .recv_timeout(crate::test_wait::HANG_BOUND)
        .unwrap();
    assert!(redone.error.is_none());
    assert_eq!(redone.state.project, edited.state.project);
}
#[test]
fn failed_device_initialization_keeps_offline_canonical_edits_available() {
    let observed = Arc::new(Observation::default());
    let service = Service::with_audio_sink(
        16,
        0,
        None,
        Box::new(Sink {
            observed: observed.clone(),
            fail: true,
            idle: false,
        }),
    )
    .unwrap();
    let event = service
        .events
        .recv_timeout(crate::test_wait::HANG_BOUND)
        .unwrap();
    assert_eq!(event.state.project.sample_rate(), 48000);
    assert_eq!(
        event.error.as_deref(),
        Some("audio unavailable: device unavailable")
    );
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "offline".into(),
        }))
        .unwrap();
    let edited = service
        .events
        .recv_timeout(crate::test_wait::HANG_BOUND)
        .unwrap();
    assert!(edited.error.is_none());
    assert_eq!(edited.state.project.tracks().len(), 1);
    assert!(edited.state.unsaved);
    drop(service);
    wait(|| observed.closed.lock().unwrap().is_some());
}
#[test]
fn idle_interval_applies_only_after_the_grace_polls() {
    assert_eq!(poll_interval(0), BUSY_POLL_INTERVAL);
    assert_eq!(poll_interval(IDLE_GRACE_POLLS), BUSY_POLL_INTERVAL);
    assert_eq!(poll_interval(IDLE_GRACE_POLLS + 1), IDLE_POLL_INTERVAL);
    assert_eq!(poll_interval(u32::MAX), IDLE_POLL_INTERVAL);
}
#[test]
fn idle_sink_backs_off_while_requests_and_busy_sinks_keep_the_prompt_cadence() {
    let start = |idle, idle_timeout_once| {
        let observed = Arc::new(Observation::default());
        observed.accelerate_polls.store(true, Ordering::Release);
        observed
            .idle_timeout_once
            .store(idle_timeout_once, Ordering::Release);
        let service = Service::with_audio_sink(
            16,
            0,
            None,
            Box::new(Sink {
                observed: observed.clone(),
                fail: false,
                idle,
            }),
        )
        .unwrap();
        let initial = service
            .events
            .recv_timeout(crate::test_wait::HANG_BOUND)
            .unwrap();
        assert!(initial.error.is_none());
        (service, observed)
    };
    let (idle, idle_obs) = start(true, true);
    let (busy, busy_obs) = start(false, false);

    // Idle sink runs IDLE_GRACE_POLLS busy polls, backs off to IDLE_POLL_INTERVAL,
    // and stays backed off across subsequent idle timeouts.
    let initial_idle_waits = idle_obs
        .wait_for_waits(|waits| waits.iter().filter(|d| **d == IDLE_POLL_INTERVAL).count() >= 2);
    let grace = IDLE_GRACE_POLLS as usize;
    assert_eq!(
        &initial_idle_waits[..grace],
        vec![BUSY_POLL_INTERVAL; grace].as_slice(),
        "idle worker should stay on the busy cadence for the full grace window"
    );
    assert_eq!(initial_idle_waits[grace], IDLE_POLL_INTERVAL);
    assert_eq!(initial_idle_waits[grace + 1], IDLE_POLL_INTERVAL);

    // Busy sink stays on BUSY_POLL_INTERVAL past the grace window and never backs off.
    let busy_waits = busy_obs.wait_for_waits(|waits| waits.len() >= grace + 10);
    assert!(
        busy_waits.iter().all(|wait| *wait == BUSY_POLL_INTERVAL),
        "busy worker unexpectedly backed off: {busy_waits:?}"
    );
    assert!(
        busy.worker_wakes() >= (grace + 10) as u64,
        "busy worker wakes should track poll iterations"
    );

    // A request ends the idle wait immediately through the channel receive,
    // resets the grace window to BUSY_POLL_INTERVAL, and backs off again.
    for name in ["one", "two", "three"] {
        let start_index = idle_obs.waits.lock().unwrap().len();
        idle.submit(Request::Edit(Command::AddTrack { name: name.into() }))
            .unwrap();
        let event = idle
            .events
            .recv_timeout(crate::test_wait::HANG_BOUND)
            .unwrap();
        assert!(event.error.is_none());
        let post_waits = idle_obs.wait_for_waits(|waits| {
            let tail = &waits[start_index..];
            let after_leading_idle = tail
                .iter()
                .position(|wait| *wait == BUSY_POLL_INTERVAL)
                .map(|idx| &tail[idx..])
                .unwrap_or(&[]);
            after_leading_idle.contains(&IDLE_POLL_INTERVAL)
        });
        let tail = &post_waits[start_index..];
        let busy_start = tail
            .iter()
            .position(|wait| *wait == BUSY_POLL_INTERVAL)
            .expect("request must restore busy grace cadence");
        let cycle = &tail[busy_start..];
        assert_eq!(
            &cycle[..grace],
            vec![BUSY_POLL_INTERVAL; grace].as_slice(),
            "request `{name}` should restore full busy grace cadence before backing off"
        );
        assert_eq!(cycle[grace], IDLE_POLL_INTERVAL);
    }
}

#[test]
fn submission_matches_stopped_history_predecessor_across_applied_pending_and_refused() {
    let context = hex_engine::AudioSourceContext {
        project_generation: 1,
        editor_revision: 2,
    };
    let submission = ResidentGraphSubmission {
        output_identity: 1,
        audio_revision: 11,
        context,
    };
    let runtime = ResidentRuntimeLoop {
        source_origin: ResidentSourceOrigin::Unknown,
        output_identity: 1,
        audio_revision: 11,
        source_context: Some(context),
        section_loop: None,
    };
    assert!(submission.matches_stopped_history_predecessor(
        ResidentPublicationReadiness::Applied {
            submission: runtime,
            publication_in_flight: false,
        }
    ));
    assert!(submission.matches_stopped_history_predecessor(
        ResidentPublicationReadiness::Applied {
            submission: runtime,
            publication_in_flight: true,
        }
    ));
    assert!(submission.matches_stopped_history_predecessor(
        ResidentPublicationReadiness::Pending {
            submission: runtime,
            applied_revision: 10,
            publication_in_flight: true,
        }
    ));
    // A later candidate (12) refused while 11 remained applied still matches 11.
    assert!(submission.matches_stopped_history_predecessor(
        ResidentPublicationReadiness::Refused {
            submission: ResidentRuntimeLoop {
                audio_revision: 12,
                ..runtime
            },
            applied_revision: 11,
            publication_in_flight: false,
        }
    ));
    assert!(
        !submission.matches_stopped_history_predecessor(ResidentPublicationReadiness::NoOutput)
    );
    assert!(
        !submission.matches_stopped_history_predecessor(ResidentPublicationReadiness::Idle {
            output_identity: 1,
            applied_revision: 11,
            publication_in_flight: false,
        })
    );
    // If 11 itself was refused (10 remained applied), 11 is not installed.
    assert!(!submission.matches_stopped_history_predecessor(
        ResidentPublicationReadiness::Refused {
            submission: runtime,
            applied_revision: 10,
            publication_in_flight: false,
        }
    ));
    assert!(!submission.matches_stopped_history_predecessor(
        ResidentPublicationReadiness::Pending {
            submission: ResidentRuntimeLoop {
                audio_revision: 10,
                ..runtime
            },
            applied_revision: 10,
            publication_in_flight: true,
        }
    ));
    assert!(!submission.matches_stopped_history_predecessor(
        ResidentPublicationReadiness::Pending {
            submission: ResidentRuntimeLoop {
                output_identity: 2,
                ..runtime
            },
            applied_revision: 10,
            publication_in_flight: true,
        }
    ));
    assert!(!submission.matches_stopped_history_predecessor(
        ResidentPublicationReadiness::Pending {
            submission: ResidentRuntimeLoop {
                source_context: Some(hex_engine::AudioSourceContext {
                    editor_revision: 3,
                    ..context
                }),
                ..runtime
            },
            applied_revision: 10,
            publication_in_flight: true,
        }
    ));
}

#[test]
fn stalled_event_channel_returns_within_bound_coalesces_and_flushes_on_drain() {
    let session = Session::new(48_000, 8, 1024).unwrap();
    let make_event = |revision: u64| {
        let project = session.project_arc();
        let state = Arc::new(ViewState {
            canonical_audio_source: None,
            comparison_audio: None,
            gain_preview: hex_engine::GainAutomationPreview::prepare(&project).map(Arc::new),
            project,
            waveforms: Arc::new(session.waveforms()),
            source_overviews: session.source_overviews().at_context(
                hex_engine::AudioSourceContext {
                    project_generation: 1,
                    editor_revision: revision,
                },
            ),
            audio_clip_waveforms: Default::default(),
            midi_preview: Arc::new(hex_engine::MidiArrangementPreview::default()),
            timeline_frames: 48_000 * 8,
            can_undo: false,
            can_redo: false,
            revision,
            unsaved: false,
            document_path: None,
        });
        Event {
            state,
            playback: None,
            status: None,
            error: None,
            resident_edit_source_unavailable: None,
            recovery_failure: None,
            audio_batch_failure: None,
            import_truncation: None,
            request_complete: true,
            sampler_load_complete: None,
            sampler_edit_complete: None,
            plugin_effect_complete: Vec::new(),
            device_insert_proposal_complete: None,
            device_insert_confirmation: None,
            device_insert_complete: None,
            plugin_parameter_observation: false,
            plugin_parameters: None,
            plugin_parameter_conversion: None,
            computer_keyboard_complete: None,
            midi_take_complete: None,
            device_automation_take_complete: None,
            tuning_audition_complete: None,
            live_expression_complete: None,
            bounce_complete: None,
            audio_batch_complete: None,
            save_complete: false,
            save_correlation: None,
            document_format_upgrade_required: false,
            save_confirmation_required: false,
            open_complete: false,
            project_replaced: false,
            stopped_playback_restore_complete: false,
            created: None,
            duplicate_complete: false,
            restore_complete: None,
            device_preview: None,
            gesture_ack: None,
            gesture_protocol_ack: false,
            gesture_request: false,
            active_device_gesture: None,
            tuning_preview: None,
            tuning_gesture_ack: None,
            tuning_gesture_request: false,
            active_tuning_gesture: None,
            tuning_library: None,
        }
    };
    let (tx, rx) = mpsc::sync_channel(1);
    let wakes = Arc::new(AtomicUsize::new(0));
    let wake_hook = super::client::WakeHook::default();
    let wake_counter = Arc::clone(&wakes);
    wake_hook.set(Some(Arc::new(move || {
        wake_counter.fetch_add(1, Ordering::AcqRel);
    })));
    let mut events =
        WorkerEvents::new(tx, Arc::new(AtomicBool::new(false)), None).with_wake_hook(wake_hook);

    // Fill the single-slot channel with revision 1.
    let pending1 = events.capture_outcome(make_event(1));
    assert!(events.send_pending_with_control_poll_bounded(
        pending1,
        Duration::from_millis(20),
        |_| {}
    ));
    assert_eq!(events.backpressure_coalesced(), 0);

    // Revision 2 hits full channel and parks within the bounded wait instead of hanging.
    let started = Instant::now();
    let pending2 = events.capture_outcome(make_event(2));
    assert!(events.send_pending_with_control_poll_bounded(
        pending2,
        Duration::from_millis(20),
        |_| {}
    ));
    assert!(
        started.elapsed() < Duration::from_millis(250),
        "bounded backpressure exceeded expected bound: {:?}",
        started.elapsed()
    );
    assert!(events.has_stalled_outcome());
    assert_eq!(events.backpressure_coalesced(), 1);
    assert!(wakes.load(Ordering::Acquire) >= 2);

    // Revision 3 coalesces into the stalled slot without deadlocking.
    let pending3 = events.capture_outcome(make_event(3));
    assert!(events.send_pending_with_control_poll_bounded(
        pending3,
        Duration::from_millis(10),
        |_| {}
    ));
    assert_eq!(events.backpressure_coalesced(), 2);

    // Once the UI drains revision 1, poll() delivers the coalesced revision 3.
    let first = rx.try_recv().unwrap();
    assert_eq!(first.state.revision, 1);
    events.poll();
    assert!(!events.has_stalled_outcome());
    let flushed = rx.try_recv().unwrap();
    assert_eq!(flushed.state.revision, 3);
}
