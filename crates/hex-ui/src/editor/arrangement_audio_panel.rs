//! Arrangement audio Clip View with canonical source and timeline controls.
use super::EditorView;
use crate::theme;
use gpui::{Context, Div, ElementId, Stateful, div, prelude::*, px, relative, rgb};
use hex_project::{AssetId, AudioViewMode, ClipId, Command, TrackId};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    Position,
    Length,
    SourceStart,
    SourceEnd,
    PlayStart,
    Looping,
    Gain,
}

#[derive(Clone, Copy)]
struct ClipWindow {
    track: TrackId,
    clip: ClipId,
    asset: AssetId,
    position: u64,
    length: u64,
    source_start: u64,
    source_length: u64,
    source_end: u64,
    source_phase: u64,
    source_looping: bool,
    placement_mode: Option<AudioViewMode>,
    gain: hex_project::ClipGain,
    asset_frames: u64,
    sample_rate: u32,
    revision: u64,
    generation: u64,
}

impl ClipWindow {
    fn looping(self) -> bool {
        self.placement_mode
            .map_or(self.source_looping, |mode| mode == AudioViewMode::Repeat)
    }

    fn loop_tooltip(self) -> &'static str {
        match (self.placement_mode, self.looping()) {
            (Some(_), true) => "disable clip repetition · retain source cycle and play start",
            (Some(_), false) => "enable clip repetition",
            (None, true) => "disable source looping · reset play start to source start",
            (None, false) => "enable source looping",
        }
    }
}

