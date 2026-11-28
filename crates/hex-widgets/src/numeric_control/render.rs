//! GPUI rendering, dial/slider/value presentations, and pointer capture for `NumericControl`.
use super::*;

impl NumericControl {
    pub(super) fn dial_element(
        &self,
        size: f32,
        value: DialValue,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let style = self.style;
        let enabled = self.availability.painted_enabled();
        let id = self.id.clone();
        let marker_id = self.id.clone();
        div()
            .id(SharedString::from(format!("{}-knob", self.id)))
            .debug_selector(move || format!("{id}-knob"))
            .relative()
            .size(px(size))
            .on_mouse_down(MouseButton::Left, self.control_mouse_down_listener(cx))
            .cursor(if enabled {
                CursorStyle::PointingHand
            } else {
                CursorStyle::Arrow
            })
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        paint_dial(bounds, size, value, enabled, style, window);
                    },
                )
                .absolute()
                .inset_0()
                .size_full(),
            )
            .when(self.dial_layout == DialLayout::Pan, |dial| {
                dial.child(
                    div()
                        .id(SharedString::from(format!("{}-pan-reset", self.id)))
                        .debug_selector(move || format!("{marker_id}-pan-reset"))
                        .absolute()
                        .top(px(0.0))
                        .left(px(size / 2.0 - 3.0))
                        .w(px(6.0))
                        .h(px(4.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|control, _, window, cx| {
                                cx.stop_propagation();
                                if control.availability.interactive() {
                                    control.focus.focus(window);
                                    control.reset(cx);
                                }
                            }),
                        ),
                )
            })
    }

    pub(super) fn slider_element(
        &self,
        value: f32,
        text: NumericTextElement,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        filled_slider(
            self.id.clone(),
            text,
            SliderPresentation {
                value,
                bipolar: self.spec.scale() == crate::NumericScale::Bipolar,
                center_mark: self.slider_center_mark,
                field: ValueFieldPresentation {
                    invalid: self.invalid_text,
                    editing: self.editing.is_editing(),
                    enabled: self.availability.painted_enabled(),
                    style: self.style,
                    width: px(60.0),
                    height: px(18.0),
                },
            },
            self.value_mouse_down_listener(cx),
        )
    }

    pub(super) fn value_mouse_down_listener(
        &self,
        cx: &mut Context<Self>,
    ) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static {
        cx.listener(Self::value_mouse_down)
    }

    pub(super) fn control_mouse_down_listener(
        &self,
        cx: &mut Context<Self>,
    ) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static {
        cx.listener(Self::control_mouse_down)
    }
}

impl EventEmitter<NumericControlEvent> for NumericControl {}

impl Focusable for NumericControl {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl gpui::Render for NumericControl {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self
            .press
            .is_some_and(|press| matches!(press.target, PressTarget::Value | PressTarget::Fader))
            && !self.focus.is_focused(window)
        {
            self.cancel(cx);
        }
        // A text draft owns printable keys even beneath editor shortcut contexts.
        // Keep numeric editing actions available in the same focused node.
        let key_context = match (self.editing.is_editing(), self.command_fine_steps) {
            (true, true) => "hex_numeric_control hex_text_editing hex_numeric_command_steps",
            (false, true) => "hex_numeric_control hex_numeric_command_steps",
            (true, false) => "hex_numeric_control hex_text_editing",
            (false, false) => "hex_numeric_control",
        };
        let entity = cx.entity();
        let enabled = self.availability.painted_enabled();
        let style = self.style;
        let value = self.spec.to_normalized(self.draft).unwrap_or(0.0) as f32;
        let dial_value = DialValue::new(value, self.spec.scale(), self.dial_layout);
        let label = self.label.clone();
        let value_text = NumericTextElement {
            input: entity.clone(),
        };

        if self.focus_blur.is_none() {
            let focus = self.focus.clone();
            self.focus_blur = Some(cx.on_blur(&focus, window, |control, _, cx| {
                if control.press.is_some() || control.pending_busy_press.is_some() {
                    control.cancel(cx);
                } else {
                    control.finish_text_edit_on_blur(cx);
                }
            }));
        }

