//! One transient input policy for browser insertion and MIDI takes. Grid geometry
//! is captured before worker preparation; canonical note authoring stays in Session.
use super::{EditorView, header, transport::MiniTooltip};
use crate::theme;
use gpui::{Context, Div, Window, prelude::*, px};
use hex_project::{InputQuantization, ResolvedInputGrid};
impl EditorView {
    pub(super) fn resolve_input_grid(&self) -> Result<ResolvedInputGrid, String> {
        let state = self.state.as_ref().ok_or("input grid requires a project")?;
        let project = &state.project;
        match self.input_quantization {
            InputQuantization::Off => Ok(ResolvedInputGrid::off()),
            InputQuantization::Adaptive => {
                let lane = self
                    .main_track_lane_bounds
                    .filter(|lane| lane.width.is_finite() && lane.width > 0.0)
                    .filter(|_| self.main_track_lane_epoch == self.drag_layout_generation)
                    .ok_or("input grid layout is not ready")?;
                self.snap_grid
                    .freeze_input_grid(
                        project,
                        self.visible_viewport(state.timeline_frames),
                        lane.width,
                    )
                    .map_err(str::to_owned)
            }
            fixed => ResolvedInputGrid::fixed(
                project.meter_map(),
                fixed.fixed_step().ok_or("invalid input subdivision")?,
            )
            .map_err(|reason| reason.to_string()),
        }
    }
    pub(super) fn set_input_quantization(&mut self, policy: InputQuantization) {
        if self.midi_recorder.blocks_edits() {
            self.error =
                Some("finish or discard the midi take before changing input quantization".into());
            return;
        }
        self.input_quantization = policy;
        self.status = format!("input quantization · {}", policy.label());
    }
    pub(super) fn toggle_input_quantization(&mut self) {
        self.set_input_quantization(if self.input_quantization == InputQuantization::Off {
            InputQuantization::Adaptive
        } else {
            InputQuantization::Off
        });
    }
    pub(super) fn input_quantization_control(&self, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        hex_widgets::action(
            theme::text(format!("in {} ▾", self.input_quantization.label())),
            theme::action_style(),
        )
        .id("input-quantization")
        .debug_selector(|| "input-quantization".into())
        .h(px(header::CONTROL_SIZE))
        .px_1()
        .text_xs()
        .tooltip(|_, cx| {
            cx.new(|_| MiniTooltip {
                kind: crate::tooltips::Kind::control("input quantization"),
                description:
                    "input quantization · recorded note starts and browser enter insertion".into(),
            })
            .into()
        })
        .on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(|view, event: &gpui::MouseDownEvent, window, cx| {
                view.open_input_quantization_menu(event.position, window, cx);
                cx.stop_propagation();
            }),
        )
    }
    fn open_input_quantization_menu(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.midi_recorder.blocks_edits() {
            self.error =
                Some("finish or discard the midi take before changing input quantization".into());
            cx.notify();
            return;
        }
        let focus = cx.focus_handle();
        let previous_focus = window.focused(cx);
        focus.focus(window);
        self.context_menu = Some(crate::context_menu::PopupMenu {
            scroll: gpui::ScrollHandle::new(),
            position,
            icon_grid: None,
            selected: InputQuantization::CHOICES
                .iter()
                .position(|p| *p == self.input_quantization)
                .unwrap_or(0),
            focus,
            previous_focus,
            items: InputQuantization::CHOICES
                .into_iter()
                .map(|policy| crate::context_menu::MenuItem {
                    label: policy.label(),
                    swatch: None,
                    checked: policy == self.input_quantization,
                    action: super::context_menu::EditorContextAction::InputQuantization {
                        generation: self.project_generation,
                        policy,
                    },
                })
                .collect(),
        });
        cx.notify();
    }
}
