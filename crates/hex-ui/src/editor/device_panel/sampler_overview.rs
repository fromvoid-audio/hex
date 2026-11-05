//! Peak-only original-source overview and static edit cursor. No PCM, file I/O,
//! playback readhead or canonical edits. Cursor positions are nearest original
//! frame boundaries, including the end boundary at `source_frames`.
use super::*;
use gpui::{MouseDownEvent, canvas, relative};
use std::{cell::Cell, rc::Rc};

const RULER_HEIGHT: f32 = 14.0;
#[path = "sampler_boundary.rs"]
mod boundary;
pub(in crate::editor) use boundary::{
    cancel as cancel_sampler_boundary, preview as sampler_boundary_preview,
};

#[derive(Clone, Copy, PartialEq, Eq)]
struct SourceTarget {
    generation: u64,
    track: TrackId,
    device: DeviceId,
    asset: hex_project::AssetId,
    content: hex_project::OriginalSourceFacts,
    frames: u64,
    rate: u32,
}

impl SourceTarget {
    fn resolve(view: &EditorView, track: TrackId, device: DeviceId) -> Option<Self> {
        if view.selected_track != Some(track)
            || view.device_panel.owner != Some(DevicePanelOwner::Track(track))
        {
            return None;
        }
        let state = view.state.as_ref()?;
        let instrument = state.project.track(track)?.instrument()?;
        if instrument.id() != device
            || instrument.state().type_id().as_str().ok() != Some("hex.instrument.sampler")
        {
            return None;
        }
        let references = instrument.state().asset_references();
        if references.len() != 1 {
            return None;
        }
        let asset = state
            .project
            .assets()
            .find(|a| a.id().value() == references[0].asset.value())?;
        let overview = view.sampler_source_overview(asset)?;
        (overview.source_frames() > 0 && overview.sample_rate() > 0).then_some(Self {
            generation: view.project_generation,
            track,
            device,
            asset: asset.id(),
            content: overview.content(),
            frames: overview.source_frames(),
            rate: overview.sample_rate(),
        })
    }

    fn current(self, view: &EditorView) -> bool {
        Self::resolve(view, self.track, self.device) == Some(self)
    }
}

#[derive(Default)]
pub(super) struct CursorState {
    cursor: Option<(SourceTarget, u64)>,
    boundary: Option<boundary::Gesture>,
}

pub(super) fn reconcile_cursor(view: &mut EditorView) {
    boundary::reconcile(view);
    if view
        .device_panel
        .sampler_cursor
        .cursor
        .is_some_and(|(target, _)| !target.current(view))
    {
        view.device_panel.sampler_cursor.cursor = None;
    }
}

fn frame_at_x(x: f32, width: f32, frames: u64) -> Option<u64> {
    if !x.is_finite() || !width.is_finite() || width <= 0.0 || x < 0.0 || x > width || frames == 0 {
        return None;
    }
    Some(((f64::from(x) / f64::from(width) * frames as f64).round() as u64).min(frames))
}

