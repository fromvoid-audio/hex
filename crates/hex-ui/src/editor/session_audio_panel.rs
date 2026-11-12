//! Compact source-window and cycle controls for the selected Console audio cell.
//! The waveform also shows the accepted Session playback position of that cell.
#[path = "session_audio_panel/playhead.rs"]
mod playhead;

use super::EditorView;
use crate::theme;
use gpui::{Context, Div, ElementId, Stateful, div, prelude::*, px, relative, rgb};
use hex_audio::prepared::SessionRouteOwner;
use hex_project::{AssetId, ClipId, Command, SessionAudioClip, SessionCellContent, TrackId};
use hex_session::CachedWaveform;
use playhead::playhead_fraction;
use std::ops::Range;

const WAVEFORM_COLUMNS: usize = 48;
pub(super) const DETAIL_HEIGHT: f32 = 38.0;

pub(super) fn view(view: &mut EditorView, cx: &mut Context<EditorView>) -> Option<Stateful<Div>> {
    if view.mode != super::EditorMode::Session {
        return None;
    }
    let edit_pending = view.edit_pending();
    let (track, scene) = view.selected_cell?;
    let state = view.state.as_ref()?;
    let SessionCellContent::Audio(clip) = state.project.session_cell(track, scene)? else {
        return None;
    };
    let id = clip.id();
    let asset_id = clip.asset();
    let start = clip.source_start();
    let end = clip.source_end();
    let asset_frames = state.project.asset(asset_id)?.playback_frames();
    let revision = state.revision;
    let project_generation = view.project_generation;
    let name = clip.name().to_lowercase();
    let gain = clip.gain();
    let loop_range = clip.loop_range();
    let playhead = view.session_audio_playhead(track, clip);
    let captured = SourceWindow {
        clip: id,
        asset: asset_id,
        track,
        scene,
        start,
        end,
        asset_frames,
        revision,
        project_generation,
    };
    let waveform = state
        .waveforms
        .get(&asset_id)
        .map(|waveform| waveform_bars(waveform, start, end));
    let mut row = div()
        .id("session-audio-source-panel")
        .w_full()
        .h(px(view.clip_detail_body_height))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(5.0))
        .px(px(6.0))
        .debug_selector(|| "session-audio-source-panel".into())
        .border_t_1()
        .border_color(rgb(theme::color(theme::PaletteRole::Border)))
        .bg(rgb(theme::color(theme::PaletteRole::Raised)))
        .text_xs();
    row = row.child(
        div()
            .w(px(100.0))
            .flex_shrink_0()
            .truncate()
            .text_color(rgb(theme::color(theme::PaletteRole::Text)))
            .child(theme::text(name)),
    );
    row = row
        .child(source_adjust_button(
            cx,
            "−",
            "move source start earlier",
            SourceWindow {
                clip: id,
                asset: asset_id,
                track,
                scene,
                start,
                end,
                asset_frames,
                revision,
                project_generation,
            },
            SourceEdge::StartEarlier,
            edit_pending,
        ))
        .child(source_adjust_button(
            cx,
            "+",
            "move source start later",
            SourceWindow {
                clip: id,
                asset: asset_id,
                track,
                scene,
                start,
                end,
                asset_frames,
                revision,
                project_generation,
            },
            SourceEdge::StartLater,
            edit_pending,
        ))
        .child(
            div()
                .w(px(72.0))
                .flex_shrink_0()
                .truncate()
                .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                .debug_selector(|| "session-audio-source-readout".into())
                .id(("session-audio-source-readout", id.value()))
                .tooltip(move |_, cx| {
                    let description = format!("source {start}–{end} / {asset_frames} frames");
                    cx.new(|_| super::transport::MiniTooltip {
                        kind: crate::tooltips::Kind::Identification,
                        description: description.into(),
                    })
                    .into()
                })
                .child(theme::text(format!("source {start}–{end}"))),
        )
        .child(source_adjust_button(
            cx,
            "−",
            "move source end earlier",
            SourceWindow {
                clip: id,
                asset: asset_id,
                track,
                scene,
                start,
                end,
                asset_frames,
                revision,
                project_generation,
            },
            SourceEdge::EndEarlier,
            edit_pending,
        ))
        .child(source_adjust_button(
            cx,
            "+",
            "move source end later",
            SourceWindow {
                clip: id,
                asset: asset_id,
                track,
                scene,
                start,
                end,
                asset_frames,
                revision,
                project_generation,
            },
            SourceEdge::EndLater,
            edit_pending,
        ));
    row = row
        .child(gain_adjust_button(cx, captured, gain, -1, edit_pending))
        .child(
            div()
                .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                .child(theme::text(format!(
                    "gain {:+.1} db",
                    f64::from(gain.millidb()) / 1_000.0
                ))),
        )
        .child(gain_adjust_button(cx, captured, gain, 1, edit_pending));
    row = row.child(session_audio_loop_button(
        cx,
        captured,
        loop_range,
        None,
        edit_pending,
    ));
    if let Some(range) = loop_range {
        for (edge, glyph) in [
            (SourceEdge::StartEarlier, "‹"),
            (SourceEdge::StartLater, "›"),
        ] {
            row = row.child(session_audio_loop_button(
                cx,
                captured,
                loop_range,
                Some((edge, glyph)),
                edit_pending,
            ));
        }
        row = row.child(
            div()
                .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                .debug_selector(|| "session-audio-loop-readout".into())
                .child(theme::text(format!("{}–{}", range.start(), range.end()))),
        );
        for (edge, glyph) in [(SourceEdge::EndEarlier, "‹"), (SourceEdge::EndLater, "›")] {
            row = row.child(session_audio_loop_button(
                cx,
                captured,
                loop_range,
                Some((edge, glyph)),
                edit_pending,
            ));
        }
    }
    if let Some(amplitudes) = waveform {
        let linear_gain = gain.linear();
        let mut bars = div()
            .debug_selector(|| "session-audio-waveform".into())
            .relative()
            .h(px(24.0))
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(1.0));
        for amplitude in amplitudes {
            bars = bars.child(
                div().flex_1().h(px(22.0)).flex().items_center().child(
                    div()
                        .w_full()
                        .h(px(
                            ((amplitude * linear_gain).clamp(0.0, 1.0) * 20.0).max(1.0)
                        ))
                        .bg(rgb(theme::color(theme::PaletteRole::Waveform))),
                ),
            );
        }
        if let Some(fraction) = playhead {
            bars = bars.child(
                div()
                    .debug_selector(|| "session-audio-playback-position".into())
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(relative(fraction))
                    .w(px(1.0))
                    .bg(rgb(theme::color(theme::PaletteRole::Secondary))),
            );
        }
        row = row.child(bars);
    }
    Some(row)
}

