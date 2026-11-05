//! Explicit project ownership; focus and opening the tuning window never acquire it.
use super::*;
use hex_mts::{ClientId, Handle, Owner, Publication, Revision, Snapshot, State};

#[derive(Default)]
pub(in crate::editor) struct Broadcast {
    handle: Option<Handle>,
    client: Option<ClientId>,
    owner: Option<Owner>,
    generation: u64,
    revision: u64,
    serial: u64,
    accepted: Option<ProjectTuning>,
    sent: Option<ProjectTuning>,
    snapshot: Option<Snapshot>,
    error: Option<String>,
}
impl Broadcast {
    pub(in crate::editor) fn observe(
        &mut self,
        generation: u64,
        revision: u64,
        tuning: ProjectTuning,
    ) {
        if generation != self.generation {
            self.disable();
            self.generation = generation;
            self.serial = 0;
            self.sent = None;
        }
        self.accepted = Some(tuning);
        self.revision = revision;
        if self.owner.is_some() && self.sent != Some(tuning) {
            self.send(false);
        }
    }
    fn disable(&mut self) {
        if let (Some(handle), Some(owner)) = (&self.handle, self.owner.take())
            && let Err(error) = handle.disable(owner)
        {
            self.error = Some(error.to_string());
        }
        self.sent = None;
    }
    fn send(&mut self, enable: bool) {
        let Some(owner) = self.owner else {
            return;
        };
        let Some(tuning) = self.accepted else {
            return;
        };
        let Some(serial) = self.serial.checked_add(1) else {
            self.error = Some("mts publication identity exhausted".into());
            return;
        };
        let result = Publication::new(
            Revision {
                owner,
                serial,
                project_revision: self.revision,
            },
            tuning,
        )
        .map_err(|error| error.to_string())
        .and_then(|publication| {
            self.handle
                .as_ref()
                .ok_or_else(|| "mts publisher unavailable".to_owned())
                .and_then(|handle| {
                    if enable {
                        handle.enable(publication)
                    } else {
                        handle.publish(publication)
                    }
                    .map_err(|error| error.to_string())
                })
        });
        match result {
            Ok(()) => {
                self.serial = serial;
                self.sent = Some(tuning);
                self.error = None;
            }
            Err(error) => {
                self.error = Some(error);
                if enable {
                    self.owner = None;
                }
            }
        }
    }
    pub(in crate::editor) fn tick(&mut self, cx: &gpui::App) -> bool {
        if self.handle.is_none() {
            self.handle = cx
                .try_global::<crate::mts::Publisher>()
                .map(|publisher| publisher.0.clone());
            if let Some(handle) = &self.handle {
                match handle.new_client() {
                    Ok(client) => self.client = Some(client),
                    Err(error) => self.error = Some(error.to_string()),
                }
            }
        }
        let snapshot = self
            .handle
            .as_ref()
            .and_then(|handle| handle.snapshot().ok());
        let changed = self.snapshot != snapshot;
        self.snapshot = snapshot;
        changed
    }
    fn toggle(&mut self, generation: u64, revision: u64, tuning: ProjectTuning) {
        if self.owner.is_some() {
            self.disable();
            return;
        }
        self.observe(generation, revision, self.accepted.unwrap_or(tuning));
        if let Some(client) = self.client {
            self.owner = Some(Owner { client, generation });
            self.send(true);
        } else {
            self.error = Some("mts publisher unavailable".into());
        }
    }
    fn retry(&mut self) {
        if let (Some(handle), Some(owner)) = (&self.handle, self.owner)
            && let Err(error) = handle.retry(owner)
        {
            self.error = Some(error.to_string());
        }
    }
    fn status(&self) -> String {
        if let Some(error) = &self.error {
            return error.to_lowercase();
        }
        let Some(snapshot) = self.snapshot else {
            return "mts unavailable".into();
        };
        if snapshot.owner.is_some() && snapshot.owner != self.owner {
            return "mts owned by another project".into();
        }
        match snapshot.state {
            State::Disabled => "mts off".into(),
            State::Disabling => "mts stopping".into(),
            State::Stopped => "mts publisher stopped".into(),
            State::Blocked => "mts blocked · another master is active · retry explicitly".into(),
            State::Unavailable(error) => format!("mts unavailable · {error}").to_lowercase(),
            State::Pending | State::Active => format!(
                "mts {} · desired {} · published {} · clients {}",
                if snapshot.state == State::Active {
                    "active"
                } else {
                    "pending"
                },
                snapshot.desired.map_or(0, |revision| revision.serial),
                snapshot.published.map_or(0, |revision| revision.serial),
                snapshot.clients
            ),
        }
    }
}
impl Drop for Broadcast {
    fn drop(&mut self) {
        self.disable();
    }
}

