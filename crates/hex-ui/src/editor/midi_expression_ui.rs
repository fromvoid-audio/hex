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
    BeatTime, ClipId, Command, MAX_MIDI_EXPRESSION_POINTS, MAX_NOTE_EXPRESSION_POINTS,
    MAX_NOTE_PITCH_OFFSET_CENTS, MIN_NOTE_PITCH_OFFSET_CENTS, MidiLaneKind, MidiLanePoint,
    NoteExpression, NoteExpressionPoint,
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

/// Finite piecewise-linear interpolation law for a note's continuous
/// per-note expression curve over `[0, duration]`.
///
/// - Empty curves hold `initial`.
/// - Offsets at or before the first point hold the first point's values.
/// - Offsets at or after the last point hold the last point's values.
/// - Interior offsets interpolate linearly between the bracketing ascending
///   points with clamped rounding into canonical bounds.
pub(super) fn evaluate_note_expression_curve(
    initial: NoteExpression,
    points: &[NoteExpressionPoint],
    offset: BeatTime,
) -> NoteExpression {
    let Some(first) = points.first() else {
        return initial;
    };
    if offset.raw() <= first.offset.raw() {
        return NoteExpression {
            pitch_offset_cents: first.pitch_offset_cents,
            timbre: first.timbre,
            pressure: first.pressure,
        };
    }
    let Some(last) = points.last() else {
        return initial;
    };
    if offset.raw() >= last.offset.raw() {
        return NoteExpression {
            pitch_offset_cents: last.pitch_offset_cents,
            timbre: last.timbre,
            pressure: last.pressure,
        };
    }
    let idx = points.partition_point(|p| p.offset.raw() < offset.raw());
    let right = points[idx];
    if right.offset.raw() == offset.raw() {
        return NoteExpression {
            pitch_offset_cents: right.pitch_offset_cents,
            timbre: right.timbre,
            pressure: right.pressure,
        };
    }
    let left = points[idx.saturating_sub(1)];
    let span = right.offset.raw().saturating_sub(left.offset.raw()).max(1);
    let t = (offset.raw().saturating_sub(left.offset.raw()) as f64) / (span as f64);
    let lerp_i16 = |a: i16, b: i16| -> i16 {
        let v = (f64::from(a) + (f64::from(b) - f64::from(a)) * t).round() as i32;
        v.clamp(
            i32::from(MIN_NOTE_PITCH_OFFSET_CENTS),
            i32::from(MAX_NOTE_PITCH_OFFSET_CENTS),
        ) as i16
    };
    let lerp_u8 = |a: u8, b: u8| -> u8 {
        (f64::from(a) + (f64::from(b) - f64::from(a)) * t)
            .round()
            .clamp(0.0, 127.0) as u8
    };
    NoteExpression {
        pitch_offset_cents: lerp_i16(left.pitch_offset_cents, right.pitch_offset_cents),
        timbre: lerp_u8(left.timbre, right.timbre),
        pressure: lerp_u8(left.pressure, right.pressure),
    }
}

