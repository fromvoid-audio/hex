//! Compact one-knob drive feedback using shared host bindings. The eleven
//! diamonds are an original visual target display, not a measured DSP response.
//! Gesture/recording/pending overrides win over the selected authored envelope
//! at the display playhead, then the ordinary base value. Public project queries
//! own interpolation; this panel neither schedules audio nor reads processor state.
use super::{
    ControlPresentation, ControlText, DeviceControlIdentity, EditorView, ParameterKey,
    effective_parameter_value, numeric_display::NumericDisplayCodec,
    parameter_control_with_presentation,
};
use crate::theme;
use gpui::{
    Context, Div, ElementId, PathBuilder, Stateful, canvas, div, point, prelude::*, px, rgb,
};
use hex_engine::{NativeDeviceClass, NativeDeviceView};
use hex_lattice_contract::{ParameterId, ParameterValue};
use hex_project::{BeatTime, Device};

pub(super) const FRAME_WIDTH: f32 = crate::device_composition::RATTLE_FRAME_WIDTH;
const DRIVE: ParameterId = ParameterId::from_static(1);
const DIAMONDS: usize = 11;

#[derive(Clone, Copy)]
pub(super) struct PreviewCache {
    drive: f32,
    radii: [f32; DIAMONDS],
}

fn radii(drive: f32) -> Option<[f32; DIAMONDS]> {
    if !drive.is_finite() || !(0.0..=1.0).contains(&drive) {
        return None;
    }
    // Fixed positive inner radius; increasing drive packs the outer rings
    // inward, tightening every adjacent spacing without reversing their order.
    Some(std::array::from_fn(|index| {
        0.06 + (index + 1) as f32 / DIAMONDS as f32 * (0.90 - 0.60 * drive)
    }))
}

fn authored_target(
    device: &Device,
    beat: Option<BeatTime>,
    base: ParameterValue,
) -> ParameterValue {
    beat.and_then(|beat| {
        device
            .automation_lane(DRIVE)
            .filter(|lane| lane.is_reading())
            .and_then(|lane| lane.value_at(beat))
    })
    .map_or(base, |value| ParameterValue::Float(value.get()))
}

fn preview(view: &mut EditorView, device: &Device, base: ParameterValue) -> Option<PreviewCache> {
    let beat = view.state.as_ref().and_then(|state| {
        state
            .project
            .tempo_map()
            .frame_to_beat(view.audible_cursor_position())
            .ok()
    });
    let value = effective_parameter_value(
        view,
        ParameterKey {
            device: device.id(),
            parameter: DRIVE,
        },
        authored_target(device, beat, base),
    );
    let ParameterValue::Float(drive) = value else {
        return None;
    };
    if let Some(cached) = view.device_panel.rattle_previews.get(&device.id())
        && cached.drive == drive
    {
        return Some(*cached);
    }
    let preview = PreviewCache {
        drive,
        radii: radii(drive)?,
    };
    view.device_panel
        .rattle_previews
        .insert(device.id(), preview);
    Some(preview)
}

pub(super) fn render_body(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: &Device,
    device_view: &NativeDeviceView,
) -> Stateful<Div> {
    let id = device.id();
    let parameter = device_view
        .parameters
        .iter()
        .find(|p| p.descriptor.id == DRIVE);
    let preview = parameter.and_then(|p| preview(view, device, p.value));
    let mut body = crate::device_composition::body()
        .flex_col()
        .gap(px(3.0))
        .p(px(4.0));
    if let (Some(parameter), Some(preview)) = (parameter, preview) {
        let color = theme::color(theme::PaletteRole::WarmAccent);
        let display = div()
            .debug_selector(move || format!("rattle-diamonds-{}", id.value()))
            .w_full()
            .flex_1()
            .min_h_0()
            .relative()
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        let width = f32::from(bounds.size.width);
                        let height = f32::from(bounds.size.height);
                        let extent = width.min(height) * 0.5 - 3.0;
                        if !extent.is_finite() || extent <= 0.0 {
                            return;
                        }
                        let center = bounds.center();
                        let mut path = PathBuilder::stroke(px(1.0));
                        for radius in preview.radii {
                            let radius = px(radius * extent);
                            path.move_to(point(center.x, center.y - radius));
                            path.line_to(point(center.x + radius, center.y));
                            path.line_to(point(center.x, center.y + radius));
                            path.line_to(point(center.x - radius, center.y));
                            path.close();
                        }
                        if let Ok(path) = path.build() {
                            window.paint_path(path, rgb(color));
                        }
                    },
                )
                .absolute()
                .inset_0(),
            );
        let drive = parameter_control_with_presentation(
            view,
            cx,
            id,
            DeviceControlIdentity {
                class: NativeDeviceClass::Effect,
                type_id: device_view.type_id,
            },
            parameter,
            ControlPresentation::CompactDial,
            ControlText {
                label: Some("drive"),
                codec: Some(NumericDisplayCodec::Percent),
            },
        );
        body = body
            .child(crate::device_composition::plot(display))
            .child(div().flex().justify_center().flex_shrink_0().child(drive));
    } else {
        view.device_panel.rattle_previews.remove(&id);
        body = body.child(theme::text("rattle drive unavailable"));
    }
    body.id(ElementId::Name(
        format!("device-body-{}", id.value()).into(),
    ))
    .debug_selector(move || format!("device-body-{}", id.value()))
}

#[cfg(all(test, feature = "ui-test-support"))]
pub(super) fn displayed(
    view: &EditorView,
    device: hex_project::DeviceId,
) -> Option<(f32, [f32; DIAMONDS])> {
    let preview = view.device_panel.rattle_previews.get(&device)?;
    Some((preview.drive, preview.radii))
}

#[cfg(all(test, not(feature = "ui-test-support")))]
mod tests {
    use super::*;
    #[test]
    fn diamond_radii_stay_finite_ordered_and_tighten_at_full_drive() {
        let low = radii(0.0).unwrap();
        let high = radii(1.0).unwrap();
        for values in [low, high] {
            assert_eq!(values.len(), 11);
            assert!(values.iter().all(|v| v.is_finite() && *v > 0.0 && *v < 1.0));
            assert!(values.windows(2).all(|p| p[0] < p[1]));
        }
        assert!(
            low.windows(2)
                .zip(high.windows(2))
                .all(|(a, b)| a[1] - a[0] > b[1] - b[0])
        );
        for invalid in [f32::NAN, f32::INFINITY, -0.1, 1.1] {
            assert!(radii(invalid).is_none());
        }
    }
}
