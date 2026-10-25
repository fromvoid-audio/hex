//! Passive Arrange-to-detail projection. Canonical ranges and musical state stay
//! with their existing owners. One bounded fit baseline survives each pointer
//! gesture; the audio/MIDI viewport owners retain their own Zoom Back state.
use super::{EditorMode, EditorView};
use crate::theme;
use gpui::{Context, Div, Stateful, Window, div, prelude::*, px, relative, rgb};
use hex_project::{AudioViewProjection, BeatTime, ClipId, TrackId};

type Intervals = [Option<(u64, u64)>; 2];
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ranges {
    Audio {
        intervals: Intervals,
        bounds: (u64, u64),
        frame_intervals: Intervals,
        frame_bounds: (u64, u64),
    },
    Midi {
        intervals: Intervals,
        bounds: (u64, u64),
    },
}
impl Ranges {
    fn union(self) -> Option<(u64, u64)> {
        let (Self::Audio { intervals, .. } | Self::Midi { intervals, .. }) = self;
        intervals
            .into_iter()
            .flatten()
            .reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)))
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct Witness {
    generation: u64,
    revision: u64,
    clip: ClipId,
    epoch: u64,
    selection: (TrackId, TrackId, u64, u64, bool),
}
enum Baseline {
    Audio(super::audio_event_editor::AudioFitBaseline),
    Midi(super::zoom_fit::MidiFitBaseline),
}
struct Gesture {
    owner: (u64, ClipId),
    baseline: Baseline,
}
#[derive(Default)]
pub(super) struct State {
    cached: Option<(Witness, Option<Ranges>)>,
    visible: bool,
    fitted: Option<(u64, ClipId, u64, Ranges)>,
    gesture: Option<Gesture>,
}
impl EditorView {
    fn arrange_range_witness(&self) -> Option<Witness> {
        if self.mode != EditorMode::Arrangement || !self.clip_view_open || self.device_panel_active
        {
            return None;
        }
        if self.selected_clip.is_some()
            || self
                .time_selection_gesture
                .as_ref()
                .is_some_and(|gesture| gesture.generation != self.project_generation)
        {
            return None;
        }
        let selection = self.time_selection?;
        if selection.anchor_frame == selection.end_frame {
            return None;
        }
        Some(Witness {
            generation: self.project_generation,
            revision: self.state.as_ref()?.revision,
            clip: self.retained_detail_identity()?,
            epoch: self.selection_intent_epoch?,
            selection: (
                selection.anchor_track,
                selection.end_track,
                selection.anchor_frame,
                selection.end_frame,
                self.time_selection_project_scope,
            ),
        })
    }
    fn project_arrange_detail_range(&self, witness: Witness) -> Option<Ranges> {
        let project = &self.state.as_ref()?.project;
        let (first, last) = self.time_selection_track_indices()?;
        let track = project.tracks().iter().position(|track| {
            track.clips().iter().any(|clip| clip.id() == witness.clip)
                || track
                    .midi_clips()
                    .iter()
                    .any(|clip| clip.id() == witness.clip)
        })?;
        if !(first..=last).contains(&track) {
            return None;
        }
        let start = witness.selection.2.min(witness.selection.3);
        let end = witness.selection.2.max(witness.selection.3);
        if let Some(clip) = project.clip(witness.clip) {
            let projection = AudioViewProjection::new(clip, project.tempo_map()).ok()?;
            let intervals = projection
                .root_ranges_for_frames(start..end)
                .ok()?
                .map(|slot| slot.map(|range| (range.start, range.end)));
            let root = clip.audio_event_root();
            let frame = |position| root.root_frame_at(position, project.tempo_map()).ok();
            let frame_bounds = (frame(root.view_start)?, frame(root.view_end)?);
            let mut frame_intervals = [None, None];
            for (range, output) in intervals.into_iter().zip(frame_intervals.iter_mut()) {
                if let Some((a, b)) = range {
                    *output = Some((frame(a)?, frame(b)?));
                }
            }
            Some(Ranges::Audio {
                intervals,
                bounds: (root.view_start, root.view_end),
                frame_intervals,
                frame_bounds,
            })
        } else {
            let clip = project.midi_clip(witness.clip)?;
            let start = project
                .tempo_map()
                .frame_to_beat(start)
                .ok()?
                .raw()
                .max(clip.start().raw());
            let end = project.tempo_map().frame_to_beat(end).ok()?.raw().min(
                clip.start()
                    .raw()
                    .checked_add(clip.arrangement_length().raw())?,
            );
            if start >= end {
                return None;
            }
            let intervals = clip
                .source_playback()
                .ok()?
                .source_ranges_at_elapsed(
                    BeatTime::from_raw(start - clip.start().raw())
                        ..BeatTime::from_raw(end - clip.start().raw()),
                )
                .ok()?
                .map(|slot| slot.map(|range| (range.start.raw(), range.end.raw())));
            Some(Ranges::Midi {
                intervals,
                bounds: (0, clip.source_length().raw()),
            })
        }
    }
    /// Called after the existing detail owner has reconciled its source identity,
    /// before it captures viewport geometry for this paint. No focus is changed.
    pub(super) fn reflect_arrange_clip_range(&mut self, window: &Window, cx: &Context<Self>) {
        self.arrange_clip_range.visible = false;
        let Some(witness) = self.arrange_range_witness() else {
            return;
        };
        if !self.focus.is_focused(window)
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
                .any(|context| context.contains("hex_text_editing"))
        {
            return;
        }
        if self
            .arrange_clip_range
            .gesture
            .as_ref()
            .is_some_and(|gesture| gesture.owner != (witness.generation, witness.clip))
        {
            self.arrange_clip_range.gesture = None;
        }
        if self
            .arrange_clip_range
            .cached
            .as_ref()
            .is_none_or(|(key, _)| *key != witness)
        {
            let ranges = self.project_arrange_detail_range(witness);
            self.arrange_clip_range.cached = Some((witness, ranges));
        }
        let Some(ranges) = self
            .arrange_clip_range
            .cached
            .and_then(|(_, ranges)| ranges)
        else {
            return;
        };
        let Some(union) = ranges.union() else {
            return;
        };
        self.arrange_clip_range.visible = true;
        let fitted = (witness.generation, witness.clip, witness.epoch, ranges);
        if self.arrange_clip_range.fitted == Some(fitted) {
            return;
        }
        let new_intent =
            self.arrange_clip_range
                .fitted
                .is_none_or(|(generation, clip, epoch, _)| {
                    (generation, clip, epoch) != (witness.generation, witness.clip, witness.epoch)
                });
        if new_intent {
            let baseline = match ranges {
                Ranges::Audio { .. } => self.begin_audio_range_fit().map(Baseline::Audio),
                Ranges::Midi { .. } => self.begin_midi_range_fit().map(Baseline::Midi),
            };
            if self.time_selection_gesture.is_some() {
                self.arrange_clip_range.gesture = baseline.map(|baseline| Gesture {
                    owner: (witness.generation, witness.clip),
                    baseline,
                });
            }
        }
        match ranges {
            Ranges::Audio { .. } => {
                self.fit_audio_interval(union, false);
            }
            Ranges::Midi { .. } => {
                self.fit_midi_interval(union, false);
            }
        }
        self.arrange_clip_range.fitted = Some(fitted);
    }
    /// Source owners retire their cache before initializing another clip viewport.
    pub(super) fn retire_arrange_range_owner(&mut self) {
        self.arrange_clip_range = State::default();
    }
    pub(super) fn finish_arrange_range_fit(&mut self) {
        self.arrange_clip_range.gesture = None;
    }
    pub(super) fn cancel_arrange_range_fit(&mut self) {
        if let Some(gesture) = self.arrange_clip_range.gesture.take() {
            match gesture.baseline {
                Baseline::Audio(baseline) => self.restore_audio_range_fit(baseline),
                Baseline::Midi(baseline) => self.restore_midi_range_fit(baseline),
            }
        }
        self.arrange_clip_range.cached = None;
        self.arrange_clip_range.fitted = None;
        self.arrange_clip_range.visible = false;
    }
    fn reflected_ranges(&self) -> Option<Ranges> {
        if !self.arrange_clip_range.visible {
            return None;
        }
        let (witness, ranges) = self.arrange_clip_range.cached?;
        (self.arrange_range_witness() == Some(witness))
            .then_some(ranges)
            .flatten()
    }
    pub(super) fn arrange_midi_highlights(
        &self,
        start: u64,
        length: u64,
        top: f32,
        prefix: &'static str,
    ) -> impl Iterator<Item = Stateful<Div>> + use<> {
        let intervals = match self.reflected_ranges() {
            Some(Ranges::Midi { intervals, .. }) => intervals,
            _ => [None, None],
        };
        overlays(intervals, start, length, top, prefix)
    }
    pub(super) fn arrange_audio_highlights(
        &self,
        source_frame_origin: u64,
        start: u64,
        length: u64,
    ) -> impl Iterator<Item = Stateful<Div>> + use<> {
        let intervals = match self.reflected_ranges() {
            Some(Ranges::Audio {
                frame_intervals, ..
            }) => frame_intervals.map(|range| {
                range.and_then(|(a, b)| {
                    Some((
                        a.checked_sub(source_frame_origin)?,
                        b.checked_sub(source_frame_origin)?,
                    ))
                })
            }),
            _ => [None, None],
        };
        overlays(intervals, start, length, 22.0, "arrange-reflected-range")
    }
}
fn overlays(
    intervals: Intervals,
    start: u64,
    length: u64,
    top: f32,
    prefix: &'static str,
) -> impl Iterator<Item = Stateful<Div>> {
    intervals
        .into_iter()
        .enumerate()
        .filter_map(move |(index, range)| {
            let (a, b) = range?;
            let a = a.max(start);
            let b = b.min(start.checked_add(length)?);
            if a >= b || length == 0 {
                return None;
            }
            Some(
                div()
                    .id((prefix, index))
                    .debug_selector(move || format!("{prefix}-{index}"))
                    .absolute()
                    .top(px(top))
                    .bottom_0()
                    .left(relative((a - start) as f32 / length as f32))
                    .w(relative((b - a) as f32 / length as f32))
                    .bg(rgb(theme::color(theme::PaletteRole::Secondary)))
                    .opacity(theme::TIME_SELECTION_OPACITY),
            )
        })
}