impl EditorView {
    /// Accepted Session playback of this cell only: the same revision, fault
    /// and route-owner guards as the piano roll's Session cursor, reading the
    /// launch published with the route snapshot. Never an optimistic launch.
    fn session_audio_playhead(&self, track: TrackId, clip: &SessionAudioClip) -> Option<f32> {
        use std::sync::atomic::Ordering::Acquire;
        if !clip.active() {
            return None;
        }
        let state = self.state.as_ref()?;
        let audible = self.audible_playback.as_ref()?;
        if audible.project_generation != self.project_generation
            || audible.state.revision != state.revision
        {
            return None;
        }
        let audio = self.audio.as_ref()?;
        let status = audio.control.status()?;
        if status.applied_revision.load(Acquire) != audible.audio_revision
            || status.device_fault.load(Acquire)
            || status.render_fault.load(Acquire)
            || status.shutdown_complete.load(Acquire)
        {
            return None;
        }
        let snapshot = self.session_routes.matching_snapshot(
            audio.control.output_identity()?,
            Some((
                audible.audio_revision,
                state.revision,
                self.project_generation,
            )),
            state.revision,
            self.project_generation,
        )?;
        let route = self
            .session_routes
            .entry_for_track(snapshot, true, track.value())?;
        if route.owner
            != (SessionRouteOwner::Session {
                clip_id: clip.id().value(),
            })
        {
            return None;
        }
        let fraction = playhead_fraction(
            clip,
            route.audio_launch_frame?,
            status.performance_frame.load(Acquire),
        )?;
        // A publication can change during scalar reads; never paint it against
        // another compiled revision's launch.
        (status.applied_revision.load(Acquire) == audible.audio_revision).then_some(fraction)
    }
}

