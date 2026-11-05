//! Throttled display of local render work from the applied scalar observation.
//! Cached values carry exact project/audio revision and stable canonical IDs.
//! No canonical edits, graph owners, clocks or DSP are created by this view.
use super::EditorView;
use crate::theme;
use gpui::{Div, div, prelude::*, px, rgb};
use hex_engine::{OutputCaptureId, RenderWorkSample};
use std::collections::BTreeMap;
use std::sync::atomic::Ordering::Acquire;
use std::time::{Duration, Instant};

const REFRESH: Duration = Duration::from_millis(100);
const STALE: Duration = Duration::from_secs(3);
// Percent of the rendered audio duration, in hundredths. This scale is Hex's
// diagnostic choice; the public Live manual does not specify its thresholds.
const THRESHOLDS: [u32; 6] = [10, 50, 100, 250, 500, 1000];

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
enum Availability {
    #[default]
    Waiting,
    Unavailable,
    Pending,
    Stopped,
    Fault,
    Stale,
    Fresh,
}

struct Entry {
    sample: RenderWorkSample,
    received: Instant,
    smoothed: f32,
    progress_verified: bool,
}

impl Entry {
    fn observe(&mut self, sample: RenderWorkSample, now: Instant, elapsed: f32) {
        // An initial or long-unobserved snapshot has unknown producer age.
        // A subsequent counter change while polling establishes recent progress.
        if elapsed > STALE.as_secs_f32() {
            self.progress_verified = false;
        }
        self.smoothed *= (-elapsed).exp();
        if self.sample != sample {
            self.progress_verified = elapsed <= STALE.as_secs_f32();
            self.sample = sample;
            self.received = now;
        }
        self.smoothed = self.smoothed.max(self.sample.hundredths_percent as f32);
    }
}

#[derive(Default)]
pub(super) struct State {
    pub(super) enabled: bool,
    identity: Option<(u64, u64, u64)>,
    availability: Availability,
    entries: BTreeMap<(u8, u64), Entry>,
    updated: Option<Instant>,
}

fn key(owner: OutputCaptureId) -> (u8, u64) {
    match owner {
        OutputCaptureId::Track(id) => (0, id.value()),
        OutputCaptureId::Return(id) => (1, id.value()),
    }
}

impl State {
    fn appearance(&self, owner: OutputCaptureId, now: Instant) -> (Availability, usize) {
        if self.availability != Availability::Fresh {
            return (self.availability, 0);
        }
        let Some(entry) = self.entries.get(&key(owner)) else {
            return (Availability::Waiting, 0);
        };
        if !entry.progress_verified {
            return (Availability::Waiting, 0);
        }
        if now.saturating_duration_since(entry.received) > STALE {
            return (Availability::Stale, 0);
        }
        (
            Availability::Fresh,
            THRESHOLDS
                .iter()
                .filter(|&&value| entry.smoothed >= value as f32)
                .count(),
        )
    }

    fn unavailable(&mut self, state: Availability) -> bool {
        let changed = self.availability != state || !self.entries.is_empty();
        self.availability = state;
        self.entries.clear();
        changed
    }
}

impl EditorView {
    pub(super) fn toggle_track_performance(&mut self) {
        self.track_performance.enabled = !self.track_performance.enabled;
        self.track_performance.updated = None;
        self.track_performance.identity = None;
        if !self.track_performance.enabled {
            if let Some(observation) = self
                .audible_playback
                .as_ref()
                .and_then(|p| p.observation.as_ref())
            {
                observation.set_render_work_enabled(false);
            }
            self.track_performance.unavailable(Availability::Waiting);
        }
    }

