//! Centered playback-interruption confirmation modal tests (Pulse #8943).
use super::*;
use gpui::{
    Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, ScrollDelta, ScrollWheelEvent,
    TestAppContext, VisualTestContext, point,
};
use hex_engine::PreparedRevision;
use hex_engine::native_device_resources::{DeviceResourceAdmission, PreparedDeviceResources};
use hex_engine::playback::Playback;
use hex_project::{Command, Project};
use hex_session::service::{
    CanonicalAudioSourceSnapshot, DeviceInsertAudioReceipt, DeviceInsertAudioState,
    ResidentEffectPreparation, ResidentPublicationAcceptance, ResidentPublicationFailure,
    ResidentPublicationInputs, ResidentPublicationMetadata, ResidentPublicationPolicy,
    ResidentPublicationReadiness, ResidentPublicationRefusal, ResidentRuntimeLoop, Service,
    WorkerAudioSink,
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU8, Ordering},
};
use std::time::{Duration, Instant};

const PENDING: u8 = 0;
const CANCELLED: u8 = 2;
const REFUSED: u8 = 3;

#[derive(Debug)]
struct ConfirmationReceipt {
    submission: ResidentGraphSubmission,
    state: AtomicU8,
}

#[derive(Clone, Debug)]
struct ReceiptHandle(Arc<ConfirmationReceipt>);

impl DeviceInsertAudioReceipt for ReceiptHandle {
    fn submission(&self) -> ResidentGraphSubmission {
        self.0.submission
    }

    fn state(&self) -> DeviceInsertAudioState {
        match self.0.state.load(Ordering::Acquire) {
            PENDING => DeviceInsertAudioState::Pending,
            CANCELLED => DeviceInsertAudioState::Cancelled,
            REFUSED => DeviceInsertAudioState::Refused(
                hex_engine::PlaybackHandoffRejection::DeviceInsertNeedsConfirmation,
            ),
            _ => DeviceInsertAudioState::Expired,
        }
    }

