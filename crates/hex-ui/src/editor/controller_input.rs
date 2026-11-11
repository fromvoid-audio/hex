//! Optional controller selection and physical-note capture for step entry.
//! Expression and ordered hardware notes use the existing prepared host leases.
//! Held snapshots remain a separate step-entry query, never a live-note scheduler.
mod monitoring;
mod preferences;
use super::EditorView;
use crate::theme;
use gpui::{
    AppContext, Context, Div, Entity, FocusHandle, MouseButton, Window, div, prelude::*, px, rgb,
};
use hex_engine::live_expression::LiveExpressionInput;
use hex_widgets::{Selector, SelectorChanged, SelectorConfig};
use std::sync::Arc;

#[cfg(all(test, feature = "ui-test-support"))]
#[gpui::test]
fn settings_uses_stable_port_identity_preserves_missing_choice_and_saves_explicit_selection(
    cx: &mut gpui::TestAppContext,
) {
    use super::midi_canvas::creation_tests::fixture;
    let (mut visual, editor, _environment, _) = fixture(cx);
    visual.run_until_parked();
    editor.update(&mut visual, |view, cx| {
        assert!(!view.controller_input.preferences.loading);
        view.controller_input.preferences.value = hex_preferences::MidiInputPreferences {
            port: Some(hex_preferences::MidiInputPort {
                id: 42,
                name: "Original Name".into(),
            }),
            channel: 3,
        };
        view.controller_input.channel = 3;
        view.install_controller_selectors(
            vec![
                hex_midi::Port {
                    id: 7,
                    name: "Original Name".into(),
                },
                hex_midi::Port {
                    id: 42,
                    name: "Renamed Device".into(),
                },
            ],
            cx,
        );
        assert_eq!(view.controller_input.selected_port.as_ref().unwrap().id, 42);
        view.install_controller_selectors(
            vec![hex_midi::Port {
                id: 7,
                name: "Other Device".into(),
            }],
            cx,
        );
        assert!(view.controller_input.selected_port.is_none());
        assert_eq!(
            view.controller_input
                .preferences
                .value
                .port
                .as_ref()
                .unwrap()
                .id,
            42
        );
    });
    visual.simulate_keystrokes("cmd-,");
    visual.run_until_parked();
    let panel = visual.debug_bounds("theme settings panel").unwrap();
    for id in [
        "midi-input-settings",
        "controller-input-port",
        "controller-input-channel",
    ] {
        let bounds = visual.debug_bounds(id).unwrap();
        assert!(bounds.origin.y >= panel.origin.y);
        assert!(bounds.origin.y + bounds.size.height <= panel.origin.y + panel.size.height);
    }
    editor.update(&mut visual, |view, cx| view.select_controller_port(1, cx));
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(!view.controller_input.preferences.saving);
        assert!(view.controller_input.preferences.error.is_none());
        let persisted = hex_preferences::load_midi_input_preferences(
            view.controller_input.preferences.path.as_ref().unwrap(),
        )
        .unwrap();
        assert_eq!(persisted.channel, 3);
        let port = persisted.port.unwrap();
        assert_eq!((port.id, port.name.as_str()), (7, "Other Device"));
    });
}

