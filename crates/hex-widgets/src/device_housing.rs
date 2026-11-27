//! Shared device-housing display state, horizontal macro/modulator panels,
//! and uniform square slot grids (`docs/design/device-host-brief.md`).
//! State lasts while the caller's stable device identity is continuously
//! rendered. Both header and body share that scope.
use crate::{
    ActionStyle, ControlStyle, DEVICE_CORNER_RADIUS, DEVICE_FRAME_BORDER_WIDTH,
    DEVICE_HEADER_ACTION_SIZE, DeviceFrameStyle, toggle_action,
};
use gpui::{
    AnyElement, App, Bounds, Context, Div, Entity, FocusHandle, IntoElement, MouseButton,
    PathBuilder, Pixels, Render, RenderOnce, SharedString, Window, canvas, div, point, prelude::*,
    px, rgb,
};

/// Canonical height of a shared device host in pixels (`device-host-brief.md`).
pub const DEVICE_HOST_PANEL_HEIGHT: f32 = 200.0;
/// Canonical width of the optional right-end nested-device widget strip inside a device frame.
pub const DEVICE_NESTED_SLOT_WIDTH: f32 = 16.0;
const HOST_PANEL_PADDING: f32 = 4.0;
const HOST_PANEL_GAP: f32 = 3.0;
const HOST_PANEL_HEADER_HEIGHT: f32 = 16.0;
const HOST_SLOT_MIN_SIZE: f32 = 24.0;

/// Uniform square slot grid geometry for a macro or modulator host panel.
///
/// Sizing follows `docs/design/device-host-brief.md`:
/// - 0..=2 assigned slots: 2-high single column
/// - 3 assigned slots: 3-high single column
/// - 4+ assigned slots: 4-high, adding columns of 4 at the same square size
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HostSlotGridLayout {
    pub rows: usize,
    pub columns: usize,
    pub slot_size: f32,
    pub gap: f32,
    pub padding: f32,
    pub header_height: f32,
    pub panel_width: f32,
}

impl HostSlotGridLayout {
    /// Compute uniform square slot geometry for `slot_count` assigned slots
    /// inside an interior panel height of `available_height` pixels.
    pub fn compute(slot_count: usize, available_height: f32) -> Self {
        let rows = match slot_count {
            0..=2 => 2,
            3 => 3,
            _ => 4,
        };
        let columns = slot_count.max(1).div_ceil(rows);
        let padding = HOST_PANEL_PADDING;
        let gap = HOST_PANEL_GAP;
        let header_height = HOST_PANEL_HEADER_HEIGHT;
        let safe_height = if available_height.is_finite() {
            available_height.max(64.0)
        } else {
            DEVICE_HOST_PANEL_HEIGHT - 2.0 * DEVICE_FRAME_BORDER_WIDTH
        };
        let grid_height = (safe_height - 2.0 * padding - header_height - gap).max(32.0);
        let slot_size = ((grid_height - gap * rows.saturating_sub(1) as f32) / rows as f32)
            .floor()
            .max(HOST_SLOT_MIN_SIZE);
        let panel_width =
            2.0 * padding + (columns as f32) * slot_size + (columns.saturating_sub(1) as f32) * gap;
        Self {
            rows,
            columns,
            slot_size,
            gap,
            padding,
            header_height,
            panel_width,
        }
    }
}

/// Visual modulation depth ring painted around a slot knob or source gauge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HostSlotModulationRing {
    /// Base normalized position in `0.0..=1.0`.
    pub base: f32,
    /// Signed modulation sweep in `-1.0..=1.0` applied from `base`.
    pub delta: f32,
    /// Color-coded ring stroke RGB (`0xRRGGBB`).
    pub color: u32,
}

/// Deliberate preview graphic rendered inside a square slot without overlapping labels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HostSlotPreview {
    /// Macro dial indicator with normalized position (`0.0..=1.0`).
    MacroDial(f32),
    /// Periodic or shaped modulator waveform preview.
    Waveform(HostSlotWaveform),
    /// Bipolar or unipolar level gauge (`0.0..=1.0`).
    Level(f32),
    /// Discrete toggle/button indicator.
    Toggle(bool),
}

