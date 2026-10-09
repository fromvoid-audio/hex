//! UI-only snap selection and bounded musical timeline helpers.

use super::viewport::Viewport;
use crate::theme::{self, Palette};
use gpui::{Context, Div, Window, div, prelude::*, px, rgb};
use hex_project::{BeatTime, Command, Meter, MeterPosition, Project, Tempo};
use hex_session::service::Request;

const MAX_RULER_TICKS: usize = 64;
const MAX_METER_MAP_POINTS: usize = 1_024;
const RULER_LABEL_SPACING_PX: f32 = 48.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RulerTick {
    pub(super) frame: u64,
    pub(super) label: String,
    pub(super) meter_marker: bool,
    pub(super) fragmentary_bar: bool,
}

pub(super) fn ruler_ticks(
    project: &Project,
    viewport: Viewport,
    pixel_width: f32,
) -> Vec<RulerTick> {
    if viewport.length == 0 || !pixel_width.is_finite() || pixel_width <= 0.0 {
        return Vec::new();
    }
    let Some(viewport_end) = viewport.end() else {
        return Vec::new();
    };
    let last_frame = viewport_end.saturating_sub(1);
    let tempo = project.tempo_map();
    let Ok(start_beat) = tempo.frame_to_beat(viewport.start) else {
        return Vec::new();
    };
    // A frame viewport can extend beyond BeatTime's representable range. Clamp
    // its far edge so valid landmarks and markers earlier in the viewport remain.
    let last_beat = tempo
        .frame_to_beat(last_frame)
        .unwrap_or_else(|_| BeatTime::from_raw(u64::MAX));
    let target_count = ((pixel_width / RULER_LABEL_SPACING_PX).floor() as usize)
        .saturating_add(1)
        .clamp(1, MAX_RULER_TICKS);
    let ruler_candidates = ruler_candidates(
        project,
        viewport,
        viewport_end,
        start_beat,
        last_beat,
        target_count,
    );
    let mut candidates = ruler_candidates.ticks;
    candidates.sort_by_key(|tick| tick.frame);
    candidates.dedup_by(|later, earlier| {
        if later.frame == earlier.frame {
            earlier.meter_marker |= later.meter_marker;
            earlier.fragmentary_bar |= later.fragmentary_bar;
            if later.meter_marker {
                earlier.label.clone_from(&later.label);
            }
            true
        } else {
            false
        }
    });

    let limit = target_count.min(MAX_RULER_TICKS);
    let mut bounded = Vec::with_capacity(limit);
    for tick in candidates.iter().filter(|tick| tick.meter_marker) {
        let x = (tick.frame - viewport.start) as f32 / viewport.length as f32 * pixel_width;
        if bounded.iter().any(|selected: &RulerTick| {
            let selected_x =
                (selected.frame - viewport.start) as f32 / viewport.length as f32 * pixel_width;
            (x - selected_x).abs() < RULER_LABEL_SPACING_PX
        }) {
            continue;
        }
        if bounded.len() >= limit {
            break;
        }
        bounded.push(tick.clone());
    }
    for tick in candidates.iter().filter(|tick| !tick.meter_marker) {
        let x = (tick.frame - viewport.start) as f32 / viewport.length as f32 * pixel_width;
        if bounded.iter().any(|selected: &RulerTick| {
            let selected_x =
                (selected.frame - viewport.start) as f32 / viewport.length as f32 * pixel_width;
            (x - selected_x).abs() < RULER_LABEL_SPACING_PX
        }) {
            continue;
        }
        if bounded.len() >= limit {
            break;
        }
        bounded.push(tick.clone());
    }
    bounded.sort_by_key(|tick| tick.frame);
    bounded
}

#[derive(Clone, Copy)]
struct MeterSection {
    point_beat: BeatTime,
    beat_length: u64,
    first_landmark: u64,
    landmark_count: u64,
}

struct RulerCandidates {
    ticks: Vec<RulerTick>,
    #[cfg(test)]
    landmark_attempts: usize,
    #[cfg(test)]
    marker_attempts: usize,
}