        if self.window_activation.is_none() {
            self.window_activation =
                Some(cx.observe_window_activation(window, |control, window, cx| {
                    if !window.is_window_active()
                        && (control.press.is_some_and(|press| {
                            matches!(press.target, PressTarget::Value | PressTarget::Fader)
                        }) || control.pending_busy_press.is_some_and(|press| {
                            matches!(press.target, PressTarget::Value | PressTarget::Fader)
                        }))
                    {
                        control.cancel(cx);
                    }
                }));
        }

        let micro_dial_width = if self.appearance == NumericControlAppearance::WideMicroDial {
            crate::WIDE_MICRO_DIAL_WIDTH
        } else {
            crate::MICRO_DIAL_WIDTH
        };
        let mut root = match (self.kind, self.appearance) {
            (
                NumericControlKind::Knob,
                NumericControlAppearance::MicroDial | NumericControlAppearance::WideMicroDial,
            ) => div()
                .id(self.id.clone())
                .relative()
                .w(px(micro_dial_width))
                .h(px(crate::MICRO_DIAL_HEIGHT))
                .track_focus(&self.focus)
                .key_context(key_context)
                .on_mouse_down(MouseButton::Left, self.control_mouse_down_listener(cx))
                .child(
                    div()
                        .absolute()
                        .debug_selector(|| format!("{}-label", self.id))
                        .top(px(0.0))
                        .left(px(2.0))
                        .w(px(micro_dial_width - 4.0))
                        .h(px(9.0))
                        .text_size(px(8.0))
                        .text_color(rgb(if enabled { style.muted } else { style.border }))
                        .overflow_hidden()
                        .child(label),
                )
                .child(
                    self.dial_element(crate::MICRO_DIAL_DIAMETER, dial_value, cx)
                        .absolute()
                        .top(px(12.0))
                        .left(px(2.0)),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(25.0))
                        .left(px(13.0))
                        .child(value_field_sized(
                            self.id.clone(),
                            value_text,
                            ValueFieldPresentation {
                                invalid: self.invalid_text,
                                editing: self.editing.is_editing(),
                                enabled,
                                style: ControlStyle {
                                    font_size: 8.0,
                                    ..style
                                },
                                width: px(micro_dial_width - 13.0),
                                height: px(12.0),
                            },
                            self.value_mouse_down_listener(cx),
                        )),
                ),
            (NumericControlKind::Knob, NumericControlAppearance::CompactDial) => {
                let (width, height, label_height, label_size, knob_size, value_height, gap) = (
                    COMPACT_DIAL_WIDTH,
                    COMPACT_DIAL_HEIGHT,
                    10.0,
                    9.0,
                    26.0,
                    14.0,
                    1.0,
                );
                div()
                    .id(self.id.clone())
                    .flex()
                    .flex_col()
                    .w(px(width))
                    .h(px(height))
                    .gap(px(gap))
                    .items_center()
                    .track_focus(&self.focus)
                    .key_context(key_context)
                    .child(
                        div()
                            .w(px(width))
                            .h(px(label_height))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(label_size))
                            .text_color(rgb(if enabled { style.muted } else { style.border }))
                            .overflow_hidden()
                            .child(label),
                    )
                    .child(self.dial_element(knob_size, dial_value, cx))
                    .child(value_field_sized(
                        self.id.clone(),
                        value_text,
                        ValueFieldPresentation {
                            invalid: self.invalid_text,
                            editing: self.editing.is_editing(),
                            enabled,
                            style,
                            width: px(width),
                            height: px(value_height),
                        },
                        self.value_mouse_down_listener(cx),
                    ))
            }
            (NumericControlKind::HorizontalSlider, NumericControlAppearance::InlineValue) => div()
                .id(self.id.clone())
                .flex()
                .w(px(self.inline_value_width.unwrap_or(INLINE_VALUE_WIDTH)))
                .h(px(INLINE_VALUE_HEIGHT))
                .items_center()
                .track_focus(&self.focus)
                .key_context(key_context)
                .child(
                    value_field_sized(
                        self.id.clone(),
                        value_text,
                        ValueFieldPresentation {
                            invalid: self.invalid_text,
                            editing: self.editing.is_editing(),
                            enabled,
                            style,
                            width: px(self
                                .inline_value_width
                                .map_or(INLINE_VALUE_WIDTH, |width| width - 2.0)),
                            height: px(INLINE_VALUE_HEIGHT),
                        },
                        self.value_mouse_down_listener(cx),
                    )
                    .rounded_none()
                    .bg(rgb(style.background))
                    .border_color(rgb(if self.invalid_text {
                        style.invalid
                    } else {
                        style.border
                    })),
                ),
            (NumericControlKind::Knob, _) => div()
                .id(self.id.clone())
                .flex()
                .flex_col()
                .w(px(48.0))
                .h(px(67.0))
                .gap(px(1.0))
                .items_center()
                .track_focus(&self.focus)
                .key_context(key_context)
                .child(
                    div()
                        .w(px(48.0))
                        .h(px(14.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(style.font_size))
                        .text_color(rgb(if enabled { style.muted } else { style.border }))
                        .overflow_hidden()
                        .child(label),
                )
                .child(self.dial_element(34.0, dial_value, cx))
                .child(value_field(
                    self.id.clone(),
                    value_text,
                    self.invalid_text,
                    self.editing.is_editing(),
                    enabled,
                    style,
                    self.value_mouse_down_listener(cx),
                )),
            (NumericControlKind::HorizontalSlider, NumericControlAppearance::LabeledSlider) => {
                div()
                    .id(self.id.clone())
                    .flex()
                    .flex_col()
                    .w_full()
                    .min_w(px(60.0))
                    .h(px(32.0))
                    .gap(px(1.0))
                    .items_center()
                    .track_focus(&self.focus)
                    .key_context(key_context)
                    .on_mouse_down(MouseButton::Left, self.value_mouse_down_listener(cx))
                    .child(
                        div()
                            .w_full()
                            .h(px(14.0))
                            .text_center()
                            .overflow_hidden()
                            .text_size(px(style.font_size))
                            .text_color(rgb(if enabled { style.muted } else { style.border }))
                            .child(label),
                    )
                    .child(
                        div()
                            .flex()
                            .w_full()
                            .h(px(14.0))
                            .child(self.slider_element(value, value_text, cx)),
                    )
            }
            (
                NumericControlKind::HorizontalSlider,
                NumericControlAppearance::FilledSlider | NumericControlAppearance::CompactSlider,
            ) => div()
                .id(self.id.clone())
                .flex()
                .w_full()
                .min_w(px(60.0))
                .h(px(
                    if self.appearance == NumericControlAppearance::CompactSlider {
                        21.0
                    } else {
                        18.0
                    },
                ))
                .items_center()
                .track_focus(&self.focus)
                .key_context(key_context)
                .on_mouse_down(MouseButton::Left, self.value_mouse_down_listener(cx))
                .child(self.slider_element(value, value_text, cx)),
            (NumericControlKind::HorizontalSlider, _) => div()
                .id(self.id.clone())
                .flex()
                .h(px(24.0))
                .min_w(px(110.0))
                .gap(px(5.0))
                .items_center()
                .track_focus(&self.focus)
                .key_context(key_context)
                .on_mouse_down(MouseButton::Left, self.value_mouse_down_listener(cx))
                .child(
                    div()
                        .w(px(42.0))
                        .min_w(px(24.0))
                        .overflow_hidden()
                        .text_size(px(style.font_size))
                        .text_color(rgb(if enabled { style.muted } else { style.border }))
                        .child(label),
                )
                .child(self.slider_element(value, value_text, cx)),
            (NumericControlKind::VerticalSlider, NumericControlAppearance::FilledSlider) => div()
                .id(self.id.clone())
                .w(px(12.0))
                .h(px(32.0))
                .track_focus(&self.focus)
                .key_context(key_context)
                .child(self.compact_vertical(value, cx)),
            (NumericControlKind::VerticalSlider, _) => div()
                .id(self.id.clone())
                .flex()
                .flex_col()
                .w(px(60.0))
                .h(px(
                    if self.appearance == NumericControlAppearance::MeterFader {
                        154.0
                    } else {
                        86.0
                    },
                ))
                .gap(px(2.0))
                .items_center()
                .track_focus(&self.focus)
                .key_context(key_context)
                .child(
                    div()
                        .w_full()
                        .h(px(14.0))
                        .overflow_hidden()
                        .text_size(px(style.font_size))
                        .text_color(rgb(if enabled { style.muted } else { style.border }))
                        .text_center()
                        .child(label),
                )
                .child(self.vertical_fader(value, cx))
                .child(value_field(
                    self.id.clone(),
                    value_text,
                    self.invalid_text,
                    self.editing.is_editing(),
                    enabled,
                    style,
                    self.value_mouse_down_listener(cx),
                )),
        };

