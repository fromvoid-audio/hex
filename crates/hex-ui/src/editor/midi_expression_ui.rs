//! Piano-roll MIDI CC, pitch bend, and per-note MPE expression lane UI.
//!
//! Pure coordinate/value transforms and canonical point editing live here alongside
//! the GPUI lane canvas. All mutations dispatch validated `hex_project::Command`
//! edits through `Request::Edit` for atomic prepare-before-commit Undo/Redo.
use super::EditorView;
use crate::theme;
use gpui::{
    Bounds, Context as ViewContext, Div, MouseButton, MouseDownEvent, ScrollWheelEvent, canvas,
    div, fill, point, prelude::*, px, rgb, size,
};
use hex_project::{
    BeatTime, ClipId, Command, MAX_MIDI_EXPRESSION_POINTS, MAX_NOTE_PITCH_OFFSET_CENTS,
    MIN_NOTE_PITCH_OFFSET_CENTS, MidiLaneKind, MidiLanePoint, NoteExpression, NoteId,
};
use hex_session::service::Request;

const LANE_VERTICAL_PADDING_PX: f32 = 4.0;
const POINT_HIT_RADIUS_PX: f32 = 8.0;

/// Active bottom lane displayed under the piano roll.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ExpressionLaneMode {
    #[default]
    Velocity,
    Cc1,
    Cc74,
    PitchBend,
    NotePitch,
    NoteTimbre,
    NotePressure,
}

impl ExpressionLaneMode {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Velocity => "velocity",
            Self::Cc1 => "cc1 mod",
            Self::Cc74 => "cc74 timbre",
            Self::PitchBend => "pitch bend",
            Self::NotePitch => "note pitch",
            Self::NoteTimbre => "note timbre",
            Self::NotePressure => "note pressure",
        }
    }

    pub(super) const fn clip_lane_kind(self) -> Option<MidiLaneKind> {
        match self {
            Self::Cc1 => Some(MidiLaneKind::ControlChange(1)),
            Self::Cc74 => Some(MidiLaneKind::ControlChange(74)),
            Self::PitchBend => Some(MidiLaneKind::PitchBend),
            Self::Velocity | Self::NotePitch | Self::NoteTimbre | Self::NotePressure => None,
        }
    }

    pub(super) const fn is_mpe_note_mode(self) -> bool {
        matches!(
            self,
            Self::NotePitch | Self::NoteTimbre | Self::NotePressure
        )
    }

    pub(super) const fn is_bipolar(self) -> bool {
        matches!(self, Self::PitchBend | Self::NotePitch)
    }
}

pub(super) fn y_for_lane_value(kind: MidiLaneKind, value: i16, height: f32) -> Option<f32> {
    if !height.is_finite() || height <= LANE_VERTICAL_PADDING_PX * 2.0 {
        return None;
    }
    let usable = height - LANE_VERTICAL_PADDING_PX * 2.0;
    let normalized = match kind {
        MidiLaneKind::ControlChange(_) => f32::from(value.clamp(0, 127)) / 127.0,
        MidiLaneKind::PitchBend => (f32::from(value.clamp(-8192, 8191)) + 8192.0) / 16383.0,
    };
    Some(LANE_VERTICAL_PADDING_PX + (1.0 - normalized) * usable)
}

pub(super) fn lane_value_for_y(kind: MidiLaneKind, y: f32, height: f32) -> Option<i16> {
    if !y.is_finite() || !height.is_finite() || height <= LANE_VERTICAL_PADDING_PX * 2.0 {
        return None;
    }
    let usable = height - LANE_VERTICAL_PADDING_PX * 2.0;
    let normalized = (1.0 - ((y - LANE_VERTICAL_PADDING_PX) / usable)).clamp(0.0, 1.0);
    let value = match kind {
        MidiLaneKind::ControlChange(_) => (normalized * 127.0).round() as i16,
        MidiLaneKind::PitchBend => {
            let signed = (normalized * 16383.0 - 8192.0).round() as i32;
            signed.clamp(-8192, 8191) as i16
        }
    };
    Some(value)
}