pub(super) fn view(view: &mut EditorView, cx: &mut Context<EditorView>) -> Option<Div> {
    if view.mode != super::EditorMode::Arrangement {
        return None;
    }
    let state = view.state.as_ref()?.clone();
    let (track, clip_id, position, length, source_start, source_length) =
        view.selected_clip_data()?;
    let clip = state.project.clip(clip_id)?;
    let asset = state.project.asset(clip.asset())?;
    let source_end = source_start.checked_add(source_length)?;
    let window = ClipWindow {
        track,
        clip: clip_id,
        asset: clip.asset(),
        position,
        length,
        source_start,
        source_length,
        source_end,
        source_phase: clip.source_phase(),
        source_looping: clip.looping(),
        placement_mode: clip
            .audio_edit()
            .and_then(|edit| edit.placement)
            .map(|p| p.mode),
        gain: clip.gain(),
        asset_frames: asset.playback_frames(),
        sample_rate: state.project.sample_rate(),
        revision: state.revision,
        generation: view.project_generation,
    };
    let pending = view.edit_pending();
    let waveform = state.waveforms.get(&window.asset).cloned();
    let title = lowercase_path_tail(asset.path());
    let mut details = div()
        .h(px(super::clip_view::CLIP_DETAIL_BODY_HEIGHT))
        .w_full()
        .flex_shrink_0()
        .flex()
        .bg(rgb(theme::color(theme::PaletteRole::Surface)))
        .border_t_1()
        .border_color(rgb(theme::color(theme::PaletteRole::Border)))
        .debug_selector(|| "arrangement-audio-clip-view".into());

    let mut controls = div()
        .id("arrangement-audio-controls")
        .w(px(206.0))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .h_full()
        .overflow_y_scroll()
        .gap(px(2.0))
        .p(px(4.0))
        .border_r_1()
        .border_color(rgb(theme::color(theme::PaletteRole::Border)))
        .bg(rgb(theme::color(theme::PaletteRole::Raised)))
        .debug_selector(|| "arrangement-audio-controls".into());
    controls = controls.child(
        div()
            .h(px(20.0))
            .flex_shrink_0()
            .truncate()
            .text_xs()
            .text_color(rgb(theme::color(theme::PaletteRole::Text)))
            .child(theme::text(title)),
    );
    controls = controls.child(super::musical_audio_controls::warp_header(
        view, clip_id, cx,
    ));
    controls = controls
        .child(field_row(
            cx,
            window,
            Field::Position,
            "pos",
            format_frame_time(position, window.sample_rate),
            pending,
        ))
        .child(field_row(
            cx,
            window,
            Field::Length,
            "len",
            format_frame_time(length, window.sample_rate),
            pending,
        ))
        .child(field_row(
            cx,
            window,
            Field::SourceStart,
            "src in",
            format_frame_time(source_start, window.sample_rate),
            pending,
        ))
        .child(field_row(
            cx,
            window,
            Field::PlayStart,
            "play at",
            format_frame_time(
                source_start.saturating_add(window.source_phase),
                window.sample_rate,
            ),
            pending,
        ))
        .child(field_row(
            cx,
            window,
            Field::SourceEnd,
            "src end",
            format_frame_time(source_end, window.sample_rate),
            pending,
        ))
        .child(field_row(
            cx,
            window,
            Field::Gain,
            "gain",
            format!("{:+.1} db", f64::from(window.gain.millidb()) / 1_000.0),
            pending,
        ))
        .child(loop_row(cx, window, pending));
    controls = controls.child(super::musical_audio_controls::view(view, clip_id, cx));
    details = details.child(controls);

    let mut waveform_panel = div()
        .id("arrangement-audio-waveform-panel")
        .flex_1()
        .min_w_0()
        .h_full()
        .flex()
        .flex_col()
        .p(px(4.0))
        .gap(px(2.0))
        .debug_selector(|| "arrangement-audio-waveform-panel".into());
    let source_view = view.audio_event_editor.source_view;
    waveform_panel = waveform_panel.child(
        div()
            .h(px(20.0))
            .flex_shrink_0()
            .flex()
            .gap_1()
            .child(
                super::waveform::button("events", "edit audio events", ("audio-events-tab", 0))
                    .w(px(56.0))
                    .debug_selector(|| "audio-events-tab".into())
                    .on_click(cx.listener(|view, _, window, cx| {
                        view.audio_event_editor.source_view = false;
                        view.audio_event_focus.focus(window);
                        cx.notify();
                    })),
            )
            .child(
                super::waveform::button(
                    "source",
                    "inspect original source and warp markers",
                    ("audio-source-tab", 0),
                )
                .w(px(56.0))
                .debug_selector(|| "audio-source-tab".into())
                .on_click(cx.listener(|view, _, window, cx| {
                    view.audio_event_editor.gesture = None;
                    view.audio_event_editor.source_view = true;
                    view.focus.focus(window);
                    cx.notify();
                })),
            ),
    );
    if !source_view {
        waveform_panel = waveform_panel.child(super::audio_event_editor::view(view, cx)?);
        return Some(details.child(waveform_panel));
    }
    if let Some(prepared) = state.audio_clip_waveforms.get(&clip_id) {
        waveform_panel = waveform_panel.child(div().h(px(52.0)).flex_shrink_0().child(
            super::waveform::prepared_waveform_view_cached(
                prepared,
                clip,
                clip.start(),
                clip.start() + clip.duration(),
                std::rc::Rc::clone(&view.waveform_geometry_cache),
            ),
        ));
        waveform_panel = waveform_panel.child(div().text_xs().child(theme::text(
            "audible output · source and warp markers below",
        )));
    }
    waveform_panel = waveform_panel.child(time_ruler(window));
    let mut canvas_region = div()
        .id("arrangement-audio-waveform")
        .relative()
        .flex_1()
        .min_h_0()
        .w_full()
        .overflow_hidden()
        .bg(rgb(theme::color(theme::PaletteRole::Surface)))
        .border_1()
        .border_color(rgb(theme::color(theme::PaletteRole::Border)))
        .debug_selector(|| "arrangement-audio-waveform".into());
    if let Some(waveform) = waveform {
        canvas_region =
            canvas_region.child(super::waveform::waveform_source_segment_gain_view_cached(
                waveform,
                0,
                0,
                window.asset_frames,
                window.gain,
                std::rc::Rc::clone(&view.waveform_geometry_cache),
            ));
    } else {
        canvas_region = canvas_region.child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .text_xs()
                .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                .child(theme::text("waveform unavailable")),
        );
    }
    if window.asset_frames > 0 {
        canvas_region = canvas_region
            .child(source_marker(
                window.source_start,
                window.asset_frames,
                "source-start-marker",
                theme::color(theme::PaletteRole::Primary),
            ))
            .child(source_marker(
                window.source_end,
                window.asset_frames,
                "source-end-marker",
                theme::color(theme::PaletteRole::Primary),
            ))
            .child(source_marker(
                window.source_start.saturating_add(window.source_phase),
                window.asset_frames,
                "play-start-marker",
                theme::color(theme::PaletteRole::Secondary),
            ));
    }
    if let Some(edit) = clip.audio_edit() {
        if let Some(warp) = edit.domain.warp.as_ref() {
            for marker in &warp.markers {
                canvas_region = canvas_region.child(source_marker(
                    marker.source_frame,
                    window.asset_frames,
                    "warp-source-marker",
                    theme::color(theme::PaletteRole::Secondary),
                ));
            }
        }
        for point in &edit.domain.source_gain {
            canvas_region = canvas_region.child(source_marker(
                point.frame,
                window.asset_frames,
                "source-gain-point",
                theme::color(theme::PaletteRole::Primary),
            ));
        }
    }
    waveform_panel = waveform_panel.child(canvas_region).child(
        div()
            .h(px(16.0))
            .flex_shrink_0()
            .text_xs()
            .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
            .debug_selector(|| "arrangement-audio-source-range".into())
            .child(theme::text(format!(
                "source {}–{} / {} frames · play at {}{}",
                window.source_start,
                window.source_end,
                window.asset_frames,
                window.source_start.saturating_add(window.source_phase),
                if window.source_looping {
                    " · loop on"
                } else {
                    ""
                }
            ))),
    );
    details = details.child(waveform_panel);
    Some(details)
}