/// Insert or update one bounded per-note expression curve point at `offset`
/// (`0..=duration`) on the active MPE dimension (`NotePitch`, `NoteTimbre`, or
/// `NotePressure`), preserving other dimensions via finite linear interpolation.
/// Refuses out-of-range offsets, invalid values, or exceeding
/// `MAX_NOTE_EXPRESSION_POINTS` rather than silently discarding points.
pub(super) fn upsert_note_expression_point(
    initial: NoteExpression,
    existing: &[NoteExpressionPoint],
    offset: BeatTime,
    duration: BeatTime,
    mode: ExpressionLaneMode,
    edited: NoteExpression,
    merge_radius_raw: u64,
) -> Result<(NoteExpression, Vec<NoteExpressionPoint>), &'static str> {
    if !mode.is_mpe_note_mode() {
        return Err("unsupported per-note expression lane");
    }
    if offset > duration {
        return Err("note expression point offset exceeds note duration");
    }
    edited
        .validate()
        .map_err(|_| "note expression value is out of range")?;
    let mut points = existing.to_vec();
    let merge_idx = points
        .iter()
        .enumerate()
        .filter(|(_, p)| p.offset.raw().abs_diff(offset.raw()) <= merge_radius_raw)
        .min_by_key(|(_, p)| p.offset.raw().abs_diff(offset.raw()))
        .map(|(i, _)| i);
    let apply_dim = |base: NoteExpression| -> NoteExpression {
        let mut out = base;
        match mode {
            ExpressionLaneMode::NotePitch => out.pitch_offset_cents = edited.pitch_offset_cents,
            ExpressionLaneMode::NoteTimbre => out.timbre = edited.timbre,
            ExpressionLaneMode::NotePressure => out.pressure = edited.pressure,
            _ => {}
        }
        out
    };
    if let Some(index) = merge_idx {
        let existing_expr = NoteExpression {
            pitch_offset_cents: points[index].pitch_offset_cents,
            timbre: points[index].timbre,
            pressure: points[index].pressure,
        };
        let next = apply_dim(existing_expr);
        points[index] = NoteExpressionPoint {
            offset: points[index].offset.min(duration),
            pitch_offset_cents: next.pitch_offset_cents,
            timbre: next.timbre,
            pressure: next.pressure,
        };
    } else {
        if points.len() >= MAX_NOTE_EXPRESSION_POINTS {
            return Err("note expression curve reached point limit");
        }
        let interp = evaluate_note_expression_curve(initial, &points, offset);
        let next = apply_dim(interp);
        points.push(NoteExpressionPoint {
            offset,
            pitch_offset_cents: next.pitch_offset_cents,
            timbre: next.timbre,
            pressure: next.pressure,
        });
    }
    points.sort_by_key(|p| p.offset.raw());
    points.dedup_by(|later, earlier| {
        if later.offset == earlier.offset {
            *earlier = *later;
            true
        } else {
            false
        }
    });
    let next_initial = if points.len() == 1 || offset.raw() <= points[0].offset.raw() {
        apply_dim(initial)
    } else {
        initial
    };
    Ok((next_initial, points))
}

/// Remove the nearest per-note expression curve point within `hit_radius_raw`,
/// or reset the active MPE dimension to default if no curve points remain.
pub(super) fn remove_note_expression_point(
    initial: NoteExpression,
    existing: &[NoteExpressionPoint],
    offset: BeatTime,
    mode: ExpressionLaneMode,
    hit_radius_raw: u64,
) -> (NoteExpression, Vec<NoteExpressionPoint>) {
    let mut points = existing.to_vec();
    if let Some(index) = points
        .iter()
        .enumerate()
        .filter(|(_, p)| p.offset.raw().abs_diff(offset.raw()) <= hit_radius_raw)
        .min_by_key(|(_, p)| p.offset.raw().abs_diff(offset.raw()))
        .map(|(i, _)| i)
    {
        points.remove(index);
        let next_initial = points
            .first()
            .map(|p| NoteExpression {
                pitch_offset_cents: p.pitch_offset_cents,
                timbre: p.timbre,
                pressure: p.pressure,
            })
            .unwrap_or_else(|| {
                let mut reset = initial;
                match mode {
                    ExpressionLaneMode::NotePitch => reset.pitch_offset_cents = 0,
                    ExpressionLaneMode::NoteTimbre => reset.timbre = 0,
                    ExpressionLaneMode::NotePressure => reset.pressure = 0,
                    _ => {}
                }
                reset
            });
        return (next_initial, points);
    }
    let mut reset = initial;
    match mode {
        ExpressionLaneMode::NotePitch => reset.pitch_offset_cents = 0,
        ExpressionLaneMode::NoteTimbre => reset.timbre = 0,
        ExpressionLaneMode::NotePressure => reset.pressure = 0,
        _ => {}
    }
    (reset, points)
}