pub(super) fn y_for_note_expr(
    mode: ExpressionLaneMode,
    expression: NoteExpression,
    height: f32,
) -> Option<f32> {
    if !height.is_finite() || height <= LANE_VERTICAL_PADDING_PX * 2.0 {
        return None;
    }
    let usable = height - LANE_VERTICAL_PADDING_PX * 2.0;
    let normalized = match mode {
        ExpressionLaneMode::NotePitch => {
            let clamped = expression
                .pitch_offset_cents
                .clamp(MIN_NOTE_PITCH_OFFSET_CENTS, MAX_NOTE_PITCH_OFFSET_CENTS);
            (f32::from(clamped) - f32::from(MIN_NOTE_PITCH_OFFSET_CENTS))
                / f32::from(MAX_NOTE_PITCH_OFFSET_CENTS - MIN_NOTE_PITCH_OFFSET_CENTS)
        }
        ExpressionLaneMode::NoteTimbre => f32::from(expression.timbre.min(127)) / 127.0,
        ExpressionLaneMode::NotePressure => f32::from(expression.pressure.min(127)) / 127.0,
        _ => return None,
    };
    Some(LANE_VERTICAL_PADDING_PX + (1.0 - normalized) * usable)
}

pub(super) fn note_expr_for_y(
    mode: ExpressionLaneMode,
    base: NoteExpression,
    y: f32,
    height: f32,
) -> Option<NoteExpression> {
    if !y.is_finite() || !height.is_finite() || height <= LANE_VERTICAL_PADDING_PX * 2.0 {
        return None;
    }
    let usable = height - LANE_VERTICAL_PADDING_PX * 2.0;
    let normalized = (1.0 - ((y - LANE_VERTICAL_PADDING_PX) / usable)).clamp(0.0, 1.0);
    let mut updated = base;
    match mode {
        ExpressionLaneMode::NotePitch => {
            let span = i32::from(MAX_NOTE_PITCH_OFFSET_CENTS - MIN_NOTE_PITCH_OFFSET_CENTS);
            let cents =
                (normalized * span as f32).round() as i32 + i32::from(MIN_NOTE_PITCH_OFFSET_CENTS);
            // Snap small clicks within ±30 cents back to concert pitch 0.
            let snapped = if cents.abs() <= 30 { 0 } else { cents };
            updated.pitch_offset_cents = snapped.clamp(
                i32::from(MIN_NOTE_PITCH_OFFSET_CENTS),
                i32::from(MAX_NOTE_PITCH_OFFSET_CENTS),
            ) as i16;
        }
        ExpressionLaneMode::NoteTimbre => {
            updated.timbre = (normalized * 127.0).round().clamp(0.0, 127.0) as u8;
        }
        ExpressionLaneMode::NotePressure => {
            updated.pressure = (normalized * 127.0).round().clamp(0.0, 127.0) as u8;
        }
        _ => return None,
    }
    Some(updated)
}

pub(super) fn beat_for_x(
    x: f32,
    width: f32,
    beat_start: i128,
    beat_span: u64,
    source_length: u64,
) -> Option<BeatTime> {
    if !x.is_finite() || !width.is_finite() || width <= 0.0 || beat_span == 0 || source_length == 0
    {
        return None;
    }
    let ratio = f64::from((x / width).clamp(0.0, 1.0));
    let raw = beat_start.saturating_add((ratio * beat_span as f64).round() as i128);
    if raw < 0 {
        return None;
    }
    let clamped = u64::try_from(raw)
        .ok()?
        .min(source_length.saturating_sub(1));
    Some(BeatTime::from_raw(clamped))
}

