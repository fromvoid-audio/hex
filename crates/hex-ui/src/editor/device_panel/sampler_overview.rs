//! Read-only, peak-only original-source overview. No PCM or filesystem access.
use super::*;
use gpui::relative;
use std::rc::Rc;

const RULER_HEIGHT: f32 = 14.0;

pub(super) fn render(view: &EditorView, device: &hex_project::Device, name: &str) -> Div {
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
    let facts = format!(
        "{frames} frames · {} hz · {:.3} s",
        overview.sample_rate(),
        frames as f64 / f64::from(overview.sample_rate())
    );
    let table = state.source_overviews.clone();
    let asset_id = asset.id();
    let waveform = div()
        .relative()
        .w_full()
        .flex_1()
        .min_h_0()
        .bg(rgb(theme::color(theme::PaletteRole::WaveBackground)))
        .debug_selector(|| "sampler-source-waveform".into())
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
    let waveform = waveform.child(
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
            .child(theme::text(facts)),
    );
    shell.child(waveform)
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