#[derive(Default)]
pub(super) struct ControllerInput {
    preferences: preferences::Preferences,
    handle: Option<hex_midi::Handle>,
    ports: Vec<hex_midi::Port>,
    port_selector: Option<Entity<Selector>>,
    channel_selector: Option<Entity<Selector>>,
    subscriptions: Vec<gpui::Subscription>,
    selected_port: Option<hex_midi::Port>,
    channel: u8,
    target: Option<(u64, hex_project::TrackId, hex_project::DeviceId)>,
    input: Option<Arc<LiveExpressionInput>>,
    held_notes: Option<Arc<hex_midi::HeldNoteInput>>,
    step_owner: Option<(u64, hex_project::TrackId, hex_project::ClipId)>,
    live_owner: Option<(u64, hex_project::TrackId, hex_project::ClipId)>,
    live_floor: u64,
    live_computer_floor: u64,
    live_floor_pending: bool,
    displayed_notes: Option<hex_midi::HeldNotes>,
    step_floor: u64,
    step_floor_pending: bool,
    step_faulted: bool,
    connected: bool,
    connecting: bool,
    expression_prepared: bool,
    note_input: Option<(
        Arc<hex_engine::computer_keyboard::ComputerKeyboardInput>,
        hex_engine::computer_keyboard::LiveNoteActivation,
    )>,
    activation: u64,
    discovery: u64,
    blocked_for_streaming: bool,
    menu_focus: Option<FocusHandle>,
    previous_focus: Option<FocusHandle>,
}
impl ControllerInput {
    fn connection_event(
        &mut self,
        activation: u64,
        result: Result<bool, String>,
    ) -> Option<String> {
        if activation != self.activation {
            return None;
        }
        self.connecting = false;
        match result {
            Ok(connected) => {
                self.connected = connected;
                None
            }
            Err(error) => {
                self.cancel();
                self.step_faulted = true;
                self.selected_port = None;
                Some(error)
            }
        }
    }
    fn discard_step_prefix(&mut self) {
        self.step_floor_pending = true;
        if let Some(notes) = &self.held_notes
            && let hex_midi::HeldNotesRead::Snapshot(notes) = notes.read()
        {
            self.step_floor = notes.through();
            self.step_floor_pending = false;
        }
    }
    fn discard_live_prefix(&mut self) {
        self.live_floor_pending = true;
        if let Some(input) = &self.held_notes
            && let hex_midi::HeldNotesRead::Snapshot(notes) = input.read()
        {
            self.live_floor = notes.through();
            self.live_floor_pending = false;
        }
    }
    fn cancel(&mut self) {
        self.connecting = false;
        self.expression_prepared = false;
        if let Some((input, activation)) = self.note_input.take()
            && let Some(notes) = input.live_notes()
        {
            let _ = notes.cancel_activation(activation);
        }
        self.step_owner = None;
        self.live_owner = None;
        self.live_floor = 0;
        self.live_floor_pending = false;
        self.displayed_notes = None;
        self.step_floor = 0;
        self.step_floor_pending = false;
        if let Some(input) = self.input.take() {
            input.cancel();
        }
        if let Some(notes) = self.held_notes.take() {
            notes.cancel();
        }
        self.target = None;
        // Exhaustion forbids another activation; stale results remain ignored.
        self.discovery = self.discovery.saturating_add(1);
        let previous_activation = self.activation;
        self.activation = self.activation.saturating_add(1);
        self.connected = false;
        if let Some(handle) = &self.handle {
            let _ = handle.send(hex_midi::Command::Disconnect {
                activation: previous_activation,
            });
        }
    }
}
impl Drop for ControllerInput {
    fn drop(&mut self) {
        self.cancel();
    }
}
impl EditorView {
    /// Planned root replacement ends its connection before cancelling the root.
    /// Preserve the configured port so fault detection cannot mistake this for
    /// disappearance and silently turn hardware input off.
    pub(super) fn reprepare_controller_for_recording(&mut self) {
        if self.controller_wants_live_notes() {
            self.controller_input.cancel();
        }
    }
    pub(super) fn controller_capture_ready(&self) -> bool {
        !self.controller_wants_live_notes()
            || (self.controller_input.connected
                && self
                    .controller_input
                    .note_input
                    .as_ref()
                    .zip(self.computer_keyboard.input.as_ref())
                    .is_some_and(|((connected, token), input)| {
                        Arc::ptr_eq(connected, input)
                            && input
                                .live_notes()
                                .is_some_and(|live| live.same_current_activation(*token))
                    }))
    }
    pub(super) fn controller_capture_activation(
        &self,
        input: &Arc<hex_engine::computer_keyboard::ComputerKeyboardInput>,
    ) -> Result<Option<hex_engine::computer_keyboard::LiveNoteActivation>, &'static str> {
        if !self.controller_wants_live_notes() {
            return Ok(None);
        }
        let (connected, token) = self
            .controller_input
            .note_input
            .as_ref()
            .ok_or("midi controller is not connected")?;
        if !self.controller_input.connected
            || !Arc::ptr_eq(connected, input)
            || input
                .live_notes()
                .is_none_or(|live| !live.same_current_activation(*token))
        {
            return Err("midi controller input changed before recording");
        }
        Ok(Some(*token))
    }

    pub(super) fn stop_controller_notes(&mut self) {
        if self.controller_input.note_input.is_some() {
            self.controller_input.cancel();
        }
    }
    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn set_test_audition_port(&mut self) {
        self.controller_input.selected_port = Some(hex_midi::Port {
            id: 1,
            name: "synthetic input".into(),
        });
        self.controller_input.target = self.controller_target();
    }

    pub(super) fn controller_wants_live_notes(&self) -> bool {
        self.controller_input.selected_port.is_some()
            && self.document_audio_mode == hex_session::DocumentAudioMode::Resident
    }
    pub(super) fn controller_expression_prepared(&mut self) {
        self.controller_input.expression_prepared = true;
        self.connect_prepared_controller();
    }
    fn controller_target(&self) -> Option<(u64, hex_project::TrackId, hex_project::DeviceId)> {
        let state = self.state.as_ref()?;
        let track = self.selected_track?;
        let instrument = state
            .project
            .track(track)?
            .instrument()
            .filter(|instrument| !instrument.bypassed())?;
        Some((self.project_generation, track, instrument.id()))
    }
    fn controller_target_uses_vst3_notes(&self) -> bool {
        self.selected_track
            .zip(self.state.as_ref())
            .and_then(|(track, state)| state.project.track(track)?.instrument())
            .is_some_and(|instrument| instrument.state().type_id().as_str() == Ok("hex.vst3"))
    }
    /// Called for every rendered focus/clip context. Both leaving and entering
    /// establish a floor, excluding keys pressed while the editor was unfocused.
    pub(super) fn sync_hardware_step_context(&mut self, window: &Window) {
        let live_owner = (window.is_window_active()
            && self.midi_focus.is_focused(window)
            && self.clip_view_open)
            .then(|| {
                self.selected_track
                    .zip(self.detail_clip_id())
                    .map(|(track, clip)| (self.project_generation, track, clip))
            })
            .flatten();
        if live_owner != self.controller_input.live_owner {
            self.controller_input.live_owner = live_owner;
            self.controller_input.live_computer_floor = self.computer_keyboard.physical_through();
            self.controller_input.discard_live_prefix();
        }
        if self.controller_input.live_floor_pending {
            self.controller_input.discard_live_prefix();
        }
        let owner = live_owner.filter(|_| self.midi_step.enabled);
        if owner != self.controller_input.step_owner {
            self.controller_input.step_owner = owner;
            self.controller_input.discard_step_prefix();
        }
        if self.controller_input.step_floor_pending {
            self.controller_input.discard_step_prefix();
        }
    }
    pub(super) fn invalidate_hardware_step(&mut self) {
        self.controller_input.discard_step_prefix();
        self.controller_input.discard_live_prefix();
        self.controller_input.live_computer_floor = self.computer_keyboard.physical_through();
    }

    pub(super) fn live_computer_floor(&self) -> u64 {
        self.controller_input.live_computer_floor
    }

    pub(super) fn live_midi_context(&self) -> bool {
        self.controller_input.live_owner.is_some()
            && self.controller_input.live_owner
                == self
                    .selected_track
                    .zip(self.detail_clip_id())
                    .map(|(track, clip)| (self.project_generation, track, clip))
    }

    pub(super) fn hardware_live_notes(&self) -> Option<(hex_midi::HeldNotes, u64, u8)> {
        if !self.live_midi_context()
            || self.controller_input.live_floor_pending
            || self.controller_input.step_faulted
            || self.controller_input.target != self.controller_target()
        {
            return None;
        }
        match self.controller_input.held_notes.as_ref()?.read() {
            hex_midi::HeldNotesRead::Snapshot(notes) => Some((
                notes,
                self.controller_input.live_floor,
                self.controller_input.channel,
            )),
            _ => None,
        }
    }

    /// Poll the existing bounded capture endpoint so NoteOff and reset repaint
    /// even when expression and project state have not changed. No audio writes.
    pub(super) fn tick_midi_pitch_feedback(&mut self) -> bool {
        let next = self.hardware_live_notes().map(|(notes, _, _)| notes);
        let changed = next != self.controller_input.displayed_notes;
        self.controller_input.displayed_notes = next;
        changed
    }

    /// Busy/invalid endpoints refuse the whole step, including a false rest.
    pub(super) fn hardware_step_notes(
        &self,
    ) -> Result<Option<(hex_midi::HeldNotes, u64, u8)>, &'static str> {
        if self.controller_input.step_faulted {
            return Err(
                "controller note input stopped · choose a port or turn controller input off",
            );
        }
        let Some(input) = &self.controller_input.held_notes else {
            return Ok(None);
        };
        // Refuse an old instrument lease even before the worker's next port tick.
        if self.controller_input.target != self.controller_target() {
            return Err("controller target changed · retry after input reconnects");
        }
        if self.controller_input.step_owner
            != self
                .selected_track
                .zip(self.detail_clip_id())
                .map(|(track, clip)| (self.project_generation, track, clip))
            || self.controller_input.step_floor_pending
        {
            return Err("controller step input is waiting for a fresh focused context");
        }
        match input.read() {
            hex_midi::HeldNotesRead::Snapshot(notes) => Ok(Some((
                notes,
                self.controller_input.step_floor,
                self.controller_input.channel,
            ))),
            hex_midi::HeldNotesRead::Busy => {
                Err("controller note snapshot is busy · retry the step")
            }
            hex_midi::HeldNotesRead::Cancelled | hex_midi::HeldNotesRead::Fault(_) => {
                Err("controller note input is unavailable · reconnect the port")
            }
        }
    }

    fn refresh_controller_inputs(&mut self, cx: &mut Context<Self>) {
        if self.document_audio_mode == hex_session::DocumentAudioMode::VerifiedStreaming {
            self.error =
                Some("controller input is unavailable for verified streaming audio".into());
            cx.notify();
            return;
        }
        if self.controller_input.handle.is_none() {
            self.controller_input.handle = cx
                .try_global::<crate::controller_input::ControllerWorker>()
                .map(|worker| worker.0.clone());
        }
        let Some(request_id) = self.controller_input.discovery.checked_add(1) else {
            self.error = Some("controller discovery identity exhausted".into());
            return;
        };
        self.controller_input.discovery = request_id;
        match &self.controller_input.handle {
            Some(handle) => {
                if let Err(error) = handle.send(hex_midi::Command::Discover { request_id }) {
                    self.error = Some(error.into());
                }
            }
            None => self.error = Some("controller input worker is unavailable".into()),
        }
        cx.notify();
    }
    fn select_controller_port(&mut self, index: usize, cx: &mut Context<Self>) {
        if !self.controller_preferences_editable() {
            self.sync_controller_selectors(cx);
            return;
        }
        self.controller_input.cancel();
        self.controller_input.step_faulted = false;
        self.controller_input.selected_port = index
            .checked_sub(1)
            .and_then(|index| self.controller_input.ports.get(index))
            .cloned();
        self.controller_input.preferences.value.port =
            self.controller_input.selected_port.as_ref().map(|port| {
                hex_preferences::MidiInputPort {
                    id: port.id,
                    name: port.name.clone(),
                }
            });
        self.save_controller_preferences(cx);
        self.computer_keyboard.suspended = false;
        self.tick_controller_input(cx);
        cx.notify();
    }
    fn install_controller_selectors(&mut self, ports: Vec<hex_midi::Port>, cx: &mut Context<Self>) {
        self.controller_input.cancel();
        let restore = self
            .controller_input
            .selected_port
            .as_ref()
            .map(|port| port.id)
            .or_else(|| {
                self.controller_input
                    .preferences
                    .value
                    .port
                    .as_ref()
                    .map(|port| port.id)
            });
        self.controller_input.selected_port =
            ports.iter().find(|port| Some(port.id) == restore).cloned();
        self.controller_input.step_faulted = false;
        self.controller_input.ports = ports;
        self.controller_input.subscriptions.clear();
        let choices = std::iter::once(theme::text("off")).chain(
            self.controller_input
                .ports
                .iter()
                .map(|port| theme::text(&port.name)),
        );
        let port_config = SelectorConfig::new(
            "controller-input-port",
            choices,
            self.controller_input
                .selected_port
                .as_ref()
                .and_then(|selected| {
                    self.controller_input
                        .ports
                        .iter()
                        .position(|port| port.id == selected.id)
                })
                .map_or(0, |index| index + 1),
            theme::control_style(),
        );
        let channel_config = SelectorConfig::new(
            "controller-input-channel",
            (1..=16).map(|channel| theme::text(format!("channel {channel}"))),
            usize::from(self.controller_input.channel),
            theme::control_style(),
        );
        let (Ok(port_config), Ok(channel_config)) = (port_config, channel_config) else {
            self.error = Some("controller input options exceed supported limits".into());
            return;
        };
        let port = cx.new(|cx| Selector::from_config(cx, port_config));
        let channel = cx.new(|cx| Selector::from_config(cx, channel_config));
        let port_subscription = cx.subscribe(&port, |view, _, event: &SelectorChanged, cx| {
            view.select_controller_port(event.index, cx)
        });
        let channel_subscription =
            cx.subscribe(&channel, |view, _, event: &SelectorChanged, cx| {
                if !view.controller_preferences_editable() || event.index >= 16 {
                    view.sync_controller_selectors(cx);
                    return;
                }
                view.controller_input.cancel();
                view.controller_input.step_faulted = false;
                view.controller_input.channel = event.index as u8;
                view.controller_input.preferences.value.channel = event.index as u8;
                view.save_controller_preferences(cx);
                view.tick_controller_input(cx);
                cx.notify();
            });
        self.controller_input
            .subscriptions
            .extend([port_subscription, channel_subscription]);
        self.controller_input.port_selector = Some(port);
        self.controller_input.channel_selector = Some(channel);
    }
    fn sync_controller_selectors(&mut self, cx: &mut Context<Self>) {
        let index = self
            .controller_input
            .selected_port
            .as_ref()
            .and_then(|selected| {
                self.controller_input
                    .ports
                    .iter()
                    .position(|port| port.id == selected.id)
            })
            .map_or(0, |index| index + 1);
        if let Some(selector) = &self.controller_input.port_selector {
            selector.update(cx, |selector, cx| {
                let _ = selector.set_selected(index, cx);
            });
        }
        if let Some(selector) = &self.controller_input.channel_selector {
            selector.update(cx, |selector, cx| {
                let _ = selector.set_selected(usize::from(self.controller_input.channel), cx);
            });
        }
    }
    fn reset_controller_selector(&mut self, cx: &mut Context<Self>) {
        if let Some(selector) = &self.controller_input.port_selector {
            selector.update(cx, |selector, cx| {
                let _ = selector.set_selected(0, cx);
            });
        }
    }
    pub(super) fn tick_controller_input(&mut self, cx: &mut Context<Self>) -> bool {
        if self.document_audio_mode == hex_session::DocumentAudioMode::VerifiedStreaming {
            if !self.controller_input.blocked_for_streaming {
                let active = self.controller_input.selected_port.is_some()
                    || self.controller_input.input.is_some();
                self.controller_input.cancel();
                self.controller_input.selected_port = None;
                self.controller_input.blocked_for_streaming = true;
                self.reset_controller_selector(cx);
                if active {
                    self.error =
                        Some("controller input is unavailable for verified streaming audio".into());
                }
                return true;
            }
            return false;
        }
        self.controller_input.blocked_for_streaming = false;
        let mut changed = false;
        for _ in 0..4 {
            let event = self
                .controller_input
                .handle
                .as_ref()
                .and_then(|handle| handle.try_event().ok());
            match event {
                Some(hex_midi::Event::Ports { request_id, result }) => {
                    if request_id == self.controller_input.discovery {
                        match result {
                            Ok(ports) => self.install_controller_selectors(ports, cx),
                            Err(error) => self.error = Some(theme::text(error).to_string().into()),
                        }
                        changed = true;
                    }
                }
                Some(hex_midi::Event::Connection { activation, result }) => {
                    if let Some(error) = self.controller_input.connection_event(activation, result)
                    {
                        self.error = Some(theme::text(error).to_string().into());
                    }
                    changed = true;
                }
                None => break,
            }
        }
        if self
            .controller_input
            .note_input
            .as_ref()
            .is_some_and(|(input, activation)| {
                input.is_cancelled()
                    || input
                        .live_notes()
                        .is_none_or(|notes| !notes.same_current_activation(*activation))
            })
        {
            self.error = Some("controller note input stopped · reconnect the port".into());
            self.controller_input.cancel();
            self.controller_input.selected_port = None;
            self.reset_controller_selector(cx);
            return true;
        }
        if self
            .controller_input
            .held_notes
            .as_ref()
            .is_some_and(|notes| notes.is_cancelled())
        {
            self.error = Some("controller note input stopped · reconnect the port".into());
            self.controller_input.cancel();
            self.controller_input.step_faulted = true;
            self.controller_input.selected_port = None;
            self.reset_controller_selector(cx);
            return true;
        }
        if self
            .controller_input
            .input
            .as_ref()
            .is_some_and(|input| input.is_cancelled())
        {
            self.controller_input.cancel();
            self.controller_input.selected_port = None;
            self.reset_controller_selector(cx);
            return true;
        }
        let Some(port) = self.controller_input.selected_port.clone() else {
            self.reset_controller_selector(cx);
            return changed;
        };
        let target = self.controller_target();
        if target != self.controller_input.target {
            self.controller_input.cancel();
            let Some((generation, track, instrument)) = target else {
                return true;
            };
            if self.controller_input.activation == u64::MAX {
                self.error = Some("controller activation identity exhausted".into());
                return true;
            }
            self.controller_input.held_notes = Some(Arc::new(hex_midi::HeldNoteInput::default()));
            if self.controller_target_uses_vst3_notes() {
                self.controller_input.target = target;
                self.connect_prepared_controller();
                return true;
            }
            let input = Arc::new(LiveExpressionInput::new(generation, track, instrument));
            let revision = self.state.as_ref().map_or(0, |state| state.revision);
            if !self.enqueue(hex_session::service::Request::PrepareLiveExpression {
                input: Arc::clone(&input),
                revision,
            }) {
                input.cancel();
                self.controller_input.cancel();
                self.controller_input.selected_port = None;
                self.reset_controller_selector(cx);
                return true;
            }
            // Preparation must complete before opening a hardware port. The tick
            // below observes the same lease only after the worker's event returns.
            self.controller_input.target = target;
            self.controller_input.input = Some(input);
            let _ = port;
            return true;
        }
        self.connect_prepared_controller();
        changed
    }
    pub(super) fn controller_input_matches(&self, input: &Arc<LiveExpressionInput>) -> bool {
        let current_target = self.controller_target();
        self.document_audio_mode == hex_session::DocumentAudioMode::Resident
            && self
                .controller_input
                .input
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, input))
            && self.controller_input.target == current_target
            && current_target == Some((input.project_generation, input.track, input.instrument))
            && !input.is_cancelled()
    }
    pub(super) fn connect_prepared_controller(&mut self) {
        let note_only = self.controller_target_uses_vst3_notes();
        let Some((generation, track, _)) = self.controller_input.target else {
            return;
        };
        if self.controller_input.connected
            || self.controller_input.connecting
            || (!note_only && !self.controller_input.expression_prepared)
            || !self.computer_keyboard.prepared
            || self.controller_input.target != self.controller_target()
            || !self
                .computer_keyboard
                .target
                .is_some_and(|(owner, _, selected)| owner == generation && selected == track)
        {
            return;
        }
        let Some(note_input) = self
            .computer_keyboard
            .input
            .as_ref()
            .filter(|input| !input.is_cancelled() && input.live_notes().is_some())
        else {
            return;
        };
        let note_input = Arc::clone(note_input);
        let expression = self
            .controller_input
            .input
            .as_ref()
            .filter(|input| !input.is_cancelled())
            .cloned();
        if !note_only && expression.is_none() {
            return;
        }
        let Some(port) = self.controller_input.selected_port.clone() else {
            return;
        };
        let Some(handle) = &self.controller_input.handle else {
            return;
        };
        let Some(notes) = self.controller_input.held_notes.as_ref() else {
            return;
        };
        let activation = match note_input.begin_live_notes_activation_on_control() {
            Ok(activation) => activation,
            Err(error) => {
                self.error = Some(
                    format!("controller note activation failed · {error}")
                        .to_lowercase()
                        .into(),
                );
                return;
            }
        };
        let held = Arc::clone(notes);
        self.controller_input.note_input = Some((Arc::clone(&note_input), activation));
        let sink = monitoring::sink(expression, held, note_input, activation);
        self.controller_input.connecting = true;
        if let Err(error) = handle.send(hex_midi::Command::Connect {
            activation: self.controller_input.activation,
            port,
            channel: self.controller_input.channel,
            sink,
        }) {
            self.controller_input.connecting = false;
            self.error = Some(error.into());
        }
    }
    pub(super) fn controller_input_is_open(&self) -> bool {
        self.controller_input.menu_focus.is_some()
    }
    pub(super) fn close_controller_input_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.controller_input.menu_focus = None;
        window.focus(
            self.controller_input
                .previous_focus
                .take()
                .as_ref()
                .unwrap_or(&self.focus),
        );
        cx.notify();
    }
    fn open_controller_input_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.controller_input_is_open() {
            self.close_controller_input_menu(window, cx);
            return;
        }
        self.invalidate_hardware_step();
        self.controller_input.previous_focus = window.focused(cx);
        let focus = cx.focus_handle();
        window.focus(&focus);
        self.controller_input.menu_focus = Some(focus);
        cx.notify();
    }
    pub(super) fn controller_input_row(&self, cx: &mut Context<Self>) -> Div {
        let connection = if self.controller_input.connected {
            "connected"
        } else if self.controller_input.selected_port.is_some()
            && self.controller_input.target.is_some()
        {
            "pending"
        } else {
            "off"
        };
        let monitoring = if self.controller_target_uses_vst3_notes() {
            "hardware notes play the selected vst3 instrument"
        } else {
            "hardware notes, pressure, pitch bend and mod wheel for the selected instrument"
        };
        let description = theme::text(format!(
            "controller input {connection} · {monitoring} · click to choose port and channel · hardware notes support piano-roll step entry"
        ));
        div().w(px(46.0)).flex_shrink_0().children([div()
            .id("controller-input-menu")
            .debug_selector(|| "controller-input-menu".into())
            .h(px(20.0))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .text_color(rgb(theme::color(if self.controller_input.connected {
                theme::PaletteRole::Primary
            } else {
                theme::PaletteRole::Muted
            })))
            .child(theme::text("midi"))
            .tooltip(move |_, cx| {
                cx.new(|_| super::transport::MiniTooltip {
                    kind: crate::tooltips::Kind::Identification,
                    description: description.clone(),
                })
                .into()
            })
            .on_click(
                cx.listener(|view, _, window, cx| view.open_controller_input_menu(window, cx)),
            )])
    }
    pub(super) fn controller_input_settings(&self, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let status = if self.controller_input.preferences.loading {
            "loading"
        } else if self.controller_input.preferences.saving {
            "saving"
        } else if self.controller_input.preferences.unwritable {
            "settings unavailable"
        } else if self.controller_input.selected_port.is_none()
            && self.controller_input.preferences.value.port.is_some()
        {
            if self
                .controller_input
                .preferences
                .value
                .port
                .as_ref()
                .is_some_and(|saved| {
                    self.controller_input
                        .ports
                        .iter()
                        .any(|port| port.id == saved.id)
                })
            {
                "stopped"
            } else {
                "saved port unavailable"
            }
        } else if self.controller_input.connected {
            "connected"
        } else if self.controller_input.selected_port.is_some() {
            "preparing"
        } else {
            "off"
        };
        div()
            .id("midi-input-settings")
            .debug_selector(|| "midi-input-settings".into())
            .flex()
            .flex_col()
            .gap(px(4.0))
            .text_xs()
            .child(
                div()
                    .id("controller-input-refresh")
                    .debug_selector(|| "controller-input-refresh".into())
                    .h(px(22.0))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .child(theme::text(format!("midi input · {status} · refresh ↻")))
                    .on_click(cx.listener(|view, _, _, cx| view.refresh_controller_inputs(cx))),
            )
            .children(self.controller_input.port_selector.clone())
            .children(self.controller_input.channel_selector.clone())
            .children(
                self.controller_input
                    .preferences
                    .error
                    .as_ref()
                    .map(|error| div().text_xs().child(theme::text(error))),
            )
    }
    pub(super) fn prepare_controller_settings(&mut self, cx: &mut Context<Self>) {
        if self.controller_input.port_selector.is_none() {
            self.install_controller_selectors(Vec::new(), cx);
        }
        self.refresh_controller_inputs(cx);
    }
    pub(super) fn controller_input_overlay(&self, cx: &mut Context<Self>) -> Option<Div> {
        if self.theme_settings.open {
            return None;
        }
        let focus = self.controller_input.menu_focus.as_ref()?;
        let panel = div()
            .id("controller-input-panel")
            .debug_selector(|| "controller-input-panel".into())
            .absolute()
            .bottom(px(24.0))
            .right(px(4.0))
            .w(px(300.0))
            .p(px(8.0))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .border_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .bg(rgb(theme::color(theme::PaletteRole::Surface)))
            .shadow_md()
            .text_xs()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
            .child(self.controller_input_settings(cx));
        Some(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .track_focus(focus)
                .capture_key_down(cx.listener(|view, event: &gpui::KeyDownEvent, window, cx| {
                    // A selector handles Escape while its own popup is open.
                    // Once closed, Escape dismisses the containing menu even
                    // while that selector retains keyboard focus.
                    let selector_open = [
                        &view.controller_input.port_selector,
                        &view.controller_input.channel_selector,
                    ]
                    .into_iter()
                    .flatten()
                    .any(|selector| selector.read(cx).is_open());
                    if event.keystroke.key.as_str() == "escape" && !selector_open {
                        cx.stop_propagation();
                        view.close_controller_input_menu(window, cx);
                    }
                }))
                .on_key_down(cx.listener(|view, event: &gpui::KeyDownEvent, window, cx| {
                    cx.stop_propagation();
                    if event.keystroke.key.as_str() == "escape" {
                        view.close_controller_input_menu(window, cx);
                    }
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|view, _, window, cx| {
                        cx.stop_propagation();
                        view.close_controller_input_menu(window, cx);
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|view, _, window, cx| {
                        cx.stop_propagation();
                        view.close_controller_input_menu(window, cx);
                    }),
                )
                .on_mouse_down(
                    MouseButton::Middle,
                    cx.listener(|view, _, window, cx| {
                        cx.stop_propagation();
                        view.close_controller_input_menu(window, cx);
                    }),
                )
                .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                .children([panel]),
        )
    }
}

#[cfg(all(test, not(feature = "ui-test-support")))]
mod tests {
    use super::*;
    #[test]
    fn delayed_connection_a_results_cannot_cancel_or_mark_connection_b() {
        let mut editor =
            hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 4).unwrap();
        let hex_project::Created::Track(track) = editor
            .execute(hex_project::Command::InsertTrackWithDevice {
                name: "controller".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().expect("fixture instrument state"),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let instrument = editor
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        let input = Arc::new(LiveExpressionInput::new(1, track, instrument));
        let mut state = ControllerInput::default();
        state.activation = 2;
        state.input = Some(Arc::clone(&input));
        let notes = Arc::new(hex_midi::HeldNoteInput::default());
        state.held_notes = Some(Arc::clone(&notes));
        assert_eq!(
            state.connection_event(1, Err("delayed a failure".into())),
            None
        );
        assert!(!input.is_cancelled());
        assert!(!notes.is_cancelled());
        assert_eq!(state.connection_event(1, Ok(true)), None);
        assert!(!state.connected);
        state.connection_event(2, Ok(true));
        assert!(state.connected);
        state.connection_event(1, Ok(false));
        assert!(state.connected);
        assert!(!input.is_cancelled());
        assert_eq!(
            state.connection_event(2, Err("b disconnected".into())),
            Some("b disconnected".into())
        );
        assert!(input.is_cancelled());
        assert!(notes.is_cancelled());
        assert!(state.step_faulted);
        assert!(!state.connected);
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[gpui::test]
fn controller_input_menu_preserves_viewport_and_dismisses_without_editor_actions(
    cx: &mut gpui::TestAppContext,
) {
    use gpui::{Modifiers, VisualTestContext, point, size};
    let (handle, editor) = super::clip_drag_ui_test::editor_window(
        cx,
        hex_project::Project::new(48_000).unwrap(),
        Arc::default(),
    );
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(880.0), px(540.0)));
    visual.run_until_parked();
    let viewport = visual.debug_bounds("arrangement-content").unwrap();
    editor.update(&mut visual, |view, cx| {
        view.install_controller_selectors(
            vec![hex_midi::Port {
                id: 1,
                name: "a synthetic endpoint with a deliberately long display name".into(),
            }],
            cx,
        );
        // Display the longest endpoint without activating any hardware.
        view.controller_input
            .port_selector
            .as_ref()
            .unwrap()
            .update(cx, |selector, cx| {
                selector.set_selected(1, cx).expect("synthetic port choice");
            });
        cx.notify();
    });
    visual.run_until_parked();
    assert_eq!(
        visual.debug_bounds("arrangement-content").unwrap(),
        viewport
    );
    let status = visual.debug_bounds("editor-status-bar").unwrap();
    assert_eq!(status.size.height, px(22.0));
    let message = visual.debug_bounds("editor-status-message").unwrap();
    assert!(message.size.width >= px(50.0), "message={message:?}");
    let revision = visual.debug_bounds("editor-revision-status").unwrap();
    assert!(revision.size.width >= px(64.0) && revision.size.width <= px(176.0));
    assert!(revision.origin.x + revision.size.width <= status.origin.x + status.size.width);
    assert!(visual.debug_bounds("controller-input-panel").is_none());
    let previous_focus = visual.update(|window, cx| window.focused(cx));
    let open = |visual: &mut VisualTestContext| {
        let button = visual.debug_bounds("controller-input-menu").unwrap();
        let position = point(button.origin.x + px(10.0), button.origin.y + px(10.0));
        visual.simulate_mouse_down(position, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(position, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
    };
    open(&mut visual);
    let panel = visual.debug_bounds("controller-input-panel").unwrap();
    assert_eq!(
        visual.debug_bounds("editor-status-message").unwrap(),
        message
    );
    assert_eq!(
        visual.debug_bounds("editor-revision-status").unwrap(),
        revision
    );
    assert!(panel.origin.y + panel.size.height <= status.origin.y);
    for id in [
        "controller-input-refresh",
        "controller-input-port",
        "controller-input-channel",
    ] {
        let control = visual
            .debug_bounds(id)
            .expect("controller control is visible");
        assert!(control.origin.y >= panel.origin.y);
        assert!(control.origin.y + control.size.height <= panel.origin.y + panel.size.height);
        assert!(control.origin.x >= panel.origin.x);
        assert!(control.origin.x + control.size.width <= panel.origin.x + panel.size.width);
    }
    assert_eq!(
        visual.debug_bounds("arrangement-content").unwrap(),
        viewport
    );
    // The deferred selector popup remains interactive above the menu backdrop.
    let channel = visual.debug_bounds("controller-input-channel").unwrap();
    let position = point(channel.origin.x + px(10.0), channel.origin.y + px(10.0));
    visual.simulate_mouse_down(position, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(position, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert!(visual.debug_bounds("controller-input-panel").is_some());
    assert!(
        visual
            .debug_bounds("controller-input-channel-popup")
            .is_none()
    );
    visual.simulate_mouse_down(position, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(position, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let option = visual
        .debug_bounds("controller-input-channel-option-15")
        .unwrap();
    let position = point(option.origin.x + px(10.0), option.origin.y + px(9.0));
    visual.simulate_mouse_down(position, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(position, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.controller_input.channel, 15);
        assert!(view.controller_input_is_open());
        assert!(
            view.controller_input.handle.is_none(),
            "menu/choices do not discover hardware"
        );
    });
    let port = visual.debug_bounds("controller-input-port").unwrap();
    let position = point(port.origin.x + px(10.0), port.origin.y + px(10.0));
    visual.simulate_mouse_down(position, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(position, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let off = visual
        .debug_bounds("controller-input-port-option-0")
        .unwrap();
    let position = point(off.origin.x + px(10.0), off.origin.y + px(9.0));
    visual.simulate_mouse_down(position, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(position, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        assert!(view.controller_input_is_open());
        assert_eq!(
            view.controller_input
                .port_selector
                .as_ref()
                .unwrap()
                .read(cx)
                .selected(),
            0
        );
        assert!(view.controller_input.selected_port.is_none());
        assert!(view.controller_input.handle.is_none());
    });
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert!(visual.debug_bounds("controller-input-panel").is_none());
    visual.update(|window, _| {
        if let Some(focus) = &previous_focus {
            assert!(focus.is_focused(window));
        }
    });
    open(&mut visual);
    visual.simulate_mouse_down(
        point(px(5.0), px(100.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_up(
        point(px(5.0), px(100.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert!(visual.debug_bounds("controller-input-panel").is_none());
    editor.read_with(cx, |view, _| assert!(view.last_enqueued_edit.is_none()));
}

#[cfg(all(test, feature = "ui-test-support"))]
#[gpui::test]
fn controller_input_streaming_guard_cancels_without_discovery_or_retry(
    cx: &mut gpui::TestAppContext,
) {
    use gpui::{AppContext, Empty};
    let mut project =
        hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 4).unwrap();
    let hex_project::Created::Track(track) = project
        .execute(hex_project::Command::InsertTrackWithDevice {
            name: "controller".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let instrument = project
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let input = Arc::new(LiveExpressionInput::new(0, track, instrument));
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service = hex_session::service::Service::new(48_000, 8, 8_000_000).unwrap();
            window.replace_root(cx, |window, cx| {
                let preferences = std::env::temp_dir()
                    .join(format!("hex-controller-input-test-{}", std::process::id()));
                EditorView::new_for_test_with_theme_preferences_path(
                    window,
                    cx,
                    service,
                    Err("test offline output".into()),
                    preferences.join("browser"),
                    preferences.join("recent"),
                    preferences.join("theme"),
                )
            })
        })
        .unwrap();
    editor.update(cx, |view, cx| {
        view.document_audio_mode = hex_session::DocumentAudioMode::VerifiedStreaming;
        view.controller_input.input = Some(Arc::clone(&input));
        view.controller_input.selected_port = Some(hex_midi::Port {
            id: 1,
            name: "synthetic".into(),
        });
        view.controller_input.target = Some((0, track, instrument));
        let requests = view.worker_requests_pending;
        view.refresh_controller_inputs(cx);
        assert!(
            view.controller_input.handle.is_none(),
            "streaming refusal must precede discovery/native worker lookup"
        );
        assert_eq!(
            view.error.as_deref(),
            Some("controller input is unavailable for verified streaming audio")
        );
        assert!(view.tick_controller_input(cx));
        assert!(input.is_cancelled());
        assert!(view.controller_input.selected_port.is_none());
        assert_eq!(view.worker_requests_pending, requests);
        let activation = view.controller_input.activation;
        assert!(!view.tick_controller_input(cx));
        assert_eq!(
            view.controller_input.activation, activation,
            "refusal must not run a repeated activation/cancellation loop"
        );
    });
}

#[cfg(all(test, feature = "ui-test-support"))]
#[gpui::test]
fn hardware_chord_step_uses_native_decoder_velocities_channel_ack_and_history(
    cx: &mut gpui::TestAppContext,
) {
    use super::midi_canvas::creation_tests::{fixture, pump};
    let (mut visual, editor, _environment, clip) = fixture(cx);
    let input = Arc::new(hex_midi::HeldNoteInput::default());
    editor.update(&mut visual, |view, cx| {
        view.midi_step.enabled = true;
        view.controller_input.channel = 2;
        view.controller_input.held_notes = Some(Arc::clone(&input));
        view.controller_input.target = view.controller_target();
        cx.notify();
    });
    visual.run_until_parked();
    let mut decoder = hex_midi::Decoder::default();
    let mut send = |bytes: &[u8]| {
        hex_midi::deliver_packets(
            &mut decoder,
            2,
            hex_midi::Controllers::default(),
            [bytes],
            &input,
            &|_| {},
        );
    };
    send(&[0x92, 60, 37, 64, 93]);
    for step in 0..2 {
        editor.update(&mut visual, |view, _| {
            view.tick_computer_keyboard();
        });
        visual.simulate_keystrokes("right");
        editor.read_with(&visual, |view, _| {
            assert_eq!(
                view.midi_marker,
                Some(step * hex_project::BeatTime::ONE.raw() / 4)
            )
        });
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            let source = view
                .state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap();
            assert_eq!(source.notes().len(), (step as usize + 1) * 2);
            let latest = &source.notes()[step as usize * 2..];
            assert_eq!(
                latest
                    .iter()
                    .map(|n| (n.pitch(), n.velocity(), n.channel()))
                    .collect::<Vec<_>>(),
                [(60, 37, 2), (64, 93, 2)]
            );
            assert_eq!(
                view.midi_marker,
                Some((step + 1) * hex_project::BeatTime::ONE.raw() / 4)
            );
        });
    }
    send(&[0x82, 60, 0, 64, 0]);
    visual.simulate_keystrokes("right");
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.midi_marker,
            Some(3 * hex_project::BeatTime::ONE.raw() / 4)
        );
        assert!(!view.edit_pending());
    });
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(hex_session::service::Request::Undo))
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .notes()
                .len(),
            2
        )
    });
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(hex_session::service::Request::Redo))
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .notes()
                .len(),
            4
        )
    });
}

#[cfg(all(test, feature = "ui-test-support"))]
#[gpui::test]
fn hardware_step_discontinuity_excludes_old_keys_and_cancel_refuses_false_rest(
    cx: &mut gpui::TestAppContext,
) {
    use super::midi_canvas::creation_tests::{fixture, pump};
    let (mut visual, editor, _environment, clip) = fixture(cx);
    let input = Arc::new(hex_midi::HeldNoteInput::default());
    editor.update(&mut visual, |view, cx| {
        view.midi_step.enabled = true;
        view.controller_input.held_notes = Some(Arc::clone(&input));
        view.controller_input.target = view.controller_target();
        cx.notify();
    });
    visual.run_until_parked();
    let mut decoder = hex_midi::Decoder::default();
    let mut send = |bytes: &[u8]| {
        hex_midi::deliver_packets(
            &mut decoder,
            0,
            hex_midi::Controllers::default(),
            [bytes],
            &input,
            &|_| {},
        );
    };
    send(&[0x90, 60, 37]);
    editor.update(&mut visual, |view, _| view.stop_computer_keyboard());
    send(&[64, 93]);
    visual.simulate_keystrokes("right");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        let source = view
            .state
            .as_ref()
            .unwrap()
            .project
            .midi_source(clip)
            .unwrap();
        assert_eq!(
            source.notes().iter().map(|n| n.pitch()).collect::<Vec<_>>(),
            [64]
        );
    });
    // A key pressed while unfocused cannot become a step after refocusing.
    visual.update(|window, cx| editor.update(cx, |view, _| view.focus.focus(window)));
    visual.run_until_parked();
    send(&[67, 70]);
    visual.update(|window, cx| editor.update(cx, |view, _| view.midi_focus.focus(window)));
    visual.run_until_parked();
    send(&[69, 80]);
    visual.simulate_keystrokes("right");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        let source = view
            .state
            .as_ref()
            .unwrap()
            .project
            .midi_source(clip)
            .unwrap();
        assert_eq!(
            source.notes().iter().map(|n| n.pitch()).collect::<Vec<_>>(),
            [64, 69]
        );
    });
    let marker = editor.read_with(&visual, |view, _| view.midi_marker);
    input.cancel();
    visual.simulate_keystrokes("right");
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.midi_marker, marker);
        assert!(!view.edit_pending());
        assert!(view.error.is_some());
    });
    editor.update(&mut visual, |view, cx| {
        assert!(view.tick_controller_input(cx))
    });
    visual.simulate_keystrokes("right");
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.midi_marker, marker,
            "retiring the failed endpoint must not turn its chord into a rest"
        );
        assert!(!view.edit_pending());
    });
}