        let selected = enabled
            && (self.focus.is_focused(window) || self.press.is_some() || self.editing.is_editing());
        root = root
            .relative()
            .debug_selector(|| self.id.to_string())
            // Retain the existing one-pixel layout inset; only its paint changes.
            .border_1()
            .border_color(gpui::rgba(0))
            .when(selected, |root| {
                root.child(corner_markers(&self.id, style.focus_marker))
            })
            .cursor(if enabled {
                CursorStyle::PointingHand
            } else {
                CursorStyle::Arrow
            })
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(|view, _: &NumericLeft, _, cx| {
                cx.stop_propagation();
                view.edit_left(false, cx)
            }))
            .on_action(cx.listener(|view, _: &NumericRight, _, cx| {
                cx.stop_propagation();
                view.edit_right(false, cx)
            }))
            .on_action(cx.listener(|view, _: &NumericSelectLeft, _, cx| {
                cx.stop_propagation();
                view.edit_left(true, cx)
            }))
            .on_action(cx.listener(|view, _: &NumericSelectRight, _, cx| {
                cx.stop_propagation();
                view.edit_right(true, cx)
            }))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::enter))
            .on_action(cx.listener(Self::escape))
            .on_action(cx.listener(|view, _: &NumericUp, _, cx| {
                cx.stop_propagation();
                view.emit_step(1.0, false, cx)
            }))
            .on_action(cx.listener(|view, _: &NumericDown, _, cx| {
                cx.stop_propagation();
                view.emit_step(-1.0, false, cx)
            }))
            .on_action(cx.listener(|view, _: &NumericUpFine, _, cx| {
                cx.stop_propagation();
                view.emit_step(1.0, true, cx)
            }))
            .on_action(cx.listener(|view, _: &NumericDownFine, _, cx| {
                cx.stop_propagation();
                view.emit_step(-1.0, true, cx)
            }));

        let owner = entity.downgrade();
        let descriptor = gpui::AccessibilityDescriptor {
            id: gpui::AccessibilityId(entity.entity_id().as_u64()),
            revision: self
                .accessibility_binding
                .as_ref()
                .map_or(0, |binding| binding.revision),
            lifetime: self.accessibility_lifetime.weak(),
            name: self
                .accessibility_binding
                .as_ref()
                .map_or_else(|| self.label.clone(), |binding| binding.name.clone()),
            value: self
                .accessibility_binding
                .as_ref()
                .map(|binding| binding.value.clone())
                .unwrap_or(gpui::AccessibilityValue::Number {
                    value: self.value,
                    min: self.spec.bounds().0,
                    max: self.spec.bounds().1,
                    text: self.spec.format(self.value).unwrap_or_default().into(),
                }),
            enabled: self.availability.interactive()
                && !self.is_interacting()
                && !self.accessibility_pending
                && self
                    .accessibility_binding
                    .as_ref()
                    .is_none_or(|binding| !binding.pending && (binding.guard)(cx)),
            focused: self.focus.is_focused(window),
            status: if self.is_interacting() {
                Some("local draft or gesture active".into())
            } else if self.availability == crate::ControlAvailability::Busy
                || self.accessibility_pending
                || self
                    .accessibility_binding
                    .as_ref()
                    .is_some_and(|binding| binding.pending)
            {
                Some("pending project edit".into())
            } else {
                None
            },
        };
        root = root.accessibility(descriptor, move |request, window, cx| {
            owner
                .update(cx, |control, cx| {
                    if control
                        .accessibility_binding
                        .as_ref()
                        .is_some_and(|binding| binding.revision != request.revision)
                    {
                        return false;
                    }
                    control.accessibility_action(&request.action, window, cx)
                })
                .unwrap_or(false)
        });
        if self.press.is_some() || self.pending_busy_press.is_some() || self.text_mouse.is_some() {
            root.child(GestureCaptureElement { entity })
        } else {
            root
        }
    }
}

