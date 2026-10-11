use crate::theme;
use gpui::{
    AnyElement, AppContext, Context, Div, FocusHandle, KeyDownEvent, Render, Window, div,
    prelude::*, px, rgb,
};
use hex_widgets::{ControlStyle, compact_device_frame, device_frame, panel_tab};

const BACKGROUND: u32 = 0x474747;
const DARK: u32 = 0x363636;
const DISPLAY: u32 = 0x191919;
const LIGHT: u32 = 0x414141;
const TEXT: u32 = 0xbcbcbc;
const SELECTED: u32 = 0xaedce7;
const ACCENT: u32 = 0x00bfd0;
const SAMPLE_HEIGHT: f32 = 116.0;
const TARGET_IDS: [&str; 3] = ["target-0", "target-1", "target-2"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Family {
    Device,
    CompactDevice,
    Display,
    Rail,
    Rows,
    Tabs,
    Sections,
}
const FAMILIES: [Family; 7] = [
    Family::Device,
    Family::CompactDevice,
    Family::Display,
    Family::Rail,
    Family::Rows,
    Family::Tabs,
    Family::Sections,
];
impl Family {
    fn label(self) -> &'static str {
        match self {
            Self::Device => "device frame",
            Self::CompactDevice => "compact device frame",
            Self::Display => "rounded dark display",
            Self::Rail => "section rail",
            Self::Rows => "grouped rows",
            Self::Tabs => "stable tabs",
            Self::Sections => "compact sections",
        }
    }
    fn target_count(self) -> usize {
        if matches!(self, Self::Device | Self::CompactDevice | Self::Display) {
            1
        } else {
            3
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Idle,
    Focused,
    Selected,
    Disabled,
}
const STATES: [State; 4] = [
    State::Idle,
    State::Focused,
    State::Selected,
    State::Disabled,
];
impl State {
    fn selector(self) -> &'static str {
        match self {
            Self::Idle => "preview-idle",
            Self::Focused => "preview-focused",
            Self::Selected => "preview-selected",
            Self::Disabled => "preview-disabled",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Focused => "focused",
            Self::Selected => "selected",
            Self::Disabled => "disabled",
        }
    }
}
fn style(state: State, header: bool) -> ControlStyle {
    ControlStyle {
        background: BACKGROUND,
        surface: DARK,
        raised: if header && state == State::Selected {
            SELECTED
        } else {
            BACKGROUND
        },
        rail: DISPLAY,
        border: if header && state == State::Focused {
            TEXT
        } else {
            BACKGROUND
        },
        text: if state == State::Disabled {
            0x888888
        } else if header && state == State::Selected {
            0x101010
        } else {
            TEXT
        },
        muted: 0x888888,
        accent: ACCENT,
        hover: LIGHT,
        font_size: 10.0,
    }
}

pub(super) struct Gallery {
    family: Family,
    selected: Option<usize>,
    disabled: bool,
    focus: [FocusHandle; 3],
}
impl Gallery {
    pub(super) fn new(cx: &mut Context<Self>) -> Self {
        Self {
            family: Family::Device,
            selected: None,
            disabled: false,
            focus: std::array::from_fn(|_| cx.focus_handle()),
        }
    }
    pub(super) fn focus_first(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus[0].focus(window, cx);
    }
    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self
            .focus
            .iter()
            .position(|handle| handle.is_focused(window))
        else {
            return;
        };
        let count = self.family.target_count();
        match event.keystroke.key.as_str() {
            "tab" | "right" | "down" => {
                let backwards = event.keystroke.key == "tab" && event.keystroke.modifiers.shift;
                let next = if backwards {
                    (index + count - 1) % count
                } else {
                    (index + 1) % count
                };
                self.focus[next].focus(window, cx);
            }
            "left" | "up" => self.focus[(index + count - 1) % count].focus(window, cx),
            "enter" | "space" if !self.disabled => self.selected = Some(index),
            "escape" => self.selected = None,
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
    fn target(
        &self,
        element: Div,
        index: usize,
        live: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !live {
            return element.into_any_element();
        }
        let focused = self.focus[index].is_focused(window);
        element
            .id(TARGET_IDS[index])
            .debug_selector(move || TARGET_IDS[index].into())
            .track_focus(&self.focus[index])
            .border_1()
            .when(focused, |element| element.border_color(rgb(TEXT)))
            .cursor_pointer()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |view, _, window, cx| {
                    if !view.disabled {
                        view.focus[index].focus(window, cx);
                        view.selected = Some(index);
                        cx.notify();
                    }
                    cx.stop_propagation();
                }),
            )
            .into_any_element()
    }
    fn sample(&self, state: State, live: bool, window: &Window, cx: &mut Context<Self>) -> Div {
        let state_at = |i| {
            if !live {
                if i == 0 || state == State::Disabled {
                    state
                } else {
                    State::Idle
                }
            } else if self.disabled {
                State::Disabled
            } else if self.selected == Some(i) {
                State::Selected
            } else if self.focus[i].is_focused(window) {
                State::Focused
            } else {
                State::Idle
            }
        };
        let mut body = div().flex().flex_col().size_full().gap(px(2.0));
        match self.family {
            Family::Device | Family::CompactDevice => {
                let current = state_at(0);
                let frame_style = style(current, true);
                let contents = div()
                    .h(px(92.0))
                    .p(px(6.0))
                    .child(theme::text("shared frame · no audio"));
                let frame = if self.family == Family::Device {
                    device_frame(theme::text("device header"), frame_style, div(), contents)
                } else {
                    compact_device_frame(theme::text("device header"), frame_style, div(), contents)
                };
                // Transparent overlay owns only the header hitbox; body clicks do not select.
                let header_height = if self.family == Family::Device {
                    hex_widgets::DEVICE_FRAME_HEADER_HEIGHT
                } else {
                    hex_widgets::COMPACT_DEVICE_FRAME_HEADER_HEIGHT
                };
                let hit = div()
                    .border_1()
                    .border_color(gpui::rgba(0))
                    .absolute()
                    .top(px(0.0))
                    .left(px(0.0))
                    .w_full()
                    .h(px(header_height + 2.0));
                body = body
                    .relative()
                    .child(frame)
                    .child(self.target(hit, 0, live, window, cx));
            }
            Family::Display => {
                let current = state_at(0);
                let panel =
                    div()
                        .size_full()
                        .rounded(px(hex_widgets::DEVICE_CORNER_RADIUS))
                        .bg(rgb(DISPLAY))
                        .border_1()
                        .border_color(rgb(if current == State::Focused {
                            TEXT
                        } else if current == State::Selected {
                            ACCENT
                        } else {
                            DISPLAY
                        }))
                        .p(px(6.0))
                        .child(theme::text("display surface"))
                        .child(div().h(px(1.0)).mt(px(22.0)).bg(rgb(
                            if current == State::Disabled {
                                0x555555
                            } else {
                                ACCENT
                            },
                        )))
                        .child(theme::text("static guide · no signal"));
                body = body.child(self.target(panel, 0, live, window, cx));
            }
            Family::Rail | Family::Rows | Family::Sections => {
                for (i, label) in ["section one", "section two", "section three"]
                    .into_iter()
                    .enumerate()
                {
                    let current = state_at(i);
                    let row = div()
                        .h(px(34.0))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .px(px(4.0))
                        .gap(px(6.0))
                        .bg(rgb(if current == State::Selected {
                            BACKGROUND
                        } else if current == State::Focused {
                            LIGHT
                        } else {
                            DARK
                        }))
                        .border_1()
                        .border_color(rgb(if current == State::Focused {
                            TEXT
                        } else {
                            BACKGROUND
                        }))
                        .text_color(rgb(style(current, false).text))
                        .child(theme::text(label));
                    let row = if self.family == Family::Sections {
                        row.child(div().flex_1()).child(theme::text("a / b"))
                    } else if self.family == Family::Rows {
                        row.child(div().flex_1()).child(theme::text("value"))
                    } else {
                        row
                    };
                    body = body.child(self.target(row, i, live, window, cx));
                }
            }
            Family::Tabs => {
                let mut tabs = div().flex().gap(px(2.0));
                for (i, label) in ["page a", "page b", "page c"].into_iter().enumerate() {
                    let current = state_at(i);
                    let tab = panel_tab(
                        theme::text(label),
                        current == State::Selected,
                        style(current, false),
                    )
                    .w(px(76.0))
                    .border_1()
                    .border_color(rgb(if current == State::Focused {
                        TEXT
                    } else {
                        BACKGROUND
                    }));
                    tabs = tabs.child(self.target(tab, i, live, window, cx));
                }
                body = body.child(tabs).child(
                    div()
                        .flex_1()
                        .bg(rgb(DISPLAY))
                        .p(px(5.0))
                        .child(theme::text("fixed page footprint")),
                );
            }
        }
        body
    }
}
impl Render for Gallery {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut navigation = div()
            .w(px(178.0))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(4.0));
        for family in FAMILIES {
            navigation = navigation.child(
                div()
                    .id(family.label())
                    .debug_selector(move || family.label().into())
                    .h(px(24.0))
                    .px(px(4.0))
                    .flex()
                    .items_center()
                    .bg(rgb(if self.family == family { LIGHT } else { DARK }))
                    .cursor_pointer()
                    .child(theme::text(family.label()))
                    .on_click(cx.listener(move |view, _, window, cx| {
                        view.family = family;
                        view.selected = None;
                        view.disabled = false;
                        view.focus_first(window, cx);
                        cx.notify();
                    })),
            );
        }
        let live = self.sample(State::Idle, true, window, cx);
        let mut states = div().flex().flex_wrap().gap(px(8.0));
        for state in STATES {
            states = states.child(
                div()
                    .w(px(248.0))
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(theme::text(state.label()))
                    .child(
                        div()
                            .h(px(SAMPLE_HEIGHT))
                            .debug_selector(move || state.selector().into())
                            .child(self.sample(state, false, window, cx)),
                    ),
            );
        }
        div().id("layout-review").size_full().flex().flex_col().p(px(8.0)).gap(px(8.0))
            .bg(rgb(BACKGROUND)).font_family(theme::FONT_FAMILY).text_size(px(10.0)).text_color(rgb(TEXT))
            .on_key_down(cx.listener(Self::key_down))
            .child(div().pl(px(theme::TITLEBAR_LEADING_INSET)).h(px(22.0)).child(theme::text("hex · native layout review")))
            .child(div().flex().gap(px(10.0)).child(navigation).child(div().flex_1().min_w_0().flex().flex_col().gap(px(8.0))
                .child(theme::text(format!("{} · interactive sample", self.family.label())))
                .child(div().w(px(300.0)).h(px(SAMPLE_HEIGHT)).debug_selector(|| "live-sample".into()).child(live))
                .child(div().id("disable-sample").debug_selector(|| "disable-sample".into()).h(px(22.0)).cursor_pointer().child(theme::text(if self.disabled { "enable sample" } else { "disable sample" }))
                    .on_click(cx.listener(|view, _, window, cx| { view.disabled = !view.disabled; view.focus_first(window, cx); cx.notify(); })))
                .child(theme::text("click selects · tab/arrows focus · enter/space selects · escape clears"))
                .child(theme::text("reference state previews · first target varies; disabled applies to all"))
                .child(states)))
            .child(theme::text("bounded catalog · reference appearance / inferred keyboard behavior · no audio"))
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;
    use gpui::{Empty, Entity, Modifiers, TestAppContext, VisualTestContext, size};

    fn open(cx: &mut TestAppContext) -> (gpui::AnyWindowHandle, Entity<Gallery>) {
        let bootstrap = cx.add_window(|_, _| Empty);
        let handle = *bootstrap;
        let view = cx
            .update_window(handle, |_, window, cx| {
                let view = window.replace_root(cx, |_, cx| Gallery::new(cx));
                view.update(cx, |view, cx| view.focus_first(window, cx));
                view
            })
            .expect("gallery window");
        (handle, view)
    }

    #[gpui::test]
    fn pointer_keyboard_selection_and_disabled_input_preserve_geometry(cx: &mut TestAppContext) {
        let (handle, view) = open(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        for family in FAMILIES {
            visual.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.family = family;
                    view.selected = None;
                    view.disabled = false;
                    view.focus_first(window, cx);
                    cx.notify();
                })
            });
            visual.run_until_parked();
            let navigation = visual
                .debug_bounds(family.label())
                .expect("family navigation");
            visual.simulate_click(navigation.center(), Modifiers::default());
            view.read_with(cx, |view, _| {
                assert_eq!(view.family, family);
                assert_eq!(view.selected, None);
            });
            let before = visual.debug_bounds("live-sample").expect("live sample");
            for index in 0..family.target_count() {
                let name = TARGET_IDS[index];
                let bounds = visual.debug_bounds(name).expect("target");
                assert!(bounds.origin.x >= before.origin.x && bounds.origin.y >= before.origin.y);
                assert!(bounds.origin.x + bounds.size.width <= before.origin.x + before.size.width);
                assert!(
                    bounds.origin.y + bounds.size.height <= before.origin.y + before.size.height
                );
                visual.simulate_click(bounds.center(), Modifiers::default());
                view.read_with(cx, |view, _| assert_eq!(view.selected, Some(index)));
                visual.simulate_keystrokes("escape");
                view.read_with(cx, |view, _| assert_eq!(view.selected, None));
                visual.simulate_keystrokes("enter");
                view.read_with(cx, |view, _| assert_eq!(view.selected, Some(index)));
                visual.simulate_keystrokes("tab space");
                let next = (index + 1) % family.target_count();
                view.read_with(cx, |view, _| assert_eq!(view.selected, Some(next)));
                visual.simulate_keystrokes("shift-tab enter");
                view.read_with(cx, |view, _| assert_eq!(view.selected, Some(index)));
                visual.simulate_keystrokes("right enter");
                view.read_with(cx, |view, _| assert_eq!(view.selected, Some(next)));
                visual.simulate_keystrokes("left enter");
                view.read_with(cx, |view, _| assert_eq!(view.selected, Some(index)));
                visual.run_until_parked();
                assert_eq!(visual.debug_bounds(name).expect("stable target"), bounds);
            }
            let disable = visual
                .debug_bounds("disable-sample")
                .expect("disable control");
            visual.simulate_click(disable.center(), Modifiers::default());
            let selected = view.read_with(cx, |view, _| view.selected);
            visual.simulate_keystrokes("enter space");
            let target = visual.debug_bounds("target-0").expect("disabled target");
            visual.simulate_click(target.center(), Modifiers::default());
            view.read_with(cx, |view, _| assert_eq!(view.selected, selected));
            visual.run_until_parked();
            assert_eq!(
                visual.debug_bounds("live-sample").expect("stable sample"),
                before
            );
        }
    }

    #[gpui::test]
    fn headers_only_select_and_catalog_stays_inside_supported_windows(cx: &mut TestAppContext) {
        let (handle, view) = open(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        for width in [760.0, 1320.0] {
            visual.simulate_resize(size(px(width), px(660.0)));
            for family in FAMILIES {
                visual.update(|window, cx| {
                    view.update(cx, |view, cx| {
                        view.family = family;
                        view.selected = None;
                        view.focus_first(window, cx);
                        cx.notify();
                    })
                });
                visual.run_until_parked();
                let mut previews = Vec::new();
                for state in STATES {
                    let bounds = visual.debug_bounds(state.selector()).expect("preview");
                    assert!(bounds.origin.x >= px(0.0) && bounds.origin.y >= px(0.0));
                    assert!(bounds.origin.x + bounds.size.width <= px(width));
                    assert!(bounds.origin.y + bounds.size.height <= px(660.0));
                    for previous in &previews {
                        assert!(!bounds.intersects(previous), "preview overlap");
                    }
                    previews.push(bounds);
                }
                if matches!(family, Family::Device | Family::CompactDevice) {
                    let sample = visual.debug_bounds("live-sample").expect("sample");
                    visual.simulate_click(sample.center(), Modifiers::default());
                    view.read_with(cx, |view, _| {
                        assert_eq!(view.selected, None, "body must not select header")
                    });
                    let header = visual.debug_bounds("target-0").expect("header");
                    visual.simulate_click(header.center(), Modifiers::default());
                    view.read_with(cx, |view, _| assert_eq!(view.selected, Some(0)));
                }
            }
        }
    }
}
