//! Read-only track latency from the acknowledged graph. Unlike sampled render
//! work, graph latency remains meaningful when transport is stopped.
use super::{
    EditorView,
    controller::{PublicationObservationDisposition, publication_observation_for_view},
    transport::MiniTooltip,
};
use crate::{theme, tooltips};
use gpui::{Div, Stateful, div, prelude::*, px, rgb};
use hex_engine::{OutputLatency, PreparedPlaybackObservation};
use hex_project::{ReturnId, TrackId};
use std::sync::atomic::Ordering::Acquire;

#[derive(Clone, Copy)]
pub(super) enum Owner {
    Track(TrackId),
    Return(ReturnId),
    Main,
}

enum Reading {
    Unavailable,
    Pending,
    Applied(OutputLatency, u32, Option<u32>),
}

impl EditorView {
    fn latency_reading(&self, owner: Owner) -> Reading {
        let Some(audio) = self.audio.as_ref() else {
            return Reading::Unavailable;
        };
        let Some(status) = audio.control.status() else {
            return Reading::Unavailable;
        };
        if status.device_fault.load(Acquire)
            || status.render_fault.load(Acquire)
            || status.shutdown_complete.load(Acquire)
        {
            return Reading::Unavailable;
        }
        let Some(source) = status.recording_source().filter(|source| {
            publication_observation_for_view(
                self.project_generation,
                self.state.as_deref(),
                audio.control.output_identity(),
                source.context,
                source.output_identity,
            ) == PublicationObservationDisposition::Apply
        }) else {
            return Reading::Pending;
        };
        let Some(candidate) = self.audible_playback.as_ref().filter(|candidate| {
            candidate.project_generation == self.project_generation
                && candidate.audio_revision == source.audio_revision
        }) else {
            return Reading::Pending;
        };
        let reading = candidate
            .observation
            .as_deref()
            .map_or(Reading::Unavailable, |observation| {
                from_observation(owner, observation)
            });
        if status.recording_source() != Some(source) {
            return Reading::Pending;
        }
        reading
    }

    pub(super) fn track_latency_indicator(&self, owner: Owner) -> Option<Stateful<Div>> {
        self.show_track_latency
            .then(|| render(owner, self.latency_reading(owner)))
    }
}

fn from_observation(owner: Owner, observation: &PreparedPlaybackObservation) -> Reading {
    let facts = match owner {
        Owner::Track(id) => observation.track_latency(id),
        Owner::Return(id) => observation.return_latency(id),
        Owner::Main => observation.main_latency(),
    };
    facts.map_or(Reading::Unavailable, |facts| {
        Reading::Applied(
            facts,
            observation.sample_rate(),
            observation.main_latency().map(|main| main.arrival_frames),
        )
    })
}

fn duration(frames: u32, rate: u32) -> String {
    let milliseconds = 1000.0 * f64::from(frames) / f64::from(rate);
    format!("{frames} samples ({milliseconds:.3} ms)")
}

fn presentation(owner: Owner, reading: Reading) -> (&'static str, theme::PaletteRole, String) {
    match reading {
        Reading::Unavailable => (
            "·",
            theme::PaletteRole::Muted,
            "track latency unavailable".into(),
        ),
        Reading::Pending => (
            "…",
            theme::PaletteRole::Muted,
            "waiting for applied track latency".into(),
        ),
        Reading::Applied(facts, rate, main) => {
            let color = if facts.local_frames > 0 {
                theme::PaletteRole::Primary
            } else {
                theme::PaletteRole::Muted
            };
            let local = duration(facts.local_frames, rate);
            let arrival = duration(facts.arrival_frames, rate);
            let detail = if matches!(owner, Owner::Main) {
                format!("main devices · {local}\nmain output arrival · {arrival}")
            } else {
                let compensation = duration(facts.default_route_delay_frames, rate);
                let main = main.map_or_else(|| "unknown".into(), |frames| duration(frames, rate));
                format!(
                    "local devices · {local}\npost-fader arrival · {arrival}\ndefault route compensation · {compensation}\nmain output arrival · {main}"
                )
            };
            (
                "lat",
                color,
                detail
                    + "\ndeclared processing delay only. sends can have separate alignment delays.",
            )
        }
    }
}

fn render(owner: Owner, reading: Reading) -> Stateful<Div> {
    let (label, color, description) = presentation(owner, reading);
    let description = theme::text(description);
    let id = match owner {
        Owner::Track(id) => format!("track-latency-{}", id.value()),
        Owner::Return(id) => format!("return-latency-{}", id.value()),
        Owner::Main => "main-latency".into(),
    };
    div()
        .id(gpui::ElementId::Name(id.clone().into()))
        .debug_selector(move || id.clone())
        .w(px(21.0))
        .flex_shrink_0()
        .text_size(px(9.0))
        .text_center()
        .text_color(rgb(theme::color(color)))
        .tooltip(move |_, cx| {
            cx.new(|_| MiniTooltip {
                kind: tooltips::Kind::Identification,
                description: description.clone(),
            })
            .into()
        })
        .child(theme::text(label))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_pending_and_unavailable_are_distinct_and_use_the_applied_rate() {
        let facts = OutputLatency {
            local_frames: 0,
            arrival_frames: 48,
            default_route_delay_frames: 0,
        };
        let (label, _, detail) =
            presentation(Owner::Main, Reading::Applied(facts, 96_000, Some(48)));
        assert_eq!(label, "lat");
        assert!(detail.contains("main devices · 0 samples (0.000 ms)"));
        assert!(detail.contains("main output arrival · 48 samples (0.500 ms)"));
        assert_eq!(presentation(Owner::Main, Reading::Pending).0, "…");
        assert_eq!(presentation(Owner::Main, Reading::Unavailable).0, "·");
    }

    #[cfg(feature = "ui-test-support")]
    #[gpui::test]
    fn view_action_mounts_the_indicator_in_actual_headers(cx: &mut gpui::TestAppContext) {
        use crate::editor::{EditorMode, ToggleTrackLatency, clip_drag_ui_test::editor_window};
        use gpui::{VisualTestContext, size};
        use hex_project::{Command, Editor, Project};
        use std::sync::Arc;

        let mut model = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
        model
            .execute(Command::AddTrack {
                name: "track".into(),
            })
            .unwrap();
        let (window, editor) = editor_window(cx, model.project().clone(), Arc::default());
        let mut visual = VisualTestContext::from_window(window, cx);
        visual.simulate_resize(size(px(1280.0), px(800.0)));
        visual.run_until_parked();
        assert!(visual.debug_bounds("track-latency-1").is_none());
        visual.update(|window, cx| window.dispatch_action(Box::new(ToggleTrackLatency), cx));
        visual.run_until_parked();
        let track = visual.debug_bounds("track-latency-1").unwrap();
        let header = visual.debug_bounds("arrangement-track-header-1").unwrap();
        assert_eq!(track.size.width, px(21.0));
        assert!(header.contains(&track.center()));
        assert!(visual.debug_bounds("main-latency").is_some());
        editor.update(cx, |view, cx| {
            view.mode = EditorMode::Session;
            cx.notify();
        });
        visual.run_until_parked();
        assert_eq!(
            visual.debug_bounds("track-latency-1").unwrap().size.width,
            px(21.0)
        );
        visual.update(|window, cx| window.dispatch_action(Box::new(ToggleTrackLatency), cx));
        visual.run_until_parked();
        assert!(visual.debug_bounds("track-latency-1").is_none());
    }
}
