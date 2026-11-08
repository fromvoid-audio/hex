//! Visible track peaks from the currently applied playback observation.
mod scale;

use super::{
    EditorView,
    console_track_page::{TrackPage, visible_track_range},
};
use crate::theme;
use gpui::{Div, div, prelude::*, px};
use hex_project::{Project, ReturnId, TrackId};
use std::collections::BTreeMap;
use std::sync::atomic::Ordering::Acquire;
use std::time::Instant;

#[derive(Default)]
pub(super) struct State {
    identity: Option<(u64, u64)>,
    levels: BTreeMap<TrackId, [f32; 2]>,
    returns: BTreeMap<ReturnId, [f32; 2]>,
    updated: Option<Instant>,
    /// Per-tick staging buffers, retained across ticks and `clear`.
    scratch: Scratch,
}

/// Reused staging for one meter observation; contents are per-tick only.
#[derive(Default)]
struct Scratch {
    visible: Vec<TrackId>,
    readings: Vec<(TrackId, [f32; 2])>,
    returns: Vec<(ReturnId, [f32; 2])>,
}

impl State {
    fn clear(&mut self) -> bool {
        let changed = !self.levels.is_empty() || !self.returns.is_empty();
        let scratch = std::mem::take(&mut self.scratch);
        *self = Self {
            scratch,
            ..Self::default()
        };
        changed
    }
    // A revision handoff can temporarily lack an observation. Retain only
    // previously accepted peaks from this project, decaying them normally;
    // never read an observation whose audio revision is not applied.
    fn hold(&mut self, generation: u64, now: Instant) -> bool {
        if self
            .identity
            .is_none_or(|(project, _)| project != generation)
        {
            return self.clear();
        }
        let decay = self.decay(now);
        let mut changed = false;
        for entry in self.levels.values_mut().chain(self.returns.values_mut()) {
            let next = entry.map(|level| decay_level(level, decay));
            changed |= next != *entry;
            *entry = next;
        }
        changed
    }
    fn adopt(&mut self, identity: (u64, u64), now: Instant) -> f32 {
        if self
            .identity
            .is_some_and(|(project, _)| project != identity.0)
        {
            self.clear();
        }
        self.identity = Some(identity);
        self.decay(now)
    }
    fn decay(&mut self, now: Instant) -> f32 {
        let elapsed = self.updated.map_or(0.0, |previous| {
            now.saturating_duration_since(previous).as_secs_f32()
        });
        self.updated = Some(now);
        10.0_f32.powf(-12.0 * elapsed / 20.0)
    }
    pub(super) fn level(&self, track: TrackId) -> [f32; 2] {
        self.levels.get(&track).copied().unwrap_or([0.0; 2])
    }
    pub(super) fn return_level(&self, id: ReturnId) -> [f32; 2] {
        self.returns.get(&id).copied().unwrap_or([0.0; 2])
    }
    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn set_test_level(&mut self, id: TrackId, peak: [f32; 2]) {
        self.levels.insert(id, peak);
    }
}

fn decay_level(level: f32, decay: f32) -> f32 {
    let next = level * decay;
    if next < scale::FLOOR_AMPLITUDE {
        0.0
    } else {
        next
    }
}

fn selected_console_meter(project: &Project, selected: Option<TrackId>) -> Option<TrackId> {
    selected.filter(|id| project.track(*id).is_some())
}

/// Use the grid's bounded page, including its frozen reorder projection. The
/// selected fallback keeps its observation warm across Console page changes.
/// Reuse the caller's staging storage; this helper owns no meter or audio state.
fn console_meter_tracks(
    project: &Project,
    frozen: Option<&TrackPage>,
    indices: &[usize],
    start: usize,
    count: usize,
    selected: Option<TrackId>,
    output: &mut Vec<TrackId>,
) {
    if let Some(page) = frozen {
        output.extend(page.columns.iter().filter_map(|column| {
            project
                .tracks()
                .get(column.index)
                .filter(|track| track.id() == column.id)
                .map(hex_project::Track::id)
        }));
    } else {
        let range = visible_track_range(indices.len(), start, count);
        output.extend(
            indices[range]
                .iter()
                .filter_map(|&index| project.tracks().get(index).map(hex_project::Track::id)),
        );
    }
    if let Some(id) = selected_console_meter(project, selected)
        && !output.contains(&id)
    {
        output.push(id);
    }
}

