//! Fixture-owned candidates composed inside a neutral shared host vocabulary.
use super::*;

impl Gallery {
    pub(super) fn composition(
        &self,
        state_at: &impl Fn(usize) -> State,
        live: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let header_state = state_at(0);
        let header = div()
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .right(px(0.0))
            .h(px(20.0))
            .rounded_tl(px(3.0))
            .rounded_tr(px(3.0))
            .border_1()
            .border_color(gpui::rgba(0))
            .bg(rgb(if header_state == State::Selected {
                SELECTED
            } else {
                BACKGROUND
            }))
            .text_color(rgb(style(header_state, true).text))
            .px(px(4.0))
            .child(theme::text("device"));
        let mut host = div()
            .size_full()
            .relative()
            .rounded(px(hex_widgets::DEVICE_CORNER_RADIUS))
            .bg(rgb(BACKGROUND))
            .border_1()
            .border_color(rgb(BACKGROUND))
            .child(self.target(header, 0, live, window, cx));
        let section = |index, label: &str| {
            let state = state_at(index);
            div()
                .rounded(px(hex_widgets::DEVICE_CORNER_RADIUS))
                .border_1()
                .border_color(rgb(BACKGROUND))
                .bg(rgb(if matches!(state, State::Focused | State::Selected) {
                    BACKGROUND
                } else {
                    DARK
                }))
                .text_color(rgb(style(state, false).text))
                .p(px(3.0))
                .child(theme::text(label))
        };
        match self.family {
            Family::Inspector => {
                let left = section(1, "section a")
                    .absolute()
                    .top(px(22.0))
                    .left(px(3.0))
                    .w(px(68.0))
                    .bottom(px(3.0));
                let right = section(2, "section b")
                    .absolute()
                    .top(px(22.0))
                    .right(px(3.0))
                    .w(px(68.0))
                    .bottom(px(3.0));
                host = host
                    .child(self.target(left, 1, live, window, cx))
                    .child(self.target(right, 2, live, window, cx));
                // The light surround is painted after the neighboring sections and
                // header, so it pushes both aside without changing their geometry.
                let screen = div()
                    .absolute()
                    .top(px(3.0))
                    .bottom(px(2.0))
                    .left(px(69.0))
                    .right(px(69.0))
                    .rounded(px(5.0))
                    .bg(rgb(BACKGROUND))
                    .p(px(3.0))
                    .when(live, |panel| {
                        panel.debug_selector(|| "inspector-surround".into())
                    })
                    .child(
                        div()
                            .size_full()
                            .rounded(px(3.0))
                            .bg(rgb(DISPLAY))
                            .p(px(3.0))
                            .when(live, |panel| {
                                panel.debug_selector(|| "inspector-screen".into())
                            })
                            .child(theme::text("inspector"))
                            .child(div().h(px(1.0)).mt(px(24.0)).bg(rgb(ACCENT)))
                            .child(theme::text(match self.selected {
                                Some(1) => "section a",
                                Some(2) => "section b",
                                _ => "selection",
                            })),
                    );
                host = host.child(screen.id("inspector-shield").occlude().on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|_, _, _, cx| cx.stop_propagation()),
                ));
            }
            Family::RaisedTabs => {
                let active = if live {
                    self.active_tab
                } else if state_at(2) == State::Selected {
                    2
                } else {
                    1
                };
                let mut tabs = div()
                    .absolute()
                    .top(px(3.0))
                    .left(px(70.0))
                    .right(px(3.0))
                    .h(px(20.0))
                    .flex()
                    .gap(px(2.0));
                for (index, label) in [(1, "page a"), (2, "page b")] {
                    let active_page = index == active;
                    let tab = div()
                        .w(px(68.0))
                        .h(px(20.0))
                        .border_1()
                        .border_color(gpui::rgba(0))
                        .child(
                            div()
                                .h(px(if active_page { 20.0 } else { 14.0 }))
                                .mt(px(if active_page { 0.0 } else { 4.0 }))
                                .rounded_tl(px(if active_page { 4.0 } else { 2.0 }))
                                .rounded_tr(px(if active_page { 4.0 } else { 2.0 }))
                                .bg(rgb(if active_page { DISPLAY } else { DARK }))
                                .text_color(rgb(style(state_at(index), false).text))
                                .px(px(3.0))
                                .child(theme::text(label)),
                        );
                    tabs = tabs.child(self.target(tab, index, live, window, cx));
                }
                let screen = div()
                    .absolute()
                    .top(px(21.0))
                    .left(px(3.0))
                    .right(px(3.0))
                    .bottom(px(3.0))
                    .rounded(px(3.0))
                    .bg(rgb(DISPLAY))
                    .p(px(6.0))
                    .when(live, |panel| {
                        panel.debug_selector(|| "tabbed-screen".into())
                    })
                    .child(theme::text(if active == 1 {
                        "page a · screen"
                    } else {
                        "page b · screen"
                    }))
                    .child(div().h(px(1.0)).mt(px(28.0)).bg(rgb(ACCENT)));
                host = host.child(screen).child(tabs);
            }
            Family::SplitScreen => {
                let upper = div()
                    .absolute()
                    .top(px(22.0))
                    .left(px(3.0))
                    .right(px(3.0))
                    .h(px(49.0))
                    .rounded_tl(px(4.0))
                    .rounded_tr(px(4.0))
                    .bg(rgb(DISPLAY))
                    .border_1()
                    .border_color(rgb(DISPLAY))
                    .p(px(4.0))
                    .child(theme::text("screen"));
                let lower = section(2, "controls")
                    .border_color(rgb(
                        if matches!(state_at(2), State::Focused | State::Selected) {
                            BACKGROUND
                        } else {
                            DARK
                        },
                    ))
                    .absolute()
                    .top(px(71.0))
                    .left(px(3.0))
                    .right(px(3.0))
                    .bottom(px(3.0))
                    .rounded_tl(px(0.0))
                    .rounded_tr(px(0.0));
                host = host
                    .child(self.target(upper, 1, live, window, cx))
                    .child(self.target(lower, 2, live, window, cx));
            }
            Family::SplitSections => {
                let upper = section(1, "long section")
                    .absolute()
                    .top(px(22.0))
                    .left(px(3.0))
                    .right(px(3.0))
                    .h(px(59.0));
                let lower = section(2, "short section")
                    .absolute()
                    .top(px(83.0))
                    .left(px(3.0))
                    .right(px(3.0))
                    .bottom(px(3.0));
                host = host
                    .child(self.target(upper, 1, live, window, cx))
                    .child(self.target(lower, 2, live, window, cx));
            }
            _ => {}
        }
        host
    }
}