/// Insert or replace an expression point at `beat` (merging points within `merge_radius_raw`).
pub(super) fn upsert_lane_point(
    existing: &[MidiLanePoint],
    beat: BeatTime,
    channel: u8,
    value: i16,
    merge_radius_raw: u64,
) -> Vec<MidiLanePoint> {
    let mut points = existing.to_vec();
    if let Some(index) = points
        .iter()
        .position(|p| p.channel == channel && p.beat.raw().abs_diff(beat.raw()) <= merge_radius_raw)
    {
        points[index] = MidiLanePoint {
            beat,
            channel,
            value,
        };
    } else if points.len() < MAX_MIDI_EXPRESSION_POINTS {
        points.push(MidiLanePoint {
            beat,
            channel,
            value,
        });
    }
    points.sort_by_key(|p| (p.beat.raw(), p.channel));
    points
}

/// Remove the point nearest to `(beat, channel)` within `hit_radius_raw`, if any.
pub(super) fn remove_nearest_lane_point(
    existing: &[MidiLanePoint],
    beat: BeatTime,
    hit_radius_raw: u64,
) -> Option<Vec<MidiLanePoint>> {
    let index = existing
        .iter()
        .enumerate()
        .filter(|(_, p)| p.beat.raw().abs_diff(beat.raw()) <= hit_radius_raw)
        .min_by_key(|(_, p)| p.beat.raw().abs_diff(beat.raw()))
        .map(|(index, _)| index)?;
    let mut points = existing.to_vec();
    points.remove(index);
    Some(points)
}

#[derive(Clone)]
struct NoteExprVisual {
    x_ratio: f32,
    expression: NoteExpression,
    selected: bool,
}

impl EditorView {
    pub(super) fn expression_mode_tabs(&self, cx: &mut ViewContext<Self>) -> Div {
        let active = self.midi_editor.velocity.mode;
        let tab = |id: &'static str,
                   label: &'static str,
                   target: ExpressionLaneMode,
                   active_match: bool| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .cursor_pointer()
                .px_1()
                .rounded_sm()
                .text_xs()
                .bg(rgb(theme::color(if active_match {
                    theme::PaletteRole::Primary
                } else {
                    theme::PaletteRole::Surface
                })))
                .text_color(rgb(theme::color(if active_match {
                    theme::PaletteRole::Background
                } else {
                    theme::PaletteRole::Muted
                })))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |view, _, _, cx| {
                        view.midi_editor.velocity.mode = target;
                        view.midi_editor.velocity.cancel_value_state();
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .child(theme::text(label))
        };
        let mpe_target = if active.is_mpe_note_mode() {
            active
        } else {
            ExpressionLaneMode::NotePitch
        };
        div().flex().items_center().gap_1().pt_0p5().children([
            tab(
                "midi-lane-vel",
                "vel",
                ExpressionLaneMode::Velocity,
                active == ExpressionLaneMode::Velocity,
            ),
            tab(
                "midi-lane-cc1",
                "cc1",
                ExpressionLaneMode::Cc1,
                active == ExpressionLaneMode::Cc1,
            ),
            tab(
                "midi-lane-cc74",
                "cc74",
                ExpressionLaneMode::Cc74,
                active == ExpressionLaneMode::Cc74,
            ),
            tab(
                "midi-lane-bend",
                "bend",
                ExpressionLaneMode::PitchBend,
                active == ExpressionLaneMode::PitchBend,
            ),
            tab(
                "midi-lane-mpe",
                "mpe",
                mpe_target,
                active.is_mpe_note_mode(),
            ),
        ])
    }

