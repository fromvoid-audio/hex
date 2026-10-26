//! Visible track peaks from the currently applied playback observation.
use super::EditorView;
use crate::theme;
use gpui::{Div, prelude::*, px};
use hex_project::{ReturnId, TrackId};
use std::collections::BTreeMap;
use std::sync::atomic::Ordering::Acquire;
use std::time::Instant;

#[derive(Default)]
pub(super) struct State {
    identity: Option<(u64, u64)>,
    levels: BTreeMap<TrackId, f32>,
    returns: BTreeMap<ReturnId, f32>,
    updated: Option<Instant>,
}
impl State {
    pub(super) fn level(&self, track: TrackId) -> f32 {
        self.levels.get(&track).copied().unwrap_or(0.0)
    }
    pub(super) fn return_level(&self, id: ReturnId) -> f32 {
        self.returns.get(&id).copied().unwrap_or(0.0)
    }
    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn set_test_level(&mut self, id: TrackId, peak: f32) {
        self.levels.insert(id, peak);
    }
}

impl EditorView {
    pub(super) fn tick_track_meters(&mut self, now: Instant) -> bool {
        let source = self.audio.as_ref().and_then(|audio| {
            let status = audio.control.status()?;
            let audible = self.audible_playback.as_ref()?;
            if audible.project_generation != self.project_generation
                || status.applied_revision.load(Acquire) != audible.audio_revision
                || status.device_fault.load(Acquire)
                || status.render_fault.load(Acquire)
                || status.shutdown_complete.load(Acquire)
            {
                return None;
            }
            Some((status, audible, audible.observation.as_ref()?))
        });
        let Some((status, audible, observation)) = source else {
            let changed =
                !self.track_meters.levels.is_empty() || !self.track_meters.returns.is_empty();
            self.track_meters = State::default();
            return changed;
        };
        let identity = (audible.project_generation, audible.audio_revision);
        let changed_identity = self.track_meters.identity != Some(identity);
        if changed_identity {
            self.track_meters.levels.clear();
            self.track_meters.returns.clear();
            self.track_meters.identity = Some(identity);
            self.track_meters.updated = None;
        }
        let elapsed = self.track_meters.updated.map_or(0.0, |previous| {
            now.saturating_duration_since(previous).as_secs_f32()
        });
        self.track_meters.updated = Some(now);
        let decay = 10.0_f32.powf(-12.0 * elapsed / 20.0);
        let viewport = self.track_scroll.list.viewport_bounds();
        let start = self.track_scroll.list.logical_scroll_top().item_ix;
        let project = self.state.as_ref().map(|state| &state.project);
        let mut visible = Vec::new();
        if let Some(project) = project {
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
        let mut changed = changed_identity;
        self.track_meters
            .levels
            .retain(|id, _| visible.contains(id));
        for id in visible {
            let Some(meter) = observation.track_output_meter(id) else {
                continue;
            };
            let peak = meter.take_track_output();
            let Some(entry) = self.track_meters.levels.get_mut(&id) else {
                // Peaks retained while a row was offscreen have unknown age.
                // Discard them before beginning this visible observation.
                self.track_meters.levels.insert(id, 0.0);
                continue;
            };
            let next = peak.max(*entry * decay);
            let next = if next < 0.001 { 0.0 } else { next };
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
            for track in project.return_tracks() {
                let id = track.id();
                let Some(meter) = observation.return_output_meter(id) else {
                    continue;
                };
                let peak = meter.take_track_output();
                let Some(entry) = self.track_meters.returns.get_mut(&id) else {
                    self.track_meters.returns.insert(id, 0.0);
                    continue;
                };
                let next = peak.max(*entry * decay);
                let next = if next < 0.001 { 0.0 } else { next };
                changed |= next != *entry;
                *entry = next;
            }
        }
        // A callback publication may race the independent peak loads. Do not
        // display readings from the previous graph after its revision changes.
        if status.applied_revision.load(Acquire) != audible.audio_revision {
            self.track_meters = State::default();
            return true;
        }
        changed
    }
}

pub(super) fn render(track: TrackId, peak: f32, height: f32) -> Div {
    render_meter(
        format!("track-output-meter-{}", track.value()),
        peak,
        height,
    )
}

pub(super) fn render_return(id: ReturnId, peak: f32, height: f32) -> Div {
    render_meter(format!("return-output-meter-{}", id.value()), peak, height)
}

fn render_meter(id: String, peak: f32, height: f32) -> Div {
    let colors = theme::meter_colors();
    let level = hex_widgets::MeterLevel::from_amplitude(peak, false);
    let fraction = if peak > 0.0 && peak.is_finite() {
        ((20.0 * peak.log10() + 60.0) / 60.0).clamp(0.0, 1.0)
    } else {
        0.0
    };
    hex_widgets::level_meter(
        id.into(),
        fraction,
        peak > 1.0,
        hex_widgets::LevelMeterStyle {
            background: theme::color(theme::PaletteRole::WaveBackground),
            fill: colors.color(level),
            above: colors.clipping,
        },
    )
    .w(px(6.0))
    .h(px((height - 8.0).clamp(10.0, 42.0)))
}
