//! Local global-tempo draft; one commit replaces the canonical project tempo map.
use super::EditorView;
use crate::theme;
use gpui::{Context, Div, Entity, Subscription, div, prelude::*, px};
use hex_project::{BeatTime, Command};
use hex_session::service::Request;
use hex_widgets::{
    NumericControl, NumericControlConfig, NumericControlEvent, NumericControlKind,
    NumericControlPresentation, NumericEventPhase, NumericScale, NumericSpec,
};

mod value;
use value::tempo_value;

pub(super) struct Binding {
    generation: u64,
    revision: u64,
    entity: Entity<NumericControl>,
    _subscription: Subscription,
    _observation: Subscription,
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
        let generation = self.project_generation;
        let revision = state.revision;
        // Generation/revision fence the draft. Cursor/playback position never
        // changes the displayed global tempo or targets a new tempo point.
        let replace = self.project_tempo_control.as_ref().is_some_and(|binding| {
            binding.generation != generation || binding.revision != revision
        });
        if replace && let Some(binding) = self.project_tempo_control.take() {
            binding.entity.update(cx, |numeric, cx| numeric.cancel(cx));
        }
        let tempo = state.project.tempo_map().tempo_at(BeatTime::ZERO);
        let value = f64::from(tempo.micro_bpm()) / 1_000_000.0;
        let enabled = !self.edit_pending();
        let replaces_points = state.project.tempo_map().points().len() > 1;
        if self.project_tempo_control.is_none() {
            let spec = NumericSpec::new(1.0, 1000.0, 120.0, NumericScale::Linear, 6, Some("bpm"))
                .and_then(|spec| spec.with_keyboard_steps(1.0, 0.1))
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
                    theme::text("project tempo · bpm"),
                    NumericControlKind::HorizontalSlider,
                    spec,
                    value,
                    theme::control_style(),
                )
            });
            let Ok(config) = config else { return div() };
            let entity = cx.new(|cx| {
                NumericControl::from_config(
                    cx,
                    config
                        .with_command_fine_steps()
                        .with_presentation(NumericControlPresentation::InlineValue),
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
                    if view.state.as_ref().is_some_and(|state| {
                        state.project.tempo_map().points().len() == 1
                            && state.project.tempo_map().tempo_at(BeatTime::ZERO) == tempo
                    }) {
                        return;
                    }
                    view.enqueue(Request::Edit(Command::SetProjectTempo { tempo }));
                    cx.notify();
                });
            let observation = cx.observe(&entity, |_, _, cx| cx.notify());
            self.project_tempo_control = Some(Binding {
                generation,
                revision,
                entity,
                _subscription: subscription,
                _observation: observation,
            });
        }
        let Some(binding) = self.project_tempo_control.as_ref() else {
            return div();
        };
        binding.entity.update(cx, |numeric, cx| {
            numeric.set_enabled(enabled, cx);
            numeric.set_commit_unchanged_text(replaces_points);
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

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    #[gpui::test]
    fn project_tempo_same_value_explicit_commit_collapses_later_points_and_cursor_is_display_only(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::editor::{bind_editor_keys, clip_drag_ui_test, tempo_input_ui_test::begin};
        use gpui::VisualTestContext;
        cx.update(bind_editor_keys);
        let mut model =
            hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
        model
            .execute(hex_project::Command::SetTempoPoint {
                beat: hex_project::BeatTime::from_quarters(2).unwrap(),
                tempo: hex_project::Tempo::new(60_000_000).unwrap(),
            })
            .unwrap();
        let (handle, editor) =
            clip_drag_ui_test::editor_window(cx, model.project().clone(), Default::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        for cursor in [0, 48_000, 96_000] {
            editor.update(cx, |view, cx| {
                view.offline_cursor = cursor;
                cx.notify();
            });
            visual.run_until_parked();
            editor.read_with(cx, |view, cx| {
                assert_eq!(
                    view.project_tempo_control
                        .as_ref()
                        .unwrap()
                        .entity
                        .read(cx)
                        .value(),
                    120.0
                )
            });
        }
        begin(&mut visual);
        visual.simulate_input("120");
        visual.simulate_keystrokes("escape");
        editor.read_with(cx, |view, _| assert!(view.last_enqueued_edit.is_none()));
        begin(&mut visual);
        visual.simulate_input("120");
        visual.simulate_keystrokes("enter");
        let command = editor.read_with(cx, |view, _| view.last_enqueued_edit.clone().expect("explicit global commit must clear later points even when the initial BPM is unchanged"));
        model.execute(command).unwrap();
        assert_eq!(model.project().tempo_map().points().len(), 1);
        assert_eq!(
            model
                .project()
                .tempo_map()
                .tempo_at(hex_project::BeatTime::from_quarters(8).unwrap())
                .micro_bpm(),
            120_000_000
        );
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod keyboard_tests;
