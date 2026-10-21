//! Read-only, peak-only original-source overview. No PCM or filesystem access.
use super::*;
use gpui::{canvas, fill, point, size};

const WIDTH: f32 = 180.0;
const WAVE_HEIGHT: f32 = 96.0;

pub(super) fn render(view: &EditorView, device: &hex_project::Device, name: &str) -> Div {
    let shell = div()
        .w(px(WIDTH))
        .flex_shrink_0()
        .h_full()
        .min_h_0()
        .flex()
        .flex_col()
        .gap(px(4.0))
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
    let parameter = |p| hex_engine::sampler_parameter_value_on_control(p, device.state()).ok();
    let Some(start) = parameter(hex_engine::SamplerParameter::StartFrame) else {
        return shell.child(theme::text("source region unavailable"));
    };
    let Some(end) = parameter(hex_engine::SamplerParameter::EndFrame) else {
        return shell.child(theme::text("source region unavailable"));
    };
    if !(start >= 0.0 && start < end && end <= frames as f64) {
        return shell.child(theme::text("source region exceeds overview"));
    }
    let facts = format!("{frames} frames · {} hz", overview.sample_rate());
    let table = state.source_overviews.clone();
    let asset_id = asset.id();
    let waveform = div()
        .relative()
        .w(px(WIDTH))
        .h(px(WAVE_HEIGHT))
        .flex_shrink_0()
        .bg(rgb(theme::color(theme::PaletteRole::WaveBackground)))
        .debug_selector(|| "sampler-source-waveform".into())
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    let width = f32::from(bounds.size.width);
                    let height = f32::from(bounds.size.height);
                    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
                        return;
                    }
                    let Some(overview) = table.get(asset_id) else {
                        return;
                    };
                    let extent = overview.source_frames() as f64;
                    for (index, peaks) in overview.peaks().enumerate() {
                        let Some(range) = overview.bin_range(index) else {
                            return;
                        };
                        let left = (range.start as f64 / extent) as f32 * width;
                        let right = (range.end as f64 / extent) as f32 * width;
                        for (channel, peak) in peaks.iter().enumerate() {
                            let half = height * 0.25;
                            let center = half + channel as f32 * height * 0.5;
                            let top = center - peak.max.clamp(-1.0, 1.0) * (half - 1.0).max(0.0);
                            let bottom = center - peak.min.clamp(-1.0, 1.0) * (half - 1.0).max(0.0);
                            window.paint_quad(fill(
                                Bounds::new(
                                    bounds.origin + point(px(left), px(top)),
                                    size(
                                        px((right - left).max(1.0).min((width - left).max(0.0))),
                                        px((bottom - top).max(1.0).min((height - top).max(0.0))),
                                    ),
                                ),
                                rgb(theme::color(theme::PaletteRole::Waveform)),
                            ));
                        }
                    }
                },
            )
            .absolute()
            .inset_0(),
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
    shell
        .child(waveform)
        .child(theme::text(name))
        .child(theme::text(facts))
        .child(theme::text("source overview · read only"))
}
fn source_marker(selector: &'static str, fraction: f64, color: theme::PaletteRole) -> Div {
    div()
        .absolute()
        .left(px(1.0 + (WIDTH - 3.0) * fraction.clamp(0.0, 1.0) as f32))
        .top_0()
        .w(px(1.0))
        .h(px(WAVE_HEIGHT))
        .bg(rgb(theme::color(color)))
        .debug_selector(move || selector.into())
}