    pub(super) fn expression_mode_controls(&self, clip: ClipId, cx: &mut ViewContext<Self>) -> Div {
        let mode = self.midi_editor.velocity.mode;
        if mode.is_mpe_note_mode() {
            let sub = |id: &'static str, label: &'static str, target: ExpressionLaneMode| {
                let selected = mode == target;
                div()
                    .id(id)
                    .debug_selector(move || id.into())
                    .cursor_pointer()
                    .px_1()
                    .text_color(rgb(theme::color(if selected {
                        theme::PaletteRole::Primary
                    } else {
                        theme::PaletteRole::Muted
                    })))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, _, cx| {
                            view.midi_editor.velocity.mode = target;
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    )
                    .child(theme::text(label))
            };
            return div().flex().items_center().gap_1().text_xs().children([
                sub("midi-mpe-pitch", "pitch", ExpressionLaneMode::NotePitch),
                sub("midi-mpe-timbre", "timbre", ExpressionLaneMode::NoteTimbre),
                sub("midi-mpe-press", "press", ExpressionLaneMode::NotePressure),
            ]);
        }
        let point_count = mode
            .clip_lane_kind()
            .and_then(|kind| {
                self.state
                    .as_ref()?
                    .project
                    .midi_source(clip)?
                    .expression_lane(kind)
                    .map(|lane| lane.points.len())
            })
            .unwrap_or(0);
        let enabled = !self.edit_pending() && point_count > 0;
        div()
            .flex()
            .items_center()
            .gap_1()
            .text_xs()
            .child(theme::text(format!("{point_count} pts")))
            .child(
                div()
                    .id("midi-expression-clear")
                    .debug_selector(|| "midi-expression-clear".into())
                    .cursor_pointer()
                    .px_1()
                    .text_color(rgb(theme::color(if enabled {
                        theme::PaletteRole::Text
                    } else {
                        theme::PaletteRole::Muted
                    })))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, _, cx| {
                            if !view.edit_pending()
                                && let Some(kind) = view.midi_editor.velocity.mode.clip_lane_kind()
                            {
                                view.enqueue(Request::Edit(Command::SetMidiExpressionLane {
                                    clip,
                                    kind,
                                    points: Vec::new(),
                                }));
                            }
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    )
                    .child(theme::text("clear")),
            )
    }