/// Waveform glyph class for modulator slot previews.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostSlotWaveform {
    Sine,
    Triangle,
    Ramp,
    Square,
    Random,
    Envelope,
    Steps,
}

/// One color-coded square slot rendered in a horizontal macro or modulator panel.
pub struct HostSlotItem {
    pub id: SharedString,
    pub label: SharedString,
    pub value: SharedString,
    pub accent_color: u32,
    pub enabled: bool,
    pub armed: bool,
    pub unsupported: bool,
    pub preview: HostSlotPreview,
    pub modulation_ring: Option<HostSlotModulationRing>,
    pub interactive: Option<AnyElement>,
}

/// Configuration for a uniform horizontal macro or modulator slot panel.
pub struct HostSlotPanelConfig {
    pub id: &'static str,
    pub title: SharedString,
    pub empty_label: SharedString,
    pub slots: Vec<HostSlotItem>,
    pub header_action: Option<AnyElement>,
    pub detail: Option<AnyElement>,
    pub style: ControlStyle,
    pub available_height: f32,
}

impl HostSlotPanelConfig {
    pub fn new(
        id: &'static str,
        title: impl Into<SharedString>,
        empty_label: impl Into<SharedString>,
        style: ControlStyle,
    ) -> Self {
        Self {
            id,
            title: title.into(),
            empty_label: empty_label.into(),
            slots: Vec::new(),
            header_action: None,
            detail: None,
            style,
            available_height: DEVICE_HOST_PANEL_HEIGHT - 2.0 * DEVICE_FRAME_BORDER_WIDTH,
        }
    }
}

/// Render a uniform square-slot panel for macros or modulators.
///
/// Every slot in the grid receives the exact same square footprint determined
/// by `HostSlotGridLayout::compute`. Labels, previews/rings, and value readouts
/// sit in non-overlapping vertical bands tuned to 2-high, 3-high, and 4-high sizes.
pub fn host_slot_panel(config: HostSlotPanelConfig) -> Div {
    let layout = HostSlotGridLayout::compute(config.slots.len(), config.available_height);
    let style = config.style;
    let panel_id = config.id;
    let has_detail = config.detail.is_some();

    let header = div()
        .h(px(layout.header_height))
        .w_full()
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_between()
        .gap(px(3.0))
        .child(
            div()
                .text_size(px(8.0))
                .text_color(rgb(style.muted))
                .overflow_hidden()
                .child(style.text_case.present(config.title.as_ref())),
        )
        .children(config.header_action);

    let mut grid = div()
        .debug_selector(move || format!("{panel_id}-grid"))
        .flex()
        .flex_row()
        .flex_shrink_0()
        .gap(px(layout.gap));

    if config.slots.is_empty() {
        grid = grid.child(
            div().flex().flex_col().gap(px(layout.gap)).child(
                div()
                    .debug_selector(move || format!("{panel_id}-empty-slot"))
                    .w(px(layout.slot_size))
                    .h(px(layout.slot_size))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .p(px(3.0))
                    .rounded(px(DEVICE_CORNER_RADIUS - 1.0))
                    .border_1()
                    .border_color(rgb(style.border))
                    .bg(rgb(style.surface))
                    .text_size(px(8.0))
                    .text_color(rgb(style.muted))
                    .overflow_hidden()
                    .child(style.text_case.present(config.empty_label.as_ref())),
            ),
        );
    } else {
        let mut iter = config.slots.into_iter();
        for _ in 0..layout.columns {
            let mut column = div().flex().flex_col().flex_shrink_0().gap(px(layout.gap));
            for _ in 0..layout.rows {
                let Some(slot) = iter.next() else {
                    break;
                };
                column = column.child(render_slot(slot, layout, style));
            }
            grid = grid.child(column);
        }
    }

    let slots_column = div()
        .w(px(layout.panel_width))
        .h_full()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .p(px(layout.padding))
        .gap(px(layout.gap))
        .child(header)
        .child(grid);

    let mut container = div()
        .h_full()
        .flex()
        .flex_row()
        .flex_shrink_0()
        .bg(rgb(style.rail))
        .text_color(rgb(style.text))
        .text_size(px(style.font_size))
        .child(slots_column);

    if let Some(detail) = config.detail {
        container = container.child(
            div()
                .h_full()
                .flex()
                .flex_col()
                .flex_shrink_0()
                .border_l_1()
                .border_color(rgb(style.border))
                .p(px(layout.padding))
                .child(detail),
        );
    }
    if !has_detail {
        container = container.w(px(layout.panel_width));
    }
    container
}

