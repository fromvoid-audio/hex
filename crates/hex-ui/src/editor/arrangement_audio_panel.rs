//! Arrangement audio Clip View with canonical source and timeline controls.
use super::EditorView;
use crate::theme;
use gpui::{Context, Div, ElementId, Stateful, div, prelude::*, px, relative, rgb};
use hex_project::{AssetId, ClipId, Command, TrackId};
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
    looping: bool,
    gain: hex_project::ClipGain,
    asset_frames: u64,
    sample_rate: u32,
    revision: u64,
    generation: u64,
}

pub(super) fn view(view: &mut EditorView, cx: &mut Context<EditorView>) -> Option<Div> {
    if view.mode != super::EditorMode::Arrangement {
        return None;
    }
    let state = view.state.as_ref()?;
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
        looping: clip.looping(),
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
                if window.looping { " · loop on" } else { "" }
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
        Field::Looping => "toggle source looping",
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
        Field::Looping => "toggle source looping",
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
    let tooltip = if window.looping {
        "disable source looping · reset play start to source start"
    } else {
        "enable source looping"
    };
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
            if window.looping { "↻" } else { "○" },
            tooltip,
            pending,
            cx,
        ))
        .child(
            div()
                .text_xs()
                .text_color(rgb(theme::color(theme::PaletteRole::Text)))
                .child(theme::text(if window.looping { "on" } else { "off" })),
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
        Field::PlayStart if window.looping => shifted(window.source_phase)
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
            looping: !window.looping,
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
                    && clip.looping() == captured.looping
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
            looping: true,
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
