//! Disposable native profiling root. No user settings, recovery journal or audio stream.
use super::{
    CopyTimeSelection, CutTimeSelection, EditorView, InsertTimeSelection, PasteTimeSelection,
};
use gpui::{
    App, Context, Entity, IntoElement, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PlatformInput, Render, Subscription, Window, canvas, div, point, prelude::*, px,
};
use hex_session::service::{Request, Service};
use std::{
    path::PathBuf,
    sync::mpsc::SyncSender,
    time::{Duration, Instant},
};

const WARMUPS: usize = 5;
const MEASURED: usize = 40;
const CASES: usize = 8;
const TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy)]
enum Operation {
    Copy,
    Cut,
    Replace,
    Ripple,
}
impl Operation {
    fn name(self) -> &'static str {
        match self {
            Self::Copy => "copy",
            Self::Cut => "cut",
            Self::Replace => "replace-paste",
            Self::Ripple => "ripple-insert",
        }
    }
    fn status(self) -> &'static str {
        match self {
            Self::Copy => "time selection copied",
            Self::Cut => "time selection cut",
            Self::Replace => "time selection pasted",
            Self::Ripple => "time selection inserted",
        }
    }
    fn revision_delta(self) -> u64 {
        if matches!(self, Self::Copy) { 0 } else { 1 }
    }
    fn needs_seed(self) -> bool {
        matches!(self, Self::Replace | Self::Ripple)
    }
}
struct Probe {
    started: Instant,
    completion: bool,
    revision: u64,
    queued: bool,
    paint: Option<Duration>,
    following: Option<Duration>,
    feedback: Option<Duration>,
    feedback_following: Option<Duration>,
}
#[derive(Default)]
struct Sample {
    selection: Option<(Duration, Duration)>,
    operation: Option<(Duration, Duration, Duration, Duration)>,
    project_revision: u64,
    selection_start: u64,
    selection_end: u64,
}
struct Profile {
    editor: Entity<EditorView>,
    _observation: Subscription,
    _activation: Subscription,
    directory: PathBuf,
    result: Option<SyncSender<Result<String, String>>>,
    index: usize,
    stage: u8,
    stage_started: Instant,
    delay_until: Instant,
    paint_sequence: u64,
    setup_paint: u64,
    generation: u64,
    probe: Option<Probe>,
    sample: Sample,
    rows: Vec<String>,
    failure: Option<String>,
}