#[cfg(all(test, feature = "ui-test-support"))]
#[gpui::test]
fn hardware_step_full_pitch_bank_combines_computer_priority_and_grows_once(
    cx: &mut gpui::TestAppContext,
) {
    use super::midi_canvas::creation_tests::{fixture, pump};
    use gpui::{KeyDownEvent, Keystroke};
    let (mut visual, editor, _environment, clip) = fixture(cx);
    let input = Arc::new(hex_midi::HeldNoteInput::default());
    editor.update(&mut visual, |view, cx| {
        view.midi_step.enabled = true;
        view.controller_input.channel = 2;
        view.controller_input.held_notes = Some(Arc::clone(&input));
        view.controller_input.target = view.controller_target();
        view.midi_marker = Some(8 * hex_project::BeatTime::ONE.raw());
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("m");
    visual.simulate_event(KeyDownEvent {
        keystroke: Keystroke::parse("a").unwrap(),
        is_held: false,
    });
    let bytes: Vec<_> = (0..128_u8).flat_map(|pitch| [0x92, pitch, 93]).collect();
    hex_midi::deliver_packets(
        &mut hex_midi::Decoder::default(),
        2,
        hex_midi::Controllers::default(),
        [bytes.as_slice()],
        &input,
        &|_| {},
    );
    visual.simulate_keystrokes("right");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        let source = view
            .state
            .as_ref()
            .unwrap()
            .project
            .midi_source(clip)
            .unwrap();
        assert_eq!(source.notes().len(), 128);
        for note in source.notes() {
            assert_eq!(note.start().raw(), 8 * hex_project::BeatTime::ONE.raw());
            assert_eq!(
                (note.velocity(), note.channel()),
                if note.pitch() == 60 {
                    (100, 0)
                } else {
                    (93, 2)
                }
            );
        }
        assert_eq!(
            source.source_length().raw(),
            8 * hex_project::BeatTime::ONE.raw() + hex_project::BeatTime::ONE.raw() / 4
        );
    });
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(hex_session::service::Request::Undo))
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .notes()
                .is_empty()
        )
    });
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(hex_session::service::Request::Redo))
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .notes()
                .len(),
            128
        )
    });
}

