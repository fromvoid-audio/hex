//! Clip-volume graph adapter. Project owns validated curves/history; Service
//! owns prepare-before-commit. Held input is local, and released display uses
//! the same ordered receipt as note edits until canonical state is installed.
use super::{EditorView, midi_pending_drag::Frozen};
use crate::theme;
use gpui::{Context, Div, Entity, SharedString, Subscription, div, prelude::*, px};
use hex_project::{
    BeatTime, ClipId, Command, MidiClipGain, MidiClipVolume, MidiClipVolumePoint, Project,
};
use hex_widgets::{
    ResponseGraph, ResponseGraphAxes, ResponseGraphEvent, ResponseGraphModel, ResponseGraphNode,
    ResponseGraphPhase as Phase,
};

#[derive(Default)]
pub(super) struct State {
    pub enabled: bool,
    graph: Option<Entity<ResponseGraph>>,
    subscription: Option<Subscription>,
    gesture: Option<Gesture>,
    selected: u32,
}
struct Gesture {
    clip: ClipId,
    generation: u64,
    revision: u64,
    start: u64,
    span: u64,
    length: BeatTime,
    grid: super::midi_drag::MidiGrid,
    original: MidiClipVolume,
    preview: MidiClipVolume,
    point: usize,
}

pub(super) fn curve(project: &Project, clip: ClipId) -> Option<&MidiClipVolume> {
    project
        .midi_clip(clip)
        .map(|c| c.volume())
        .or_else(|| project.session_midi_clip(clip).map(|c| c.volume()))
}

// Read the graph's effective curve, including its held/released preview. Point
// ids are one-based; cancellation, deletion or source replacement may leave no
// corresponding point. This readout never supplies values back to an edit.
fn point_values(volume: &MidiClipVolume, selected: u32) -> Option<String> {
    let point = volume.points().get(selected.checked_sub(1)? as usize)?;

    Some(format!(
        "beat {} · gain {}",
        compact_number(point.beat.raw() as f64 / BeatTime::ONE.raw() as f64),
        compact_number(point.value.linear()),
    ))
}