impl EditorView {
    pub(super) fn tick_track_meters(&mut self, now: Instant) -> bool {
        let mut scratch = std::mem::take(&mut self.track_meters.scratch);
        let changed = self.observe_track_meters(now, &mut scratch);
        self.track_meters.scratch = scratch;
        changed
    }

    fn observe_track_meters(&mut self, now: Instant, scratch: &mut Scratch) -> bool {
        let Scratch {
            visible,
            readings,
            returns: return_readings,
        } = scratch;
        visible.clear();
        readings.clear();
        return_readings.clear();
        let Some(status) = self.audio.as_ref().and_then(|audio| audio.control.status()) else {
            return self.track_meters.clear();
        };
        if status.device_fault.load(Acquire)
            || status.render_fault.load(Acquire)
            || status.shutdown_complete.load(Acquire)
        {
            return self.track_meters.clear();
        }
        let Some((audible, observation)) = self.audible_playback.as_ref().and_then(|audible| {
            (audible.project_generation == self.project_generation
                && status.applied_revision.load(Acquire) == audible.audio_revision)
                .then_some((audible, audible.observation.as_ref()?))
        }) else {
            return self.track_meters.hold(self.project_generation, now);
        };
        let identity = (audible.project_generation, audible.audio_revision);
        let project = self.state.as_ref().map(|state| &state.project);
        if let Some(project) = project {
            if self.mode == super::EditorMode::Session {
                let frozen = self.pending_console_reorder_page();
                console_meter_tracks(
                    project,
                    frozen.as_deref(),
                    &self.arrangement_focus.projection.console_indices,
                    self.session_track_start,
                    self.session_visible_columns,
                    self.selected_track,
                    visible,
                );
            } else {
                let viewport = self.track_scroll.list.viewport_bounds();
                let start = self.track_scroll.list.logical_scroll_top().item_ix;
                for row in start..self.arrangement_track_indices().len() {
                    let Some(bounds) = self.track_scroll.list.bounds_for_item(row) else {
                        break;
                    };
                    if bounds.top() >= viewport.bottom() {
                        break;
                    }
                    let Some(track) = self
                        .arrangement_track_index(row)
                        .and_then(|index| project.tracks().get(index))
                    else {
                        continue;
                    };
                    visible.push(track.id());
                }
            }
        }
        // Stage the readings until the applied revision is rechecked. A race
        // discards them without wiping the last valid display.
        readings.extend(visible.iter().map(|id| {
            (
                *id,
                observation
                    .track_output_meter(*id)
                    .map_or([0.0; 2], |meter| meter.take_track_output_stereo()),
            )
        }));
        return_readings.extend(
            project
                .into_iter()
                .flat_map(|project| project.return_tracks())
                .map(|track| {
                    let id = track.id();
                    (
                        id,
                        observation
                            .return_output_meter(id)
                            .map_or([0.0; 2], |meter| meter.take_track_output_stereo()),
                    )
                }),
        );
        if status.device_fault.load(Acquire)
            || status.render_fault.load(Acquire)
            || status.shutdown_complete.load(Acquire)
        {
            return self.track_meters.clear();
        }
        if status.applied_revision.load(Acquire) != audible.audio_revision {
            return self.track_meters.hold(self.project_generation, now);
        }
        let changed_identity = self.track_meters.identity != Some(identity);
        let decay = self.track_meters.adopt(identity, now);
        let mut changed = changed_identity;
        self.track_meters
            .levels
            .retain(|id, _| visible.contains(id));
        for &(id, peak) in readings.iter() {
            let Some(entry) = self.track_meters.levels.get_mut(&id) else {
                // Peaks retained while a row was offscreen have unknown age.
                // Discard them before beginning this visible observation.
                self.track_meters.levels.insert(id, [0.0; 2]);
                continue;
            };
            let next = std::array::from_fn(|channel| {
                peak[channel].max(decay_level(entry[channel], decay))
            });
            changed |= next != *entry;
            *entry = next;
        }
        if let Some(project) = project {
            self.track_meters.returns.retain(|id, _| {
                project
                    .return_tracks()
                    .iter()
                    .any(|track| track.id() == *id)
            });
            for &(id, peak) in return_readings.iter() {
                let Some(entry) = self.track_meters.returns.get_mut(&id) else {
                    self.track_meters.returns.insert(id, [0.0; 2]);
                    continue;
                };
                let next = std::array::from_fn(|channel| {
                    peak[channel].max(decay_level(entry[channel], decay))
                });
                changed |= next != *entry;
                *entry = next;
            }
        }
        changed
    }
}