struct GestureCaptureElement {
    entity: Entity<NumericControl>,
}

impl IntoElement for GestureCaptureElement {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for GestureCaptureElement {
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
        // Capture installs window listeners only; it must never enter flex flow
        // or add a gap/shrink the dial while a pointer is held.
        let mut style = Style {
            position: gpui::Position::Absolute,
            ..Style::default()
        };
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
        let control = self.entity.read(cx);
        let hidden = control.press.is_some_and(|press| {
            press.dragging && matches!(press.target, PressTarget::Value | PressTarget::Fader)
        }) && window.is_window_active();
        let cursor =
            if hidden {
                CursorStyle::Hidden
            } else if control.press.is_some_and(|press| {
                matches!(press.target, PressTarget::Control | PressTarget::Fader)
            }) || control.pending_busy_press.is_some_and(|press| {
                matches!(press.target, PressTarget::Control | PressTarget::Fader)
            }) {
                CursorStyle::PointingHand
            } else {
                CursorStyle::Arrow
            };
        window.renew_pointer_capture(self.entity.entity_id(), cursor);
    }
}

fn value_field(
    id: SharedString,
    child: NumericTextElement,
    invalid: bool,
    editing: bool,
    enabled: bool,
    style: ControlStyle,
    on_down: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    value_field_sized(
        id,
        child,
        ValueFieldPresentation {
            invalid,
            editing,
            enabled,
            style,
            width: px(46.0),
            height: px(18.0),
        },
        on_down,
    )
}

