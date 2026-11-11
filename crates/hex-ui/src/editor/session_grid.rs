//! Compact, ID-backed Session grid and guarded playback requests.
mod actions;
pub(crate) use actions::PendingPlaybackAction;
use actions::with_quantization;
pub(super) use actions::{DeleteTarget, action_is_current, launch_action, resolve_delete_target};

use super::console_columns::ColumnFrame;
use super::console_track_page::TrackPage;
use super::session_route_view::cell_route_markers;
use super::{EditorMode, EditorView};
use crate::theme;
use gpui::{Context, Div, ElementId, Window, div, prelude::*, px, rgb, uniform_list};
use hex_audio::prepared::{
    SessionRouteEntry, SessionRouteOwner, SessionRouteSnapshot, SessionRouteTarget,
};
use hex_engine::{LaunchQuantization, SessionPlaybackCommand};
use hex_project::{
    BeatTime, ClipId, Command, SceneId, SessionCellContent, SessionSceneTarget, TrackId,
};
use hex_session::{import_batch::ConsoleDestination, service::Request};
use std::sync::Arc;

const TRACK_COLUMN_WIDTH: f32 = 148.0;
const SCENE_ROW_HEIGHT: f32 = 31.0;
// Shared by the pinned Return bank so scene and mixer columns stay aligned.
pub(super) const TRACK_HEADER_HEIGHT: f32 = 28.0;
// Eight nested ancestors fit while preserving a usable header label.
const GROUP_BAND_HEIGHT: f32 = 1.0;
const SCENE_HEADER_WIDTH: f32 = 110.0;

/// Paint occupies an inset surface; the cell keeps its original IDs, layout,
/// rename/launch controls and drop targets. Console clips are one caption row,
/// so their entire surface uses the header fill without an arrangement divider.
fn session_clip_backdrop(clip: ClipId, fill: u32, stroke: f32) -> Div {
    let outline = theme::color(theme::PaletteRole::ClipOutline);
    div()
        .debug_selector(move || format!("console-clip-{}", clip.value()))
        .absolute()
        .inset(px(2.0))
        .bottom(px(3.0))
        .child(super::clip_boundary::backdrop_shell(
            div(),
            0.0,
            1.0,
            super::clip_boundary::Backdrop {
                body: fill,
                header: fill,
                outline,
                hover_outline: outline,
                stroke,
                bottom_radius: 0.0,
                header_height: SCENE_ROW_HEIGHT,
                header_gradient: None,
                notch: super::clip_boundary::Notch::new(true),
            },
            true,
            move || format!("console-clip-trailing-{}", clip.value()),
        ))
}

#[derive(Clone)]
struct SessionRowContext {
    frame: ColumnFrame,
    revision: u64,
    generation: u64,
    page: Arc<TrackPage>,
    route_snapshot: Option<SessionRouteSnapshot>,
    routes_ready: bool,
    transport_playing: bool,
    stroke: f32,
    return_width: f32,
}

#[derive(Clone, Copy)]
enum CellExpectation {
    Empty,
    Stop,
    Midi(ClipId),
    Audio(ClipId),
}

impl EditorView {
    pub(super) fn navigate_session(
        &mut self,
        direction: super::session_navigation::Direction,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        if self.opening_project || self.close_prompt_open {
            return;
        }
        let selection = self.state.as_ref().and_then(|state| {
            super::session_navigation::resolve_visible_navigation(
                &state.project,
                &self.arrangement_focus.projection.console_indices,
                self.selected_cell,
                self.selected_scene,
                direction,
            )
        });
        match selection {
            Some(super::session_navigation::Selection::Scene(scene)) => {
                if self.focus_empty_details(window, cx) {
                    self.advance_selection_intent();
                    self.select_session_scene(scene);
                }
            }
            Some(super::session_navigation::Selection::Cell { track, scene }) => {
                let content = self
                    .state
                    .as_ref()
                    .and_then(|state| state.project.session_cell(track, scene))
                    .and_then(|content| match content {
                        SessionCellContent::Midi(clip) => Some(Some(clip.id())),
                        SessionCellContent::Audio(_) => Some(None),
                        SessionCellContent::Stop => None,
                    });
                match content {
                    Some(Some(clip)) => {
                        if self.focus_clip_details(window, cx) {
                            self.advance_selection_intent();
                            self.select_session_midi(track, scene, clip);
                        }
                    }
                    Some(None) => {
                        if self.focus_clip_details(window, cx) {
                            self.advance_selection_intent();
                            self.select_session_cell(track, scene);
                        }
                    }
                    None => {
                        self.focus_track_details(window);
                        self.advance_selection_intent();
                        self.select_session_cell(track, scene);
                    }
                }
            }
            None => {}
        }
    }

