use crate::ControlStyle;
use gpui::{
    App, Bounds, Context, CursorStyle, Element, ElementId, Entity, EventEmitter, GlobalElementId,
    IntoElement, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    Point, Render, SharedString, Style, Window, canvas, div, linear_color_stop, linear_gradient,
    prelude::*, px, rgb, rgba,
};
use std::cell::Cell;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RgbColor {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl RgbColor {
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }

    pub const fn packed(self) -> u32 {
        ((self.red as u32) << 16) | ((self.green as u32) << 8) | self.blue as u32
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HsvColor {
    /// Hue in degrees, normalized to `[0, 360)`.
    pub hue: f64,
    /// Saturation and value in `[0, 100]` percent.
    pub saturation: f64,
    pub value: f64,
}

impl Default for HsvColor {
    fn default() -> Self {
        Self {
            hue: 0.0,
            saturation: 0.0,
            value: 0.0,
        }
    }
}

/// Convert opaque RGB to HSV, retaining `preferred_hue` for achromatic colors.
/// This keeps the hue strip useful while saturation is zero or brightness is black.
pub fn rgb_to_hsv(color: RgbColor, preferred_hue: f64) -> HsvColor {
    let red = f64::from(color.red) / 255.0;
    let green = f64::from(color.green) / 255.0;
    let blue = f64::from(color.blue) / 255.0;
    let max = red.max(green).max(blue);
    let min = red.min(green).min(blue);
    let chroma = max - min;
    let hue = if chroma <= f64::EPSILON {
        normalize_hue(preferred_hue)
    } else if max == red {
        normalize_hue(60.0 * ((green - blue) / chroma).rem_euclid(6.0))
    } else if max == green {
        normalize_hue(60.0 * ((blue - red) / chroma + 2.0))
    } else {
        normalize_hue(60.0 * ((red - green) / chroma + 4.0))
    };
    HsvColor {
        hue,
        saturation: if max <= f64::EPSILON {
            0.0
        } else {
            chroma / max * 100.0
        },
        value: max * 100.0,
    }
}

/// Convert HSV to opaque RGB using rounded 8-bit channels.
pub fn hsv_to_rgb(color: HsvColor) -> Option<RgbColor> {
    if !color.hue.is_finite()
        || !color.saturation.is_finite()
        || !color.value.is_finite()
        || !(0.0..=100.0).contains(&color.saturation)
        || !(0.0..=100.0).contains(&color.value)
    {
        return None;
    }
    let hue = normalize_hue(color.hue);
    let saturation = color.saturation / 100.0;
    let value = color.value / 100.0;
    let chroma = value * saturation;
    let section = hue / 60.0;
    let secondary = chroma * (1.0 - (section.rem_euclid(2.0) - 1.0).abs());
    let (red, green, blue) = match section.floor() as u8 {
        0 => (chroma, secondary, 0.0),
        1 => (secondary, chroma, 0.0),
        2 => (0.0, chroma, secondary),
        3 => (0.0, secondary, chroma),
        4 => (secondary, 0.0, chroma),
        _ => (chroma, 0.0, secondary),
    };
    let match_value = value - chroma;
    Some(RgbColor::new(
        channel_to_u8(red + match_value),
        channel_to_u8(green + match_value),
        channel_to_u8(blue + match_value),
    ))
}

fn channel_to_u8(channel: f64) -> u8 {
    (channel.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn normalize_hue(hue: f64) -> f64 {
    if hue.is_finite() {
        hue.rem_euclid(360.0)
    } else {
        0.0
    }
}

#[derive(Clone, Debug)]
pub struct ColorPickerConfig {
    id: SharedString,
    color: RgbColor,
    hsv: HsvColor,
    style: ControlStyle,
}

impl ColorPickerConfig {
    pub fn new(
        id: impl Into<SharedString>,
        color: RgbColor,
        preferred_hue: f64,
        style: ControlStyle,
    ) -> Result<Self, &'static str> {
        style.validate()?;
        if !preferred_hue.is_finite() {
            return Err("picker hue must be finite");
        }
        Ok(Self {
            id: id.into(),
            color,
            hsv: rgb_to_hsv(color, preferred_hue),
            style,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorPickerEvent {
    pub color: RgbColor,
    pub hsv: HsvColor,
}

#[derive(Clone, Copy, Debug, Default)]
struct Rect {
    left: f32,
    top: f32,
    width: f32,
    height: f32,
}

impl Rect {
    fn from_bounds(bounds: Bounds<Pixels>) -> Self {
        Self {
            left: f32::from(bounds.origin.x),
            top: f32::from(bounds.origin.y),
            width: f32::from(bounds.size.width),
            height: f32::from(bounds.size.height),
        }
    }

    fn clamped_fraction(self, position: Point<Pixels>) -> Option<(f64, f64)> {
        if self.width <= 0.0 || self.height <= 0.0 {
            return None;
        }
        let x = f32::from(position.x);
        let y = f32::from(position.y);
        Some((
            f64::from((x - self.left) / self.width).clamp(0.0, 1.0),
            f64::from((y - self.top) / self.height).clamp(0.0, 1.0),
        ))
    }
}

#[derive(Clone, Copy)]
enum GestureSurface {
    Field,
    Hue,
}

/// Native saturation/value field and hue strip for an opaque RGB color.
/// Held input uses the shared Window pointer lease; callers retain role identity,
/// preview validation and preference/history ownership. Input loss keeps live color.
pub struct ColorPicker {
    id: SharedString,
    color: RgbColor,
    hsv: HsvColor,
    style: ControlStyle,
    field_bounds: std::rc::Rc<Cell<Rect>>,
    hue_bounds: std::rc::Rc<Cell<Rect>>,
    gesture: Option<GestureSurface>,
}

impl ColorPicker {
    pub fn from_config(_: &mut Context<Self>, config: ColorPickerConfig) -> Self {
        Self {
            id: config.id,
            color: config.color,
            hsv: config.hsv,
            style: config.style,
            field_bounds: std::rc::Rc::new(Cell::new(Rect::default())),
            hue_bounds: std::rc::Rc::new(Cell::new(Rect::default())),
            gesture: None,
        }
    }

    pub fn color(&self) -> RgbColor {
        self.color
    }

    pub fn hsv(&self) -> HsvColor {
        self.hsv
    }

    /// Synchronize canonical RGB without emitting, preserving hue for gray/black.
    /// Changed RGB ends old pointer input; same-RGB owner echoes retain the gesture.
    pub fn set_external_color(
        &mut self,
        color: RgbColor,
        preferred_hue: f64,
        cx: &mut Context<Self>,
    ) {
        if self.color != color {
            self.cancel_pointer(cx);
        }
        self.color = color;
        self.hsv = rgb_to_hsv(color, preferred_hue);
        cx.notify();
    }

    /// Synchronize from HSV, emitting one typed RGB/HSV change for the owner.
    pub fn set_hsv(&mut self, hsv: HsvColor, cx: &mut Context<Self>) -> bool {
        let Some(color) = hsv_to_rgb(hsv) else {
            return false;
        };
        let hsv = HsvColor {
            hue: normalize_hue(hsv.hue),
            saturation: hsv.saturation,
            value: hsv.value,
        };
        if self.color == color && self.hsv == hsv {
            return true;
        }
        self.hsv = hsv;
        self.color = color;
        cx.emit(ColorPickerEvent {
            color,
            hsv: self.hsv,
        });
        cx.notify();
        true
    }

    /// End pointer input without reverting already emitted live color. Owners
    /// call this before rebinding a picker to a different role, even if RGB matches.
    pub fn cancel_pointer(&mut self, cx: &mut Context<Self>) {
        if self.gesture.take().is_some() {
            cx.notify();
        }
    }

    fn begin(
        &mut self,
        surface: GestureSurface,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let owner = cx.entity().downgrade();
        let cancelled = owner.clone();
        if !window.capture_pointer(
            cx.entity_id(),
            MouseButton::Left,
            CursorStyle::Arrow,
            move |event, _, cx| {
                let _ = owner.update(cx, |picker, cx| {
                    if let Some(event) = event.downcast_ref::<MouseMoveEvent>() {
                        picker.pointer_move(event, cx);
                    } else if let Some(event) = event.downcast_ref::<MouseUpEvent>() {
                        picker.pointer_up(event, cx);
                    }
                });
            },
            move |_, cx| {
                let _ = cancelled.update(cx, |picker, cx| picker.cancel_pointer(cx));
            },
            cx,
        ) {
            return;
        }
        self.gesture = Some(surface);
        // Held input must reach cached paint even when the selected color does
        // not change (for example pressing the current hue position).
        cx.notify();
        self.move_to(surface, position, cx);
    }

    fn move_to(
        &mut self,
        surface: GestureSurface,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let rect = match surface {
            GestureSurface::Field => self.field_bounds.get(),
            GestureSurface::Hue => self.hue_bounds.get(),
        };
        let Some((x, y)) = rect.clamped_fraction(position) else {
            return;
        };
        let hsv = match surface {
            GestureSurface::Field => HsvColor {
                saturation: x * 100.0,
                value: (1.0 - y) * 100.0,
                ..self.hsv
            },
            GestureSurface::Hue => HsvColor {
                hue: x * 360.0,
                ..self.hsv
            },
        };
        self.set_hsv(hsv, cx);
    }

    fn pointer_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) -> bool {
        let Some(surface) = self.gesture else {
            return false;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.cancel_pointer(cx);
            return false;
        }
        self.move_to(surface, event.position, cx);
        true
    }

    fn pointer_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) -> bool {
        if event.button != MouseButton::Left {
            return false;
        }
        let Some(surface) = self.gesture.take() else {
            return false;
        };
        cx.notify();
        self.move_to(surface, event.position, cx);
        true
    }
}

impl EventEmitter<ColorPickerEvent> for ColorPicker {}

impl Render for ColorPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let style = self.style;
        let hue = self.hsv.hue;
        let saturation = self.hsv.saturation / 100.0;
        let value = self.hsv.value / 100.0;
        let field_bounds = self.field_bounds.clone();
        let hue_bounds = self.hue_bounds.clone();
        let entity = cx.entity();
        let id: SharedString = self.id.clone();
        let field_id: SharedString = format!("{}-field", id).into();
        let hue_id: SharedString = format!("{}-hue", id).into();
        let hue_color = hsv_to_rgb(HsvColor {
            hue,
            saturation: 100.0,
            value: 100.0,
        })
        .unwrap_or_default();
        let selector_x = (saturation * 180.0).clamp(0.0, 180.0);
        let selector_y = ((1.0 - value) * 112.0).clamp(0.0, 112.0);

        div()
            .id(id)
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .id(field_id.clone())
                    .debug_selector(move || field_id.to_string())
                    .relative()
                    .w(px(180.0))
                    .h(px(112.0))
                    .overflow_hidden()
                    .border_1()
                    .border_color(rgb(style.border))
                    .bg(rgb(hue_color.packed()))
                    .child(div().absolute().inset_0().bg(linear_gradient(
                        90.0,
                        linear_color_stop(rgb(0xffffff), 0.0),
                        linear_color_stop(rgba(0xffffff00), 1.0),
                    )))
                    .child(div().absolute().inset_0().bg(linear_gradient(
                        180.0,
                        linear_color_stop(rgba(0x00000000), 0.0),
                        linear_color_stop(rgba(0x000000ff), 1.0),
                    )))
                    .child(
                        div()
                            .absolute()
                            .left(px(selector_x as f32 - 4.0))
                            .top(px(selector_y as f32 - 4.0))
                            .size(px(8.0))
                            .rounded_full()
                            .border_1()
                            .border_color(rgb(0xffffff))
                            .bg(rgba(0x00000033)),
                    )
                    .child(
                        canvas(
                            |_, _, _| (),
                            move |bounds, _, _, _| field_bounds.set(Rect::from_bounds(bounds)),
                        )
                        .absolute()
                        .inset_0()
                        .size_full(),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|picker, event: &MouseDownEvent, window, cx| {
                            picker.begin(GestureSurface::Field, event.position, window, cx);
                        }),
                    ),
            )
            .child(
                div()
                    .id(hue_id.clone())
                    .debug_selector(move || hue_id.to_string())
                    .flex()
                    .relative()
                    .w(px(180.0))
                    .h(px(14.0))
                    .overflow_hidden()
                    .border_1()
                    .border_color(rgb(style.border))
                    .children((0..6).map(|step| {
                        let start = hsv_to_rgb(HsvColor {
                            hue: f64::from(step) * 60.0,
                            saturation: 100.0,
                            value: 100.0,
                        })
                        .unwrap_or_default();
                        let end = hsv_to_rgb(HsvColor {
                            hue: (f64::from(step) + 1.0) * 60.0,
                            saturation: 100.0,
                            value: 100.0,
                        })
                        .unwrap_or_default();
                        div().flex_1().bg(linear_gradient(
                            90.0,
                            linear_color_stop(rgb(start.packed()), 0.0),
                            linear_color_stop(rgb(end.packed()), 1.0),
                        ))
                    }))
                    .child(
                        div()
                            .absolute()
                            .left(px((hue as f32 / 360.0 * 180.0).clamp(0.0, 176.0)))
                            .top(px(0.0))
                            .w(px(4.0))
                            .h(px(14.0))
                            .border_1()
                            .border_color(rgb(0xffffff))
                            .bg(rgba(0x00000033)),
                    )
                    .child(
                        canvas(
                            |_, _, _| (),
                            move |bounds, _, _, _| hue_bounds.set(Rect::from_bounds(bounds)),
                        )
                        .absolute()
                        .inset_0()
                        .size_full(),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|picker, event: &MouseDownEvent, window, cx| {
                            picker.begin(GestureSurface::Hue, event.position, window, cx);
                        }),
                    ),
            )
            .child(PickerCaptureElement { entity })
    }
}