#[derive(Clone, Copy)]
struct ValueFieldPresentation {
    invalid: bool,
    editing: bool,
    enabled: bool,
    style: ControlStyle,
    width: Pixels,
    height: Pixels,
}

fn value_field_sized(
    id: SharedString,
    child: NumericTextElement,
    presentation: ValueFieldPresentation,
    on_down: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let ValueFieldPresentation {
        invalid,
        editing,
        enabled,
        style,
        width,
        height,
    } = presentation;
    div()
        .id(SharedString::from(format!("{}-value", id)))
        .debug_selector(|| format!("{}-value", id))
        .flex()
        .w(width)
        .h(height)
        .rounded(px(crate::CONTROL_CORNER_RADIUS))
        .px(px(2.0))
        .items_center()
        .justify_center()
        .border_1()
        .border_color(if invalid {
            rgb(style.invalid)
        } else {
            gpui::rgba(0)
        })
        .when(editing || invalid, |field| field.bg(rgb(style.background)))
        .text_size(px(style.font_size))
        .text_color(rgb(if enabled { style.text } else { style.muted }))
        .cursor(CursorStyle::Arrow)
        .on_mouse_down(MouseButton::Left, on_down)
        .child(child)
}

/// Horizontal filled field; vertical controls use their separate fader geometry.
struct SliderPresentation {
    value: f32,
    bipolar: bool,
    center_mark: bool,
    field: ValueFieldPresentation,
}

fn filled_slider(
    id: SharedString,
    text: NumericTextElement,
    presentation: SliderPresentation,
    on_down: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let SliderPresentation {
        value,
        bipolar,
        center_mark,
        field,
    } = presentation;
    let origin = if bipolar { 0.5 } else { 0.0 };
    let fill_id = format!("{id}-bar-fill");
    let center_id = format!("{id}-bar-center");
    let slider_id = format!("{id}-slider");
    let fill = div()
        .debug_selector(move || fill_id.clone())
        .absolute()
        .left(relative(value.min(origin)))
        .w(relative((value - origin).abs()))
        .h_full()
        .bg(rgb(if field.enabled {
            field.style.numeric_fill
        } else {
            field.style.muted
        }))
        .when(!field.enabled, |fill| fill.opacity(0.35));
    div()
        .debug_selector(move || slider_id.clone())
        .relative()
        .flex()
        .flex_1()
        .min_w(px(46.0))
        // Keep the surrounding row's hit area while thinning the painted field.
        .h(px(14.0))
        .overflow_hidden()
        .bg(rgb(field.style.rail))
        .child(fill)
        .when(bipolar && center_mark, |bar| {
            bar.child(
                div()
                    .debug_selector(move || center_id.clone())
                    .absolute()
                    .left(relative(0.5))
                    .w(px(1.0))
                    .h_full()
                    .bg(rgb(field.style.muted))
                    .opacity(0.6),
            )
        })
        .child(
            value_field_sized(id, text, field, on_down)
                .relative()
                .w_full()
                .h_full(),
        )
        // Paint the frame over the field without shrinking its editable hit area.
        .child(
            div()
                .absolute()
                .inset_0()
                .border_1()
                .border_color(rgb(if field.invalid {
                    field.style.invalid
                } else {
                    field.style.border
                })),
        )
}
