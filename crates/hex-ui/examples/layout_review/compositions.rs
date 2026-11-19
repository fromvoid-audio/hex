//! Real production slot compositions, populated with retained view-local controls.
use super::*;
use hex_widgets::{DeviceLayout, device_layout};

impl Gallery {
    pub(super) fn sample(&mut self, _: &Window, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let body = if let Some(fixture) = self.fixtures.get(&self.entry) {
            fixture.sync(self.disabled, cx);
            self.fixture_body(fixture, cx)
        } else {
            device_composition::unsupported_body(theme::text(
                self.error.as_deref().unwrap_or("fixture unavailable"),
            ))
        };
        let body = body.debug_selector(|| "fixture-body".into());
        let (on_color, off_color) = theme::device_enable_colors();
        let actions = hex_widgets::device_state_indicator(
            "fixture-header-bypass".into(),
            !self.bypassed,
            on_color,
            off_color,
        )
        .on_click(cx.listener(|view, _, _, cx| {
            if !view.disabled {
                view.bypassed = !view.bypassed;
                cx.notify();
            }
        }));
        hex_widgets::device_frame(
            hex_widgets::vertical_device_name(theme::text(self.entry.label())),
            theme::device_frame_style(self.selected),
            actions,
            div(),
            body,
        )
        .id(gpui::ElementId::Name(
            format!("catalog-card-{}", self.entry.key()).into(),
        ))
        .debug_selector(|| "catalog-card".into())
        .track_focus(&self.header_focus)
        .w(px(
            self.entry.width() + hex_widgets::DEVICE_FRAME_HEADER_WIDTH
        ))
        .h(px(device_composition::FRAME_HEIGHT))
        .flex_shrink_0()
        .capture_any_mouse_down(cx.listener(
            |view, event: &gpui::MouseDownEvent, _, cx| {
                if event.button == gpui::MouseButton::Left && !view.disabled {
                    view.selected = true;
                    cx.notify();
                }
            },
        ))
    }
    fn fixture_body(&self, fixture: &Fixture, cx: &mut Context<Self>) -> Div {
        // Track/Return are ownership labels around the exact same effect body.
        // No per-owner geometry, widget state or style branch is allowed here.
        match self.entry {
            Entry::Vector => self.vector_body(fixture, cx),
            Entry::Utility => {
                let input = device_composition::rail_column()
                    .w(px(64.0))
                    .h_full()
                    .justify_between()
                    .child(hex_widgets::device_caption(
                        theme::text("input"),
                        theme::device_style(theme::DeviceStyleRole::Control),
                    ))
                    .child(
                        device_layout(DeviceLayout::Row)
                            .gap(px(2.0))
                            .child(fixture.toggle(0, 31.0))
                            .child(fixture.toggle(1, 31.0)),
                    )
                    .child(fixture.selector(0, 64.0))
                    .child(fixture.number(0))
                    .child(fixture.toggle(2, 64.0))
                    .child(fixture.toggle(3, 64.0))
                    .child(fixture.number(3));
                let output = device_composition::rail_column()
                    .w(px(64.0))
                    .h_full()
                    .justify_between()
                    .child(hex_widgets::device_caption(
                        theme::text("output"),
                        theme::device_style(theme::DeviceStyleRole::Control),
                    ))
                    .child(fixture.number(1))
                    .child(fixture.number(2))
                    .child(
                        device_layout(DeviceLayout::Row)
                            .gap(px(2.0))
                            .child(fixture.toggle(4, 31.0))
                            .child(fixture.toggle(5, 31.0)),
                    );
                device_composition::utility(input, output)
            }
            Entry::Saturator => device_composition::stacked_plot(
                fixture.selector(0, 120.0),
                graph(fixture),
                caption("normalized fixture curve"),
                device_composition::control_row()
                    .h(px(52.0))
                    .flex_shrink_0()
                    .justify_between()
                    .child(fixture.number(0))
                    .child(fixture.number(1))
                    .child(fixture.number(2)),
            ),
            Entry::Eq => eq::body(fixture),
            Entry::Eq8 => {
                let mut bands = device_composition::rail_column()
                    .w(px(96.0))
                    .h_full()
                    .gap(px(1.0))
                    .child(caption("bands"));
                for index in 0..8 {
                    bands = bands.child(
                        hex_widgets::device_compact_row(
                            theme::device_style(theme::DeviceStyleRole::Layout),
                            index == self.active_tab,
                        )
                        .w(px(92.0))
                        .h(px(17.0))
                        .child(
                            hex_widgets::device_inline_caption(
                                theme::text((index + 1).to_string()),
                                theme::device_style(theme::DeviceStyleRole::Control),
                            )
                            .w(px(10.0)),
                        )
                        .child(fixture.toggle(index, 18.0))
                        .child(fixture.selector(index, 64.0)),
                    );
                }
                let toolbar = device_layout(DeviceLayout::Row)
                    .h(px(16.0))
                    .items_center()
                    .child(fixture.selector(8, 64.0));
                let mut controls = device_composition::control_row().h(px(40.0)).gap(px(2.0));
                for index in 0..4 {
                    controls = controls.child(fixture.number(index));
                }
                device_composition::split_body(
                    device_composition::compact_rail(bands)
                        .w(px(96.0))
                        .flex_shrink_0(),
                    device_composition::plot_column(toolbar, graph(fixture), controls),
                )
            }
            Entry::Compressor => {
                let rail = device_composition::dense_rail(
                    device_composition::rail_column()
                        .w(px(64.0))
                        .child(fixture.number(0))
                        .child(fixture.number(1))
                        .child(fixture.number(2)),
                )
                .w(px(64.0))
                .flex_shrink_0();
                let toolbar = device_layout(DeviceLayout::Row)
                    .h(px(14.0))
                    .justify_between()
                    .child(fixture.number(3))
                    .child(fixture.number(4));
                device_composition::split_body(
                    rail,
                    device_composition::plot_column(
                        toolbar,
                        graph(fixture),
                        device_layout(DeviceLayout::Row)
                            .h(px(14.0))
                            .child(fixture.number(5)),
                    ),
                )
            }
            Entry::Gate => device_composition::paired_rails(
                device_composition::rail_column().children((0..3).map(|i| fixture.number(i))),
                device_composition::rail_column().children((3..6).map(|i| fixture.number(i))),
            ),
            Entry::AutoPan => device_composition::centered_rails(device_composition::dense_rail(
                device_composition::rail_column().children((0..3).map(|i| fixture.number(i))),
            )),
            Entry::Instrument | Entry::Effect => {
                div().w_full().h(px(device_composition::BODY_HEIGHT)).child(
                    device_composition::wrapped_body(
                        (0..3)
                            .map(|i| fixture.number(i))
                            .chain([fixture.toggle(0, 136.0), fixture.selector(0, 136.0)]),
                    )
                    .id("fixture-generic-body")
                    .overflow_y_scroll(),
                )
            }
            Entry::Unsupported => device_composition::unsupported_body(theme::text(
                "unsupported fixture state · shared recoverable display",
            )),
            Entry::Parts => {
                let rail = device_composition::dense_rail(
                    device_composition::rail_column()
                        .w(px(64.0))
                        .children((0..3).map(|i| fixture.number(i))),
                )
                .w(px(64.0))
                .flex_shrink_0();
                device_composition::split_body(
                    rail,
                    device_composition::plot_column(
                        fixture.selector(0, 64.0),
                        graph(fixture),
                        fixture.toggle(0, 40.0),
                    ),
                )
            }
        }
    }
    fn vector_body(&self, fixture: &Fixture, cx: &mut Context<Self>) -> Div {
        let style = theme::device_style(theme::DeviceStyleRole::Layout);
        let mut shell = hex_widgets::device_rail(style)
            .w(px(device_composition::VECTOR_SHELL_WIDTH))
            .h_full()
            .flex_shrink_0()
            .pr(px(4.0));
        let mut preview = hex_widgets::device_rail(style)
            .w(px(device_composition::VECTOR_PREVIEW_WIDTH))
            .h_full()
            .flex_shrink_0()
            .pl(px(2.0));
        for row in 0..4 {
            let operator = 3 - row;
            shell = shell.child(
                hex_widgets::device_grouped_row(
                    style,
                    !self.disabled && self.section == Some(operator),
                )
                .id(("fixture-shell-row", operator))
                .debug_selector(move || format!("fixture-shell-row-{operator}"))
                .capture_any_mouse_down(cx.listener(
                    move |view, event: &gpui::MouseDownEvent, _, cx| {
                        if event.button == gpui::MouseButton::Left && !view.disabled {
                            view.section = Some(operator);
                            cx.notify();
                        }
                    },
                ))
                .h(px(40.0))
                .justify_between()
                .child(fixture.number(operator * 3))
                .child(fixture.number(operator * 3 + 1))
                .child(fixture.toggle(operator, 18.0))
                .child(fixture.number(operator * 3 + 2))
                .child(hex_widgets::device_badge(
                    theme::text(["a", "b", "c", "d"][operator]),
                    theme::device_identity_style(operator, !self.disabled, false),
                )),
            );
            preview = preview.child(
                hex_widgets::device_grouped_row(
                    style,
                    !self.disabled && self.section == Some(4 + row),
                )
                .id(("fixture-preview-row", row))
                .debug_selector(move || format!("fixture-preview-row-{row}"))
                .capture_any_mouse_down(cx.listener(
                    move |view, event: &gpui::MouseDownEvent, _, cx| {
                        if event.button == gpui::MouseButton::Left && !view.disabled {
                            view.section = Some(4 + row);
                            cx.notify();
                        }
                    },
                ))
                .h(px(40.0))
                .justify_between()
                .child(
                    div()
                        .w(px(44.0))
                        .flex_shrink_0()
                        .child(caption(["lfo", "filter", "pitch env", "global"][row])),
                )
                .children((0..3).map(|column| fixture.number(12 + row * 3 + column))),
            );
        }
        let tabs = self.tabs(cx);
        let detail = device_composition::rail_column()
            .h(px(160.0))
            .child(div().h(px(86.0)).flex_shrink_0().child(graph(fixture)))
            .child(div().h(px(16.0)).flex_shrink_0())
            .child(caption(if self.active_tab == 0 {
                "envelope · fixture"
            } else {
                "oscillator · fixture"
            }))
            .child(
                device_layout(DeviceLayout::Row)
                    .child(fixture.number(24))
                    .child(fixture.number(25)),
            );
        device_composition::three_pane(
            shell,
            device_composition::tabbed_detail(
                detail,
                tabs,
                device_composition::VECTOR_DETAIL_WIDTH,
            ),
            preview,
        )
    }
    fn tabs(&self, cx: &mut Context<Self>) -> Div {
        let mut tabs = device_layout(DeviceLayout::Row)
            .w_full()
            .h(px(16.0))
            .justify_between();
        for (index, label) in ["envelope", "oscillator"].into_iter().enumerate() {
            tabs = tabs.child(
                hex_widgets::panel_tab(
                    theme::text(label),
                    self.active_tab == index,
                    theme::device_style(theme::DeviceStyleRole::Display),
                )
                .id(label)
                .debug_selector(move || label.into())
                .track_focus(&self.page_focus[index])
                .w(px(90.0))
                .h(px(16.0))
                .cursor_pointer()
                .on_click(cx.listener(move |view, _, window, cx| {
                    if !view.disabled && view.finish_drafts(cx) {
                        view.active_tab = index;
                        view.page_focus[index].focus(window);
                        cx.notify();
                    }
                }))
                .on_key_down(cx.listener(
                    move |view, event: &KeyDownEvent, window, cx| {
                        if view.disabled {
                            return;
                        }
                        match event.keystroke.key.as_str() {
                            "enter" | "space" if view.finish_drafts(cx) => view.active_tab = index,
                            "left" | "right" => view.page_focus[1 - index].focus(window),
                            _ => return,
                        }
                        cx.stop_propagation();
                        cx.notify();
                    },
                )),
            );
        }
        tabs
    }
}
fn caption(label: &str) -> Div {
    hex_widgets::device_caption(
        theme::text(label),
        theme::device_style(theme::DeviceStyleRole::Control),
    )
}
fn graph(fixture: &Fixture) -> Div {
    div()
        .w_full()
        .h_full()
        .min_h_0()
        .child(fixture.graph.clone())
}
