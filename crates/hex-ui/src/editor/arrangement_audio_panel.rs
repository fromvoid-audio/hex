//! Arrangement audio Clip View with canonical source and timeline controls.
use super::{EditorView, viewport::Viewport};
use crate::theme;
use gpui::{Context, Div, div, prelude::*, px, relative, rgb};
use hex_project::{AssetId, AudioViewMode, ClipId, Command, TrackId};
use std::path::Path;
pub(crate) mod controls;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Field {
    Position,
    Length,
    SourceStart,
    SourceEnd,
    PlayStart,
    Looping,
    Gain,
    Pitch,
}

#[derive(Clone, Copy, PartialEq, Eq)]
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
    pitch_cents: i16,
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

pub(super) fn view(
    view: &mut EditorView,
    render_window: &gpui::Window,
    cx: &mut Context<EditorView>,
) -> Option<Div> {
    if view.mode != super::EditorMode::Arrangement {
        return None;
    }
    let state = view.state.as_ref()?.clone();
    let (track, clip_id, position, length, source_start, source_length) =
        view.detail_audio_clip_data()?;
    let clip = state.project.clip(clip_id)?;
    let retained_source_count = clip
        .audio_edit()
        .and_then(|edit| edit.domain.events.as_ref())
        .map_or(0, |events| events.sources.len());
    let grouped_audio = retained_source_count > 0;
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
        pitch_cents: clip.audio_edit().map_or(0, |e| e.domain.pitch_cents),
        asset_frames: asset.playback_frames(),
        sample_rate: state.project.sample_rate(),
        revision: state.revision,
        generation: view.project_generation,
    };
    let waveform = (!grouped_audio)
        .then(|| state.waveforms.get(&window.asset).cloned())
        .flatten();
    let title = if grouped_audio {
        format!("grouped audio · {retained_source_count} sources")
    } else {
        lowercase_path_tail(asset.path())
    };
    let mut details = div()
        .h(px(view.clip_detail_body_height))
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
    controls = controls.child(controls::toggle(view, window, true, cx));
    if !grouped_audio {
        controls = controls.child(super::musical_audio_controls::warp_header(
            view, clip_id, cx,
        ));
        let pitch = controls::number(view, window, Field::Pitch, "pitch", cx);
        let gain = controls::number(view, window, Field::Gain, "gain", cx);
        controls = controls.child(
            div()
                .flex()
                .items_center()
                .gap(px(4.0))
                .flex_shrink_0()
                .children([pitch, gain.flex_1().min_w_0()]),
        );
    }
    for (field, label) in [
        (Field::Position, "pos"),
        (Field::Length, "len"),
        (Field::SourceStart, "src in"),
        (Field::PlayStart, "play at"),
        (Field::SourceEnd, "src end"),
        (Field::Gain, "gain"),
    ] {
        if field == Field::Gain && !grouped_audio {
            continue; // Already beside the pitch dial; grouped sources retain their gain row.
        }
        if !grouped_audio || matches!(field, Field::Position | Field::Length | Field::Gain) {
            controls = controls.child(controls::number(view, window, field, label, cx));
        }
    }
    if !grouped_audio {
        controls = controls.child(controls::toggle(view, window, false, cx));
        controls = controls.child(super::musical_audio_controls::view(view, clip_id, cx));
    }
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
    view.sync_audio_navigation();
    if grouped_audio && view.audio_event_editor.source_view {
        view.audio_event_editor.source_view = false;
    }
    let source_view = view.audio_event_editor.source_view;
    let control = if grouped_audio {
        div()
            .id("audio-source-tab")
            .w(px(56.0))
            .h(px(20.0))
            .text_xs()
            .text_color(rgb(theme::color(theme::PaletteRole::SecondaryText)))
            .tooltip(|_, cx| {
                cx.new(|_| super::waveform::ClipTooltip {
                    kind: crate::tooltips::Kind::Identification,
                    name: "source view unavailable for grouped audio".into(),
                })
                .into()
            })
            .child(theme::text("n/a"))
            .debug_selector(|| "audio-source-tab-unavailable".into())
    } else {
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
            view.audio_event_focus.focus(window);
            cx.notify();
        }))
    };
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
            .child(control)
            .children(source_view.then(|| source_navigation(view, cx))),
    );
    if !source_view {
        waveform_panel =
            waveform_panel.child(super::audio_event_editor::view(view, render_window, cx)?);
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
    let viewport = view.audio_source_viewport();
    waveform_panel = waveform_panel.child(time_ruler(window, viewport));
    let mut canvas_region = div()
        .id("arrangement-audio-waveform")
        .on_scroll_wheel(cx.listener(|view, event, window, cx| {
            if view.audio_navigation_wheel(event, true, window, cx) {
                cx.stop_propagation();
                cx.notify();
            }
        }))
        .relative()
        .flex_1()
        .min_h_0()
        .w_full()
        .overflow_hidden()
        .bg(rgb(theme::color(theme::PaletteRole::Surface)))
        .border_1()
        .border_color(rgb(theme::color(theme::PaletteRole::Border)))
        .debug_selector(|| "arrangement-audio-waveform".into())
        .track_focus(&view.audio_event_focus)
        .on_action(cx.listener(|view, _: &super::ZoomIn, _, cx| {
            view.event_zoom(true);
            cx.stop_propagation();
            cx.notify();
        }))
        .on_action(cx.listener(|view, _: &super::ZoomOut, _, cx| {
            view.event_zoom(false);
            cx.stop_propagation();
            cx.notify();
        }))
        .on_action(cx.listener(|view, _: &super::PanTimelineLeft, _, cx| {
            view.event_pan(false);
            cx.stop_propagation();
            cx.notify();
        }))
        .on_action(cx.listener(|view, _: &super::PanTimelineRight, _, cx| {
            view.event_pan(true);
            cx.stop_propagation();
            cx.notify();
        }))
        .on_action(cx.listener(|view, _: &super::FitTimeline, _, cx| {
            view.audio_fit_all();
            cx.stop_propagation();
            cx.notify();
        }))
        .on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(|view, _, window, _| view.audio_event_focus.focus(window)),
        );
    let measure = cx.entity().clone();
    canvas_region = canvas_region.children([gpui::canvas(
        |_, _, _| (),
        move |bounds, _, _, cx| {
            measure.update(cx, |view, _| {
                view.audio_event_editor.source_bounds = Some(bounds)
            });
        },
    )
    .absolute()
    .inset_0()]);
    if let Some(waveform) = waveform {
        let end = viewport
            .end()
            .unwrap_or(viewport.start)
            .min(window.asset_frames);
        if end > viewport.start && viewport.length > 0 {
            let visible = super::waveform::waveform_source_segment_gain_view_cached(
                waveform,
                0,
                viewport.start,
                end,
                window.gain,
                false,
                std::rc::Rc::clone(&view.waveform_geometry_cache),
            );
            canvas_region = canvas_region.child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(relative(
                        (end - viewport.start) as f32 / viewport.length as f32,
                    ))
                    .overflow_hidden()
                    .debug_selector(|| "audio-source-visible-waveform".into())
                    .children([visible]),
            );
        }
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
            .children(source_marker(
                window.source_start,
                viewport,
                "source-start-marker",
                theme::color(theme::PaletteRole::Primary),
            ))
            .children(source_marker(
                window.source_end,
                viewport,
                "source-end-marker",
                theme::color(theme::PaletteRole::Primary),
            ))
            .children(source_marker(
                window.source_start.saturating_add(window.source_phase),
                viewport,
                "play-start-marker",
                theme::color(theme::PaletteRole::Secondary),
            ));
    }
    if let Some(edit) = clip.audio_edit() {
        if let Some(warp) = edit.domain.warp.as_ref() {
            for marker in &warp.markers {
                canvas_region = canvas_region.children(source_marker(
                    marker.source_frame,
                    viewport,
                    "warp-source-marker",
                    theme::color(theme::PaletteRole::Secondary),
                ));
            }
        }
        for point in &edit.domain.source_gain {
            canvas_region = canvas_region.children(source_marker(
                point.frame,
                viewport,
                "source-gain-point",
                theme::color(theme::PaletteRole::Primary),
            ));
        }
    }
    if let Some(position) = view
        .audio_playhead_media_frame()
        .filter(|frame| *frame < window.asset_frames)
    {
        canvas_region = canvas_region.children(source_marker(
            position,
            viewport,
            "audio-source-playback-position",
            theme::color(theme::PaletteRole::Text),
        ));
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

fn frame_step(sample_rate: u32, fine: bool) -> u64 {
    if fine {
        1
    } else {
        u64::from((sample_rate / 1_000).max(1))
    }
}

#[cfg(test)]
fn selection_command_for(
    window: ClipWindow,
    field: Field,
    direction: i8,
    step: u64,
    multiple: bool,
) -> Option<Command> {
    if field == Field::Gain && multiple {
        let millidb = i32::try_from(step)
            .ok()?
            .checked_mul(i32::from(direction))?;
        Some(Command::EditClips {
            clips: vec![window.clip],
            edit: hex_project::ClipEdit::GainBy { millidb },
        })
    } else {
        command_for(window, field, direction, step)
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
            .and_then(|duration| {
                if window.placement_mode.is_some() {
                    Some(Command::ResizeAudioClipView {
                        clip: window.clip,
                        operation: hex_project::AudioViewResizeOperation::OuterEnd {
                            project_frame: window.position.checked_add(duration)?,
                        },
                    })
                } else {
                    Some(Command::TrimClip {
                        clip: window.clip,
                        start: window.position,
                        duration,
                        source_offset: window.source_start,
                        source_length: window.source_length,
                    })
                }
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
        Field::Pitch => None, // Signed pitch uses its musical-command owner.
        Field::Looping => Some(Command::SetClipLooping {
            clip: window.clip,
            looping: !window.looping(),
        }),
    }
}

fn clip_window_is_current(view: &EditorView, captured: ClipWindow) -> bool {
    if view.mode != super::EditorMode::Arrangement
        || view.project_generation != captured.generation
        || view
            .detail_audio_clip_data()
            .is_none_or(|(track, clip, ..)| track != captured.track || clip != captured.clip)
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
                    && clip.audio_edit().map_or(0, |e| e.domain.pitch_cents) == captured.pitch_cents
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

fn time_ruler(window: ClipWindow, viewport: Viewport) -> Div {
    let mut ruler = super::timeline_strip::ruler_surface()
        .w_full()
        .debug_selector(|| "arrangement-audio-time-ruler".into());
    if viewport.length == 0 {
        return ruler;
    }
    for index in 0..=4_u64 {
        let frame = viewport
            .start
            .saturating_add((u128::from(viewport.length) * u128::from(index) / 4) as u64);
        let x = index as f32 / 4.0;
        ruler = ruler.children(super::timeline_strip::ruler_tick(
            x,
            format_frame_time(frame, window.sample_rate),
            index == 0,
            false,
            false,
        ));
    }
    ruler
}

fn source_marker(frame: u64, viewport: Viewport, id: &'static str, color: u32) -> Option<Div> {
    let fraction = viewport.fraction(frame)?;
    let marker = div().debug_selector(move || id.into()).absolute();
    let marker = if fraction == 1.0 {
        marker.right_0()
    } else {
        marker.left(relative(fraction))
    };
    Some(marker.top_0().bottom_0().w(px(1.0)).bg(rgb(color)))
}

fn source_navigation(view: &EditorView, cx: &mut Context<EditorView>) -> Div {
    let mut toolbar = div()
        .h(px(20.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(4.0));
    for (label, id, inward) in [
        ("+", "audio-source-zoom-in", true),
        ("−", "audio-source-zoom-out", false),
    ] {
        toolbar = toolbar.child(
            super::waveform::button(label, "zoom original audio source", (id, 0)).on_click(
                cx.listener(move |view, _, window, cx| {
                    view.audio_event_focus.focus(window);
                    view.event_zoom(inward);
                    cx.notify();
                }),
            ),
        );
    }
    toolbar
        .child(view.zoom_fit_button(super::zoom_fit::Target::Audio, false, cx))
        .child(view.zoom_fit_button(super::zoom_fit::Target::Audio, true, cx))
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
            .debug_bounds("audio-clip-loop-toggle")
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
                pitch_cents: clip.audio_edit().map_or(0, |e| e.domain.pitch_cents),
                asset_frames: state.project.asset(clip.asset()).unwrap().playback_frames(),
                sample_rate: state.project.sample_rate(),
                revision: state.revision,
                generation: view.project_generation,
            };
            assert!(clip_window_is_current(view, captured));
            captured
        });
        editor.update(&mut visual, |view, cx| {
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
                source_overviews: old.source_overviews.clone(),
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
            cx.notify(); // Model mutation above stands in for a received state.
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
        for (direction, step, delta) in [(-1, 100, -100), (1, 1_000, 1_000)] {
            assert!(
                matches!(selection_command_for(captured, Field::Gain, direction, step, true),
                Some(Command::EditClips { clips, edit: hex_project::ClipEdit::GainBy { millidb } })
                if clips == vec![captured.clip] && millidb == delta)
            );
        }
        captured.gain = hex_project::ClipGain::new(24_000).unwrap();
        assert!(command_for(captured, Field::Gain, 1, 1_000).is_none());
        captured.gain = hex_project::ClipGain::new(-60_000).unwrap();
        assert!(command_for(captured, Field::Gain, -1, 100).is_none());
    }

    pub(super) fn window() -> ClipWindow {
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
            pitch_cents: 0,
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

    #[cfg(feature = "ui-test-support")]
    mod length_worker_returns {
        use super::*;
        use gpui::{AppContext, Modifiers, TestAppContext, VisualTestContext};
        use hex_project::{AudioViewResizeOperation, Editor};
        use hex_session::service::{Event, Request, Service};
        use std::time::{Duration, Instant};

        fn next(service: &Service) -> Event {
            let deadline = crate::test_wait::hang_deadline();
            loop {
                if let Ok(event) = service.try_event() {
                    assert!(event.error.is_none(), "{:?}", event.error);
                    return event;
                }
                assert!(
                    Instant::now() < deadline,
                    "actual service response timed out"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
        }

        fn pump(editor: &gpui::Entity<EditorView>, visual: &mut VisualTestContext) {
            let deadline = crate::test_wait::hang_deadline();
            loop {
                let done = editor.update(&mut *visual, |v, cx| {
                    v.tick(cx);
                    cx.notify();
                    v.worker_requests_pending == 0 && v.pending_requests.is_empty()
                });
                if done {
                    visual.run_until_parked();
                    return;
                }
                assert!(Instant::now() < deadline, "actual clip edit timed out");
                std::thread::sleep(Duration::from_millis(1));
            }
        }

        fn fixture(
            cx: &mut TestAppContext,
            mode: Option<AudioViewMode>,
            phase: u64,
            length: u64,
        ) -> (
            gpui::AnyWindowHandle,
            gpui::Entity<EditorView>,
            ClipId,
            super::super::super::external_audio_drop_ui_test::BatchFiles,
        ) {
            let files = super::super::super::external_audio_drop_ui_test::BatchFiles::new();
            let wav = files.wav("placed-length.wav", 48_000, 96_000);
            let root = wav.parent().unwrap();
            std::fs::create_dir(root.join("samples")).unwrap();
            let service = Service::new(48_000, 32, 8_000_000).unwrap();
            next(&service);
            service.submit(Request::Import(wav.clone())).unwrap();
            let imported = next(&service);
            let track = imported.state.project.tracks()[0].id();
            let clip = imported.state.project.track(track).unwrap().clips()[0].id();
            let edit = |command| {
                service.submit(Request::Edit(command)).unwrap();
                next(&service)
            };
            edit(Command::TrimClip {
                clip,
                start: 0,
                duration: 24_000,
                source_offset: 12_000,
                source_length: 24_000,
            });
            edit(Command::SetClipLooping {
                clip,
                looping: true,
            });
            edit(Command::SetAudioClipPitch { clip, cents: 123 });
            let a = hex_engine::default_native_device_state(
                hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
            )
            .unwrap()
            .with_input_gain_millidb(-1234)
            .unwrap()
            .with_output_gain_millidb(4321)
            .unwrap();
            let view = hex_engine::native_device_view(&a).unwrap();
            let parameter = view
                .parameters
                .iter()
                .find(|p| matches!(p.value, hex_lattice_contract::ParameterValue::Bool(_)))
                .unwrap();
            let hex_lattice_contract::ParameterValue::Bool(value) = parameter.value else {
                unreachable!()
            };
            let b = hex_engine::patch_native_device_parameter(
                &a,
                parameter.descriptor.id,
                hex_lattice_contract::ParameterValue::Bool(!value),
            )
            .unwrap();
            let inserted = edit(Command::InsertDevice {
                track,
                before: None,
                state: a.clone(),
            });
            let device = inserted.state.project.track(track).unwrap().devices()[0].id();
            edit(Command::ReplaceDeviceComparison {
                device,
                state: a,
                comparison: Some(hex_project::DeviceComparison::new(
                    hex_project::DeviceVariant::B,
                    b,
                )),
            });
            let scale = hex_tuning::Scale::new(
                7,
                *hex_tuning::Scale::default().cents(),
                57,
                432.0,
                3,
                1300.0,
            )
            .unwrap();
            edit(Command::SetTuning(
                hex_tuning::ProjectTuning::new(true, scale, hex_tuning::Audition::default())
                    .unwrap(),
            ));
            if let Some(mode) = mode {
                edit(Command::ResizeAudioClipView {
                    clip,
                    operation: AudioViewResizeOperation::OuterEnd {
                        project_frame: 48_000,
                    },
                });
                if mode == AudioViewMode::Once {
                    edit(Command::SetClipLooping {
                        clip,
                        looping: false,
                    });
                }
                if phase > 0 {
                    edit(Command::TrimClipEdge {
                        clip,
                        edge: hex_project::TrimEdge::Left,
                        frame: phase,
                    });
                    edit(Command::MoveClip {
                        clip,
                        track,
                        start: 0,
                    });
                }
                let placed = edit(Command::ResizeAudioClipView {
                    clip,
                    operation: AudioViewResizeOperation::OuterEnd {
                        project_frame: length,
                    },
                });
                let placement = placed
                    .state
                    .project
                    .clip(clip)
                    .unwrap()
                    .audio_edit()
                    .unwrap()
                    .placement
                    .unwrap();
                assert_eq!(placement.mode, mode);
                assert_eq!(placement.cycle_phase, phase);
            }
            edit(Command::SetClipFades {
                clip,
                fade_in: length.min(100),
                fade_out: length.min(200),
            });
            let path = root.join("placed.hex");
            service.submit(Request::SaveNew(path.clone())).unwrap();
            let saved = next(&service);
            assert!(saved.save_complete);
            service.submit(Request::Open(path)).unwrap();
            let event = next(&service);
            assert!(event.project_replaced && event.open_complete);
            assert_eq!(
                event.state.project, saved.state.project,
                "actual current-format SaveNew/Open"
            );
            let handle = *cx.add_window(|_, _| gpui::Empty);
            let editor = cx
                .update_window(handle, |_, window, cx| {
                    window.activate_window();
                    let editor = window.replace_root(cx, |window, cx| {
                        EditorView::new_for_test_with_browser_directory(window, cx, service, root)
                    });
                    editor.update(cx, |v, cx| {
                        v.receive(event);
                        v.selected_track = Some(track);
                        v.selected_clip = Some(clip);
                        v.clip_view_open = true;
                        v.device_panel_active = false;
                        v.focus.focus(window);
                        cx.notify();
                    });
                    editor
                })
                .unwrap();
            (handle, editor, clip, files)
        }

        fn length_steps(cx: &mut TestAppContext, mode: Option<AudioViewMode>, phase: u64) {
            cx.update(super::super::super::bind_editor_keys);
            for direction in [-1, 1] {
                let (handle, editor, clip, _files) = fixture(cx, mode, phase, 48_000);
                let mut visual = VisualTestContext::from_window(handle, cx);
                visual.run_until_parked();
                let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().clone());
                let old = before.project.clip(clip).unwrap();
                let duration = if direction < 0 {
                    old.duration() - 48
                } else {
                    old.duration() + 48
                };
                let expected = if mode.is_some() {
                    Command::ResizeAudioClipView {
                        clip,
                        operation: AudioViewResizeOperation::OuterEnd {
                            project_frame: old.start() + duration,
                        },
                    }
                } else {
                    Command::TrimClip {
                        clip,
                        start: old.start(),
                        duration,
                        source_offset: old.source_offset(),
                        source_length: old.source_length(),
                    }
                };
                let mut oracle = Editor::new((*before.project).clone(), 16).unwrap();
                oracle.execute(expected).unwrap();
                let button = visual.debug_bounds("audio-clip-length").unwrap().center();
                let controls = visual.debug_bounds("arrangement-audio-controls").unwrap();
                assert!(controls.contains(&button));
                visual.simulate_click(button, Modifiers::default());
                visual.simulate_keystrokes(if direction < 0 {
                    "down enter"
                } else {
                    "up enter"
                });
                pump(&editor, &mut visual);
                let after = editor.read_with(&visual, |v, _| {
                    assert!(v.error.is_none(), "{:?}", v.error);
                    assert_eq!(v.state.as_ref().unwrap().project.clip(clip).unwrap().audio_edit().unwrap().placement.map(|p| p.mode), mode,
                        "actual Length click lost placement: mode={mode:?} phase={phase} direction={direction}");
                    assert_eq!(*v.state.as_ref().unwrap().project, *oracle.project(), "complete canonical metadata including source/fade/tuning/A-B");
                    v.state.as_ref().unwrap().clone()
                });
                editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
                pump(&editor, &mut visual);
                editor.read_with(&visual, |v, _| {
                    assert_eq!(v.state.as_ref().unwrap().project, before.project)
                });
                editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
                pump(&editor, &mut visual);
                editor.read_with(&visual, |v, _| {
                    assert_eq!(v.state.as_ref().unwrap().project, after.project)
                });
            }
        }

        #[gpui::test]
        fn placed_length_minimum_refuses_without_worker_or_history(cx: &mut TestAppContext) {
            for mode in [AudioViewMode::Once, AudioViewMode::Repeat] {
                let (handle, editor, _clip, _files) = fixture(cx, Some(mode), 6_000, 1);
                let mut visual = VisualTestContext::from_window(handle, cx);
                visual.run_until_parked();
                let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().clone());
                let minus = visual.debug_bounds("audio-clip-length").unwrap().center();
                visual.simulate_click(minus, Modifiers::default());
                visual.simulate_keystrokes("down");
                editor.read_with(&visual, |v, _| {
                    assert_eq!(v.worker_requests_pending, 0);
                    assert!(v.pending_requests.is_empty());
                    assert!(v.last_enqueued_edit.is_none());
                    let state = v.state.as_ref().unwrap();
                    assert_eq!(state.project, before.project);
                    assert_eq!(
                        (
                            state.revision,
                            state.can_undo,
                            state.can_redo,
                            state.unsaved
                        ),
                        (
                            before.revision,
                            before.can_undo,
                            before.can_redo,
                            before.unsaved
                        )
                    );
                });
            }
        }

        #[gpui::test]
        fn placed_length_once_zero_phase_real_worker(cx: &mut TestAppContext) {
            length_steps(cx, Some(AudioViewMode::Once), 0);
        }
        #[gpui::test]
        fn placed_length_once_nonzero_phase_real_worker(cx: &mut TestAppContext) {
            length_steps(cx, Some(AudioViewMode::Once), 6_000);
        }
        #[gpui::test]
        fn placed_length_repeat_zero_phase_real_worker(cx: &mut TestAppContext) {
            length_steps(cx, Some(AudioViewMode::Repeat), 0);
        }
        #[gpui::test]
        fn placed_length_repeat_nonzero_phase_real_worker(cx: &mut TestAppContext) {
            length_steps(cx, Some(AudioViewMode::Repeat), 6_000);
        }
        #[gpui::test]
        fn placed_length_ordinary_trim_real_worker_control(cx: &mut TestAppContext) {
            length_steps(cx, None, 0);
        }
    }
}