pub(super) fn render(track: TrackId, peak: [f32; 2], height: f32) -> Div {
    render_meter(
        format!("track-output-meter-{}", track.value()),
        peak,
        height,
    )
}

pub(super) fn render_return(id: ReturnId, peak: [f32; 2], height: f32) -> Div {
    render_meter(format!("return-output-meter-{}", id.value()), peak, height)
}

fn render_meter(id: String, peaks: [f32; 2], height: f32) -> Div {
    let style = theme::track_meter_style();
    let description = format!(
        "output peak l {} · r {} · scale −70 to +6 dbfs · marks at 0 and +3 dbfs",
        scale::readout(peaks[0]),
        scale::readout(peaks[1]),
    );
    let mut bars = div()
        .debug_selector({
            let id = id.clone();
            move || id.clone()
        })
        .flex()
        .gap(px(hex_widgets::STEREO_PEAK_BAR_GAP))
        .w(px(
            2.0 * hex_widgets::STEREO_PEAK_BAR_WIDTH + hex_widgets::STEREO_PEAK_BAR_GAP
        ))
        .h(px((height - 8.0).clamp(10.0, 42.0)))
        .flex_shrink_0();
    for (side, peak) in ["left", "right"].into_iter().zip(peaks) {
        let channel_id = format!("{id}-{side}");
        let mut bar = hex_widgets::signal_meter(
            channel_id.clone().into(),
            scale::fraction(peak),
            hex_widgets::LevelMeterStyle {
                background: style.rail,
                fill: style.accent,
                above: style.accent,
            },
        )
        .w(px(hex_widgets::STEREO_PEAK_BAR_WIDTH))
        .h_full()
        .child(
            div()
                .absolute()
                .inset_0()
                .border_1()
                .border_color(gpui::rgb(style.border)),
        );
        for (name, fraction) in [
            ("unity", scale::UNITY_FRACTION),
            ("headroom", scale::fraction(10_f32.powf(3.0 / 20.0))),
        ] {
            let selector = format!("{channel_id}-{name}");
            bar = bar.child(
                div()
                    .debug_selector(move || selector.clone())
                    .absolute()
                    .top(gpui::relative(1.0 - fraction))
                    .w_full()
                    .h(px(1.0))
                    .bg(gpui::rgb(style.text)),
            );
        }
        bars = bars.child(bar);
    }
    div()
        .flex_shrink_0()
        .child(bars.id(gpui::SharedString::from(id)).tooltip(move |_, cx| {
            cx.new(|_| super::transport::MiniTooltip {
                kind: crate::tooltips::Kind::Identification,
                description: theme::text(description.clone()),
            })
            .into()
        }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor, Project};
    use std::time::Duration;

    #[test]
    fn decay_keeps_the_new_finite_floor_without_changing_its_rate() {
        assert_eq!(decay_level(0.001, 0.5), 0.0005);
        assert_eq!(
            decay_level(scale::FLOOR_AMPLITUDE, 1.0),
            scale::FLOOR_AMPLITUDE
        );
        assert_eq!(decay_level(scale::FLOOR_AMPLITUDE, 0.5), 0.0);
    }

    #[cfg(feature = "ui-test-support")]
    #[gpui::test]
    fn compact_and_expanded_meter_geometry_has_headroom_and_a_unity_mark(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::{Context, IntoElement, Render, VisualTestContext, Window};
        struct Fixture {
            peak: [f32; 2],
            height: f32,
        }
        impl Render for Fixture {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div()
                    .size_full()
                    .child(render_meter("scale-probe".into(), self.peak, self.height))
            }
        }
        let handle = cx.add_window(|_, _| Fixture {
            peak: [0.0; 2],
            height: 18.0,
        });
        let view = handle.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*handle, cx);
        for height in [18.0, 50.0] {
            for (peak, expected) in [
                (0.0, 0.0),
                (scale::FLOOR_AMPLITUDE, 0.0),
                (10_f32.powf(-24.0 / 20.0), 0.5),
                (1.0, 0.8972),
                (10_f32.powf(6.0 / 20.0), 1.0),
                (100.0, 1.0),
            ] {
                view.update(&mut visual, |view, cx| {
                    view.peak = [peak, 0.001];
                    view.height = height;
                    cx.notify();
                });
                visual.run_until_parked();
                let well = visual.debug_bounds("scale-probe").unwrap();
                assert_eq!(well.size.width, px(7.0));
                let left = visual.debug_bounds("scale-probe-left").unwrap();
                let right = visual.debug_bounds("scale-probe-right").unwrap();
                assert_eq!(left.size.width, px(3.0));
                assert_eq!(right.size.width, px(3.0));
                assert_eq!(right.left() - left.right(), px(1.0));
                assert_eq!(left.size.height, right.size.height);
                assert!(visual.debug_bounds("scale-probe-left-above").is_none());
                assert!(visual.debug_bounds("scale-probe-right-above").is_none());
                assert_eq!(well.size.height, px((height - 8.0).clamp(10.0, 42.0)));
                let fill = visual
                    .debug_bounds("scale-probe-left-fill")
                    .map_or(0.0, |b| f32::from(b.size.height));
                let expected_height = f32::from(well.size.height) * expected;
                if expected == 0.0 || expected == 1.0 {
                    assert_eq!(fill, expected_height);
                } else {
                    // GPUI snaps these compact wells to physical pixels.
                    assert!(
                        (fill - expected_height).abs() <= 1.0,
                        "height={height}, peak={peak}, fill={fill}, expected={expected_height}"
                    );
                }
                let right_fill = visual.debug_bounds("scale-probe-right-fill").unwrap();
                let expected_right = f32::from(well.size.height) * scale::fraction(0.001);
                assert!((f32::from(right_fill.size.height) - expected_right).abs() <= 1.0);
                if peak >= 1.0 {
                    assert!(fill > f32::from(right_fill.size.height));
                }
                if peak >= 1.0 {
                    assert_eq!(
                        visual.rendered_background("scale-probe-left-fill"),
                        visual.rendered_background("scale-probe-right-fill")
                    );
                }
                let unity = visual.debug_bounds("scale-probe-left-unity").unwrap();
                let headroom = visual.debug_bounds("scale-probe-left-headroom").unwrap();
                assert!(headroom.top() <= unity.top());
                assert!(unity.top() >= well.top() && unity.bottom() <= well.bottom());
                let expected_unity = f32::from(well.size.height) * (1.0 - 0.8972);
                assert!((f32::from(unity.top() - well.top()) - expected_unity).abs() <= 1.0);
            }
        }
    }

    #[test]
    fn clearing_meters_keeps_the_per_tick_staging_capacity() {
        let mut state = State::default();
        state.scratch.visible.reserve(64);
        state.scratch.readings.reserve(64);
        state.scratch.returns.reserve(16);
        let pointers = (
            state.scratch.visible.as_ptr(),
            state.scratch.readings.as_ptr(),
            state.scratch.returns.as_ptr(),
        );
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack { name: "t".into() })
            .unwrap()
        else {
            panic!("track")
        };
        state.levels.insert(track, [0.5, 0.25]);
        assert!(state.clear());
        assert!(state.levels.is_empty());
        assert!(state.scratch.visible.capacity() >= 64);
        assert!(state.scratch.readings.capacity() >= 64);
        assert!(state.scratch.returns.capacity() >= 16);
        assert_eq!(
            (
                state.scratch.visible.as_ptr(),
                state.scratch.readings.as_ptr(),
                state.scratch.returns.as_ptr(),
            ),
            pointers,
            "a fault/stop clear must not drop the retained buffers"
        );
    }

    #[test]
    fn console_meter_page_matches_folded_paging_and_frozen_reorder_ids() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let tracks: Vec<_> = (0..4)
            .map(|index| {
                let Created::Track(id) = editor
                    .execute(Command::AddTrack {
                        name: format!("track {index}"),
                    })
                    .unwrap()
                else {
                    panic!("track fixture")
                };
                id
            })
            .collect();
        let indices = [0, 2, 3]; // The folded projection omits track 1.
        let frozen = TrackPage::capture(editor.project(), &indices, 0, 2);
        let mut output = Vec::with_capacity(8);
        let pointer = output.as_ptr();
        for (page, projection, start, count, selected, expected) in [
            (
                None,
                &indices[..],
                1,
                2,
                Some(tracks[1]),
                vec![tracks[2], tracks[3], tracks[1]],
            ),
            (
                None,
                &indices[..],
                0,
                2,
                Some(tracks[0]),
                vec![tracks[0], tracks[2]],
            ),
            (None, &indices[..], usize::MAX, 0, None, vec![tracks[3]]),
            (None, &[usize::MAX][..], 0, 2, None, vec![]),
            (
                Some(&frozen),
                &[3][..],
                99,
                1,
                None,
                vec![tracks[0], tracks[2]],
            ),
        ] {
            output.clear();
            console_meter_tracks(
                editor.project(),
                page,
                projection,
                start,
                count,
                selected,
                &mut output,
            );
            assert_eq!(output, expected);
            assert_eq!(
                output.as_ptr(),
                pointer,
                "warm observations reuse staging storage"
            );
        }
        editor.execute(Command::RemoveTrack(tracks[0])).unwrap();
        output.clear();
        console_meter_tracks(
            editor.project(),
            Some(&frozen),
            &indices,
            0,
            2,
            None,
            &mut output,
        );
        assert!(
            output.is_empty(),
            "frozen index/ID pairs cannot read a shifted or deleted track"
        );
    }

    #[test]
    fn console_meter_tracks_the_selected_live_identity_including_groups() {
        use hex_project::TrackSelectionIds;
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "track".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Track(group) = editor
            .execute(Command::GroupTracks {
                tracks: TrackSelectionIds::new(&[track]).unwrap(),
                name: "group".into(),
            })
            .unwrap()
        else {
            panic!("group")
        };
        assert_eq!(
            selected_console_meter(editor.project(), Some(group)),
            Some(group)
        );
        assert_eq!(
            selected_console_meter(editor.project(), Some(track)),
            Some(track)
        );
        assert_eq!(selected_console_meter(editor.project(), None), None);
        let mut visible = Vec::new();
        console_meter_tracks(editor.project(), None, &[0], 0, 8, None, &mut visible);
        assert_eq!(visible, vec![group], "folded child meters are not observed");
        editor.execute(Command::RemoveTrack(group)).unwrap();
        assert_eq!(selected_console_meter(editor.project(), Some(group)), None);
        assert_eq!(selected_console_meter(editor.project(), Some(track)), None);
    }

    #[test]
    fn revision_handoffs_decay_accepted_levels_without_crossing_projects() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "track".into(),
            })
            .unwrap()
        else {
            panic!("track fixture");
        };
        let Created::ReturnTrack(bus) = editor
            .execute(Command::AddReturnTrack {
                name: "return".into(),
            })
            .unwrap()
        else {
            panic!("return fixture");
        };
        let now = Instant::now();
        let mut state = State::default();
        state.adopt((1, 10), now);
        state.levels.insert(track, [0.8, 0.2]);
        state.returns.insert(bus, [0.6, 0.1]);
        assert!(state.hold(1, now + Duration::from_millis(16)));
        let held = state.level(track);
        let returned = state.return_level(bus);
        assert!(held[0] > 0.7 && held[1] > 0.17 && returned[0] > 0.5 && returned[1] > 0.08);
        state.adopt((1, 11), now + Duration::from_millis(32));
        assert_eq!(state.level(track), held);
        assert_eq!(state.return_level(bus), returned);
        assert!(state.hold(1, now + Duration::from_millis(48)));
        assert!(state.level(track)[0] > 0.7);
        assert!(state.hold(1, now + Duration::from_secs(10)));
        assert_eq!(
            state.level(track),
            [0.0; 2],
            "a missing observation must not leave a frozen meter"
        );
        assert!(state.hold(2, now + Duration::from_secs(11)));
        assert_eq!(state.level(track), [0.0; 2]);
        assert_eq!(state.return_level(bus), [0.0; 2]);
        assert!(state.identity.is_none());
    }

    #[cfg(feature = "ui-test-support")]
    #[gpui::test]
    fn revision_handoff_keeps_meter_visible_across_repaints(cx: &mut gpui::TestAppContext) {
        use crate::editor::clip_drag_ui_test::editor_window;
        use gpui::{VisualTestContext, px, size};
        use std::sync::Arc;

        let mut model = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = model
            .execute(Command::AddTrack {
                name: "track".into(),
            })
            .unwrap()
        else {
            panic!("track fixture");
        };
        let (window, editor) = editor_window(cx, model.project().clone(), Arc::default());
        let mut visual = VisualTestContext::from_window(window, cx);
        visual.simulate_resize(size(px(1_280.0), px(800.0)));
        let now = Instant::now();
        editor.update(cx, |view, cx| {
            view.track_meters.adopt((1, 10), now);
            view.track_meters.levels.insert(track, [0.8, 0.2]);
            cx.notify();
        });
        visual.run_until_parked();
        let id: &'static str =
            Box::leak(format!("track-output-meter-{}-left-fill", track.value()).into_boxed_str());
        let first = visual.debug_bounds(id).unwrap().size.height;
        assert!(first > px(1.0));
        for (revision, millis) in [(None, 16), (Some(11), 32), (None, 48)] {
            editor.update(cx, |view, cx| {
                let time = now + Duration::from_millis(millis);
                if let Some(revision) = revision {
                    view.track_meters.adopt((1, revision), time);
                } else {
                    view.track_meters.hold(1, time);
                }
                cx.notify();
            });
            visual.run_until_parked();
            let height = visual.debug_bounds(id).unwrap().size.height;
            assert!(height > px(1.0), "meter disappeared during handoff");
            assert!(height <= first);
        }
    }

    /// Inspection check (hex-ui hosts no allocation counter): steady-state
    /// observed ticks stage readings in the same retained buffers.
    #[cfg(feature = "ui-test-support")]
    #[gpui::test]
    fn steady_state_meter_ticks_reuse_the_staging_buffers(cx: &mut gpui::TestAppContext) {
        use crate::editor::clip_drag_ui_test::editor_window;
        use gpui::{VisualTestContext, px, size};
        use std::{collections::BTreeMap, sync::Arc, sync::atomic::Ordering::Acquire};

        let mut model = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        for name in ["a", "b"] {
            model
                .execute(Command::AddTrack { name: name.into() })
                .unwrap();
        }
        let Created::ReturnTrack(bus) = model
            .execute(Command::AddReturnTrack {
                name: "return".into(),
            })
            .unwrap()
        else {
            panic!("return fixture")
        };
        let project = model.project().clone();
        let track = project.tracks()[0].id();
        let prepared = Arc::new(
            hex_engine::PreparedRevision::prepare(
                &project,
                &BTreeMap::new(),
                Arc::new(hex_engine::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let observation = Arc::new(
            hex_engine::PreparedPlaybackObservation::prepare_on_control(&prepared).unwrap(),
        );
        let deadline = crate::test_wait::hang_deadline();
        let (mut output, _backend) = loop {
            match hex_audio::worker_fixture::open(48_000) {
                Ok(output) => break output,
                Err(error)
                    if error == "audio output is open or still shutting down"
                        && Instant::now() < deadline =>
                {
                    std::thread::yield_now()
                }
                Err(error) => panic!("{error}"),
            }
        };
        let client = output.take_client().unwrap();
        let applied = client.status().unwrap().applied_revision.load(Acquire);
        let (window, editor) = editor_window(cx, project, Arc::default());
        let visual = VisualTestContext::from_window(window, cx);
        visual.simulate_resize(size(px(1_280.0), px(800.0)));
        visual.run_until_parked();
        let now = Instant::now();
        let pointers = editor.update(cx, |view, _| {
            view.audio = Some(crate::EditorAudio {
                control: client,
                sample_rate: 48_000,
                label: "meter scratch fixture".into(),
                spectrum_error: None,
                mode: hex_session::DocumentAudioMode::Resident,
            });
            view.audible_playback = Some(super::super::PlaybackCandidate {
                worker_requires_preroll: None,
                observation: Some(observation),
                audio_revision: applied,
                project_generation: view.project_generation,
                state: Arc::clone(view.state.as_ref().unwrap()),
                prepared_revision: Some(prepared),
                seek_request_id: None,
                device_preview_identity: None,
                tuning_preview_identity: None,
            });
            // Warm-up: the first observed ticks size the staging buffers.
            for tick in 0..2 {
                view.tick_track_meters(now + Duration::from_millis(16 * tick));
            }
            let observation = view
                .audible_playback
                .as_ref()
                .unwrap()
                .observation
                .as_ref()
                .unwrap();
            observation
                .track_output_meter(track)
                .unwrap()
                .observe_track_output(&[[0.8, -0.1]]);
            observation
                .return_output_meter(bus)
                .unwrap()
                .observe_track_output(&[[0.2, -0.6]]);
            view.tick_track_meters(now + Duration::from_millis(32));
            assert_eq!(view.track_meters.level(track), [0.8, 0.1]);
            assert_eq!(view.track_meters.return_level(bus), [0.2, 0.6]);
            let scratch = &view.track_meters.scratch;
            assert_eq!(scratch.visible.len(), 2, "both rows are observed");
            assert_eq!(scratch.readings.len(), 2, "both rows are read");
            (
                scratch.visible.as_ptr(),
                scratch.readings.as_ptr(),
                scratch.returns.as_ptr(),
            )
        });
        editor.update(cx, |view, _| {
            for tick in 3..10 {
                view.tick_track_meters(now + Duration::from_millis(16 * tick));
                let scratch = &view.track_meters.scratch;
                assert_eq!(scratch.readings.len(), 2);
                assert_eq!(
                    (
                        scratch.visible.as_ptr(),
                        scratch.readings.as_ptr(),
                        scratch.returns.as_ptr(),
                    ),
                    pointers,
                    "tick {tick} reallocated a staging buffer"
                );
            }
            view.mode = super::super::EditorMode::Session;
            view.session_track_start = 1;
            view.session_visible_columns = 1;
            view.selected_track = Some(track);
            let other = view.state.as_ref().unwrap().project.tracks()[1].id();
            view.audible_playback
                .as_ref()
                .unwrap()
                .observation
                .as_ref()
                .unwrap()
                .track_output_meter(other)
                .unwrap()
                .observe_track_output(&[[0.3, -0.9]]);
            view.tick_track_meters(now + Duration::from_millis(160));
            assert_eq!(view.track_meters.scratch.visible, vec![other, track]);
            assert_eq!(
                view.track_meters.level(other),
                [0.3, 0.9],
                "visible unselected Console track uses its own stereo observation"
            );
            view.selected_track = None;
            view.tick_track_meters(now + Duration::from_millis(176));
            assert_eq!(view.track_meters.scratch.visible, vec![other]);
            assert_eq!(
                view.track_meters.level(track),
                [0.0; 2],
                "off-page unselected levels are retired"
            );
            assert_eq!(view.track_meters.scratch.visible.as_ptr(), pointers.0);
            assert_eq!(view.track_meters.scratch.readings.as_ptr(), pointers.1);
        });
    }
}