pub(super) fn render(
    view: &EditorView,
    track: TrackId,
    device: &hex_project::Device,
    name: &str,
    cx: &mut Context<EditorView>,
) -> Div {
    let shell = div()
        .w_full()
        .min_w_0()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .overflow_hidden()
        .debug_selector(|| "sampler-source-overview".into());
    let Some(state) = view.state.as_ref() else {
        return shell.child(theme::text("source overview unavailable"));
    };
    let Some(reference) = device
        .state()
        .asset_references()
        .first()
        .filter(|_| device.state().asset_references().len() == 1)
    else {
        return shell.child(theme::text("source overview unavailable"));
    };
    let Some(asset) = state
        .project
        .assets()
        .find(|a| a.id().value() == reference.asset.value())
    else {
        return shell.child(theme::text("source overview unavailable"));
    };
    let Some(overview) = view.sampler_source_overview(asset) else {
        return shell.child(theme::text("source overview unavailable or mismatched"));
    };
    let frames = overview.source_frames();
    if frames == 0 {
        return shell.child(theme::text("source overview extent unavailable"));
    }
    let parameter = |p| view.sampler_display_value(track, device, p, cx).ok();
    let Some(start) = parameter(hex_engine::SamplerParameter::StartFrame) else {
        return shell.child(theme::text("source region unavailable"));
    };
    let Some(end) = parameter(hex_engine::SamplerParameter::EndFrame) else {
        return shell.child(theme::text("source region unavailable"));
    };
    if !(start >= 0.0 && start < end && end <= frames as f64) {
        return shell.child(theme::text("source region exceeds overview"));
    }
    let facts = format!(
        "{frames} frames · {} hz · {:.3} s",
        overview.sample_rate(),
        frames as f64 / f64::from(overview.sample_rate())
    );
    let table = state.source_overviews.clone();
    let asset_id = asset.id();
    let target = (view.selected_track == Some(track)
        && view.device_panel.owner == Some(DevicePanelOwner::Track(track)))
    .then_some(SourceTarget {
        generation: view.project_generation,
        track,
        device: device.id(),
        asset: asset_id,
        content: overview.content(),
        frames,
        rate: overview.sample_rate(),
    });
    let cursor = view
        .device_panel
        .sampler_cursor
        .cursor
        .filter(|(key, frame)| Some(*key) == target && *frame <= frames)
        .map(|(_, frame)| frame);
    let hit_bounds = Rc::new(Cell::new(None::<Bounds<Pixels>>));
    let measured_bounds = Rc::clone(&hit_bounds);
    let pointer_entity = cx.entity();
    let waveform = div()
        .relative()
        .w_full()
        .flex_1()
        .min_h_0()
        .bg(rgb(theme::color(theme::PaletteRole::WaveBackground)))
        .when(target.is_some(), |waveform| waveform.cursor_crosshair())
        .debug_selector(|| "sampler-source-waveform".into())
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                let Some(target) = target.filter(|target| target.current(view)) else {
                    return;
                };
                let Some(bounds) = hit_bounds.get() else {
                    return;
                };
                if !bounds.contains(&event.position) {
                    return;
                }
                if boundary::begin(view, target, bounds, event.position, window, cx) {
                    view.focus.focus(window);
                    window.prevent_default();
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                let Some(frame) = frame_at_x(
                    f32::from(event.position.x - bounds.left()),
                    f32::from(bounds.size.width),
                    target.frames,
                ) else {
                    return;
                };
                view.device_panel.sampler_cursor.cursor = Some((target, frame));
                view.focus.focus(window);
                window.prevent_default();
                cx.stop_propagation();
                cx.notify();
            }),
        )
        .child(
            div()
                .absolute()
                .left(relative((start / frames as f64) as f32))
                .w(relative(((end - start) / frames as f64) as f32))
                .top_0()
                .bottom(px(RULER_HEIGHT))
                .bg(rgb(theme::color(theme::PaletteRole::Surface)))
                .debug_selector(|| "sampler-source-active-region".into()),
        )
        .child(
            super::super::overview_waveform::render(
                super::super::overview_waveform::Source::Sampler {
                    table,
                    asset: asset_id,
                },
                Rc::clone(&view.overview_waveform_cache),
                Some((start / frames as f64, end / frames as f64)),
            )
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bottom(px(RULER_HEIGHT)),
        )
        .child(source_marker(
            "sampler-source-start",
            start / frames as f64,
            theme::PaletteRole::Primary,
        ))
        .child(source_marker(
            "sampler-source-end",
            end / frames as f64,
            theme::PaletteRole::Primary,
        ))
        .child(boundary::handle(
            true,
            start / frames as f64,
            !view.edit_pending(),
        ))
        .child(boundary::handle(
            false,
            end / frames as f64,
            !view.edit_pending(),
        ));
    let waveform = if let (Some(enabled), Some(loop_start), Some(loop_end)) = (
        parameter(hex_engine::SamplerParameter::LoopEnabled),
        parameter(hex_engine::SamplerParameter::LoopStartFrame),
        parameter(hex_engine::SamplerParameter::LoopEndFrame),
    ) {
        if loop_start >= 0.0 && loop_start < loop_end && loop_end <= frames as f64 {
            let color = if enabled == 1.0 {
                theme::PaletteRole::Secondary
            } else {
                theme::PaletteRole::Muted
            };
            waveform
                .child(source_marker(
                    "sampler-source-loop-start",
                    loop_start / frames as f64,
                    color,
                ))
                .child(source_marker(
                    "sampler-source-loop-end",
                    loop_end / frames as f64,
                    color,
                ))
        } else {
            waveform
        }
    } else {
        waveform
    };
    let waveform = waveform
        .when_some(cursor, |waveform, frame| {
            waveform.child(source_marker(
                "sampler-source-cursor",
                frame as f64 / frames as f64,
                theme::PaletteRole::Text,
            ))
        })
        .child(
            canvas(
                move |bounds, _, _| {
                    measured_bounds.set(Some(bounds));
                },
                move |bounds, _, window, cx| {
                    boundary::renew(
                        pointer_entity.read(cx),
                        bounds,
                        pointer_entity.entity_id(),
                        window,
                    );
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bottom(px(RULER_HEIGHT)),
        )
        .child(
            div()
                .absolute()
                .bottom_0()
                .left_0()
                .right_0()
                .h(px(RULER_HEIGHT))
                .flex()
                .items_center()
                .gap(px(8.0))
                .px(px(3.0))
                .text_size(px(9.0))
                .line_height(px(RULER_HEIGHT))
                .bg(rgb(theme::color(theme::PaletteRole::WaveBackground)))
                .child(theme::text("0 s"))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_ellipsis()
                        .child(theme::text(name)),
                )
                .child(
                    div()
                        .debug_selector(|| "sampler-source-cursor-readout".into())
                        .child(theme::text(cursor.map_or(facts, |frame| {
                            cursor_label(frame, overview.sample_rate())
                        }))),
                )
                .child(boundary::cursor_action(
                    view,
                    hex_engine::SamplerParameter::StartFrame,
                    cx,
                ))
                .child(boundary::cursor_action(
                    view,
                    hex_engine::SamplerParameter::EndFrame,
                    cx,
                )),
        );
    shell.child(waveform)
}

fn cursor_label(frame: u64, sample_rate: u32) -> String {
    format!(
        "cursor · {frame} frames · {:.3} s",
        frame as f64 / f64::from(sample_rate)
    )
}
fn source_marker(selector: &'static str, fraction: f64, color: theme::PaletteRole) -> Div {
    div()
        .absolute()
        .left(relative(fraction.clamp(0.0, 1.0) as f32))
        .ml(px(-fraction.clamp(0.0, 1.0) as f32))
        .top_0()
        .w(px(1.0))
        .bottom(px(RULER_HEIGHT))
        .bg(rgb(theme::color(color)))
        .debug_selector(move || selector.into())
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "sampler_cursor_tests.rs"]
mod tests;
