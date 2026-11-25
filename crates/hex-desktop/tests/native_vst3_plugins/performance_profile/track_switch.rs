//! Measure submission to the first callback installing a switch, its early
//! control view and its reliable terminal on the same playing mixed graph.
use super::*;
use hex_session::device_gesture::{GestureClient, ParameterId, ParameterPairValue, ParameterValue};
use hex_session::service::ViewState;
use hex_session::service::diagnostics::{DiagnosticObserver, DiagnosticStage};

const WARMUP_SWITCHES: usize = 8;
const MEASURED: usize = 32;

#[test]
#[ignore = "optimized signed VST3 playback measurement"]
fn playing_switches_with_idle_controls() {
    profile(16, 44_100, PlaybackProfile::TrackSwitch { paired: false });
}

#[test]
#[ignore = "optimized signed VST3 playback measurement"]
fn playing_switches_with_changing_eq() {
    profile(16, 44_100, PlaybackProfile::TrackSwitch { paired: true });
}

pub(super) fn measure(
    harness: &mut ResidentHarness,
    state: &ViewState,
    paired: bool,
    budget: Duration,
) {
    let track = state.project.tracks()[0].id();
    render_paced_fixture_blocks(harness, budget, 8);
    let (send, phases) = std::sync::mpsc::sync_channel(4);
    harness
        .service
        .set_diagnostic_observer(std::sync::Arc::new(DiagnosticObserver::new(move |event| {
            if matches!(
                event.stage,
                DiagnosticStage::OutcomeReady {
                    request_complete: true,
                    ..
                }
            ) && matches!(
                event.request.and_then(|r| r.operation),
                Some("toggle_solo" | "toggle_mute")
            ) {
                send.try_send(event.details.clone()).unwrap();
            }
        })))
        .unwrap();
    let mut drag = paired.then(|| begin_eq_drag(harness, state));
    let mut solo_times = Vec::with_capacity(MEASURED / 2);
    let mut mute_times = Vec::with_capacity(MEASURED / 2);
    let mut next_callback = Instant::now();
    for i in 0..(MEASURED + WARMUP_SWITCHES) {
        assert!(
            harness
                .audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .playing
                .load(Acquire),
            "transport stopped during switch profile"
        );
        let solo = i % 4 < 2;
        let expected = i % 2 == 0;
        let command = if solo {
            Command::ToggleSolo {
                track,
                exclusive: true,
            }
        } else {
            Command::ToggleMute(track)
        };
        let old = harness
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .recording_source()
            .unwrap();
        if let Some(drag) = &mut drag {
            drag.change_pair(ParameterPairValue {
                first: ParameterValue::Float(if expected { 635.0 } else { 1400.0 }),
                second: ParameterValue::Float(if expected { -9.0 } else { 4.5 }),
            })
            .unwrap();
            let change = drag.next_request().unwrap();
            drag.mark_submitted();
            harness
                .service
                .submit(Request::device_gesture(change))
                .unwrap();
        }
        let mut preview_accepted = !paired;
        let now = Instant::now();
        harness.service.submit(Request::Edit(command)).unwrap();
        let mut terminal = None;
        let mut applied = None;
        let mut early_view = None;
        while terminal.is_none() || applied.is_none() || early_view.is_none() || !preview_accepted {
            harness.pump();
            if Instant::now() >= next_callback {
                let callback_entry = Instant::now();
                next_callback = callback_entry + budget;
                harness.render_plugin_block();
                let installed = harness
                    .audio
                    .as_ref()
                    .unwrap()
                    .status()
                    .unwrap()
                    .recording_source()
                    .unwrap();
                if installed.context.editor_revision == old.context.editor_revision + 1 {
                    assert_eq!(installed.output_identity, old.output_identity);
                    assert_eq!(
                        installed.context.project_generation,
                        old.context.project_generation
                    );
                    assert!(installed.audio_revision > old.audio_revision);
                    applied.get_or_insert(callback_entry.duration_since(now));
                }
            }
            if let Ok(view) = harness.service.try_track_switch_view() {
                assert_eq!(view.revision, old.context.editor_revision + 1);
                let target = view.tracks.iter().find(|row| row.track == track).unwrap();
                assert_eq!(if solo { target.soloed } else { target.muted }, expected);
                early_view.get_or_insert(now.elapsed());
            }
            if let Ok(event) = harness.service.try_event() {
                assert!(
                    event.error.is_none(),
                    "switch {i} failed: {:?}",
                    event.error
                );
                if let Some(ack) = event.gesture_ack.as_ref() {
                    assert!(ack.accepted, "{ack:?}");
                    preview_accepted |= drag.as_mut().expect("EQ drag").acknowledge(ack);
                }
                if event.request_complete && event.state.revision == old.context.editor_revision + 1
                {
                    assert_eq!(event.gesture_ack.is_some(), paired, "switch rebase receipt");
                    let target = event.state.project.track(track).unwrap();
                    assert_eq!(
                        if solo {
                            target.soloed()
                        } else {
                            target.muted()
                        },
                        expected
                    );
                    terminal.get_or_insert(now.elapsed());
                }
            }
            assert!(
                now.elapsed() < Duration::from_secs(3),
                "missing switch terminal: terminal={terminal:?} installed={applied:?}"
            );
            std::thread::sleep(Duration::from_micros(100));
        }
        let phases = phases.try_recv().expect("switch outcome diagnostics");
        if i >= WARMUP_SWITCHES {
            let times = [applied.unwrap(), early_view.unwrap(), terminal.unwrap()];
            if solo {
                solo_times.push(times);
            } else {
                mute_times.push(times);
            }
            eprintln!(
                "switch_sample paired={paired} kind={} expected={expected} terminal_us={} installed_callback_entry_us={} observed_view_us={} {phases}",
                if solo { "solo" } else { "mute" },
                times[2].as_micros(),
                times[0].as_micros(),
                times[1].as_micros()
            );
        }
    }

    report("solo", paired, &solo_times);
    report("mute", paired, &mute_times);
    if !paired {
        verify_solo_history(harness, state, budget);
    }
}

