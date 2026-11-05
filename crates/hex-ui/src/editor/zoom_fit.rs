//! One prior fit viewport per existing view owner, never musical history. Clip
//! changes retire clip-local back state; document replacement retires every view.
use super::{EditorMode, EditorView, Viewport};
use gpui::{Context, Div, Stateful, Window, prelude::*, px};
use hex_project::{BeatTime, ClipId};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Target {
    Arrange,
    Audio,
    Midi,
}
#[derive(Clone, Copy)]
struct PianoView {
    start: i128,
    span: BeatTime,
    low: u8,
    rows: u8,
}
fn bounded_piano_view(prior: PianoView, length: u64) -> PianoView {
    let minimum = (BeatTime::ONE.raw() / 16).max(1).min(length.max(1));
    let span = prior
        .span
        .raw()
        .clamp(minimum, super::midi_scene::zoom_limit(length));
    PianoView {
        start: super::midi_scene::clamp_start(prior.start, span),
        span: BeatTime::from_raw(span),
        ..prior
    }
}
struct ArrangeFit {
    generation: u64,
    viewport: Option<Viewport>,
    vertical: Option<super::track_layout::VerticalFitSnapshot>,
}
#[derive(Default)]
pub(super) struct State {
    arrange: Option<ArrangeFit>,
    midi: Option<((u64, ClipId), PianoView)>,
}
impl State {
    pub(super) fn clear_clip(&mut self) {
        self.midi = None;
    }
}
pub(super) fn interval(a: u64, b: u64, horizon: u64, minimum: u64) -> Option<Viewport> {
    let first = a.min(b).min(horizon);
    let last = a.max(b).min(horizon);
    (last > first && horizon > 0).then(|| {
        Viewport {
            start: first,
            length: (last - first).max(minimum.max(1)).min(horizon),
        }
        .clamp(horizon)
    })
}
impl EditorView {
    fn remember_midi_fit(&mut self) {
        let Some(clip) = self.detail_clip_id() else {
            return;
        };
        let current = PianoView {
            start: self.piano_beat_start,
            span: self.piano_beat_span,
            low: self.piano_pitch_low,
            rows: self.piano_pitch_rows,
        };
        self.zoom_fit.midi = Some(((self.project_generation, clip), current));
    }
    /// Manual fit calculation; the caller owns when to retain back state.
    pub(super) fn fit_midi_interval(&mut self, range: (u64, u64), remember: bool) -> bool {
        let Some(length) = self.midi_clip().map(|clip| clip.source_length().raw()) else {
            return false;
        };
        let minimum = (BeatTime::ONE.raw() / 16).max(1).min(length.max(1));
        let Some(viewport) = interval(
            range.0,
            range.1,
            super::midi_scene::zoom_limit(length),
            minimum,
        ) else {
            return false;
        };
        if (self.piano_beat_start, self.piano_beat_span.raw())
            == (i128::from(viewport.start), viewport.length)
        {
            return false;
        }
        if remember {
            self.remember_midi_fit();
        }
        self.cancel_midi_note_gesture();
        self.piano_zoom_wheel_accum = 0.0;
        self.piano_beat_start = i128::from(viewport.start);
        self.piano_beat_span = BeatTime::from_raw(viewport.length);
        true
    }
    pub(super) fn reconcile_zoom_fit(&mut self) {
        if self
            .zoom_fit
            .arrange
            .as_ref()
            .is_some_and(|prior| prior.generation != self.project_generation)
        {
            self.zoom_fit.arrange = None;
        }
        if self.zoom_fit.midi.is_some_and(|(owner, _)| {
            owner.0 != self.project_generation
                || self.detail_clip_id() != Some(owner.1)
                || self.midi_clip().is_none()
        }) {
            self.zoom_fit.midi = None;
        }
    }
    pub(super) fn zoom_fit_key(&mut self, back: bool, window: &Window, cx: &Context<Self>) -> bool {
        if self.track_resize_active()
            || self.computer_keyboard.enabled
            || self.track_name.is_editing()
            || self.clip_name_edit.is_some()
            || self.project_tempo_editing(cx)
            || self.gain_numeric_editing(cx)
            || self.clip_numeric_editing(cx)
            || self.tuning.open
            || self.theme_settings.open
            || self.context_menu.is_some()
            || self.controller_input_is_open()
            || self.opening_project
            || self.close_prompt_open
            || self.save_dialog_open
            || self.export_dialog_open
            || self.preset_dialog_open
            || window
                .context_stack()
                .iter()
                .any(|c| c.contains("hex_text_editing"))
        {
            return false;
        }
        let target = if self.midi_focus.is_focused(window) {
            Target::Midi
        } else if self.audio_event_focus.is_focused(window) {
            Target::Audio
        } else if (self.focus.is_focused(window) || self.track_header_focused(window))
            && self.mode == EditorMode::Arrangement
        {
            Target::Arrange
        } else {
            return false;
        };
        self.apply_zoom_fit(target, back)
    }
    fn zoom_back_available(&self, target: Target) -> bool {
        match target {
            Target::Arrange => self
                .zoom_fit
                .arrange
                .as_ref()
                .is_some_and(|prior| prior.generation == self.project_generation),
            Target::Midi => self.zoom_fit.midi.is_some_and(|(owner, _)| {
                owner.0 == self.project_generation && self.detail_clip_id() == Some(owner.1)
            }),
            Target::Audio => self.event_zoom_back_available(),
        }
    }
    pub(super) fn apply_zoom_fit(&mut self, target: Target, back: bool) -> bool {
        if target == Target::Arrange && self.track_resize_active() {
            return false;
        }
        self.reconcile_zoom_fit();
        if target == Target::Audio {
            return self.event_fit_selection(back);
        }
        if target == Target::Midi {
            let Some(clip) = self.detail_clip_id() else {
                return false;
            };
            let Some(length) = self.midi_clip().map(|c| c.source_length().raw()) else {
                return false;
            };
            let next = if back {
                let Some((owner, prior)) = self.zoom_fit.midi.take() else {
                    return false;
                };
                if owner != (self.project_generation, clip) {
                    return false;
                }
                bounded_piano_view(prior, length)
            } else {
                return self.fit_midi_interval(
                    self.midi_time_selection_bounds().unwrap_or((0, length)),
                    true,
                );
            };
            self.cancel_midi_note_gesture();
            self.piano_zoom_wheel_accum = 0.0;
            self.piano_beat_start = next.start;
            self.piano_beat_span = next.span;
            self.piano_pitch_low = next.low;
            self.piano_pitch_rows = next.rows;
            return true;
        }
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let current = self.visible_viewport(state.timeline_frames);
        let horizon = if back {
            self.viewport_horizon(state.timeline_frames)
        } else {
            self.normal_navigation_horizon(state.timeline_frames)
        };
        let next = if back {
            let Some(prior) = self.zoom_fit.arrange.take() else {
                return false;
            };
            if prior.generation != self.project_generation {
                return false;
            }
            // Back is an explicit restoration, rather than scrolling into a
            // retired tail. Retain exactly the saved view when content shrank.
            let viewport = prior
                .viewport
                .map(|viewport| viewport.clamp(horizon.max(viewport.end().unwrap_or(horizon))));
            if let Some(vertical) = prior.vertical {
                self.restore_vertical_fit(vertical);
            }
            viewport
        } else {
            let project = &state.project;
            let audio_range = |c: &hex_project::Clip| {
                c.start()
                    .checked_add(c.duration())
                    .map(|end| (c.start(), end))
            };
            let midi_range = |c: &hex_project::MidiClip| {
                let end =
                    BeatTime::from_raw(c.start().raw().checked_add(c.arrangement_length().raw())?);
                Some((
                    project.tempo_map().beat_to_frame(c.start()).ok()?,
                    project.tempo_map().beat_to_frame(end).ok()?,
                ))
            };
            let clip_range = |id| {
                project
                    .clip(id)
                    .and_then(audio_range)
                    .or_else(|| project.midi_clip(id).and_then(midi_range))
            };
            let range = self
                .normalized_time_selection()
                .map(|r| (r.start(), r.end()))
                .or_else(|| self.selected_clip.and_then(clip_range))
                .or_else(|| {
                    self.sections.selected.and_then(|id| {
                        let section = project.arrangement_section(id)?;
                        Some((
                            project.tempo_map().beat_to_frame(section.start()).ok()?,
                            project.tempo_map().beat_to_frame(section.end()).ok()?,
                        ))
                    })
                })
                .or_else(|| {
                    project
                        .tracks()
                        .iter()
                        .flat_map(|t| {
                            t.clips()
                                .iter()
                                .filter_map(audio_range)
                                .chain(t.midi_clips().iter().filter_map(midi_range))
                        })
                        .chain(project.arrangement_sections().iter().filter_map(|section| {
                            Some((
                                project.tempo_map().beat_to_frame(section.start()).ok()?,
                                project.tempo_map().beat_to_frame(section.end()).ok()?,
                            ))
                        }))
                        .fold(None, |bounds, (a, b)| {
                            Some(bounds.map_or((a, b), |(first, last): (u64, u64)| {
                                (first.min(a), last.max(b))
                            }))
                        })
                });
            let (a, b) = range.unwrap_or((0, horizon));
            // Explicit fitting can restore an empty selected interval beyond
            // shortened content. Ordinary scrolling still obeys retired bounds.
            let target_horizon = horizon.max(a).max(b);
            let Some(next) = interval(a, b, target_horizon, 1) else {
                return false;
            };
            let members = self
                .normalized_time_selection()
                .map(|range| range.tracks().to_vec())
                .unwrap_or_else(|| {
                    let clips: std::collections::BTreeSet<_> =
                        self.selected_clip_ids().into_iter().collect();
                    let owners: Vec<_> = project
                        .tracks()
                        .iter()
                        .filter(|_| !clips.is_empty())
                        .filter(|track| {
                            track.clips().iter().any(|clip| clips.contains(&clip.id()))
                                || track
                                    .midi_clips()
                                    .iter()
                                    .any(|clip| clips.contains(&clip.id()))
                        })
                        .map(|track| track.id())
                        .collect();
                    if !owners.is_empty() {
                        return owners;
                    }
                    let selected = self.selected_track_ids();
                    if selected.is_empty() {
                        project.tracks().iter().map(|track| track.id()).collect()
                    } else {
                        selected
                    }
                });
            let same_horizontal =
                next == current && target_horizon == self.viewport_horizon(state.timeline_frames);
            let vertical = self.capture_vertical_fit();
            let changed_vertical = self.fit_tracks_vertically(&members);
            if same_horizontal && !changed_vertical {
                return false;
            }
            self.zoom_fit.arrange = Some(ArrangeFit {
                generation: self.project_generation,
                viewport: Some(current),
                vertical,
            });
            Some(next)
        };
        if let Some(next) = next {
            self.install_arranger_navigation(next);
        } else {
            self.clear_retained_navigation();
            self.viewport = None;
        }
        self.zoom_wheel_accum = 0.0;
        true
    }
    pub(super) fn zoom_fit_button(
        &self,
        target: Target,
        back: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let id = match (target, back) {
            (Target::Arrange, false) => "arrange-zoom-fit",
            (Target::Arrange, true) => "arrange-zoom-back",
            (Target::Midi, false) => "midi-zoom-fit",
            (Target::Midi, true) => "midi-zoom-back",
            (Target::Audio, false) => "audio-zoom-fit",
            (Target::Audio, true) => "audio-zoom-back",
        };
        let enabled = self.state.is_some()
            && (target != Target::Arrange || self.mode == EditorMode::Arrangement)
            && (!back || self.zoom_back_available(target));
        let generation = self.project_generation;
        let clip = self.detail_clip_id();
        super::waveform::button(
            if back { "↶" } else { "⤢" },
            if back {
                "zoom back · v"
            } else if target == Target::Arrange {
                "fit time and selected tracks · z"
            } else {
                "fit time selection or clip/content · z"
            },
            (id, 0),
        )
        .debug_selector(move || id.into())
        .opacity(if enabled { 1.0 } else { 0.35 })
        .when(!enabled, |el| el.cursor_default())
        .on_click(cx.listener(move |view, _, window, cx| {
            if !enabled
                || generation != view.project_generation
                || (target != Target::Arrange && clip != view.detail_clip_id())
            {
                return;
            }
            if view.apply_zoom_fit(target, back) {
                cx.notify();
            }
            match target {
                Target::Arrange => view.focus.focus(window),
                Target::Midi => view.midi_focus.focus(window),
                Target::Audio => view.audio_event_focus.focus(window),
            }
        }))
        .w(px(21.0))
    }
}
#[cfg(all(test, feature = "ui-test-support"))]
#[path = "zoom_fit_ui_tests.rs"]
mod tests;