    pub(super) fn tick_track_performance(&mut self, now: Instant) -> bool {
        let state = &mut self.track_performance;
        if !state.enabled
            || state
                .updated
                .is_some_and(|previous| now.saturating_duration_since(previous) < REFRESH)
        {
            return false;
        }
        let elapsed = state.updated.map_or(0.0, |previous| {
            now.saturating_duration_since(previous).as_secs_f32()
        });
        state.updated = Some(now);
        let Some(audio) = self.audio.as_ref() else {
            return state.unavailable(Availability::Unavailable);
        };
        let Some(status) = audio.control.status() else {
            return state.unavailable(Availability::Unavailable);
        };
        if status.device_fault.load(Acquire)
            || status.render_fault.load(Acquire)
            || status.shutdown_complete.load(Acquire)
        {
            return state.unavailable(Availability::Fault);
        }
        let Some(source) = status.recording_source().filter(|source| {
            super::controller::publication_observation_for_view(
                self.project_generation,
                self.state.as_deref(),
                audio.control.output_identity(),
                source.context,
                source.output_identity,
            ) == super::controller::PublicationObservationDisposition::Apply
        }) else {
            return state.unavailable(Availability::Pending);
        };
        let Some(audible) = self.audible_playback.as_ref().filter(|audible| {
            audible.project_generation == self.project_generation
                && source.audio_revision == audible.audio_revision
        }) else {
            return state.unavailable(Availability::Pending);
        };
        let Some(observation) = audible.observation.as_ref() else {
            return state.unavailable(Availability::Unavailable);
        };
        if !status.playing.load(Acquire) {
            observation.set_render_work_enabled(false);
            state.identity = None;
            return state.unavailable(Availability::Stopped);
        }
        let identity = (
            self.project_generation,
            source.output_identity,
            audible.audio_revision,
        );
        let mut changed = false;
        if state.identity != Some(identity) {
            state.entries.clear();
            state.identity = Some(identity);
            observation.set_render_work_enabled(false);
            observation.set_render_work_enabled(true);
            changed = true;
        }
        changed |= state.availability != Availability::Fresh;
        state.availability = Availability::Fresh;
        for (owner, meter) in observation.render_work_meters() {
            let before = state.appearance(owner, now.checked_sub(REFRESH).unwrap_or(now));
            if let Some(sample) = meter.sample() {
                let entry = state.entries.entry(key(owner)).or_insert(Entry {
                    sample,
                    received: now,
                    smoothed: sample.hundredths_percent as f32,
                    progress_verified: false,
                });
                entry.observe(sample, now, elapsed);
            }
            changed |= before != state.appearance(owner, now);
        }
        // A concurrent handoff makes all staged readings pending immediately.
        if status.recording_source() != Some(source)
            || status.device_fault.load(Acquire)
            || status.render_fault.load(Acquire)
        {
            return state.unavailable(Availability::Pending) || changed;
        }
        changed
    }

    pub(super) fn track_performance_indicator(
        &self,
        owner: OutputCaptureId,
    ) -> Option<gpui::Stateful<Div>> {
        self.track_performance
            .enabled
            .then(|| render(&self.track_performance, owner, Instant::now()))
    }
}

fn render(state: &State, owner: OutputCaptureId, now: Instant) -> gpui::Stateful<Div> {
    let (availability, segments) = state.appearance(owner, now);
    let status = match availability {
        Availability::Waiting => "waiting for a sample",
        Availability::Unavailable => "timing unavailable for this output",
        Availability::Pending => "audio update pending",
        Availability::Stopped => "transport stopped",
        Availability::Fault => "audio unavailable",
        Availability::Stale => "sample stale",
        Availability::Fresh => "sampled audio-thread wall time",
    };
    let value = state
        .entries
        .get(&key(owner))
        .filter(|_| availability == Availability::Fresh)
        .map(|entry| {
            format!(
                " · {:.2}% of audio duration",
                entry.sample.hundredths_percent as f64 / 100.0
            )
        })
        .unwrap_or_default();
    let description = theme::text(format!(
        "track performance · {status}{value}\nlocal source and processing work; groups exclude children; shared instruments belong to the receiver. sampled in rotation; the observed peak decays over one second. brief spikes can be missed.\nsix steps: 0.1, 0.5, 1, 2.5, 5, 10%. main, old detached tails and shared overhead remain in the global meter. external worker cpu is excluded. this is not latency or proof of a dropout."
    ));
    let (kind, id) = key(owner);
    div()
        .id(gpui::ElementId::Name(
            format!("track-performance-{kind}-{id}").into(),
        ))
        .flex()
        .items_center()
        .gap(px(1.0))
        .w(px(23.0))
        .h(px(8.0))
        .flex_shrink_0()
        .debug_selector(move || format!("track-performance-{kind}-{id}"))
        .tooltip(move |_, cx| {
            cx.new(|_| super::transport::MiniTooltip {
                kind: crate::tooltips::Kind::Identification,
                description: description.clone(),
            })
            .into()
        })
        .children((0..6).map(|index| {
            div()
                .debug_selector(move || format!("track-performance-segment-{kind}-{id}-{index}"))
                .w(px(3.0))
                .h(px(5.0))
                .border_1()
                .border_color(rgb(theme::color(theme::PaletteRole::Muted)))
                .bg(rgb(theme::color(
                    if availability == Availability::Fresh && index < segments {
                        theme::PaletteRole::Secondary
                    } else {
                        theme::PaletteRole::Background
                    },
                )))
                .when(availability != Availability::Fresh, |segment| {
                    segment.opacity(0.35)
                })
        }))
}

#[cfg(test)]
mod tests;