impl EditorView {
    pub(super) fn tuning_mts_controls(&self, cx: &mut Context<Self>) -> Div {
        let retry = self.tuning.broadcast.owner.is_some()
            && self.tuning.broadcast.snapshot.is_some_and(|snapshot| {
                matches!(snapshot.state, State::Blocked | State::Unavailable(_))
            });
        let mut controls = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                button("broadcast mts", self.tuning.broadcast.owner.is_some())
                    .debug_selector(|| "tuning-mts-broadcast".into())
                    .on_click(cx.listener(|view, _, _, cx| {
                        if let Some(state) = view.state.as_ref() {
                            view.tuning.broadcast.toggle(
                                view.project_generation,
                                state.revision,
                                state.project.tuning(),
                            );
                        }
                        cx.notify();
                    })),
            )
            .child(theme::text(self.tuning.broadcast.status()));
        if retry {
            controls = controls.child(button("retry mts", false).on_click(cx.listener(
                |view, _, _, cx| {
                    view.tuning.broadcast.retry();
                    cx.notify();
                },
            )));
        }
        controls
    }
}

// A Linux UI fixture exercises the real unavailable backend without acquiring
// any installed system master on a developer's Mac.
#[cfg(all(test, feature = "ui-test-support", target_os = "linux"))]
mod tests {
    use super::*;
    use gpui::{AppContext, Modifiers, TestAppContext, VisualTestContext};
    use std::sync::Arc;

