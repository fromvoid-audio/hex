//! Shared numeric/toggle bindings. Drafts are local; one commit uses canonical
//! commands. Released values remain with their exact ordinary-edit receipt until
//! its terminal and canonical state are installed together by receive. Pitch
//! uses the same receipt with a signed-cent dial and the musical action owner.
use super::*;
use crate::editor::pending_edit::{Disposition, Receipt};
use gpui::{Entity, FocusHandle, Focusable, Subscription, Window};
use hex_session::service::{Event, Request};
use hex_widgets::{
    NumericControl, NumericControlConfig, NumericControlEvent, NumericControlKind,
    NumericControlPresentation, NumericEventPhase, NumericScale, NumericSpec, Toggle,
    ToggleAppearance, ToggleChanged, ToggleConfig,
};
use std::collections::BTreeMap;

#[derive(Default)]
pub(crate) struct Controls {
    key: Option<(ClipId, u64, u64)>,
    capture: Option<ClipWindow>,
    gain_preview: Option<(ClipWindow, hex_project::ClipGain)>,
    numbers: BTreeMap<Field, Number>,
    toggles: BTreeMap<bool, Switch>,
    pending: Option<Pending>,
    retired_focus: Vec<RetiredFocus>,
}
#[derive(Clone, Copy)]
enum Target {
    Number(Field),
    Switch(bool),
    Bpm,
}
struct RetiredFocus {
    focus: FocusHandle,
    target: Target,
    key: Option<(ClipId, u64, u64)>,
}
struct Number {
    entity: Entity<NumericControl>,
    focus: FocusHandle,
    _subscription: Subscription,
}
struct Switch {
    entity: Entity<Toggle>,
    focus: FocusHandle,
    _subscription: Subscription,
}
struct Pending {
    command: Command,
    receipt: Receipt,
}
impl Controls {
    pub(crate) fn editing(&self, cx: &gpui::App) -> bool {
        self.numbers
            .values()
            .any(|n| n.entity.read(cx).is_interacting())
    }
    pub(crate) fn reset(&mut self) -> bool {
        let changed = self.key.is_some() || self.pending.is_some();
        self.retire_bindings();
        let retired_focus = std::mem::take(&mut self.retired_focus);
        *self = Self {
            retired_focus,
            ..Self::default()
        };
        changed
    }
    fn retire_bindings(&mut self) {
        self.gain_preview = None;
        self.retired_focus
            .extend(self.numbers.iter().map(|(field, n)| RetiredFocus {
                focus: n.focus.clone(),
                target: Target::Number(*field),
                key: self.key,
            }));
        self.retired_focus
            .extend(self.toggles.iter().map(|(warp, t)| RetiredFocus {
                focus: t.focus.clone(),
                target: Target::Switch(*warp),
                key: self.key,
            }));
        self.numbers.clear();
        self.toggles.clear();
    }
    pub(crate) fn matches(&self, request: &Request) -> bool {
        let Some(p) = self.pending.as_ref().filter(|p| p.receipt.is_queued()) else {
            return false;
        };
        let Request::Edit(command) = request else {
            return false;
        };
        same_command(&p.command, command)
    }
    pub(crate) fn submitted(&mut self, count: usize) {
        if let Some(p) = &mut self.pending {
            p.receipt.submitted(count);
        }
    }
    pub(crate) fn observe(&mut self, generation: u64, event: &Event) -> bool {
        if self.pending.as_mut().is_some_and(|p| {
            p.receipt
                .observe(generation, event.project_replaced, event.request_complete)
                != Disposition::Waiting
        }) || event.project_replaced
        {
            return self.reset();
        }
        false
    }
    pub(crate) fn has_pending(&self) -> bool {
        self.pending.is_some()
    }
    pub(crate) fn has_gain_preview(&self) -> bool {
        self.gain_preview.is_some()
    }
    pub(in crate::editor) fn gain_preview(
        &self,
        view: &EditorView,
    ) -> Option<(ClipId, hex_project::ClipGain, hex_project::ClipGain)> {
        let (window, gain) = self.gain_preview?;
        clip_window_is_current(view, window).then_some((window.clip, window.gain, gain))
    }
}
fn same_command(a: &Command, b: &Command) -> bool {
    match (a, b) {
        (
            Command::MoveClip {
                clip: a,
                track: at,
                start: as_,
            },
            Command::MoveClip {
                clip: b,
                track: bt,
                start: bs,
            },
        ) => (a, at, as_) == (b, bt, bs),
        (
            Command::TrimClip {
                clip: a,
                start: as_,
                duration: ad,
                source_offset: ao,
                source_length: al,
            },
            Command::TrimClip {
                clip: b,
                start: bs,
                duration: bd,
                source_offset: bo,
                source_length: bl,
            },
        ) => (a, as_, ad, ao, al) == (b, bs, bd, bo, bl),
        (
            Command::ResizeAudioClipView {
                clip: a,
                operation: ao,
            },
            Command::ResizeAudioClipView {
                clip: b,
                operation: bo,
            },
        ) => (a, ao) == (b, bo),
        (
            Command::SetClipSourcePhase { clip: a, phase: ap },
            Command::SetClipSourcePhase { clip: b, phase: bp },
        ) => (a, ap) == (b, bp),
        (
            Command::SetAudioClipPitch { clip: a, cents: av },
            Command::SetAudioClipPitch { clip: b, cents: bv },
        ) => (a, av) == (b, bv),
        (
            Command::SetAudioClipGain {
                clip: a,
                millidb: av,
            },
            Command::SetAudioClipGain {
                clip: b,
                millidb: bv,
            },
        ) => (a, av) == (b, bv),
        (
            Command::SetClipLooping {
                clip: a,
                looping: av,
            },
            Command::SetClipLooping {
                clip: b,
                looping: bv,
            },
        ) => (a, av) == (b, bv),
        (
            Command::SetAudioClipWarp {
                clip: a,
                settings: av,
            },
            Command::SetAudioClipWarp {
                clip: b,
                settings: bv,
            },
        ) => (a, av) == (b, bv),
        (
            Command::SetAudioClipSampleBpm { clip: a, tempo: av },
            Command::SetAudioClipSampleBpm { clip: b, tempo: bv },
        ) => (a, av) == (b, bv),
        (Command::EditClips { clips: a, edit: av }, Command::EditClips { clips: b, edit: bv }) => {
            (a, av) == (b, bv)
        }
        _ => false,
    }
}
fn sync(view: &mut EditorView, window: ClipWindow) {
    let key = (window.clip, window.generation, window.revision);
    if view.audio_clip_controls.capture != Some(window)
        && (view.audio_clip_controls.pending.is_none()
            || view
                .audio_clip_controls
                .key
                .is_none_or(|old| old.0 != key.0 || old.1 != key.1))
    {
        view.audio_clip_controls.retire_bindings();
        view.audio_clip_controls.key = Some(key);
        view.audio_clip_controls.capture = Some(window);
    }
}
fn submit(view: &mut EditorView, window: ClipWindow, command: Command) {
    if view.edit_pending() || !clip_window_is_current(view, window) {
        return;
    }
    queue(view, window.generation, command);
}
pub(crate) fn queue(view: &mut EditorView, generation: u64, command: Command) {
    // Freeze the canonical selection scope once before installing the matcher.
    let request = match view.resolve_clip_action_scope(Request::Edit(command), false) {
        Ok(request) => request,
        Err(error) => {
            view.error = Some(error.0.into());
            view.retire_clip_detail();
            return;
        }
    };
    let Request::Edit(command) = &request else {
        return;
    };
    view.audio_clip_controls.pending = Some(Pending {
        command: command.clone(),
        receipt: Receipt::new(generation),
    });
    if !view.enqueue(request) {
        view.retire_clip_detail();
    }
}