    fn cancel(&self) -> bool {
        self.0
            .state
            .compare_exchange(PENDING, CANCELLED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
}

#[derive(Default)]
struct ConfirmationAudioFixture {
    active: Option<ResidentRuntimeLoop>,
    candidate: Option<Arc<ConfirmationReceipt>>,
    offer_deadline: Option<Instant>,
    revision: u64,
}

#[derive(Clone)]
struct SinkHandle(Arc<Mutex<ConfirmationAudioFixture>>);

impl WorkerAudioSink for SinkHandle {
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

    fn resident_publication_readiness(&mut self) -> ResidentPublicationReadiness {
        let fixture = self.0.lock().unwrap();
        fixture
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
        let fixture = self.0.lock().unwrap();
        let receipt = fixture.candidate.as_ref()?;
        (receipt.submission == submission)
            .then(|| Box::new(ReceiptHandle(Arc::clone(receipt))) as Box<_>)
    }

    fn offer_device_insert_consent(
        &mut self,
        submission: ResidentGraphSubmission,
    ) -> Option<Instant> {
        let fixture = self.0.lock().unwrap();
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
        Ok(self.0.lock().unwrap().active)
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
        let mut fixture = self.0.lock().unwrap();
        fixture.revision += 1;
        let observation = ResidentRuntimeLoop {
            output_identity: 51,
            audio_revision: fixture.revision,
            source_context: Some(metadata.context),
            source_origin: metadata.source_origin,
            section_loop: None,
        };
        if let ResidentPublicationPolicy::DeviceInsert { .. } = policy {
            let receipt = Arc::new(ConfirmationReceipt {
                submission: ResidentGraphSubmission {
                    output_identity: observation.output_identity,
                    audio_revision: observation.audio_revision,
                    context: metadata.context,
                },
                state: AtomicU8::new(PENDING),
            });
            if let Some(control) = inputs.device_insert_control {
                control.bind_device_insert_receipt(Box::new(ReceiptHandle(Arc::clone(&receipt))));
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
}

fn next_event(service: &Service) -> hex_session::service::Event {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Ok(event) = service.try_event() {
            return event;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for service event"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn make_confirmation_offer(lifetime: Duration) -> (Service, DeviceInsertConfirmation) {
    let fixture = Arc::new(Mutex::new(ConfirmationAudioFixture::default()));
    let max = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES;
    let service = Service::with_audio_sink_and_native_reservation(
        16,
        1024,
        None,
        Box::new(SinkHandle(Arc::clone(&fixture))),
        hex_session::ResidentNativeReservation {
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
    let _ready = next_event(&service);
    let state = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap();
    service
        .submit(hex_session::service::Request::Edit(
            Command::InsertMainDevice {
                before: None,
                state,
            },
        ))
        .unwrap();
    let _pending = next_event(&service);
    let receipt = {
        let mut guard = fixture.lock().unwrap();
        guard.offer_deadline = Some(Instant::now() + lifetime);
        guard.candidate.clone().unwrap()
    };
    receipt.state.store(REFUSED, Ordering::Release);
    let offered = next_event(&service);
    let offer = offered
        .device_insert_confirmation
        .expect("worker must emit device_insert_confirmation offer");
    (service, offer)
}

#[test]
fn device_confirmation_titles_cover_all_operations_in_lowercase() {
    let operations = [
        GraphEditOperation::InsertDevice,
        GraphEditOperation::ReplaceDevice,
        GraphEditOperation::RemoveDevice,
        GraphEditOperation::MoveDevice,
        GraphEditOperation::MoveTrack,
        GraphEditOperation::PowerDevice,
        GraphEditOperation::RemoveTrack,
    ];
    let mut titles = std::collections::BTreeSet::new();
    for operation in operations {
        let title = device_confirmation_title(operation);
        assert!(!title.is_empty());
        assert_eq!(title, theme::text(title).as_ref());
        assert!(title.contains("interrupt playback"));
        assert!(titles.insert(title));
    }
}

#[gpui::test]
fn confirmation_modal_renders_centered_isolates_backdrop_and_answers_clicks(
    cx: &mut TestAppContext,
) {
    cx.update(super::super::bind_editor_keys);
    let (project, _, _, clip) = crate::editor::rendered_test_fixture::audio_project();
    let (handle, editor) =
        crate::editor::rendered_test_fixture::editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();

    // Establish initial editor focus and record initial track count.
    let initial_tracks = editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.tracks().len()
    });
    let editor_focus = editor.update(&mut visual, |view, _| view.focus.clone());
    visual.update(|window, _| editor_focus.focus(window));
    visual.run_until_parked();

    // 1. Install a pending offer and verify centered modal overlay + focus capture + backdrop isolation.
    let (_service_cancel, offer_cancel) = make_confirmation_offer(Duration::from_secs(30));
    editor.update(&mut visual, |view, cx| {
        view.install_device_confirmation(offer_cancel.clone());
        cx.notify();
    });
    visual.run_until_parked();

    let backdrop = visual
        .debug_bounds("device-insert-confirmation-modal")
        .expect("centered confirmation modal backdrop must render");
    let card = visual
        .debug_bounds("device-insert-confirmation")
        .expect("confirmation modal card must render");
    // Card is centered inside the backdrop, not docked as a bottom bar.
    assert!(
        (card.center().x - backdrop.center().x).abs() <= px(2.0)
            && (card.center().y - backdrop.center().y).abs() <= px(2.0),
        "modal card {card:?} must be centered inside backdrop {backdrop:?}"
    );
    assert!(
        card.bottom() < backdrop.bottom() - px(40.0),
        "modal card must not be docked at the bottom of the window"
    );

    // Opening the modal moves focus onto the cancel button.
    let (cancel_focused, editor_focused) = visual.update(|window, cx| {
        editor.read_with(cx, |view, _| {
            let cancel_focused = view
                .device_panel
                .insert_confirmation
                .as_ref()
                .and_then(|c| c.action_focus.as_ref())
                .is_some_and(|[cancel, _]| cancel.is_focused(window));
            (cancel_focused, view.focus.is_focused(window))
        })
    });
    assert!(cancel_focused, "opening modal must focus cancel action");
    assert!(!editor_focused, "editor focus must yield to modal");

    // Clicking the backdrop outside the card and pressing editor shortcuts (`t`) must not leak through.
    let outside_card = point(backdrop.left() + px(8.0), backdrop.top() + px(8.0));
    assert!(!card.contains(&outside_card));
    visual.simulate_click(outside_card, Modifiers::default());
    visual.simulate_keystrokes("t");
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("device-insert-confirmation").is_some(),
        "backdrop click must keep modal open"
    );
    assert!(offer_cancel.is_pending());
    assert_eq!(
        editor.read_with(&visual, |view, _| view
            .state
            .as_ref()
            .unwrap()
            .project
            .tracks()
            .len()),
        initial_tracks,
        "editor shortcut `t` must not pass through modal"
    );

    let arrangement = visual.debug_bounds("arrangement-content").unwrap();
    let grid = point(
        arrangement.origin.x + arrangement.size.width * 0.5,
        arrangement.top() + px(12.0),
    );
    let modifiers = Modifiers::default();
    let clip_center = visual
        .debug_bounds(Box::leak(
            format!("clip-name-{}", clip.value()).into_boxed_str(),
        ))
        .unwrap()
        .center();
    let initial_selection = editor.read_with(&visual, |view, _| view.selected_clip);
    visual.simulate_event(MouseDownEvent {
        position: clip_center,
        button: MouseButton::Left,
        modifiers,
        click_count: 2,
        first_mouse: false,
    });
    visual.simulate_event(MouseUpEvent {
        position: clip_center,
        button: MouseButton::Left,
        modifiers,
        click_count: 2,
    });
    visual.run_until_parked();
    assert_eq!(
        editor.read_with(&visual, |view, _| view.selected_clip),
        initial_selection,
        "double-clicking through the backdrop must not select a clip"
    );

    let initial_viewport = editor.read_with(&visual, |view, _| view.viewport);
    visual.simulate_event(ScrollWheelEvent {
        position: grid,
        delta: ScrollDelta::Pixels(point(px(0.0), px(80.0))),
        ..Default::default()
    });
    visual.run_until_parked();
    assert_eq!(
        editor.read_with(&visual, |view, _| view.viewport),
        initial_viewport,
        "scrolling the backdrop must not zoom the timeline"
    );

    // The Arrangement canvas handles middle presses in the window capture phase.
    visual.simulate_mouse_down(grid, MouseButton::Middle, modifiers);
    visual.run_until_parked();
    assert!(
        editor.read_with(&visual, |view, _| view.arrangement_pan.is_none()),
        "the modal must block Arrangement capture before it starts a pan"
    );
    visual.simulate_mouse_up(grid, MouseButton::Middle, modifiers);

    // Clicking cancel answers the one-shot offer, closes the modal, and restores editor focus.
    let cancel_center = visual
        .debug_bounds("device-insert-confirmation-cancel")
        .unwrap()
        .center();
    visual.simulate_click(cancel_center, Modifiers::default());
    visual.run_until_parked();
    assert!(!offer_cancel.is_pending());
    assert!(
        !offer_cancel.proceed(),
        "one-shot offer must reject proceed after cancel"
    );
    assert!(visual.debug_bounds("device-insert-confirmation").is_none());
    assert!(
        visual.update(|window, _| editor_focus.is_focused(window)),
        "cancelling modal must restore previous editor focus"
    );

    visual.simulate_mouse_down(grid, MouseButton::Middle, modifiers);
    visual.run_until_parked();
    assert!(
        editor.read_with(&visual, |view, _| view.arrangement_pan.is_some()),
        "the same point must start a pan once the modal closes"
    );
    visual.simulate_mouse_up(grid, MouseButton::Middle, modifiers);
    visual.run_until_parked();

    // 2. Clicking proceed answers the one-shot offer, closes the modal, and restores focus.
    let (_service_proceed, offer_proceed) = make_confirmation_offer(Duration::from_secs(30));
    editor.update(&mut visual, |view, cx| {
        view.install_device_confirmation(offer_proceed.clone());
        cx.notify();
    });
    visual.run_until_parked();
    let proceed_center = visual
        .debug_bounds("device-insert-confirmation-proceed")
        .unwrap()
        .center();
    visual.simulate_click(proceed_center, Modifiers::default());
    visual.run_until_parked();
    assert!(!offer_proceed.is_pending());
    assert!(
        !offer_proceed.cancel(),
        "one-shot offer must reject cancel after proceed"
    );
    assert!(visual.debug_bounds("device-insert-confirmation").is_none());
    assert!(
        visual.update(|window, _| editor_focus.is_focused(window)),
        "proceeding modal must restore previous editor focus"
    );
}

#[gpui::test]
fn confirmation_modal_keyboard_precedence_expiry_and_project_replace(cx: &mut TestAppContext) {
    cx.update(super::super::bind_editor_keys);
    let (handle, editor) = crate::editor::clip_drag_ui_test::editor_window(
        cx,
        Project::new(48_000).unwrap(),
        Arc::default(),
    );
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();

    let editor_focus = editor.update(&mut visual, |view, _| view.focus.clone());
    visual.update(|window, _| editor_focus.focus(window));
    visual.run_until_parked();

    // 1. Escape dismisses the modal, cancels the offer, and restores editor focus.
    let (_service_esc, offer_esc) = make_confirmation_offer(Duration::from_secs(30));
    editor.update(&mut visual, |view, cx| {
        view.install_device_confirmation(offer_esc.clone());
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("device-insert-confirmation").is_some());
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert!(!offer_esc.is_pending());
    assert!(!offer_esc.proceed());
    assert!(visual.debug_bounds("device-insert-confirmation").is_none());
    assert!(visual.update(|window, _| editor_focus.is_focused(window)));

    // 2. Tab cycles cancel -> proceed -> cancel, and Enter/Space activates focused button.
    let (_service_tab, offer_tab) = make_confirmation_offer(Duration::from_secs(30));
    editor.update(&mut visual, |view, cx| {
        view.install_device_confirmation(offer_tab.clone());
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("tab");
    visual.run_until_parked();
    let proceed_focused = visual.update(|window, cx| {
        editor.read_with(cx, |view, _| {
            view.device_panel
                .insert_confirmation
                .as_ref()
                .and_then(|c| c.action_focus.as_ref())
                .is_some_and(|[_, proceed]| proceed.is_focused(window))
        })
    });
    assert!(
        proceed_focused,
        "Tab must move focus from cancel to proceed"
    );
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert!(!offer_tab.is_pending());
    assert!(!offer_tab.cancel(), "Enter on proceed must call proceed()");
    assert!(visual.debug_bounds("device-insert-confirmation").is_none());
    assert!(visual.update(|window, _| editor_focus.is_focused(window)));

    // 3. Space on cancel activates cancel and does not toggle playback underneath.
    let (_service_space, offer_space) = make_confirmation_offer(Duration::from_secs(30));
    editor.update(&mut visual, |view, cx| {
        view.install_device_confirmation(offer_space.clone());
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("space");
    visual.run_until_parked();
    assert!(!offer_space.is_pending());
    assert!(!offer_space.proceed(), "Space on cancel must call cancel()");
    assert!(visual.debug_bounds("device-insert-confirmation").is_none());
    assert!(visual.update(|window, _| editor_focus.is_focused(window)));

    // 4. Dialog precedence (`opening_project`, `close_prompt_open`, `save_dialog_open`) hides
    //    the modal without cancelling the offer, and restores it when cleared.
    let (_service_prec, offer_prec) = make_confirmation_offer(Duration::from_secs(30));
    editor.update(&mut visual, |view, cx| {
        view.install_device_confirmation(offer_prec.clone());
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("device-insert-confirmation").is_some());

    for flag in 0..3 {
        editor.update(&mut visual, |view, cx| {
            match flag {
                0 => view.persistence.opening_project = true,
                1 => view.persistence.close_prompt_open = true,
                _ => view.persistence.save_dialog_open = true,
            }
            cx.notify();
        });
        visual.run_until_parked();
        assert!(
            visual.debug_bounds("device-insert-confirmation").is_none(),
            "higher-priority dialog flag {flag} must suppress confirmation modal"
        );
        assert!(
            offer_prec.is_pending(),
            "precedence suppression must not cancel pending offer"
        );
        editor.update(&mut visual, |view, cx| {
            view.persistence.opening_project = false;
            view.persistence.close_prompt_open = false;
            view.persistence.save_dialog_open = false;
            cx.notify();
        });
        visual.run_until_parked();
        assert!(visual.debug_bounds("device-insert-confirmation").is_some());
    }

    // 5. Project replacement cancels the pending offer, removes the modal, and restores focus.
    editor.update(&mut visual, |view, cx| {
        view.clear_device_confirmation_on_project_replace();
        cx.notify();
    });
    visual.run_until_parked();
    assert!(!offer_prec.is_pending());
    assert!(!offer_prec.proceed());
    assert!(visual.debug_bounds("device-insert-confirmation").is_none());
    assert!(
        visual.update(|window, _| editor_focus.is_focused(window)),
        "project replacement must restore editor focus cleanly"
    );

    // 6. External expiry retires the modal automatically and restores editor focus.
    let (_service_exp, offer_exp) = make_confirmation_offer(Duration::from_millis(60));
    editor.update(&mut visual, |view, cx| {
        view.install_device_confirmation(offer_exp.clone());
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("device-insert-confirmation").is_some());
    let deadline = Instant::now() + Duration::from_secs(1);
    while offer_exp.is_pending() {
        assert!(Instant::now() < deadline, "short offer did not expire");
        std::thread::sleep(Duration::from_millis(5));
    }
    editor.update(&mut visual, |_, cx| cx.notify());
    visual.run_until_parked();
    assert!(visual.debug_bounds("device-insert-confirmation").is_none());
    assert!(editor.read_with(&visual, |view, _| {
        view.device_panel.insert_confirmation.is_none()
    }));
    assert!(
        visual.update(|window, _| editor_focus.is_focused(window)),
        "expired offer retirement must restore previous editor focus"
    );
}

#[gpui::test]
fn live_mode_settings_toggle_persists_and_controls_confirmation_modal(cx: &mut TestAppContext) {
    cx.update(super::super::bind_editor_keys);
    let temp_dir = std::env::temp_dir().join(format!(
        "hex-ui-live-mode-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&temp_dir).unwrap();
    let prefs_path = hex_preferences::playback_preferences_path(&temp_dir);

    let (handle, editor) = crate::editor::clip_drag_ui_test::editor_window(
        cx,
        Project::new(48_000).unwrap(),
        Arc::default(),
    );
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();

    // Point playback_settings at an isolated temporary path and load defaults cleanly.
    editor.update(&mut visual, |view, cx| {
        view.settings.playback.path = Some(prefs_path.clone());
        view.settings.playback.loading = false;
        view.settings.playback.saving = false;
        view.settings.playback.writable = true;
        view.settings.playback.error = None;
        view.settings.playback.preferences = hex_preferences::PlaybackPreferences {
            note_chase: false,
            live_mode: true,
        };
        cx.notify();
    });
    visual.run_until_parked();

    // Open Hex Settings (`Cmd+,`) and toggle Live Mode OFF via the UI row.
    visual.simulate_keystrokes("cmd-,");
    visual.run_until_parked();
    let toggle_bounds = visual
        .debug_bounds("live mode settings")
        .expect("live mode toggle must render in Hex Settings");
    visual.simulate_click(toggle_bounds.center(), Modifiers::default());
    let deadline = Instant::now() + Duration::from_secs(2);
    while editor.read_with(&visual, |view, _| view.settings.playback.saving) {
        assert!(Instant::now() < deadline, "live mode save did not settle");
        visual.run_until_parked();
        std::thread::sleep(Duration::from_millis(2));
    }
    visual.run_until_parked();

    let saved = hex_preferences::load_playback_preferences(&prefs_path).unwrap();
    assert!(
        !saved.live_mode,
        "toggling Live Mode off must persist live_mode = false"
    );
    assert!(
        !saved.note_chase,
        "toggling Live Mode must preserve note_chase"
    );
    assert!(editor.read_with(&visual, |view, _| !view.live_mode_enabled()));

    // Close Settings. With Live Mode OFF (non-live mode), an interrupting offer auto-proceeds
    // without displaying the confirmation modal.
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert!(visual.debug_bounds("theme settings panel").is_none());

    let (_service_non_live, offer_non_live) = make_confirmation_offer(Duration::from_secs(30));
    editor.update(&mut visual, |view, cx| {
        view.install_device_confirmation(offer_non_live.clone());
        cx.notify();
    });
    visual.run_until_parked();
    assert!(
        !offer_non_live.is_pending(),
        "non-live mode must immediately resolve the confirmation offer"
    );
    assert!(
        !offer_non_live.cancel(),
        "non-live mode must call proceed() rather than cancel()"
    );
    assert!(
        visual.debug_bounds("device-insert-confirmation").is_none(),
        "non-live mode must not show confirmation modal"
    );
    assert!(editor.read_with(&visual, |view, _| {
        view.device_panel.insert_confirmation.is_none()
    }));

    // Reopen Settings and toggle Live Mode back ON; verify persistence and modal prompting.
    visual.simulate_keystrokes("cmd-,");
    visual.run_until_parked();
    let toggle_bounds = visual.debug_bounds("live mode settings").unwrap();
    visual.simulate_click(toggle_bounds.center(), Modifiers::default());
    let deadline = Instant::now() + Duration::from_secs(2);
    while editor.read_with(&visual, |view, _| view.settings.playback.saving) {
        assert!(
            Instant::now() < deadline,
            "live mode re-enable save did not settle"
        );
        visual.run_until_parked();
        std::thread::sleep(Duration::from_millis(2));
    }
    visual.run_until_parked();
    let reloaded = hex_preferences::load_playback_preferences(&prefs_path).unwrap();
    assert!(reloaded.live_mode);
    assert!(!reloaded.note_chase);

    // Non-writable/error state disables clicks and renders status row.
    editor.update(&mut visual, |view, cx| {
        view.settings.playback.writable = false;
        view.settings.playback.error = Some("could not save playback settings · read-only".into());
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("playback settings status").is_some());
    let toggle_bounds = visual.debug_bounds("live mode settings").unwrap();
    visual.simulate_click(toggle_bounds.center(), Modifiers::default());
    visual.run_until_parked();
    assert!(
        editor.read_with(&visual, |view, _| view.live_mode_enabled()
            && !view.settings.playback.saving),
        "disabled live mode row must ignore clicks"
    );

    // Close Settings and verify Live Mode ON shows the centered confirmation modal.
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    let (_service_live, offer_live) = make_confirmation_offer(Duration::from_secs(30));
    editor.update(&mut visual, |view, cx| {
        view.install_device_confirmation(offer_live.clone());
        cx.notify();
    });
    visual.run_until_parked();
    assert!(offer_live.is_pending());
    assert!(
        visual.debug_bounds("device-insert-confirmation").is_some(),
        "Live Mode ON must show confirmation modal"
    );
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert!(!offer_live.is_pending());
    let _ = std::fs::remove_dir_all(&temp_dir);
}
