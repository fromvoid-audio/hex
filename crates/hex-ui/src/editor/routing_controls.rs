//! Canonical routing controls shared by Arrangement and Session.
use super::{EditorView, device_panel};
use crate::theme;
use gpui::{Context, Div, div, prelude::*, px, rgb};
use hex_project::{Command, RoutingOwner};

impl EditorView {
    pub(super) fn routing_controls(&self, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let mut panel = div().id("routing-controls").flex_shrink_0();
        if !self.routing_open {
            return panel;
        }
        let Some(state) = self.state.as_ref() else {
            return panel;
        };
        panel = panel
            .max_h(px(130.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .px_2()
            .py_1()
            .gap_1()
            .text_xs()
            .bg(rgb(theme::color(theme::PaletteRole::Surface)))
            .border_t_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)));
        if let Some(destination) = self.focused_return {
            if let Some(bus) = state
                .project
                .return_tracks()
                .iter()
                .find(|bus| bus.id() == destination)
            {
                panel = panel.child(theme::text(format!("{} · return inputs", bus.name())));
                for source in state
                    .project
                    .return_tracks()
                    .iter()
                    .filter(|bus| bus.id() != destination)
                {
                    let id = source.id();
                    let gain = state
                        .project
                        .authored_routing()
                        .iter()
                        .find(|feed| feed.source() == id && feed.destination() == destination)
                        .map_or(0.0, |feed| feed.gain());
                    panel = panel.child(gain_row(
                        source.name(),
                        gain,
                        format!("return-feed-{}-{}", id.value(), destination.value()),
                        move |gain| Command::SetReturnFeed {
                            source: id,
                            destination,
                            gain,
                        },
                        cx,
                    ));
                }
            }
            return panel;
        }
        let Some(destination) = self.selected_track.and_then(|id| state.project.track(id)) else {
            return panel.child(theme::text("select a track to edit routing"));
        };
        let destination_id = destination.id();
        let count =
            state.project.tracks().len().saturating_sub(1) + state.project.return_tracks().len();
        let last_page = count.saturating_sub(1) / 16;
        let page = self.routing_page.min(last_page);
        if last_page != 0 {
            panel = panel.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        button("◀", "previous routing sources", "routing-previous".into())
                            .on_click(cx.listener(move |view, _, _, cx| {
                                view.routing_page = page.saturating_sub(1);
                                cx.notify();
                            })),
                    )
                    .child(theme::text(format!(
                        "sources · page {} of {}",
                        page + 1,
                        last_page + 1
                    )))
                    .child(
                        button("▶", "next routing sources", "routing-next".into()).on_click(
                            cx.listener(move |view, _, _, cx| {
                                view.routing_page = (page + 1).min(last_page);
                                cx.notify();
                            }),
                        ),
                    ),
            );
        }

        panel = panel.child(theme::text(format!(
            "{} · additive stereo inputs before effects",
            destination.name()
        )));
        for source in state
            .project
            .tracks()
            .iter()
            .filter(|track| track.id() != destination_id)
            .map(|track| (RoutingOwner::Track(track.id()), track.name()))
            .chain(
                state
                    .project
                    .return_tracks()
                    .iter()
                    .map(|bus| (RoutingOwner::Return(bus.id()), bus.name())),
            )
            .skip(page * 16)
            .take(16)
        {
            let (owner, name) = source;
            let gain = state
                .project
                .audio_feeds()
                .iter()
                .find(|feed| feed.source() == owner && feed.destination() == destination_id)
                .map_or(0.0, |feed| feed.gain());
            let key = match owner {
                RoutingOwner::Track(id) => format!("track-{}", id.value()),
                RoutingOwner::Return(id) => format!("return-{}", id.value()),
                RoutingOwner::Main => continue,
            };
            panel = panel.child(gain_row(
                name,
                gain,
                format!("audio-feed-{key}-{}", destination_id.value()),
                move |gain| Command::SetAudioFeed {
                    source: owner,
                    destination: destination_id,
                    gain,
                },
                cx,
            ));
        }
        if destination.instrument().is_some() {
            panel = panel.child(theme::text(
                "midi source · replaces local notes; upstream routes follow their source",
            ));
            let selected = state
                .project
                .midi_routes()
                .iter()
                .find(|route| route.destination() == destination_id)
                .map(|route| route.source());
            let source_name = selected
                .and_then(|id| state.project.track(id))
                .map_or("local", |track| track.name());
            panel = panel.child(theme::text(format!("selected midi source · {source_name}")));
            let mut sources = div().flex().flex_wrap().gap_1();
            for (source, name) in std::iter::once((None, "local")).chain(
                state
                    .project
                    .tracks()
                    .iter()
                    .filter(|track| track.id() != destination_id)
                    .map(|track| (Some(track.id()), track.name()))
                    .skip(page * 16)
                    .take(16),
            ) {
                let key = source.map_or(0, |id| id.value());
                let label = format!("{} {}", if selected == source { "●" } else { "○" }, name);
                sources = sources.child(
                    button(
                        label,
                        "select midi event source",
                        format!("midi-route-{key}"),
                    )
                    .on_click(cx.listener(move |view, _, _, cx| {
                        device_panel::enqueue_return_edit(
                            view,
                            Command::SetMidiSource {
                                destination: destination_id,
                                source,
                            },
                            cx,
                        );
                        cx.notify();
                    })),
                );
            }
            panel = panel.child(sources);
        }
        panel
    }
}
fn gain_row(
    name: &str,
    gain: f32,
    key: String,
    command: impl Fn(f32) -> Command + Copy + 'static,
    cx: &mut Context<EditorView>,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap_1()
        .child(div().w(px(160.0)).truncate().child(theme::text(name)))
        .child(
            button("−", "decrease feed gain", format!("{key}-down")).on_click(cx.listener(
                move |view, _, _, cx| {
                    device_panel::enqueue_return_edit(
                        view,
                        command((gain - 0.1).clamp(0.0, 2.0)),
                        cx,
                    );
                    cx.notify();
                },
            )),
        )
        .child(
            div()
                .w(px(40.0))
                .child(theme::text(format!("{:.0}%", gain * 100.0))),
        )
        .child(
            button("+", "increase feed gain", format!("{key}-up")).on_click(cx.listener(
                move |view, _, _, cx| {
                    device_panel::enqueue_return_edit(
                        view,
                        command((gain + 0.1).clamp(0.0, 2.0)),
                        cx,
                    );
                    cx.notify();
                },
            )),
        )
        .child(
            button("×", "remove feed", format!("{key}-remove")).on_click(cx.listener(
                move |view, _, _, cx| {
                    device_panel::enqueue_return_edit(view, command(0.0), cx);
                    cx.notify();
                },
            )),
        )
}