fn report(kind: &str, paired: bool, samples: &[[Duration; 3]]) {
    assert_eq!(samples.len(), MEASURED / 2);
    for (index, metric) in ["callback_entry", "early_view", "terminal"]
        .into_iter()
        .enumerate()
    {
        let mut values: Vec<_> = samples.iter().map(|times| times[index]).collect();
        values.sort_unstable();
        let middle = values.len() / 2;
        let median = (values[middle - 1] + values[middle]) / 2;
        eprintln!(
            "switch_summary paired={paired} kind={kind} metric={metric} n={} median_us={} worst_us={}",
            values.len(),
            median.as_micros(),
            values.last().unwrap().as_micros()
        );
    }
}

fn verify_solo_history(harness: &mut ResidentHarness, state: &ViewState, budget: Duration) {
    let first = state.project.tracks()[0].id();
    let second = state.project.tracks()[3].id();
    let device = state.project.tracks()[0].devices().last().unwrap().id();
    let live_worker = |harness: &ResidentHarness| {
        let bindings = harness.plugin_progress.lock().unwrap().source_bindings;
        let (_, worker, completed) = bindings
            .into_iter()
            .flatten()
            .find(|(id, _, _)| *id == device.value())
            .expect("first track's accepted VST3 worker");
        (worker, completed.expect("observed worker progress"))
    };
    let (original, mut completed) = live_worker(harness);
    for (request, second_soloed) in [
        (
            Request::Edit(Command::ToggleSolo {
                track: first,
                exclusive: true,
            }),
            false,
        ),
        (
            Request::Edit(Command::ToggleSolo {
                track: second,
                exclusive: false,
            }),
            true,
        ),
        (Request::Undo, false),
        (Request::Redo, true),
    ] {
        let old = harness
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .recording_source()
            .unwrap();
        let event = harness.run(request);
        assert!(event.request_complete);
        assert!(event.state.project.track(first).unwrap().soloed());
        assert_eq!(
            event.state.project.track(second).unwrap().soloed(),
            second_soloed
        );
        let (_, applied) = harness.applied(old.audio_revision);
        assert_eq!(applied.context.editor_revision, event.state.revision);
        render_paced_fixture_blocks(harness, budget, 8);
        let (worker, next) = live_worker(harness);
        assert_eq!(worker, original);
        assert!(next > completed, "the unchanged track must keep processing");
        completed = next;
    }
}

fn begin_eq_drag(harness: &mut ResidentHarness, state: &ViewState) -> GestureClient {
    let mut drag = GestureClient::default();
    let eq = &state.project.tracks()[1].devices()[1];
    let first = ParameterId::new(18).unwrap();
    let second = ParameterId::new(19).unwrap();
    drag.begin_pair(
        state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .context()
            .project_generation,
        eq.id(),
        first,
        second,
        ParameterPairValue {
            first: hex_engine::native_device_parameter(eq.state(), first).unwrap(),
            second: hex_engine::native_device_parameter(eq.state(), second).unwrap(),
        },
    )
    .unwrap();
    let begin = drag.next_request().unwrap();
    drag.mark_submitted();
    let event = harness.run(Request::device_gesture(begin));
    let ack = event.gesture_ack.as_ref().unwrap();
    assert!(ack.accepted && drag.acknowledge(ack));
    drag
}
