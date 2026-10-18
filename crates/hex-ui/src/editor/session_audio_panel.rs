//! Compact source-window controls for the selected Session audio one-shot.
use super::EditorView;
use crate::theme;
use gpui::{Context, Div, ElementId, Stateful, div, prelude::*, px, rgb};
use hex_project::{AssetId, ClipId, Command, SessionCellContent};
use hex_session::CachedWaveform;
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
        .h(px(DETAIL_HEIGHT))
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
    if let Some(amplitudes) = waveform {
        let linear_gain = gain.linear();
        let mut bars = div()
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
        row = row.child(bars);
    }
    Some(row)
}

fn gain_adjust_button(
    cx: &mut Context<EditorView>,
    window: SourceWindow,
    gain: hex_project::ClipGain,
    direction: i32,
    pending: bool,
) -> Stateful<Div> {
    div()
        .id(ElementId::Name(
            format!("session-audio-gain-{}-{direction}", window.clip.value()).into(),
        ))
        .size(px(18.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .debug_selector(move || format!("session-audio-gain-{direction}"))
        .tooltip(|_, cx| {
            cx.new(|_| super::transport::MiniTooltip {
                description: "adjust clip gain by 1 db · alt: 0.1 db".into(),
            })
            .into()
        })
        .child(theme::text(if direction < 0 { "−" } else { "+" }))
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
            if current
                && view.mode == super::EditorMode::Session
                && !pending
                && !view.edit_pending()
            {
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
    div()
        .id(ElementId::Name(
            format!("session-audio-source-control-{}-{control_id}", clip.value()).into(),
        ))
        .size(px(18.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgb(if edit_pending {
            theme::color(theme::PaletteRole::Muted)
        } else {
            theme::color(theme::PaletteRole::Secondary)
        }))
        .hover(|style| style.bg(rgb(theme::color(theme::PaletteRole::Background))))
        .tooltip(move |_, cx| {
            let description = tooltip.clone();
            cx.new(|_| super::transport::MiniTooltip {
                description: description.into(),
            })
            .into()
        })
        .child(theme::text(glyph))
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
    let source_frames = waveform.source_frames;
    let start = usize::try_from(start)
        .unwrap_or(usize::MAX)
        .min(source_frames);
    let end = usize::try_from(end)
        .unwrap_or(usize::MAX)
        .min(source_frames)
        .max(start);
    if start == end || source_frames == 0 || waveform.bins.is_empty() {
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
                .bins
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
    let start = start.min(waveform.source_frames);
    let end = end.min(waveform.source_frames).max(start);
    let count = waveform.bins.len();
    if start == end || count == 0 || waveform.source_frames == 0 {
        return 0..0;
    }
    let width = waveform.source_frames / count;
    let extra = waveform.source_frames % count;
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

#[cfg(test)]
mod tests {
    use super::{
        SourceEdge, SourceWindow, WAVEFORM_COLUMNS, adjusted_source_range, bin_range, source_step,
        source_window_is_current, waveform_bars,
    };
    #[cfg(feature = "ui-test-support")]
    use gpui::{AnyWindowHandle, AppContext, Empty, TestAppContext, VisualTestContext, px};
    use hex_project::{Command, Created, Editor, Project, SessionCellContent};
    use hex_session::{CachedPeak, CachedWaveform};
    #[cfg(feature = "ui-test-support")]
    use std::sync::Arc;

    #[test]
    fn source_waveform_is_cropped_to_the_half_open_one_shot_window() {
        let waveform = CachedWaveform::from_peaks(
            9,
            (0..9)
                .map(|index| {
                    let value = if (3..7).contains(&index) { 0.75 } else { 0.0 };
                    [
                        CachedPeak {
                            min: -value,
                            max: value,
                        },
                        CachedPeak { min: 0.0, max: 0.0 },
                    ]
                })
                .collect(),
        )
        .expect("valid waveform fixture");
        let bars = waveform_bars(&waveform, 3, 7);
        assert_eq!(bars.len(), WAVEFORM_COLUMNS);
        assert!(bars.iter().all(|value| *value == 0.75));
        assert_eq!(bin_range(&waveform, 3, 7), 3..7);
        assert!(
            waveform_bars(&waveform, 7, 7)
                .iter()
                .all(|value| *value == 0.0)
        );
    }

    #[test]
    fn source_nudges_keep_the_half_open_window_inside_the_asset() {
        assert_eq!(
            adjusted_source_range(1, 10, 10, SourceEdge::StartEarlier, 1),
            (0, 10)
        );
        assert_eq!(
            adjusted_source_range(1, 10, 10, SourceEdge::StartLater, 1),
            (2, 10)
        );
        assert_eq!(
            adjusted_source_range(1, 10, 10, SourceEdge::EndEarlier, 1),
            (1, 9)
        );
        assert_eq!(
            adjusted_source_range(1, 10, 10, SourceEdge::EndLater, 1),
            (1, 10)
        );
        assert_eq!(source_step(48_000, false), 48);
        assert_eq!(source_step(44_100, false), 44);
        assert_eq!(source_step(192_000, true), 1);
        assert_eq!(source_step(1, false), 1);
        assert_eq!(
            adjusted_source_range(10, 20, 100, SourceEdge::StartLater, 48),
            (19, 20)
        );
        assert_eq!(
            adjusted_source_range(10, 20, 100, SourceEdge::EndEarlier, 48),
            (10, 11)
        );
        assert_eq!(
            adjusted_source_range(10, 20, 100, SourceEdge::EndLater, u64::MAX),
            (10, 100)
        );
    }

    #[test]
    fn source_controls_reject_stale_generation_revision_or_selection() {
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
        let track = match editor
            .execute(Command::AddTrack {
                name: "audio".into(),
            })
            .expect("audio track")
        {
            Created::Track(id) => id,
            _ => panic!("track"),
        };
        let scene = match editor
            .execute(Command::AddScene {
                name: "scene".into(),
            })
            .expect("scene")
        {
            Created::Scene(id) => id,
            _ => panic!("scene"),
        };
        let asset = match editor
            .execute(Command::AddAsset {
                path: "source.wav".into(),
                sample_rate: 48_000,
                frames: 32,
            })
            .expect("asset")
        {
            Created::Asset(id) => id,
            _ => panic!("asset"),
        };
        let clip = match editor
            .execute(Command::CreateSessionAudioClip {
                track,
                scene,
                name: "audio".into(),
                asset,
                source_start: 4,
                source_end: 20,
            })
            .expect("Session audio clip")
        {
            Created::SessionClip(id) => id,
            _ => panic!("Session clip"),
        };
        let window = SourceWindow {
            clip,
            asset,
            track,
            scene,
            start: 4,
            end: 20,
            asset_frames: 32,
            revision: 7,
            project_generation: 3,
        };
        let project = editor.project();
        assert!(source_window_is_current(
            window,
            3,
            Some((track, scene)),
            7,
            project
        ));
        assert!(!source_window_is_current(
            window,
            4,
            Some((track, scene)),
            7,
            project
        ));
        assert!(!source_window_is_current(
            window,
            3,
            Some((track, scene)),
            8,
            project
        ));
        assert!(!source_window_is_current(window, 3, None, 7, project));
        assert!(matches!(
            project.session_cell(track, scene),
            Some(SessionCellContent::Audio(item)) if item.id() == clip
        ));
    }

    #[cfg(feature = "ui-test-support")]
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
                    project: hex_session::SharedProject::new_on_control(project.project().clone()),
                    waveforms: Default::default(),
                    audio_clip_waveforms: Default::default(),
                    midi_preview: Arc::new(Default::default()),
                    gain_preview: Ok(hex_engine::SharedGainAutomationPreview::new_on_control(
                        hex_engine::GainAutomationPreview::default(),
                    )),
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
}