    pub(super) fn expression_lane_canvas(
        &self,
        clip: ClipId,
        cx: &mut ViewContext<Self>,
    ) -> Option<gpui::Stateful<Div>> {
        let state = self.state.as_ref()?;
        let source = state.project.midi_source(clip)?;
        let mode = self.midi_editor.velocity.mode;
        let beat_start = self.midi_editor.piano_beat_start;
        let beat_span = self.midi_editor.piano_beat_span.raw();
        let source_length = source.source_length().raw();
        let entity = cx.entity();

        let lane_points: Vec<MidiLanePoint> = mode
            .clip_lane_kind()
            .and_then(|kind| source.expression_lane(kind))
            .map(|lane| lane.points.clone())
            .unwrap_or_default();

        let note_visuals: Vec<NoteExprVisual> = if mode.is_mpe_note_mode() && beat_span > 0 {
            let selection = self.midi_editor.selection.shared_ids();
            source
                .notes()
                .iter()
                .filter_map(|note| {
                    let raw = i128::from(note.start().raw());
                    let end = beat_start.saturating_add(i128::from(beat_span));
                    if raw < beat_start || raw > end {
                        return None;
                    }
                    let x_ratio = ((raw - beat_start) as f64 / beat_span as f64) as f32;
                    Some(NoteExprVisual {
                        x_ratio,
                        expression: note.expression(),
                        selected: selection.binary_search(&note.id()).is_ok(),
                    })
                })
                .collect()
        } else {
            Vec::new()
        };

        let paint_points = lane_points.clone();
        let paint_notes = note_visuals.clone();

        Some(
            div()
                .id("midi-expression-lane")
                .debug_selector(|| "midi-expression-lane".into())
                .relative()
                .flex_1()
                .h_full()
                .min_w_0()
                .overflow_hidden()
                .bg(rgb(theme::color(theme::PaletteRole::Background)))
                .track_focus(&self.midi_editor.focus)
                .key_context("hex_midi")
                .child(
                    canvas(
                        |bounds, window, _| {
                            window.insert_hitbox(bounds, gpui::HitboxBehavior::Normal)
                        },
                        move |bounds, _hitbox, window, _cx| {
                            let width = f32::from(bounds.size.width);
                            let height = f32::from(bounds.size.height);
                            if !width.is_finite()
                                || !height.is_finite()
                                || width <= 0.0
                                || height <= LANE_VERTICAL_PADDING_PX * 2.0
                            {
                                return;
                            }
                            let mut quad = |x: f32, y: f32, w: f32, h: f32, color: u32| {
                                if w > 0.0 && h > 0.0 {
                                    window.paint_quad(fill(
                                        Bounds::new(
                                            point(bounds.origin.x + px(x), bounds.origin.y + px(y)),
                                            size(px(w), px(h)),
                                        ),
                                        rgb(color),
                                    ));
                                }
                            };

                            // Reference guide lines (centerline highlighted for bipolar lanes).
                            let border = theme::color(theme::PaletteRole::Border);
                            let muted = theme::color(theme::PaletteRole::Muted);
                            let primary = theme::color(theme::PaletteRole::Primary);
                            let top_y = LANE_VERTICAL_PADDING_PX;
                            let mid_y = height * 0.5;
                            let bot_y = height - LANE_VERTICAL_PADDING_PX;
                            quad(0.0, top_y, width, 1.0, border);
                            quad(
                                0.0,
                                mid_y,
                                width,
                                1.0,
                                if mode.is_bipolar() { muted } else { border },
                            );
                            quad(0.0, bot_y, width, 1.0, border);

                            if let Some(kind) = mode.clip_lane_kind() {
                                let mut prev_xy: Option<(f32, f32)> = None;
                                for pt in &paint_points {
                                    let raw = i128::from(pt.beat.raw());
                                    let x = ((raw - beat_start) as f64 / beat_span.max(1) as f64)
                                        as f32
                                        * width;
                                    let Some(y) = y_for_lane_value(kind, pt.value, height) else {
                                        continue;
                                    };
                                    if let Some((px_prev, py_prev)) = prev_xy {
                                        let seg_x = px_prev.clamp(0.0, width);
                                        let seg_w = (x.clamp(0.0, width) - seg_x).max(0.0);
                                        if seg_w > 0.0 {
                                            quad(seg_x, py_prev, seg_w, 1.5, primary);
                                        }
                                    }
                                    if x >= -4.0 && x <= width + 4.0 {
                                        let stem_top =
                                            if mode.is_bipolar() { y.min(mid_y) } else { y };
                                        let stem_bottom = if mode.is_bipolar() {
                                            y.max(mid_y)
                                        } else {
                                            bot_y
                                        };
                                        quad(
                                            x.clamp(0.0, width - 1.0),
                                            stem_top,
                                            1.0,
                                            (stem_bottom - stem_top).max(1.0),
                                            primary,
                                        );
                                        quad(
                                            (x - 3.0).clamp(0.0, width),
                                            (y - 3.0).clamp(0.0, height),
                                            6.0,
                                            6.0,
                                            primary,
                                        );
                                    }
                                    prev_xy = Some((x, y));
                                }
                            } else if mode.is_mpe_note_mode() {
                                for note in &paint_notes {
                                    let Some(y) = y_for_note_expr(mode, note.expression, height)
                                    else {
                                        continue;
                                    };
                                    let x = (note.x_ratio * width).clamp(0.0, width);
                                    let color = if note.selected { primary } else { muted };
                                    let stem_top = if mode.is_bipolar() { y.min(mid_y) } else { y };
                                    let stem_bottom = if mode.is_bipolar() {
                                        y.max(mid_y)
                                    } else {
                                        bot_y
                                    };
                                    quad(
                                        x,
                                        stem_top,
                                        1.0,
                                        (stem_bottom - stem_top).max(1.0),
                                        color,
                                    );
                                    quad(
                                        (x - 3.0).max(0.0),
                                        y - 2.0,
                                        6.0_f32.min(width - x + 3.0),
                                        4.0,
                                        color,
                                    );
                                }
                            }

                            let wheel = entity.clone();
                            window.on_mouse_event(move |event: &ScrollWheelEvent, phase, _, cx| {
                                if !phase.capture() || !bounds.contains(&event.position) {
                                    return;
                                }
                                let delta =
                                    -f32::from(event.physical_delta().pixel_delta(px(80.0)).y);
                                if !delta.is_finite() || delta == 0.0 {
                                    return;
                                }
                                cx.stop_propagation();
                                wheel.update(cx, |view, cx| {
                                    if view.zoom_piano_wheel_at(
                                        [
                                            f32::from(bounds.origin.x),
                                            f32::from(bounds.origin.y),
                                            width,
                                            height,
                                        ],
                                        f32::from(event.position.x),
                                        delta,
                                    ) {
                                        cx.notify();
                                    }
                                });
                            });

                            let down = entity.clone();
                            window.on_mouse_event(
                                move |event: &MouseDownEvent, phase, window, cx| {
                                    if !phase.bubble() || !bounds.contains(&event.position) {
                                        return;
                                    }
                                    if event.button != MouseButton::Left
                                        && event.button != MouseButton::Right
                                    {
                                        return;
                                    }
                                    let local_x = f32::from(event.position.x - bounds.origin.x);
                                    let local_y = f32::from(event.position.y - bounds.origin.y);
                                    let is_remove =
                                        event.button == MouseButton::Right || event.modifiers.alt;
                                    down.update(cx, |view, cx| {
                                        if view.edit_pending() || view.context_menu.is_some() {
                                            return;
                                        }
                                        view.midi_editor.focus.focus(window);
                                        view.handle_expression_lane_click(
                                            clip,
                                            mode,
                                            local_x,
                                            local_y,
                                            width,
                                            height,
                                            beat_start,
                                            beat_span,
                                            source_length,
                                            is_remove,
                                        );
                                        cx.stop_propagation();
                                        cx.notify();
                                    });
                                },
                            );
                        },
                    )
                    .absolute()
                    .inset_0(),
                ),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn handle_expression_lane_click(
        &mut self,
        clip: ClipId,
        mode: ExpressionLaneMode,
        local_x: f32,
        local_y: f32,
        width: f32,
        height: f32,
        beat_start: i128,
        beat_span: u64,
        source_length: u64,
        is_remove: bool,
    ) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let Some(source) = state.project.midi_source(clip) else {
            return;
        };
        if let Some(kind) = mode.clip_lane_kind() {
            let Some(beat) = beat_for_x(local_x, width, beat_start, beat_span, source_length)
            else {
                return;
            };
            let hit_beats = ((f64::from(POINT_HIT_RADIUS_PX) / f64::from(width.max(1.0)))
                * beat_span as f64)
                .round()
                .max(1.0) as u64;
            let existing = source
                .expression_lane(kind)
                .map(|lane| lane.points.as_slice())
                .unwrap_or(&[]);
            if is_remove {
                if let Some(points) = remove_nearest_lane_point(existing, beat, hit_beats) {
                    self.enqueue(Request::Edit(Command::SetMidiExpressionLane {
                        clip,
                        kind,
                        points,
                    }));
                }
                return;
            }
            let Some(value) = lane_value_for_y(kind, local_y, height) else {
                return;
            };
            let channel = source
                .notes()
                .first()
                .map_or(0, hex_project::MidiNote::channel);
            let points = upsert_lane_point(existing, beat, channel, value, hit_beats);
            self.enqueue(Request::Edit(Command::SetMidiExpressionLane {
                clip,
                kind,
                points,
            }));
        } else if mode.is_mpe_note_mode() && beat_span > 0 {
            let mut best: Option<(
                f32,
                NoteId,
                NoteExpression,
                Vec<hex_project::NoteExpressionPoint>,
            )> = None;
            for note in source.notes() {
                let raw = i128::from(note.start().raw());
                let x = ((raw - beat_start) as f64 / beat_span as f64) as f32 * width;
                let dist = (x - local_x).abs();
                if dist <= POINT_HIT_RADIUS_PX * 2.0
                    && best
                        .as_ref()
                        .is_none_or(|(best_dist, ..)| dist < *best_dist)
                {
                    best = Some((
                        dist,
                        note.id(),
                        note.expression(),
                        note.expression_points().to_vec(),
                    ));
                }
            }
            let Some((_, note_id, current, points)) = best else {
                return;
            };
            let updated = if is_remove {
                let mut reset = current;
                match mode {
                    ExpressionLaneMode::NotePitch => reset.pitch_offset_cents = 0,
                    ExpressionLaneMode::NoteTimbre => reset.timbre = 0,
                    ExpressionLaneMode::NotePressure => reset.pressure = 0,
                    _ => {}
                }
                reset
            } else {
                let Some(next) = note_expr_for_y(mode, current, local_y, height) else {
                    return;
                };
                next
            };
            if updated != current {
                self.enqueue(Request::Edit(Command::SetMidiNoteExpression {
                    clip,
                    note: note_id,
                    expression: updated,
                    points,
                }));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cc_and_pitch_bend_coordinate_round_trip() {
        let h = 64.0;
        let y_top = y_for_lane_value(MidiLaneKind::ControlChange(1), 127, h).unwrap();
        let y_bot = y_for_lane_value(MidiLaneKind::ControlChange(1), 0, h).unwrap();
        assert_eq!(
            lane_value_for_y(MidiLaneKind::ControlChange(1), y_top, h),
            Some(127)
        );
        assert_eq!(
            lane_value_for_y(MidiLaneKind::ControlChange(1), y_bot, h),
            Some(0)
        );

        let y_center = y_for_lane_value(MidiLaneKind::PitchBend, 0, h).unwrap();
        let center_val = lane_value_for_y(MidiLaneKind::PitchBend, y_center, h).unwrap();
        assert!(center_val.abs() <= 1, "expected ~0, got {center_val}");
        assert_eq!(
            lane_value_for_y(MidiLaneKind::PitchBend, LANE_VERTICAL_PADDING_PX, h),
            Some(8191)
        );
        assert_eq!(
            lane_value_for_y(MidiLaneKind::PitchBend, h - LANE_VERTICAL_PADDING_PX, h),
            Some(-8192)
        );
    }

    #[test]
    fn note_expression_coordinate_transforms_and_upsert_work() {
        let h = 64.0;
        let base = NoteExpression::default();
        let pitched = note_expr_for_y(ExpressionLaneMode::NotePitch, base, 4.0, h).unwrap();
        assert_eq!(pitched.pitch_offset_cents, MAX_NOTE_PITCH_OFFSET_CENTS);
        let centered = note_expr_for_y(ExpressionLaneMode::NotePitch, pitched, h * 0.5, h).unwrap();
        assert_eq!(centered.pitch_offset_cents, 0);

        let timbre = note_expr_for_y(ExpressionLaneMode::NoteTimbre, base, 4.0, h).unwrap();
        assert_eq!(timbre.timbre, 127);

        let p1 = upsert_lane_point(&[], BeatTime::ONE, 0, 64, 100);
        assert_eq!(p1.len(), 1);
        let p2 = upsert_lane_point(
            &p1,
            BeatTime::from_raw(BeatTime::ONE.raw() + 50),
            0,
            100,
            100,
        );
        assert_eq!(p2.len(), 1);
        assert_eq!(p2[0].value, 100);
        let p3 = upsert_lane_point(&p2, BeatTime::from_raw(BeatTime::ONE.raw() * 2), 0, 32, 100);
        assert_eq!(p3.len(), 2);
        let removed = remove_nearest_lane_point(&p3, BeatTime::ONE, 100).unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].value, 32);
    }
}