    pub(super) fn launch_session_selection(&mut self) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let revision = state.revision;
        if self.selected_cell.is_some_and(|(track, _)| {
            !self
                .arrangement_focus
                .projection
                .console_indices
                .iter()
                .any(|index| {
                    state
                        .project
                        .tracks()
                        .get(*index)
                        .is_some_and(|item| item.id() == track)
                })
        }) {
            return;
        }
        let Some(target) = super::session_navigation::resolve_enter_target(
            &state.project,
            self.selected_cell,
            self.selected_scene,
        ) else {
            return;
        };
        match target {
            super::session_navigation::EnterTarget::LaunchClip { track, clip } => {
                self.launch_clip(track, clip, revision, self.session_quantization);
            }
            super::session_navigation::EnterTarget::StopTrack(track) => {
                self.queue_session_command(
                    SessionPlaybackCommand::StopTrack {
                        track,
                        quantization: self.session_quantization,
                    },
                    revision,
                );
            }
            super::session_navigation::EnterTarget::LaunchScene(scene) => {
                self.launch_scene(scene, revision, self.session_quantization);
            }
        }
    }

    pub(super) fn duplicate_selected_scene(&mut self) {
        let Some((scene, revision, generation)) = self.selected_scene.and_then(|scene| {
            self.state
                .as_ref()
                .map(|state| (scene, state.revision, self.project_generation))
        }) else {
            return;
        };
        self.duplicate_scene_if_current(scene, revision, generation);
    }

    pub(super) fn move_selected_scene(&mut self, direction: super::session_navigation::Direction) {
        let Some((scene, revision, generation)) = self.selected_scene.and_then(|scene| {
            self.state
                .as_ref()
                .map(|state| (scene, state.revision, self.project_generation))
        }) else {
            return;
        };
        self.move_scene_if_current(scene, direction, revision, generation);
    }

    pub(super) fn duplicate_scene_if_current(
        &mut self,
        scene: SceneId,
        revision: u64,
        generation: u64,
    ) {
        if self.scene_edit_is_current(scene, revision, generation)
            && self.enqueue(Request::Edit(Command::DuplicateScene(scene)))
        {
            self.pending_scene_duplicate = Some(super::PendingSceneDuplicate {
                source_scene: scene,
                project_generation: generation,
                select_on_completion: true,
            });
        }
    }

    pub(super) fn cancel_scene_duplicate_selection(&mut self) {
        if let Some(intent) = &mut self.pending_scene_duplicate {
            intent.select_on_completion = false;
        }
    }

    pub(super) fn move_scene_if_current(
        &mut self,
        scene: SceneId,
        direction: super::session_navigation::Direction,
        revision: u64,
        generation: u64,
    ) {
        let Some(target) = self.state.as_ref().and_then(|state| {
            super::session_navigation::resolve_scene_move(&state.project, scene, direction)
        }) else {
            return;
        };
        if self.scene_edit_is_current(scene, revision, generation) {
            self.enqueue(Request::Edit(Command::MoveScene {
                scene: target.scene,
                before: target.before,
            }));
        }
    }

    fn scene_edit_is_current(&self, scene: SceneId, revision: u64, generation: u64) -> bool {
        self.mode == EditorMode::Session
            && self.selected_cell.is_none()
            && self.selected_scene == Some(scene)
            && self.project_generation == generation
            && self.state.as_ref().is_some_and(|state| {
                state.revision == revision
                    && state.project.scenes().iter().any(|item| item.id() == scene)
            })
            && !self.edit_pending()
            && self.recovery.generation().is_none()
            && self.pending_open_path.is_none()
            && !self.save_dialog_open
            && !self.export_dialog_open
            && !self.export_pending
            && !self.saving_project
    }

    pub(super) fn session_grid(&mut self, window: &Window, cx: &mut Context<Self>) -> Div {
        let Some(state) = self.state.clone() else {
            return div().flex_1().min_h_0();
        };
        let revision = state.revision;
        let generation = self.project_generation;
        let tracks = state.project.tracks();
        let scenes = state.project.scenes();
        let audible = self.audible_playback.as_ref().map(|candidate| {
            (
                candidate.audio_revision,
                candidate.state.revision,
                candidate.project_generation,
            )
        });
        let route_snapshot = self.audio.as_ref().and_then(|audio| {
            self.session_routes.matching_snapshot(
                audio.control.output_identity()?,
                audible,
                revision,
                generation,
            )
        });
        let transport_playing = self
            .audio_status()
            .is_some_and(|status| status.playing.load(std::sync::atomic::Ordering::Acquire));
        let routes_ready = route_snapshot.is_some_and(|snapshot| {
            self.session_routes
                .routes_available(snapshot, &state.project, revision, generation)
        });
        let available_width = self.workspace_body_width(window);
        super::device_panel::retire_removed_send_controls(self, cx);
        let pending_send = super::device_panel::pending_send(self, cx);
        let pending_return = self
            .track_name
            .pending_return(generation)
            .or_else(|| self.track_gain_controls.pending_return_gain(generation, cx))
            .or_else(|| pending_send.map(|(_, id)| id));
        let return_page = self.console_returns.page(
            &state.project,
            generation,
            available_width - TRACK_COLUMN_WIDTH - SCENE_HEADER_WIDTH - 12.0,
            pending_return,
        );
        let return_width = return_page.width();
        let visible_columns = ((available_width - return_width - SCENE_HEADER_WIDTH - 12.0)
            / TRACK_COLUMN_WIDTH)
            .floor()
            .max(1.0) as usize;
        self.session_visible_columns = visible_columns;
        let indices = &self.arrangement_focus.projection.console_indices;
        if let Some(index) = pending_send.and_then(|(track, _)| {
            indices.iter().position(|&index| {
                tracks
                    .get(index)
                    .is_some_and(|candidate| candidate.id() == track)
            })
        }) {
            let range = super::console_track_page::visible_track_range(
                indices.len(),
                self.session_track_start,
                visible_columns,
            );
            if index < range.start || index >= range.end {
                self.session_track_start = index.saturating_sub(visible_columns - 1);
            }
        }
        let page = self.pending_console_reorder_page().unwrap_or_else(|| {
            Arc::new(TrackPage::capture(
                &state.project,
                indices,
                self.session_track_start,
                visible_columns,
            ))
        });
        let column_start = page.range.start;
        let next_track = page.next;
        let track_count = indices.len();
        let frame = ColumnFrame::capture(self, window, revision);
        let row_context = SessionRowContext {
            frame,
            revision,
            generation,
            page: page.clone(),
            route_snapshot,
            routes_ready,
            transport_playing,
            stroke: 1.0 / window.scale_factor().max(1.0),
            return_width,
        };
        let header = div()
            .relative()
            .w_full()
            .h(px(TRACK_HEADER_HEIGHT))
            .flex_shrink_0()
            .flex()
            .border_b_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .children(
                page.columns
                    .iter()
                    .filter_map(|column| {
                        tracks
                            .get(column.index)
                            .filter(|track| track.id() == column.id)
                    })
                    .map(|track| {
                        let track_id = track.id();
                        let selected = self.track_selection.contains(track_id);
                        let hierarchy_indent = self.track_hierarchy_indent.pixels(track_id);
                        let ancestors = self.track_hierarchy_indent.ancestors(track_id);
                        let band_height = ancestors.len() as f32 * GROUP_BAND_HEIGHT;
                        let group_bands = ancestors
                            .iter()
                            .enumerate()
                            .map(|(depth, &(ancestor, color))| {
                                div()
                                    .absolute()
                                    .left_0()
                                    .right_0()
                                    .top(px(depth as f32 * GROUP_BAND_HEIGHT))
                                    .h(px(GROUP_BAND_HEIGHT))
                                    .debug_selector(move || {
                                        format!(
                                            "session-group-band-{}-{}",
                                            track_id.value(),
                                            ancestor.value()
                                        )
                                    })
                                    .bg(rgb(theme::element_colors(color).accent))
                            })
                            .collect::<Vec<_>>();
                        let header_focus = self.track_header_focus(track_id, cx);
                        let click_focus = header_focus.clone();
                        let header = div()
                            .id(("session-track", track.id().value()))
                            .track_focus(&header_focus)
                            .key_context("hex_track_header")
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(
                                    move |view, event: &gpui::MouseDownEvent, window, cx| {
                                        if event.click_count == 1 {
                                            view.begin_track_reorder(
                                                track_id,
                                                event.position,
                                                window,
                                                cx,
                                            );
                                        }
                                    },
                                ),
                            )
                            .on_click(cx.listener(
                                move |view, event: &gpui::ClickEvent, window, cx| {
                                    view.select_track_with_modifiers(
                                        track_id,
                                        event.modifiers(),
                                        window,
                                    );
                                    view.selected_cell = None;
                                    view.selected_scene = None;
                                    click_focus.focus(window);
                                    cx.notify();
                                },
                            ))
                            .debug_selector(move || format!("session-track-{}", track_id.value()))
                            .relative()
                            .children(selected.then(|| {
                                div()
                                    .absolute()
                                    .inset_0()
                                    .border_1()
                                    .border_color(rgb(theme::color(theme::PaletteRole::Primary)))
                                    .debug_selector(move || {
                                        format!("selected-console-track-{}", track_id.value())
                                    })
                            }))
                            .children(group_bands)
                            .child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .right_0()
                                    .top(px(band_height))
                                    .h(px(4.0))
                                    .debug_selector(move || {
                                        format!("session-track-color-strip-{}", track_id.value())
                                    })
                                    .bg(rgb(theme::element_colors(track.color()).accent)),
                            )
                            .on_mouse_down(
                                gpui::MouseButton::Right,
                                cx.listener(
                                    move |view, event: &gpui::MouseDownEvent, window, cx| {
                                        cx.stop_propagation();
                                        view.open_track_color_context_menu(
                                            event.position,
                                            track_id,
                                            window,
                                            cx,
                                        );
                                    },
                                ),
                            )
                            .w(px(TRACK_COLUMN_WIDTH))
                            .pt(px(if band_height > 0.0 {
                                band_height + 4.0
                            } else {
                                0.0
                            }))
                            .flex_shrink_0()
                            .px_2()
                            .flex()
                            .items_center()
                            .border_l_1()
                            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
                            .bg(rgb(theme::element_colors(track.color()).accent))
                            .text_size(px(11.0))
                            .line_height(px(14.0))
                            .text_color(rgb(theme::color(theme::PaletteRole::ClipText)))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .truncate()
                                    .flex()
                                    .items_center()
                                    .debug_selector(move || {
                                        format!("session-track-label-{}", track_id.value())
                                    })
                                    .children((hierarchy_indent > 0.0).then(|| {
                                        div()
                                            .w(px(hierarchy_indent))
                                            .flex_shrink_0()
                                            .debug_selector(move || {
                                                format!(
                                                    "track-hierarchy-indent-{}",
                                                    track_id.value()
                                                )
                                            })
                                    }))
                                    .children(track.is_group().then(|| {
                                        let folded =
                                            self.arrangement_focus.projection.folded(track_id);
                                        div()
                                            .id(("console-group-fold", track_id.value()))
                                            .debug_selector(move || {
                                                format!("console-group-fold-{}", track_id.value())
                                            })
                                            .w(px(14.0))
                                            .flex_shrink_0()
                                            .cursor_pointer()
                                            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                                cx.stop_propagation()
                                            })
                                            .on_click(cx.listener(move |view, _, _, cx| {
                                                cx.stop_propagation();
                                                if view.mode == EditorMode::Session
                                                    && view.track_view_change_available(cx)
                                                    && view
                                                        .state
                                                        .as_ref()
                                                        .and_then(|state| {
                                                            state.project.track(track_id)
                                                        })
                                                        .is_some_and(hex_project::Track::is_group)
                                                {
                                                    view.arrangement_focus
                                                        .projection
                                                        .toggle_fold(track_id);
                                                    view.reconcile_arrangement_focus();
                                                    cx.notify();
                                                }
                                            }))
                                            .tooltip(move |_, cx| {
                                                cx.new(|_| super::transport::MiniTooltip {
                                                    kind: crate::tooltips::Kind::Identification,
                                                    description: if folded {
                                                        "expand group"
                                                    } else {
                                                        "fold group"
                                                    }
                                                    .into(),
                                                })
                                                .into()
                                            })
                                            .child(theme::text(if folded { "▸" } else { "▾" }))
                                    }))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .child(theme::text(track.name().to_lowercase())),
                                    )
                                    .children(self.track_performance_indicator(
                                        hex_engine::OutputCaptureId::Track(track_id),
                                    ))
                                    .children(self.track_latency_indicator(
                                        super::track_latency::Owner::Track(track_id),
                                    ))
                                    .children(self.track_tags_header(track, cx)),
                            )
                            .children(self.route_marker_for_track(
                                track.id().value(),
                                route_snapshot,
                                routes_ready,
                                transport_playing,
                            ));
                        let header = self.device_drop_header(track_id, header, cx);
                        header.child(self.console_column_edge(
                            Some(track_id),
                            SessionSceneTarget::Insert { before: None },
                            frame,
                            true,
                            format!("console-column-header-before-{}", track_id.value()),
                            cx,
                        ))
                    }),
            )
            .child(
                self.console_column_surface(
                    div()
                        .id("console-column-header-tail")
                        .debug_selector(|| "console-column-header-tail".into())
                        .flex_1()
                        .min_w(px(12.0))
                        .h_full(),
                    next_track,
                    SessionSceneTarget::Insert { before: None },
                    frame,
                    true,
                    cx,
                ),
            );
        let header = super::console_columns::pinned_row(
            header,
            return_width,
            self.console_main_header(SCENE_HEADER_WIDTH, &return_page, frame, cx),
            TRACK_HEADER_HEIGHT,
        )
        .child(self.console_reorder_layer(
            page.clone(),
            frame,
            TRACK_COLUMN_WIDTH,
            SCENE_HEADER_WIDTH + return_width,
            cx,
        ));

        let scene_rows = uniform_list(
            "session-scene-rows",
            scenes.len().saturating_add(1),
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                let mut rows = Vec::with_capacity(range.end.saturating_sub(range.start));
                for index in range {
                    rows.push(this.session_scene_row(index, row_context.clone(), cx));
                }
                rows
            }),
        )
        .flex_1()
        .min_h_0()
        .track_scroll(self.session_scene_scroll.clone());

        let route_feedback = if routes_ready {
            None
        } else if route_snapshot.is_some() || self.audio.is_none() {
            Some("session routes unavailable")
        } else {
            Some("audio updating")
        };
        let body = div()
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .flex_shrink_0()
                    .flex()
                    .overflow_hidden()
                    .child(header),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .flex()
                    .child(scene_rows),
            )
            .child(self.console_mixer_row(
                &page,
                TRACK_COLUMN_WIDTH,
                SCENE_HEADER_WIDTH,
                &return_page,
                cx,
            ))
            .children(self.console_return_bank(&return_page, frame, SCENE_HEADER_WIDTH, cx));
        div()
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(self.session_toolbar(
                cx,
                visible_columns,
                column_start,
                track_count,
                route_feedback,
            ))
            .child(body)
            .child(self.track_reorder_events(cx))
    }

    fn session_toolbar(
        &self,
        cx: &mut Context<Self>,
        page_size: usize,
        page_start: usize,
        track_count: usize,
        route_feedback: Option<&'static str>,
    ) -> Div {
        let revision = self.state.as_ref().map(|state| state.revision);
        let generation = self.project_generation;
        let selected_scene = self.selected_scene;
        let selected_track = self
            .selected_cell
            .map(|cell| cell.0)
            .or(self.selected_track);
        let mut row = div()
            .h(px(26.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_1()
            .px_1()
            .border_b_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .bg(rgb(theme::color(theme::PaletteRole::Surface)));
        row = row
            .child(
                self.session_icon(
                    "‹",
                    "previous tracks",
                    "session-page-previous",
                    cx.listener(move |view, _, _, cx| {
                        if !view.track_reorder_active() && view.track_view_change_available(cx) {
                            view.session_track_start = page_start.saturating_sub(page_size.max(1));
                        }
                        cx.notify();
                    }),
                )
                .opacity(if page_start == 0 { 0.45 } else { 1.0 }),
            )
            .child(self.session_icon(
                "+",
                "add scene",
                "session-add-scene",
                cx.listener(move |view, _, _, cx| {
                    if revision
                        .is_some_and(|revision| view.session_callback_current(revision, generation))
                    {
                        view.enqueue(Request::Edit(Command::AddScene {
                            name: format!(
                                "scene {}",
                                view.state
                                    .as_ref()
                                    .map_or(1, |state| state.project.scenes().len() + 1)
                            ),
                        }));
                    }
                    cx.notify();
                }),
            ))
            .child(self.session_icon(
                "▶",
                "launch selected scene",
                "session-launch-selected-scene",
                cx.listener(move |view, _, _, cx| {
                    if let (Some(scene), Some(revision)) = (selected_scene, revision)
                        && view.session_callback_current(revision, generation)
                    {
                        view.launch_scene(scene, revision, view.session_quantization);
                    }
                    cx.notify();
                }),
            ))
            .child(self.session_icon(
                "■",
                "stop selected track",
                "session-stop-track",
                cx.listener(move |view, _, _, cx| {
                    if let (Some(track), Some(revision)) = (selected_track, revision)
                        && view.session_callback_current(revision, generation)
                    {
                        view.queue_session_command(
                            with_quantization(
                                SessionPlaybackCommand::StopTrack {
                                    track,
                                    quantization: LaunchQuantization::Immediate,
                                },
                                view.session_quantization,
                            ),
                            revision,
                        );
                    }
                    cx.notify();
                }),
            ))
            .child(self.session_icon(
                "■■",
                "stop all session clips",
                "session-stop-all",
                cx.listener(move |view, _, _, cx| {
                    if let Some(revision) = revision
                        .filter(|revision| view.session_callback_current(*revision, generation))
                    {
                        view.queue_session_command(
                            with_quantization(
                                SessionPlaybackCommand::StopAllClips {
                                    quantization: LaunchQuantization::Immediate,
                                },
                                view.session_quantization,
                            ),
                            revision,
                        );
                    }
                    cx.notify();
                }),
            ))
            .child(self.session_icon(
                "↩",
                "back to arrange view",
                "session-back-arrangement",
                cx.listener(move |view, _, _, cx| {
                    if let Some(revision) = revision
                        .filter(|revision| view.session_callback_current(*revision, generation))
                    {
                        view.queue_session_command(
                            with_quantization(
                                SessionPlaybackCommand::BackToArrangement {
                                    quantization: LaunchQuantization::Immediate,
                                },
                                view.session_quantization,
                            ),
                            revision,
                        );
                    }
                    view.set_mode(EditorMode::Arrangement);
                    cx.notify();
                }),
            ));
        if self.selected_cell.is_none()
            && let (Some(scene), Some(revision)) = (selected_scene, revision)
        {
            let can_move_up = self.state.as_ref().is_some_and(|state| {
                super::session_navigation::resolve_scene_move(
                    &state.project,
                    scene,
                    super::session_navigation::Direction::Up,
                )
                .is_some()
            });
            let can_move_down = self.state.as_ref().is_some_and(|state| {
                super::session_navigation::resolve_scene_move(
                    &state.project,
                    scene,
                    super::session_navigation::Direction::Down,
                )
                .is_some()
            });
            row = row
                .child(self.session_icon(
                    "⧉",
                    "duplicate scene · cmd-d",
                    "session-duplicate-scene",
                    cx.listener(move |view, _, _, cx| {
                        view.duplicate_scene_if_current(scene, revision, generation);
                        cx.notify();
                    }),
                ))
                .child(
                    self.session_icon(
                        "↑",
                        "move scene up · cmd-alt-up",
                        "session-move-scene-up",
                        cx.listener(move |view, _, _, cx| {
                            view.move_scene_if_current(
                                scene,
                                super::session_navigation::Direction::Up,
                                revision,
                                generation,
                            );
                            cx.notify();
                        }),
                    )
                    .opacity(if can_move_up { 1.0 } else { 0.45 }),
                )
                .child(
                    self.session_icon(
                        "↓",
                        "move scene down · cmd-alt-down",
                        "session-move-scene-down",
                        cx.listener(move |view, _, _, cx| {
                            view.move_scene_if_current(
                                scene,
                                super::session_navigation::Direction::Down,
                                revision,
                                generation,
                            );
                            cx.notify();
                        }),
                    )
                    .opacity(if can_move_down { 1.0 } else { 0.45 }),
                );
        }
        row = row.child(self.session_icon(
            "×",
            "cancel queued session launches",
            "session-cancel-pending",
            cx.listener(move |view, _, _, cx| {
                if let Some(revision) = revision
                    && view.session_callback_current(revision, generation)
                {
                    view.queue_session_command(
                        SessionPlaybackCommand::CancelPending { track: None },
                        revision,
                    );
                }
                cx.notify();
            }),
        ));
        row = row.child(
            self.session_icon(
                "›",
                "next tracks",
                "session-page-next",
                cx.listener(move |view, _, _, cx| {
                    if !view.track_reorder_active() && view.track_view_change_available(cx) {
                        view.session_track_start = page_start
                            .saturating_add(page_size.max(1))
                            .min(track_count.saturating_sub(page_size));
                    }
                    cx.notify();
                }),
            )
            .opacity(if page_start.saturating_add(page_size) >= track_count {
                0.45
            } else {
                1.0
            }),
        );
        if track_count > 0 {
            let first = page_start.saturating_add(1).min(track_count);
            let last = page_start.saturating_add(page_size).min(track_count);
            row = row.child(
                div()
                    .px_1()
                    .text_xs()
                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                    .child(theme::text(format!(
                        "tracks {first}–{last} / {track_count}"
                    ))),
            );
        }

        if let Some((track, scene)) = self.selected_cell {
            let is_empty_destination = self.state.as_ref().is_some_and(|state| {
                state.project.track(track).is_some()
                    && state.project.session_cell(track, scene).is_none()
            });
            if is_empty_destination {
                row = row.child(self.session_icon(
                    "↓",
                    "import audio source to empty cell · one-shot",
                    "session-import-audio-source",
                    cx.listener(move |view, _, _, cx| {
                        if revision.is_some_and(|revision| {
                            view.session_callback_current(revision, generation)
                        }) {
                            view.import_audio_to_session_dialog(track, scene, cx);
                        }
                        cx.notify();
                    }),
                ));
                row = row.child(self.session_icon(
                    "♪",
                    "create midi cell",
                    "session-create-midi",
                    cx.listener(move |view, _, _, cx| {
                        if revision.is_some_and(|revision| {
                            view.session_callback_current(revision, generation)
                        }) {
                            view.create_session_midi(track, scene);
                        }
                        cx.notify();
                    }),
                ));
            }
            row = row
                .child(self.session_icon(
                    "⇥",
                    "copy audio source from arrange view · one-shot source only",
                    "session-copy-audio-source",
                    cx.listener(move |view, _, _, cx| {
                        if revision.is_some_and(|revision| {
                            view.session_callback_current(revision, generation)
                        }) {
                            view.copy_arrangement_audio_to_session(track, scene);
                        }
                        cx.notify();
                    }),
                ))
                .child(self.session_icon(
                    "♪",
                    "copy selected arrange view midi clip",
                    "session-copy-midi",
                    cx.listener(move |view, _, _, cx| {
                        if revision.is_some_and(|revision| {
                            view.session_callback_current(revision, generation)
                        }) {
                            view.copy_arrangement_midi_to_session(track, scene);
                        }
                        cx.notify();
                    }),
                ))
                .child(self.session_icon(
                    "■",
                    "set stop cell",
                    "session-set-stop",
                    cx.listener(move |view, _, _, cx| {
                        if revision.is_some_and(|revision| {
                            view.session_callback_current(revision, generation)
                        }) {
                            view.set_session_stop_cell(track, scene);
                        }
                        cx.notify();
                    }),
                ));
        }
        if let Some(message) = route_feedback {
            row = row.child(
                div()
                    .px_1()
                    .text_xs()
                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                    .child(theme::text(message)),
            );
        }
        row
    }

    fn session_scene_row(
        &mut self,
        index: usize,
        context: SessionRowContext,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let SessionRowContext {
            frame,
            revision,
            generation,
            page,
            route_snapshot,
            routes_ready,
            transport_playing,
            stroke,
            return_width,
        } = context;
        let Some(state) = self.state.as_ref() else {
            return div().id(("session-scene-row-empty", index));
        };
        let Some(scene) = state.project.scenes().get(index) else {
            return self.console_append_row(&page, frame, return_width, cx);
        };
        let scene_id = scene.id();
        let next_scene = state
            .project
            .scenes()
            .get(index + 1)
            .map(|scene| scene.id());
        let selected = self.selected_scene == Some(scene_id);
        let mut row = div()
            .w_full()
            .h(px(SCENE_ROW_HEIGHT))
            .flex_shrink_0()
            .flex()
            .border_b_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)));
        for track in page.columns.iter().filter_map(|column| {
            state
                .project
                .tracks()
                .get(column.index)
                .filter(|track| track.id() == column.id)
        }) {
            let track_id = track.id();
            let content = state.project.session_cell(track_id, scene_id);
            let route = self.route_entry_for_track(track_id.value(), route_snapshot, routes_ready);
            let selected_cell = self.selected_cell == Some((track_id, scene_id));
            let mut cell = div()
                .id(ElementId::Name(
                    format!("session-cell-{}-{}", track_id.value(), scene_id.value()).into(),
                ))
                .debug_selector(move || {
                    format!("console-cell-{}-{}", track_id.value(), scene_id.value())
                })
                .relative()
                .w(px(TRACK_COLUMN_WIDTH))
                .h(px(SCENE_ROW_HEIGHT))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_between()
                .px_2()
                .border_l_1()
                .border_color(rgb(theme::color(theme::PaletteRole::Border)))
                .bg(rgb(if selected_cell {
                    theme::color(theme::PaletteRole::Raised)
                } else {
                    theme::color(theme::PaletteRole::Background)
                }))
                .text_xs()
                .line_height(px(super::clip_name::CLIP_NAME_HEIGHT));
            match content {
                Some(SessionCellContent::Midi(clip)) => {
                    let clip_id = clip.id();
                    let title = clip.name().to_lowercase();
                    let route_markers =
                        cell_route_markers(route, clip_id.value(), transport_playing);
                    cell = cell
                        .gap(px(4.0))
                        .px(px(6.0))
                        .child(session_clip_backdrop(
                            clip_id,
                            theme::clip_activity_fill(
                                track.color(),
                                clip.flavor(),
                                selected_cell,
                                clip.active(),
                            ),
                            stroke,
                        ))
                        .text_color(rgb(if !clip.active() {
                            theme::readable_text(
                                theme::clip_activity_fill(
                                    track.color(),
                                    clip.flavor(),
                                    selected_cell,
                                    false,
                                ),
                                0x000000,
                            )
                        } else {
                            match clip.flavor() {
                                hex_project::ClipFlavor::Muted => {
                                    theme::element_colors(track.color()).muted_text
                                }
                                hex_project::ClipFlavor::Accent => {
                                    theme::element_colors(track.color()).accent_text
                                }
                            }
                        }))
                        .on_mouse_down(
                            gpui::MouseButton::Right,
                            cx.listener(move |view, event: &gpui::MouseDownEvent, window, cx| {
                                cx.stop_propagation();
                                view.open_clip_context_menu(
                                    event.position,
                                    track_id,
                                    clip_id,
                                    window,
                                    cx,
                                );
                            }),
                        )
                        .when(self.clip_has_alias(clip_id), |cell| cell.pr(px(15.0)))
                        .children(
                            self.clip_alias_indicator(
                                clip_id,
                                theme::clip_activity_fill(
                                    track.color(),
                                    clip.flavor(),
                                    selected_cell,
                                    clip.active(),
                                ),
                            )
                            .map(|badge| badge.right(px(8.0))),
                        )
                        .child(
                            self.session_icon(
                                "▶",
                                "launch midi clip",
                                ElementId::Name(
                                    format!(
                                        "session-launch-cell-{}-{}",
                                        track_id.value(),
                                        scene_id.value()
                                    )
                                    .into(),
                                ),
                                cx.listener(move |view, _, window, cx| {
                                    if view.session_callback_current(revision, generation)
                                        && view.cell_still_contains(
                                            track_id,
                                            scene_id,
                                            CellExpectation::Midi(clip_id),
                                        )
                                        && view.focus_clip_details(window, cx)
                                    {
                                        view.focus.focus(window);
                                        view.advance_selection_intent();
                                        view.select_session_cell(track_id, scene_id);
                                        view.selected_track = Some(track_id);
                                        view.selected_clip = Some(clip_id);
                                        view.launch_clip(
                                            track_id,
                                            clip_id,
                                            revision,
                                            view.session_quantization,
                                        );
                                    }
                                    cx.notify();
                                }),
                            )
                            .size(px(19.0))
                            .debug_selector(move || {
                                format!("launch-midi-{}-{}", track_id.value(), scene_id.value())
                            }),
                        )
                        .child(self.clip_name_label(clip_id, &title))
                        .children(route_markers.into_iter().flatten().map(|marker| {
                            self.route_marker(
                                marker.glyph,
                                marker.label,
                                track_id.value(),
                                scene_id.value(),
                            )
                        }))
                        .on_click(cx.listener(move |view, _, window, cx| {
                            if view.session_callback_current(revision, generation)
                                && view.cell_still_contains(
                                    track_id,
                                    scene_id,
                                    CellExpectation::Midi(clip_id),
                                )
                                && view.focus_clip_details(window, cx)
                            {
                                view.focus.focus(window);
                                view.advance_selection_intent();
                                view.select_session_midi(track_id, scene_id, clip_id);
                            }
                            cx.notify();
                        }));
                }
                Some(SessionCellContent::Audio(clip)) => {
                    let clip_id = clip.id();
                    let route_markers =
                        cell_route_markers(route, clip_id.value(), transport_playing);
                    cell = cell
                        .gap(px(4.0))
                        .px(px(6.0))
                        .child(session_clip_backdrop(
                            clip_id,
                            theme::clip_activity_fill(
                                track.color(),
                                clip.flavor(),
                                selected_cell,
                                clip.active(),
                            ),
                            stroke,
                        ))
                        .text_color(rgb(if !clip.active() {
                            theme::readable_text(
                                theme::clip_activity_fill(
                                    track.color(),
                                    clip.flavor(),
                                    selected_cell,
                                    false,
                                ),
                                0x000000,
                            )
                        } else {
                            match clip.flavor() {
                                hex_project::ClipFlavor::Muted => {
                                    theme::element_colors(track.color()).muted_text
                                }
                                hex_project::ClipFlavor::Accent => {
                                    theme::element_colors(track.color()).accent_text
                                }
                            }
                        }))
                        .on_mouse_down(
                            gpui::MouseButton::Right,
                            cx.listener(move |view, event: &gpui::MouseDownEvent, window, cx| {
                                cx.stop_propagation();
                                view.open_clip_context_menu(
                                    event.position,
                                    track_id,
                                    clip_id,
                                    window,
                                    cx,
                                );
                            }),
                        )
                        .when(self.clip_has_alias(clip_id), |cell| cell.pr(px(15.0)))
                        .children(
                            self.clip_alias_indicator(
                                clip_id,
                                theme::clip_activity_fill(
                                    track.color(),
                                    clip.flavor(),
                                    selected_cell,
                                    clip.active(),
                                ),
                            )
                            .map(|badge| badge.right(px(8.0))),
                        )
                        .child(
                            self.session_icon(
                                "▶",
                                "launch audio one-shot",
                                ElementId::Name(
                                    format!(
                                        "session-launch-audio-cell-{}-{}",
                                        track_id.value(),
                                        scene_id.value()
                                    )
                                    .into(),
                                ),
                                cx.listener(move |view, _, window, cx| {
                                    if view.session_callback_current(revision, generation)
                                        && view.cell_still_contains(
                                            track_id,
                                            scene_id,
                                            CellExpectation::Audio(clip_id),
                                        )
                                        && view.focus_clip_details(window, cx)
                                    {
                                        view.focus.focus(window);
                                        view.advance_selection_intent();
                                        view.select_session_cell(track_id, scene_id);
                                        view.launch_clip(
                                            track_id,
                                            clip_id,
                                            revision,
                                            view.session_quantization,
                                        );
                                    }
                                    cx.notify();
                                }),
                            )
                            .size(px(19.0))
                            .debug_selector(move || {
                                format!("launch-audio-{}-{}", track_id.value(), scene_id.value())
                            }),
                        )
                        .child(self.clip_name_label(clip_id, clip.name()))
                        .children(route_markers.into_iter().flatten().map(|marker| {
                            self.route_marker(
                                marker.glyph,
                                marker.label,
                                track_id.value(),
                                scene_id.value(),
                            )
                        }))
                        .on_click(cx.listener(move |view, _, window, cx| {
                            if view.session_callback_current(revision, generation)
                                && view.cell_still_contains(
                                    track_id,
                                    scene_id,
                                    CellExpectation::Audio(clip_id),
                                )
                                && view.focus_clip_details(window, cx)
                            {
                                view.focus.focus(window);
                                view.advance_selection_intent();
                                view.select_session_cell(track_id, scene_id);
                            }
                            cx.notify();
                        }));
                }
                Some(SessionCellContent::Stop) => {
                    cell = cell
                        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                        .child(theme::text("stop"))
                        .child(theme::text("■"))
                        .on_click(cx.listener(move |view, _, window, cx| {
                            if view.session_callback_current(revision, generation)
                                && view.cell_still_contains(
                                    track_id,
                                    scene_id,
                                    CellExpectation::Stop,
                                )
                            {
                                view.focus_track_details(window);
                                view.focus.focus(window);
                                view.advance_selection_intent();
                                view.select_session_cell(track_id, scene_id);
                                view.queue_session_command(
                                    with_quantization(
                                        SessionPlaybackCommand::StopTrack {
                                            track: track_id,
                                            quantization: LaunchQuantization::Immediate,
                                        },
                                        view.session_quantization,
                                    ),
                                    revision,
                                );
                            }
                            cx.notify();
                        }));
                }
                None => {
                    cell = cell
                        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                        .child(theme::text("·"))
                        .on_click(cx.listener(move |view, _, window, cx| {
                            if view.session_callback_current(revision, generation)
                                && view.cell_still_contains(
                                    track_id,
                                    scene_id,
                                    CellExpectation::Empty,
                                )
                            {
                                view.focus_track_details(window);
                                view.focus.focus(window);
                                view.advance_selection_intent();
                                view.select_session_cell(track_id, scene_id);
                            }
                            cx.notify();
                        }));
                }
            }
            // Separate narrow edge hit surfaces select insertion, never replace
            // the cell body. Neighboring edges carry the same stable anchor.
            let top = self.console_insertion_strip(track_id, Some(scene_id), true, scene_id, cx);
            let bottom = self.console_insertion_strip(track_id, next_scene, false, scene_id, cx);
            let cell = cell.children([top, bottom]);
            let cell = self.console_drop_surface(
                cell,
                hex_session::import_batch::ConsoleDestination::Cell {
                    track: track_id,
                    scene: scene_id,
                },
                cx,
            );
            let cell = cell.child(self.console_column_edge(
                Some(track_id),
                SessionSceneTarget::Existing(scene_id),
                frame,
                false,
                format!(
                    "console-column-before-{}-{}",
                    track_id.value(),
                    scene_id.value()
                ),
                cx,
            ));
            row = row.child(cell);
        }
        let scene_column = div()
            .id(("session-scene", scene_id.value()))
            .on_mouse_down(
                gpui::MouseButton::Right,
                cx.listener(move |view, event: &gpui::MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    view.open_console_scene_context_menu(
                        event.position,
                        Some(scene_id),
                        window,
                        cx,
                    );
                }),
            )
            .w(px(SCENE_HEADER_WIDTH))
            .flex_shrink_0()
            .px_2()
            .flex()
            .items_center()
            .gap(px(4.0))
            .bg(rgb(if selected {
                theme::color(theme::PaletteRole::Raised)
            } else {
                theme::color(theme::PaletteRole::Surface)
            }))
            .text_xs()
            .text_color(rgb(theme::color(theme::PaletteRole::Text)))
            .child(
                self.session_icon(
                    "▶",
                    "launch scene",
                    ("session-launch-scene", scene_id.value()),
                    cx.listener(move |view, _, window, cx| {
                        if view.session_callback_current(revision, generation)
                            && view.focus_empty_details(window, cx)
                        {
                            view.focus.focus(window);
                            view.advance_selection_intent();
                            view.select_session_scene(scene_id);
                            view.launch_scene(scene_id, revision, view.session_quantization);
                        }
                        cx.notify();
                    }),
                )
                .debug_selector(move || format!("console-launch-scene-{}", scene_id.value())),
            )
            .child(
                div()
                    .id(("session-scene-select", scene_id.value()))
                    .debug_selector(move || format!("console-scene-menu-{}", scene_id.value()))
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .child(theme::text(scene.name().to_lowercase()))
                    .on_click(cx.listener(move |view, _, window, cx| {
                        if view.session_callback_current(revision, generation)
                            && view.focus_empty_details(window, cx)
                        {
                            view.focus.focus(window);
                            view.advance_selection_intent();
                            view.select_session_scene(scene_id);
                        }
                        cx.notify();
                    })),
            );
        #[cfg(feature = "ui-test-support")]
        let scene_column = scene_column.debug_selector(|| "session-scene-column".into());
        let tail = self.console_column_surface(
            div()
                .id(("console-column-tail", scene_id.value()))
                .debug_selector(move || format!("console-column-tail-{}", scene_id.value()))
                .flex_1()
                .min_w(px(12.0))
                .h_full(),
            page.next,
            SessionSceneTarget::Existing(scene_id),
            frame,
            false,
            cx,
        );
        super::console_columns::pinned_row(
            row.child(tail),
            return_width,
            scene_column,
            SCENE_ROW_HEIGHT,
        )
        .id(("session-scene-row", scene_id.value()))
    }

    fn console_insertion_strip(
        &self,
        track: TrackId,
        before: Option<SceneId>,
        top: bool,
        row: SceneId,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let strip = div()
            .id(ElementId::Name(
                format!("console-insert-{}-{}-{}", track.value(), row.value(), top).into(),
            ))
            .debug_selector(move || {
                if top {
                    format!("console-insert-before-{}-{}", track.value(), row.value())
                } else {
                    format!("console-insert-after-{}-{}", track.value(), row.value())
                }
            })
            .absolute()
            .left_0()
            .right_0()
            .h(px(4.0))
            .when(top, |strip| strip.top_0())
            .when(!top, |strip| strip.bottom_0());
        self.console_drop_surface(strip, ConsoleDestination::InsertScene { track, before }, cx)
    }

    fn console_append_row(
        &self,
        page: &TrackPage,
        frame: ColumnFrame,
        return_width: f32,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let mut row = div()
            .w_full()
            .h(px(SCENE_ROW_HEIGHT))
            .flex()
            .flex_shrink_0()
            .border_b_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)));
        if let Some(state) = self.state.as_ref() {
            for track in page.columns.iter().filter_map(|column| {
                state
                    .project
                    .tracks()
                    .get(column.index)
                    .filter(|track| track.id() == column.id)
            }) {
                let id = track.id();
                let cell = div()
                    .id(("console-append-track", id.value()))
                    .relative()
                    .debug_selector(move || format!("console-append-track-{}", id.value()))
                    .w(px(TRACK_COLUMN_WIDTH))
                    .h(px(SCENE_ROW_HEIGHT))
                    .flex_shrink_0()
                    .border_l_1()
                    .border_color(rgb(theme::color(theme::PaletteRole::Border)))
                    .flex()
                    .items_center()
                    .px_2()
                    .text_xs()
                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                    .child(theme::text("+"));
                let cell = self.console_drop_surface(
                    cell,
                    ConsoleDestination::InsertScene {
                        track: id,
                        before: None,
                    },
                    cx,
                );
                let cell = cell.child(self.console_column_edge(
                    Some(id),
                    SessionSceneTarget::Insert { before: None },
                    frame,
                    false,
                    format!("console-column-append-before-{}", id.value()),
                    cx,
                ));
                row = row.child(cell);
            }
        }
        let tail = self.console_column_surface(
            div()
                .id("console-column-append-tail")
                .debug_selector(|| "console-column-append-tail".into())
                .flex_1()
                .min_w(px(12.0))
                .h_full(),
            page.next,
            SessionSceneTarget::Insert { before: None },
            frame,
            false,
            cx,
        );
        let main = div()
            .w(px(SCENE_HEADER_WIDTH))
            .flex_shrink_0()
            .px_2()
            .flex()
            .items_center()
            .text_xs()
            .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
            .child(theme::text("add scene"));
        super::console_columns::pinned_row(row.child(tail), return_width, main, SCENE_ROW_HEIGHT)
            .id("console-append-row")
            .debug_selector(|| "console-append-row".into())
            .on_mouse_down(
                gpui::MouseButton::Right,
                cx.listener(|view, event: &gpui::MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    view.open_console_scene_context_menu(event.position, None, window, cx);
                }),
            )
    }

    fn session_icon(
        &self,
        glyph: &'static str,
        label: &'static str,
        id: impl Into<ElementId>,
        listener: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
    ) -> gpui::Stateful<Div> {
        let icon = hex_widgets::action(theme::text(glyph), theme::action_style())
            .id(id)
            .size(px(23.0))
            .text_xs()
            .tooltip(move |_, cx| {
                cx.new(|_| super::transport::MiniTooltip {
                    kind: crate::tooltips::Kind::Identification,
                    description: label.into(),
                })
                .into()
            })
            .on_click(listener);
        #[cfg(feature = "ui-test-support")]
        let icon = icon.debug_selector(move || label.into());
        icon
    }

    fn route_entry_for_track(
        &self,
        track_id: u64,
        snapshot: Option<SessionRouteSnapshot>,
        routes_ready: bool,
    ) -> Option<SessionRouteEntry> {
        let snapshot = snapshot?;
        if !routes_ready {
            return None;
        }
        self.session_routes
            .entry_for_track(snapshot, routes_ready, track_id)
    }

    fn route_marker_for_track(
        &self,
        track_id: u64,
        snapshot: Option<SessionRouteSnapshot>,
        routes_ready: bool,
        transport_playing: bool,
    ) -> Option<gpui::Stateful<Div>> {
        let entry = self.route_entry_for_track(track_id, snapshot, routes_ready)?;
        let (glyph, label) = match entry.pending.map(|pending| pending.target) {
            Some(SessionRouteTarget::Arrangement) => ("↩", "arrangement return pending"),
            Some(SessionRouteTarget::Session { .. }) => ("…", "session launch pending"),
            Some(SessionRouteTarget::Silence) => ("■", "track stop pending"),
            None => match entry.owner {
                SessionRouteOwner::Session { .. } if transport_playing => {
                    ("▶", "session route active · transport running")
                }
                SessionRouteOwner::Session { .. } => {
                    ("●", "session route selected · transport stopped")
                }
                SessionRouteOwner::Silence => ("■", "track clips stopped"),
                SessionRouteOwner::Arrangement => return None,
            },
        };
        Some(self.route_marker(glyph, label, track_id, 0))
    }

    fn route_marker(
        &self,
        glyph: &'static str,
        label: &'static str,
        track: u64,
        scene: u64,
    ) -> gpui::Stateful<Div> {
        div()
            .id(ElementId::Name(
                format!("session-route-{track}-{scene}-{glyph}").into(),
            ))
            .size(px(16.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .text_xs()
            .text_color(rgb(theme::color(theme::PaletteRole::SecondaryText)))
            .tooltip(move |_, cx| {
                cx.new(|_| super::transport::MiniTooltip {
                    kind: crate::tooltips::Kind::Identification,
                    description: label.into(),
                })
                .into()
            })
            .child(theme::text(glyph))
    }

    fn session_callback_current(&self, revision: u64, generation: u64) -> bool {
        !self.edit_pending()
            && self.project_generation == generation
            && self
                .state
                .as_ref()
                .is_some_and(|state| state.revision == revision)
    }

    fn cell_still_contains(
        &self,
        track: TrackId,
        scene: SceneId,
        expected: CellExpectation,
    ) -> bool {
        self.state.as_ref().is_some_and(|state| {
            match (expected, state.project.session_cell(track, scene)) {
                (CellExpectation::Midi(expected), Some(SessionCellContent::Midi(current))) => {
                    current.id() == expected
                }
                (CellExpectation::Audio(expected), Some(SessionCellContent::Audio(current))) => {
                    current.id() == expected
                }
                (CellExpectation::Stop, Some(SessionCellContent::Stop)) => true,
                (CellExpectation::Empty, None) => true,
                _ => false,
            }
        })
    }

    pub(super) fn select_session_midi(&mut self, track: TrackId, scene: SceneId, clip: ClipId) {
        self.cancel_duplicate_chain();
        if self.detail_clip_id() != Some(clip) {
            self.zoom_fit.clear_clip();
        }
        self.cancel_scene_duplicate_selection();
        if let Some(selected) = self.selected_clip
            && self.state.as_ref().is_some_and(|state| {
                state.project.midi_clip(selected).is_some()
                    || state
                        .project
                        .tracks()
                        .iter()
                        .any(|track| track.clips().iter().any(|clip| clip.id() == selected))
            })
        {
            self.session_copy_source = Some(selected);
        }
        self.selected_scene = Some(scene);
        self.selected_cell = Some((track, scene));
        self.selected_track = Some(track);
        self.selected_clip = Some(clip);
        self.mode = EditorMode::Session;
        self.midi_selection.clear();
        self.cancel_midi_note_gesture();
        self.cancel_midi_stroke();
        self.midi_group_gesture = None;
        self.midi_hit_cycle = None;
        self.piano_beat_start = 0;
        self.reveal_session_position(Some(track), Some(scene));
    }

    pub(super) fn select_session_scene(&mut self, scene: SceneId) {
        self.cancel_duplicate_chain();
        self.zoom_fit.clear_clip();
        self.cancel_scene_duplicate_selection();
        if let Some(selected) = self.selected_clip
            && self.state.as_ref().is_some_and(|state| {
                state.project.midi_clip(selected).is_some()
                    || state
                        .project
                        .tracks()
                        .iter()
                        .any(|track| track.clips().iter().any(|clip| clip.id() == selected))
            })
        {
            self.session_copy_source = Some(selected);
        }
        self.selected_scene = Some(scene);
        self.selected_cell = None;
        self.selected_track = None;
        self.selected_clip = None;
        self.mode = EditorMode::Session;
        self.midi_selection.clear();
        self.cancel_midi_note_gesture();
        self.cancel_midi_stroke();
        self.midi_group_gesture = None;
        self.midi_hit_cycle = None;
        self.reveal_session_position(None, Some(scene));
    }

    pub(super) fn select_session_cell(&mut self, track: TrackId, scene: SceneId) {
        self.cancel_duplicate_chain();
        if self.selected_cell != Some((track, scene)) {
            self.zoom_fit.clear_clip();
        }
        self.cancel_scene_duplicate_selection();
        if let Some(selected) = self.selected_clip
            && self.state.as_ref().is_some_and(|state| {
                state.project.midi_clip(selected).is_some()
                    || state
                        .project
                        .tracks()
                        .iter()
                        .any(|track| track.clips().iter().any(|clip| clip.id() == selected))
            })
        {
            self.session_copy_source = Some(selected);
        }
        self.selected_scene = Some(scene);
        self.selected_cell = Some((track, scene));
        self.selected_track = Some(track);
        self.selected_clip = None;
        self.mode = EditorMode::Session;
        self.midi_selection.clear();
        self.cancel_midi_note_gesture();
        self.cancel_midi_stroke();
        self.midi_group_gesture = None;
        self.midi_hit_cycle = None;
        self.reveal_session_position(Some(track), Some(scene));
    }

    pub(super) fn reveal_session_position(
        &mut self,
        track: Option<TrackId>,
        scene: Option<SceneId>,
    ) {
        let positions = self.state.as_ref().map(|state| {
            (
                track.and_then(|track| {
                    self.arrangement_focus
                        .projection
                        .console_indices
                        .iter()
                        .position(|index| {
                            state
                                .project
                                .tracks()
                                .get(*index)
                                .is_some_and(|item| item.id() == track)
                        })
                }),
                scene.and_then(|scene| {
                    state
                        .project
                        .scenes()
                        .iter()
                        .position(|item| item.id() == scene)
                }),
            )
        });
        let Some((track_index, scene_index)) = positions else {
            return;
        };
        if let Some(index) = track_index.filter(|_| !self.track_reorder_active()) {
            let page_size = self.session_visible_columns.max(1);
            if index < self.session_track_start
                || index >= self.session_track_start.saturating_add(page_size)
            {
                self.session_track_start = index / page_size * page_size;
            }
        }
        if let Some(index) = scene_index {
            self.session_scene_scroll
                .scroll_to_item(index, gpui::ScrollStrategy::Top);
        }
    }

    pub(super) fn set_mode(&mut self, mode: EditorMode) {
        if self.mode != mode {
            self.cancel_arrangement_pan();
            self.cancel_active_browser_audio_drag();
            if self.time_selection_gesture.is_some() || self.time_range_move_gesture.is_some() {
                self.cancel_time_selection_gesture();
            }
            self.cancel_scene_duplicate_selection();
        }
        if mode == EditorMode::Session
            && let Some(selected) = self.selected_clip
            && self
                .state
                .as_ref()
                .is_some_and(|state| state.project.midi_clip(selected).is_some())
        {
            self.session_copy_source = Some(selected);
        }
        self.mode = mode;
    }

    fn launch_clip(
        &mut self,
        _track: TrackId,
        clip: ClipId,
        revision: u64,
        quantization: LaunchQuantization,
    ) {
        self.queue_session_launch_batch(
            SessionPlaybackCommand::LaunchClip { clip, quantization },
            revision,
        );
    }

    fn launch_scene(&mut self, scene: SceneId, revision: u64, quantization: LaunchQuantization) {
        self.queue_session_launch_batch(
            SessionPlaybackCommand::LaunchScene {
                scene,
                quantization,
            },
            revision,
        );
    }

    fn create_session_midi(&mut self, track: TrackId, scene: SceneId) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        if self.edit_pending() || state.project.track(track).is_none() {
            return;
        }
        if !matches!(
            state.project.session_cell(track, scene),
            None | Some(SessionCellContent::Stop)
        ) {
            self.error = Some("session cell is occupied".into());
            return;
        }
        self.enqueue(Request::Edit(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "midi clip".into(),
            source_length: BeatTime::from_quarters(4).unwrap_or(BeatTime::ONE),
        }));
    }

    fn copy_arrangement_midi_to_session(&mut self, track: TrackId, scene: SceneId) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let Some(source) = self
            .session_copy_source
            .or(self.selected_clip)
            .filter(|clip| state.project.midi_clip(*clip).is_some())
        else {
            self.error = Some("select an arrangement midi clip to copy".into());
            return;
        };
        let source_track = state.project.tracks().iter().find_map(|track| {
            track
                .midi_clips()
                .iter()
                .any(|clip| clip.id() == source)
                .then_some(track.id())
        });
        if source_track != Some(track) {
            self.error = Some("copy into the session cell on the source track".into());
            return;
        }
        if self.edit_pending()
            || state.project.track(track).is_none()
            || !matches!(
                state.project.session_cell(track, scene),
                None | Some(SessionCellContent::Stop)
            )
        {
            self.error = Some("session destination cell is unavailable".into());
            return;
        }
        self.enqueue(Request::Edit(Command::CopyArrangementMidiToSession {
            source,
            scene,
        }));
    }

    fn copy_arrangement_audio_to_session(&mut self, track: TrackId, scene: SceneId) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let Some(source) = self
            .session_copy_source
            .or(self.selected_clip)
            .filter(|clip| {
                state
                    .project
                    .tracks()
                    .iter()
                    .any(|item| item.clips().iter().any(|source| source.id() == *clip))
            })
        else {
            self.error = Some("select an arrangement audio clip to copy".into());
            return;
        };
        let source_track = state.project.tracks().iter().find_map(|item| {
            item.clips()
                .iter()
                .any(|clip| clip.id() == source)
                .then_some(item.id())
        });
        if source_track != Some(track) {
            self.error = Some("copy audio into the session cell on the source track".into());
            return;
        }
        if self.edit_pending()
            || state.project.track(track).is_none()
            || state.project.session_cell(track, scene).is_some()
        {
            self.error = Some("session destination cell is unavailable".into());
            return;
        }
        self.enqueue(Request::Edit(Command::CopyArrangementAudioClipToSession {
            source,
            scene,
        }));
    }

    fn set_session_stop_cell(&mut self, track: TrackId, scene: SceneId) {
        if self.edit_pending() {
            return;
        }
        self.enqueue(Request::Edit(Command::SetSessionStopCell { track, scene }));
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::launch_action;
    use hex_engine::{LaunchQuantization, PlaybackAction, SessionPlaybackCommand};
    use hex_project::{Command, Created, Editor, Project};

    #[test]
    fn queued_scene_launch_retains_quantization_selected_before_launch() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 3).unwrap();
        let Created::Scene(scene) = editor
            .execute(Command::AddScene {
                name: "scene".into(),
            })
            .unwrap()
        else {
            panic!("scene")
        };

        let mut selected_quantization = LaunchQuantization::FourQuarter;
        let command = super::with_quantization(
            SessionPlaybackCommand::LaunchScene {
                scene,
                quantization: LaunchQuantization::Immediate,
            },
            selected_quantization,
        );
        let queued = launch_action(command, 19, 3);
        selected_quantization = LaunchQuantization::Immediate;
        assert_eq!(selected_quantization, LaunchQuantization::Immediate);
        assert!(matches!(
            queued.action,
            PlaybackAction::LaunchAndPlay(SessionPlaybackCommand::LaunchScene {
                quantization: LaunchQuantization::FourQuarter,
                ..
            })
        ));
    }
}
