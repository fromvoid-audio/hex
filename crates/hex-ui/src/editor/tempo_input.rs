//! Local tempo draft; one commit uses the existing canonical tempo-point command.
use super::EditorView;
use crate::theme;
use gpui::{Context, Div, Entity, Subscription, div, prelude::*, px};
use hex_project::{BeatTime, Command, Tempo};
use hex_session::service::Request;
use hex_widgets::{
    NumericControl, NumericControlAppearance, NumericControlConfig, NumericControlEvent,
    NumericControlKind, NumericEventPhase, NumericScale, NumericSpec,
};

pub(super) struct Binding {
    generation: u64,
    revision: u64,
    beat: BeatTime,
    entity: Entity<NumericControl>,
    _subscription: Subscription,
    _observation: Subscription,
}

fn tempo_value(value: f64) -> Option<Tempo> {
    if !value.is_finite() || !(1.0..=1000.0).contains(&value) {
        return None;
    }
    Tempo::new((value * 1_000_000.0).round() as u32).ok()
}

impl EditorView {
    pub(super) fn project_tempo_editing(&self, cx: &Context<Self>) -> bool {
        self.project_tempo_control.as_ref().is_some_and(|binding| {
            binding.generation == self.project_generation
                && self
                    .state
                    .as_ref()
                    .is_some_and(|state| state.revision == binding.revision)
                && binding.entity.read(cx).is_editing()
        })
    }

    pub(super) fn project_tempo_input(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(state) = self.state.as_ref() else {
            return div();
        };
        let Some(cursor_beat) = self.cursor_beat() else {
            return div();
        };
        let generation = self.project_generation;
        let revision = state.revision;
        // Playback may move the cursor while typing. Retain the originally
        // displayed tempo point until this draft ends, never retarget its write.
        let replace = self.project_tempo_control.as_ref().is_some_and(|binding| {
            binding.generation != generation
                || binding.revision != revision
                || (binding.beat != cursor_beat && !binding.entity.read(cx).is_interacting())
        });
        if replace && let Some(binding) = self.project_tempo_control.take() {
            binding.entity.update(cx, |numeric, cx| numeric.cancel(cx));
        }
        let beat = self
            .project_tempo_control
            .as_ref()
            .map_or(cursor_beat, |binding| binding.beat);
        let tempo = state.project.tempo_map().tempo_at(beat);
        let value = f64::from(tempo.micro_bpm()) / 1_000_000.0;
        let enabled = !self.edit_pending();
        if self.project_tempo_control.is_none() {
            let spec = NumericSpec::new(1.0, 1000.0, 120.0, NumericScale::Linear, 6, Some("bpm"))
                .map(|spec| {
                    spec.with_codec(
                        |value| {
                            format!("{value:.6}")
                                .trim_end_matches('0')
                                .trim_end_matches('.')
                                .to_owned()
                        },
                        |text| text.trim().parse::<f64>().ok(),
                    )
                });
            let config = spec.and_then(|spec| {
                NumericControlConfig::new(
                    "project-tempo",
                    theme::text("project tempo at cursor quarter · bpm"),
                    NumericControlKind::HorizontalSlider,
                    spec,
                    value,
                    super::device_panel::control_style(),
                )
            });
            let Ok(config) = config else { return div() };
            let entity = cx.new(|cx| {
                NumericControl::from_config(
                    cx,
                    config.with_appearance(NumericControlAppearance::InlineValue),
                )
            });
            let subscription =
                cx.subscribe(&entity, move |view, _, event: &NumericControlEvent, cx| {
                    if event.phase != NumericEventPhase::Commit {
                        return;
                    }
                    if view.project_generation != generation
                        || view.edit_pending()
                        || view
                            .state
                            .as_ref()
                            .is_none_or(|state| state.revision != revision)
                    {
                        return;
                    }
                    let Some(tempo) = tempo_value(event.value) else {
                        return;
                    };
                    // A sub-micro-BPM edit can round back to the same canonical value.
                    if view
                        .state
                        .as_ref()
                        .is_some_and(|state| state.project.tempo_map().tempo_at(beat) == tempo)
                    {
                        return;
                    }
                    view.enqueue(Request::Edit(Command::SetTempoPoint { beat, tempo }));
                    cx.notify();
                });
            let observation = cx.observe(&entity, |_, _, cx| cx.notify());
            self.project_tempo_control = Some(Binding {
                generation,
                revision,
                beat,
                entity,
                _subscription: subscription,
                _observation: observation,
            });
        }
        let binding = self.project_tempo_control.as_ref().unwrap();
        binding.entity.update(cx, |numeric, cx| {
            numeric.set_enabled(enabled, cx);
            let _ = numeric.set_external_value(value, cx);
        });
        div()
            .debug_selector(|| "project-tempo-field".into())
            .h_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(2.0))
            .px(px(4.0))
            .child(binding.entity.clone())
            .child(div().text_xs().child(theme::text("bpm")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn project_tempo_range_and_micro_bpm_precision() {
        for value in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            0.0,
            -1.0,
            0.999999,
            1000.000001,
        ] {
            assert!(tempo_value(value).is_none());
        }
        for (value, micro) in [
            (1.0, 1_000_000),
            (123.456789, 123_456_789),
            (1000.0, 1_000_000_000),
        ] {
            assert_eq!(tempo_value(value).unwrap().micro_bpm(), micro);
        }
    }
}
