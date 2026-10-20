//! One local numeric draft; only commit reaches the canonical worker.
use super::*;
use gpui::{Entity, Subscription};
use hex_widgets::{
    NumericControl, NumericControlAppearance, NumericControlConfig, NumericControlEvent,
    NumericControlKind, NumericEventPhase, NumericScale, NumericSpec,
};

pub(crate) struct Binding {
    key: (ClipId, u64, u64),
    entity: Entity<NumericControl>,
    _subscription: Subscription,
}
impl EditorView {
    pub(crate) fn sample_bpm_editing(&self, cx: &Context<Self>) -> bool {
        self.sample_bpm_control.as_ref().is_some_and(|binding| {
            let selection_current = match self.selected_clip_data() {
                Ok(selection) => selection.is_some_and(|selection| selection.1 == binding.key.0),
                Err(_) => return binding.entity.read(cx).is_editing(),
            };
            let revision_current = match self.state.as_ref() {
                Some(state) => match state.read_with(|snapshot| snapshot.revision()) {
                    Ok(revision) => revision == binding.key.2,
                    Err(_) => return binding.entity.read(cx).is_editing(),
                },
                None => false,
            };
            selection_current
                && self.project_generation == binding.key.1
                && revision_current
                && binding.entity.read(cx).is_editing()
        })
    }
}
pub(super) fn field(view: &mut EditorView, id: ClipId, cx: &mut Context<EditorView>) -> Div {
    let Some((warped, value, revision)) = view.state.as_ref().and_then(|state| {
        state
            .read_with(|snapshot| {
                let project = snapshot.model();
                let clip = project.tracks().find_map(|track| track.clip(id))?;
                let edit = clip.audio_edit();
                let value = edit.and_then(|edit| edit.sample_bpm()).unwrap_or_else(|| {
                    let tempo = project.tempo();
                    tempo.tempo_at(tempo.frame_to_beat(clip.start()).unwrap_or(BeatTime::ZERO))
                });
                Some((
                    edit.is_some_and(|edit| edit.warp_enabled()),
                    value,
                    snapshot.revision(),
                ))
            })
            .ok()
            .flatten()
    }) else {
        return div();
    };
    let enabled = warped && !view.edit_pending();
    let value = f64::from(value.micro_bpm()) / 1_000_000.0;
    let key = (id, view.project_generation, revision);
    if view
        .sample_bpm_control
        .as_ref()
        .is_some_and(|binding| binding.key != key)
    {
        view.sample_bpm_control = None;
    }
    if view.sample_bpm_control.is_none() {
        let spec = NumericSpec::new(1.0, 1000.0, value, NumericScale::Linear, 6, Some("bpm")).map(
            |spec| {
                spec.with_codec(
                    |v| {
                        format!("{v:.6}")
                            .trim_end_matches('0')
                            .trim_end_matches('.')
                            .to_owned()
                    },
                    |s| s.trim().trim_end_matches("bpm").trim().parse::<f64>().ok(),
                )
            },
        );
        let config = spec.and_then(|spec| {
            NumericControlConfig::new(
                "audio-sample-bpm",
                theme::text("sample bpm"),
                NumericControlKind::HorizontalSlider,
                spec,
                value,
                super::super::device_panel::control_style(),
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
                if view.project_generation != key.1
                    || view.edit_pending()
                    || view
                        .state
                        .as_ref()
                        .is_none_or(|state| {
                            !state.read_with(|snapshot| snapshot.revision())
                                .is_ok_and(|revision| revision == key.2)
                        })
                    || !matches!(view.selected_clip_data(), Ok(Some(selection)) if selection.1 == id)
                {
                    return;
                }
                let Ok(tempo) = parse_bpm(event.value) else {
                    return;
                };
                view.enqueue(hex_session::service::Request::Edit(
                    hex_project::Command::SetAudioClipSampleBpm { clip: id, tempo },
                ));
                cx.notify();
            });
        view.sample_bpm_control = Some(Binding {
            key,
            entity,
            _subscription: subscription,
        });
    }
    let binding = view.sample_bpm_control.as_ref().unwrap();
    binding
        .entity
        .update(cx, |numeric, cx| numeric.set_enabled(enabled, cx));
    div()
        .flex()
        .h(px(20.0))
        .flex_shrink_0()
        .child(musical_control_label("sample bpm"))
        .child(binding.entity.clone())
        .debug_selector(|| "audio-sample-bpm-field".into())
}
fn parse_bpm(value: f64) -> Result<hex_project::Tempo, hex_project::Invalid> {
    if !value.is_finite() || !(1.0..=1000.0).contains(&value) {
        return Err(hex_project::Invalid(
            "sample bpm must be finite and within 1..1000",
        ));
    }
    hex_project::Tempo::new((value * 1_000_000.0).round() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bpm_uses_current_bounds_and_rejects_nonfinite_before_integer_conversion() {
        for value in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            0.0,
            -1.0,
            0.999,
            1000.001,
        ] {
            assert!(parse_bpm(value).is_err());
        }
        for value in [1.0, 60.0, 123.456789, 1000.0] {
            assert_eq!(
                parse_bpm(value).unwrap().micro_bpm(),
                (value * 1_000_000.0).round() as u32
            );
        }
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod ui_tests {
    use super::*;
    use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext};
    use hex_session::service::ViewState;
    use std::sync::Arc;
    #[gpui::test]
    fn sample_bpm_actual_text_input_commits_once_and_escape_cancels(cx: &mut TestAppContext) {
        let (handle, editor, _, clip) =
            super::super::super::clip_view_ui_test::audio_detail_fixture(cx);
        cx.update(hex_widgets::bind_numeric_control_keys);
        editor.update(cx, |view, cx| {
            let old = view.state.as_ref().unwrap();
            let mut model = hex_project::Editor::new((*old.project).clone(), 8).unwrap();
            model
                .execute(command(model.project(), clip, Action::Warp).unwrap())
                .unwrap();
            view.state = Some(Arc::new(ViewState {
                canonical_audio_source: None,
                project: hex_session::SharedProject::new_on_control(model.project().clone()),
                waveforms: old.waveforms.clone(),
                audio_clip_waveforms: old.audio_clip_waveforms.clone(),
                midi_preview: old.midi_preview.clone(),
                gain_preview: old.gain_preview.clone(),
                timeline_frames: old.timeline_frames,
                can_undo: true,
                can_redo: false,
                revision: old.revision + 1,
                unsaved: true,
                document_path: None,
            }));
            cx.notify();
        });
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let bounds = visual
            .debug_bounds("audio-sample-bpm-value")
            .expect("visible numeric value");
        visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input("nan");
        visual.simulate_keystrokes("enter");
        editor.read_with(cx, |view, _| assert!(view.last_enqueued_edit.is_none()));
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input("60");
        visual.simulate_keystrokes("escape");
        editor.read_with(cx, |view, _| assert!(view.last_enqueued_edit.is_none()));
        visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input("60");
        editor.read_with(cx, |view, _| {
            assert!(
                view.last_enqueued_edit.is_none(),
                "draft does not reach worker"
            )
        });
        visual.simulate_keystrokes("enter");
        editor.read_with(cx, |view, _| assert!(matches!(view.last_enqueued_edit, Some(hex_project::Command::SetAudioClipSampleBpm { clip: id, tempo }) if id == clip && tempo.micro_bpm() == 60_000_000)));
    }
}