pub(crate) fn open(
    cx: &mut App,
    directory: PathBuf,
    settings: PathBuf,
    result: SyncSender<Result<String, String>>,
) -> Result<(), String> {
    super::configure_editor_app(cx)?;
    let service = Service::new(48_000, 8, 8_000_000).map_err(|error| error.to_string())?;
    cx.open_window(super::editor_window_options(cx), move |window, cx| {
        let editor = cx.new(|cx| {
            EditorView::new_with_browser_preferences_path(
                window,
                cx,
                service,
                Err("offline profiling".into()),
                settings.join("browser.json"),
                settings.join("recent.json"),
                settings.join("theme.json"),
            )
        });
        cx.new(|cx: &mut Context<Profile>| {
            let observation = cx.observe(&editor, |_, _, cx| cx.notify());
            let activation = cx.observe_window_activation(window, |profile, window, _| {
                if !window.is_window_active() && profile.probe.is_some() {
                    profile.failure =
                        Some("profiling window lost activation during a measured probe".into());
                }
            });
            let profile = Profile {
                editor,
                _observation: observation,
                _activation: activation,
                directory,
                result: Some(result),
                index: 0,
                stage: 0,
                stage_started: Instant::now(),
                delay_until: Instant::now(),
                paint_sequence: 0,
                setup_paint: 0,
                generation: 0,
                probe: None,
                sample: Sample::default(),
                rows: Vec::with_capacity((WARMUPS + MEASURED) * CASES),
                failure: None,
            };
            cx.spawn_in(window, async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(3))
                        .await;
                    match this.update_in(cx, |view, window, cx| view.step(window, cx)) {
                        Ok(true) => {}
                        Ok(false) => break,
                        Err(_) => {
                            // A closed/lost root drops its result sender; desktop
                            // publishes a failed report before terminating.
                            break;
                        }
                    }
                }
            })
            .detach();
            let close_profile = cx.entity().downgrade();
            window.on_window_should_close(cx, move |_, cx| {
                let _ = close_profile.update(cx, |profile, cx| {
                    profile.finish(Some("profiling window closed before completion".into()), cx);
                });
                false
            });
            profile
        })
    })
    .map_err(|error| format!("could not open profiling window: {error}"))?;
    cx.activate(true);
    Ok(())
}
impl Profile {
    fn tracks(&self) -> usize {
        if self.index.is_multiple_of(2) { 1 } else { 100 }
    }
    fn operation(&self) -> Operation {
        match (self.index / 2) % 4 {
            0 => Operation::Copy,
            1 => Operation::Cut,
            2 => Operation::Replace,
            _ => Operation::Ripple,
        }
    }
    fn phase(&mut self, stage: u8) {
        self.stage = stage;
        self.stage_started = Instant::now();
    }
    fn pending(view: &EditorView) -> usize {
        view.worker_requests_pending + view.pending_requests.len()
    }
    fn ready(&self, cx: &App) -> bool {
        let view = self.editor.read(cx);
        !view.opening_project && Self::pending(view) == 0 && view.state.is_some()
    }
    fn finish(&mut self, error: Option<String>, _cx: &mut Context<Self>) -> bool {
        let header = "tracks,operation,iteration,warmup,selection_start_frame,selection_end_frame,selection_first_paint_us,selection_following_frame_us,operation_first_paint_us,operation_first_following_frame_us,operation_ui_completion_paint_us,operation_completion_following_frame_us,project_revision,failures\n";
        let csv = format!("{header}{}\n", self.rows.join("\n"));
        if let Some(sender) = self.result.take() {
            let outcome = match error {
                Some(error) => Err(format!(
                    "failures=1; completed_samples={}; {error}\n{csv}",
                    self.rows.len()
                )),
                None => Ok(csv),
            };
            let _ = sender.try_send(outcome);
        }
        false
    }
    fn step(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.probe.is_some() && !window.is_window_active() {
            return self.finish(
                Some("profiling window is inactive during a measured probe".into()),
                cx,
            );
        }
        if let Some(error) = self.failure.take() {
            return self.finish(Some(error), cx);
        }
        if self.index == (WARMUPS + MEASURED) * CASES {
            return self.finish(None, cx);
        }
        if self.stage_started.elapsed() > TIMEOUT {
            return self.finish(
                Some(format!(
                    "timeout at case {} stage {}; active={}; pending={}; opening={}; state={}; lane={:?}; lane_epoch={}; layout_generation={}; paints={}/{}; probe={:?}",
                    self.index,
                    self.stage,
                    window.is_window_active(),
                    Self::pending(self.editor.read(cx)),
                    self.editor.read(cx).opening_project,
                    self.editor.read(cx).state.is_some(),
                    self.editor.read(cx).main_track_lane_bounds,
                    self.editor.read(cx).main_track_lane_epoch,
                    self.editor.read(cx).drag_layout_generation,
                    self.paint_sequence,
                    self.setup_paint,
                    self.probe.as_ref().map(|probe| (
                        probe.queued,
                        probe.paint.is_some(),
                        probe.following.is_some(),
                        probe.feedback.is_some(),
                        probe.feedback_following.is_some()
                    ))
                )),
                cx,
            );
        }
        if let Some(error) = self.editor.read(cx).error.clone() {
            return self.finish(Some(format!("editor error: {error}")), cx);
        }
        match self.stage {
            0 if window.is_window_active() => {
                self.sample = Sample::default();
                self.probe = None;
                let path = self.directory.join(format!("{}.hex", self.tracks()));
                self.generation = self.editor.read(cx).project_generation;
                self.editor.update(cx, |view, cx| {
                    view.begin_open_request(Request::Open(path));
                    cx.notify();
                });
                self.phase(1);
            }
            1 if self.ready(cx) && self.editor.read(cx).project_generation != self.generation => {
                let tracks = self.tracks();
                let valid = self.editor.read(cx).state.as_ref().is_some_and(|state| {
                    state.project.sample_rate() == 48_000
                        && state.project.tracks().len() == tracks
                        && state.project.tracks().iter().all(|track| {
                            track.clips().len() == 50
                                && track.midi_clips().len() == 50
                                && track
                                    .midi_clips()
                                    .iter()
                                    .all(|clip| clip.notes().len() == 8)
                        })
                });
                if !valid {
                    return self.finish(Some("fixture density or sample rate mismatch".into()), cx);
                }
                self.editor.update(cx, |view, cx| {
                    if let Some(state) = &view.state {
                        view.viewport = Some(super::viewport::Viewport::fit(state.timeline_frames));
                    }
                    view.focus.focus(window);
                    cx.notify();
                });
                self.setup_paint = self.paint_sequence;
                cx.notify();
                self.phase(2);
            }
            2 if self.paint_sequence > self.setup_paint => {
                let view = self.editor.read(cx);
                if view.main_track_lane_bounds.is_some()
                    && view.main_track_lane_epoch == view.drag_layout_generation
                {
                    self.delay_until =
                        Instant::now() + Duration::from_millis(7 + (self.index as u64 * 7) % 11);
                    self.phase(3);
                }
            }
            3 if window.is_window_active() && Instant::now() >= self.delay_until => {
                self.probe = Some(Probe {
                    started: Instant::now(),
                    completion: false,
                    revision: 0,
                    queued: true,
                    paint: None,
                    following: None,
                    feedback: None,
                    feedback_following: None,
                });
                if let Err(error) = self.pointer_range(window, cx, false) {
                    return self.finish(Some(error), cx);
                }
                cx.notify();
                self.phase(4);
            }
            4 if self
                .probe
                .as_ref()
                .is_some_and(|probe| probe.following.is_some()) =>
            {
                let view = self.editor.read(cx);
                let Some(range) = view.normalized_time_selection() else {
                    return self
                        .finish(Some("native pointer dispatch produced no range".into()), cx);
                };
                let Some(state) = &view.state else {
                    return self.finish(Some("project disappeared".into()), cx);
                };
                if !view.time_selection_project_scope
                    || range.tracks().iter().copied().ne(state
                        .project
                        .tracks()
                        .iter()
                        .map(|track| track.id()))
                {
                    return self.finish(
                        Some("selection did not cover every project track".into()),
                        cx,
                    );
                }
                self.sample.selection_start = range.start();
                self.sample.selection_end = range.end();
                self.sample.project_revision = state.revision;
                self.sample.selection = self
                    .probe
                    .take()
                    .and_then(|probe| Some((probe.paint?, probe.following?)));
                if self.operation().needs_seed() {
                    self.dispatch(Operation::Copy, false, window, cx);
                    self.phase(5);
                } else {
                    self.delay_until =
                        Instant::now() + Duration::from_millis(7 + (self.index as u64 * 3) % 11);
                    self.phase(7);
                }
            }
            5 if self.ready(cx) && self.editor.read(cx).status == Operation::Copy.status() => {
                if let Err(error) = self.pointer_range(window, cx, true) {
                    return self.finish(Some(error), cx);
                }
                self.setup_paint = self.paint_sequence;
                cx.notify();
                self.phase(6);
            }
            6 if self.paint_sequence > self.setup_paint => {
                if self.editor.read(cx).normalized_time_selection().is_some() {
                    return self.finish(
                        Some("paste cursor retained a nonempty selection".into()),
                        cx,
                    );
                }
                let view = self.editor.read(cx);
                let valid_cursor = view.edit_cursor.is_some_and(|cursor| {
                    cursor.generation == view.project_generation
                        && cursor.frame > self.sample.selection_end
                        && view.time_selection_project_scope
                        && view.time_selection.is_some_and(|range| {
                            range.anchor_frame == cursor.frame && range.end_frame == cursor.frame
                        })
                });
                if !valid_cursor {
                    return self.finish(
                        Some("paste cursor did not settle beyond the copied range".into()),
                        cx,
                    );
                }
                self.delay_until =
                    Instant::now() + Duration::from_millis(7 + (self.index as u64 * 3) % 11);
                self.phase(7);
            }
            7 if window.is_window_active() && Instant::now() >= self.delay_until => {
                self.dispatch(self.operation(), true, window, cx);
                cx.notify();
                self.phase(8);
            }
            8 if self
                .probe
                .as_ref()
                .is_some_and(|probe| probe.following.is_some()) =>
            {
                let Some(probe) = self.probe.take() else {
                    return self.finish(Some("completion probe disappeared".into()), cx);
                };
                if !probe.queued {
                    return self.finish(Some("operation was not queued".into()), cx);
                }
                self.sample.operation = probe
                    .feedback
                    .zip(probe.feedback_following)
                    .zip(probe.paint.zip(probe.following))
                    .map(
                        |((first, first_following), (complete, complete_following))| {
                            (first, first_following, complete, complete_following)
                        },
                    );
                let Some((selection, selection_following)) = self.sample.selection else {
                    return self.finish(Some("selection paint was not observed".into()), cx);
                };
                let Some((first, first_following, operation, operation_following)) =
                    self.sample.operation
                else {
                    return self.finish(Some("operation paint was not observed".into()), cx);
                };
                self.rows.push(format!(
                    "{},{},{},{},{},{},{},{},{},{},{},{},{},0",
                    self.tracks(),
                    self.operation().name(),
                    self.index / CASES,
                    self.index / CASES < WARMUPS,
                    self.sample.selection_start,
                    self.sample.selection_end,
                    selection.as_micros(),
                    selection_following.as_micros(),
                    first.as_micros(),
                    first_following.as_micros(),
                    operation.as_micros(),
                    operation_following.as_micros(),
                    self.sample.project_revision
                ));
                self.index += 1;
                self.phase(0);
            }
            _ => {}
        }
        true
    }
    fn pointer_range(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        cursor: bool,
    ) -> Result<(), String> {
        let lane = self
            .editor
            .read(cx)
            .main_track_lane_bounds
            .ok_or("main lane has not been painted")?;
        let y = lane.top + lane.height * 0.8;
        let from = point(
            px(lane.left + lane.width * if cursor { 0.71 } else { 0.213 }),
            px(y),
        );
        let to = point(
            px(lane.left + lane.width * if cursor { 0.71 } else { 0.619 }),
            px(y),
        );
        let modifiers = Modifiers {
            // Main-lane and regular-lane free projection currently differ by
            // one pixel. Snap only this untimed cursor setup; measured selection
            // drags remain free to exercise partial clip boundaries.
            platform: !cursor,
            ..Default::default()
        };
        if let Some(probe) = &mut self.probe
            && !probe.completion
        {
            probe.started = Instant::now();
        }
        window.dispatch_input(
            PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position: from,
                modifiers,
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );
        if !cursor {
            window.dispatch_input(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position: to,
                    pressed_button: Some(MouseButton::Left),
                    modifiers,
                }),
                cx,
            );
        }
        window.dispatch_input(
            PlatformInput::MouseUp(MouseUpEvent {
                button: MouseButton::Left,
                position: to,
                modifiers,
                click_count: 1,
            }),
            cx,
        );
        Ok(())
    }
    fn dispatch(
        &mut self,
        operation: Operation,
        timed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let action: Box<dyn gpui::Action> = match operation {
            Operation::Copy => Box::new(CopyTimeSelection),
            Operation::Cut => Box::new(CutTimeSelection),
            Operation::Replace => Box::new(PasteTimeSelection),
            Operation::Ripple => Box::new(InsertTimeSelection),
        };
        if timed {
            self.probe = Some(Probe {
                started: Instant::now(),
                completion: true,
                revision: self.sample.project_revision + operation.revision_delta(),
                queued: false,
                paint: None,
                following: None,
                feedback: None,
                feedback_following: None,
            });
        }
        window.dispatch_action(action, cx);
        let entity = cx.entity();
        // dispatch_action defers to the rendered focus path. Inspect admission
        // in the following deferred callback, before the next worker polling tick.
        cx.defer(move |cx| {
            entity.update(cx, |profile, cx| {
                if Self::pending(profile.editor.read(cx)) == 0 {
                    profile.failure = Some("bound action did not enqueue a worker request".into());
                } else if timed && let Some(probe) = &mut profile.probe {
                    probe.queued = true;
                }
            })
        });
    }
    fn paint(&mut self, at: Instant, window: &mut Window, cx: &mut Context<Self>) {
        self.paint_sequence += 1;
        let operation = self.operation();
        let view = self.editor.read(cx);
        let Some(probe) = &mut self.probe else {
            return;
        };
        let complete = if probe.completion {
            probe.queued
                && Self::pending(view) == 0
                && view.status == operation.status()
                && view
                    .state
                    .as_ref()
                    .is_some_and(|state| state.revision == probe.revision)
        } else {
            view.time_selection_project_scope
                && view
                    .time_selection
                    .is_some_and(|range| range.anchor_frame != range.end_frame)
        };
        let feedback = probe.completion && probe.queued && (Self::pending(view) > 0 || complete);
        let first_feedback = feedback && probe.feedback.is_none();
        let first_complete = complete && probe.paint.is_none();
        if first_feedback {
            probe.feedback = Some(at.duration_since(probe.started));
        }
        if first_complete {
            probe.paint = Some(at.duration_since(probe.started));
        }
        let index = self.index;
        let completion = probe.completion;
        for (feedback, observed) in [(true, first_feedback), (false, first_complete)] {
            if !observed {
                continue;
            }
            let entity = cx.entity();
            // Scheduled from paint: the next frame boundary follows submission
            // of this scene. Neither timestamp measures compositor/physical pixels.
            window.on_next_frame(move |_, cx| {
                entity.update(cx, |profile, _| {
                    if profile.index == index
                        && let Some(probe) = &mut profile.probe
                        && probe.completion == completion
                    {
                        let elapsed = probe.started.elapsed();
                        if feedback {
                            probe.feedback_following = Some(elapsed);
                        } else {
                            probe.following = Some(elapsed);
                        }
                    }
                })
            });
            window.request_animation_frame();
        }
    }
}
impl Render for Profile {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let content = self.editor.clone();
        div().size_full().relative().child(content).child(
            canvas(
                |_, _, _| (),
                move |_, _, window, cx| {
                    let at = Instant::now();
                    entity.update(cx, |profile, cx| profile.paint(at, window, cx));
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .w(px(1.0))
            .h(px(1.0)),
        )
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::super::clip_drag_ui_test::{audio_project, editor_window};
    use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point};
    use std::sync::Arc;

    #[gpui::test]
    fn snapped_paste_setup_clears_free_main_range(cx: &mut TestAppContext) {
        cx.update(super::super::bind_editor_keys);
        let (project, _, _, _) = audio_project();
        let (handle, editor) = editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let lane = visual.debug_bounds("main-track-lane").expect("main lane");
        let position = |fraction| {
            point(
                lane.origin.x + lane.size.width * fraction,
                lane.origin.y + lane.size.height * 0.8,
            )
        };
        let free = Modifiers {
            platform: true,
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(position(0.213), MouseButton::Left, free);
        visual.simulate_mouse_move(position(0.619), MouseButton::Left, free);
        visual.simulate_mouse_up(position(0.619), MouseButton::Left, free);
        visual.run_until_parked();
        let copied_end = editor.read_with(cx, |view, _| {
            assert!(view.time_selection_project_scope);
            view.normalized_time_selection()
                .expect("source range")
                .end()
        });
        visual.simulate_mouse_down(position(0.71), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(position(0.71), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert!(view.normalized_time_selection().is_none());
            assert!(view.time_selection_project_scope);
            let cursor = view.edit_cursor.expect("new cursor");
            assert!(cursor.frame > copied_end);
            assert_eq!(cursor.generation, view.project_generation);
            let range = view.time_selection.expect("zero-width cursor range");
            assert_eq!(range.anchor_frame, cursor.frame);
            assert_eq!(range.end_frame, cursor.frame);
        });
    }
}
