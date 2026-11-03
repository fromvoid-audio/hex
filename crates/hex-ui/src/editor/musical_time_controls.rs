//! Editor controls for arrangement snap density and meter.
use super::musical_time::AdaptiveGridDensity;
use crate::theme;
use gpui::{Context, Div, Window, div, prelude::*, px};
use hex_project::{BeatTime, Command, Meter};
use hex_session::service::Request;

impl super::EditorView {
    /// Context-menu and keyboard density share the arranger resolver. Piano-roll
    /// MidiGrid remains its independent fixed-note policy. No captured edit is changed.
    pub(super) fn set_adaptive_grid_density(&mut self, density: AdaptiveGridDensity) {
        self.snap_grid = self.snap_grid.with_density(density);
        self.status = format!("arranger · {}", density.label());
    }

    pub(super) fn step_adaptive_grid_density(&mut self, coarser: bool) {
        if self.mode == super::EditorMode::Arrangement {
            self.set_adaptive_grid_density(self.snap_grid.density().stepped(coarser));
        }
    }

    pub(super) fn toggle_snap_grid(&mut self) {
        self.snap_grid = self.snap_grid.toggled();
        self.status = format!("arranger · snap {}", self.snap_grid.label());
    }

    pub(super) fn cursor_beat(&self) -> Option<BeatTime> {
        let state = self.state.as_ref()?;
        let beat = state
            .project
            .tempo_map()
            .frame_to_beat(self.cursor_position())
            .ok()?;
        beat.snap_to(BeatTime::ONE).ok()
    }

    fn time_edit_admitted(&self) -> bool {
        !self.opening_project
            && !self.close_prompt_open
            && self.worker_requests_pending == 0
            && self.pending_requests.is_empty()
    }

    pub(super) fn cycle_meter(&mut self) {
        if !self.time_edit_admitted() {
            self.error = Some("meter edit waits for pending project work".into());
            return;
        }
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let Some(beat) = self.cursor_beat() else {
            self.error = Some("meter point unavailable at this cursor position".into());
            return;
        };
        let current = state.project.meter_map().meter_at(beat);
        let signature = match (current.numerator(), current.denominator()) {
            (4, 4) => (3, 4),
            (3, 4) => (6, 8),
            (6, 8) => (5, 4),
            _ => (4, 4),
        };
        let Ok(meter) = Meter::new(signature.0, signature.1) else {
            self.error = Some("meter point unavailable".into());
            return;
        };
        self.enqueue(Request::Edit(Command::SetMeterPoint { beat, meter }));
    }

    pub(super) fn time_controls(&self, cx: &mut Context<Self>) -> Div {
        let Some(state) = self.state.as_ref() else {
            return div().w(px(self.track_header_width())).flex_shrink_0();
        };
        let beat = self.cursor_beat();
        let meter_label = beat.map_or_else(
            || "—".into(),
            |beat| {
                let meter = state.project.meter_map().meter_at(beat);
                format!("{}/{}", meter.numerator(), meter.denominator())
            },
        );
        let enabled = self.time_edit_admitted() && beat.is_some();
        div()
            .w(px(self.track_header_width()))
            .h_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(2.0))
            .children([self.time_button(
                &meter_label,
                "cycle meter at cursor snapped quarter · 4/4, 3/4, 6/8, 5/4",
                enabled,
                |view, _, _| view.cycle_meter(),
                cx,
            )])
    }

    fn time_button(
        &self,
        label: &str,
        description: &'static str,
        enabled: bool,
        handler: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        hex_widgets::action(theme::text(label), theme::action_style())
            .id(description)
            .w(px(20.0))
            .h(px(19.0))
            .text_xs()
            .opacity(if enabled { 1.0 } else { 0.45 })
            .tooltip(move |_, cx| {
                cx.new(|_| super::transport::MiniTooltip {
                    kind: crate::tooltips::Kind::control("cycle meter at cursor"),
                    description: description.into(),
                })
                .into()
            })
            .on_click(cx.listener(move |view, _, window, cx| {
                handler(view, window, cx);
                cx.notify();
            }))
    }
}