fn session_audio_loop_button(
    cx: &mut Context<EditorView>,
    window: SourceWindow,
    range: Option<hex_project::SessionAudioLoop>,
    edge: Option<(SourceEdge, &'static str)>,
    pending: bool,
) -> Stateful<Div> {
    let label = edge.map_or("↻", |(_, glyph)| glyph);
    let selector = match edge.map(|(edge, _)| edge) {
        None => "session-audio-loop-toggle",
        Some(SourceEdge::StartEarlier) => "session-audio-loop-start-earlier",
        Some(SourceEdge::StartLater) => "session-audio-loop-start-later",
        Some(SourceEdge::EndEarlier) => "session-audio-loop-end-earlier",
        Some(SourceEdge::EndLater) => "session-audio-loop-end-later",
    };
    hex_widgets::action(theme::text(label), theme::action_style())
        .id((selector, window.clip.value()))
        .size(px(18.0))
        .debug_selector(move || selector.into())
        .text_color(rgb(theme::color(if range.is_some() {
            theme::PaletteRole::Primary
        } else {
            theme::PaletteRole::Muted
        })))
        .tooltip(move |_, cx| {
            let description = match edge.map(|(edge, _)| edge) {
                None => "toggle audio loop; enabling repeats the source window",
                Some(SourceEdge::StartEarlier) => "move loop start earlier · alt: 1 frame",
                Some(SourceEdge::StartLater) => "move loop start later · alt: 1 frame",
                Some(SourceEdge::EndEarlier) => "move loop end earlier · alt: 1 frame",
                Some(SourceEdge::EndLater) => "move loop end later · alt: 1 frame",
            };
            cx.new(|_| super::transport::MiniTooltip {
                kind: crate::tooltips::Kind::control("audio loop"),
                description: description.into(),
            })
            .into()
        })
        .on_click(cx.listener(move |view, event: &gpui::ClickEvent, _, cx| {
            let Some(state) = view.state.as_ref() else {
                return;
            };
            if pending
                || view.edit_pending()
                || view.mode != super::EditorMode::Session
                || !source_window_is_current(
                    window,
                    view.project_generation,
                    view.selected_cell,
                    state.revision,
                    &state.project,
                )
            {
                return;
            }
            let next = if let Some((edge, _)) = edge {
                let Some(range) = range else {
                    return;
                };
                let (start, mut end) = adjusted_source_range(
                    range.start(),
                    range.end(),
                    window.end,
                    edge,
                    source_step(state.project.sample_rate(), event.modifiers().alt),
                );
                end = end.max(window.start.saturating_add(1));
                let Ok(next) = hex_project::SessionAudioLoop::new(start, end) else {
                    return;
                };
                Some(next)
            } else if range.is_some() {
                None
            } else {
                hex_project::SessionAudioLoop::new(window.start, window.end).ok()
            };
            if next != range {
                view.enqueue(hex_session::service::Request::Edit(
                    Command::SetSessionAudioClipLoop {
                        clip: window.clip,
                        loop_range: next,
                    },
                ));
            }
            cx.notify();
        }))
}

fn gain_adjust_button(
    cx: &mut Context<EditorView>,
    window: SourceWindow,
    gain: hex_project::ClipGain,
    direction: i32,
    pending: bool,
) -> Stateful<Div> {
    hex_widgets::action(
        theme::text(if direction < 0 { "−" } else { "+" }),
        theme::action_style(),
    )
    .id(ElementId::Name(
        format!("session-audio-gain-{}-{direction}", window.clip.value()).into(),
    ))
    .size(px(18.0))
    .debug_selector(move || format!("session-audio-gain-{direction}"))
    .tooltip(|_, cx| {
        cx.new(|_| super::transport::MiniTooltip {
            kind: crate::tooltips::Kind::control("clip gain"),
            description: "adjust clip gain by 1 db · alt: 0.1 db".into(),
        })
        .into()
    })
    .on_click(cx.listener(move |view, event: &gpui::ClickEvent, _, cx| {
        let current = view.state.as_ref().is_some_and(|state| {
            source_window_is_current(
                window,
                view.project_generation,
                view.selected_cell,
                state.revision,
                &state.project,
            )
        });
        if current && view.mode == super::EditorMode::Session && !pending && !view.edit_pending() {
            let step = if event.modifiers().alt { 100 } else { 1_000 };
            if let Some(millidb) = gain.millidb().checked_add(direction * step)
                && hex_project::ClipGain::new(millidb).is_ok()
            {
                view.enqueue(hex_session::service::Request::Edit(
                    Command::SetAudioClipGain {
                        clip: window.clip,
                        millidb,
                    },
                ));
            }
        }
        cx.notify();
    }))
}

#[derive(Clone, Copy)]
enum SourceEdge {
    StartEarlier,
    StartLater,
    EndEarlier,
    EndLater,
}

#[derive(Clone, Copy)]
struct SourceWindow {
    clip: ClipId,
    asset: AssetId,
    track: hex_project::TrackId,
    scene: hex_project::SceneId,
    start: u64,
    end: u64,
    asset_frames: u64,
    revision: u64,
    project_generation: u64,
}

fn source_adjust_button(
    cx: &mut Context<EditorView>,
    glyph: &'static str,
    tooltip: &'static str,
    window: SourceWindow,
    edge: SourceEdge,
    edit_pending: bool,
) -> Stateful<Div> {
    let SourceWindow {
        clip,
        asset,
        start,
        end,
        ..
    } = window;
    let tooltip = if edit_pending {
        "audio edit pending".to_owned()
    } else {
        format!("{tooltip} by about 1 ms · alt: 1 frame")
    };
    let control_id = match edge {
        SourceEdge::StartEarlier => "start-earlier",
        SourceEdge::StartLater => "start-later",
        SourceEdge::EndEarlier => "end-earlier",
        SourceEdge::EndLater => "end-later",
    };
    hex_widgets::action(theme::text(glyph), theme::action_style())
        .id(ElementId::Name(
            format!("session-audio-source-control-{}-{control_id}", clip.value()).into(),
        ))
        .size(px(18.0))
        .text_color(rgb(if edit_pending {
            theme::color(theme::PaletteRole::Muted)
        } else {
            theme::color(theme::PaletteRole::Secondary)
        }))
        .tooltip(move |_, cx| {
            let description = tooltip.clone();
            cx.new(|_| super::transport::MiniTooltip {
                kind: crate::tooltips::Kind::Identification,
                description: description.into(),
            })
            .into()
        })
        .on_click(cx.listener(move |view, event: &gpui::ClickEvent, _, cx| {
            let still_current = view.state.as_ref().is_some_and(|state| {
                source_window_is_current(
                    window,
                    view.project_generation,
                    view.selected_cell,
                    state.revision,
                    &state.project,
                )
            });
            let sample_rate = view
                .state
                .as_ref()
                .map_or(0, |state| state.project.sample_rate());
            let step = source_step(sample_rate, event.modifiers().alt);
            let (next_start, next_end) = adjusted_window(window, edge, step);
            if still_current
                && view.mode == super::EditorMode::Session
                && !edit_pending
                && !view.edit_pending()
                && (next_start, next_end) != (start, end)
            {
                view.enqueue(hex_session::service::Request::Edit(
                    Command::SetSessionAudioClipSource {
                        clip,
                        asset,
                        source_start: next_start,
                        source_end: next_end,
                    },
                ));
            }
            cx.notify();
        }))
}

fn source_window_is_current(
    window: SourceWindow,
    project_generation: u64,
    selected_cell: Option<(hex_project::TrackId, hex_project::SceneId)>,
    revision: u64,
    project: &hex_project::Project,
) -> bool {
    project_generation == window.project_generation
        && selected_cell == Some((window.track, window.scene))
        && revision == window.revision
        && matches!(
            project.session_cell(window.track, window.scene),
            Some(SessionCellContent::Audio(current))
                if current.id() == window.clip
                    && current.asset() == window.asset
                    && current.source_start() == window.start
                    && current.source_end() == window.end
        )
}

fn source_step(sample_rate: u32, fine: bool) -> u64 {
    if fine {
        1
    } else {
        u64::from(sample_rate / 1_000).max(1)
    }
}

fn adjusted_window(window: SourceWindow, edge: SourceEdge, step: u64) -> (u64, u64) {
    let SourceWindow { start, end, .. } = window;
    adjusted_source_range(start, end, window.asset_frames, edge, step)
}

fn adjusted_source_range(
    start: u64,
    end: u64,
    asset_frames: u64,
    edge: SourceEdge,
    step: u64,
) -> (u64, u64) {
    match edge {
        SourceEdge::StartEarlier => (start.saturating_sub(step), end),
        SourceEdge::StartLater => (start.saturating_add(step).min(end - 1), end),
        SourceEdge::EndEarlier => (start, end.saturating_sub(step).max(start + 1)),
        SourceEdge::EndLater => (start, end.saturating_add(step).min(asset_frames)),
    }
}

fn waveform_bars(waveform: &CachedWaveform, start: u64, end: u64) -> Vec<f32> {
    let source_frames = waveform.source_frames();
    let start = usize::try_from(start)
        .unwrap_or(usize::MAX)
        .min(source_frames);
    let end = usize::try_from(end)
        .unwrap_or(usize::MAX)
        .min(source_frames)
        .max(start);
    if start == end || source_frames == 0 || waveform.bins().is_empty() {
        return vec![0.0; WAVEFORM_COLUMNS];
    }
    (0..WAVEFORM_COLUMNS)
        .map(|column| {
            let left = start + fraction(end - start, column, WAVEFORM_COLUMNS);
            if left >= end {
                return 0.0;
            }
            let right = (start + fraction(end - start, column + 1, WAVEFORM_COLUMNS))
                .max(left + 1)
                .min(end);
            let bins = bin_range(waveform, left, right);
            waveform
                .bins()
                .get(bins)
                .unwrap_or(&[])
                .iter()
                .flat_map(|stereo| stereo.iter())
                .map(|peak| peak.min.abs().max(peak.max.abs()))
                .fold(0.0_f32, f32::max)
                .clamp(0.0, 1.0)
        })
        .collect()
}

fn fraction(length: usize, numerator: usize, denominator: usize) -> usize {
    usize::try_from((length as u128 * numerator as u128) / denominator as u128).unwrap_or(length)
}

fn bin_range(waveform: &CachedWaveform, start: usize, end: usize) -> Range<usize> {
    let start = start.min(waveform.source_frames());
    let end = end.min(waveform.source_frames()).max(start);
    let count = waveform.bins().len();
    if start == end || count == 0 || waveform.source_frames() == 0 {
        return 0..0;
    }
    let width = waveform.source_frames() / count;
    let extra = waveform.source_frames() % count;
    let wide_frames = (width + 1).saturating_mul(extra);
    let at = |frame: usize| {
        if frame < wide_frames {
            frame / (width + 1)
        } else {
            extra + (frame - wide_frames) / width
        }
    };
    at(start).min(count)..(at(end - 1) + 1).min(count)
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use gpui::{AnyWindowHandle, AppContext, Empty, TestAppContext, VisualTestContext, px};
    use hex_project::{Command, Editor, Project, SessionCellContent};
    use std::sync::Arc;

    #[gpui::test]
    fn source_panel_keeps_its_compact_row_while_an_edit_is_pending(cx: &mut TestAppContext) {
        use super::super::{EditorMode, EditorView, install_close_guard};
        use hex_project::Created;
        use hex_session::service::{Service, ViewState};

        let mut project =
            Editor::new(Project::new(48_000).expect("project"), 8).expect("project editor");
        let Created::Track(track) = project
            .execute(Command::AddTrack {
                name: "audio".into(),
            })
            .expect("audio track")
        else {
            panic!("audio track expected")
        };
        let Created::Scene(scene) = project
            .execute(Command::AddScene {
                name: "scene".into(),
            })
            .expect("scene")
        else {
            panic!("scene expected")
        };
        let Created::Asset(asset) = project
            .execute(Command::AddAsset {
                path: "source.wav".into(),
                sample_rate: 48_000,
                frames: 32,
            })
            .expect("asset")
        else {
            panic!("asset expected")
        };
        project
            .execute(Command::CreateSessionAudioClip {
                track,
                scene,
                name: "source".into(),
                asset,
                source_start: 4,
                source_end: 20,
            })
            .expect("session audio clip");

        let bootstrap = cx.add_window(|_, _| Empty);
        let handle: AnyWindowHandle = *bootstrap;
        cx.update_window(handle, |_, window, window_cx| {
            let service = Service::new(48_000, 32, 8_000_000).expect("test service");
            let editor = window.replace_root(window_cx, |window, window_cx| {
                EditorView::new(
                    window,
                    window_cx,
                    service,
                    Err("test offline output".into()),
                )
            });
            install_close_guard(window, window_cx, editor.downgrade());
            editor.update(window_cx, |view, window_cx| {
                view.state = Some(Arc::new(ViewState {
                    canonical_audio_source: None,
                    comparison_audio: None,
                    project: Arc::new(project.project().clone()),
                    waveforms: Default::default(),
                    source_overviews: Default::default(),
                    audio_clip_waveforms: std::collections::BTreeMap::new(),
                    midi_preview: Arc::new(Default::default()),
                    gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
                    timeline_frames: 384_000,
                    can_undo: false,
                    can_redo: false,
                    revision: 1,
                    unsaved: false,
                    document_path: None,
                }));
                view.mode = EditorMode::Session;
                view.selected_cell = Some((track, scene));
                view.worker_requests_pending = 1;
                view.focus.focus(window);
                window_cx.notify();
            });
        })
        .expect("audio editor window");

        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let row = visual
            .debug_bounds("session-audio-source-panel")
            .expect("source panel should remain visible while editing");
        assert!(row.size.height > px(0.0));
        assert!(row.size.height <= px(38.0));
        let readout = visual
            .debug_bounds("session-audio-source-readout")
            .expect("source bounds remain visible while editing");
        assert!(readout.size.width > px(0.0));
    }
    #[gpui::test]
    fn console_audio_loop_controls_edit_real_cell_history_and_saved_state(cx: &mut TestAppContext) {
        use super::super::{EditorMode, clip_drag_ui_test, midi_ruler};
        use gpui::{Modifiers, MouseButton};
        use hex_session::service::Request;
        let (files, handle, editor) = clip_drag_ui_test::upper_rail_owned_fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::AddScene {
                name: "loop scene".into()
            })));
        });
        clip_drag_ui_test::pump_move(&editor, &mut visual);
        let (track, scene, source) = editor.read_with(&visual, |v, _| {
            let p = &v.state.as_ref().unwrap().project;
            (
                p.tracks()[0].id(),
                p.scenes()[0].id(),
                p.tracks()[0].clips()[0].id(),
            )
        });
        editor.update(&mut visual, |v, _| {
            assert!(
                v.enqueue(Request::Edit(Command::CopyArrangementAudioClipToSession {
                    source,
                    scene
                }))
            );
        });
        clip_drag_ui_test::pump_move(&editor, &mut visual);
        let clip = editor.update(&mut visual, |v, cx| {
            let SessionCellContent::Audio(c) = v
                .state
                .as_ref()
                .unwrap()
                .project
                .session_cell(track, scene)
                .unwrap()
            else {
                panic!("audio")
            };
            let id = c.id();
            v.mode = EditorMode::Session;
            v.selected_cell = Some((track, scene));
            v.selected_clip = Some(id);
            v.clip_view_open = true;
            v.device_panel_active = false;
            cx.notify();
            id
        });
        visual.run_until_parked();
        let original = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.clone());
        for selector in [
            "session-audio-loop-toggle",
            "session-audio-loop-start-later",
            "session-audio-loop-end-earlier",
        ] {
            let b = visual.debug_bounds(selector).unwrap();
            visual.simulate_mouse_down(
                b.center(),
                MouseButton::Left,
                Modifiers {
                    alt: true,
                    ..Modifiers::default()
                },
            );
            visual.simulate_mouse_up(
                b.center(),
                MouseButton::Left,
                Modifiers {
                    alt: true,
                    ..Modifiers::default()
                },
            );
            clip_drag_ui_test::pump_move(&editor, &mut visual);
            editor.read_with(&visual, |v, _| assert!(v.error.is_none(), "{:?}", v.error));
        }
        let changed = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.clone());
        let c = changed.session_audio_clip(clip).unwrap();
        assert_eq!(
            c.loop_range(),
            Some(
                hex_project::SessionAudioLoop::new(c.source_start() + 1, c.source_end() - 1)
                    .unwrap()
            )
        );
        assert_eq!(changed.tracks(), original.tracks());
        let reopened = midi_ruler::roundtrip(&editor, &mut visual, &files);
        assert_eq!(*reopened, *changed);
        for _ in 0..3 {
            editor.update(&mut visual, |v, _| {
                assert!(v.enqueue(Request::Undo));
            });
            clip_drag_ui_test::pump_move(&editor, &mut visual);
        }
        editor.read_with(&visual, |v, _| {
            assert_eq!(*v.state.as_ref().unwrap().project, *original)
        });
        for _ in 0..3 {
            editor.update(&mut visual, |v, _| {
                assert!(v.enqueue(Request::Redo));
            });
            clip_drag_ui_test::pump_move(&editor, &mut visual);
        }
        editor.read_with(&visual, |v, _| {
            assert_eq!(*v.state.as_ref().unwrap().project, *changed)
        });
    }
}
#[cfg(all(test, not(feature = "ui-test-support")))]
#[path = "session_audio_panel/unit_tests.rs"]
mod unit_tests;