#[derive(Clone)]
struct NoteExprVisual {
    x_ratio: f32,
    end_x_ratio: f32,
    start_raw: u64,
    duration_raw: u64,
    expression: NoteExpression,
    points: Vec<NoteExpressionPoint>,
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
                    let start_raw = i128::from(note.start().raw());
                    let end_raw = start_raw.saturating_add(i128::from(note.duration().raw()));
                    let view_end = beat_start.saturating_add(i128::from(beat_span));
                    if end_raw < beat_start || start_raw > view_end {
                        return None;
                    }
                    let x_ratio = ((start_raw - beat_start) as f64 / beat_span as f64) as f32;
                    let end_x_ratio = ((end_raw - beat_start) as f64 / beat_span as f64) as f32;
                    let (expression, points) = self
                        .displayed_note_expression(clip, note.id())
                        .map(|(expr, pts)| (expr, pts.to_vec()))
                        .unwrap_or_else(|| (note.expression(), note.expression_points().to_vec()));
                    Some(NoteExprVisual {
                        x_ratio,
                        end_x_ratio,
                        start_raw: note.start().raw(),
                        duration_raw: note.duration().raw(),
                        expression,
                        points,
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
                                    let end_x = (note.end_x_ratio * width).clamp(0.0, width);
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
                                    // Paint continuous per-note expression curve across [start, end].
                                    if note.points.is_empty() {
                                        let span_w = (end_x - x).max(0.0);
                                        if span_w > 0.0 {
                                            quad(x, y, span_w, 1.5, color);
                                        }
                                    } else {
                                        let mut prev: Option<(f32, f32)> = None;
                                        for pt in &note.points {
                                            let pt_raw = i128::from(note.start_raw.saturating_add(
                                                pt.offset.raw().min(note.duration_raw),
                                            ));
                                            let px_x = ((pt_raw - beat_start) as f64
                                                / beat_span.max(1) as f64)
                                                as f32
                                                * width;
                                            let pt_expr = NoteExpression {
                                                pitch_offset_cents: pt.pitch_offset_cents,
                                                timbre: pt.timbre,
                                                pressure: pt.pressure,
                                            };
                                            let Some(py_y) = y_for_note_expr(mode, pt_expr, height)
                                            else {
                                                continue;
                                            };
                                            if let Some((lx, ly)) = prev {
                                                let steps = ((px_x - lx).abs() / 4.0)
                                                    .ceil()
                                                    .clamp(1.0, 32.0)
                                                    as usize;
                                                for s in 0..steps {
                                                    let t0 = s as f32 / steps as f32;
                                                    let t1 = (s + 1) as f32 / steps as f32;
                                                    let sx =
                                                        (lx + (px_x - lx) * t0).clamp(0.0, width);
                                                    let ex =
                                                        (lx + (px_x - lx) * t1).clamp(0.0, width);
                                                    let sy = ly + (py_y - ly) * ((t0 + t1) * 0.5);
                                                    quad(sx, sy, (ex - sx).max(1.0), 1.5, color);
                                                }
                                            }
                                            if px_x >= -4.0 && px_x <= width + 4.0 {
                                                quad(
                                                    (px_x - 2.5).clamp(0.0, width),
                                                    (py_y - 2.5).clamp(0.0, height),
                                                    5.0,
                                                    5.0,
                                                    color,
                                                );
                                            }
                                            prev = Some((px_x, py_y));
                                        }
                                        if let Some((lx, ly)) = prev {
                                            let tail_x = lx.clamp(0.0, width);
                                            let tail_w = (end_x - tail_x).max(0.0);
                                            if tail_w > 0.0 {
                                                quad(tail_x, ly, tail_w, 1.5, color);
                                            }
                                        }
                                    }
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
            let Some(clicked_beat) =
                beat_for_x(local_x, width, beat_start, beat_span, source_length)
            else {
                return;
            };
            let hit_beats = ((f64::from(POINT_HIT_RADIUS_PX) / f64::from(width.max(1.0)))
                * beat_span as f64)
                .round()
                .max(1.0) as u64;
            let selection = self.midi_editor.selection.shared_ids();
            let target_note = if selection.len() > 1 {
                self.status = "select a single note to edit per-note expression".into();
                return;
            } else if let Some(&selected_id) = selection.first() {
                let Some(note) = source.notes().iter().find(|n| n.id() == selected_id) else {
                    self.status = "selected note no longer exists in this clip".into();
                    return;
                };
                let note_start = note.start().raw();
                let note_end = note_start.saturating_add(note.duration().raw());
                if clicked_beat.raw().saturating_add(hit_beats) < note_start
                    || clicked_beat.raw() > note_end.saturating_add(hit_beats)
                {
                    self.status = "click within the selected note to edit its expression".into();
                    return;
                }
                note
            } else {
                let mut matches: Vec<&hex_project::MidiNote> = source
                    .notes()
                    .iter()
                    .filter(|note| {
                        let start = note.start().raw();
                        let end = start.saturating_add(note.duration().raw());
                        clicked_beat.raw().saturating_add(hit_beats) >= start
                            && clicked_beat.raw() <= end.saturating_add(hit_beats)
                    })
                    .collect();
                if matches.len() > 1 {
                    self.status =
                        "overlapping notes · select one note to edit per-note expression".into();
                    return;
                }
                let Some(note) = matches.pop() else {
                    return;
                };
                note
            };
            let note_id = target_note.id();
            let note_duration = target_note.duration();
            let offset = BeatTime::from_raw(
                clicked_beat
                    .raw()
                    .saturating_sub(target_note.start().raw())
                    .min(note_duration.raw()),
            );
            let (current_expr, current_points) = self
                .displayed_note_expression(clip, note_id)
                .map(|(expr, pts)| (expr, pts.to_vec()))
                .unwrap_or_else(|| {
                    (
                        target_note.expression(),
                        target_note.expression_points().to_vec(),
                    )
                });
            let (updated_expr, updated_points) = if is_remove {
                remove_note_expression_point(current_expr, &current_points, offset, mode, hit_beats)
            } else {
                let Some(next) = note_expr_for_y(mode, current_expr, local_y, height) else {
                    return;
                };
                match upsert_note_expression_point(
                    current_expr,
                    &current_points,
                    offset,
                    note_duration,
                    mode,
                    next,
                    hit_beats,
                ) {
                    Ok(result) => result,
                    Err(reason) => {
                        self.error = Some(reason.into());
                        return;
                    }
                }
            };
            if updated_expr != current_expr || updated_points != current_points {
                let preview = super::midi_pending_drag::Frozen::NoteExpression {
                    note: note_id,
                    expression: updated_expr,
                    points: updated_points.clone(),
                };
                let command = Command::SetMidiNoteExpression {
                    clip,
                    note: note_id,
                    expression: updated_expr,
                    points: updated_points,
                };
                self.submit_midi_drag_preview(clip, preview, command);
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

    #[test]
    fn finite_note_expression_curve_interpolation_and_bounded_edits() {
        let dur = BeatTime::from_quarters(2).unwrap();
        let initial = NoteExpression {
            pitch_offset_cents: 100,
            timbre: 20,
            pressure: 10,
        };
        // Empty curve holds initial expression.
        assert_eq!(
            evaluate_note_expression_curve(initial, &[], BeatTime::ONE),
            initial
        );

        let (init1, pts1) = upsert_note_expression_point(
            initial,
            &[],
            BeatTime::ZERO,
            dur,
            ExpressionLaneMode::NotePitch,
            NoteExpression {
                pitch_offset_cents: 200,
                ..initial
            },
            10,
        )
        .unwrap();
        assert_eq!(init1.pitch_offset_cents, 200);
        assert_eq!(pts1.len(), 1);

        let (init2, pts2) = upsert_note_expression_point(
            init1,
            &pts1,
            dur,
            dur,
            ExpressionLaneMode::NotePitch,
            NoteExpression {
                pitch_offset_cents: 600,
                ..init1
            },
            10,
        )
        .unwrap();
        assert_eq!(init2.pitch_offset_cents, 200);
        assert_eq!(pts2.len(), 2);
        // Linear midpoint at offset = 1.0 beat -> (200 + 600) / 2 = 400 cents!
        assert_eq!(
            evaluate_note_expression_curve(init2, &pts2, BeatTime::ONE).pitch_offset_cents,
            400
        );

        // Out-of-range offset (> duration) and invalid pitch bend (> 4800) must refuse explicitly.
        assert!(
            upsert_note_expression_point(
                init2,
                &pts2,
                BeatTime::from_quarters(3).unwrap(),
                dur,
                ExpressionLaneMode::NotePitch,
                init2,
                10,
            )
            .is_err()
        );
        assert!(
            upsert_note_expression_point(
                init2,
                &pts2,
                BeatTime::ONE,
                dur,
                ExpressionLaneMode::NotePitch,
                NoteExpression {
                    pitch_offset_cents: 5_000,
                    ..init2
                },
                10,
            )
            .is_err()
        );
        // Removing point at offset 0 promotes second point to initial.
        let (init3, pts3) = remove_note_expression_point(
            init2,
            &pts2,
            BeatTime::ZERO,
            ExpressionLaneMode::NotePitch,
            10,
        );
        assert_eq!(pts3.len(), 1);
        assert_eq!(init3.pitch_offset_cents, 600);
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_expression_ui_tests.rs"]
mod ui_tests;