fn ruler_candidates(
    project: &Project,
    viewport: Viewport,
    viewport_end: u64,
    start_beat: BeatTime,
    last_beat: BeatTime,
    target_count: usize,
) -> RulerCandidates {
    let tempo = project.tempo_map();
    let meters = project.meter_map();
    let points = meters.points();
    let mut sections = Vec::with_capacity(points.len().min(MAX_METER_MAP_POINTS));
    let mut ticks = Vec::with_capacity(target_count.saturating_add(points.len()));
    for (index, point) in points.iter().take(MAX_METER_MAP_POINTS).enumerate() {
        let point_raw = point.beat().raw();
        let section_end = points.get(index + 1).map(|next| next.beat().raw());
        if point_raw <= last_beat.raw() && !section_end.is_some_and(|end| end <= start_beat.raw()) {
            let effective_start = start_beat.raw().max(point_raw);
            let effective_end = section_end
                .and_then(|end| end.checked_sub(1))
                .unwrap_or(last_beat.raw())
                .min(last_beat.raw());
            if effective_start <= effective_end {
                let beat_length =
                    (4_u64 * BeatTime::ONE.raw()) / u64::from(point.meter().denominator());
                if beat_length > 0 {
                    let first_landmark = effective_start
                        .saturating_sub(point_raw)
                        .div_ceil(beat_length);
                    let last_landmark = effective_end.saturating_sub(point_raw) / beat_length;
                    if first_landmark <= last_landmark {
                        sections.push(MeterSection {
                            point_beat: point.beat(),
                            beat_length,
                            first_landmark,
                            landmark_count: last_landmark - first_landmark + 1,
                        });
                    }
                }
            }
        }
        // Markers are considered even when sub-quarter or quantized frame
        // positions make inverse lookup ambiguous.
        if index > 0
            && let Ok(frame) = tempo.beat_to_frame(point.beat())
            && (viewport.start..viewport_end).contains(&frame)
            && let Ok(position) = meters.position(point.beat())
        {
            let truncated_prior_bar = point
                .beat()
                .raw()
                .checked_sub(1)
                .map(BeatTime::from_raw)
                .and_then(|before| meters.position(before).ok())
                .is_some_and(|before| {
                    before.fragmentary() && before.bar_end() == Some(point.beat())
                });
            ticks.push(RulerTick {
                frame,
                label: meter_label(position),
                meter_marker: true,
                fragmentary_bar: truncated_prior_bar,
            });
        }
    }

    let section_count = sections.len();
    let sampled_sections = section_count.min(target_count);
    #[cfg(test)]
    let mut landmark_attempts = 0;
    if sampled_sections > 0 {
        for sampled_index in 0..sampled_sections {
            // Spread the quota evenly across visible meter sections. This keeps a
            // long, dense section from consuming the ruler's whole tick budget.
            let section_index = if sampled_sections == section_count {
                sampled_index
            } else if sampled_sections == 1 {
                section_count / 2
            } else {
                sampled_index * (section_count - 1) / (sampled_sections - 1)
            };
            let Some(section) = sections.get(section_index).copied() else {
                continue;
            };
            let quota = target_count / sampled_sections
                + usize::from(sampled_index < target_count % sampled_sections);
            let quota = u64::try_from(quota)
                .unwrap_or(u64::MAX)
                .min(section.landmark_count);
            for sample in 0..quota {
                let offset = if quota <= 1 {
                    0
                } else {
                    let Ok(offset) = u64::try_from(
                        u128::from(sample) * u128::from(section.landmark_count - 1)
                            / u128::from(quota - 1),
                    ) else {
                        continue;
                    };
                    offset
                };
                let Some(landmark_index) = section.first_landmark.checked_add(offset) else {
                    continue;
                };
                let raw_beat = u128::from(section.point_beat.raw())
                    + u128::from(landmark_index) * u128::from(section.beat_length);
                let Ok(raw_beat) = u64::try_from(raw_beat) else {
                    continue;
                };
                let beat = BeatTime::from_raw(raw_beat);
                #[cfg(test)]
                {
                    landmark_attempts += 1;
                }
                if let (Ok(frame), Ok(position)) =
                    (tempo.beat_to_frame(beat), meters.position(beat))
                    && (viewport.start..viewport_end).contains(&frame)
                {
                    ticks.push(RulerTick {
                        frame,
                        label: meter_label(position),
                        meter_marker: false,
                        fragmentary_bar: position.fragmentary(),
                    });
                }
            }
        }
    }

    RulerCandidates {
        ticks,
        #[cfg(test)]
        landmark_attempts,
        #[cfg(test)]
        marker_attempts: points.len().min(MAX_METER_MAP_POINTS),
    }
}