fn render_slot(slot: HostSlotItem, layout: HostSlotGridLayout, style: ControlStyle) -> Div {
    let size = layout.slot_size;
    let slot_id = slot.id.clone();
    let ring_id = slot.id.clone();
    let armed_id = slot.id.clone();
    let unsupported_id = slot.id.clone();
    let active = slot.enabled && !slot.unsupported;
    let border_color = if (slot.armed && !slot.unsupported) || active {
        slot.accent_color
    } else {
        style.border
    };
    let label_size = if layout.rows >= 4 { 7.0 } else { 8.0 };
    let value_size = if layout.rows >= 4 { 7.0 } else { 8.0 };
    let label_height = if layout.rows >= 4 { 9.0 } else { 11.0 };
    let value_height = if layout.rows >= 4 { 9.0 } else { 11.0 };
    let graphic_size = (size - label_height - value_height - 8.0).clamp(12.0, 42.0);
    let preview = slot.preview;
    let ring = if slot.unsupported {
        None
    } else {
        slot.modulation_ring
    };
    let accent = slot.accent_color;
    let enabled = active;

    let graphic = div()
        .debug_selector(move || format!("host-slot-ring-{}", ring_id.as_ref()))
        .w(px(graphic_size))
        .h(px(graphic_size))
        .flex_shrink_0()
        .child(
            canvas(
                move |_, _, _| (),
                move |bounds, _, window, _| {
                    paint_slot_graphic(bounds, preview, ring, accent, enabled, style, window);
                },
            )
            .size_full(),
        );

    let mut card = div()
        .debug_selector(move || format!("host-slot-{}", slot_id.as_ref()))
        .relative()
        .w(px(size))
        .h(px(size))
        .flex()
        .flex_col()
        .flex_shrink_0()
        .items_center()
        .justify_between()
        .px(px(2.0))
        .py(px(2.0))
        .rounded(px(DEVICE_CORNER_RADIUS - 1.0))
        .border_1()
        .border_color(rgb(border_color))
        .bg(rgb(if slot.armed && !slot.unsupported {
            style.raised
        } else {
            style.surface
        }))
        .overflow_hidden()
        .child(
            div()
                .w_full()
                .h(px(label_height))
                .flex()
                .items_center()
                .justify_center()
                .overflow_hidden()
                .text_size(px(label_size))
                .text_color(rgb(if active { style.text } else { style.muted }))
                .child(style.text_case.present(slot.label.as_ref())),
        )
        .child(graphic)
        .child(
            div()
                .w_full()
                .h(px(value_height))
                .flex()
                .items_center()
                .justify_center()
                .overflow_hidden()
                .text_size(px(value_size))
                .text_color(rgb(if active { style.text } else { style.muted }))
                .child(style.text_case.present(slot.value.as_ref())),
        );
    if slot.armed && !slot.unsupported {
        card = card.child(
            div()
                .debug_selector(move || format!("host-slot-armed-{}", armed_id.as_ref()))
                .absolute()
                .top(px(2.0))
                .right(px(2.0))
                .size(px(4.0))
                .rounded_full()
                .bg(rgb(accent)),
        );
    }
    if slot.unsupported {
        card = card.child(
            div()
                .debug_selector(move || {
                    format!("host-slot-unsupported-{}", unsupported_id.as_ref())
                })
                .absolute()
                .top(px(2.0))
                .right(px(2.0))
                .size(px(4.0))
                .rounded_full()
                .bg(rgb(style.muted)),
        );
    }
    if let Some(interactive) = slot.interactive {
        card = card.child(div().absolute().inset_0().child(interactive));
    }
    card
}

