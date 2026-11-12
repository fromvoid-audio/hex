//! Applied track/Return peaks and final-output Main peaks, with shared presentation.
mod decay;
mod follower;
#[cfg(all(test, feature = "ui-test-support"))]
mod main_output;
mod scale;
mod selection;

use super::EditorView;
use crate::theme;
use decay::{decay_factor, decay_level};
use gpui::{Div, div, prelude::*, px};
use hex_project::{ReturnId, TrackId};
use selection::console_meter_tracks;
use std::collections::BTreeMap;
use std::sync::atomic::Ordering::Acquire;
use std::time::Instant;

#[derive(Default)]
pub(super) struct State {
    main: follower::State,
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
        // Main observes the output itself, including preview without an applied
        // project observation. Clearing track observations must not erase it.
        let main = std::mem::take(&mut self.main);
        *self = Self {
            scratch,
            main,
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
        decay_factor(&mut self.updated, now)
    }
    pub(super) fn level(&self, track: TrackId) -> [f32; 2] {
        self.levels.get(&track).copied().unwrap_or([0.0; 2])
    }
    pub(super) fn return_level(&self, id: ReturnId) -> [f32; 2] {
        self.returns.get(&id).copied().unwrap_or([0.0; 2])
    }
    pub(super) fn main_level(&self) -> [f32; 2] {
        self.main.level()
    }
    pub(super) fn observe_main(
        &mut self,
        frame: Option<super::refresh::AudioFrame>,
        now: Instant,
    ) -> bool {
        self.main.observe(frame, now)
    }
    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn set_test_level(&mut self, id: TrackId, peak: [f32; 2]) {
        self.levels.insert(id, peak);
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

pub(super) fn render_main(peak: [f32; 2], height: f32) -> Div {
    render_meter("main-output-meter".into(), peak, height)
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
        let mut bar = hex_widgets::level_meter(
            channel_id.clone().into(),
            scale::fraction(peak),
            peak.is_finite() && peak > 1.0,
            hex_widgets::LevelMeterStyle {
                background: style.rail,
                fill: style.accent,
                above: theme::meter_colors().clipping,
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
            for (peak, expected, clipping) in [
                (0.0, 0.0, false),
                (scale::FLOOR_AMPLITUDE, 0.0, false),
                (10_f32.powf(-24.0 / 20.0), 0.5, false),
                (1.0, 0.8972, false),
                (10_f32.powf(6.0 / 20.0), 1.0, true),
                // Continued clipping must not undo the user's acknowledgement.
                (100.0, 1.0, false),
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
                assert_eq!(
                    visual.debug_bounds("scale-probe-left-above").is_some(),
                    clipping
                );
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
                if let Some(cap) = visual.debug_bounds("scale-probe-left-above") {
                    visual.simulate_click(cap.center(), gpui::Modifiers::default());
                    visual.run_until_parked();
                    assert!(visual.debug_bounds("scale-probe-left-above").is_none());
                }
            }
        }
    }

    #[cfg(all(test, not(feature = "ui-test-support")))]
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

    #[cfg(all(test, not(feature = "ui-test-support")))]
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
        let _output_serial = hex_audio::worker_fixture::serial_output();
        let (mut output, mut backend) =
            hex_audio::worker_fixture::open_after_retirement(48_000, false).unwrap();
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
                clip_preview_identity: None,
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
        editor.update(cx, |view, _| {
            view.audio.take().unwrap().control.close();
        });
        backend.close();
        drop(output);
    }
}
