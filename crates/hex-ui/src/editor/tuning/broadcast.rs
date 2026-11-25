//! Project-scoped MTS-ESP broadcast lifecycle. Enabling project tuning—or
//! loading a project with tuning enabled—automatically claims MTS broadcast;
//! disabling tuning or replacing the project releases it. Manual toggling and
//! explicit blocked-master recovery remain available through the same control.
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
    scale_name: Option<String>,
    sent_name: Option<String>,
    snapshot: Option<Snapshot>,
    error: Option<String>,
    auto_enable_pending: bool,
}
impl Broadcast {
    pub(in crate::editor) fn set_scale_name(&mut self, name: Option<String>) {
        let normalized = name.map(|raw| hex_mts::sanitize_scale_name(&raw));
        if self.scale_name == normalized {
            return;
        }
        self.scale_name = normalized;
        if self.owner.is_some() && self.sent_name != self.scale_name {
            self.send(false);
        }
    }

    pub(in crate::editor) fn observe(
        &mut self,
        generation: u64,
        revision: u64,
        tuning: ProjectTuning,
    ) {
        let project_changed = self.accepted.is_none() || generation != self.generation;
        if generation != self.generation {
            self.error = None;
            self.disable();
            self.generation = generation;
            self.serial = 0;
            self.sent = None;
            self.scale_name = None;
            self.sent_name = None;
            self.auto_enable_pending = false;
        }
        let was_enabled = !project_changed && self.accepted.is_some_and(ProjectTuning::enabled);
        self.accepted = Some(tuning);
        self.revision = revision;
        if tuning.enabled() {
            if !was_enabled && self.owner.is_none() {
                self.auto_enable_pending = true;
            }
            self.try_auto_enable();
        } else {
            self.auto_enable_pending = false;
            if was_enabled && self.owner.is_some() {
                self.disable();
            }
        }
        if self.owner.is_some() && (self.sent != Some(tuning) || self.sent_name != self.scale_name)
        {
            self.send(false);
        }
    }
    fn try_auto_enable(&mut self) {
        if !self.auto_enable_pending || self.owner.is_some() {
            return;
        }
        let Some(client) = self.client else {
            return;
        };
        self.auto_enable_pending = false;
        self.owner = Some(Owner {
            client,
            generation: self.generation,
        });
        self.send(true);
    }
    fn disable(&mut self) {
        if let (Some(handle), Some(owner)) = (&self.handle, self.owner.take()) {
            if let Err(error) = handle.disable(owner) {
                self.error = Some(error.to_string());
            }
            self.snapshot = handle.snapshot().ok();
        }
        self.sent = None;
        self.sent_name = None;
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
        let result = Publication::new_named(
            Revision {
                owner,
                serial,
                project_revision: self.revision,
            },
            tuning,
            self.scale_name.as_deref(),
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
                self.sent_name = self.scale_name.clone();
                self.error = None;
            }
            Err(error) => {
                self.error = Some(error);
                if enable {
                    self.owner = None;
                }
            }
        }
        self.snapshot = self
            .handle
            .as_ref()
            .and_then(|handle| handle.snapshot().ok());
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
        let had_owner = self.owner.is_some();
        let had_error = self.error.is_some();
        self.try_auto_enable();
        let snapshot = self
            .handle
            .as_ref()
            .and_then(|handle| handle.snapshot().ok());
        let changed = self.snapshot != snapshot
            || self.owner.is_some() != had_owner
            || self.error.is_some() != had_error;
        self.snapshot = snapshot;
        changed
    }
    fn toggle(&mut self, generation: u64, revision: u64, tuning: ProjectTuning) {
        self.auto_enable_pending = false;
        if self.owner.is_some() {
            self.disable();
            return;
        }
        let tuning = if generation != self.generation {
            self.error = None;
            self.disable();
            self.generation = generation;
            self.serial = 0;
            self.sent = None;
            tuning
        } else {
            self.accepted.unwrap_or(tuning)
        };
        self.accepted = Some(tuning);
        self.revision = revision;
        if let Some(client) = self.client {
            self.owner = Some(Owner { client, generation });
            self.send(true);
        } else {
            self.error = Some("mts publisher unavailable".into());
        }
    }
    fn force_claim_owner(&self) -> Option<Owner> {
        match self.snapshot {
            Some(Snapshot {
                owner,
                state:
                    State::Blocked {
                        recovery_available: true,
                    },
                ..
            }) if owner == self.owner => owner,
            _ => None,
        }
    }
    fn force_claim(&mut self, owner: Owner) {
        if self.force_claim_owner() == Some(owner)
            && let Some(handle) = &self.handle
        {
            self.error = handle.recover(owner).err().map(|error| error.to_string());
            self.snapshot = handle.snapshot().ok();
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
            State::Blocked {
                recovery_available: true,
            } => "mts blocked · active or stale master registration".into(),
            State::Blocked {
                recovery_available: false,
            } => "mts blocked · a master is already registered".into(),
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
        let broadcast = &self.settings.tuning.broadcast;
        let force_owner = broadcast.force_claim_owner();
        let (label, selector, active) = if force_owner.is_some() {
            ("force claim mts", "tuning-mts-force-claim", false)
        } else {
            (
                "broadcast mts",
                "tuning-mts-broadcast",
                broadcast.owner.is_some(),
            )
        };
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                button(label, active)
                    .debug_selector(move || selector.into())
                    .on_click(cx.listener(move |view, _, _, cx| {
                        if let Some(owner) = force_owner {
                            view.settings.tuning.broadcast.force_claim(owner);
                        } else if let Some(state) = view.state.as_ref() {
                            view.settings.tuning.broadcast.toggle(
                                view.project_generation,
                                state.revision,
                                state.project.tuning(),
                            );
                        }
                        cx.notify();
                    })),
            )
            .child(theme::text(broadcast.status()))
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[gpui::test]
fn blocked_mts_replaces_broadcast_with_one_direct_force_claim(cx: &mut gpui::TestAppContext) {
    use gpui::{Modifiers, VisualTestContext};

    let worker = hex_mts::Worker::start().unwrap();
    let publisher = worker.handle();
    let owner = Owner {
        client: publisher.new_client().unwrap(),
        generation: 0,
    };
    let (window, editor) = crate::editor::clip_drag_ui_test::editor_window(
        cx,
        hex_project::Project::new(48_000).unwrap(),
        Default::default(),
    );
    editor.update(cx, |view, cx| {
        view.settings.tuning.open = true;
        let broadcast = &mut view.settings.tuning.broadcast;
        broadcast.handle = Some(publisher.clone());
        broadcast.owner = Some(owner);
        // Inject the blocked receipt without touching the installed MTS library.
        broadcast.snapshot = Some(Snapshot {
            owner: Some(owner),
            state: State::Blocked {
                recovery_available: true,
            },
            ..Snapshot::default()
        });
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(window, cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("tuning-mts-broadcast").is_none());
    let button = visual
        .debug_bounds("tuning-mts-force-claim")
        .unwrap()
        .center();
    visual.simulate_mouse_down(button, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(button, MouseButton::Left, Modifiers::default());
    assert!(!visual.has_pending_prompt());
    assert_eq!(publisher.snapshot().unwrap(), Snapshot::default());
    editor.read_with(&visual, |view, _| {
        // The idle publisher rejects the request from the injected UI receipt.
        // This observes dispatch without resetting the machine's shared MTS state.
        assert_eq!(
            view.settings.tuning.broadcast.error,
            Some(hex_mts::ControlError::Stale.to_string())
        );
        assert_eq!(view.settings.tuning.broadcast.force_claim_owner(), None);
    });
}

#[cfg(all(test, feature = "ui-test-support"))]
mod auto_broadcast_tests {
    use super::*;
    use gpui::{Modifiers, TestAppContext, VisualTestContext};
    use std::path::PathBuf;
    use std::sync::{Arc, atomic::Ordering::Relaxed};

    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn scratch() -> Scratch {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "hex-tuning-mts-auto-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Scratch(path)
    }

    fn settle(editor: &gpui::Entity<EditorView>, cx: &mut VisualTestContext) {
        let deadline = crate::test_wait::hang_deadline();
        loop {
            let done = editor.update(cx, |view, cx| {
                view.tick(cx);
                view.state.is_some()
                    && view.settings.tuning.client.snapshot().is_none()
                    && view.worker_requests_pending == 0
                    && view.pending_requests.is_empty()
                    && !view.persistence.saving_project
                    && !view.persistence.opening_project
            });
            if done {
                cx.run_until_parked();
                return;
            }
            assert!(Instant::now() < deadline, "editor did not settle");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn wait_mts(handle: &Handle, predicate: impl Fn(Snapshot) -> bool) -> Snapshot {
        let deadline = crate::test_wait::hang_deadline();
        loop {
            let snapshot = handle.snapshot().unwrap();
            if predicate(snapshot) {
                return snapshot;
            }
            assert!(
                Instant::now() < deadline,
                "mts snapshot did not reach expected state: {snapshot:?}"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn custom_enabled_tuning(hz: f64) -> ProjectTuning {
        let base = ProjectTuning::default();
        let mut scale = base.scale();
        scale.set_reference(60, hz, 0).unwrap();
        ProjectTuning::new(true, scale, base.audition()).unwrap()
    }

    #[gpui::test]
    fn tuning_enabled_saves_per_project_and_auto_enables_mts_on_toggle_and_reload(
        cx: &mut TestAppContext,
    ) {
        let mut worker = hex_mts::Worker::start_test_mock().unwrap();
        let publisher = worker.handle();
        cx.update(|cx| crate::mts::install_mts_publisher(cx, publisher.clone()));
        let dir = scratch();
        let path = dir.0.join("tuned-project.sqlite");

        let (window, editor) = crate::editor::clip_drag_ui_test::editor_window(
            cx,
            hex_project::Project::new(48_000).unwrap(),
            Arc::default(),
        );
        let mut visual = VisualTestContext::from_window(window, cx);
        settle(&editor, &mut visual);
        assert_eq!(publisher.snapshot().unwrap().state, State::Disabled);
        assert!(publisher.snapshot().unwrap().owner.is_none());

        // Open tuning and click "enable tuning".
        editor.update(&mut visual, |view, cx| {
            view.settings.tuning.open = true;
            view.sync_tuning(cx);
            cx.notify();
        });
        visual.run_until_parked();
        let enable_button = visual.debug_bounds("tuning-enable").unwrap().center();
        visual.simulate_mouse_down(enable_button, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(enable_button, MouseButton::Left, Modifiers::default());
        settle(&editor, &mut visual);

        editor.read_with(&visual, |view, _| {
            assert!(view.state.as_ref().unwrap().project.tuning().enabled());
            assert!(view.settings.tuning.broadcast.owner.is_some());
        });
        let enabled_snapshot = wait_mts(&publisher, |s| s.state == State::Active);
        assert_eq!(enabled_snapshot.desired, enabled_snapshot.published);
        assert!(enabled_snapshot.owner.is_some());

        // Close tuning, save the project, then switch to a fresh project with tuning off.
        editor.update(&mut visual, |view, cx| {
            view.settings.tuning.open = false;
            view.persistence.saving_project = true;
            assert!(view.enqueue(Request::SaveNew(path.clone())));
            cx.notify();
        });
        settle(&editor, &mut visual);
        assert!(path.exists());

        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::NewWithAudioMode {
                mode: hex_session::DocumentAudioMode::Resident,
                reservation: None,
            }));
        });
        settle(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            assert!(!view.state.as_ref().unwrap().project.tuning().enabled());
            assert!(view.settings.tuning.broadcast.owner.is_none());
        });
        wait_mts(&publisher, |s| {
            s.state == State::Disabled && s.owner.is_none()
        });

        // Reload the saved project with the tuning panel closed: tuning stays on
        // and MTS broadcast automatically turns on for the new generation.
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::Open(path.clone())));
        });
        settle(&editor, &mut visual);
        let reloaded_generation = editor.read_with(&visual, |view, _| {
            assert!(!view.settings.tuning.open);
            assert!(view.state.as_ref().unwrap().project.tuning().enabled());
            assert!(view.settings.tuning.broadcast.owner.is_some());
            view.project_generation
        });
        let reloaded_snapshot = wait_mts(&publisher, |s| {
            s.state == State::Active
                && s.owner
                    .is_some_and(|owner| owner.generation == reloaded_generation)
        });
        assert_eq!(reloaded_snapshot.desired, reloaded_snapshot.published);

        // Shut down the worker cleanly after dropping the window's ownership.
        editor.update(&mut visual, |view, _| {
            view.settings.tuning.broadcast.disable();
        });
        wait_mts(&publisher, |s| s.state == State::Disabled);
        worker.shutdown();
    }

    #[gpui::test]
    fn manual_mts_overrides_and_disable_transitions_are_respected(cx: &mut TestAppContext) {
        let mut worker = hex_mts::Worker::start_test_mock().unwrap();
        let publisher = worker.handle();
        cx.update(|cx| crate::mts::install_mts_publisher(cx, publisher.clone()));

        let tuned_a = custom_enabled_tuning(432.0);
        let tuned_b = custom_enabled_tuning(444.0);
        let disabled = tuned_b.with_enabled(false);

        // 1. Observe before tick: auto-enables once tick attaches the publisher.
        let mut broadcast = Broadcast::default();
        broadcast.observe(0, 1, tuned_a);
        assert!(broadcast.owner.is_none());
        assert!(broadcast.error.is_none());
        let changed = cx.update(|cx| broadcast.tick(cx));
        assert!(changed);
        assert!(broadcast.owner.is_some());
        wait_mts(&publisher, |s| s.state == State::Active);

        // 2. Manual toggle OFF while tuning stays ON: further edits stay OFF.
        broadcast.toggle(0, 1, tuned_a);
        assert!(broadcast.owner.is_none());
        wait_mts(&publisher, |s| {
            s.state == State::Disabled && s.owner.is_none()
        });
        broadcast.observe(0, 2, tuned_b);
        assert!(
            broadcast.owner.is_none(),
            "subsequent edits must not override a manual MTS turn-off"
        );
        assert_eq!(publisher.snapshot().unwrap().owner, None);

        // 3. Reloading a project with tuning ON re-enables MTS automatically.
        broadcast.observe(1, 0, tuned_b);
        assert!(broadcast.owner.is_some());
        let gen1 = wait_mts(&publisher, |s| {
            s.state == State::Active && s.owner.is_some_and(|owner| owner.generation == 1)
        });
        assert_eq!(gen1.desired, gen1.published);

        // 4. Turning tuning OFF disables MTS automatically.
        broadcast.observe(1, 1, disabled);
        assert!(broadcast.owner.is_none());
        wait_mts(&publisher, |s| {
            s.state == State::Disabled && s.owner.is_none()
        });

        // 5. Explicitly turning MTS ON while tuning is OFF persists across edits
        //    and releases on switching to a project with tuning OFF.
        broadcast.toggle(1, 1, disabled);
        assert!(broadcast.owner.is_some());
        wait_mts(&publisher, |s| s.state == State::Active);
        broadcast.observe(1, 2, disabled);
        assert!(broadcast.owner.is_some());
        broadcast.observe(2, 0, ProjectTuning::default());
        assert!(broadcast.owner.is_none());
        wait_mts(&publisher, |s| {
            s.state == State::Disabled && s.owner.is_none()
        });

        // 6. Another Hex project holding MTS prevents stealing and does not spam retries.
        let foreign_owner = Owner {
            client: publisher.new_client().unwrap(),
            generation: 99,
        };
        publisher
            .enable(
                Publication::new(
                    Revision {
                        owner: foreign_owner,
                        serial: 1,
                        project_revision: 0,
                    },
                    tuned_a,
                )
                .unwrap(),
            )
            .unwrap();
        wait_mts(&publisher, |s| {
            s.state == State::Active && s.owner == Some(foreign_owner)
        });
        broadcast.observe(3, 0, tuned_a);
        assert!(broadcast.owner.is_none());
        assert_eq!(
            broadcast.error.as_deref(),
            Some("another hex project owns mts")
        );
        assert_eq!(publisher.snapshot().unwrap().owner, Some(foreign_owner));
        // Subsequent edits on generation 3 stay failed without retrying or stealing.
        broadcast.observe(3, 1, tuned_b);
        assert!(broadcast.owner.is_none());
        assert_eq!(publisher.snapshot().unwrap().owner, Some(foreign_owner));

        publisher.disable(foreign_owner).unwrap();
        drop(broadcast);
        worker.shutdown();
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
            view.settings.tuning.broadcast.observe(0, 0, original);
            let key = view.settings.tuning.client.begin(0, original).unwrap();
            let request = view.settings.tuning.client.next_request().unwrap();
            view.settings.tuning.client.mark_submitted();
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
            let sequence = view.settings.tuning.client.change(tuning).unwrap();
            let request = view.settings.tuning.client.next_request().unwrap();
            view.settings.tuning.client.mark_submitted();
            (sequence, request)
        });
        service.submit(Request::TuningGesture(change)).unwrap();
        let preview = next(&service);
        assert!(preview.tuning_preview.is_some());
        editor.update(cx, |view, _| {
            view.receive(preview);
            assert!(view.pending_playback.is_some());
            assert_eq!(
                view.settings.tuning.broadcast.accepted,
                Some(original),
                "worker acceptance is not the callback's applied receipt"
            );
        });
        // Seed a previously applied preview in the control observer. A rejected
        // nonterminal update must not restore the older canonical table.
        editor.update(cx, |view, _| {
            view.settings.tuning.broadcast.observe(0, 0, tuning)
        });
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
            assert_eq!(view.settings.tuning.broadcast.accepted, Some(tuning));
        });
        let commit = editor.update(cx, |view, _| {
            view.settings.tuning.client.finish(tuning).unwrap();
            let request = view.settings.tuning.client.next_request().unwrap();
            view.settings.tuning.client.mark_submitted();
            request
        });
        service.submit(Request::TuningGesture(commit)).unwrap();
        let committed = next(&service);
        assert!(committed.tuning_gesture_ack.as_ref().unwrap().accepted);
        let committed_revision = committed.state.revision;
        editor.update(cx, |view, _| {
            view.receive(committed);
            assert_eq!(
                view.settings.tuning.broadcast.accepted,
                Some(tuning),
                "canonical commit remains available without an audio output"
            );
            assert_eq!(view.settings.tuning.broadcast.revision, committed_revision);
            assert!(view.settings.tuning.client.snapshot().is_none());
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
                view.settings.tuning.broadcast.tick(cx);
                view.settings.tuning.open = true;
                view.settings.tuning.focus.focus(window);
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
            assert!(!view.settings.tuning.value.enabled());
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
            view.settings
                .tuning
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