fn meter_label(position: MeterPosition) -> String {
    let fraction = position.beat_fraction().raw();
    if fraction == 0 {
        format!("{}.{}", position.bar(), position.beat())
    } else {
        let tenths = (u128::from(fraction) * 10 / u128::from(BeatTime::ONE.raw())) as u8;
        format!("{}.{}+{}", position.bar(), position.beat(), tenths)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum SnapGrid {
    #[default]
    Off,
    Quarter,
    Eighth,
    Sixteenth,
}

impl SnapGrid {
    pub(super) const fn next(self) -> Self {
        match self {
            Self::Off => Self::Quarter,
            Self::Quarter => Self::Eighth,
            Self::Eighth => Self::Sixteenth,
            Self::Sixteenth => Self::Off,
        }
    }

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Quarter => "1/4",
            Self::Eighth => "1/8",
            Self::Sixteenth => "1/16",
        }
    }

    /// Q32.32 quarter-note interval; the canonical project map performs all
    /// frame conversion and nearest-grid tie handling.
    pub(super) const fn step_raw(self) -> Option<u64> {
        const QUARTER: u64 = 1_u64 << 32;
        match self {
            Self::Off => None,
            Self::Quarter => Some(QUARTER),
            Self::Eighth => Some(QUARTER / 2),
            Self::Sixteenth => Some(QUARTER / 4),
        }
    }

    pub(super) fn snap_frame(self, project: &Project, candidate_frame: u64) -> Option<u64> {
        let Some(raw) = self.step_raw() else {
            return Some(candidate_frame);
        };
        let grid = BeatTime::from_raw(raw);
        let tempo = project.tempo_map();
        let beat = tempo.frame_to_beat(candidate_frame).ok()?;
        let snapped = beat.snap_to(grid).ok()?;
        tempo.beat_to_frame(snapped).ok()
    }
}

impl super::EditorView {
    pub(super) fn cycle_snap_grid(&mut self) {
        self.snap_grid = self.snap_grid.next();
        self.status = format!("clip move · snap {}", self.snap_grid.label());
    }

