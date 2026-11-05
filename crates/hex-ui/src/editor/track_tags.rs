//! Compact tag controls and bounded menus over canonical track metadata.
//! Selection delegates to its existing owner; the cached visibility projection
//! owns the view filter. Neither interaction sends an audio/project edit.
use super::{EditorView, context_menu::EditorContextAction};
use crate::{
    context_menu::{MenuItem, PopupMenu},
    theme,
};
use gpui::{Context, Div, MouseButton, Pixels, Point, Window, div, prelude::*, px, rgb};
use hex_project::{Track, TrackId, TrackTag};
use std::collections::BTreeSet;
const PAGE: usize = 24;
#[derive(Default)]
pub(super) struct State {
    stamp: Option<(u64, u64)>,
    catalog: Vec<TrackTag>,
}
#[derive(Clone)]
pub(super) enum Action {
    Edit {
        generation: u64,
        revision: u64,
        track: TrackId,
    },
    Filter {
        generation: u64,
        tag: Option<TrackTag>,
    },
    Select {
        generation: u64,
        track: TrackId,
        tag: TrackTag,
    },
    Page {
        generation: u64,
        revision: u64,
        page: usize,
        position: Point<Pixels>,
    },
}
impl Action {
    pub(super) fn stale(&self, view: &EditorView) -> bool {
        match self {
            Self::Edit {
                generation,
                revision,
                track,
            } => {
                *generation != view.project_generation
                    || view.state.as_ref().is_none_or(|s| {
                        s.revision != *revision || s.project.track(*track).is_none()
                    })
            }
            Self::Page {
                generation,
                revision,
                ..
            } => {
                *generation != view.project_generation
                    || view.state.as_ref().is_none_or(|s| s.revision != *revision)
            }
            Self::Filter { generation, .. } | Self::Select { generation, .. } => {
                *generation != view.project_generation
            }
        }
    }
    pub(super) fn label(&self) -> gpui::SharedString {
        match self {
            Self::Edit { .. } => "edit tags · comma separated".into(),
            Self::Filter { tag: Some(tag), .. } => format!("filter: {}", tag.as_str()).into(),
            Self::Filter { tag: None, .. } => "clear tag filter".into(),
            Self::Select { tag, .. } => format!("select: {}", tag.as_str()).into(),
            Self::Page { page, .. } => format!("more tags · page {}", page + 1).into(),
        }
    }
}
fn item(action: Action, checked: bool) -> MenuItem<EditorContextAction> {
    MenuItem {
        label: "tag",
        swatch: None,
        checked,
        action: EditorContextAction::Tags(action),
    }
}
impl EditorView {
    pub(super) fn perform_tag_action(
        &mut self,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if action.stale(self) {
            return;
        }
        match action {
            Action::Edit {
                generation,
                revision,
                track,
            } => self.begin_captured_track_tags(track, generation, revision, window, cx),
            Action::Filter { tag, .. } => self.set_track_tag_filter(tag, cx),
            Action::Select { track, tag, .. } => {
                self.select_tag_tracks(&tag, gpui::Modifiers::default(), Some(track), window, cx)
            }
            Action::Page { page, position, .. } => {
                self.open_track_tag_filter(position, page, window, cx)
            }
        }
    }
    fn set_track_tag_filter(&mut self, tag: Option<TrackTag>, cx: &mut Context<Self>) {
        if !self.track_view_change_available(cx) {
            return;
        }
        self.arrangement_focus.projection.set_tag_filter(tag);
        self.reconcile_arrangement_focus();
        self.status = self.arrangement_focus.projection.tag_filter().map_or_else(
            || "tag filter cleared".into(),
            |tag| format!("tag filter: {}", tag.as_str()),
        );
        cx.notify();
    }
    fn tag_menu(
        &mut self,
        items: Vec<MenuItem<EditorContextAction>>,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !super::device_panel::finalize_controls(self, cx)
            || !self.track_view_change_available(cx)
        {
            return;
        }
        let previous_focus = window.focused(cx);
        let focus = cx.focus_handle();
        window.focus(&focus);
        self.context_menu = Some(PopupMenu {
            scroll: gpui::ScrollHandle::new(),
            position,
            icon_grid: None,
            items,
            selected: 0,
            focus,
            previous_focus,
        });
        cx.notify();
    }
    fn open_track_tag_filter(
        &mut self,
        position: Point<Pixels>,
        page: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = &self.state else { return };
        let generation = self.project_generation;
        let revision = state.revision;
        if self.track_tags.stamp != Some((generation, revision)) {
            self.track_tags.catalog = state
                .project
                .tracks()
                .iter()
                .flat_map(|track| track.tags().iter().cloned())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            self.track_tags.stamp = Some((generation, revision));
        }
        let page = page.min(self.track_tags.catalog.len().saturating_sub(1) / PAGE);
        let mut items = vec![item(
            Action::Filter {
                generation,
                tag: None,
            },
            self.arrangement_focus.projection.tag_filter().is_none(),
        )];
        items.extend(
            self.track_tags
                .catalog
                .iter()
                .skip(page * PAGE)
                .take(PAGE)
                .cloned()
                .map(|tag| {
                    let checked = self.arrangement_focus.projection.tag_filter() == Some(&tag);
                    item(
                        Action::Filter {
                            generation,
                            tag: Some(tag),
                        },
                        checked,
                    )
                }),
        );
        if page > 0 {
            items.push(item(
                Action::Page {
                    generation,
                    revision,
                    page: page - 1,
                    position,
                },
                false,
            ));
        }
        if (page + 1) * PAGE < self.track_tags.catalog.len() {
            items.push(item(
                Action::Page {
                    generation,
                    revision,
                    page: page + 1,
                    position,
                },
                false,
            ));
        }
        self.tag_menu(items, position, window, cx);
    }
    fn open_track_tag_choices(
        &mut self,
        track: TrackId,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = &self.state else { return };
        let Some(owner) = state.project.track(track) else {
            return;
        };
        let generation = self.project_generation;
        let revision = state.revision;
        let mut items = vec![item(
            Action::Edit {
                generation,
                revision,
                track,
            },
            false,
        )];
        for tag in owner.tags() {
            items.push(item(
                Action::Select {
                    generation,
                    track,
                    tag: tag.clone(),
                },
                false,
            ));
            items.push(item(
                Action::Filter {
                    generation,
                    tag: Some(tag.clone()),
                },
                self.arrangement_focus.projection.tag_filter() == Some(tag),
            ));
        }
        self.tag_menu(items, position, window, cx);
    }
    pub(super) fn track_tags_header(
        &mut self,
        track: &Track,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Stateful<Div>> {
        let id = track.id();
        if let Some(input) = self.track_tags_input(id) {
            return Some(
                div()
                    .id(("track-tags-editor", id.value()))
                    .debug_selector(move || format!("track-tags-input-{}", id.value()))
                    .w(px(92.))
                    .min_w_0()
                    .child(input),
            );
        }
        let first = track.tags().first()?.clone();
        let click = first.clone();
        let all = track
            .tags()
            .iter()
            .map(|tag| tag.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        Some(div().id(("track-tags-row",id.value())).flex().items_center().flex_shrink_0().max_w(px(76.)).text_xs()
            .debug_selector(move || format!("track-tags-{}",id.value()))
            .tooltip(move |_,cx| cx.new(|_| super::transport::MiniTooltip { kind:crate::tooltips::Kind::control("track tags"),description:format!("{all} · click selects matches · shift adds · cmd/ctrl toggles · right click filters").into() }).into())
            .child(div().id(("track-tag",id.value())).debug_selector(move || format!("track-tag-first-{}",id.value())).cursor_pointer().max_w(px(54.)).truncate().px(px(2.)).rounded(px(2.)).bg(rgb(theme::color(theme::PaletteRole::Raised)))
                .on_mouse_down(MouseButton::Left,|_,_,cx| cx.stop_propagation())
                .on_click(cx.listener(move |view,event:&gpui::ClickEvent,window,cx| { cx.stop_propagation(); view.select_tag_tracks(&click,event.modifiers(),Some(id),window,cx); }))
                .on_mouse_down(MouseButton::Right,cx.listener(move |view,event:&gpui::MouseDownEvent,window,cx| { cx.stop_propagation();view.open_track_tag_choices(id,event.position,window,cx); }))
                .child(theme::text(first.as_str())))
            .child(div().id(("track-tag-more",id.value())).debug_selector(move || format!("track-tag-more-{}",id.value())).cursor_pointer().px(px(2.))
                .on_mouse_down(MouseButton::Left,cx.listener(move |view,event:&gpui::MouseDownEvent,window,cx| { cx.stop_propagation();view.open_track_tag_choices(id,event.position,window,cx); }))
                .child(theme::text(if track.tags().len() > 1 { format!("+{}",track.tags().len()-1) } else { "⋯".into() }))))
    }
    pub(super) fn track_tag_filter_control(
        &mut self,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let active = self.arrangement_focus.projection.tag_filter().cloned();
        let label = active
            .as_ref()
            .map_or_else(|| "tags".into(), |tag| format!("tag: {}", tag.as_str()));
        div()
            .id("track-tag-filter-control")
            .debug_selector(|| "track-tag-filter-control".into())
            .flex()
            .items_center()
            .h(px(20.))
            .max_w(px(110.))
            .text_xs()
            .child(
                div()
                    .id("track-tag-filter-menu")
                    .debug_selector(|| "track-tag-filter-menu".into())
                    .cursor_pointer()
                    .truncate()
                    .px(px(3.))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|view, event: &gpui::MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            view.open_track_tag_filter(event.position, 0, window, cx);
                        }),
                    )
                    .child(theme::text(label)),
            )
            .children(active.map(|_| {
                div()
                    .id("clear-track-tag-filter")
                    .debug_selector(|| "clear-track-tag-filter".into())
                    .cursor_pointer()
                    .px(px(3.))
                    .on_click(cx.listener(|view, _, _, cx| {
                        cx.stop_propagation();
                        view.set_track_tag_filter(None, cx);
                    }))
                    .child(theme::text("×"))
            }))
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests;