fn lowercase_path_tail(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .to_lowercase()
}

fn format_frame_time(frames: u64, sample_rate: u32) -> String {
    let rate = u64::from(sample_rate.max(1));
    let millis = frames.saturating_mul(1_000) / rate;
    format!("{frames} · {}.{:03}s", millis / 1_000, millis % 1_000)
}

fn field_row(
    cx: &mut Context<EditorView>,
    window: ClipWindow,
    field: Field,
    label: &'static str,
    value: String,
    pending: bool,
) -> Div {
    let tooltip_minus = match field {
        Field::Position => "move clip earlier · alt: one frame",
        Field::Length => "shorten timeline length · alt: one frame",
        Field::SourceStart => {
            "move source interval start earlier · trim resets loop phase · alt: one frame"
        }
        Field::SourceEnd => {
            "move source interval end earlier · trim resets loop phase · alt: one frame"
        }
        Field::PlayStart => "move play start earlier inside the source interval · alt: one frame",
        Field::Looping => window.loop_tooltip(),
        Field::Gain => "adjust clip gain by 1 db · alt: 0.1 db",
    };
    let tooltip_plus = match field {
        Field::Position => "move clip later · alt: one frame",
        Field::Length => "extend timeline length · alt: one frame",
        Field::SourceStart => {
            "move source interval start later · trim resets loop phase · alt: one frame"
        }
        Field::SourceEnd => {
            "move source interval end later · trim resets loop phase · alt: one frame"
        }
        Field::PlayStart => "move play start later inside the source interval · alt: one frame",
        Field::Looping => window.loop_tooltip(),
        Field::Gain => "adjust clip gain by 1 db · alt: 0.1 db",
    };
    div()
        .h(px(24.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(2.0))
        .child(
            div()
                .w(px(54.0))
                .flex_shrink_0()
                .text_xs()
                .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                .child(theme::text(label)),
        )
        .child(step_button(
            cx,
            window,
            field,
            -1,
            "−",
            tooltip_minus,
            pending,
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_xs()
                .text_color(rgb(theme::color(theme::PaletteRole::Text)))
                .debug_selector(move || format!("audio-clip-{field:?}-value").to_lowercase())
                .child(theme::text(value)),
        )
        .child(step_button(
            cx,
            window,
            field,
            1,
            "+",
            tooltip_plus,
            pending,
        ))
}

fn loop_row(cx: &mut Context<EditorView>, window: ClipWindow, pending: bool) -> Div {
    let tooltip = window.loop_tooltip();
    div()
        .h(px(24.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(3.0))
        .child(
            div()
                .w(px(54.0))
                .flex_shrink_0()
                .text_xs()
                .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                .child(theme::text("loop")),
        )
        .child(control_button(
            window,
            Field::Looping,
            1,
            if window.looping() { "↻" } else { "○" },
            tooltip,
            pending,
            cx,
        ))
        .child(
            div()
                .text_xs()
                .text_color(rgb(theme::color(theme::PaletteRole::Text)))
                .child(theme::text(if window.looping() { "on" } else { "off" })),
        )
}

fn step_button(
    cx: &mut Context<EditorView>,
    window: ClipWindow,
    field: Field,
    direction: i8,
    glyph: &'static str,
    tooltip: &'static str,
    pending: bool,
) -> Stateful<Div> {
    control_button(window, field, direction, glyph, tooltip, pending, cx)
}

fn control_button(
    window: ClipWindow,
    field: Field,
    direction: i8,
    glyph: &'static str,
    tooltip: &'static str,
    pending: bool,
    cx: &mut Context<EditorView>,
) -> Stateful<Div> {
    let id = ElementId::Name(
        format!("audio-clip-{}-{field:?}-{direction}", window.clip.value())
            .to_lowercase()
            .into(),
    );
    div()
        .id(id)
        .size(px(18.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgb(theme::color(theme::PaletteRole::SecondaryText)))
        .hover(|style| style.bg(rgb(theme::color(theme::PaletteRole::Background))))
        .debug_selector(move || {
            let field = match field {
                Field::Position => "position",
                Field::Length => "length",
                Field::SourceStart => "source-start",
                Field::SourceEnd => "source-end",
                Field::PlayStart => "play-start",
                Field::Looping => "looping",
                Field::Gain => "gain",
            };
            format!("audio-clip-{field}-{direction}")
        })
        .tooltip(move |_, cx| {
            cx.new(|_| super::transport::MiniTooltip {
                description: tooltip.into(),
            })
            .into()
        })
        .child(theme::text(glyph))
        .on_click(cx.listener(move |view, event: &gpui::ClickEvent, _, cx| {
            if pending || !clip_window_is_current(view, window) || view.edit_pending() {
                return;
            }
            let fine = event.modifiers().alt;
            let step = if field == Field::Gain {
                if fine { 100 } else { 1_000 }
            } else {
                frame_step(window.sample_rate, fine)
            };
            if let Some(command) = command_for(window, field, direction, step) {
                view.enqueue(hex_session::service::Request::Edit(command));
            }
            cx.notify();
        }))
}

fn frame_step(sample_rate: u32, fine: bool) -> u64 {
    if fine {
        1
    } else {
        u64::from((sample_rate / 1_000).max(1))
    }
}

fn command_for(window: ClipWindow, field: Field, direction: i8, step: u64) -> Option<Command> {
    let shifted = |value: u64| {
        if direction < 0 {
            value.checked_sub(step)
        } else if direction > 0 {
            value.checked_add(step)
        } else {
            None
        }
    };
    match field {
        Field::Position => shifted(window.position).map(|start| Command::MoveClip {
            clip: window.clip,
            track: window.track,
            start,
        }),
        Field::Length => shifted(window.length)
            .filter(|length| *length > 0)
            .map(|duration| Command::TrimClip {
                clip: window.clip,
                start: window.position,
                duration,
                source_offset: window.source_start,
                source_length: window.source_length,
            }),
        Field::SourceStart => {
            let end = window.source_end;
            let start = if direction < 0 {
                window.source_start.checked_sub(step)?
            } else if direction > 0 {
                window
                    .source_start
                    .checked_add(step)?
                    .min(end.saturating_sub(1))
            } else {
                return None;
            };
            if start >= end {
                return None;
            }
            Some(Command::TrimClip {
                clip: window.clip,
                start: window.position,
                duration: window.length,
                source_offset: start,
                source_length: end - start,
            })
        }
        Field::SourceEnd => {
            let end = if direction < 0 {
                window
                    .source_end
                    .checked_sub(step)?
                    .max(window.source_start + 1)
            } else if direction > 0 {
                window
                    .source_end
                    .checked_add(step)?
                    .min(window.asset_frames)
            } else {
                return None;
            };
            if end <= window.source_start {
                return None;
            }
            Some(Command::TrimClip {
                clip: window.clip,
                start: window.position,
                duration: window.length,
                source_offset: window.source_start,
                source_length: end - window.source_start,
            })
        }
        Field::PlayStart if window.source_looping => shifted(window.source_phase)
            .filter(|phase| *phase < window.source_length)
            .map(|phase| Command::SetClipSourcePhase {
                clip: window.clip,
                phase,
            }),
        Field::PlayStart => {
            let start = shifted(window.source_start)?;
            if start >= window.source_end {
                return None;
            }
            Some(Command::TrimClip {
                clip: window.clip,
                start: window.position,
                duration: window.length,
                source_offset: start,
                source_length: window.source_end - start,
            })
        }
        Field::Gain => {
            let delta = i32::try_from(step)
                .ok()?
                .checked_mul(i32::from(direction))?;
            let millidb = window.gain.millidb().checked_add(delta)?;
            hex_project::ClipGain::new(millidb).ok()?;
            Some(Command::SetAudioClipGain {
                clip: window.clip,
                millidb,
            })
        }
        Field::Looping => Some(Command::SetClipLooping {
            clip: window.clip,
            looping: !window.looping(),
        }),
    }
}

fn clip_window_is_current(view: &EditorView, captured: ClipWindow) -> bool {
    if view.mode != super::EditorMode::Arrangement
        || view.project_generation != captured.generation
        || view.selected_track != Some(captured.track)
        || view.selected_clip != Some(captured.clip)
    {
        return false;
    }
    view.state.as_ref().is_some_and(|state| {
        state.revision == captured.revision
            && state.project.clip(captured.clip).is_some_and(|clip| {
                clip.asset() == captured.asset
                    && clip.start() == captured.position
                    && clip.duration() == captured.length
                    && clip.source_offset() == captured.source_start
                    && clip.source_length() == captured.source_length
                    && clip.gain() == captured.gain
                    && clip.looping() == captured.source_looping
                    && clip
                        .audio_edit()
                        .and_then(|edit| edit.placement)
                        .map(|p| p.mode)
                        == captured.placement_mode
                    && clip.source_phase() == captured.source_phase
            })
            && state
                .project
                .asset(captured.asset)
                .is_some_and(|asset| asset.playback_frames() == captured.asset_frames)
    })
}

fn time_ruler(window: ClipWindow) -> Div {
    let mut ruler = div()
        .relative()
        .h(px(18.0))
        .flex_shrink_0()
        .w_full()
        .overflow_hidden()
        .bg(rgb(theme::color(theme::PaletteRole::Raised)))
        .text_xs()
        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
        .debug_selector(|| "arrangement-audio-time-ruler".into());
    if window.asset_frames == 0 {
        return ruler;
    }
    for index in 0..=4_u64 {
        let frame = window.asset_frames.saturating_mul(index) / 4;
        let x = frame as f32 / window.asset_frames as f32;
        let ruler_label = div().absolute();
        let ruler_label = if index == 4 {
            ruler_label.right_0()
        } else {
            ruler_label.left(relative(x))
        };
        let ruler_label = ruler_label
            .top_0()
            .bottom_0()
            .flex()
            .items_center()
            .child(theme::text(format_frame_time(frame, window.sample_rate)));
        ruler = ruler.child(ruler_label);
    }
    ruler
}

fn source_marker(frame: u64, total: u64, id: &'static str, color: u32) -> Div {
    let marker = div().debug_selector(move || id.into()).absolute();
    let marker = if frame >= total {
        marker.right_0()
    } else {
        marker.left(relative((frame as f32 / total as f32).clamp(0.0, 1.0)))
    };
    marker.top_0().bottom_0().w(px(1.0)).bg(rgb(color))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "ui-test-support")]
    fn placed_loop_fixture(
        cx: &mut gpui::TestAppContext,
        mode: AudioViewMode,
    ) -> (gpui::AnyWindowHandle, gpui::Entity<EditorView>, ClipId) {
        use gpui::AppContext;
        use hex_project::{
            AudioDomainClock, AudioViewPlacement, ClipFade, Created, Editor, Project,
        };
        use std::sync::Arc;
        let mut model = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Imported { track, clip, .. } = model
            .execute(Command::ImportAudioTrack {
                name: "loop fixture".into(),
                path: "never-opened.wav".into(),
                sample_rate: 48_000,
                frames: 480_000,
            })
            .unwrap()
        else {
            panic!("import")
        };
        model
            .execute(Command::SetAudioClipPitch { clip, cents: 123 })
            .unwrap();
        let raw_looping = mode == hex_project::AudioViewMode::Once;
        model
            .execute(Command::SetClipLooping {
                clip,
                looping: raw_looping,
            })
            .unwrap();
        let mut snapshot = model.project().snapshot();
        let c = &mut snapshot.tracks[0].audio_clips_mut().unwrap()[0];
        let edit = c.edit.as_mut().unwrap();
        edit.view_start = 12_000;
        edit.view_end = 36_000;
        edit.placement = Some(AudioViewPlacement {
            mode,
            outer_length: AudioDomainClock::Frames(48_000),
            cycle_phase: 6_000,
        });
        c.duration = 48_000;
        c.fade = ClipFade::neutral(48_000).unwrap();
        let project = Project::from_snapshot(snapshot).unwrap();
        let (handle, editor) =
            super::super::clip_drag_ui_test::editor_window(cx, project, Arc::default());
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                // The merged test constructor owns these paths before starting workers.
                assert!(
                    view.browser
                        .directory
                        .starts_with(view.test_environment.root())
                );
                assert!(
                    view.browser
                        .places
                        .iter()
                        .all(|p| p.starts_with(view.test_environment.root()))
                );
                view.selected_track = Some(track);
                view.selected_clip = Some(clip);
                view.clip_view_open = true;
                view.focus.focus(window);
                cx.notify();
            })
        })
        .unwrap();
        (handle, editor, clip)
    }

    #[cfg(feature = "ui-test-support")]
    fn visible_loop_button(visual: &mut gpui::VisualTestContext) -> gpui::Point<gpui::Pixels> {
        use gpui::Modifiers;
        visual.run_until_parked();
        let controls = visual.debug_bounds("arrangement-audio-controls").unwrap();
        visual.simulate_event(gpui::ScrollWheelEvent {
            position: controls.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.0), px(-200.0))),
            modifiers: Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
            ..Default::default()
        });
        visual.run_until_parked();
        let button = visual
            .debug_bounds("audio-clip-looping-1")
            .unwrap()
            .center();
        let controls = visual.debug_bounds("arrangement-audio-controls").unwrap();
        assert!(
            controls.contains(&button),
            "loop button {button:?} outside controls {controls:?}"
        );
        button
    }

    #[cfg(feature = "ui-test-support")]
    fn placed_loop_control(cx: &mut gpui::TestAppContext, mode: AudioViewMode) {
        use gpui::{Modifiers, VisualTestContext};
        let (handle, editor, clip) = placed_loop_fixture(cx, mode);
        let mut visual = VisualTestContext::from_window(handle, cx);
        let button = visible_loop_button(&mut visual);
        let raw_looping = mode == AudioViewMode::Once;
        editor.read_with(&visual, |view, _| assert!(!view.edit_pending()));
        visual.simulate_click(button, Modifiers::default());
        editor.read_with(&visual, |view, _| {
            let state = view.state.as_ref().unwrap();
            assert_eq!(state.project.clip(clip).unwrap().looping(), raw_looping);
            assert_eq!(
                state
                    .project
                    .clip(clip)
                    .unwrap()
                    .audio_edit()
                    .unwrap()
                    .placement
                    .unwrap()
                    .mode,
                mode
            );
            assert!(
                matches!(view.last_enqueued_edit,
                    Some(Command::SetClipLooping { clip: id, looping })
                    if id == clip && looping == (mode == hex_project::AudioViewMode::Once)
                ),
                "loaded mode={mode:?}, retained source looping={raw_looping}, dispatched={:?}",
                view.last_enqueued_edit
            );
            // Apply the actual UI command through canonical history, without
            // claiming this injected-view fixture proves a worker acknowledgement.
            let mut model = hex_project::Editor::new((*state.project).clone(), 8).unwrap();
            let before = model.project().snapshot();
            model
                .execute(view.last_enqueued_edit.clone().unwrap())
                .unwrap();
            let current = model.project().clip(clip).unwrap();
            assert_eq!(current.looping(), raw_looping);
            assert_eq!(
                current.source_phase(),
                state.project.clip(clip).unwrap().source_phase()
            );
            assert_ne!(current.audio_edit().unwrap().placement.unwrap().mode, mode);
            assert!(model.undo());
            assert_eq!(model.project().snapshot(), before);
            assert!(model.redo());
            assert_ne!(
                model
                    .project()
                    .clip(clip)
                    .unwrap()
                    .audio_edit()
                    .unwrap()
                    .placement
                    .unwrap()
                    .mode,
                mode
            );
        });
    }

    #[cfg(feature = "ui-test-support")]
    #[gpui::test]
    fn review_placed_mode_only_change_invalidates_captured_control(cx: &mut gpui::TestAppContext) {
        use gpui::{Modifiers, VisualTestContext};
        use std::sync::Arc;
        let (handle, editor, _) = placed_loop_fixture(cx, AudioViewMode::Repeat);
        let mut visual = VisualTestContext::from_window(handle, cx);
        let button = visible_loop_button(&mut visual);
        let captured = editor.read_with(&visual, |view, _| {
            let state = view.state.as_ref().unwrap();
            let clip = state.project.clip(view.selected_clip.unwrap()).unwrap();
            let captured = ClipWindow {
                track: view.selected_track.unwrap(),
                clip: clip.id(),
                asset: clip.asset(),
                position: clip.start(),
                length: clip.duration(),
                source_start: clip.source_offset(),
                source_length: clip.source_length(),
                source_end: clip.source_offset() + clip.source_length(),
                source_phase: clip.source_phase(),
                source_looping: clip.looping(),
                placement_mode: clip
                    .audio_edit()
                    .and_then(|edit| edit.placement)
                    .map(|p| p.mode),
                gain: clip.gain(),
                asset_frames: state.project.asset(clip.asset()).unwrap().playback_frames(),
                sample_rate: state.project.sample_rate(),
                revision: state.revision,
                generation: view.project_generation,
            };
            assert!(clip_window_is_current(view, captured));
            captured
        });
        editor.update(&mut visual, |view, _| {
            let old = view.state.as_ref().unwrap();
            let mut snapshot = old.project.snapshot();
            snapshot.tracks[0].audio_clips_mut().unwrap()[0]
                .edit
                .as_mut()
                .unwrap()
                .placement
                .as_mut()
                .unwrap()
                .mode = AudioViewMode::Once;
            // Deliberately retain revision: exercise captured scalar freshness
            // independently of the ordinary revision gate.
            view.state = Some(Arc::new(hex_session::service::ViewState {
                canonical_audio_source: old.canonical_audio_source.clone(),
                comparison_audio: old.comparison_audio.clone(),
                project: Arc::new(hex_project::Project::from_snapshot(snapshot).unwrap()),
                waveforms: old.waveforms.clone(),
                audio_clip_waveforms: old.audio_clip_waveforms.clone(),
                midi_preview: old.midi_preview.clone(),
                gain_preview: old.gain_preview.clone(),
                timeline_frames: old.timeline_frames,
                can_undo: old.can_undo,
                can_redo: old.can_redo,
                revision: old.revision,
                unsaved: old.unsaved,
                document_path: old.document_path.clone(),
            }));
            assert!(!clip_window_is_current(view, captured));
            assert!(view.last_enqueued_edit.is_none());
            assert_eq!(view.worker_requests_pending, 0);
        });
        // A newly rendered control is valid and must use the new Once mode.
        visual.run_until_parked();
        visual.simulate_click(button, Modifiers::default());
        editor.read_with(&visual, |view, _| {
            assert!(matches!(
                view.last_enqueued_edit,
                Some(Command::SetClipLooping { looping: true, .. })
            ));
        });
    }

    #[test]
    fn review_play_start_uses_retained_source_looping_independent_of_placement_mode() {
        for mode in [AudioViewMode::Once, AudioViewMode::Repeat] {
            for raw in [false, true] {
                let mut captured = window();
                captured.source_looping = raw;
                captured.source_phase = if raw { 7 } else { 0 };
                captured.placement_mode = Some(mode);
                let command = command_for(captured, Field::PlayStart, 1, 1).unwrap();
                if raw {
                    assert!(matches!(
                        command,
                        Command::SetClipSourcePhase { phase: 8, .. }
                    ));
                } else {
                    assert!(matches!(
                        command,
                        Command::TrimClip {
                            source_offset: 21,
                            source_length: 79,
                            ..
                        }
                    ));
                }
                assert!(matches!(command_for(captured, Field::Looping, 1, 1),
                    Some(Command::SetClipLooping { looping, .. }) if looping == (mode == AudioViewMode::Once)));
                assert!(!captured.loop_tooltip().contains("reset"));
            }
        }
        assert!(window().loop_tooltip().contains("reset play start"));
        let mut ordinary = window();
        ordinary.source_looping = false;
        assert!(matches!(
            command_for(ordinary, Field::Looping, 1, 1),
            Some(Command::SetClipLooping { looping: true, .. })
        ));
    }

    #[cfg(feature = "ui-test-support")]
    #[gpui::test]
    fn review_placed_repeat_loop_control_dispatches_off(cx: &mut gpui::TestAppContext) {
        placed_loop_control(cx, hex_project::AudioViewMode::Repeat);
    }

    #[cfg(feature = "ui-test-support")]
    #[gpui::test]
    fn review_placed_once_loop_control_dispatches_on(cx: &mut gpui::TestAppContext) {
        placed_loop_control(cx, hex_project::AudioViewMode::Once);
    }

    #[test]
    fn gain_steps_use_db_units_and_refuse_the_bounds() {
        let mut captured = window();
        assert!(matches!(
            command_for(captured, Field::Gain, -1, 100),
            Some(Command::SetAudioClipGain { millidb: -100, .. })
        ));
        captured.gain = hex_project::ClipGain::new(24_000).unwrap();
        assert!(command_for(captured, Field::Gain, 1, 1_000).is_none());
        captured.gain = hex_project::ClipGain::new(-60_000).unwrap();
        assert!(command_for(captured, Field::Gain, -1, 100).is_none());
    }

    fn window() -> ClipWindow {
        use hex_project::{Created, Editor, Project};
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "audio".into(),
            })
            .expect("track")
        else {
            panic!("track")
        };
        let Created::Asset(asset) = editor
            .execute(Command::AddAsset {
                path: "sample.wav".into(),
                sample_rate: 48_000,
                frames: 1_000,
            })
            .expect("asset")
        else {
            panic!("asset")
        };
        let Created::Clip(clip) = editor
            .execute(Command::PlaceClip {
                track,
                asset,
                start: 100,
                duration: 500,
                source_offset: 20,
                source_length: 80,
                looping: true,
            })
            .expect("clip")
        else {
            panic!("clip")
        };
        ClipWindow {
            track,
            clip,
            asset,
            position: 100,
            length: 500,
            source_start: 20,
            source_length: 80,
            source_end: 100,
            source_phase: editor
                .project()
                .clip(clip)
                .expect("clip data")
                .source_phase(),
            source_looping: true,
            placement_mode: None,
            gain: hex_project::ClipGain::UNITY,
            asset_frames: 1_000,
            sample_rate: 48_000,
            revision: 7,
            generation: 4,
        }
    }

    #[test]
    fn controls_map_to_typed_clip_edits_and_loop_toggle() {
        let window = window();
        assert!(matches!(
            command_for(window, Field::Position, 1, 48),
            Some(Command::MoveClip { clip, track, start })
                if clip == window.clip && track == window.track && start == 148
        ));
        assert!(matches!(
            command_for(window, Field::Length, -1, 48),
            Some(Command::TrimClip { clip, start, duration, source_offset, source_length })
                if clip == window.clip && start == 100 && duration == 452
                    && source_offset == 20 && source_length == 80
        ));
        assert!(matches!(
            command_for(window, Field::SourceStart, 1, 48),
            Some(Command::TrimClip { clip, source_offset, source_length, .. })
                if clip == window.clip && source_offset == 68 && source_length == 32
        ));
        assert!(matches!(
            command_for(window, Field::SourceEnd, -1, 48),
            Some(Command::TrimClip { clip, source_offset, source_length, .. })
                if clip == window.clip && source_offset == 20 && source_length == 32
        ));
        assert!(matches!(
            command_for(window, Field::PlayStart, 1, 1),
            Some(Command::SetClipSourcePhase { clip, phase })
                if clip == window.clip && phase == 1
        ));
        assert!(matches!(
            command_for(window, Field::Looping, 1, 48),
            Some(Command::SetClipLooping { clip, looping: false }) if clip == window.clip
        ));
    }

    #[test]
    fn invalid_edges_and_overflow_refuse_without_partial_command() {
        let window = window();
        assert!(command_for(window, Field::Length, -1, 1_000).is_none());
        assert!(matches!(
            command_for(window, Field::SourceStart, 1, 100),
            Some(Command::TrimClip {
                source_offset: 99,
                source_length: 1,
                ..
            })
        ));
        assert!(command_for(window, Field::SourceEnd, 1, u64::MAX).is_none());
        assert!(command_for(window, Field::Position, 1, u64::MAX).is_none());
        assert!(command_for(window, Field::PlayStart, 1, 80).is_none());
    }
}