    #[test]
    fn review_final_native_mts_errors_are_lowercase_at_the_ui_boundary() {
        let mut worker = hex_mts::Worker::start().unwrap();
        let handle = worker.handle();
        let owner = Owner {
            client: handle.new_client().unwrap(),
            generation: 0,
        };
        handle
            .enable(
                Publication::new(
                    Revision {
                        owner,
                        serial: 1,
                        project_revision: 0,
                    },
                    ProjectTuning::default(),
                )
                .unwrap(),
            )
            .unwrap();
        let deadline = crate::test_wait::hang_deadline();
        let snapshot = loop {
            let snapshot = handle.snapshot().unwrap();
            if matches!(snapshot.state, State::Unavailable(_)) {
                break snapshot;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "review_final_native_mts_errors_are_lowercase_at_the_ui_boundary: hang guard expired"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        };
        let State::Unavailable(error) = snapshot.state else {
            panic!("unavailable backend");
        };
        let original_error = error.to_string();
        assert!(original_error.contains("MTS"));
        let mut broadcast = Broadcast::default();
        broadcast.owner = snapshot.owner;
        broadcast.snapshot = Some(snapshot);
        let status = broadcast.status();
        assert_eq!(status, status.to_lowercase());
        broadcast.error = Some(original_error.clone());
        assert_eq!(broadcast.status(), original_error.to_lowercase());
        assert_eq!(
            error.to_string(),
            original_error,
            "native/source metadata stays unchanged"
        );
        worker.shutdown();
    }

    #[gpui::test]
    fn worker_tuning_preview_does_not_publish_before_audio_acceptance(cx: &mut TestAppContext) {
        use hex_session::service::{Request, Service};
        use hex_session::tuning_gesture::TuningGestureRequest;
        fn next(service: &Service) -> hex_session::service::Event {
            let deadline = crate::test_wait::hang_deadline();
            loop {
                match service.try_event() {
                    Ok(event) => return event,
                    Err(std::sync::mpsc::TryRecvError::Empty)
                        if std::time::Instant::now() < deadline =>
                    {
                        std::thread::sleep(std::time::Duration::from_millis(2))
                    }
                    Err(error) => panic!("tuning worker did not reply: {error}"),
                }
            }
        }
        let service = Service::new(48_000, 8, 1_000_000).unwrap();
        let initial = next(&service);
        let project = initial.state.project.as_ref().clone();
        let original = project.tuning();
        let (_, editor) =
            crate::editor::clip_drag_ui_test::editor_window(cx, project, Arc::default());
        let (key, begin) = editor.update(cx, |view, _| {
            view.tuning.broadcast.observe(0, 0, original);
            let key = view.tuning.client.begin(0, original).unwrap();
            let request = view.tuning.client.next_request().unwrap();
            view.tuning.client.mark_submitted();
            (key, request)
        });
        service.submit(Request::TuningGesture(begin)).unwrap();
        let begun = next(&service);
        assert!(begun.tuning_gesture_ack.as_ref().unwrap().accepted);
        editor.update(cx, |view, _| {
            view.receive(begun);
        });
        let mut scale = original.scale();
        scale.set_reference(60, 333.0, 0).unwrap();
        let tuning = ProjectTuning::new(true, scale, original.audition()).unwrap();
        let (sequence, change) = editor.update(cx, |view, _| {
            let sequence = view.tuning.client.change(tuning).unwrap();
            let request = view.tuning.client.next_request().unwrap();
            view.tuning.client.mark_submitted();
            (sequence, request)
        });
        service.submit(Request::TuningGesture(change)).unwrap();
        let preview = next(&service);
        assert!(preview.tuning_preview.is_some());
        editor.update(cx, |view, _| {
            view.receive(preview);
            assert!(view.pending_playback.is_some());
            assert_eq!(
                view.tuning.broadcast.accepted,
                Some(original),
                "worker acceptance is not the callback's applied receipt"
            );
        });
        // Seed a previously applied preview in the control observer. A rejected
        // nonterminal update must not restore the older canonical table.
        editor.update(cx, |view, _| view.tuning.broadcast.observe(0, 0, tuning));
        service
            .submit(Request::TuningGesture(TuningGestureRequest::Change {
                key,
                sequence,
                tuning,
            }))
            .unwrap();
        let refused = next(&service);
        assert!(!refused.tuning_gesture_ack.as_ref().unwrap().accepted);
        editor.update(cx, |view, _| {
            view.receive(refused);
            assert_eq!(view.tuning.broadcast.accepted, Some(tuning));
        });
        let commit = editor.update(cx, |view, _| {
            view.tuning.client.finish(tuning).unwrap();
            let request = view.tuning.client.next_request().unwrap();
            view.tuning.client.mark_submitted();
            request
        });
        service.submit(Request::TuningGesture(commit)).unwrap();
        let committed = next(&service);
        assert!(committed.tuning_gesture_ack.as_ref().unwrap().accepted);
        let committed_revision = committed.state.revision;
        editor.update(cx, |view, _| {
            view.receive(committed);
            assert_eq!(
                view.tuning.broadcast.accepted,
                Some(tuning),
                "canonical commit remains available without an audio output"
            );
            assert_eq!(view.tuning.broadcast.revision, committed_revision);
            assert!(view.tuning.client.snapshot().is_none());
        });
    }

    #[gpui::test]
    fn explicit_broadcast_click_is_independent_of_tuning_focus_and_project_dirty_state(
        cx: &mut TestAppContext,
    ) {
        let mut worker = hex_mts::Worker::start().unwrap();
        let publisher = worker.handle();
        cx.update(|cx| crate::mts::install_mts_publisher(cx, publisher.clone()));
        let project = hex_project::Project::new(48000).unwrap();
        let original = project.clone();
        let (handle, editor) =
            crate::editor::clip_drag_ui_test::editor_window(cx, project, Arc::default());
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                view.tuning.broadcast.tick(cx);
                view.tuning.open = true;
                view.tuning.focus.focus(window);
                view.sync_tuning(cx);
                cx.notify();
            });
        })
        .unwrap();
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        assert!(
            publisher.snapshot().unwrap().owner.is_none(),
            "opening/focusing must not claim mts"
        );
        let bounds = visual.debug_bounds("tuning-mts-broadcast").unwrap();
        visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
        assert!(publisher.snapshot().unwrap().owner.is_some());
        editor.read_with(cx, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project.as_ref(), &original);
            assert!(!view.state.as_ref().unwrap().unsaved);
            assert!(!view.tuning.value.enabled());
        });
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| view.close_tuning(window, cx));
        })
        .unwrap();
        assert!(
            publisher.snapshot().unwrap().owner.is_some(),
            "closing tuning does not silently disable explicit broadcast"
        );
        editor.update(cx, |view, _| {
            view.tuning
                .broadcast
                .observe(1, 0, ProjectTuning::default())
        });
        assert!(
            publisher.snapshot().unwrap().owner.is_none(),
            "project replacement must release ownership"
        );
        worker.shutdown();
    }
}