    fn cursor_beat(&self) -> Option<BeatTime> {
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

    pub(super) fn adjust_tempo(&mut self, direction: i32) {
        if !self.time_edit_admitted() {
            self.error = Some("tempo edit waits for pending project work".into());
            return;
        }
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let Some(beat) = self.cursor_beat() else {
            self.error = Some("tempo point unavailable at this cursor position".into());
            return;
        };
        let current = state.project.tempo_map().tempo_at(beat);
        let adjusted = i64::from(current.micro_bpm()) + i64::from(direction) * 1_000_000;
        let Ok(micro_bpm) = u32::try_from(adjusted) else {
            self.error = Some("tempo must stay within 1–1000 bpm".into());
            return;
        };
        let Ok(tempo) = Tempo::new(micro_bpm) else {
            self.error = Some("tempo must stay within 1–1000 bpm".into());
            return;
        };
        self.enqueue(Request::Edit(Command::SetTempoPoint { beat, tempo }));
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
            return div()
                .w(px(super::tracks::TRACK_HEADER_WIDTH))
                .flex_shrink_0();
        };
        let beat = self.cursor_beat();
        let tempo_label = beat.map_or_else(
            || "—".into(),
            |beat| {
                let micro = state.project.tempo_map().tempo_at(beat).micro_bpm();
                let tenths = (micro + 50_000) / 100_000;
                format!("{}.{:01}", tenths / 10, tenths % 10)
            },
        );
        let meter_label = beat.map_or_else(
            || "—".into(),
            |beat| {
                let meter = state.project.meter_map().meter_at(beat);
                format!("{}/{}", meter.numerator(), meter.denominator())
            },
        );
        let enabled = self.time_edit_admitted() && beat.is_some();
        div()
            .w(px(super::tracks::TRACK_HEADER_WIDTH))
            .h_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(2.0))
            .children([
                self.time_button(
                    "−",
                    "decrease tempo at cursor quarter by 1 bpm",
                    enabled,
                    |view, _, _| view.adjust_tempo(-1),
                    cx,
                ),
                self.time_readout(tempo_label, "tempo at cursor snapped quarter · bpm"),
                self.time_button(
                    "+",
                    "increase tempo at cursor quarter by 1 bpm",
                    enabled,
                    |view, _, _| view.adjust_tempo(1),
                    cx,
                ),
            ])
            .child(div().w(px(1.0)).h(px(14.0)).mx_1().bg(rgb(Palette::BORDER)))
            .children([self.time_button(
                &meter_label,
                "cycle meter at cursor snapped quarter · 4/4, 3/4, 6/8, 5/4",
                enabled,
                |view, _, _| view.cycle_meter(),
                cx,
            )])
    }

    fn time_readout(&self, label: String, description: &'static str) -> gpui::Stateful<Div> {
        div()
            .id(description)
            .w(px(38.0))
            .h(px(19.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .text_xs()
            .text_color(rgb(Palette::TEXT))
            .tooltip(move |_, cx| {
                cx.new(|_| super::transport::MiniTooltip {
                    description: description.into(),
                })
                .into()
            })
            .child(theme::text(label))
    }

    fn time_button(
        &self,
        label: &str,
        description: &'static str,
        enabled: bool,
        handler: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        div()
            .id(description)
            .w(px(20.0))
            .h(px(19.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .text_xs()
            .bg(rgb(Palette::RAISED))
            .text_color(rgb(Palette::TEXT))
            .opacity(if enabled { 1.0 } else { 0.45 })
            .hover(|style| style.bg(rgb(Palette::HOVER)))
            .tooltip(move |_, cx| {
                cx.new(|_| super::transport::MiniTooltip {
                    description: description.into(),
                })
                .into()
            })
            .on_click(cx.listener(move |view, _, window, cx| {
                handler(view, window, cx);
                cx.notify();
            }))
            .child(theme::text(label))
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_METER_MAP_POINTS, MAX_RULER_TICKS, SnapGrid, ruler_candidates, ruler_ticks};
    use crate::editor::viewport::Viewport;
    use hex_project::{BeatTime, Command, Editor, Meter, Project, Tempo};

    #[test]
    fn snap_choice_is_local_opt_in_and_cycles_back_to_off() {
        assert_eq!(SnapGrid::default(), SnapGrid::Off);
        assert_eq!(SnapGrid::Off.next(), SnapGrid::Quarter);
        assert_eq!(SnapGrid::Quarter.next(), SnapGrid::Eighth);
        assert_eq!(SnapGrid::Eighth.next(), SnapGrid::Sixteenth);
        assert_eq!(SnapGrid::Sixteenth.next(), SnapGrid::Off);
        assert_eq!(SnapGrid::Off.step_raw(), None);
        assert_eq!(SnapGrid::Quarter.step_raw(), Some(1_u64 << 32));
        assert_eq!(SnapGrid::Eighth.step_raw(), Some(1_u64 << 31));
        assert_eq!(SnapGrid::Sixteenth.step_raw(), Some(1_u64 << 30));
    }

    #[test]
    fn snap_resolution_uses_canonical_map_and_rounds_exact_ties_later() {
        let project = Project::new(48_000).expect("project");
        let tie_frame = project
            .tempo_map()
            .beat_to_frame(BeatTime::from_ratio(1, 2).expect("half beat"))
            .expect("frame");
        assert_eq!(
            SnapGrid::Off.snap_frame(&project, tie_frame),
            Some(tie_frame)
        );
        assert_eq!(
            SnapGrid::Quarter.snap_frame(&project, tie_frame),
            Some(24_000)
        );

        let eighth_tie_frame = project
            .tempo_map()
            .beat_to_frame(BeatTime::from_ratio(3, 4).expect("eighth tie"))
            .expect("frame");
        assert_eq!(
            SnapGrid::Eighth.snap_frame(&project, eighth_tie_frame),
            Some(24_000)
        );
        assert_eq!(SnapGrid::Quarter.snap_frame(&project, u64::MAX), None);
    }

    #[test]
    fn bounded_ruler_marks_quarter_positions_and_fragmentary_meter_changes() {
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 32).expect("editor");
        let beat = BeatTime::from_ratio(3, 2).expect("meter beat");
        let meter = Meter::new(3, 4).expect("meter");
        let _ = editor
            .execute(Command::SetMeterPoint { beat, meter })
            .expect("meter point");
        assert_eq!(editor.project().meter_map().points().len(), 2);

        let ticks = ruler_ticks(
            editor.project(),
            Viewport {
                start: 0,
                length: 120_000,
            },
            600.0,
        );
        assert!(ticks.len() <= 13);
        let marker = ticks
            .iter()
            .find(|tick| tick.meter_marker)
            .expect("visible meter marker");
        assert_eq!(marker.frame, 36_000);
        assert!(marker.fragmentary_bar);
        assert_eq!(marker.label, "2.1");
    }

    #[test]
    fn six_eight_ruler_uses_denominator_beats() {
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 32).expect("editor");
        let _ = editor
            .execute(Command::SetMeterPoint {
                beat: BeatTime::ZERO,
                meter: Meter::new(6, 8).expect("meter"),
            })
            .expect("meter point");

        let ticks = ruler_ticks(
            editor.project(),
            Viewport {
                start: 0,
                length: 80_000,
            },
            600.0,
        );
        assert!(
            ticks
                .iter()
                .any(|tick| tick.frame == 12_000 && tick.label == "1.2")
        );
        assert!(
            ticks
                .iter()
                .any(|tick| tick.frame == 24_000 && tick.label == "1.3")
        );
    }

    #[test]
    fn fractional_meter_section_starts_its_denominator_beat_landmarks() {
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 32).expect("editor");
        let _ = editor
            .execute(Command::SetMeterPoint {
                beat: BeatTime::from_ratio(1, 2).expect("fractional point"),
                meter: Meter::new(3, 4).expect("meter"),
            })
            .expect("meter point");

        let ticks = ruler_ticks(
            editor.project(),
            Viewport {
                start: 0,
                length: 100_000,
            },
            600.0,
        );
        assert!(
            ticks
                .iter()
                .any(|tick| tick.frame == 12_000 && tick.label == "2.1")
        );
        assert!(
            ticks
                .iter()
                .any(|tick| tick.frame == 36_000 && tick.label == "2.2")
        );
    }

    #[test]
    fn huge_fractional_meter_viewport_keeps_tick_attempts_and_markers_bounded() {
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 32).expect("editor");
        let beat = BeatTime::from_ratio(1, 16).expect("fractional meter point");
        let meter = Meter::new(1, 1).expect("meter");
        let _ = editor
            .execute(Command::SetMeterPoint { beat, meter })
            .expect("meter point");

        let viewport = Viewport {
            start: 0,
            length: 1_000_000_000_000,
        };
        let viewport_end = viewport.end().expect("viewport end");
        let start_beat = editor
            .project()
            .tempo_map()
            .frame_to_beat(viewport.start)
            .expect("start beat");
        let last_beat = editor
            .project()
            .tempo_map()
            .frame_to_beat(viewport_end - 1)
            .unwrap_or_else(|_| BeatTime::from_raw(u64::MAX));
        let candidates = ruler_candidates(
            editor.project(),
            viewport,
            viewport_end,
            start_beat,
            last_beat,
            13,
        );
        assert!(candidates.landmark_attempts <= 13);
        assert!(candidates.marker_attempts <= 1_024);
        let ticks = ruler_ticks(editor.project(), viewport, 600.0);
        assert!(ticks.len() <= 13);
        assert!(
            ticks
                .iter()
                .any(|tick| tick.meter_marker && tick.frame == 1_500)
        );

        let sub_quarter_ticks = ruler_ticks(
            editor.project(),
            Viewport {
                start: 1_400,
                length: 200,
            },
            200.0,
        );
        assert!(
            sub_quarter_ticks
                .iter()
                .any(|tick| tick.meter_marker && tick.frame == 1_500)
        );
    }

    #[test]
    fn ruler_landmarks_follow_tempo_boundaries_and_deduplicate_quantized_markers() {
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 32).expect("editor");
        let _ = editor
            .execute(Command::SetTempoPoint {
                beat: BeatTime::from_quarters(2).expect("tempo beat"),
                tempo: Tempo::new(60_000_000).expect("tempo"),
            })
            .expect("tempo point");
        let ticks = ruler_ticks(
            editor.project(),
            Viewport {
                start: 0,
                length: 150_000,
            },
            600.0,
        );
        assert!(
            ticks
                .iter()
                .any(|tick| tick.frame == 48_000 && tick.label == "1.3")
        );
        assert!(
            ticks
                .iter()
                .any(|tick| tick.frame == 96_000 && tick.label == "1.4")
        );

        let mut editor = Editor::new(Project::new(48_000).expect("project"), 32).expect("editor");
        for (raw, meter) in [
            (1, Meter::new(3, 4).expect("first meter")),
            (2, Meter::new(6, 8).expect("second meter")),
        ] {
            let _ = editor
                .execute(Command::SetMeterPoint {
                    beat: BeatTime::from_raw(raw),
                    meter,
                })
                .expect("meter point");
        }
        let ticks = ruler_ticks(
            editor.project(),
            Viewport {
                start: 0,
                length: 200,
            },
            200.0,
        );
        let at_zero: Vec<_> = ticks.iter().filter(|tick| tick.frame == 0).collect();
        assert_eq!(at_zero.len(), 1);
        assert!(at_zero[0].meter_marker);
        assert_eq!(at_zero[0].label, "3.1");
    }

    #[test]
    fn full_meter_map_keeps_marker_priority_spacing_and_tick_cap() {
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 32).expect("editor");
        let meter = Meter::new(3, 4).expect("meter");
        for index in 1..MAX_METER_MAP_POINTS {
            let beat = BeatTime::from_ratio(index as u64, 64).expect("meter beat");
            let _ = editor
                .execute(Command::SetMeterPoint { beat, meter })
                .expect("meter point");
        }
        let viewport = Viewport {
            start: 0,
            length: 384_000,
        };
        let viewport_end = viewport.end().expect("viewport end");
        let start_beat = BeatTime::ZERO;
        let last_beat = editor
            .project()
            .tempo_map()
            .frame_to_beat(viewport_end - 1)
            .expect("last beat");
        let candidates = ruler_candidates(
            editor.project(),
            viewport,
            viewport_end,
            start_beat,
            last_beat,
            MAX_RULER_TICKS,
        );
        assert_eq!(candidates.marker_attempts, MAX_METER_MAP_POINTS);
        assert!(candidates.landmark_attempts <= MAX_RULER_TICKS);

        let ticks = ruler_ticks(editor.project(), viewport, 6_400.0);
        assert_eq!(ticks.len(), MAX_RULER_TICKS);
        assert!(ticks.iter().all(|tick| tick.meter_marker));
        assert!(ticks.windows(2).all(|pair| {
            let left = (pair[0].frame as f64 / viewport.length as f64) * 6_400.0;
            let right = (pair[1].frame as f64 / viewport.length as f64) * 6_400.0;
            right - left >= 48.0
        }));
    }
}