fn compact_number(value: f64) -> String {
    if (value != 0.0 && value < 0.0001) || value >= 1_000_000.0 {
        format!("{value:.3e}")
    } else {
        format!("{value:.4}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned()
    }
}

// Grid guides use source beats, just like point snapping. Coarsen by powers of
// two at wide zoom; keep at most 32 axis entries and let the shared graph suppress
// overlapping labels. This changes only display density, never the snap grid.
fn beat_axes(start: u64, span: u64, grid: super::midi_drag::MidiGrid) -> Vec<(f32, SharedString)> {
    let Some(end) = start.checked_add(span).filter(|_| span > 0) else {
        return Vec::new();
    };
    let label = |raw| {
        theme::text(format!(
            "{}b",
            compact_number(raw as f64 / BeatTime::ONE.raw() as f64)
        ))
    };
    let mut ticks = vec![(0.0, label(start))];
    let mut step = grid.step_raw().unwrap_or(BeatTime::ONE.raw());
    while span / step > 28 {
        let Some(next) = step.checked_mul(2) else {
            break;
        };
        step = next;
    }
    let mut next = (start / step)
        .checked_add(1)
        .and_then(|v| v.checked_mul(step));
    for _ in 0..30 {
        let Some(raw) = next.filter(|raw| *raw < end) else {
            break;
        };
        ticks.push((((raw - start) as f64 / span as f64) as f32, label(raw)));
        next = raw.checked_add(step);
    }
    ticks.push((1.0, label(end)));
    ticks
}

fn point_at(
    position: [f32; 2],
    start: u64,
    span: u64,
    length: BeatTime,
    grid: super::midi_drag::MidiGrid,
    bypass: bool,
) -> Option<MidiClipVolumePoint> {
    if !position
        .iter()
        .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
        || span == 0
    {
        return None;
    }
    let offset = (f64::from(position[0]) * span as f64).round() as u64;
    let raw = start.checked_add(offset)?.min(length.raw());
    let beat = super::midi_drag::snap_raw(raw, super::midi_drag::effective_grid(grid, bypass))
        .min(length.raw());
    let gain = MidiClipGain::new(f64::from(position[1]) * 4.0).ok()?;
    Some(MidiClipVolumePoint::linear(BeatTime::from_raw(beat), gain))
}

fn moved(gesture: &Gesture, mut point: MidiClipVolumePoint) -> Option<MidiClipVolume> {
    let mut points = gesture.original.points().to_vec();
    if gesture.point >= points.len() {
        return None;
    }
    let min = gesture
        .point
        .checked_sub(1)
        .map_or(0, |i| points[i].beat.raw().saturating_add(1));
    let max = points
        .get(gesture.point + 1)
        .map_or(gesture.length.raw(), |p| p.beat.raw().saturating_sub(1));
    if min > max {
        return None;
    }
    point.beat = BeatTime::from_raw(point.beat.raw().clamp(min, max));
    // A translated boundary keeps an authored incoming/value discontinuity.
    // Vertical motion changes both limits by the same bounded gain delta.
    let old = points[gesture.point];
    let delta = point.value.linear() - old.value.linear();
    point.incoming = MidiClipGain::new((old.incoming.linear() + delta).clamp(0.0, 4.0)).ok()?;
    points[gesture.point] = point;
    MidiClipVolume::new(points, gesture.length).ok()
}

impl EditorView {
    pub(super) fn volume_mode_button(&self, cx: &mut Context<Self>) -> Div {
        let label = if self.midi_volume.enabled {
            "notes"
        } else {
            "envelopes"
        };
        div().children([self
            .midi_button_with_id(
                "midi-volume-mode",
                label,
                "switch between notes and clip volume envelope",
                cx.listener(|view, _, window, cx| {
                    view.cancel_midi_note_gesture();
                    view.cancel_midi_audition();
                    view.cancel_computer_keyboard();
                    view.focus.focus(window);
                    view.midi_volume.enabled = !view.midi_volume.enabled;
                    if let Some(graph) = view.midi_volume.graph.clone() {
                        graph.update(cx, |g, cx| g.cancel(cx));
                    }
                    cx.notify();
                }),
            )
            .w_auto()
            .px_2()])
    }
    pub(super) fn cancel_midi_volume_input(&mut self) -> bool {
        self.midi_volume.gesture.take().is_some()
    }
    pub(super) fn displayed_midi_volume(&self, clip: ClipId) -> Option<MidiClipVolume> {
        if let Some(g) = &self.midi_volume.gesture
            && self.volume_gesture_current(g)
            && g.clip == clip
        {
            return Some(g.preview.clone());
        }
        self.pending_midi_volume(clip)
            .or_else(|| curve(&self.state.as_ref()?.project, clip).cloned())
    }
    fn volume_gesture_current(&self, g: &Gesture) -> bool {
        self.midi_volume.enabled
            && self.project_generation == g.generation
            && self.detail_clip_id() == Some(g.clip)
            && self
                .state
                .as_ref()
                .is_some_and(|s| s.revision == g.revision)
            && self.piano_beat_start.raw() == g.start
            && self.piano_beat_span.raw() == g.span
            && !self.edit_pending()
    }
    fn volume_submit(&mut self, clip: ClipId, volume: MidiClipVolume) {
        let Some(original) = self.state.as_ref().and_then(|s| curve(&s.project, clip)) else {
            return;
        };
        if original == &volume {
            return;
        }
        let command = Command::ReplaceMidiClipVolume {
            clip,
            volume: volume.clone(),
        };
        self.submit_midi_drag_preview(clip, Frozen::Volume(volume), command);
    }
    pub(super) fn midi_volume_editor(&mut self, cx: &mut Context<Self>) -> Option<Div> {
        let clip = self.detail_clip_id()?;
        self.state.as_ref()?.project.midi_source(clip)?;
        if self
            .midi_volume
            .gesture
            .as_ref()
            .is_some_and(|g| !self.volume_gesture_current(g))
        {
            self.cancel_midi_volume_input();
            if let Some(graph) = self.midi_volume.graph.clone() {
                graph.update(cx, |g, cx| g.cancel(cx));
            }
        }
        if self.midi_volume.graph.is_none() {
            let graph = cx.new(|cx| {
                let mut g = ResponseGraph::new(
                    cx,
                    "midi-volume",
                    theme::device_style(theme::DeviceStyleRole::Display),
                );
                g.set_creation_enabled(true);
                g.set_creation_drag_enabled(true);
                g.set_backspace_removal(true);
                g.set_tab_navigation(true, cx);
                g
            });
            let subscription = cx.subscribe(&graph, |view, _, event: &ResponseGraphEvent, cx| {
                view.midi_volume_event(*event, cx);
            });
            self.midi_volume.graph = Some(graph);
            self.midi_volume.subscription = Some(subscription);
        }
        let volume = self.displayed_midi_volume(clip)?;
        let start = self.piano_beat_start.raw();
        let span = self.piano_beat_span.raw().max(1);
        let model = model(&volume, start, span, self.midi_volume.selected);
        let graph = self.midi_volume.graph.clone()?;
        let editable = !self.edit_pending();
        let axes = ResponseGraphAxes {
            horizontal: beat_axes(start, span, self.midi_snap_grid),
            vertical: vec![
                (0.0, theme::text("0")),
                (0.25, theme::text("1")),
                (1.0, theme::text("4")),
            ],
            right: vec![],
        };
        let error = graph
            .update(cx, |g, cx| {
                g.set_editable(editable, cx);
                g.set_axes(Some(axes), cx)?;
                g.set_model(
                    model,
                    theme::device_style(theme::DeviceStyleRole::Display),
                    cx,
                )
            })
            .err();
        if let Some(error) = error {
            self.error = Some(error.into());
        }
        let generation = self.project_generation;
        let revision = self.state.as_ref()?.revision;
        let grid = self.midi_snap_grid;
        let mode_control = self.volume_mode_button(cx);
        let toolbar = div()
            .min_h(px(28.0))
            .flex_shrink_0()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1()
            .px_1()
            .child(mode_control)
            .child(theme::text("clip volume"))
            .children([self.midi_button(
                grid.label(),
                "envelope grid · cmd/control inverts snap · shift fine drag",
                cx.listener(|v, _, _, cx| {
                    v.cycle_midi_grid();
                    cx.notify();
                }),
            )])
            .children([self.midi_button(
                "↺",
                "reset clip volume to unity",
                cx.listener(move |v, _, _, cx| {
                    if !v.edit_pending()
                        && v.detail_clip_id() == Some(clip)
                        && v.project_generation == generation
                        && v.state.as_ref().is_some_and(|s| s.revision == revision)
                    {
                        v.cancel_midi_volume_input();
                        v.volume_submit(clip, MidiClipVolume::default());
                    }
                    cx.notify();
                }),
            )])
            .children([self.midi_button(
                "←",
                "pan envelope left",
                cx.listener(|v, _, _, cx| {
                    v.pan_piano(false);
                    cx.notify();
                }),
            )])
            .children([self.midi_button(
                "→",
                "pan envelope right",
                cx.listener(|v, _, _, cx| {
                    v.pan_piano(true);
                    cx.notify();
                }),
            )])
            .children([self.midi_button(
                "−",
                "zoom envelope out",
                cx.listener(|v, _, _, cx| {
                    v.zoom_piano(false);
                    cx.notify();
                }),
            )])
            .children([self.midi_button(
                "+",
                "zoom envelope in",
                cx.listener(|v, _, _, cx| {
                    v.zoom_piano(true);
                    cx.notify();
                }),
            )])
            .child(self.zoom_fit_button(super::zoom_fit::Target::Midi, false, cx))
            .child(self.zoom_fit_button(super::zoom_fit::Target::Midi, true, cx));
        // Reserve feedback height even before a point is selected: revealing or
        // changing a value must not move the graph under a captured pointer.
        let feedback = div()
            .h(px(22.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_2()
            .px_1()
            .text_xs()
            .child(theme::text(format!("{} points", volume.points().len())))
            .children(
                point_values(&volume, self.midi_volume.selected).map(|values| {
                    div()
                        .ml_auto()
                        .min_w_0()
                        .truncate()
                        .debug_selector(|| "midi-volume-point-values".into())
                        .child(theme::text(values))
                }),
            );
        Some(
            div()
                .h(px(self.clip_detail_body_height))
                .w_full()
                .flex()
                .flex_col()
                .debug_selector(|| "midi-volume-editor".into())
                .child(toolbar)
                .child(feedback)
                .child(div().flex_1().min_h_0().child(graph)),
        )
    }
    fn midi_volume_event(&mut self, event: ResponseGraphEvent, cx: &mut Context<Self>) {
        if event.phase == Phase::Cancel {
            self.cancel_midi_volume_input();
            cx.notify();
            return;
        }
        if !self.midi_volume.enabled || self.edit_pending() {
            return;
        }
        let Some(clip) = self.detail_clip_id() else {
            return;
        };
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let Some(source) = state.project.midi_source(clip) else {
            return;
        };
        let Some(original) = curve(&state.project, clip).cloned() else {
            return;
        };
        let length = source.source_length();
        if event.phase == Phase::Select {
            self.midi_volume.selected = event.node_id;
            cx.notify();
            return;
        }
        if matches!(event.phase, Phase::Press | Phase::Create) {
            let mut original = original.clone();
            let index = if event.phase == Phase::Create {
                let Some(point) = point_at(
                    event.position,
                    self.piano_beat_start.raw(),
                    self.piano_beat_span.raw(),
                    length,
                    self.midi_snap_grid,
                    event.modifiers.platform,
                ) else {
                    return;
                };
                let mut points = original.points().to_vec();
                let i = match points.binary_search_by_key(&point.beat, |p| p.beat) {
                    Ok(i) => i,
                    Err(i) => {
                        points.insert(i, point);
                        i
                    }
                };
                let Ok(volume) = MidiClipVolume::new(points, length) else {
                    self.error = Some("clip volume point limit reached".into());
                    return;
                };
                original = volume;
                i
            } else {
                let Some(index) = event
                    .node_id
                    .checked_sub(1)
                    .map(|v| v as usize)
                    .filter(|i| *i < original.points().len())
                else {
                    return;
                };
                index
            };
            self.midi_volume.selected = index as u32 + 1;
            self.midi_volume.gesture = Some(Gesture {
                clip,
                generation: self.project_generation,
                revision: state.revision,
                start: self.piano_beat_start.raw(),
                span: self.piano_beat_span.raw(),
                length,
                grid: self.midi_snap_grid,
                preview: original.clone(),
                original,
                point: index,
            });
        }
        if event.phase == Phase::Remove {
            let Some(index) = event
                .node_id
                .checked_sub(1)
                .map(|v| v as usize)
                .filter(|i| *i < original.points().len())
            else {
                return;
            };
            let mut points = original.points().to_vec();
            points.remove(index);
            if let Ok(volume) = MidiClipVolume::new(points, length) {
                self.volume_submit(clip, volume);
            }
        } else if matches!(event.phase, Phase::Change | Phase::Commit) {
            let current = self
                .midi_volume
                .gesture
                .as_ref()
                .is_some_and(|g| self.volume_gesture_current(g));
            if !current {
                self.cancel_midi_volume_input();
                return;
            }
            if let Some(g) = self.midi_volume.gesture.as_mut()
                && let Some(point) = point_at(
                    event.position,
                    g.start,
                    g.span,
                    g.length,
                    g.grid,
                    event.modifiers.platform,
                )
                && let Some(volume) = moved(g, point)
            {
                g.preview = volume;
            }
        }
        if matches!(
            event.phase,
            Phase::Commit | Phase::Release | Phase::ReleaseOutside
        ) && let Some(g) = self.midi_volume.gesture.take()
            && self.volume_gesture_current(&g)
        {
            self.volume_submit(g.clip, g.preview);
        }
        cx.notify();
    }
}

fn model(volume: &MidiClipVolume, start: u64, span: u64, selected: u32) -> ResponseGraphModel {
    let end = start.saturating_add(span);
    let x = |beat: u64| ((beat.saturating_sub(start)) as f64 / span as f64).clamp(0.0, 1.0) as f32;
    let mut points = vec![[
        0.0,
        (volume.gain_at(BeatTime::from_raw(start)).linear() / 4.0) as f32,
    ]];
    let mut nodes = Vec::new();
    for (index, p) in volume
        .points()
        .iter()
        .enumerate()
        .filter(|(_, p)| (start..=end).contains(&p.beat.raw()))
    {
        points.push([x(p.beat.raw()), (p.incoming.linear() / 4.0) as f32]);
        points.push([x(p.beat.raw()), (p.value.linear() / 4.0) as f32]);
        let id = index as u32 + 1;
        nodes.push(ResponseGraphNode {
            id,
            position: [x(p.beat.raw()), (p.value.linear() / 4.0) as f32],
            enabled: true,
            selected: selected == id,
        });
    }
    points.push([
        1.0,
        (volume.gain_before(BeatTime::from_raw(end)).linear() / 4.0) as f32,
    ]);
    ResponseGraphModel {
        primary: points.clone(),
        secondary: points,
        analyzer: None,
        input_analyzer: None,
        nodes,
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_volume_ui_tests.rs"]
mod ui_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn beat_guides_align_to_source_grid_and_remain_bounded_at_extreme_zoom() {
        use super::super::midi_drag::MidiGrid;
        let beat = BeatTime::ONE.raw();
        let ticks = beat_axes(0, 4 * beat, MidiGrid::Sixteenth);
        assert_eq!(ticks.len(), 17);
        assert_eq!(ticks[4], (0.25, theme::text("1b")));
        assert_eq!(
            beat_axes(beat / 2, 4 * beat, MidiGrid::Sixteenth)[2].1,
            theme::text("1b")
        );
        assert_eq!(beat_axes(0, 4 * beat, MidiGrid::Off).len(), 5);
        for (start, span) in [(0, u64::MAX), (u64::MAX - 4 * beat, 4 * beat), (0, 1)] {
            let ticks = beat_axes(start, span, MidiGrid::ThirtySecond);
            assert!((2..=32).contains(&ticks.len()));
            assert!(
                ticks.iter().all(|(x, label)| x.is_finite()
                    && (0.0..=1.0).contains(x)
                    && label.len() <= 24)
            );
            assert!(ticks.windows(2).all(|w| w[0].0 <= w[1].0));
        }
        assert!(beat_axes(u64::MAX, 1, MidiGrid::Quarter).is_empty());
        assert!(beat_axes(0, 0, MidiGrid::Quarter).is_empty());
    }

    #[test]
    fn dense_original_points_and_boundary_jumps_remain_visible_and_bounded() {
        let length = BeatTime::from_raw(2048);
        let points: Vec<_> = (0..1024)
            .map(|i| MidiClipVolumePoint {
                beat: BeatTime::from_raw(i * 2),
                incoming: MidiClipGain::new(0.25).unwrap(),
                value: MidiClipGain::new(0.75).unwrap(),
            })
            .collect();
        let curve = MidiClipVolume::new(points, length).unwrap();
        let mut model = model(&curve, 0, length.raw(), 1024);
        assert_eq!(model.nodes.len(), 1024);
        assert_eq!(model.primary.len(), 2050);
        assert!(model.validate().is_ok());
        assert!(model.nodes[1023].selected);
        model.nodes.push(ResponseGraphNode {
            id: 1025,
            ..model.nodes[0]
        });
        assert!(
            model.validate().is_err(),
            "dense node bound remains enforced"
        );
        model.nodes.pop();
        assert_eq!(model.primary[1], [0.0, 0.0625]);
        assert_eq!(model.primary[2], [0.0, 0.1875]);
        let zoom = model_for_zoom(&curve);
        assert_eq!(zoom.nodes.len(), 2);
        assert_eq!(zoom.nodes[0].id, 501);
    }
    fn model_for_zoom(curve: &MidiClipVolume) -> ResponseGraphModel {
        model(curve, 1000, 3, 0)
    }
    #[test]
    fn pointer_numbers_snap_and_neighbor_clamps_preserve_authored_jump() {
        let length = BeatTime::from_quarters(4).unwrap();
        let grid = super::super::midi_drag::MidiGrid::Sixteenth;
        assert!(point_at([f32::NAN, 0.0], 0, length.raw(), length, grid, false).is_none());
        assert!(point_at([0.0, f32::INFINITY], 0, length.raw(), length, grid, false).is_none());
        let snapped = point_at([0.3, 0.5], 0, length.raw(), length, grid, false).unwrap();
        let exact = point_at([0.3, 0.5], 0, length.raw(), length, grid, true).unwrap();
        assert_eq!(
            snapped.beat,
            BeatTime::from_raw(BeatTime::ONE.raw() * 5 / 4)
        );
        assert_ne!(snapped.beat, exact.beat);
        let points = vec![
            MidiClipVolumePoint::linear(BeatTime::ZERO, MidiClipGain::UNITY),
            MidiClipVolumePoint {
                beat: BeatTime::ONE,
                incoming: MidiClipGain::new(0.25).unwrap(),
                value: MidiClipGain::new(0.75).unwrap(),
            },
            MidiClipVolumePoint::linear(length, MidiClipGain::UNITY),
        ];
        let original = MidiClipVolume::new(points, length).unwrap();
        let mut editor = hex_project::Editor::new(Project::new(8000).unwrap(), 8).unwrap();
        let hex_project::Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "curve".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let hex_project::Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length,
            })
            .unwrap()
        else {
            panic!("clip")
        };
        let g = Gesture {
            clip,
            generation: 0,
            revision: 0,
            start: 0,
            span: length.raw(),
            length,
            grid,
            preview: original.clone(),
            original,
            point: 1,
        };
        let changed = moved(
            &g,
            MidiClipVolumePoint::linear(BeatTime::ZERO, MidiClipGain::new(1.0).unwrap()),
        )
        .unwrap();
        assert_eq!(changed.points()[1].beat.raw(), 1);
        assert_eq!(changed.points()[1].incoming.linear(), 0.5);
        assert_eq!(changed.points()[1].value.linear(), 1.0);
    }
}