// f64 represents every integer through 2^53 exactly. Refuse larger frame fields
// rather than rounding a saved u64 coordinate into another authored position.
const MAX_EXACT_FRAME: u64 = (1_u64 << 53) - 1;
fn frame_value(value: f64) -> Option<u64> {
    (value.is_finite() && (0.0..=MAX_EXACT_FRAME as f64).contains(&value))
        .then(|| value.round() as u64)
}
fn value(window: ClipWindow, field: Field) -> u64 {
    match field {
        Field::Position => window.position,
        Field::Length => window.length,
        Field::SourceStart => window.source_start,
        Field::SourceEnd => window.source_end,
        Field::PlayStart => window.source_start.saturating_add(window.source_phase),
        Field::Gain | Field::Looping | Field::Pitch => 0,
    }
}
fn absolute_command(window: ClipWindow, field: Field, v: f64, multiple: bool) -> Option<Command> {
    if field == Field::Gain {
        if !v.is_finite() {
            return None;
        }
        let millidb = (v * 1000.0).round();
        if !(f64::from(hex_project::ClipGain::MIN_MILLIDB)
            ..=f64::from(hex_project::ClipGain::MAX_MILLIDB))
            .contains(&millidb)
        {
            return None;
        }
        let millidb = millidb as i32;
        return (millidb != window.gain.millidb()).then(|| {
            if multiple {
                Command::EditClips {
                    clips: vec![window.clip],
                    edit: hex_project::ClipEdit::GainBy {
                        millidb: millidb - window.gain.millidb(),
                    },
                }
            } else {
                Command::SetAudioClipGain {
                    clip: window.clip,
                    millidb,
                }
            }
        });
    }
    let desired = frame_value(v)?;
    let old = value(window, field);
    if desired == old {
        return None;
    }
    command_for(
        window,
        field,
        if desired > old { 1 } else { -1 },
        desired.abs_diff(old),
    )
}
fn spec(window: ClipWindow, field: Field, horizon: u64) -> Option<NumericSpec> {
    if field == Field::Pitch {
        return NumericSpec::new(-4800., 4800., 0., NumericScale::Linear, 0, Some("ct"))
            .ok()?
            .with_integer_values()
            .ok()?
            .with_keyboard_steps(100., 1.)
            .ok()
            .map(|spec| {
                spec.with_codec(
                    |v| format!("{v:+.0} ct"),
                    |s| s.trim().trim_end_matches("ct").trim().parse::<f64>().ok(),
                )
            });
    }
    if field == Field::Gain {
        return NumericSpec::new(-60.0, 24.0, 0.0, NumericScale::Linear, 1, Some("db"))
            .ok()?
            .with_keyboard_steps(1.0, 0.1)
            .ok();
    }
    let current = value(window, field);
    let (min, max) = match field {
        Field::Position => (
            0,
            horizon
                .max(window.position)
                .saturating_add(window.asset_frames),
        ),
        Field::Length => (1, horizon.max(window.length).max(window.asset_frames)),
        Field::SourceStart => (0, window.source_end.checked_sub(1)?),
        Field::SourceEnd => (window.source_start.checked_add(1)?, window.asset_frames),
        Field::PlayStart => (window.source_start, window.source_end.checked_sub(1)?),
        _ => return None,
    };
    if current > MAX_EXACT_FRAME || min > max || current < min || current > max {
        return None;
    }
    let rate = window.sample_rate;
    NumericSpec::new(
        min as f64,
        max.max(min.saturating_add(1)).min(MAX_EXACT_FRAME) as f64,
        current as f64,
        NumericScale::Linear,
        0,
        None::<&str>,
    )
    .ok()?
    .with_keyboard_steps(frame_step(rate, false) as f64, 1.0)
    .ok()
    .map(|s| {
        s.with_codec(
            move |v| {
                frame_value(v).map_or_else(String::new, |v| format_frame_time(i128::from(v), rate))
            },
            move |s| {
                let s = s.trim();
                let frames = if let Some(seconds) = s.strip_suffix('s').filter(|_| !s.contains('·'))
                {
                    seconds.trim().parse::<f64>().ok()? * f64::from(rate)
                } else {
                    s.split('·').next()?.trim().parse::<f64>().ok()?
                };
                frame_value(frames).map(|v| v as f64)
            },
        )
    })
}
pub(super) fn number(
    view: &mut EditorView,
    window: ClipWindow,
    field: Field,
    label: &'static str,
    cx: &mut Context<EditorView>,
) -> Div {
    sync(view, window);
    if !view.audio_clip_controls.numbers.contains_key(&field) {
        let horizon = view
            .state
            .as_ref()
            .map_or(window.asset_frames, |s| s.timeline_frames);
        let initial = match field {
            Field::Gain => f64::from(window.gain.millidb()) / 1000.,
            Field::Pitch => f64::from(window.pitch_cents),
            _ => value(window, field) as f64,
        };
        let Some(spec) = spec(window, field, horizon) else {
            return div().child(theme::text(format!(
                "{label} · value outside control range"
            )));
        };
        let id = format!("audio-clip-{field:?}").to_lowercase();
        let Ok(config) = NumericControlConfig::new(
            id,
            theme::text(label),
            if field == Field::Pitch {
                NumericControlKind::Knob
            } else {
                NumericControlKind::HorizontalSlider
            },
            spec,
            initial,
            theme::control_style(),
        ) else {
            return div();
        };
        let entity = cx.new(|cx| {
            NumericControl::from_config(
                cx,
                config
                    .with_presentation(if field == Field::Pitch {
                        NumericControlPresentation::CompactDial
                    } else {
                        NumericControlPresentation::FilledSlider
                    })
                    .with_command_fine_steps(),
            )
        });
        entity.update(cx, |n, cx| n.set_tab_navigation(true, cx));
        let sub = cx.subscribe(&entity, move |view, _, e: &NumericControlEvent, cx| {
            if field == Field::Gain {
                match e.phase {
                    NumericEventPhase::Change | NumericEventPhase::Commit
                        if clip_window_is_current(view, window)
                            && !view.audio_clip_controls.has_pending()
                            && e.value.is_finite() =>
                    {
                        view.audio_clip_controls.gain_preview =
                            hex_project::ClipGain::new((e.value * 1000.0).round() as i32)
                                .ok()
                                .map(|gain| (window, gain));
                        if e.phase == NumericEventPhase::Change && e.gesture.is_some() {
                            let audition =
                                view.audio_clip_controls.gain_preview.and_then(|(_, gain)| {
                                    view.clip_gain_audition_edit(window.clip, window.gain, gain)
                                });
                            view.set_held_gain_audition(audition);
                        }
                    }
                    NumericEventPhase::Cancel => {
                        view.audio_clip_controls.gain_preview = None;
                        view.set_held_gain_audition(None);
                    }
                    _ => {}
                }
            }
            if e.phase == NumericEventPhase::Commit {
                if !view.edit_pending() && clip_window_is_current(view, window) {
                    let multiple = view.selected_clip_ids().len() > 1;
                    let command = if field == Field::Pitch {
                        if e.value == f64::from(window.pitch_cents) {
                            None
                        } else if let Some(state) = view.state.as_ref() {
                            match super::super::musical_audio_controls::pitch_command(
                                &state.project,
                                window.clip,
                                e.value,
                                multiple,
                            ) {
                                Ok(command) => Some(command),
                                Err(error) => {
                                    view.error = Some(error.0.into());
                                    None
                                }
                            }
                        } else {
                            None
                        }
                    } else {
                        absolute_command(window, field, e.value, multiple)
                    };
                    if let Some(command) = command {
                        if field == Field::Gain {
                            view.finish_gain_audition();
                        }
                        submit(view, window, command);
                    } else {
                        view.retire_clip_detail();
                    }
                } else if !view.audio_clip_controls.has_pending() {
                    view.retire_clip_detail();
                }
            }
            cx.notify();
        });
        view.audio_clip_controls.numbers.insert(
            field,
            Number {
                focus: entity.focus_handle(cx),
                entity,
                _subscription: sub,
            },
        );
    }
    let Some(binding) = view.audio_clip_controls.numbers.get(&field) else {
        return div();
    };
    let available = match field {
        Field::SourceStart => window.source_end > 1,
        Field::SourceEnd => window.asset_frames.saturating_sub(window.source_start) > 1,
        Field::PlayStart => window.source_length > 1,
        Field::Pitch => !view
            .state
            .as_ref()
            .and_then(|s| s.project.clip(window.clip))
            .and_then(|c| c.audio_edit())
            .is_some_and(|e| {
                e.domain.warp_enabled
                    && e.domain
                        .warp
                        .as_ref()
                        .is_some_and(|w| w.mode == hex_project::WarpMode::RePitch)
            }),
        _ => true,
    };
    binding.entity.update(cx, |n, cx| {
        n.set_availability(
            hex_widgets::ControlAvailability::for_edit(available, view.edit_pending()),
            cx,
        );
        let _ = n.set_external_style(theme::control_style(), cx);
    });
    if field == Field::Pitch {
        return div()
            .w(px(hex_widgets::COMPACT_DIAL_WIDTH))
            .h(px(hex_widgets::COMPACT_DIAL_HEIGHT))
            .flex_shrink_0()
            .flex()
            .justify_center()
            .child(binding.entity.clone());
    }
    div()
        .h(px(22.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(3.0))
        .child(
            div()
                .w(px(54.0))
                .flex_shrink_0()
                .text_xs()
                .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                .child(theme::text(label)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .debug_selector(move || format!("audio-clip-{field:?}-value").to_lowercase())
                .child(binding.entity.clone()),
        )
}
pub(super) fn toggle(
    view: &mut EditorView,
    window: ClipWindow,
    warp: bool,
    cx: &mut Context<EditorView>,
) -> Div {
    sync(view, window);
    let checked = if warp {
        view.state
            .as_ref()
            .and_then(|s| s.project.clip(window.clip))
            .and_then(|c| c.audio_edit())
            .is_some_and(|e| e.domain.warp_enabled)
    } else {
        window.looping()
    };
    if let std::collections::btree_map::Entry::Vacant(entry) =
        view.audio_clip_controls.toggles.entry(warp)
    {
        let label = if warp { "warp" } else { "loop" };
        let Ok(config) = ToggleConfig::new(
            format!("audio-clip-{label}-toggle"),
            theme::text(label),
            checked,
            theme::control_style(),
        ) else {
            return div();
        };
        let appearance = if warp {
            ToggleAppearance::CompactButton
        } else {
            ToggleAppearance::CompactSquare
        };
        let entity = cx.new(|cx| Toggle::from_config(cx, config.with_appearance(appearance)));
        entity.update(cx, |t, cx| t.set_tab_navigation(true, cx));
        let sub = cx.subscribe(&entity, move |view, _, _: &ToggleChanged, cx| {
            if view.edit_pending() || !clip_window_is_current(view, window) {
                if !view.audio_clip_controls.has_pending() {
                    view.retire_clip_detail();
                }
                cx.notify();
                return;
            }
            let command = if warp {
                match super::super::musical_audio_controls::warp_command(view, window.clip) {
                    Ok(command) => Some(command),
                    Err(error) => {
                        view.error = Some(error.0.into());
                        None
                    }
                }
            } else {
                command_for(window, Field::Looping, 1, 1)
            };
            if let Some(command) = command {
                submit(view, window, command);
            } else {
                view.retire_clip_detail();
            }
            cx.notify();
        });
        entry.insert(Switch {
            focus: entity.focus_handle(cx),
            entity,
            _subscription: sub,
        });
    }
    let Some(binding) = view.audio_clip_controls.toggles.get(&warp) else {
        return div();
    };
    binding.entity.update(cx, |t, cx| {
        t.set_availability(
            hex_widgets::ControlAvailability::for_edit(true, view.edit_pending()),
            cx,
        );
        let _ = t.set_external_style(theme::control_style(), cx);
    });
    let tooltip = if warp {
        "enable or disable clip warping"
    } else {
        window.loop_tooltip()
    };
    div().child(
        div()
            .id(if warp {
                "audio-warp-owner"
            } else {
                "audio-loop-owner"
            })
            .h(px(20.0))
            .flex_shrink_0()
            .when(warp, |owner| owner.w(px(44.0)))
            .tooltip(move |_, cx| {
                cx.new(|_| super::super::transport::MiniTooltip {
                    kind: crate::tooltips::Kind::Identification,
                    description: theme::text(tooltip),
                })
                .into()
            })
            .child(binding.entity.clone()),
    )
}
impl EditorView {
    pub(crate) fn retire_clip_detail(&mut self) -> bool {
        if self.audio_clip_controls.has_gain_preview() {
            self.set_held_gain_audition(None);
            self.service.retire_arrangement_gain_audition();
        }
        let changed = self.audio_clip_controls.reset();
        self.retire_sample_bpm_control();
        changed
    }

    pub(crate) fn retire_sample_bpm_control(&mut self) {
        if let Some(binding) = self.sample_bpm_control.take() {
            self.audio_clip_controls.retired_focus.push(RetiredFocus {
                focus: binding.focus,
                target: Target::Bpm,
                key: Some(binding.key),
            });
        }
    }
    pub(crate) fn restore_clip_control_focus(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let retired = std::mem::take(&mut self.audio_clip_controls.retired_focus);
        for RetiredFocus {
            focus: old,
            target,
            key,
        } in retired
        {
            if !old.is_focused(window) {
                continue;
            }
            let current = key.is_some_and(|(clip, generation, _)| {
                generation == self.project_generation && self.detail_clip_id() == Some(clip)
            });
            let focus = if current {
                match target {
                    Target::Number(field) => self
                        .audio_clip_controls
                        .numbers
                        .get(&field)
                        .map(|n| n.focus.clone()),
                    Target::Switch(warp) => self
                        .audio_clip_controls
                        .toggles
                        .get(&warp)
                        .map(|t| t.focus.clone()),
                    Target::Bpm => self.sample_bpm_control.as_ref().map(|b| b.focus.clone()),
                }
            } else {
                None
            };
            focus.unwrap_or_else(|| self.focus.clone()).focus(window);
            cx.notify();
            break;
        }
    }
    pub(crate) fn audio_clip_controls_key_is_current(&self) -> bool {
        self.audio_clip_controls
            .key
            .is_some_and(|(clip, generation, revision)| {
                generation == self.project_generation
                    && self.detail_clip_id() == Some(clip)
                    && self.state.as_ref().is_some_and(|s| s.revision == revision)
            })
    }
    pub(crate) fn retire_abandoned_clip_detail(&mut self) -> bool {
        let abandoned = self.audio_clip_controls.pending.as_ref().is_some_and(|p| {
            p.receipt.is_queued()
                && !self
                    .pending_requests
                    .iter()
                    .chain(self.device_gesture_barriers.iter())
                    .any(|r| self.audio_clip_controls.matches(r))
        });
        abandoned && self.retire_clip_detail()
    }
}

#[cfg(all(test, not(feature = "ui-test-support")))]
mod tests {
    use super::*;
    #[test]
    fn pitch_dial_spec_preserves_signed_cents_bounds_and_steps() {
        let spec = spec(super::super::tests::window(), Field::Pitch, 96_000).unwrap();
        assert_eq!(spec.format(137.).unwrap(), "+137 ct");
        assert_eq!(spec.parse("-137 ct").unwrap(), -137.);
        assert_eq!(spec.step(137., 1., false).unwrap(), 237.);
        assert_eq!(spec.step(137., 1., true).unwrap(), 138.);
        for text in ["nan", "inf", "4801 ct", "-4801", "0.5 ct"] {
            assert!(spec.parse(text).is_err());
        }
        for value in [-4800., 0., 4800.] {
            assert_eq!(spec.parse(&spec.format(value).unwrap()).unwrap(), value);
        }
    }
    #[test]
    fn clip_slider_units_and_conversion_preserve_exact_frames_and_bounds() {
        let window = super::super::tests::window();
        let position = spec(window, Field::Position, 96_000).unwrap();
        assert_eq!(position.parse("0.5s").unwrap(), 24_000.0);
        assert_eq!(position.parse("123 · 0.002s").unwrap(), 123.0);
        assert!(position.parse("nan").is_err());
        for v in [f64::NAN, f64::INFINITY, -1.0, (1_u64 << 53) as f64] {
            assert!(frame_value(v).is_none());
        }
        assert_eq!(frame_value(MAX_EXACT_FRAME as f64), Some(MAX_EXACT_FRAME));
        assert!(matches!(
            absolute_command(window, Field::Position, 124.0, false),
            Some(Command::MoveClip { start: 124, .. })
        ));
        assert!(matches!(
            absolute_command(window, Field::Gain, -6.1, false),
            Some(Command::SetAudioClipGain { millidb: -6100, .. })
        ));
        for (gain_db, delta) in [(-0.1, -100), (1.0, 1_000)] {
            assert!(matches!(
                absolute_command(window, Field::Gain, gain_db, true),
                Some(Command::EditClips {
                    clips,
                    edit: hex_project::ClipEdit::GainBy { millidb },
                }) if clips == vec![window.clip] && millidb == delta
            ));
        }
        assert!(absolute_command(window, Field::Gain, 24.001, false).is_none());
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod ui_tests {
    use super::*;
    use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point};
    use std::sync::Arc;
    #[gpui::test]
    fn clip_slider_cancel_and_frozen_release_wait_for_own_terminal(cx: &mut TestAppContext) {
        cx.update(hex_widgets::bind_numeric_control_keys);
        for accepted in [true, false] {
            let (handle, editor, _, clip) =
                super::super::super::clip_view_ui_test::audio_detail_fixture(cx);
            let mut visual = VisualTestContext::from_window(handle, cx);
            visual.run_until_parked();
            let before = editor.read_with(cx, |v, _| v.state.as_ref().unwrap().clone());
            let hit = visual
                .debug_bounds("audio-clip-position-value")
                .unwrap()
                .center();
            let moved = hit + point(px(0.0), px(-12.0));
            visual.simulate_mouse_down(hit, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(moved, Some(MouseButton::Left), Modifiers::default());
            visual.run_until_parked();
            editor.read_with(cx, |v, _| {
                assert_eq!(v.state.as_ref().unwrap().project, before.project);
                assert!(v.last_enqueued_edit.is_none());
            });
            visual.simulate_keystrokes("escape");
            visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            editor.read_with(cx, |v, cx| {
                assert_eq!(
                    v.audio_clip_controls.numbers[&Field::Position]
                        .entity
                        .read(cx)
                        .value(),
                    before.project.clip(clip).unwrap().start() as f64
                );
                assert!(v.last_enqueued_edit.is_none());
            });
            visual.simulate_mouse_down(hit, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(moved, Some(MouseButton::Left), Modifiers::default());
            visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            let (command, final_value) = editor.read_with(cx, |v, cx| {
                assert_eq!(
                    v.worker_requests_pending, 1,
                    "one release, one canonical request"
                );
                assert!(v.audio_clip_controls.has_pending());
                (
                    v.last_enqueued_edit.clone().unwrap(),
                    v.audio_clip_controls.numbers[&Field::Position]
                        .entity
                        .read(cx)
                        .value(),
                )
            });
            assert_ne!(
                final_value,
                before.project.clip(clip).unwrap().start() as f64
            );
            for _ in 0..2 {
                let mut intermediate =
                    super::super::super::arrangement_main_track_ui_test::completed_event(&before);
                intermediate.request_complete = false;
                editor.update(cx, |v, cx| {
                    v.receive(intermediate);
                    cx.notify();
                });
                visual.run_until_parked();
                editor.read_with(cx, |v, cx| {
                    assert!(v.audio_clip_controls.has_pending());
                    assert_eq!(v.state.as_ref().unwrap().project, before.project);
                    assert_eq!(
                        v.audio_clip_controls.numbers[&Field::Position]
                            .entity
                            .read(cx)
                            .value(),
                        final_value
                    );
                });
                assert!(visual.debug_bounds("audio-clip-position-value").is_some());
            }
            let mut terminal =
                super::super::super::arrangement_main_track_ui_test::completed_event(&before);
            if accepted {
                let mut oracle = hex_project::Editor::new((*before.project).clone(), 8).unwrap();
                oracle.execute(command).unwrap();
                let state = Arc::get_mut(&mut terminal.state).unwrap();
                state.project = Arc::new(oracle.project().clone());
                state.can_undo = true;
            } else {
                terminal.error = Some("fixture refusal".into());
            }
            editor.update(cx, |v, cx| {
                v.receive(terminal);
                cx.notify();
            });
            visual.run_until_parked();
            editor.read_with(cx, |v, cx| {
                assert!(!v.audio_clip_controls.has_pending());
                let canonical = v
                    .state
                    .as_ref()
                    .unwrap()
                    .project
                    .clip(clip)
                    .unwrap()
                    .start() as f64;
                assert_eq!(
                    v.audio_clip_controls.numbers[&Field::Position]
                        .entity
                        .read(cx)
                        .value(),
                    canonical
                );
                assert_eq!(
                    canonical,
                    if accepted {
                        final_value
                    } else {
                        before.project.clip(clip).unwrap().start() as f64
                    }
                );
            });
        }
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "pitch_ui_tests.rs"]
mod pitch_ui_tests;