fn button(label: impl Into<String>, tooltip: &'static str, key: String) -> gpui::Stateful<Div> {
    hex_widgets::action(theme::text(label.into()), theme::action_style())
        .id(gpui::ElementId::Name(key.clone().into()))
        .debug_selector(move || key.clone())
        .px_1()
        .h(px(21.0))
        .tooltip(move |_, cx| {
            cx.new(|_| super::transport::MiniTooltip {
                kind: crate::tooltips::Kind::Identification,
                description: tooltip.into(),
            })
            .into()
        })
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;
    use gpui::{Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, size};
    use hex_project::{Created, Editor, Project, TrackId};
    use std::sync::Arc;
    fn fixture(
        cx: &mut TestAppContext,
    ) -> (gpui::AnyWindowHandle, Entity<EditorView>, TrackId, TrackId) {
        let mut editor = Editor::new(Project::new(48000).unwrap(), 8).unwrap();
        let mut tracks = Vec::new();
        for name in ["source", "destination"] {
            let Created::Track(track) = editor
                .execute(Command::InsertTrackWithDevice {
                    name: name.into(),
                    before_track: None,
                    slot: hex_project::TrackDeviceSlot::Instrument,
                    state: hex_project::default_instrument_state()
                        .expect("fixture instrument state"),
                })
                .unwrap()
            else {
                panic!("track")
            };
            tracks.push(track);
        }
        let (source, destination) = (tracks[0], tracks[1]);
        editor
            .execute(Command::SetAudioFeed {
                source: RoutingOwner::Track(source),
                destination,
                gain: 0.5,
            })
            .unwrap();
        editor
            .execute(Command::SetMidiSource {
                destination,
                source: Some(source),
            })
            .unwrap();
        let (handle, entity) = super::super::clip_drag_ui_test::editor_window(
            cx,
            editor.project().clone(),
            Arc::default(),
        );
        entity.update(cx, |view, cx| {
            view.routing_open = true;
            view.selected_track = Some(destination);
            view.clip_view_open = false;
            cx.notify();
        });
        (handle, entity, source, destination)
    }
    #[gpui::test]
    fn routing_panel_edits_a_feed_through_the_canonical_command_at_minimum_size(
        cx: &mut TestAppContext,
    ) {
        let (handle, entity, source, destination) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(size(px(880.0), px(540.0)));
        visual.run_until_parked();
        let bounds = visual
            .debug_bounds(Box::leak(
                format!(
                    "audio-feed-track-{}-{}-up",
                    source.value(),
                    destination.value()
                )
                .into_boxed_str(),
            ))
            .unwrap();
        visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
        entity.update(&mut visual, |view, _| { assert!(matches!(view.last_enqueued_edit, Some(Command::SetAudioFeed { source: RoutingOwner::Track(actual), destination: target, gain }) if actual == source && target == destination && (gain - 0.6).abs() < 0.00001)); });
    }
    #[gpui::test]
    fn selecting_local_midi_restores_the_destination_authored_source(cx: &mut TestAppContext) {
        let (handle, entity, _, destination) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let bounds = visual.debug_bounds("midi-route-0").unwrap();
        visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
        entity.update(&mut visual, |view, _| { assert!(matches!(view.last_enqueued_edit, Some(Command::SetMidiSource { destination: target, source: None }) if target == destination)); });
    }
}