#[cfg(all(test, feature = "ui-test-support"))]
#[gpui::test]
fn hardware_step_refuses_old_instrument_before_port_reconciliation(cx: &mut gpui::TestAppContext) {
    use super::midi_canvas::creation_tests::{fixture, pump};
    let (mut visual, editor, _environment, clip) = fixture(cx);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(hex_session::service::Request::Edit(
            hex_project::Command::InsertInstrument {
                track: view.selected_track.unwrap(),
                state: hex_project::default_instrument_state().unwrap(),
            }
        )));
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |view, cx| {
        view.midi_step.enabled = true;
        view.controller_input.held_notes = Some(Arc::default());
        view.controller_input.target = view.controller_target();
        cx.notify();
    });
    visual.run_until_parked();
    editor.update(&mut visual, |view, _| {
        assert!(view.hardware_step_notes().is_ok());
        let device = view.controller_input.target.unwrap().2;
        assert!(view.enqueue(hex_session::service::Request::Edit(
            hex_project::Command::SetDeviceBypass {
                device,
                bypassed: true
            }
        )));
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(
            view.hardware_step_notes().is_err(),
            "a stale device lease cannot supply even an empty rest snapshot"
        );
    });
    let marker = editor.read_with(&visual, |view, _| view.midi_marker);
    visual.simulate_keystrokes("right");
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.midi_marker, marker);
        assert!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .notes()
                .is_empty()
        );
    });
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_pitch_feedback_ui_tests.rs"]
mod pitch_feedback_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[gpui::test]
fn midi_recording_reprepare_preserves_configured_hardware_when_replacing_cancelled_root(
    cx: &mut gpui::TestAppContext,
) {
    use super::midi_canvas::creation_tests::{fixture, pump};
    let (mut visual, editor, _environment, _) = fixture(cx);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(hex_session::service::Request::Edit(
            hex_project::Command::InsertInstrument {
                track: view.selected_track.unwrap(),
                state: hex_project::default_instrument_state().unwrap(),
            }
        )));
    });
    pump(&editor, &mut visual);
    for computer_enabled in [false, true] {
        editor.update(&mut visual, |view, cx| {
            let old = Arc::new(
                hex_engine::computer_keyboard::ComputerKeyboardInput::default()
                    .with_live_notes()
                    .unwrap(),
            );
            let token = old
                .live_notes()
                .unwrap()
                .current_activation_on_control()
                .unwrap();
            view.computer_keyboard.enabled = computer_enabled;
            view.computer_keyboard.input = Some(Arc::clone(&old));
            view.controller_input.selected_port = Some(hex_midi::Port {
                id: 42,
                name: "configured".into(),
            });
            view.controller_input.note_input = Some((Arc::clone(&old), token));
            view.controller_input.connected = true;
            view.reprepare_controller_for_recording();
            view.cancel_computer_keyboard();
            assert!(old.is_cancelled());
            assert!(view.controller_input.note_input.is_none());
            assert!(!view.controller_input.connected);
            assert_eq!(view.controller_input.selected_port.as_ref().unwrap().id, 42);
            view.tick_controller_input(cx);
            assert_eq!(view.controller_input.selected_port.as_ref().unwrap().id, 42);
            view.controller_input.cancel();
        });
        pump(&editor, &mut visual);
    }
}