fn paint_slot_graphic(
    bounds: Bounds<Pixels>,
    preview: HostSlotPreview,
    ring: Option<HostSlotModulationRing>,
    accent: u32,
    enabled: bool,
    style: ControlStyle,
    window: &mut Window,
) {
    let w: f32 = bounds.size.width.into();
    let h: f32 = bounds.size.height.into();
    let diameter = w.min(h);
    if diameter < 6.0 {
        return;
    }
    let center = bounds.origin + point(px(w * 0.5), px(h * 0.5));
    let outer_radius = (diameter * 0.5 - 1.5).max(3.0);
    let start_angle = 3.0 * std::f32::consts::FRAC_PI_4;
    let sweep = 3.0 * std::f32::consts::FRAC_PI_2;

    let stroke_arc =
        |start: f32, end: f32, radius: f32, width: f32, color: u32, win: &mut Window| {
            if (end - start).abs() < 1e-4 {
                return;
            }
            let polar =
                |angle: f32| center + point(px(angle.cos() * radius), px(angle.sin() * radius));
            let mut path = PathBuilder::stroke(px(width));
            path.move_to(polar(start));
            for step in 1..=28 {
                let t = step as f32 / 28.0;
                path.line_to(polar(start + (end - start) * t));
            }
            if let Ok(built) = path.build() {
                win.paint_path(built, rgb(color));
            }
        };

    // Modulation ring sits on the outer perimeter when present so it never
    // collides with the inner dial/waveform preview or slot text bands.
    if let Some(ring) = ring {
        let base = ring.base.clamp(0.0, 1.0);
        let modulated = (base + ring.delta).clamp(0.0, 1.0);
        let a = start_angle + base.min(modulated) * sweep;
        let b = start_angle + base.max(modulated) * sweep;
        stroke_arc(
            start_angle,
            start_angle + sweep,
            outer_radius,
            1.2,
            style.dial_rail,
            window,
        );
        if enabled {
            stroke_arc(a, b, outer_radius, 2.0, ring.color, window);
        }
    }

    let inner_radius = if ring.is_some() {
        (outer_radius - 3.0).max(2.5)
    } else {
        outer_radius
    };

    match preview {
        HostSlotPreview::MacroDial(position) | HostSlotPreview::Level(position) => {
            let pos = position.clamp(0.0, 1.0);
            stroke_arc(
                start_angle,
                start_angle + sweep,
                inner_radius,
                1.6,
                if enabled {
                    style.dial_rail
                } else {
                    style.muted
                },
                window,
            );
            if enabled {
                stroke_arc(
                    start_angle,
                    start_angle + pos * sweep,
                    inner_radius,
                    1.8,
                    accent,
                    window,
                );
            }
        }
        HostSlotPreview::Toggle(on) => {
            stroke_arc(
                start_angle,
                start_angle + sweep,
                inner_radius,
                1.8,
                if enabled && on {
                    accent
                } else {
                    style.dial_rail
                },
                window,
            );
        }
        HostSlotPreview::Waveform(shape) => {
            let half_w = (inner_radius * 0.9).max(3.0);
            let half_h = (inner_radius * 0.6).max(2.0);
            let mut path = PathBuilder::stroke(px(1.4));
            let steps = 16usize;
            for i in 0..=steps {
                let t = i as f32 / steps as f32;
                let y =
                    match shape {
                        HostSlotWaveform::Sine => -(t * std::f32::consts::TAU).sin(),
                        HostSlotWaveform::Triangle => {
                            if t < 0.25 {
                                -4.0 * t
                            } else if t < 0.75 {
                                -2.0 + 4.0 * t
                            } else {
                                4.0 - 4.0 * t
                            }
                        }
                        HostSlotWaveform::Ramp => 1.0 - 2.0 * t,
                        HostSlotWaveform::Square => {
                            if t < 0.5 {
                                -1.0
                            } else {
                                1.0
                            }
                        }
                        HostSlotWaveform::Random => [0.6, -0.7, 0.2, 0.9, -0.4, 0.5, -0.8, 0.1]
                            [(i * 8 / (steps + 1)).min(7)],
                        HostSlotWaveform::Envelope => {
                            if t < 0.2 {
                                1.0 - 10.0 * t
                            } else if t < 0.5 {
                                -1.0 + 2.0 * (t - 0.2) / 0.3
                            } else {
                                0.0 + 2.0 * (t - 0.5)
                            }
                        }
                        HostSlotWaveform::Steps => [0.8, 0.2, -0.5, -0.9, -0.2, 0.4, 0.9, -0.3]
                            [(i * 8 / (steps + 1)).min(7)],
                    };
                let pt = center
                    + point(
                        px((t * 2.0 - 1.0) * half_w),
                        px(y.clamp(-1.0, 1.0) * half_h),
                    );
                if i == 0 {
                    path.move_to(pt);
                } else {
                    path.line_to(pt);
                }
            }
            if let Ok(built) = path.build() {
                window.paint_path(built, rgb(if enabled { accent } else { style.muted }));
            }
        }
    }
}

