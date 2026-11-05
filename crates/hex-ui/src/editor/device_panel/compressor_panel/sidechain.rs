//! Acknowledged Compressor key selection and the clipped track-drop target.
use crate::{editor::EditorView, theme};
use gpui::{
    Bounds, Context, Div, Entity, Pixels, Point, Subscription, canvas, div, prelude::*, px, rgb,
};
use hex_project::{Command, DeviceId, TrackId};
use hex_session::service::Request;
use hex_widgets::{Selector, SelectorAppearance, SelectorChanged, SelectorConfig};
use std::{cell::Cell, collections::HashMap, rc::Rc, sync::Arc};

type Epoch = (u64, u64);

#[derive(Default)]
pub(in crate::editor) struct Controls {
    epoch: Option<Epoch>,
    entries: HashMap<DeviceId, Control>,
    visible: Vec<DeviceId>,
}
struct Control {
    selector: Entity<Selector>,
    _subscription: Subscription,
    sources: Arc<[Option<TrackId>]>,
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}

impl Controls {
    pub(in crate::editor) fn clear_visible(&mut self) {
        self.visible.clear();
    }
}

fn epoch(view: &EditorView) -> Option<Epoch> {
    Some((view.project_generation, view.state.as_ref()?.revision))
}

fn submit(
    view: &mut EditorView,
    device: DeviceId,
    source: Option<TrackId>,
    cx: &mut Context<EditorView>,
) {
    if view.edit_pending() || !super::super::finalize_controls(view, cx) {
        return;
    }
    let _ = view.enqueue(Request::Edit(Command::SetSidechain { device, source }));
    cx.notify();
}

pub(super) fn control(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
) -> Option<Div> {
    let current = epoch(view)?;
    let drag = view.sidechain_drag();
    let controls = &mut view.device_panel.compressor_sidechains;
    if controls.epoch != Some(current) {
        controls.entries.clear();
        controls.epoch = Some(current);
    }
    if let std::collections::hash_map::Entry::Vacant(entry) = controls.entries.entry(device) {
        let project = &view.state.as_ref()?.project;
        let sources: Arc<[_]> = std::iter::once(None)
            .chain(
                project
                    .eligible_sidechain_sources(device)
                    .ok()?
                    .into_iter()
                    .map(Some),
            )
            .collect();
        let committed = project
            .sidechains()
            .iter()
            .find(|route| route.device == device)
            .map(|route| route.source);
        let selected = sources.iter().position(|&source| source == committed)?;
        let labels = sources.iter().map(|source| {
            let track = source.and_then(|source| {
                project
                    .tracks()
                    .iter()
                    .enumerate()
                    .find(|(_, track)| track.id() == source)
            });
            track.map_or_else(
                || theme::text("internal"),
                |(index, track)| {
                    theme::text(format!(
                        "{} {}",
                        index + 1,
                        hex_session::service::text::verbatim(track.name())
                    ))
                },
            )
        });
        let config = SelectorConfig::new(
            format!("compressor-sidechain-{}", device.value()),
            labels,
            selected,
            theme::control_style(),
        )
        .ok()?
        .with_accessibility_name(theme::text("sidechain source"))
        .with_appearance(SelectorAppearance::CompactSelector);
        let selector = cx.new(|cx| Selector::from_config(cx, config));
        let options = Arc::clone(&sources);
        let subscription = cx.subscribe(
            &selector,
            move |view, emitter, event: &SelectorChanged, cx| {
                let state = &view.device_panel.compressor_sidechains;
                if epoch(view) != Some(current)
                    || !state.visible.contains(&device)
                    || state
                        .entries
                        .get(&device)
                        .is_none_or(|control| control.selector.entity_id() != emitter.entity_id())
                {
                    return;
                }
                // The displayed source follows the acknowledged project, including
                // worker refusal and Undo. A widget click is only a request.
                emitter.update(cx, |selector, cx| {
                    let _ = selector.set_selected(selected, cx);
                });
                if event.index != selected
                    && let Some(&source) = options.get(event.index)
                {
                    submit(view, device, source, cx);
                }
            },
        );
        entry.insert(Control {
            selector,
            _subscription: subscription,
            sources,
            bounds: Default::default(),
        });
    }

    let controls = &mut view.device_panel.compressor_sidechains;
    let control = controls.entries.get(&device)?;
    controls.visible.push(device);
    control.bounds.set(None);
    let target = Rc::clone(&control.bounds);
    let selector = control.selector.clone();
    let source = drag.filter(|(source, _)| control.sources.contains(&Some(*source)));
    let accent = rgb(theme::color(theme::PaletteRole::Primary));
    Some(
        div()
            .h(px(14.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(3.0))
            .child(theme::text("sidechain"))
            .child(
                div().relative().flex_1().min_w_0().child(selector).child(
                    canvas(
                        move |bounds, window, _| {
                            let mask = window.content_mask().bounds;
                            let clipped = bounds.intersects(&mask).then(|| bounds.intersect(&mask));
                            target.set(clipped);
                            source.is_some_and(|(_, point)| {
                                clipped.is_some_and(|visible| visible.contains(&point))
                            })
                        },
                        move |bounds, highlighted, window, _| {
                            if highlighted {
                                window.paint_quad(gpui::outline(
                                    bounds,
                                    accent,
                                    gpui::BorderStyle::Solid,
                                ));
                            }
                        },
                    )
                    .absolute()
                    .inset_0(),
                ),
            ),
    )
}

pub(in crate::editor) fn drop_track_key(
    view: &mut EditorView,
    source: TrackId,
    position: Point<Pixels>,
    cx: &mut Context<EditorView>,
) -> bool {
    let controls = &view.device_panel.compressor_sidechains;
    if !view.device_panel_active || !view.clip_view_open || controls.epoch != epoch(view) {
        return false;
    }
    let Some((device, eligible)) = controls.visible.iter().find_map(|&device| {
        let control = controls.entries.get(&device)?;
        control
            .bounds
            .get()
            .is_some_and(|bounds| bounds.contains(&position))
            .then(|| (device, control.sources.contains(&Some(source))))
    }) else {
        return false;
    };
    if eligible {
        submit(view, device, Some(source), cx);
    } else {
        view.error = Some("track cannot feed this compressor sidechain".into());
        cx.notify();
    }
    true
}
