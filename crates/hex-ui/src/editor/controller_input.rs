//! Optional controller selection; no hardware notes, recording, sustain or MPE.
use super::EditorView;
use crate::theme;
use gpui::{AppContext, Context, Div, Entity, div, prelude::*, px};
use hex_engine::live_expression::LiveExpressionInput;
use hex_widgets::{Selector, SelectorChanged, SelectorConfig};
use std::sync::Arc;

#[derive(Default)]
pub(super) struct ControllerInput {
    handle: Option<hex_midi::Handle>,
    ports: Vec<hex_midi::Port>,
    port_selector: Option<Entity<Selector>>,
    channel_selector: Option<Entity<Selector>>,
    subscriptions: Vec<gpui::Subscription>,
    selected_port: Option<hex_midi::Port>,
    channel: u8,
    target: Option<(u64, hex_project::TrackId, hex_project::DeviceId)>,
    input: Option<Arc<LiveExpressionInput>>,
    connected: bool,
    activation: u64,
    discovery: u64,
    blocked_for_streaming: bool,
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
        match result {
            Ok(connected) => {
                self.connected = connected;
                None
            }
            Err(error) => {
                self.cancel();
                self.selected_port = None;
                Some(error)
            }
        }
    }
    fn cancel(&mut self) {
        if let Some(input) = self.input.take() {
            input.cancel();
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
        self.controller_input.cancel();
        self.controller_input.selected_port = index
            .checked_sub(1)
            .and_then(|index| self.controller_input.ports.get(index))
            .cloned();
        self.tick_controller_input(cx);
        cx.notify();
    }
    fn install_controller_selectors(&mut self, ports: Vec<hex_midi::Port>, cx: &mut Context<Self>) {
        self.controller_input.cancel();
        self.controller_input.selected_port = None;
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
            0,
            super::device_panel::control_style(),
        );
        let channel_config = SelectorConfig::new(
            "controller-input-channel",
            (1..=16).map(|channel| theme::text(format!("channel {channel}"))),
            usize::from(self.controller_input.channel),
            super::device_panel::control_style(),
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
                view.controller_input.cancel();
                view.controller_input.channel = event.index as u8;
                view.tick_controller_input(cx);
                cx.notify();
            });
        self.controller_input
            .subscriptions
            .extend([port_subscription, channel_subscription]);
        self.controller_input.port_selector = Some(port);
        self.controller_input.channel_selector = Some(channel);
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
                            Err(error) => self.error = Some(theme::text(error).to_string()),
                        }
                        changed = true;
                    }
                }
                Some(hex_midi::Event::Connection { activation, result }) => {
                    if let Some(error) = self.controller_input.connection_event(activation, result)
                    {
                        self.error = Some(theme::text(error).to_string());
                    }
                    changed = true;
                }
                None => break,
            }
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
        let target = self.state.as_ref().and_then(|state| {
            let track = self.selected_track?;
            let instrument = state
                .project
                .track(track)?
                .instrument()
                .filter(|instrument| !instrument.bypassed())?;
            Some((self.project_generation, track, instrument.id()))
        });
        if target != self.controller_input.target {
            self.controller_input.cancel();
            let Some((generation, track, instrument)) = target else {
                return true;
            };
            if self.controller_input.activation == u64::MAX {
                self.error = Some("controller activation identity exhausted".into());
                return true;
            }
            let input = Arc::new(LiveExpressionInput::new(generation, track, instrument));
            let revision = self.state.as_ref().map_or(0, |state| state.revision);
            if !self.enqueue(hex_session::service::Request::PrepareLiveExpression {
                input: Arc::clone(&input),
                revision,
            }) {
                input.cancel();
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
        changed
    }
    pub(super) fn controller_input_matches(&self, input: &Arc<LiveExpressionInput>) -> bool {
        let current_target = self.state.as_ref().and_then(|state| {
            let track = self.selected_track?;
            let instrument = state
                .project
                .track(track)?
                .instrument()
                .filter(|instrument| !instrument.bypassed())?;
            Some((self.project_generation, track, instrument.id()))
        });
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
        let Some(input) = self
            .controller_input
            .input
            .as_ref()
            .filter(|input| !input.is_cancelled())
        else {
            return;
        };
        let Some(port) = self.controller_input.selected_port.clone() else {
            return;
        };
        let Some(handle) = &self.controller_input.handle else {
            return;
        };
        let writer = Arc::clone(input);
        let cancel = Arc::clone(input);
        let cancelled = Arc::clone(input);
        let sink = hex_midi::Sink {
            publish: Box::new(move |value| {
                writer.publish(hex_engine::live_expression::Snapshot {
                    pressure: value.pressure,
                    bend: value.bend,
                    wheel: value.wheel,
                })
            }),
            cancel: Box::new(move || cancel.cancel()),
            is_cancelled: Box::new(move || cancelled.is_cancelled()),
        };
        if let Err(error) = handle.send(hex_midi::Command::Connect {
            activation: self.controller_input.activation,
            port,
            channel: self.controller_input.channel,
            sink,
        }) {
            self.error = Some(error.into());
        }
    }
    pub(super) fn controller_input_row(&self, cx: &mut Context<Self>) -> Div {
        div().h(px(26.0)).flex().gap(px(8.0)).items_center().px(px(8.0))
            .child(div().id("controller-input-refresh").cursor_pointer().child(theme::text("controller input ↻"))
                .tooltip(|_, cx| cx.new(|_| super::transport::MiniTooltip { description: "pressure, pitch bend and mod wheel for the selected instrument · click to discover ports · hardware notes, sustain, recording and mpe are not included".into() }).into())
                .on_click(cx.listener(|view, _, _, cx| view.refresh_controller_inputs(cx))))
            .children(self.controller_input.port_selector.clone())
            .children(self.controller_input.channel_selector.clone())
            .child(theme::text(if self.controller_input.connected { "connected" } else { "off" }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn delayed_connection_a_results_cannot_cancel_or_mark_connection_b() {
        let mut editor =
            hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 4).unwrap();
        let hex_project::Created::Track(track) = editor
            .execute(hex_project::Command::AddMidiTrack {
                name: "controller".into(),
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
        assert_eq!(
            state.connection_event(1, Err("delayed a failure".into())),
            None
        );
        assert!(!input.is_cancelled());
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
        assert!(!state.connected);
    }
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
        .execute(hex_project::Command::AddMidiTrack {
            name: "controller".into(),
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