struct Panels {
    visible: [bool; 2],
    focus: [FocusHandle; 2],
}
fn state(window: &mut Window, cx: &mut App) -> Entity<Panels> {
    window.use_keyed_state("device-housing-panels", cx, |_, cx| Panels {
        visible: [false; 2],
        focus: [
            cx.focus_handle().tab_stop(true).tab_index(0),
            cx.focus_handle().tab_stop(true).tab_index(0),
        ],
    })
}

#[derive(IntoElement)]
pub(super) struct Housing {
    pub body: Option<AnyElement>,
    pub macro_panel: Option<AnyElement>,
    pub modulator_panel: Option<AnyElement>,
    pub nested_slot: Option<AnyElement>,
    pub title: AnyElement,
    pub leading: AnyElement,
    pub actions: AnyElement,
    pub style: DeviceFrameStyle,
}
impl RenderOnce for Housing {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = state(window, cx);
        let header = crate::device_header(self.title, self.style, self.leading, self.actions);
        let row = div().w_full().flex().flex_1().min_w_0().min_h_0().h_full();
        let Some(body) = self.body else {
            return row.child(header);
        };
        let style = self.style.controls;
        let mut controls = div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .items_center()
            .gap(px(crate::DEVICE_FRAME_HEADER_GAP));
        for (index, (id, glyph, label)) in [
            (
                "device-macro-toggle",
                "m",
                if self.macro_panel.is_some() {
                    "macros"
                } else {
                    "macros · empty"
                },
            ),
            (
                "device-modulator-toggle",
                "~",
                if self.modulator_panel.is_some() {
                    "modulators"
                } else {
                    "modulators · empty"
                },
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let checked = state.read(cx).visible[index];
            let focus = state.read(cx).focus[index].clone();
            let other_focus = state.read(cx).focus[1 - index].clone();
            let click_state = state.clone();
            let key_state = state.clone();
            let background = if checked { style.accent } else { style.raised };
            let text = if checked {
                style.accent_text
            } else {
                style.text
            };
            controls = controls.child(
                toggle_action(
                    glyph.into(),
                    ActionStyle {
                        background,
                        text,
                        hover_background: background,
                        hover_text: Some(text),
                    },
                )
                .id(id)
                .debug_selector(move || id.into())
                .size(px(DEVICE_HEADER_ACTION_SIZE))
                .flex_shrink_0()
                .text_size(px(8.0))
                .track_focus(&focus)
                .border_1()
                .border_color(rgb(background))
                .focus(move |button| button.border_color(rgb(style.focus_marker)))
                .tooltip(move |_, cx| cx.new(|_| PanelTooltip(label, style)).into())
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    click_state.update(cx, |panels, cx| {
                        panels.focus[index].focus(window);
                        panels.visible[index] = !panels.visible[index];
                        cx.notify();
                    });
                })
                .on_key_down(move |event, window, cx| {
                    if event.keystroke.key == "tab"
                        && !event.keystroke.modifiers.control
                        && !event.keystroke.modifiers.alt
                        && !event.keystroke.modifiers.platform
                    {
                        if event.keystroke.modifiers.shift {
                            window.focus_prev();
                        } else {
                            window.focus_next();
                        }
                        cx.stop_propagation();
                    } else if matches!(event.keystroke.key.as_str(), "up" | "down")
                        && !event.keystroke.modifiers.control
                        && !event.keystroke.modifiers.alt
                        && !event.keystroke.modifiers.platform
                    {
                        other_focus.focus(window);
                        cx.stop_propagation();
                    }
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        cx.stop_propagation();
                        if !event.is_held {
                            key_state.update(cx, |panels, cx| {
                                panels.visible[index] = !panels.visible[index];
                                cx.notify();
                            });
                        }
                    }
                }),
            );
        }
        let visible = state.read(cx).visible;
        let mut row = row.child(header.child(controls));
        if visible[0] {
            let panel = self.macro_panel.unwrap_or_else(|| {
                host_slot_panel(HostSlotPanelConfig::new(
                    "device-macro-slots",
                    "macros",
                    "no macros",
                    style,
                ))
                .into_any_element()
            });
            row = row.child(
                div()
                    .debug_selector(|| "device-macro-panel".into())
                    .h_full()
                    .flex()
                    .flex_shrink_0()
                    .border_r_1()
                    .border_color(rgb(style.border))
                    .bg(rgb(style.rail))
                    .overflow_hidden()
                    .child(panel),
            );
        }
        if visible[1] {
            let panel = self.modulator_panel.unwrap_or_else(|| {
                host_slot_panel(HostSlotPanelConfig::new(
                    "device-modulator-slots",
                    "modulators",
                    "no modulators",
                    style,
                ))
                .into_any_element()
            });
            row = row.child(
                div()
                    .debug_selector(|| "device-modulator-panel".into())
                    .h_full()
                    .flex()
                    .flex_shrink_0()
                    .border_r_1()
                    .border_color(rgb(style.border))
                    .bg(rgb(style.rail))
                    .overflow_hidden()
                    .child(panel),
            );
        }
        let mut row = row.child(
            div()
                .id("device-housing-content")
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .h_full()
                .overflow_y_scroll()
                .child(
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .flex_shrink_0()
                        .min_h_full()
                        .child(body),
                ),
        );
        if let Some(nested_slot) = self.nested_slot {
            row = row.child(
                div()
                    .debug_selector(|| "device-nested-slot".into())
                    .w(px(DEVICE_NESTED_SLOT_WIDTH))
                    .h_full()
                    .flex()
                    .flex_col()
                    .flex_shrink_0()
                    .border_l_1()
                    .border_color(rgb(style.border))
                    .bg(rgb(style.rail))
                    .overflow_hidden()
                    .child(nested_slot),
            );
        }
        row
    }
}