struct PickerCaptureElement {
    entity: Entity<ColorPicker>,
}

impl IntoElement for PickerCaptureElement {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for PickerCaptureElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = px(0.0).into();
        style.size.height = px(0.0).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Window,
        _: &mut App,
    ) -> Self::PrepaintState {
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if self.entity.read(cx).gesture.is_some() {
            window.renew_pointer_capture(self.entity.entity_id(), CursorStyle::Arrow);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_primaries_and_hue_wrap_are_stable() {
        for (hue, expected) in [
            (0.0, RgbColor::new(255, 0, 0)),
            (60.0, RgbColor::new(255, 255, 0)),
            (120.0, RgbColor::new(0, 255, 0)),
            (180.0, RgbColor::new(0, 255, 255)),
            (240.0, RgbColor::new(0, 0, 255)),
            (300.0, RgbColor::new(255, 0, 255)),
            (360.0, RgbColor::new(255, 0, 0)),
            (-60.0, RgbColor::new(255, 0, 255)),
        ] {
            assert_eq!(
                hsv_to_rgb(HsvColor {
                    hue,
                    saturation: 100.0,
                    value: 100.0,
                }),
                Some(expected)
            );
        }
    }

    #[test]
    fn rgb_hsv_roundtrip_does_not_drift_and_gray_keeps_hue() {
        let colors = [
            RgbColor::new(42, 40, 40),
            RgbColor::new(195, 197, 192),
            RgbColor::new(240, 100, 73),
            RgbColor::new(112, 232, 227),
            RgbColor::new(17, 91, 202),
            RgbColor::new(127, 127, 127),
            RgbColor::new(0, 0, 0),
        ];
        for color in colors {
            let hsv = rgb_to_hsv(color, 218.0);
            assert_eq!(hsv_to_rgb(hsv), Some(color));
        }
        assert_eq!(rgb_to_hsv(RgbColor::new(128, 128, 128), 218.0).hue, 218.0);
        assert_eq!(rgb_to_hsv(RgbColor::new(0, 0, 0), 218.0).hue, 218.0);
    }

    #[test]
    fn hsv_rejects_non_finite_and_out_of_range_inputs() {
        for hsv in [
            HsvColor {
                hue: f64::NAN,
                saturation: 20.0,
                value: 30.0,
            },
            HsvColor {
                hue: 0.0,
                saturation: f64::INFINITY,
                value: 30.0,
            },
            HsvColor {
                hue: 0.0,
                saturation: 101.0,
                value: 30.0,
            },
            HsvColor {
                hue: 0.0,
                saturation: 20.0,
                value: -1.0,
            },
        ] {
            assert_eq!(hsv_to_rgb(hsv), None);
        }
    }
}