struct PanelTooltip(&'static str, ControlStyle);
impl Render for PanelTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .p(px(4.0))
            .bg(rgb(self.1.raised))
            .text_color(rgb(self.1.text))
            .text_size(px(self.1.font_size))
            .child(self.1.text_case.present(self.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_grid_layout_uses_uniform_2_3_and_4_high_square_progression() {
        let h = DEVICE_HOST_PANEL_HEIGHT - 2.0 * DEVICE_FRAME_BORDER_WIDTH;
        let zero = HostSlotGridLayout::compute(0, h);
        let one = HostSlotGridLayout::compute(1, h);
        let two = HostSlotGridLayout::compute(2, h);
        assert_eq!((zero.rows, zero.columns), (2, 1));
        assert_eq!(one, two);
        assert_eq!((one.rows, one.columns), (2, 1));

        let three = HostSlotGridLayout::compute(3, h);
        assert_eq!((three.rows, three.columns), (3, 1));
        assert!(three.slot_size < two.slot_size);

        let four = HostSlotGridLayout::compute(4, h);
        let five = HostSlotGridLayout::compute(5, h);
        let eight = HostSlotGridLayout::compute(8, h);
        let nine = HostSlotGridLayout::compute(9, h);
        assert_eq!((four.rows, four.columns), (4, 1));
        assert_eq!((five.rows, five.columns), (4, 2));
        assert_eq!((eight.rows, eight.columns), (4, 2));
        assert_eq!((nine.rows, nine.columns), (4, 3));
        assert!(four.slot_size < three.slot_size);
        // Never mix slot sizes when adding a second or third column at 4-high.
        assert_eq!(four.slot_size, five.slot_size);
        assert_eq!(four.slot_size, eight.slot_size);
        assert_eq!(four.slot_size, nine.slot_size);
        assert!(five.panel_width > four.panel_width);
        assert!(nine.panel_width > eight.panel_width);
    }
}
